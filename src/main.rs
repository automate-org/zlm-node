use anyhow::{Context, Result};
use log::{info, warn};
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;

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
fn default_enable_rtc() -> bool {
    true
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

// ---------- 获取公网 IP（按优先级）----------
async fn get_public_ip(client: &Client, config: &Config, url_index: &mut usize) -> Result<String> {
    // 1. 手动指定 IP
    if let Some(ip) = &config.custom_ip {
        return Ok(ip.clone());
    }

    // 2. 指定网卡 IP
    if let Some(iface) = &config.interface {
        if let Ok(ip) = get_interface_ip(iface) {
            return Ok(ip);
        }
        warn!(
            "failed to get IP for interface '{}', falling back to external APIs",
            iface
        );
    }

    // 3. 自定义 IP 服务（新增）
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

    // 4. 外网 API（轮流访问，只访问一个）
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
    // 1. 确定 http_fmp4_base（优先使用环境变量）
    let http_fmp4_base = if let Some(custom) = &config.http_fmp4_base {
        custom.clone()
    } else {
        let proto = if config.use_https { "https" } else { "http" };
        format!("{}://{}:{}/", proto, public_ip, config.zlm_http_port)
    };

    // 2. 确定 static_base（若未设置则回退到 http_fmp4_base）
    let static_base = config
        .static_base
        .clone()
        .unwrap_or_else(|| http_fmp4_base.clone());

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

// ---------- 持续运行主循环 ----------
async fn run_loop(client: &Client, config: &Config) {
    let interval = Duration::from_secs(config.report_interval_secs);
    info!(
        "Starting ZLM node reporter (interval={}s, TLS insecure={}, rtc.externIP update={})",
        config.report_interval_secs,
        config.tls_accept_invalid_certs,
        config.enable_rtc_extern_ip_update
    );

    let mut last_ip: Option<String> = None; // 用于上报缓存
    let mut last_extern_ip: Option<String> = None; // 用于 rtc.externIP 变化检测
    let mut url_index: usize = 0;

    loop {
        match get_public_ip(client, config, &mut url_index).await {
            Ok(ip) => {
                // 更新上报缓存
                last_ip = Some(ip.clone());
                let version = get_zlm_version(client, config).await;
                info!("Public IP: {}, Version: {}", ip, version);

                // 若启用 rtc.externIP 更新且 IP 与上次设置的不同，则调用 ZLM API
                if config.enable_rtc_extern_ip_update {
                    if last_extern_ip.as_ref() != Some(&ip) {
                        update_extern_ip(client, config, &ip).await;
                        last_extern_ip = Some(ip.clone());
                    }
                }

                if let Err(e) = report_status(client, config, &ip, &version).await {
                    warn!("Report failed: {:?}", e);
                } else {
                    info!("Report sent successfully.");
                }
            }
            Err(e) => {
                warn!("Failed to get IP: {:?}", e);
                if let Some(ref ip) = last_ip {
                    warn!("Using last known IP: {}", ip);
                    let version = get_zlm_version(client, config).await;
                    if let Err(e) = report_status(client, config, ip, &version).await {
                        warn!("Report failed (with cached IP): {:?}", e);
                    } else {
                        info!("Report sent (using cached IP).");
                    }
                } else {
                    warn!("No cached IP, skipping this cycle.");
                }
            }
        }

        tokio::time::sleep(interval).await;
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

    run_loop(&client, &config).await;

    #[allow(unreachable_code)]
    Ok(())
}
