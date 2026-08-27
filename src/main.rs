// =============================================================================
// HYDRA-UMC-TWIN - src/main.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0 - see LICENSE
// =============================================================================
//! Entry point for HYDRA-UMC-TWIN.
//!
//! Bare invocation (no arguments) is unchanged: prints identity, version
//! and role, exits 0.
//!
//! The real `family-status` subcommand runs this project's actual v0
//! readiness check - honest for an integration hub that runs no physics
//! or rendering engine itself yet. See `manifest.rs`/`family.rs` for what
//! "real" means here, and their own module docs for what is still out of
//! scope (the real Bevy engine and physics backend).

mod family;
mod manifest;

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

const PROJECT_NAME: &str = "HYDRA-UMC-TWIN";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const ROLE: &str =
    "Physics-based Digital Twin engine for safe robotic simulation (integration parent).";

fn find_flag(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// The real sibling-checkout layout this whole ecosystem already uses:
/// every repo is a sibling directory under one workspace folder. Since
/// `run.sh`/`run.bat` `cd` into this repo's own directory before running
/// the binary, the current directory's parent is that workspace by
/// default.
fn default_workspace() -> PathBuf {
    env::current_dir()
        .ok()
        .and_then(|d| d.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn run_family_status(args: &[String]) -> ExitCode {
    let workspace: PathBuf = find_flag(args, "--workspace")
        .map(PathBuf::from)
        .unwrap_or_else(default_workspace);

    println!(
        "Digital Twin family status (workspace: {}):",
        workspace.display()
    );

    let statuses = family::check_family_status(&workspace);
    for status in &statuses {
        if status.is_present() {
            let m = status.manifest.as_ref().unwrap();
            println!(
                "  {}: v{}, maturity={}, role={}",
                status.name, m.version, m.maturity, m.role
            );
        } else {
            println!(
                "  {}: NOT FOUND (expected at {})",
                status.name,
                workspace.join(&status.name).display()
            );
        }
    }
    println!();
    if statuses.iter().all(|s| s.is_present()) {
        println!("All {} children present.", statuses.len());
        ExitCode::SUCCESS
    } else {
        println!("Some children are missing - see NOT FOUND lines above.");
        ExitCode::from(1)
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();

    match args.first().map(|s| s.as_str()) {
        Some("family-status") => run_family_status(&args[1..]),
        _ => {
            println!("{PROJECT_NAME} v{VERSION}");
            println!("{ROLE}");
            ExitCode::SUCCESS
        }
    }
}
