//! `studio-tools`: command-line facade over the app's local tool broker. All behavior
//! lives in `fframes_studio::agent_tools::client::run_cli`.
use fframes_studio::agent_tools::client::{CAPABILITY_ENV, run_cli};
use std::process::ExitCode;

fn main() -> ExitCode {
    let code = run_cli(
        std::env::args_os().skip(1).collect(),
        std::env::var_os(CAPABILITY_ENV),
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    );
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}
