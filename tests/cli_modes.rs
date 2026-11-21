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

#[test]
fn the_collision_prediction_is_off_and_refuses_half_configurations() {
    let dir = scratch("collision");
    let w = p(&dir);
    // the options of the prediction mean nothing without its switch
    for flag in [
        "--collision-urdf",
        "--collision-zones",
        "--collision-config",
        "--collision-timeout-ms",
        "--collision-max-states",
    ] {
        let (code, _, stderr) = run(&["serve", "--workspace", &w, flag, "1"]);
        assert_eq!(code, Some(2), "{flag}: {stderr}");
        assert!(
            stderr.contains("needs --collision-predict"),
            "{flag}: {stderr}"
        );
    }
    // the switch needs the token of the interlock routes
    let (code, _, stderr) = run(&["serve", "--workspace", &w, "--collision-predict"]);
    assert_eq!(code, Some(2));
    assert!(
        stderr.contains("needs the signing key and the certify token"),
        "{stderr}"
    );
    // with a key and a token, every file must be given and exist
    let key = dir.join("key.txt");
    let (code, _, _) = run(&["keygen", "--key-file", &p(&key)]);
    assert_eq!(code, Some(0));
    let token = dir.join("token.txt");
    std::fs::write(&token, "a-token-of-at-least-16-characters").unwrap();
    let base = [
        "serve",
        "--workspace",
        w.as_str(),
        "--signing-key-file",
        &p(&key),
        "--certify-token-file",
        &p(&token),
        "--collision-predict",
    ];
    let (code, _, stderr) = run(&base);
    assert_eq!(code, Some(2));
    assert!(stderr.contains("--collision-urdf"), "{stderr}");
    let missing = p(&dir.join("absent.json"));
    let mut args = base.to_vec();
    args.extend([
        "--collision-urdf",
        &missing,
        "--collision-zones",
        &missing,
        "--collision-config",
        &missing,
    ]);
    let (code, _, stderr) = run(&args);
    assert_eq!(code, Some(2));
    assert!(stderr.contains("is not a file"), "{stderr}");
    // and a bad number is named
    let real = p(&token);
    let mut args = base.to_vec();
    args.extend([
        "--collision-urdf",
        &real,
        "--collision-zones",
        &real,
        "--collision-config",
        &real,
        "--physics-replica-bin",
        &real,
        "--collision-timeout-ms",
        "5",
    ]);
    let (code, _, stderr) = run(&args);
    assert_eq!(code, Some(2));
    assert!(
        stderr.contains("--collision-timeout-ms must be a whole number from 100 to 60000"),
        "{stderr}"
    );
}

