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

mod certificate;
mod contract;
mod family;
mod manifest;
mod server;

use contract::SyncReadiness;
use family::FamilySyncOutcome;

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

const PROJECT_NAME: &str = "HYDRA-UMC-TWIN";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const ROLE: &str =
    "Digital Twin integration hub: family readiness and state-sync contract for PHYSICS-REPLICA, HIL-BRIDGE and SYNTHETIC-DATA-GEN (physics engine planned).";

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
        } else if let Some(problem) = &status.problem {
            println!("  {}: UNUSABLE - {}", status.name, problem);
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
        println!(
            "Some children are missing or unusable - see the NOT FOUND and UNUSABLE lines above."
        );
        ExitCode::from(1)
    }
}

fn run_family_sync(args: &[String]) -> ExitCode {
    let workspace: PathBuf = find_flag(args, "--workspace")
        .map(PathBuf::from)
        .unwrap_or_else(default_workspace);

    println!(
        "Digital Twin family sync contract (workspace: {}):",
        workspace.display()
    );

    let statuses = family::assess_family_sync(&workspace);
    for status in &statuses {
        match &status.outcome {
            FamilySyncOutcome::Missing => {
                println!("  {}: MISSING (not checked out)", status.name);
            }
            FamilySyncOutcome::Unusable { reason } => {
                println!("  {}: UNUSABLE - {reason}", status.name);
            }
            FamilySyncOutcome::Assessed(SyncReadiness::Ready(snapshot)) => {
                println!(
                    "  {}: READY (v{}, maturity={})",
                    status.name, snapshot.version, snapshot.maturity
                );
            }
            FamilySyncOutcome::Assessed(SyncReadiness::ImmatureMaturity { reason }) => {
                println!("  {}: REJECTED (immature) - {reason}", status.name);
            }
            FamilySyncOutcome::Assessed(SyncReadiness::IncompatibleVersion { reason }) => {
                println!(
                    "  {}: REJECTED (incompatible version) - {reason}",
                    status.name
                );
            }
            FamilySyncOutcome::Assessed(SyncReadiness::UnparseableManifest { reason }) => {
                println!(
                    "  {}: REJECTED (unparseable manifest) - {reason}",
                    status.name
                );
            }
        }
    }
    println!();
    if statuses.iter().all(|s| s.outcome.is_ready()) {
        println!("All {} children are sync-ready.", statuses.len());
        ExitCode::SUCCESS
    } else {
        println!("Not every child is sync-ready - see the lines above.");
        ExitCode::from(1)
    }
}

/// The key and the token of `POST /interlock/certify`, from `--signing-key-file` / `--certify-token-file` (or HYDRA_UMC_TWIN_CERTIFY_TOKEN).
/// Neither: the route is off. One without the other is an error, so a half-configured server never starts.
fn load_certifier(args: &[String]) -> Result<Option<server::Certifier>, String> {
    let key_file = find_flag(args, "--signing-key-file")
        .or_else(|| env::var("HYDRA_UMC_TWIN_SIGNING_KEY_FILE").ok());
    let token = match find_flag(args, "--certify-token-file") {
        Some(path) => Some(
            std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read --certify-token-file: {e}"))?
                .trim()
                .to_string(),
        ),
        None => env::var("HYDRA_UMC_TWIN_CERTIFY_TOKEN")
            .ok()
            .map(|t| t.trim().to_string()),
    };
    match (key_file, token) {
        (None, None) => Ok(None),
        (Some(_), None) => Err("--signing-key-file needs --certify-token-file (or HYDRA_UMC_TWIN_CERTIFY_TOKEN): without a token anyone who reaches the port could ask for a signature".into()),
        (None, Some(_)) => Err("a certify token needs --signing-key-file".into()),
        (Some(path), Some(token)) => {
            if token.chars().count() < 16 {
                return Err("the certify token must have at least 16 characters".into());
            }
            let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read --signing-key-file: {e}"))?;
            let signer = certificate::Signer::from_seed_hex(&text)?;
            Ok(Some(server::Certifier { signer, token }))
        }
    }
}

