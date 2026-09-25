use anyhow::{Context, Result, anyhow};
use log::{error, info, warn};
use reqwest::Client;
use rustls::pki_types::ServerName;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, RwLock};
use tokio_rustls::TlsConnector;

mod record_hook;
mod tunnel;

// ---------- 外网 API 列表 ----------
const API_URLS: &[&str] = &["https://ip.3322.net", "https://ip.automate.org.cn"];

// ---------- 共享缓存类型 ----------
/// mediaServerId 缓存：None = 未获取，Some = 已获取且永久缓存
type MediaServerIdCache = Arc<RwLock<Option<String>>>;
/// ZLM version 缓存：None = 未获取，Some = 已获取且永久缓存
type VersionCache = Arc<RwLock<Option<String>>>;

// ---------- 配置 ----------
#[derive(Deserialize, Clone)]
struct Config {
    /// 可选兜底 server_id：mediaServerId 拿不到时使用；拿到则忽略。
    /// 未设置时（None），mediaServerId 未就绪阶段会跳过上报。
    server_id: Option<String>,

    #[serde(default = "default_api_base")]
    api_base: String,

    #[serde(default = "default_secret")]
    secret: String,

    #[serde(default = "default_mgr_url")]
    mgr_url: String,

    #[serde(default)]
    node_token: String,

    custom_ip: Option<String>,
    interface: Option<String>,

    #[serde(default = "default_zlm_http_port")]
    zlm_http_port: u16,

    #[serde(default)]
    use_https: bool,

    #[serde(default = "default_report_interval")]
    report_interval_secs: u64,

    #[serde(default = "default_ip_refresh_interval")]
    ip_refresh_interval_secs: u64,

    static_base: Option<String>,

    #[serde(default)]
    tls_accept_invalid_certs: bool,

    #[serde(default = "default_enable_rtc")]
    enable_rtc_extern_ip_update: bool,

    #[serde(default)]
    http_fmp4_base: Option<String>,

    ip_echo_api: Option<String>,

    // ---------- TLS 隧道配置（可选） ----------
    #[serde(default)]
    tunnel_enable: bool,

    #[serde(default = "default_tunnel_local_addr")]
    tunnel_local_addr: String,

    #[serde(default)]
    tunnel_remote_addr: String,

    #[serde(default = "default_tunnel_server_name")]
    tunnel_server_name: String,

    #[serde(default)]
    tunnel_ca_cert: Option<String>,

    #[serde(default)]
    tunnel_client_cert: Option<String>,

    #[serde(default)]
    tunnel_client_key: Option<String>,

    #[serde(default = "default_tunnel_max_connections")]
    tunnel_max_connections: usize,

    #[serde(default = "default_tunnel_idle_timeout_secs")]
    tunnel_idle_timeout_secs: u64,

    #[serde(default = "default_tunnel_connect_timeout_secs")]
    tunnel_connect_timeout_secs: u64,

    #[serde(default = "default_tunnel_retry_delay_secs")]
    tunnel_retry_delay_secs: u64,

    #[serde(default = "default_tunnel_max_retry_delay_secs")]
    tunnel_max_retry_delay_secs: u64,

    #[serde(default = "default_tunnel_buffer_size")]
    tunnel_buffer_size: usize,

    #[serde(default)]
    tunnel_disable_tls_resumption: bool,

    // ---------- 录像 hook 服务（可选） ----------
    #[serde(default)]
    record_hook_enable: bool,

    #[serde(default = "default_hook_listen")]
    hook_listen: String,

    #[serde(default = "default_mgr_base")]
    mgr_base: String,

    #[serde(default)]
    internal_api_token: String,

    #[serde(default = "default_record_keep_on_failure")]
    record_keep_on_failure: bool,

    #[serde(default = "default_record_hook_sync_interval")]
    record_hook_sync_interval_secs: u64,

    /// 是否启用 S3 上传（默认 false）
    #[serde(default = "default_record_s3_enable")]
    record_s3_enable: bool,
}

