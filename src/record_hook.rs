// src/record_hook.rs
use anyhow::{Context, Result};
use axum::{Json, Router, extract::State, routing::post};
use log::{error, info, warn};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

/// 与 main.rs 里上报节点状态时用的 hash 完全一致
fn hash_node_token(token: &str) -> String {
    blake3::hash(token.as_bytes()).to_string()
}

/// 与 main.rs 里 MediaServerIdCache 保持同一类型
pub type MediaServerIdCache = std::sync::Arc<tokio::sync::RwLock<Option<String>>>;

#[derive(Clone)]
pub struct HookState {
    pub mgr_base: String,
    pub http_client: reqwest::Client,
    pub keep_on_failure: bool,
    /// 是否启用 S3 上传（关闭时只上报 mgr，本地保留）
    pub s3_enable: bool,
    /// 共享的 mediaServerId 缓存：hook body 未带时从这里读
    pub media_server_id: MediaServerIdCache,
    /// 可选兜底：缓存也为空时用它（来自 main 里 config.server_id）
    pub fallback_server_id: Option<String>,
    pub node_token: String,
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

/// 解析本次 hook 应该用的 server_id：
/// 1) hook body 里的 mediaServerId（ZLM 正常一定会带）
/// 2) 共享缓存里的 mediaServerId（zlm-node 从 ZLM getServerConfig 拿到的）
/// 3) 配置里的 SERVER_ID（可选兜底）
/// 全部为空 → 报错，让 ZLM 按 hook.retry 重试
async fn resolve_hook_server_id(state: &HookState, body: &Value) -> Result<String> {
    if let Some(id) = body["mediaServerId"].as_str().filter(|s| !s.is_empty()) {
        return Ok(id.to_string());
    }
    warn!("[record] hook body missing mediaServerId, falling back");

    if let Some(id) = state.media_server_id.read().await.clone() {
        if !id.is_empty() {
            warn!("[record] using cached mediaServerId = {}", id);
            return Ok(id);
        }
    }

    if let Some(fb) = &state.fallback_server_id {
        if !fb.is_empty() {
            warn!("[record] using configured SERVER_ID fallback = {}", fb);
            return Ok(fb.clone());
        }
    }

    anyhow::bail!("[record] mediaServerId unavailable (body / cache / fallback all empty)")
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

    // ⭐ 解析 server_id（= mediaServerId）
    let server_id = resolve_hook_server_id(&state, &body).await?;

    let local_path = std::path::Path::new(&file_path);

    // 把 ZLM 的数字时间戳转成 mgr 期望的格式
    //   ZLM hook 的 start_time 是秒级 int
    //   time_len 是秒数（float）
    //   mgr 期望：start_time/end_time 是 "YYYY-MM-DD HH:MM:SS" 字符串
    //            record_start_ms 是毫秒 i64
    let start_ts = body["start_time"].as_i64().unwrap_or(0);
    let time_len = body["time_len"].as_f64().unwrap_or(0.0);

    // 注意：from_timestamp 是 UTC；如果 mgr 期望本地时间，需改成
    //   chrono::Local.timestamp_opt(start_ts, 0).single()
    let start_time_str = chrono::DateTime::from_timestamp(start_ts, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default();
    let end_time_str = chrono::DateTime::from_timestamp(start_ts + time_len as i64, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default();

    // ── 最终上报给 mgr 的字段 ──
    let mut final_play_url = String::new();
    let mut s3_uploaded = false;
    let mut local_relative_url = String::new(); // S3 关闭/失败时用
    let mut channel_id: Option<String> = None;

    // ── 1. S3 上传（如果启用）──
    if state.s3_enable {
        if local_path.exists() {
            match upload_to_s3(&state, &stream, &app, &vhost, &file_path).await {
                Ok((access_url, cid)) => {
                    final_play_url = access_url.clone();
                    s3_uploaded = true;
                    channel_id = Some(cid);
                    info!("[record] uploaded {} → {}", file_path, access_url);
                }
                Err(e) => {
                    error!("[record] S3 upload failed for {}: {:#}", file_path, e);
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
        info!("[record] S3 disabled, keep local file: {}", file_path);
        local_relative_url = relative_url.clone();
    }

    // ── 2. 通知 mgr ──
    let notify_payload = {
        let mut p = body.clone();
        p["play_url"] = json!(final_play_url);
        p["s3_uploaded"] = json!(s3_uploaded);
        p["relative_url"] = json!(local_relative_url);
        // node_id = mediaServerId
        p["node_id"] = json!(server_id);

        // 覆盖/补齐 mgr 期望的字段
        p["start_time"] = json!(start_time_str);
        p["end_time"] = json!(end_time_str);
        p["record_start_ms"] = json!(start_ts * 1000); // 秒 → 毫秒

        if let Some(cid) = channel_id {
            p["channel_id"] = json!(cid);
        }

        p
    };

    let url = format!("{}/api/internal/on-record-event", state.mgr_base);
    let notify_ok = match state
        .http_client
        .post(&url)
        .header("X-Node-Token", hash_node_token(&state.node_token))
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
///
/// 返回 `(access_url, channel_id)`：
///   - `access_url`：S3 对象的访问 URL（公网 / CDN）
///   - `channel_id`：mgr 反查出的通道 ID
async fn upload_to_s3(
    state: &HookState,
    stream: &str,
    app: &str,
    vhost: &str,
    file_path: &str,
) -> Result<(String, String)> {
    // ── 1. 向 mgr 要 presigned PUT URL ──
    let presign_url = format!("{}/api/internal/presign-record", state.mgr_base);
    let presign_resp: Value = state
        .http_client
        .post(&presign_url)
        .header("X-Node-Token", hash_node_token(&state.node_token))
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

    let channel_id = presign_resp["channel_id"]
        .as_str()
        .unwrap_or("")
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
    Ok((access_url, channel_id))
}
