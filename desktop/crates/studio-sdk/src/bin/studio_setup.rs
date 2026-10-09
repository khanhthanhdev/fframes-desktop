use std::path::PathBuf;
use studio_sdk::{CompatibilityManifest, Doctor, ProjectManager};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        print_usage();
        return Ok(());
    }

    match args[1].as_str() {
        "doctor" => {
            let json_mode = args.iter().any(|a| a == "--json");
            let manifest = CompatibilityManifest::default_linux_x64();
            let report = Doctor::run_host_preflight(&manifest);

            if json_mode {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("{}", report.format_summary());
            }

            if !report.is_ready() {
                std::process::exit(1);
            }
        }
        "new" => {
            let mut name = "studio-video".to_string();
            let mut dir = PathBuf::from("./studio-video");

            let mut i = 2;
            while i < args.len() {
                if args[i] == "--name" && i + 1 < args.len() {
                    name = args[i + 1].clone();
                    i += 2;
                } else if args[i] == "--dir" && i + 1 < args.len() {
                    dir = PathBuf::from(&args[i + 1]);
                    i += 2;
                } else {
                    i += 1;
                }
            }

            println!("Creating new project '{name}' at {:?}", dir);
            let manifest = CompatibilityManifest::default_linux_x64();
            let created =
                ProjectManager::generate_cpu_project(&name, &dir, &manifest.fframes_version)?;
            println!("Project generated successfully at {:?}", created);
        }
        "help" | "--help" | "-h" => {
            print_usage();
        }
        other => {
            eprintln!("Unknown command: {other}");
            print_usage();
            std::process::exit(1);
        }
    }

    Ok(())
}

fn print_usage() {
    println!("studio_setup - GPUI-free setup and diagnostic companion for fframes studio");
    println!("Usage:");
    println!("  studio_setup doctor [--json]                  Run host prerequisite doctor");
    println!("  studio_setup new --name <name> --dir <path>   Generate CPU fframes project");
    println!("  studio_setup help                             Show this help message");
}
