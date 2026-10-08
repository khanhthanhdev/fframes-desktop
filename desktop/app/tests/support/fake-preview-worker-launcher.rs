use std::{
    env,
    process::{Command, Stdio},
};

fn main() {
    let mut worker = Command::new("python3");
    worker
        .arg("fake-preview-worker.py")
        .args(env::args_os().skip(1))
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let status = worker.status().unwrap_or_else(|error| {
        eprintln!("cannot start fake preview worker: {error}");
        std::process::exit(127);
    });
    std::process::exit(status.code().unwrap_or(1));
}
