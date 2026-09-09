use anyhow::{Context, Result, anyhow};
use log::{info, warn};
use reqwest::Client;
use rustls::pki_types::ServerName;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tokio_rustls::TlsConnector;

mod tunnel;

// ---------- 外网 API 列表 ----------
const API_URLS: &[&str] = &["https://ip.3322.net", "https://ip.automate.org.cn"];

// ---------- 配置 ----------
#[derive(Deserialize)]
struct Config {
    #[serde(default = "default_server_id")]
    server_id: String,

    #[serde(default = "default_api_base")]
    api_base: String,

    #[serde(default = "default_secret")]
    secret: String,

    #[serde(default = "default_mgr_url")]
    mgr_url: String,

    #[serde(default)]
    node_token: String,

    /// 手动指定公网 IP（优先级最高）
    custom_ip: Option<String>,

    /// 指定网卡名称（如 eth0），优先级在 custom_ip 之后、外网 API 之前
    interface: Option<String>,

    #[serde(default = "default_zlm_http_port")]
    zlm_http_port: u16,

    #[serde(default)]
    use_https: bool,

    #[serde(default = "default_report_interval")]
    report_interval_secs: u64,

    /// IP 刷新间隔（秒），默认 5 秒
    #[serde(default = "default_ip_refresh_interval")]
    ip_refresh_interval_secs: u64,

    /// 静态资源基地址（若不设置则与 http_fmp4_base 相同）
    static_base: Option<String>,

    #[serde(default)]
    tls_accept_invalid_certs: bool,

    /// 是否启用自动更新 ZLM 的 rtc.externIP（默认 true）
    #[serde(default = "default_enable_rtc")]
    enable_rtc_extern_ip_update: bool,

    /// 手动指定 http_fmp4_base（优先级最高，覆盖自动生成）
    #[serde(default)]
    http_fmp4_base: Option<String>,

    /// 自定义 IP 获取服务地址（优先级高于内置公共 API）
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

    // 隧道高级配置
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
}

fn default_server_id() -> String {
    "1".into()
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
    "127.0.0.1:18080".to_string()
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
    64 * 1024 // 64KB
}

// ---------- 工具函数 ----------
fn hash_token(token: &str) -> String {
    blake3::hash(token.as_bytes()).to_string()
}

/// 从文本中提取第一个 IPv4 地址
fn extract_ipv4(text: &str) -> Option<String> {
    text.split(|c: char| !c.is_ascii_digit() && c != '.')
        .filter(|s| !s.is_empty())
        .find(|part| part.matches('.').count() == 3 && part.parse::<std::net::Ipv4Addr>().is_ok())
        .map(String::from)
}

/// 获取指定网卡的第一个 IPv4 地址
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

/// 将配置字符串中的 {ip} 和 {interface:网卡名} 替换为实际 IP
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

// ---------- 上报状态 ----------
async fn report_status(
    client: &Client,
    config: &Config,
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
        "server_id": config.server_id,
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

// ---------- 构建 TLS 连接器（用于隧道） ----------
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
            let certs = rustls_pemfile::certs(&mut std::fs::read(cert)?.as_slice())
                .collect::<Result<Vec<_>, _>>()?;
            let key = rustls_pemfile::private_key(&mut std::fs::read(key)?.as_slice())?
                .ok_or_else(|| anyhow!("Invalid client key"))?;
            builder.with_client_auth_cert(certs, key)?
        }
        _ => builder.with_no_client_auth(),
    };

    // 根据配置决定是否禁用 TLS 会话恢复
    if config.tunnel_disable_tls_resumption {
        client_config.resumption = rustls::client::Resumption::disabled();
    }

    Ok(TlsConnector::from(Arc::new(client_config)))
}

// ---------- 持续运行主循环 ----------
async fn run_loop(client: &Client, config: &Config, shutdown: Arc<Notify>) {
    let cached_version = get_zlm_version(client, config).await;
    info!("ZLM version (cached): {}", cached_version);

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
                if let Some(ip) = &cached_ip {
                    if let Err(e) = report_status(client, config, ip, &cached_version).await {
                        warn!("Periodic report failed: {:?}", e);
                    } else {
                        info!("Periodic report sent.");
                    }
                } else {
                    match get_public_ip(client, config, &mut url_index).await {
                        Ok(ip) => {
                            cached_ip = Some(ip.clone());
                            if let Err(e) = report_status(client, config, &ip, &cached_version).await {
                                warn!("Initial report failed: {:?}", e);
                            } else {
                                info!("Initial report sent.");
                            }
                        }
                        Err(e) => warn!("Failed to get IP for initial report: {:?}", e),
                    }
                }
            }
            _ = ip_refresh_tick.tick() => {
                match get_public_ip(client, config, &mut url_index).await {
                    Ok(ip) => {
                        if cached_ip.as_ref() != Some(&ip) {
                            info!("Public IP changed from {:?} to {}", cached_ip, ip);
                            cached_ip = Some(ip.clone());

                            if config.enable_rtc_extern_ip_update {
                                update_extern_ip(client, config, &ip).await;
                            }

                            if let Err(e) = report_status(client, config, &ip, &cached_version).await {
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

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let config: Config =
        envy::from_env().context("Failed to load configuration from environment")?;

    let client = Client::builder()
        .timeout(Duration::from_secs(5))
        .danger_accept_invalid_certs(config.tls_accept_invalid_certs)
        .build()
        .context("Failed to build HTTP client")?;

    let shutdown = Arc::new(Notify::new());

    // 监听 Ctrl+C，触发通知
    let shutdown_clone = shutdown.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        shutdown_clone.notify_waiters();
    });

    // 如果启用了 TLS 隧道，则启动它
    if config.tunnel_enable {
        // 验证远程地址格式（可解析）
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

    run_loop(&client, &config, shutdown).await;

    Ok(())
}
