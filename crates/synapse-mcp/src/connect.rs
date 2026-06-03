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

use std::process::ExitCode;

use anyhow::Context;
use rmcp::transport::{
    Transport,
    async_rw::AsyncRwTransport,
    streamable_http_client::{StreamableHttpClientTransport, StreamableHttpClientTransportConfig},
};

/// Run the stdio<->HTTP bridge against the daemon listening at `bind`
/// (`host:port`). Exits 0 when the client closes stdin or the daemon stream
/// ends.
pub async fn run_connect(bind: &str) -> anyhow::Result<ExitCode> {
    let uri = format!("http://{bind}/mcp");
    let token = crate::http::load_token_value().context("load daemon bearer token for bridge")?;
    tracing::info!(
        code = "MCP_CONNECT_STARTING",
        daemon_uri = %uri,
        "starting stdio<->http bridge to shared daemon"
    );

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