fn default_api_base() -> String {
    "http://127.0.0.1:9080".into()
}
fn default_secret() -> String {
    "935v73f7-bb6b-4889-a715-d9eb2d1936aa".into()
}
fn default_mgr_url() -> String {
    "http://127.0.0.1:3002/api/zlm/report-status".into()
}
fn default_zlm_http_port() -> u16 {
    9080
}
fn default_report_interval() -> u64 {
    30
}
fn default_ip_refresh_interval() -> u64 {
    5
}
fn default_enable_rtc() -> bool {
    true
}
fn default_tunnel_local_addr() -> String {
    "127.0.0.1:3003".to_string()
}
fn default_tunnel_server_name() -> String {
    "tunnel.example.com".to_string()
}
fn default_tunnel_max_connections() -> usize {
    100
}
fn default_tunnel_idle_timeout_secs() -> u64 {
    300
}
fn default_tunnel_connect_timeout_secs() -> u64 {
    10
}
fn default_tunnel_retry_delay_secs() -> u64 {
    2
}
fn default_tunnel_max_retry_delay_secs() -> u64 {
    30
}
fn default_tunnel_buffer_size() -> usize {
    64 * 1024
}
fn default_hook_listen() -> String {
    "127.0.0.1:3004".into()
}
fn default_mgr_base() -> String {
    "http://127.0.0.1:3002".into()
}
fn default_record_keep_on_failure() -> bool {
    true
}
fn default_record_hook_sync_interval() -> u64 {
    60
}
fn default_record_s3_enable() -> bool {
    false
}

// ---------- 工具函数 ----------
fn hash_token(token: &str) -> String {
    blake3::hash(token.as_bytes()).to_string()
}

fn extract_ipv4(text: &str) -> Option<String> {
    text.split(|c: char| !c.is_ascii_digit() && c != '.')
        .filter(|s| !s.is_empty())
        .find(|part| part.matches('.').count() == 3 && part.parse::<std::net::Ipv4Addr>().is_ok())
        .map(String::from)
}

fn get_interface_ip(iface: &str) -> Result<String> {
    let ifaces = get_if_addrs::get_if_addrs().context("Failed to query network interfaces")?;
    for iface_info in ifaces {
        if iface_info.name == iface {
            if let std::net::IpAddr::V4(ipv4) = iface_info.ip() {
                return Ok(ipv4.to_string());
            }
        }
    }
    anyhow::bail!("Interface '{}' not found or has no IPv4 address", iface)
}

fn resolve_placeholder(base: &str, public_ip: &str) -> String {
    let mut result = base.replace("{ip}", public_ip);

    let mut search_from = 0;
    while let Some(start_rel) = result[search_from..].find("{interface:") {
        let start = search_from + start_rel;
        if let Some(end_rel) = result[start..].find('}') {
            let end = start + end_rel;
            let iface = result[start + "{interface:".len()..end].to_string();
            let replacement = match get_interface_ip(&iface) {
                Ok(ip) => ip,
                Err(e) => {
                    warn!("Failed to get IP for interface '{}': {:?}", iface, e);
                    format!("{{interface:{}}}", iface)
                }
            };
            result.replace_range(start..=end, &replacement);
            search_from = start + replacement.len();
        } else {
            break;
        }
    }

    result
}

