use clap::{Parser, ValueEnum};

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum Mode {
    Stdio,
    Http,
}

#[derive(Debug, Parser)]
#[command(name = "synapse-mcp", version, about = "Synapse MCP server")]
struct Cli {
    #[arg(long, value_enum, default_value_t = Mode::Stdio)]
    mode: Mode,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    synapse_telemetry::init()?;
    tracing::info!(mode = ?cli.mode, "synapse-mcp scaffold started");
    Ok(())
}
