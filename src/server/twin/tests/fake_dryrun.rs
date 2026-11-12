// =============================================================================
// HYDRA-UMC-TWIN - src/server/twin/tests/fake_dryrun.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0-or-later - see LICENSE
// =============================================================================
//! The dry run of a trajectory with a stand-in for the `check-trajectory` child, so that it is run where the real binary of HYDRA-UMC-PHYSICS-REPLICA is
//! not built: the verdict for each answer of the child, the status each failure of the child becomes, what a bad request gets, and the signature of the
//! verdict. Unix only: the stand-in is a shell script.

use super::*;
use std::os::unix::fs::PermissionsExt;

/// A program that answers `check-trajectory` the way `mode` says.
fn fake_checker(tag: &str, mode: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("twin-fake-checker-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("fake-checker");
    let body = format!(
        r#"#!/bin/sh
mode={mode}
case "$mode" in
  clean) echo '{{"ok":true,"complete":true}}'; exit 0 ;;
  partial) echo '{{"ok":true,"complete":false}}'; exit 0 ;;
  broken) echo '{{"ok":false,"complete":true}}'; exit 1 ;;
  liar) echo '{{"ok":true,"complete":true}}'; exit 1 ;;
  nojson) echo 'not json'; exit 0 ;;
  nofields) echo '{{"ok":true}}'; exit 0 ;;
  refuse) echo "check-trajectory: unknown joint zz" >&2; exit 2 ;;
  refuse_plain) echo "something else" >&2; exit 2 ;;
  crash) exit 9 ;;
  slow) sleep 5; exit 0 ;;
esac
"#
    );
    std::fs::write(&path, body).unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&path, permissions).unwrap();
    path
}

fn dryrun_server(tag: &str, mode: &str, require_collision: bool, timeout_ms: u64) -> u16 {
    let route = DryRunRoute::new(
        fake_checker(tag, mode),
        urdf_file(tag),
        None,
        "arm".into(),
        crate::twincfg::DryRunCfg {
            timeout_ms,
            require_collision_check: require_collision,
            ..Default::default()
        },
    );
    start(
        Setup {
            extras: Extras {
                dryrun: Some(route),
                ..Extras::default()
            },
            sim: false,
            collision: false,
        },
        None,
        tag,
    )
}

const SWING: &str = r#"{"samples":[
    {"t":0.0,"joints":{"shoulder":1.5,"elbow":0.0}},
    {"t":0.5,"joints":{"shoulder":1.6,"elbow":0.0}},
    {"t":1.0,"joints":{"shoulder":1.7,"elbow":0.0}}]}"#;

fn post(port: u16, path: &str, body: &str) -> (u16, serde_json::Value) {
    let (status, text) = request(port, "POST", path, &auth(), body);
    (status, json_of(&text))
}