/// `keygen --key-file PATH`: write a new signing key (never over an existing file) and print the public key to give to the bridge.
fn run_keygen(args: &[String]) -> ExitCode {
    let Some(path) = find_flag(args, "--key-file") else {
        eprintln!("usage: hydra-umc-twin keygen --key-file PATH");
        return ExitCode::from(2);
    };
    let seed = match certificate::random_seed_hex() {
        Ok(seed) => seed,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(1);
        }
    };
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = options.open(&path).and_then(|mut file| {
        use std::io::Write;
        writeln!(file, "{seed}")
    });
    if let Err(e) = written {
        eprintln!("error: cannot create {path} (an existing key is never overwritten): {e}");
        return ExitCode::from(1);
    }
    let signer =
        certificate::Signer::from_seed_hex(&seed).expect("a freshly generated seed is valid");
    println!("signing key written to {path}");
    println!("key id:     {}", signer.key_id());
    println!("public key: {}", signer.public_key_hex());
    ExitCode::SUCCESS
}

fn run_serve(args: &[String]) -> ExitCode {
    let addr = find_flag(args, "--addr").unwrap_or_else(|| "127.0.0.1".to_string());
    let port = find_flag(args, "--port").unwrap_or_else(|| "8111".to_string());
    let workspace: PathBuf = find_flag(args, "--workspace")
        .map(PathBuf::from)
        .unwrap_or_else(default_workspace);
    // Opt-in production hardening: unset (the default), a `?workspace=`
    // override can point at any real directory the process can read -
    // the deliberate, tested behavior this API has always had, kept
    // unchanged. Set, every override must resolve inside this root -
    // see server.rs's own resolve_workspace_override() for the full
    // rationale. Canonicalized at startup, not per-request: a root that
    // doesn't exist is a real, loud startup error, not a silent
    // always-reject once the server is already listening.
    let strict_root: Option<PathBuf> = match find_flag(args, "--strict-workspace-root") {
        Some(raw) => match PathBuf::from(&raw).canonicalize() {
            Ok(canonical) => Some(canonical),
            Err(e) => {
                eprintln!("[twin] fatal: --strict-workspace-root {raw} does not exist or is not accessible: {e}");
                return ExitCode::from(2);
            }
        },
        None => None,
    };

    // The interlock certificates (certificate.rs): off unless a signing key AND a token for the callers that ask for a signature are given.
    let certifier = match load_certifier(args) {
        Ok(certifier) => certifier,
        Err(message) => {
            eprintln!("[twin] fatal: {message}");
            return ExitCode::from(2);
        }
    };

    // How many requests are served at once (default 8): a slow one holds one worker, not the server.
    let workers: usize = match find_flag(args, "--workers") {
        Some(raw) => match raw.parse::<usize>() {
            Ok(n) if (1..=256).contains(&n) => n,
            _ => {
                eprintln!(
                    "[twin] fatal: --workers must be a whole number from 1 to 256, got {raw}"
                );
                return ExitCode::from(2);
            }
        },
        None => server::DEFAULT_WORKERS,
    };

    let bind_addr = format!("{addr}:{port}");
    match server::bind(&bind_addr) {
        Ok(bound) => {
            eprintln!(
                "[twin] HTTP API listening on {bind_addr} (workspace={})",
                workspace.display()
            );
            match &strict_root {
                Some(root) => eprintln!("[twin] --strict-workspace-root set: ?workspace= overrides are bounded to {}", root.display()),
                None => eprintln!("[twin] --strict-workspace-root not set: ?workspace= overrides may point at any real, readable directory"),
            }
            eprintln!("[twin] GET /family-status, GET /family-sync, GET /stats");
            match &certifier {
                Some(c) => eprintln!("[twin] POST /interlock/certify signs risk reports with key {} (public key {})", c.signer.key_id(), c.signer.public_key_hex()),
                None => eprintln!("[twin] POST /interlock/certify is off: no --signing-key-file and --certify-token-file"),
            }
            eprintln!("[twin] serving with {workers} worker thread(s)");
            server::run_with_workers(bound, workspace, strict_root, certifier, workers);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("[twin] fatal: could not start HTTP server on {bind_addr}: {e}");
            ExitCode::from(2)
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();

    match args.first().map(|s| s.as_str()) {
        Some("family-status") => run_family_status(&args[1..]),
        Some("family-sync") => run_family_sync(&args[1..]),
        Some("serve") => run_serve(&args[1..]),
        Some("keygen") => run_keygen(&args[1..]),
        _ => {
            println!("{PROJECT_NAME} v{VERSION}");
            println!("{ROLE}");
            ExitCode::SUCCESS
        }
    }
}