// ---------- 获取公网 IP（按优先级）----------
async fn get_public_ip(client: &Client, config: &Config, url_index: &mut usize) -> Result<String> {
    if let Some(ip) = &config.custom_ip {
        return Ok(ip.clone());
    }

    if let Some(iface) = &config.interface {
        if let Ok(ip) = get_interface_ip(iface) {
            return Ok(ip);
        }
        warn!(
            "failed to get IP for interface '{}', falling back to external APIs",
            iface
        );
    }

    if let Some(url) = &config.ip_echo_api {
        let resp = client
            .get(url)
            .send()
            .await
            .with_context(|| format!("Failed to query custom IP service {}", url))?;
        let text = resp
            .text()
            .await
            .with_context(|| format!("Failed to read response from custom IP service {}", url))?;
        let trimmed = text.trim();
        if trimmed.parse::<std::net::Ipv4Addr>().is_ok() {
            return Ok(trimmed.to_string());
        }
        if let Some(ip) = extract_ipv4(trimmed) {
            return Ok(ip);
        }
        warn!("Custom IP service returned invalid response, falling back to built-in APIs");
    }

    let url = API_URLS[*url_index % API_URLS.len()];
    *url_index += 1;

    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("Failed to query {}", url))?;
    let text = resp
        .text()
        .await
        .with_context(|| format!("Failed to read response from {}", url))?;
    let trimmed = text.trim();
    if trimmed.parse::<std::net::Ipv4Addr>().is_ok() {
        return Ok(trimmed.to_string());
    }
    if let Some(ip) = extract_ipv4(trimmed) {
        return Ok(ip);
    }
    anyhow::bail!("No valid IPv4 address in response from {}", url)
}

// ---------- 获取 ZLM 版本 ----------
async fn get_zlm_version(client: &Client, config: &Config) -> String {
    let url = format!(
        "{}/index/api/version?secret={}",
        config.api_base, config.secret
    );
    match client.get(&url).send().await {
        Ok(resp) => match resp.text().await {
            Ok(text) => {
                let json: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => return "online (unknown version)".to_string(),
                };
                let branch = get_json_str(&json, "branchName");
                let hash = get_json_str(&json, "commitHash");
                match (branch, hash) {
                    (Some(b), Some(h)) => format!("{}({})", b, h),
                    (Some(b), None) => b,
                    _ => "online (unknown version)".to_string(),
                }
            }
            Err(_) => "online (unknown version)".to_string(),
        },
        Err(_) => "offline".to_string(),
    }
}

/// 已缓存 → 直接返回；未缓存 → 请求一次；
/// 只有拿到真实版本号才写入缓存，"offline" / "online (unknown version)" 不缓存。
async fn get_or_fetch_version(
    client: &Client,
    config: &Config,
    cache: &VersionCache,
) -> Option<String> {
    if let Some(v) = cache.read().await.clone() {
        return Some(v);
    }
    let v = get_zlm_version(client, config).await;
    if v != "offline" && v != "online (unknown version)" {
        info!("ZLM version: {}", v);
        *cache.write().await = Some(v.clone());
        Some(v)
    } else {
        None
    }
}

fn get_json_str(value: &serde_json::Value, key: &str) -> Option<String> {
    if let Some(v) = value.get(key).and_then(|v| v.as_str()) {
        return Some(v.to_string());
    }
    value
        .get("data")
        .and_then(|data| data.get(key))
        .and_then(|v| v.as_str())
        .map(String::from)
}

// ---------- 获取 ZLM 的 mediaServerId ----------
async fn get_zlm_media_server_id(client: &Client, config: &Config) -> Option<String> {
    let url = format!(
        "{}/index/api/getServerConfig?secret={}",
        config.api_base, config.secret
    );
    log::info!("[mediaServerId] GET {}", url); // ← 看实际用的 secret

    let resp: serde_json::Value = client
        .get(&url)
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;

    log::info!(
        "[mediaServerId] resp code={:?} msg={:?}",
        resp["code"],
        resp["msg"]
    );

    let id = resp["data"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|d| d["general.mediaServerId"].as_str())
        .map(String::from);

    log::info!("[mediaServerId] resolved = {:?}", id);
    id
}

/// 已缓存 → 直接返回；未缓存 → 请求一次，成功则永久缓存。
async fn get_or_fetch_media_server_id(
    client: &Client,
    config: &Config,
    cache: &MediaServerIdCache,
) -> Option<String> {
    if let Some(id) = cache.read().await.clone() {
        return Some(id);
    }
    match get_zlm_media_server_id(client, config).await {
        Some(id) => {
            info!("[init] detected ZLM mediaServerId = {}", id);
            *cache.write().await = Some(id.clone());
            Some(id)
        }
        None => None,
    }
}

