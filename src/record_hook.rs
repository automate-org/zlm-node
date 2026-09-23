// src/record_hook.rs
use anyhow::{Context, Result};
use axum::{Json, Router, extract::State, routing::post};
use log::{error, info, warn};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct HookState {
    pub mgr_base: String,
    pub internal_api_token: String,
    pub http_client: reqwest::Client,
    pub keep_on_failure: bool,
    /// zlm-node 的 server_id（= ZLM 的 mediaServerId）
    pub server_id: String,
    /// 是否启用 S3 上传（关闭时只上报 mgr，本地保留）
    pub s3_enable: bool,
}

pub fn router(state: Arc<HookState>) -> Router {
    Router::new()
        .route("/hook/on_record_mp4", post(handle_record_mp4))
        .route("/hook/health", axum::routing::get(|| async { "ok" }))
        .with_state(state)
}

async fn handle_record_mp4(
    State(state): State<Arc<HookState>>,
    Json(body): Json<Value>,
) -> Json<Value> {
    // 立即返回 200，上传放后台
    let body_clone = body.clone();
    let state_clone = state.clone();
    tokio::spawn(async move {
        if let Err(e) = do_upload_and_notify(state_clone, body_clone).await {
            error!("[record] upload+notify failed: {:#}", e);
        }
    });
    Json(json!({"code": 0}))
}

async fn do_upload_and_notify(state: Arc<HookState>, body: Value) -> Result<()> {
    let file_path = body["file_path"].as_str().unwrap_or("").to_string();
    let stream = body["stream"].as_str().unwrap_or("").to_string();
    let app = body["app"].as_str().unwrap_or("rtp").to_string();
    let vhost = body["vhost"]
        .as_str()
        .unwrap_or("__defaultVhost__")
        .to_string();
    // ZLM hook 里的相对路径（用于本地存储模式拼 URL）
    let relative_url = body["url"].as_str().unwrap_or("").to_string();

    if file_path.is_empty() || stream.is_empty() {
        anyhow::bail!("missing file_path or stream");
    }

    let local_path = std::path::Path::new(&file_path);

    // ── 最终上报给 mgr 的字段 ──
    let mut final_play_url = String::new();
    let mut s3_uploaded = false;
    let mut local_relative_url = String::new(); // S3 关闭/失败时用

    // ── 1. S3 上传（如果启用）──
    if state.s3_enable {
        if local_path.exists() {
            match upload_to_s3(&state, &stream, &app, &vhost, &file_path).await {
                Ok(access_url) => {
                    final_play_url = access_url.clone();
                    s3_uploaded = true;
                    info!("[record] uploaded {} → {}", file_path, access_url);
                }
                Err(e) => {
                    error!("[record] S3 upload failed for {}: {:#}", file_path, e);
                    // 上传失败：让 mgr 用本地 URL 拼
                    local_relative_url = relative_url.clone();
                    if !state.keep_on_failure {
                        let _ = tokio::fs::remove_file(local_path).await;
                    }
                }
            }
        } else {
            warn!("[record] local file not found: {}", file_path);
            local_relative_url = relative_url.clone();
        }
    } else {
        // S3 关闭：本地保留，交给 mgr 拼 URL
        info!("[record] S3 disabled, keep local file: {}", file_path);
        local_relative_url = relative_url.clone();
    }

    // ── 2. 通知 mgr ──
    let notify_payload = {
        let mut p = body.clone();
        p["play_url"] = json!(final_play_url);
        p["s3_uploaded"] = json!(s3_uploaded);
        p["relative_url"] = json!(local_relative_url);
        p["node_id"] = json!(state.server_id); // = mediaServerId
        p
    };

    let url = format!("{}/internal/on-record-event", state.mgr_base);
    let notify_ok = match state
        .http_client
        .post(&url)
        .header("X-Internal-Token", &state.internal_api_token)
        .json(&notify_payload)
        .timeout(Duration::from_secs(10))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            if log::log_enabled!(log::Level::Debug) {
                log::debug!("[record] notified mgr: {}", file_path);
            }
            true
        }
        Ok(resp) => {
            warn!("[record] mgr notify returned HTTP {}", resp.status());
            false
        }
        Err(e) => {
            warn!("[record] mgr notify failed: {}", e);
            false
        }
    };

    // ── 3. 通知成功且 S3 上传成功 → 删本地 ──
    if s3_uploaded && notify_ok {
        if let Err(e) = tokio::fs::remove_file(local_path).await {
            warn!("[record] remove local failed: {}", e);
        } else {
            // 清空目录（非空会失败，忽略）
            if let Some(parent) = local_path.parent() {
                let _ = tokio::fs::remove_dir(parent).await;
            }
        }
    } else if s3_uploaded {
        warn!(
            "[record] S3 uploaded but mgr notify failed, keep local file: {}",
            file_path
        );
    }

    Ok(())
}

/// 上传录像到 S3（流式，避免大文件占内存）
async fn upload_to_s3(
    state: &HookState,
    stream: &str,
    app: &str,
    vhost: &str,
    file_path: &str,
) -> Result<String> {
    // ── 1. 向 mgr 要 presigned PUT URL ──
    let presign_url = format!("{}/api/internal/presign-record", state.mgr_base);
    let presign_resp: Value = state
        .http_client
        .post(&presign_url)
        .header("X-Internal-Token", &state.internal_api_token)
        .json(&json!({
            "stream": stream,
            "app": app,
            "vhost": vhost,
            "file_path": file_path,
        }))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .context("presign request failed")?
        .json()
        .await
        .context("presign response parse failed")?;

    if presign_resp["code"].as_i64() != Some(0) {
        anyhow::bail!(
            "presign rejected: {}",
            presign_resp["msg"].as_str().unwrap_or("unknown")
        );
    }

    let upload_url = presign_resp["upload_url"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("no upload_url"))?;
    let access_url = presign_resp["access_url"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("no access_url"))?
        .to_string();

    // ── 2. 流式 PUT 到 S3 ──
    let file = tokio::fs::File::open(file_path)
        .await
        .with_context(|| format!("open file failed: {}", file_path))?;
    let file_size = file
        .metadata()
        .await
        .with_context(|| format!("stat file failed: {}", file_path))?
        .len();

    let stream_body = tokio_util::io::ReaderStream::new(file);
    let body = reqwest::Body::wrap_stream(stream_body);

    let put_resp = state
        .http_client
        .put(upload_url)
        .header("Content-Length", file_size.to_string())
        .body(body)
        .timeout(Duration::from_secs(600)) // 大文件给 10 分钟
        .send()
        .await
        .context("S3 PUT failed")?;

    if !put_resp.status().is_success() {
        anyhow::bail!("S3 PUT returned HTTP {}", put_resp.status());
    }

    info!("[record] S3 PUT ok: {} bytes → {}", file_size, access_url);
    Ok(access_url)
}
