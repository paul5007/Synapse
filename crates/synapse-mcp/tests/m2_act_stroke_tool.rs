use std::path::Path;

use anyhow::Context;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use synapse_core::error_codes;
use synapse_test_utils::stdio_mcp_client::StdioMcpClient;
use tempfile::TempDir;

#[tokio::test]
async fn act_stroke_tools_call_recording_backend_and_path_edges() -> anyhow::Result<()> {
    let log_dir = TempDir::new()?;
    let mut client = StdioMcpClient::launch_and_init_with_env(
        Some(log_dir.path()),
        &[
            ("SYNAPSE_MCP_SYNTHETIC_FIXTURE", "notepad"),
            ("SYNAPSE_MCP_RECORDING_BACKEND", "1"),
        ],
    )
    .await?;
    activate_notepad_profile(&mut client).await?;

    let tools = client.tools_list().await?;
    let tools = tools
        .get("tools")
        .and_then(Value::as_array)
        .context("tools array missing")?;
    assert!(tools.iter().any(|tool| tool["name"] == "act_stroke"));

    let response = client
        .tools_call(
            "act_stroke",
            json!({
                "path": {
                    "kind": "line",
                    "from": {"x": 0.0, "y": 0.0},
                    "to": {"x": 4.0, "y": 0.0}
                },
                "button": "left",
                "velocity_profile": "constant",
                "duration_or_speed": {"kind": "duration_ms", "duration_ms": 4},
                "backend": "software"
            }),
        )
        .await?;
    let stroke: ActStrokeWireResponse = structured(&response)?;
    println!(
        "readback=mcp_act_stroke edge=line after=ok:{} path_kind:{} points:{} path_length:{} duration_ms:{} backend:{}",
        stroke.ok,
        stroke.path_kind,
        stroke.point_stream_count,
        stroke.path_length_px,
        stroke.duration_ms,
        stroke.backend_used
    );
    assert!(stroke.ok);
    assert_eq!(stroke.path_kind, "line");
    assert_eq!(stroke.point_stream_count, 5);
    assert_eq!(stroke.path_length_px, 4.0);
    assert_eq!(stroke.duration_ms, 4.0);
    assert_eq!(stroke.backend_used, "software");

    let one_point = client
        .tools_call_error(
            "act_stroke",
            json!({
                "path": {
                    "kind": "polyline",
                    "points": [{"x": 1.0, "y": 1.0}]
                },
                "duration_or_speed": {"kind": "duration_ms", "duration_ms": 4}
            }),
        )
        .await?;
    println!("readback=mcp_act_stroke edge=one_point after_error={one_point}");
    assert_eq!(
        error_code(&one_point),
        Some(error_codes::TOOL_PARAMS_INVALID)
    );

    let over_cap_points = (0_u32..10_000)
        .map(|index| json!({"x": f64::from(index), "y": 0.0}))
        .collect::<Vec<_>>();
    let over_cap = client
        .tools_call_error(
            "act_stroke",
            json!({
                "path": {
                    "kind": "polyline",
                    "points": over_cap_points
                },
                "duration_or_speed": {"kind": "duration_ms", "duration_ms": 4}
            }),
        )
        .await?;
    println!("readback=mcp_act_stroke edge=over_cap after_error={over_cap}");
    assert_eq!(
        error_code(&over_cap),
        Some(error_codes::TOOL_PARAMS_INVALID)
    );

    assert!(client.shutdown().await?.success());
    let logs = read_logs(log_dir.path())?;
    let contains_recording_readback = logs.contains("M2_ACT_STROKE_RECORDING_READBACK")
        && logs.contains("readback=recording_backend tool=act_stroke");
    println!(
        "readback=daemon_log edge=act_stroke after_bytes={} contains_recording_readback={contains_recording_readback}",
        logs.len()
    );
    assert!(contains_recording_readback);

    Ok(())
}

async fn activate_notepad_profile(client: &mut StdioMcpClient) -> anyhow::Result<()> {
    client
        .tools_call("profile_activate", json!({"profile_id": "notepad"}))
        .await?;
    Ok(())
}

fn structured<T: DeserializeOwned>(resp: &Value) -> anyhow::Result<T> {
    serde_json::from_value(resp["structuredContent"].clone()).context("decode structuredContent")
}

fn error_code(error: &Value) -> Option<&str> {
    error
        .get("data")
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
}

fn read_logs(path: &Path) -> anyhow::Result<String> {
    let mut logs = String::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        if entry.metadata()?.is_file() {
            logs.push_str(&std::fs::read_to_string(entry.path())?);
        }
    }
    Ok(logs)
}

#[derive(serde::Deserialize)]
struct ActStrokeWireResponse {
    ok: bool,
    path_kind: String,
    point_stream_count: u32,
    path_length_px: f64,
    duration_ms: f64,
    backend_used: String,
}