#[test]
fn a_clean_run_is_a_signed_pass_bound_to_the_trajectory_and_it_changes_with_the_trajectory() {
    let port = dryrun_server("fd-clean", "clean", false, 20_000);
    let (status, answer) = post(port, "/dryrun/trajectory", SWING);
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["verdict"], "pass");
    assert_eq!(answer["robot_id"], "arm");
    assert_eq!(answer["samples"], 3);
    assert_eq!(answer["trajectory_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(answer["signature"].as_str().unwrap().len(), 128);
    assert_eq!(answer["checks"]["collision"]["checked"], false);
    assert!(answer["valid_until_ms"].as_u64().unwrap() > answer["issued_at_ms"].as_u64().unwrap());
    let (_, again) = post(port, "/dryrun/trajectory", SWING);
    assert_eq!(again["verdict"], "pass");
    // a nonce of its own for each verdict: two answers for the same trajectory are not the same document
    assert_ne!(answer, again);
    let longer = SWING.replace("1.7", "1.75");
    let (_, other) = post(port, "/dryrun/trajectory", &longer);
    assert_eq!(other["verdict"], "pass");
    assert_ne!(
        other["trajectory_sha256"], answer["trajectory_sha256"],
        "{other}"
    );
}

#[test]
fn what_the_child_found_decides_the_verdict_and_nothing_is_given_the_benefit_of_the_doubt() {
    for (mode, require_collision, verdict) in [
        ("broken", false, "fail"),
        ("partial", false, "incomplete"),
        // a clean answer is still not a pass when the collision prediction was demanded and did not run
        ("clean", true, "incomplete"),
    ] {
        let port = dryrun_server(
            &format!("fd-{mode}-{require_collision}"),
            mode,
            require_collision,
            20_000,
        );
        let (status, answer) = post(port, "/dryrun/trajectory", SWING);
        assert_eq!(status, 200, "{mode}: {answer}");
        assert_eq!(answer["verdict"], verdict, "{mode}: {answer}");
    }
}

#[test]
fn a_child_that_lies_or_fails_gives_no_verdict_at_all() {
    for (mode, status, fragment) in [
        ("liar", 502, "does not agree with its exit status"),
        ("nojson", 502, "is not JSON"),
        ("nofields", 502, "has no ok or complete"),
        ("refuse", 422, "check-trajectory: unknown joint zz"),
        ("refuse_plain", 422, "something else"),
        ("crash", 502, "status Some(9)"),
    ] {
        let port = dryrun_server(&format!("fd-{mode}"), mode, false, 20_000);
        let (got, answer) = post(port, "/dryrun/trajectory", SWING);
        assert_eq!(got, status, "{mode}: {answer}");
        assert!(
            answer["error"].as_str().unwrap().contains(fragment),
            "{mode}: {answer}"
        );
        assert!(
            answer.get("verdict").is_none(),
            "{mode}: no verdict is given: {answer}"
        );
    }
    let port = dryrun_server("fd-slow", "slow", false, 150);
    let (status, answer) = post(port, "/dryrun/trajectory", SWING);
    assert_eq!(status, 504, "{answer}");
    assert_eq!(answer["error"], "the check did not end within 150 ms");
}

#[test]
fn a_request_that_is_not_a_trajectory_is_a_400_and_the_child_is_never_asked() {
    let port = dryrun_server("fd-bad", "crash", false, 20_000);
    for body in [
        "not json",
        "{}",
        r#"{"samples":[]}"#,
        r#"{"samples":[{"t":0.0,"joints":{"shoulder":1.0}}]}"#,
        r#"{"samples":[{"t":1.0,"joints":{"shoulder":1.0}},{"t":1.0,"joints":{"shoulder":1.1}}]}"#,
        r#"{"samples":[{"t":0.0,"joints":{}},{"t":1.0,"joints":{"shoulder":1.1}}]}"#,
        r#"{"samples":[{"t":0.0,"joints":{"shoulder":"far"}},{"t":1.0,"joints":{"shoulder":1.1}}]}"#,
        r#"{"samples":[{"t":"now","joints":{"shoulder":1.0}},{"t":1.0,"joints":{"shoulder":1.1}}]}"#,
    ] {
        let (status, answer) = post(port, "/dryrun/trajectory", body);
        assert_eq!(status, 400, "{body}: {answer}");
        assert!(answer["error"].is_string(), "{body}: {answer}");
    }
}

#[test]
fn the_route_needs_the_token_and_a_post() {
    let port = dryrun_server("fd-auth", "clean", false, 20_000);
    assert_eq!(
        request(port, "POST", "/dryrun/trajectory", "", SWING).0,
        401
    );
    assert_eq!(
        request(
            port,
            "POST",
            "/dryrun/trajectory",
            "Authorization: Bearer not-the-token\r\n",
            SWING
        )
        .0,
        401
    );
    assert_ne!(
        request(port, "GET", "/dryrun/trajectory", &auth(), "").0,
        200
    );
}

#[test]
fn the_verdict_speaks_the_language_asked() {
    let port = dryrun_server("fd-lang", "clean", false, 20_000);
    let (_, english) = post(port, "/dryrun/trajectory?lang=en", SWING);
    let (_, spanish) = post(port, "/dryrun/trajectory?lang=es", SWING);
    let (_, japanese) = post(port, "/dryrun/trajectory?lang=ja", SWING);
    assert_ne!(english["message"], spanish["message"]);
    assert_ne!(english["message"], japanese["message"]);
    assert_eq!(
        english["verdict"], spanish["verdict"],
        "what a program reads stays English"
    );
}
