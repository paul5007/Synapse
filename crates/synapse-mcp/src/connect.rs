//! `--mode connect`: native stdio<->HTTP bridge to the shared Synapse daemon.
//!
//! Lets a stdio-only MCP client (Claude Desktop, Codex) reach the single shared
//! HTTP daemon instead of spawning its own embedded server (which would contend
//! for the one RocksDB lock). The bridge is a transport-level pump: it forwards
//! raw JSON-RPC between the client's stdio transport and an rmcp
//! Streamable-HTTP client transport pointed at the daemon, so the initialize
//! handshake, `Mcp-Session-Id` sessions, and SSE server->client notifications
//! are all handled by rmcp's client worker. No message interpretation, no
//! external proxy dependency.

use std::{path::Path, process::ExitCode, time::Duration};

use anyhow::Context;
use rmcp::transport::{
    Transport,
    async_rw::AsyncRwTransport,
    streamable_http_client::{StreamableHttpClientTransport, StreamableHttpClientTransportConfig},
};

/// How long to wait for a freshly spawned daemon to become healthy.
const DAEMON_READY_TIMEOUT: Duration = Duration::from_secs(15);
const DAEMON_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Probe the daemon `/health` endpoint. Returns true only on a 2xx response.
async fn probe_health(bind: &str, token: &str) -> bool {
    let url = format!("http://{bind}/health");
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_millis(1500))
        .build()
    else {
        return false;
    };
    match client.get(&url).bearer_auth(token).send().await {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

/// Spawn the shared daemon detached (its own stdio = null so it never writes to
/// the bridge's MCP stdout, and it outlives the bridge). The T1 single-instance
/// guard ensures that if several bridges race to spawn, only one daemon wins.
fn spawn_detached_daemon(bind: &str, db: Option<&Path>) -> anyhow::Result<()> {
    let exe = std::env::current_exe().context("resolve current executable path")?;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["--mode", "http", "--bind", bind]);
    if let Some(db) = db {
        cmd.arg("--db").arg(db);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS (no inherited console) | CREATE_NO_WINDOW.
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);
    }
    cmd.spawn().context("spawn shared daemon process")?;
    Ok(())
}

/// Ensure a shared daemon is reachable at `bind`: probe, and if absent spawn one
/// (guarded) and wait until it is healthy. Errors (no fallback) if it never
/// comes up within [`DAEMON_READY_TIMEOUT`].
async fn ensure_daemon_running(bind: &str, db: Option<&Path>, token: &str) -> anyhow::Result<()> {
    if probe_health(bind, token).await {
        tracing::info!(
            code = "MCP_CONNECT_DAEMON_PRESENT",
            bind = %bind,
            "shared daemon already running"
        );
        return Ok(());
    }
    tracing::info!(
        code = "MCP_CONNECT_DAEMON_SPAWNING",
        bind = %bind,
        "no daemon detected; spawning shared daemon"
    );
    spawn_detached_daemon(bind, db).context("spawn shared daemon")?;

    let max_attempts = (DAEMON_READY_TIMEOUT.as_millis() / DAEMON_POLL_INTERVAL.as_millis()) as u32;
    for attempt in 1..=max_attempts {
        tokio::time::sleep(DAEMON_POLL_INTERVAL).await;
        if probe_health(bind, token).await {
            tracing::info!(
                code = "MCP_CONNECT_DAEMON_READY",
                bind = %bind,
                attempts = attempt,
                "spawned daemon is healthy"
            );
            return Ok(());
        }
    }
    anyhow::bail!(
        "MCP_DAEMON_SPAWN_FAILED: shared daemon at {bind} did not become healthy within {}s after spawn",
        DAEMON_READY_TIMEOUT.as_secs()
    );
}

/// Run the stdio<->HTTP bridge against the daemon listening at `bind`
/// (`host:port`). Exits 0 when the client closes stdin or the daemon stream
/// ends.
pub async fn run_connect(bind: &str, db: Option<&Path>) -> anyhow::Result<ExitCode> {
    let uri = format!("http://{bind}/mcp");
    let token = crate::http::load_token_value().context("load daemon bearer token for bridge")?;
    tracing::info!(
        code = "MCP_CONNECT_STARTING",
        daemon_uri = %uri,
        "starting stdio<->http bridge to shared daemon"
    );

    // Ensure exactly one shared daemon is up (spawn it if needed) before bridging.
    ensure_daemon_running(bind, db, &token)
        .await
        .context("ensure shared daemon is running")?;

    let config = StreamableHttpClientTransportConfig::with_uri(uri).auth_header(token);
    let mut daemon = StreamableHttpClientTransport::from_config(config);

    let (stdin, stdout) = rmcp::transport::stdio();
    let mut client = AsyncRwTransport::new_server(stdin, stdout);

    loop {
        tokio::select! {
            from_client = client.receive() => {
                match from_client {
                    Some(message) => daemon
                        .send(message)
                        .await
                        .context("forward client->daemon message")?,
                    None => {
                        tracing::info!(
                            code = "MCP_CONNECT_STDIN_EOF",
                            "client closed stdin; shutting down bridge"
                        );
                        break;
                    }
                }
            }
            from_daemon = daemon.receive() => {
                match from_daemon {
                    Some(message) => client
                        .send(message)
                        .await
                        .context("forward daemon->client message")?,
                    None => {
                        tracing::info!(
                            code = "MCP_CONNECT_DAEMON_CLOSED",
                            "daemon stream closed; shutting down bridge"
                        );
                        break;
                    }
                }
            }
        }
    }

    let _ = daemon.close().await;
    let _ = client.close().await;
    Ok(ExitCode::SUCCESS)
}
