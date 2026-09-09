use anyhow::{Result, anyhow};
use log::{debug, info, warn};
use rustls::pki_types::ServerName;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{self, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, Semaphore};
use tokio::time::{Instant, sleep, timeout};
use tokio_rustls::TlsConnector;

pub struct TunnelConfig {
    pub local_addr: String,
    pub remote_addr: String,
    pub server_name: ServerName<'static>,
    pub connector: TlsConnector,
    pub max_connections: usize,
    pub idle_timeout: Option<Duration>,
    pub connect_timeout: Duration,
    pub retry_delay: Duration,
    pub max_retry_delay: Duration,
    pub buffer_size: usize,
    pub auth_token: Option<String>, // 新增：Token 认证
}

pub async fn run_tls_tunnel(cfg: Arc<TunnelConfig>, shutdown: Arc<Notify>) -> Result<()> {
    let listener = TcpListener::bind(&cfg.local_addr).await?;
    info!(
        "TLS tunnel listening on {} -> {}",
        cfg.local_addr, cfg.remote_addr
    );

    let semaphore = if cfg.max_connections > 0 {
        Some(Arc::new(Semaphore::new(cfg.max_connections)))
    } else {
        None
    };

    let mut tasks = tokio::task::JoinSet::new();

    loop {
        tokio::select! {
            _ = shutdown.notified() => {
                info!("TLS tunnel stopping...");
                tasks.abort_all();
                while let Some(_) = tasks.join_next().await {}
                info!("TLS tunnel stopped");
                break;
            }
            res = listener.accept() => {
                let (client_stream, peer) = match res {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("Accept error: {}", e);
                        continue;
                    }
                };

                let permit = if let Some(sem) = &semaphore {
                    match sem.clone().try_acquire_owned() {
                        Ok(p) => Some(p),
                        Err(_) => {
                            warn!("Max connections reached, rejecting {}", peer);
                            drop(client_stream);
                            continue;
                        }
                    }
                } else {
                    None
                };

                let cfg_clone = cfg.clone();
                let shutdown_clone = shutdown.clone();

                tasks.spawn(async move {
                    let _permit = permit;
                    if let Err(e) = handle_tunnel_conn(client_stream, cfg_clone, shutdown_clone).await {
                        warn!("Tunnel conn {} error: {}", peer, e);
                    }
                });
            }
        }
    }

    Ok(())
}

async fn handle_tunnel_conn(
    mut client: TcpStream,
    cfg: Arc<TunnelConfig>,
    shutdown: Arc<Notify>,
) -> Result<()> {
    let (tls, early_data) = connect_remote_forever(&mut client, &cfg, &shutdown).await?;

    let (mut cr, mut cw) = io::split(client);
    let (mut tr, mut tw) = io::split(tls);

    if let Some(data) = early_data {
        if !data.is_empty() {
            tw.write_all(&data).await?;
        }
    }

    let total = if let Some(idle) = cfg.idle_timeout {
        copy_bidirectional_with_idle(&mut cr, &mut cw, &mut tr, &mut tw, idle, cfg.buffer_size)
            .await?
    } else {
        let (a2b, b2a) = tokio::try_join!(io::copy(&mut cr, &mut tw), io::copy(&mut tr, &mut cw))?;
        a2b + b2a
    };

    info!("Tunnel conn closed, bytes: {}", total);
    Ok(())
}

async fn connect_remote_forever(
    client: &mut TcpStream,
    cfg: &Arc<TunnelConfig>,
    shutdown: &Arc<Notify>,
) -> Result<(tokio_rustls::client::TlsStream<TcpStream>, Option<Vec<u8>>)> {
    let mut early_data = Vec::new();
    let mut attempt = 0u64;

    loop {
        attempt += 1;

        let eof = read_available_data(client, &mut early_data, cfg.buffer_size).await?;
        if eof {
            return Err(anyhow!(
                "client disconnected before remote connection established"
            ));
        }

        match timeout(
            cfg.connect_timeout,
            try_connect_remote(&cfg.remote_addr, &cfg.server_name, &cfg.connector),
        )
        .await
        {
            Ok(Ok(mut tls)) => {
                info!("Remote connection established after {} attempt(s)", attempt);

                // 发送认证 token（如果配置）
                if let Some(token) = &cfg.auth_token {
                    tls.write_all(token.as_bytes()).await?;
                    tls.write_all(b"\n").await?;
                    // 可选：等待服务端确认，这里简单认为发送成功即可
                    info!("Auth token sent");
                }

                return Ok((tls, Some(early_data)));
            }
            Ok(Err(e)) => {
                warn!("Connection attempt {} failed: {}", attempt, e);
            }
            Err(_) => {
                warn!("Connection attempt {} timed out", attempt);
            }
        }

        let delay = std::cmp::min(
            cfg.retry_delay * 2u32.pow(attempt.min(5) as u32),
            cfg.max_retry_delay,
        );
        debug!("Retrying in {:?} (attempt {})", delay, attempt);

        tokio::select! {
            _ = shutdown.notified() => {
                return Err(anyhow!("shutdown requested"));
            }
            _ = sleep(delay) => {}
            _ = client.readable() => {
                let eof = read_available_data(client, &mut early_data, cfg.buffer_size).await?;
                if eof {
                    return Err(anyhow!("client disconnected"));
                }
            }
        }
    }
}

async fn read_available_data(
    client: &mut TcpStream,
    buf: &mut Vec<u8>,
    buffer_size: usize,
) -> Result<bool> {
    let mut tmp = vec![0u8; buffer_size];
    loop {
        match client.try_read(&mut tmp) {
            Ok(0) => return Ok(true),
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
            Err(e) => return Err(e.into()),
        }
    }
}

async fn try_connect_remote(
    remote: &str,
    server_name: &ServerName<'static>,
    connector: &TlsConnector,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>> {
    let tcp = TcpStream::connect(remote).await?;
    let tls = connector.connect(server_name.clone(), tcp).await?;
    Ok(tls)
}

async fn copy_bidirectional_with_idle<R1, W1, R2, W2>(
    cr: &mut R1,
    cw: &mut W1,
    tr: &mut R2,
    tw: &mut W2,
    idle: Duration,
    buffer_size: usize,
) -> Result<u64>
where
    R1: tokio::io::AsyncReadExt + Unpin,
    W1: tokio::io::AsyncWriteExt + Unpin,
    R2: tokio::io::AsyncReadExt + Unpin,
    W2: tokio::io::AsyncWriteExt + Unpin,
{
    let mut total = 0u64;
    let mut buf1 = vec![0u8; buffer_size];
    let mut buf2 = vec![0u8; buffer_size];
    let mut last_activity = Instant::now();

    loop {
        tokio::select! {
            _ = sleep_until(last_activity + idle) => {
                return Err(anyhow!("idle timeout"));
            }
            res = cr.read(&mut buf1) => {
                let n = res?;
                if n == 0 {
                    tw.shutdown().await?;
                    return Ok(total);
                }
                tw.write_all(&buf1[..n]).await?;
                total += n as u64;
                last_activity = Instant::now();
            }
            res = tr.read(&mut buf2) => {
                let n = res?;
                if n == 0 {
                    cw.shutdown().await?;
                    return Ok(total);
                }
                cw.write_all(&buf2[..n]).await?;
                total += n as u64;
                last_activity = Instant::now();
            }
        }
    }
}

async fn sleep_until(deadline: Instant) {
    let now = Instant::now();
    if deadline > now {
        sleep(deadline - now).await;
    }
}