/// 解析本次上报要用的 server_id：
/// 1) mediaServerId（优先，拿到即缓存）
/// 2) 配置的 SERVER_ID（仅当 mediaServerId 拿不到，且用户显式配置了）
/// 3) None → 调用方跳过本次上报
async fn resolve_server_id(
    client: &Client,
    config: &Config,
    cache: &MediaServerIdCache,
) -> Option<String> {
    if let Some(id) = get_or_fetch_media_server_id(client, config, cache).await {
        return Some(id);
    }
    if let Some(fallback) = &config.server_id {
        warn!(
            "mediaServerId unavailable, using configured SERVER_ID={} as fallback",
            fallback
        );
        return Some(fallback.clone());
    }
    None
}

// ---------- 更新 ZLM 的 rtc.externIP ----------
async fn update_extern_ip(client: &Client, config: &Config, ip: &str) {
    let url = format!(
        "{}/index/api/setServerConfig?secret={}",
        config.api_base, config.secret
    );
    let body = serde_json::json!({
        "rtc.externIP": ip
    });

    match client.post(&url).json(&body).send().await {
        Ok(resp) => {
            if resp.status().is_success() {
                info!("Successfully updated rtc.externIP to {}", ip);
            } else {
                warn!(
                    "Failed to update rtc.externIP, HTTP {}",
                    resp.status().as_u16()
                );
            }
        }
        Err(e) => {
            warn!("Failed to update rtc.externIP: {:?}", e);
        }
    }
}

// ---------- 设置 ZLM 的 on_record_mp4 hook ----------
/// 返回 true 表示真正设置成功（HTTP 200 且 ZLM 返回 code == 0）
async fn set_record_hook(client: &Client, config: &Config) -> bool {
    let hook_url = {
        let port = config.hook_listen.rsplit(':').next().unwrap_or("3004");
        let host = config
            .hook_listen
            .rsplit_once(':')
            .map(|(h, _)| h)
            .unwrap_or("127.0.0.1");
        let host = if host == "0.0.0.0" || host == "::" {
            "127.0.0.1"
        } else {
            host
        };
        format!("http://{}:{}/hook/on_record_mp4", host, port)
    };

    let url = format!(
        "{}/index/api/setServerConfig?secret={}",
        config.api_base, config.secret
    );
    let body = serde_json::json!({
        "hook.on_record_mp4": hook_url
    });

    match client.post(&url).json(&body).send().await {
        Ok(resp) => {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();

            // ✅ 同时校验 HTTP 200 和 ZLM 的 code == 0
            let code = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| v.get("code").and_then(|c| c.as_i64()));

            if status.is_success() && code == Some(0) {
                info!("[record-hook] set ZLM hook.on_record_mp4 = {}", hook_url);
                true
            } else {
                warn!(
                    "[record-hook] failed to set hook, HTTP {} code={:?} body={}",
                    status.as_u16(),
                    code,
                    text
                );
                false
            }
        }
        Err(e) => {
            warn!("[record-hook] failed to set hook: {:?}", e);
            false
        }
    }
}

// ---------- 上报状态 ----------
async fn report_status(
    client: &Client,
    config: &Config,
    server_id: &str,
    public_ip: &str,
    version: &str,
) -> Result<()> {
    let http_fmp4_base_raw = if let Some(custom) = &config.http_fmp4_base {
        custom.clone()
    } else {
        let proto = if config.use_https { "https" } else { "http" };
        format!("{}://{}:{}/", proto, public_ip, config.zlm_http_port)
    };
    let http_fmp4_base = resolve_placeholder(&http_fmp4_base_raw, public_ip);

    let static_base_raw = config
        .static_base
        .clone()
        .unwrap_or_else(|| http_fmp4_base.clone());
    let static_base = resolve_placeholder(&static_base_raw, public_ip);

    let hashed_token = hash_token(&config.node_token);

    let payload = serde_json::json!({
        "server_id": server_id,
        "api_base": config.api_base,
        "secret": config.secret,
        "public_ip": public_ip,
        "http_fmp4_base": http_fmp4_base,
        "static_base": static_base,
        "version": version,
    });

    let resp = client
        .post(&config.mgr_url)
        .header("Content-Type", "application/json")
        .header("X-Node-Token", hashed_token)
        .json(&payload)
        .send()
        .await
        .context("Failed to send report")?;

    if !resp.status().is_success() {
        warn!("report returned HTTP {}", resp.status().as_u16());
    }
    Ok(())
}

