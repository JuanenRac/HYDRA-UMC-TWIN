// HYDRA-UMC-TWIN - tests/cli_modes.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0-or-later - see LICENSE
//
// What a caller of the binary sees: the identity banner, the exit code and
// output of the family checks on an empty workspace, the refusals of
// `validate-state`, the fatal start-up errors of `serve` (none of these
// starts a server), and `keygen` never overwriting a key.

use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("twin-cli-modes-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(args: &[&str]) -> (Option<i32>, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_hydra-umc-twin"))
        .args(args)
        .env_remove("HYDRA_UMC_TWIN_SIGNING_KEY_FILE")
        .env_remove("HYDRA_UMC_TWIN_CERTIFY_TOKEN")
        .env_remove("HYDRA_UMC_TWIN_HEALTH_PROBES")
        .env_remove("HYDRA_UMC_TWIN_PHYSICS_REPLICA_BIN")
        .env_remove("HYDRA_UMC_TWIN_HIL_ADDR")
        .output()
        .unwrap();
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

fn p(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

#[test]
fn no_arguments_or_an_unknown_command_print_the_identity() {
    for args in [vec![], vec!["nonsense"]] {
        let (code, stdout, _) = run(&args);
        assert_eq!(code, Some(0));
        assert!(stdout.starts_with("HYDRA-UMC-TWIN v"), "{stdout}");
        assert!(stdout.contains("Digital Twin integration hub"), "{stdout}");
    }
}

#[test]
fn family_status_on_an_empty_workspace_reports_every_child_missing() {
    let ws = scratch("status");
    let (code, stdout, _) = run(&["family-status", "--workspace", &p(&ws)]);
    assert_eq!(code, Some(1), "{stdout}");
    assert!(stdout.contains("Digital Twin family status"), "{stdout}");
    assert!(stdout.contains("NOT FOUND (expected at"), "{stdout}");
    assert!(
        stdout.contains("Some children are missing or unusable"),
        "{stdout}"
    );
    assert!(!stdout.contains("All "), "{stdout}");
}

#[test]
fn family_sync_on_an_empty_workspace_reports_every_child_missing() {
    let ws = scratch("sync");
    let (code, stdout, _) = run(&["family-sync", "--workspace", &p(&ws)]);
    assert_eq!(code, Some(1), "{stdout}");
    assert!(stdout.contains("MISSING (not checked out)"), "{stdout}");
    assert!(stdout.contains("Not every child is sync-ready"), "{stdout}");
}

#[test]
fn an_unusable_child_manifest_is_named_not_skipped() {
    let ws = scratch("unusable");
    let (_, stdout, _) = run(&["family-status", "--workspace", &p(&ws)]);
    let first_child = stdout
        .lines()
        .find(|l| l.contains("NOT FOUND"))
        .and_then(|l| l.trim().split(':').next())
        .unwrap()
        .to_string();
    let child_dir = ws.join(&first_child);
    std::fs::create_dir_all(&child_dir).unwrap();
    std::fs::write(child_dir.join("hydra-umc.project.json"), "{broken").unwrap();
    let (code, stdout, _) = run(&["family-status", "--workspace", &p(&ws)]);
    assert_eq!(code, Some(1));
    assert!(
        stdout.contains(&format!("{first_child}: UNUSABLE - ")),
        "{stdout}"
    );
}

#[test]
fn family_health_checks_its_timeout_and_never_hides_unhealthy_children() {
    let ws = scratch("health");
    for bad in ["abc", "99", "60001"] {
        let (code, _, stderr) =
            run(&["family-health", "--workspace", &p(&ws), "--timeout-ms", bad]);
        assert_eq!(code, Some(2), "{bad}: {stderr}");
        assert!(
            stderr.contains("--timeout-ms must be a whole number from 100 to 60000"),
            "{stderr}"
        );
    }
    let (code, stdout, _) = run(&[
        "family-health",
        "--workspace",
        &p(&ws),
        "--hil-bridge-addr",
        "127.0.0.1:1",
        "--timeout-ms",
        "100",
    ]);
    assert_eq!(code, Some(1), "{stdout}");
    assert!(stdout.contains("HIL bridge at 127.0.0.1:1"), "{stdout}");
    assert!(
        stdout.contains("Not every child was found healthy"),
        "{stdout}"
    );
}

#[test]
fn validate_state_refuses_missing_unreadable_and_non_json_input() {
    let (code, _, stderr) = run(&["validate-state"]);
    assert_eq!(code, Some(2));
    assert!(
        stderr.contains("usage: hydra-umc-twin validate-state"),
        "{stderr}"
    );

    let (code, _, stderr) = run(&["validate-state", "no-such-state.json"]);
    assert_eq!(code, Some(2));
    assert!(stderr.contains("cannot read"), "{stderr}");

    let dir = scratch("state");
    let junk = dir.join("junk.json");
    std::fs::write(&junk, "not json").unwrap();
    let (code, _, stderr) = run(&["validate-state", &p(&junk)]);
    assert_eq!(code, Some(2));
    assert!(stderr.contains("is not JSON"), "{stderr}");
}

#[test]
fn validate_state_fails_an_empty_object_with_a_problem_count() {
    let dir = scratch("state-empty");
    let empty = dir.join("empty.json");
    std::fs::write(&empty, "{}").unwrap();
    let (code, stdout, _) = run(&["validate-state", &p(&empty)]);
    assert_eq!(code, Some(1), "{stdout}");
    let last = stdout.lines().last().unwrap();
    assert!(last.starts_with("TWIN_STATE=FAIL problems="), "{stdout}");
    assert!(!last.ends_with("problems=0"), "{stdout}");
}

#[test]
fn serve_refuses_inconsistent_options_before_binding_any_port() {
    let ws = scratch("serve");
    let w = p(&ws);
    let cases: Vec<(Vec<&str>, &str)> = vec![
        (
            vec![
                "serve",
                "--workspace",
                &w,
                "--allow-any-workspace",
                "--strict-workspace-root",
                &w,
            ],
            "cannot be used together",
        ),
        (
            vec![
                "serve",
                "--workspace",
                &w,
                "--strict-workspace-root",
                "no-such-root-folder",
            ],
            "does not exist or is not accessible",
        ),
        (
            vec!["serve", "--workspace", &w, "--workers", "0"],
            "--workers must be a whole number from 1 to 256",
        ),
        (
            vec!["serve", "--workspace", &w, "--workers", "300"],
            "--workers must be a whole number from 1 to 256",
        ),
        (
            vec!["serve", "--workspace", &w, "--require-fresh-state-ms", "0"],
            "--require-fresh-state-ms must be a whole number",
        ),
        (
            vec![
                "serve",
                "--workspace",
                &w,
                "--require-fresh-state-ms",
                "5000",
            ],
            "needs the signing key and the certify token",
        ),
        (
            vec!["serve", "--workspace", &w, "--signing-key-file", "key.txt"],
            "--signing-key-file needs --certify-token-file",
        ),
        (
            vec![
                "serve",
                "--workspace",
                &w,
                "--health-probes",
                "--health-cache-ms",
                "99999999",
            ],
            "--health-cache-ms must be a whole number",
        ),
        (
            vec![
                "serve",
                "--workspace",
                &w,
                "--health-probes",
                "--timeout-ms",
                "5",
            ],
            "--timeout-ms must be a whole number from 100 to 60000",
        ),
    ];
    for (args, fragment) in cases {
        let (code, _, stderr) = run(&args);
        assert_eq!(code, Some(2), "{args:?}: {stderr}");
        assert!(stderr.contains(fragment), "{args:?}: {stderr}");
        assert!(!stderr.contains("HTTP API listening"), "{args:?}: {stderr}");
    }
}

#[test]
fn a_short_certify_token_is_refused() {
    let dir = scratch("token");
    let token = dir.join("token.txt");
    std::fs::write(&token, "short").unwrap();
    let key = dir.join("key.txt");
    let (code, _, stderr) = run(&[
        "serve",
        "--signing-key-file",
        &p(&key),
        "--certify-token-file",
        &p(&token),
    ]);
    assert_eq!(code, Some(2));
    assert!(stderr.contains("at least 16 characters"), "{stderr}");

    let (code, _, stderr) = run(&["serve", "--certify-token-file", &p(&dir.join("absent.txt"))]);
    assert_eq!(code, Some(2));
    assert!(
        stderr.contains("cannot read --certify-token-file"),
        "{stderr}"
    );
}

#[test]
fn keygen_writes_a_key_once_and_never_overwrites_it() {
    let (code, _, stderr) = run(&["keygen"]);
    assert_eq!(code, Some(2));
    assert!(
        stderr.contains("usage: hydra-umc-twin keygen --key-file PATH"),
        "{stderr}"
    );

    let dir = scratch("keygen");
    let key = dir.join("signing.key");
    let (code, stdout, stderr) = run(&["keygen", "--key-file", &p(&key)]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stdout.contains("signing key written to"), "{stdout}");
    assert!(
        stdout.contains("key id:") && stdout.contains("public key:"),
        "{stdout}"
    );
    let seed = std::fs::read_to_string(&key).unwrap();
    assert_eq!(seed.trim().len(), 64, "a 32-byte seed in hex");
    assert!(
        !stdout.contains(seed.trim()),
        "the private seed is never printed"
    );

    let (code, _, stderr) = run(&["keygen", "--key-file", &p(&key)]);
    assert_eq!(code, Some(1));
    assert!(
        stderr.contains("an existing key is never overwritten"),
        "{stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(&key).unwrap(),
        seed,
        "the key file is untouched"
    );
}