#[test]
fn the_websocket_and_the_vehicle_input_are_off_and_refuse_half_configurations() {
    let dir = scratch("ws-uav");
    let w = p(&dir);
    // the websocket pushes what the poses and the vehicle input hold: without either it has nothing to say
    let (code, _, stderr) = run(&["serve", "--workspace", &w, "--ws"]);
    assert_eq!(code, Some(2));
    assert!(
        stderr.contains("--ws needs --poses or --uav-input"),
        "{stderr}"
    );
    // a switch needs the token of the routes and the configuration file
    let (code, _, stderr) = run(&["serve", "--workspace", &w, "--uav-input"]);
    assert_eq!(code, Some(2));
    assert!(stderr.contains("--uav-input"), "{stderr}");
    assert!(stderr.contains("signing key"), "{stderr}");
    let key = dir.join("key.txt");
    let token = dir.join("token.txt");
    std::fs::write(&token, "a-certify-token-of-sixteen+\n").unwrap();
    let (code, _, stderr) = run(&["keygen", "--key-file", &p(&key)]);
    assert_eq!(code, Some(0), "{stderr}");
    let base = [
        "serve",
        "--workspace",
        &w,
        "--signing-key-file",
        &p(&key),
        "--certify-token-file",
        &p(&token),
        "--uav-input",
    ];
    let (code, _, stderr) = run(&base);
    assert_eq!(code, Some(2));
    assert!(stderr.contains("--twin-config FILE"), "{stderr}");
    // a configuration that listens on every interface for the websocket is refused unless it says so
    let config = dir.join("twin.json");
    std::fs::write(&config, r#"{"ws":{"listen":"0.0.0.0:0"}}"#).unwrap();
    let mut args = base.to_vec();
    let config_arg = p(&config);
    args.extend(["--ws", "--twin-config", &config_arg]);
    let (code, _, stderr) = run(&args);
    assert_eq!(code, Some(2));
    assert!(stderr.contains("ws.allow_remote"), "{stderr}");
    // and a field the file does not know is named
    std::fs::write(&config, r#"{"ws":{"port":1}}"#).unwrap();
    let (code, _, stderr) = run(&args);
    assert_eq!(code, Some(2));
    assert!(stderr.contains("port"), "{stderr}");
}

// -- verify-grasp: the verdict comes from the measured poses; a controller that says "done" proves nothing, and data that cannot
// support a verdict is never a pass (exit 3, not 0).

fn carry(object_z: f64, claim: bool) -> String {
    let samples: Vec<String> = (0..6)
        .map(|i| {
            let x = 40.0 * f64::from(i);
            format!(
                r#"{{"t_s": {}, "object": [{x}, 0, {object_z}], "gripper": [{x}, 0, {}]}}"#,
                0.1 * f64::from(i),
                object_z + 2.0
            )
        })
        .collect();
    format!(
        r#"{{"controller_reports_done": {claim}, "rest_object": [0, 0, 0], "transport": [{}], "destination": [200, 0, 0], "final_object": [200, 0, 0]}}"#,
        samples.join(",")
    )
}

#[test]
fn verify_grasp_says_verified_failed_or_unverified_by_its_exit_code() {
    let dir = scratch("verify");
    let good = dir.join("good.json");
    std::fs::write(&good, carry(50.0, true)).unwrap();
    let (code, stdout, _) = run(&["verify-grasp", &p(&good)]);
    assert_eq!(code, Some(0), "{stdout}");
    assert!(stdout.contains("VERIFY=VERIFIED"), "{stdout}");

    // the controller claims success and the object never left the table
    let stuck = dir.join("stuck.json");
    std::fs::write(&stuck, carry(0.0, true)).unwrap();
    let (code, stdout, _) = run(&["verify-grasp", &p(&stuck), "--check", "grasp"]);
    assert_eq!(code, Some(1), "{stdout}");
    assert!(stdout.contains("VERIFY=FAILED"), "{stdout}");
    assert!(stdout.contains("lift_too_low"), "{stdout}");

    // nothing measured: not a pass
    let empty = dir.join("empty.json");
    std::fs::write(&empty, r#"{"controller_reports_done": true}"#).unwrap();
    let (code, stdout, _) = run(&["verify-grasp", &p(&empty)]);
    assert_eq!(code, Some(3), "{stdout}");
    assert!(stdout.contains("VERIFY=UNVERIFIED"), "{stdout}");
}

#[test]
fn verify_grasp_reads_its_tolerances_from_the_configuration_file() {
    let dir = scratch("verify-config");
    let low = dir.join("low.json");
    std::fs::write(&low, carry(12.0, false)).unwrap();
    let (code, _, _) = run(&["verify-grasp", &p(&low), "--check", "grasp"]);
    assert_eq!(code, Some(1));
    let config = dir.join("verify.json");
    std::fs::write(&config, r#"{"min_lift_mm": 10}"#).unwrap();
    let (code, stdout, _) = run(&["verify-grasp", &p(&low), "--config", &p(&config)]);
    assert_eq!(code, Some(0), "{stdout}");
}

#[test]
fn verify_grasp_refuses_bad_usage_with_exit_code_two() {
    let dir = scratch("verify-usage");
    let (code, _, stderr) = run(&["verify-grasp"]);
    assert_eq!(code, Some(2));
    assert!(stderr.contains("usage"), "{stderr}");
    let (code, _, _) = run(&["verify-grasp", &p(&dir.join("missing.json"))]);
    assert_eq!(code, Some(2));
    let bad = dir.join("bad.json");
    std::fs::write(&bad, r#"{"success": true}"#).unwrap();
    let (code, _, stderr) = run(&["verify-grasp", &p(&bad)]);
    assert_eq!(code, Some(2));
    assert!(stderr.contains("not an observation"), "{stderr}");
    let good = dir.join("good.json");
    std::fs::write(&good, carry(50.0, true)).unwrap();
    let (code, _, _) = run(&["verify-grasp", &p(&good), "--check", "nonsense"]);
    assert_eq!(code, Some(2));
    let zero = dir.join("zero.json");
    std::fs::write(&zero, r#"{"hold_radius_mm": 0}"#).unwrap();
    let (code, _, _) = run(&["verify-grasp", &p(&good), "--config", &p(&zero)]);
    assert_eq!(code, Some(2));
}

// -- sweep: the synthetic scenario run over a sweep, as a Markdown or CSV table; an aborted sweep is never a result (exit 3).

#[test]
fn sweep_without_a_file_runs_the_default_sweep_and_prints_the_table() {
    let (code, stdout, stderr) = run(&["sweep"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stderr.contains("SWEEP=COMPLETE rows=24"), "{stderr}");
    assert!(stdout.starts_with("| kind | name |"), "{stdout}");
    assert!(
        stdout.contains("| nominal | nominal | 1 |  | 5 | 5 | 1.000 |"),
        "{stdout}"
    );
    assert!(!stdout.contains("ABORTED"), "{stdout}");
    // the same command gives the same table
    let (_, again, _) = run(&["sweep"]);
    assert_eq!(stdout, again);
}

#[test]
fn sweep_reads_a_file_writes_csv_and_never_overwrites_a_report() {
    let dir = scratch("sweep-file");
    let config = dir.join("sweep.json");
    std::fs::write(
        &config,
        r#"{"parameters": [{"name": "mass", "nominal": 0.6, "factors": [0.5, 2.0]}], "seeds": 3}"#,
    )
    .unwrap();
    let (code, stdout, _) = run(&["sweep", &p(&config), "--format", "csv"]);
    assert_eq!(code, Some(0));
    let mut lines = stdout.lines();
    assert_eq!(
        lines.next(),
        Some("kind,name,factor,value,runs,passed,pass_rate")
    );
    assert_eq!(lines.next(), Some("nominal,nominal,1,,3,3,1.000"));
    assert_eq!(lines.next(), Some("parameter,mass,0.5,0.3,3,3,1.000"));
    assert_eq!(lines.next(), Some("parameter,mass,2,1.2,3,0,0.000"));
    let out = dir.join("report.csv");
    let (code, stdout, _) = run(&[
        "sweep",
        &p(&config),
        "--format",
        "csv",
        "--output",
        &p(&out),
    ]);
    assert_eq!(code, Some(0));
    assert!(stdout.is_empty(), "{stdout}");
    assert!(std::fs::read_to_string(&out)
        .unwrap()
        .contains("parameter,mass,2,1.2,3,0,0.000"));
    // a second run does not overwrite it
    let (code, _, stderr) = run(&["sweep", &p(&config), "--output", &p(&out)]);
    assert_eq!(code, Some(2));
    assert!(stderr.contains("cannot write"), "{stderr}");
    assert!(std::fs::read_to_string(&out).unwrap().starts_with("kind,"));
}

#[test]
fn sweep_that_cannot_be_evaluated_is_aborted_with_exit_code_three() {
    let dir = scratch("sweep-abort");
    let config = dir.join("sweep.json");
    // the synthetic scenario has no such parameter: the sweep stops there and says so
    std::fs::write(
        &config,
        r#"{"parameters": [{"name": "colour", "nominal": 1.0}], "seeds": 2}"#,
    )
    .unwrap();
    let (code, stdout, stderr) = run(&["sweep", &p(&config)]);
    assert_eq!(code, Some(3), "{stderr}");
    assert!(
        stdout.contains("ABORTED: parameter colour seed 0"),
        "{stdout}"
    );
    assert!(stderr.contains("SWEEP=ABORTED"), "{stderr}");
    // data the checks cannot judge is also an abort, never a pass
    let verify = dir.join("verify.json");
    std::fs::write(&verify, r#"{"min_samples": 50}"#).unwrap();
    let (code, stdout, _) = run(&["sweep", "--verify-config", &p(&verify)]);
    assert_eq!(code, Some(3));
    assert!(stdout.contains("too_few_samples"), "{stdout}");
}

#[test]
fn sweep_refuses_bad_usage_with_exit_code_two() {
    let dir = scratch("sweep-usage");
    let (code, _, stderr) = run(&["sweep", "--nonsense", "x"]);
    assert_eq!(code, Some(2));
    assert!(stderr.contains("usage"), "{stderr}");
    let (code, _, _) = run(&["sweep", "--format"]);
    assert_eq!(code, Some(2));
    let (code, _, stderr) = run(&["sweep", "--format", "xml"]);
    assert_eq!(code, Some(2));
    assert!(stderr.contains("markdown or csv"), "{stderr}");
    let (code, _, _) = run(&["sweep", &p(&dir.join("missing.json"))]);
    assert_eq!(code, Some(2));
    let bad = dir.join("bad.json");
    std::fs::write(&bad, r#"{"seeds": 0}"#).unwrap();
    let (code, _, stderr) = run(&["sweep", &p(&bad)]);
    assert_eq!(code, Some(2));
    assert!(stderr.contains("seeds"), "{stderr}");
    let (code, _, _) = run(&["sweep", "--verify-config", &p(&dir.join("missing.json"))]);
    assert_eq!(code, Some(2));
}