// ---------- 构建 TLS 连接器 ----------
fn build_tls_connector(config: &Config) -> Result<TlsConnector> {
    let mut roots = rustls::RootCertStore::empty();
    if let Some(ca) = &config.tunnel_ca_cert {
        let bytes = std::fs::read(ca)?;
        for c in rustls_pemfile::certs(&mut bytes.as_slice()) {
            roots.add(c?)?;
        }
    } else {
        for c in rustls_native_certs::load_native_certs()? {
            roots.add(c)?;
        }
    }

    let builder = rustls::ClientConfig::builder().with_root_certificates(roots);
    let mut client_config = match (&config.tunnel_client_cert, &config.tunnel_client_key) {
        (Some(cert), Some(key)) => {
            let cert_bytes = std::fs::read(cert)?;
            let certs: Vec<_> =
                rustls_pemfile::certs(&mut cert_bytes.as_slice()).collect::<Result<Vec<_>, _>>()?;

            let key_bytes = std::fs::read(key)?;
            let key = rustls_pemfile::private_key(&mut key_bytes.as_slice())?
                .ok_or_else(|| anyhow!("Invalid client key"))?;
            builder.with_client_auth_cert(certs, key)?
        }
        _ => builder.with_no_client_auth(),
    };

    if config.tunnel_disable_tls_resumption {
        client_config.resumption = rustls::client::Resumption::disabled();
    }

    Ok(TlsConnector::from(Arc::new(client_config)))
}

// ---------- 持续运行主循环 ----------
async fn run_loop(
    client: &Client,
    config: &Config,
    shutdown: Arc<Notify>,
    media_server_id: MediaServerIdCache,
    version: VersionCache,
) {
    let report_interval = Duration::from_secs(config.report_interval_secs);
    let ip_refresh_interval = Duration::from_secs(config.ip_refresh_interval_secs);

    let mut report_tick = tokio::time::interval(report_interval);
    let mut ip_refresh_tick = tokio::time::interval(ip_refresh_interval);

    let mut cached_ip: Option<String> = None;
    let mut url_index = 0;

    loop {
        tokio::select! {
            _ = shutdown.notified() => {
                info!("Shutdown signal received, exiting main loop.");
                break;
            }

            _ = report_tick.tick() => {
                // mediaServerId 优先，其次配置 SERVER_ID，都没有则跳过本次上报
                let sid = match resolve_server_id(client, config, &media_server_id).await {
                    Some(id) => id,
                    None => {
                        warn!("Skip report: mediaServerId not ready and no SERVER_ID fallback");
                        continue;
                    }
                };

                let ver = get_or_fetch_version(client, config, &version)
                    .await
                    .unwrap_or_else(|| "offline".to_string());

                if cached_ip.is_none() {
                    match get_public_ip(client, config, &mut url_index).await {
                        Ok(ip) => cached_ip = Some(ip),
                        Err(e) => {
                            warn!("Failed to get IP for initial report: {:?}", e);
                            continue;
                        }
                    }
                }
                let ip = cached_ip.as_ref().unwrap();

                if let Err(e) = report_status(client, config, &sid, ip, &ver).await {
                    warn!("Periodic report failed: {:?}", e);
                } else {
                    info!("Periodic report sent (server_id={}, version={}).", sid, ver);
                }
            }

            _ = ip_refresh_tick.tick() => {
                let sid = match resolve_server_id(client, config, &media_server_id).await {
                    Some(id) => id,
                    None => continue,
                };

                let ver = get_or_fetch_version(client, config, &version)
                    .await
                    .unwrap_or_else(|| "offline".to_string());

                match get_public_ip(client, config, &mut url_index).await {
                    Ok(ip) => {
                        if cached_ip.as_ref() != Some(&ip) {
                            info!("Public IP changed from {:?} to {}", cached_ip, ip);
                            cached_ip = Some(ip.clone());

                            if config.enable_rtc_extern_ip_update {
                                update_extern_ip(client, config, &ip).await;
                            }

                            if let Err(e) =
                                report_status(client, config, &sid, &ip, &ver).await
                            {
                                warn!("Immediate report after IP change failed: {:?}", e);
                            } else {
                                info!("Immediate report after IP change sent.");
                            }
                        } else {
                            cached_ip = Some(ip);
                        }
                    }
                    Err(e) => {
                        warn!("Failed to refresh IP: {:?}", e);
                    }
                }
            }
        }
    }
}

