//! `studio-mcp`: stdio MCP server forwarding the six project tools to the app's local
//! tool broker. All behavior lives in `fframes_studio::agent_tools::mcp::run_stdio`.
use fframes_studio::agent_tools::{client::CAPABILITY_ENV, mcp::run_stdio};
use std::process::ExitCode;

fn main() -> ExitCode {
    let code = run_stdio(
        std::env::args_os().skip(1).collect(),
        std::env::var_os(CAPABILITY_ENV),
        &mut std::io::stdin().lock(),
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    );
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}
