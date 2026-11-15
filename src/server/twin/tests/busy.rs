// =============================================================================
// HYDRA-UMC-TWIN - src/server/twin/tests/busy.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0-or-later - see LICENSE
// =============================================================================
//! The poses route and the dry run when every slot of the route is taken: the answer is `503`, it says why, and no child process is started (the
//! programs named below do not exist, so a started process would be a different answer). No child binary is needed, so this runs everywhere.

use super::*;

const SWING: &str = r#"{"samples":[
    {"t":0.0,"joints":{"shoulder":1.5,"elbow":0.0}},
    {"t":0.5,"joints":{"shoulder":1.6,"elbow":0.0}}]}"#;

#[test]
fn a_poses_request_while_every_slot_is_taken_is_a_503_and_starts_no_process() {
    let route = PoseRoute::new(
        PathBuf::from("no-such-program-for-the-poses"),
        urdf_file("busy-poses"),
        "arm".into(),
        crate::twincfg::PosesCfg {
            joints: vec!["shoulder".into(), "elbow".into()],
            max_age_ms: 60_000,
            timeout_ms: 20_000,
        },
    );
    route.occupy_every_slot();
    let port = start(
        Setup {
            extras: Extras {
                poses: Some(route),
                ..Extras::default()
            },
            sim: false,
            collision: false,
        },
        None,
        "busy-poses",
    );
    report(port, "shoulder", 0.0, 1, "measured");
    report(port, "elbow", 0.0, 1, "measured");
    let (status, body) = request(port, "GET", "/twin/poses", &auth(), "");
    assert_eq!(status, 503, "{body}");
    assert!(
        json_of(&body)["error"]
            .as_str()
            .unwrap()
            .contains("too many computations"),
        "{body}"
    );
}

#[test]
fn a_dry_run_while_every_slot_is_taken_is_a_503_and_starts_no_process() {
    let route = DryRunRoute::new(
        PathBuf::from("no-such-program-for-the-dry-run"),
        urdf_file("busy-dryrun"),
        None,
        "arm".into(),
        crate::twincfg::DryRunCfg {
            timeout_ms: 20_000,
            require_collision_check: false,
            ..Default::default()
        },
    );
    route.occupy_every_slot();
    let port = start(
        Setup {
            extras: Extras {
                dryrun: Some(route),
                ..Extras::default()
            },
            sim: false,
            collision: false,
        },
        None,
        "busy-dryrun",
    );
    let (status, body) = request(port, "POST", "/dryrun/trajectory", &auth(), SWING);
    assert_eq!(status, 503, "{body}");
    assert!(
        json_of(&body)["error"]
            .as_str()
            .unwrap()
            .contains("too many dry runs"),
        "{body}"
    );
}