/// 生成随机 token，输出明文和 blake3 hash，然后退出。
///
/// 输出格式（供 shell 脚本解析）：
///   NODE_TOKEN=<64位小写hex>
///   NODE_REPORT_TOKEN=<64位小写hex>
///
/// 语义：
///   - NODE_TOKEN          明文，填给 zlm-node 的 `NODE_TOKEN` 环境变量
///   - NODE_REPORT_TOKEN   blake3(NODE_TOKEN)，填给 mgr 的 `NODE_REPORT_TOKEN`
fn generate_token_and_exit() -> Result<()> {
    // 32 字节密码学随机数
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow!("getrandom failed: {}", e))?;

    // 转 hex 作为明文 token
    let token_plain = bytes
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>();

    // blake3 hash，与运行时 hash_token() 完全一致
    let hash = blake3::hash(token_plain.as_bytes()).to_string();

    // 只输出两行，不写日志，避免污染 stdout
    println!("NODE_TOKEN={}", token_plain);
    println!("NODE_REPORT_TOKEN={}", hash);
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // ⬇️ 新增：--gen-token 生成 token 后立即退出
    if std::env::args().any(|a| a == "--gen-token") {
        return generate_token_and_exit();
    }

    let config: Config =
        envy::from_env().context("Failed to load configuration from environment")?;

    let client = Client::builder()
        .timeout(Duration::from_secs(5))
        .danger_accept_invalid_certs(config.tls_accept_invalid_certs)
        .build()
        .context("Failed to build HTTP client")?;

    // ---------- 共享缓存 ----------
    let media_server_id: MediaServerIdCache = Arc::new(RwLock::new(None));
    let version: VersionCache = Arc::new(RwLock::new(None));

    // 启动时各尝试一次（失败不阻塞，等 run_loop 里重试）
    if get_or_fetch_media_server_id(&client, &config, &media_server_id)
        .await
        .is_none()
    {
        match &config.server_id {
            Some(id) => warn!(
                "[init] mediaServerId unavailable, will fallback to SERVER_ID={}",
                id
            ),
            None => {
                warn!("[init] mediaServerId unavailable, report will be skipped until ZLM ready")
            }
        }
    }
    if get_or_fetch_version(&client, &config, &version)
        .await
        .is_none()
    {
        warn!("[init] ZLM version unavailable, will retry later");
    }

    let shutdown = Arc::new(Notify::new());

    let shutdown_clone = shutdown.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        shutdown_clone.notify_waiters();
    });

    // ---------- 录像 hook server ----------
    if config.record_hook_enable {
        if config.internal_api_token.is_empty() {
            return Err(anyhow!(
                "INTERNAL_API_TOKEN must be set when RECORD_HOOK_ENABLE=true"
            ));
        }

        let hook_state = Arc::new(record_hook::HookState {
            mgr_base: config.mgr_base.clone(),
            internal_api_token: config.internal_api_token.clone(),
            http_client: client.clone(),
            keep_on_failure: config.record_keep_on_failure,
            s3_enable: config.record_s3_enable,
            media_server_id: media_server_id.clone(),
            fallback_server_id: config.server_id.clone(),
        });

        let app = record_hook::router(hook_state);
        let listen = config.hook_listen.clone();

        let listener = tokio::net::TcpListener::bind(&listen)
            .await
            .with_context(|| format!("bind hook server to {} failed", listen))?;

        info!("[record-hook] listening on {}", listen);

        let shutdown_clone = shutdown.clone();
        tokio::spawn(async move {
            let server = axum::serve(listener, app).with_graceful_shutdown(async move {
                shutdown_clone.notified().await;
            });
            if let Err(e) = server.await {
                error!("[record-hook] server error: {}", e);
            }
        });

        // 启动时立即设置 hook
        set_record_hook(&client, &config).await;

        // 定期兜底：ZLM 重启后 hook 会丢；失败指数退避，成功后回落到稳态
        let client_clone = client.clone();
        let config_clone = config.clone();
        let shutdown_clone = shutdown.clone();
        tokio::spawn(async move {
            let steady_delay =
                Duration::from_secs(config_clone.record_hook_sync_interval_secs.max(10));
            let mut delay = Duration::from_secs(2);
            loop {
                tokio::select! {
                    _ = shutdown_clone.notified() => {
                        info!("[record-hook] sync loop exit");
                        break;
                    }
                    _ = tokio::time::sleep(delay) => {}
                }
                let ok = set_record_hook(&client_clone, &config_clone).await;
                delay = if ok {
                    steady_delay
                } else {
                    (delay * 2).min(Duration::from_secs(30))
                };
            }
        });
    }

    // ---------- TLS 隧道 ----------
    if config.tunnel_enable {
        if config.tunnel_remote_addr.is_empty() {
            return Err(anyhow!(
                "TUNNEL_REMOTE_ADDR must be set when tunnel is enabled"
            ));
        }

        let mut addrs = tokio::net::lookup_host(&config.tunnel_remote_addr)
            .await
            .context("Invalid TUNNEL_REMOTE_ADDR")?;
        if addrs.next().is_none() {
            return Err(anyhow!("TUNNEL_REMOTE_ADDR did not resolve to any address"));
        }

        let server_name = ServerName::try_from(config.tunnel_server_name.clone())
            .map_err(|e| anyhow!("Invalid tunnel server name: {}", e))?;
        let connector = build_tls_connector(&config)?;
        let tunnel_auth_token = std::env::var("TUNNEL_AUTH_TOKEN")
            .ok()
            .filter(|s| !s.is_empty());
        let tunnel_cfg = Arc::new(tunnel::TunnelConfig {
            local_addr: config.tunnel_local_addr.clone(),
            remote_addr: config.tunnel_remote_addr.clone(),
            server_name,
            connector,
            max_connections: config.tunnel_max_connections,
            idle_timeout: (config.tunnel_idle_timeout_secs > 0)
                .then(|| Duration::from_secs(config.tunnel_idle_timeout_secs)),
            connect_timeout: Duration::from_secs(config.tunnel_connect_timeout_secs),
            retry_delay: Duration::from_secs(config.tunnel_retry_delay_secs),
            max_retry_delay: Duration::from_secs(config.tunnel_max_retry_delay_secs),
            buffer_size: config.tunnel_buffer_size,
            auth_token: tunnel_auth_token,
        });

        let shutdown_clone = shutdown.clone();
        tokio::spawn(async move {
            if let Err(e) = tunnel::run_tls_tunnel(tunnel_cfg, shutdown_clone).await {
                log::error!("TLS tunnel failed: {}", e);
            }
        });

        info!(
            "TLS tunnel enabled: {} -> {}",
            config.tunnel_local_addr, config.tunnel_remote_addr
        );
    }

    run_loop(&client, &config, shutdown, media_server_id, version).await;

    Ok(())
}
