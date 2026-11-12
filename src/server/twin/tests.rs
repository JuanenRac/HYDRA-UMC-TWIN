// =============================================================================
// HYDRA-UMC-TWIN - src/server/twin/tests.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0-or-later - see LICENSE
// =============================================================================
//! End-to-end tests of the routes of the kinematic twin: a real server on a real socket, the simulated robot, and, where it is built next to
//! this repository, the real binary of HYDRA-UMC-PHYSICS-REPLICA.

use super::*;
use crate::certificate::Signer;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;

const TOKEN: &str = "a-certify-token-of-sixteen+";

const PLANAR_URDF: &str = r#"<robot name="planar">
  <link name="base"/>
  <link name="l1"><collision><origin xyz="0.5 0 0" rpy="0 1.5708 0"/><geometry><cylinder radius="0.05" length="1.0"/></geometry></collision></link>
  <link name="l2"><collision><origin xyz="0.5 0 0" rpy="0 1.5708 0"/><geometry><cylinder radius="0.05" length="1.0"/></geometry></collision></link>
  <link name="tool"/>
  <joint name="shoulder" type="revolute"><parent link="base"/><child link="l1"/>
    <axis xyz="0 0 1"/><limit lower="-3.1" upper="3.1" effort="10" velocity="2"/></joint>
  <joint name="elbow" type="revolute"><parent link="l1"/><child link="l2"/>
    <origin xyz="1 0 0"/><axis xyz="0 0 1"/><limit lower="-3.1" upper="3.1" effort="10" velocity="2"/></joint>
  <joint name="tip" type="fixed"><parent link="l2"/><child link="tool"/>
    <origin xyz="1 0 0"/></joint>
</robot>"#;
const PRESS_ZONES: &str = r#"{"zones":[{"id":"press","level":"danger","min":{"x":1.9,"y":-0.2,"z":-0.2},"max":{"x":2.2,"y":0.2,"z":0.2}}]}"#;
const SIZES: &str = r#"{"link_radius_m":0.05,"tool_extent_m":0.05}"#;
const SIM_MODEL: &str = r#"{"joints":[
    {"name":"shoulder","lower":-2.0,"upper":2.0,"max_velocity":1.0,"max_acceleration":5.0},
    {"name":"elbow","lower":-1.5,"upper":1.5,"max_velocity":2.0,"max_acceleration":10.0,"initial":0.5}
]}"#;

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("twin-routes-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn sibling_binary() -> Option<PathBuf> {
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    // only a binary that already knows the commands of these routes counts; HYDRA_UMC_TWIN_PHYSICS_REPLICA_BIN names one built elsewhere
    std::env::var("HYDRA_UMC_TWIN_PHYSICS_REPLICA_BIN")
        .ok()
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| crate::health::built_binary(&workspace))
        .filter(|bin| {
            ["fk", "check-trajectory", "predict-collision"]
                .iter()
                .all(|c| {
                    std::process::Command::new(bin)
                        .arg(c)
                        .output()
                        .map(|o| String::from_utf8_lossy(&o.stderr).contains(&format!("{c}:")))
                        .unwrap_or(false)
                })
        })
}

fn request(port: u16, method: &str, path: &str, headers: &str, body: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let text = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(text.as_bytes()).unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw.as_str(), ""));
    let status = head
        .lines()
        .next()
        .unwrap_or("")
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    (status, body.to_string())
}

fn auth() -> String {
    format!("Authorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\n")
}

fn json_of(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("{e}: {body}"))
}

struct Setup {
    extras: Extras,
    sim: bool,
    collision: bool,
}

fn start(setup: Setup, binary: Option<PathBuf>, tag: &str) -> u16 {
    let server = bind("127.0.0.1:0").expect("bind");
    let port = server.server_addr().to_ip().expect("ip").port();
    let certifier = Certifier {
        signer: Signer::from_seed([7u8; 32]),
        token: TOKEN.to_string(),
    };
    // the directory may already hold the URDF of the route (`urdf_file`): it is created, not emptied
    let dir = std::env::temp_dir().join(format!("twin-routes-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let collision = if setup.collision {
        let (urdf, zones, config) = (
            dir.join("arm.urdf"),
            dir.join("zones.json"),
            dir.join("sizes.json"),
        );
        std::fs::write(&urdf, PLANAR_URDF).unwrap();
        std::fs::write(&zones, PRESS_ZONES).unwrap();
        std::fs::write(&config, SIZES).unwrap();
        Some(crate::collision::CollisionRoute::new(
            binary.unwrap(),
            urdf,
            zones,
            config,
            std::time::Duration::from_millis(20_000),
            64,
        ))
    } else {
        None
    };
    let sim = setup
        .sim
        .then(|| SimRoute::new(&crate::dynamics::SimConfig::parse(SIM_MODEL).unwrap(), None));
    let extras = setup.extras;
    std::thread::spawn(move || {
        run_with_extras(
            server,
            dir,
            None,
            Some(certifier),
            DEFAULT_WORKERS,
            None,
            None,
            collision,
            sim,
            extras,
        )
    });
    port
}

fn urdf_file(tag: &str) -> PathBuf {
    let path = tempdir(tag).join("robot.urdf");
    std::fs::write(&path, PLANAR_URDF).unwrap();
    path
}

fn report(port: u16, joint: &str, position: f64, seq: u64, source: &str) {
    let body = format!(
        r#"{{"joint":"{joint}","position":{position},"seq":{seq},"stamp_ms":1700000000000,"source":"{source}"}}"#
    );
    assert_eq!(
        request(port, "POST", "/interlock/state", &auth(), &body).0,
        200
    );
}

fn drift_extras(max_age_ms: u64, alarm_after: u32) -> Extras {
    let cfg = crate::twincfg::DriftCfg {
        alarm_after,
        max_age_ms,
        ..Default::default()
    };
    Extras {
        drift: Some(DriftRoute {
            max_age_ms,
            monitor: std::sync::Mutex::new(DriftMonitor::new(cfg, None, None)),
        }),
        ..Extras::default()
    }
}

#[test]
fn without_their_switches_the_routes_do_not_exist() {
    let port = start(
        Setup {
            extras: Extras::default(),
            sim: false,
            collision: false,
        },
        None,
        "off",
    );
    assert_eq!(request(port, "GET", "/twin/poses", &auth(), "").0, 404);
    assert_eq!(request(port, "GET", "/drift", &auth(), "").0, 404);
    assert_eq!(
        request(port, "POST", "/dryrun/trajectory", &auth(), "{}").0,
        405
    );
    assert_eq!(
        request(port, "POST", "/drift/observe", &auth(), "{}").0,
        405
    );
}

#[test]
fn the_routes_that_are_on_need_the_token_and_refuse_browsers_and_bad_bodies() {
    let port = start(
        Setup {
            extras: drift_extras(2000, 3),
            sim: false,
            collision: false,
        },
        None,
        "guard",
    );
    assert_eq!(request(port, "GET", "/drift", "", "").0, 401);
    assert_eq!(
        request(
            port,
            "POST",
            "/drift/observe",
            "Content-Type: application/json\r\n",
            "{}"
        )
        .0,
        401
    );
    let browser = format!("{}Origin: http://evil.example\r\n", auth());
    assert_eq!(
        request(
            port,
            "POST",
            "/drift/observe",
            &browser,
            r#"{"joint":"a","real":1,"simulated":1}"#
        )
        .0,
        403
    );
    for bad in [
        "nope",
        "{}",
        r#"{"joint":"a","extra":1}"#,
        r#"{"joint":"a","real":"x"}"#,
    ] {
        assert_eq!(
            request(port, "POST", "/drift/observe", &auth(), bad).0,
            400,
            "{bad}"
        );
    }
    // a joint with no report and no simulated robot is a 409, never zero
    assert_eq!(
        request(port, "POST", "/drift/observe", &auth(), r#"{"joint":"a"}"#).0,
        409
    );
    assert_eq!(
        request(
            port,
            "POST",
            "/drift/observe",
            &auth(),
            r#"{"joint":"a","real":1}"#
        )
        .0,
        409
    );
}

#[test]
fn the_drift_of_a_real_joint_against_the_simulated_one_is_classified_and_summarised() {
    let port = start(
        Setup {
            extras: drift_extras(60_000, 2),
            sim: true,
            collision: false,
        },
        None,
        "drift",
    );
    // the simulated robot is the one the twin follows as measured: the same position is no drift
    assert_eq!(
        request(port, "POST", "/sim/step", &auth(), r#"{"seconds":0.01}"#).0,
        200
    );
    let (status, body) = request(
        port,
        "POST",
        "/drift/observe",
        &auth(),
        r#"{"joint":"elbow"}"#,
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(json_of(&body)["level"], "ok");
    // the real joint (given by the bridge) is 0.1 away from the simulated one (0.5)
    let (_, body) = request(
        port,
        "POST",
        "/drift/observe",
        &auth(),
        r#"{"joint":"elbow","real":0.6}"#,
    );
    let first = json_of(&body);
    assert_eq!(first["level"], "warn");
    assert!((first["error"].as_f64().unwrap() - 0.1).abs() < 1e-9);
    // two in a row above the alarm threshold, with the sentence in Portuguese of Portugal
    let (_, body) = request(
        port,
        "POST",
        "/drift/observe?lang=pt-PT",
        &auth(),
        r#"{"joint":"elbow","real":0.6}"#,
    );
    let second = json_of(&body);
    assert_eq!(second["level"], "alarm");
    assert_eq!(second["message"], say(Lang::Pt, Say::DriftAlarm));
    let (status, body) = request(port, "GET", "/drift", &auth(), "");
    assert_eq!(status, 200);
    let summary = json_of(&body);
    assert_eq!(summary["level"], "alarm");
    assert_eq!(summary["joints"][0]["joint"], "elbow");
    assert_eq!(summary["joints"][0]["observations"], 3);
    assert_eq!(summary["datalake"]["enabled"], false);
    // the language also comes from Accept-Language
    let (_, body) = request(
        port,
        "GET",
        "/drift",
        &format!("{}Accept-Language: ja\r\n", auth()),
        "",
    );
    assert_eq!(
        json_of(&body)["joints"][0]["message"],
        say(Lang::Ja, Say::DriftAlarm)
    );
}

#[test]
fn a_report_that_is_only_commanded_or_too_old_is_not_the_real_position() {
    let port = start(
        Setup {
            extras: drift_extras(150, 3),
            sim: true,
            collision: false,
        },
        None,
        "drift-age",
    );
    report(port, "shoulder", 0.3, 1, "commanded");
    let observe = || {
        request(
            port,
            "POST",
            "/drift/observe",
            &auth(),
            r#"{"joint":"shoulder"}"#,
        )
        .0
    };
    assert_eq!(observe(), 409);
    report(port, "shoulder", 0.3, 2, "measured");
    assert_eq!(observe(), 200);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(observe(), 409);
}

#[test]
fn the_poses_of_the_real_kinematics_follow_the_reports_and_satisfy_the_contract() {
    let Some(binary) = sibling_binary() else {
        eprintln!("skipped: HYDRA-UMC-PHYSICS-REPLICA with fk, check-trajectory and predict-collision is not built next to this repository");
        return;
    };
    let route = PoseRoute::new(
        binary,
        urdf_file("poses"),
        "arm".into(),
        crate::twincfg::PosesCfg {
            joints: vec!["shoulder".into(), "elbow".into()],
            max_age_ms: 60_000,
            timeout_ms: 20_000,
        },
    );
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
        "poses",
    );
    assert_eq!(request(port, "GET", "/twin/poses", "", "").0, 401);
    // nothing reported: the joints are named and no pose is computed
    let (status, body) = request(port, "GET", "/twin/poses?lang=de", &auth(), "");
    assert_eq!(status, 409, "{body}");
    let answer = json_of(&body);
    assert_eq!(answer["missing"], serde_json::json!(["shoulder", "elbow"]));
    assert_eq!(answer["message"], say(Lang::De, Say::PosesIncomplete));
    report(port, "shoulder", 0.0, 1, "measured");
    let (status, body) = request(port, "GET", "/twin/poses", &auth(), "");
    assert_eq!(status, 409, "{body}");
    assert_eq!(json_of(&body)["missing"], serde_json::json!(["elbow"]));
    report(port, "elbow", 0.0, 1, "commanded");
    let (status, body) = request(port, "GET", "/twin/poses", &auth(), "");
    assert_eq!(status, 200, "{body}");
    let state = json_of(&body);
    assert!(crate::schema::state_problems(&state).is_empty());
    assert_eq!(state["source"], "real");
    assert_eq!(state["robot_id"], "arm");
    let pose_of = |state: &serde_json::Value, name: &str| {
        state["poses"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == name)
            .unwrap_or_else(|| panic!("{name} in {state}"))["pose"]
            .clone()
    };
    // the straight arm: the frame of the elbow is one metre along x
    assert!((pose_of(&state, "elbow")["position"]["x"].as_f64().unwrap() - 1.0).abs() < 1e-6);
    assert_eq!(state["joint_states"][1]["reading"], "commanded");
    // the shoulder turns a quarter of a turn: the elbow is one metre along y
    report(port, "shoulder", std::f64::consts::FRAC_PI_2, 2, "measured");
    let (_, body) = request(port, "GET", "/twin/poses", &auth(), "");
    let moved = json_of(&body);
    let elbow = pose_of(&moved, "elbow");
    assert!(
        elbow["position"]["x"].as_f64().unwrap().abs() < 1e-6,
        "{elbow}"
    );
    assert!(
        (elbow["position"]["y"].as_f64().unwrap() - 1.0).abs() < 1e-6,
        "{elbow}"
    );
    assert!(moved["sequence"].as_u64().unwrap() >= 2);
}

fn dryrun_extras(binary: PathBuf, tag: &str, require_collision: bool) -> Extras {
    Extras {
        dryrun: Some(DryRunRoute::new(
            binary,
            urdf_file(tag),
            None,
            "arm".into(),
            crate::twincfg::DryRunCfg {
                timeout_ms: 20_000,
                require_collision_check: require_collision,
                ..Default::default()
            },
        )),
        ..Extras::default()
    }
}

const CLEAR_SWING: &str = r#"{"samples":[
    {"t":0.0,"joints":{"shoulder":1.5,"elbow":0.0}},
    {"t":0.5,"joints":{"shoulder":1.6,"elbow":0.0}},
    {"t":1.0,"joints":{"shoulder":1.7,"elbow":0.0}}]}"#;

#[test]
fn a_trajectory_is_run_in_simulation_and_the_verdict_is_signed_and_bound_to_it() {
    let Some(binary) = sibling_binary() else {
        eprintln!("skipped: HYDRA-UMC-PHYSICS-REPLICA with fk, check-trajectory and predict-collision is not built next to this repository");
        return;
    };
    let port = start(
        Setup {
            extras: dryrun_extras(binary.clone(), "dry", true),
            sim: false,
            collision: true,
        },
        Some(binary),
        "dry",
    );
    let post = |body: &str| request(port, "POST", "/dryrun/trajectory?lang=es", &auth(), body);
    assert_eq!(
        request(
            port,
            "POST",
            "/dryrun/trajectory",
            "Content-Type: application/json\r\n",
            "{}"
        )
        .0,
        401
    );
    // a slow swing away from the press: every limit holds and nothing is touched
    let (status, body) = post(CLEAR_SWING);
    assert_eq!(status, 200, "{body}");
    let verdict = json_of(&body);
    // the verdict is `pass` only if the replica could run every check on this URDF; either way it says what ran
    assert!(
        ["pass", "incomplete"].contains(&verdict["verdict"].as_str().unwrap()),
        "{body}"
    );
    eprintln!("clear swing: {body}");
    assert_eq!(verdict["checks"]["collision"]["checked"], true);
    assert_eq!(verdict["checks"]["collision"]["collision"], false);
    assert_eq!(verdict["checks"]["trajectory"]["ok"], true);
    assert_eq!(verdict["robot_id"], "arm");
    assert_eq!(verdict["trajectory_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(verdict["signature"].as_str().unwrap().len(), 128);
    assert_eq!(verdict["nonce"].as_str().unwrap().len(), 32);
    assert_eq!(
        verdict["valid_until_ms"].as_u64().unwrap() - verdict["issued_at_ms"].as_u64().unwrap(),
        5000
    );
    let wanted = if verdict["verdict"] == "pass" {
        Say::VerdictPass
    } else {
        Say::VerdictIncomplete
    };
    assert_eq!(verdict["message"], say(Lang::Es, wanted));
    // the same samples give the same digest and a fresh nonce
    let (_, again) = post(CLEAR_SWING);
    let again = json_of(&again);
    assert_eq!(again["trajectory_sha256"], verdict["trajectory_sha256"]);
    assert_ne!(again["nonce"], verdict["nonce"]);
    // a jump of three radians in a tenth of a second breaks the speed limit of the URDF
    let (status, body) = post(
        r#"{"samples":[{"t":0.0,"joints":{"shoulder":-1.5,"elbow":0.0}},{"t":0.1,"joints":{"shoulder":1.5,"elbow":0.0}}]}"#,
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(json_of(&body)["verdict"], "fail");
    // the straight arm reaches the press at two metres
    let (status, body) = post(
        r#"{"samples":[{"t":0.0,"joints":{"shoulder":0.0,"elbow":0.0}},{"t":1.0,"joints":{"shoulder":0.0,"elbow":0.0}}]}"#,
    );
    assert_eq!(status, 200, "{body}");
    let hit = json_of(&body);
    assert_eq!(hit["verdict"], "fail", "{body}");
    assert_eq!(hit["checks"]["collision"]["collision"], true);
    // an unknown joint is the refusal of the replica: no verdict at all
    let (status, body) =
        post(r#"{"samples":[{"t":0.0,"joints":{"wrist":0.0}},{"t":1.0,"joints":{"wrist":0.1}}]}"#);
    assert!(status == 422 || status == 502, "{status} {body}");
    assert!(!body.contains("\"verdict\""), "{body}");
    // malformed requests
    assert_eq!(post("{}").0, 400);
    assert_eq!(
        post(r#"{"samples":[{"t":0,"joints":{"shoulder":0}}]}"#).0,
        400
    );
}

#[test]
fn without_the_collision_prediction_a_verdict_is_never_a_pass_unless_the_configuration_allows_it() {
    let Some(binary) = sibling_binary() else {
        eprintln!("skipped: HYDRA-UMC-PHYSICS-REPLICA is not built next to this repository");
        return;
    };
    for (tag, require) in [("strict", true), ("relaxed", false)] {
        let port = start(
            Setup {
                extras: dryrun_extras(binary.clone(), tag, require),
                sim: false,
                collision: false,
            },
            None,
            tag,
        );
        let (status, body) = request(port, "POST", "/dryrun/trajectory", &auth(), CLEAR_SWING);
        assert_eq!(status, 200, "{body}");
        let verdict = json_of(&body);
        assert_eq!(verdict["checks"]["collision"]["checked"], false);
        if require {
            assert_eq!(verdict["verdict"], "incomplete", "{body}");
        } else {
            assert_ne!(verdict["verdict"], "fail", "{body}");
        }
    }
}

// ---- the input for the state of a vehicle and the WebSocket ----

fn uav_body(robot: &str, sequence: u64, east: f64, source: &str) -> String {
    serde_json::json!({
        "schema_version": "1.0",
        "source": source,
        "robot_id": robot,
        "frame": "base",
        "timestamp_utc": "2030-01-01T00:00:00.000Z",
        "sequence": sequence,
        "joint_states": [],
        "poses": [{
            "name": "vehicle",
            "pose": {
                "schema_version": "1.0",
                "frame": "base",
                "position": {"x": east, "y": 20.0, "z": 12.5},
                "orientation": {"w": 1.0, "x": 0.0, "y": 0.0, "z": 0.0},
            },
        }],
    })
    .to_string()
}

fn uav_extras(cfg: crate::twincfg::UavCfg) -> Extras {
    Extras {
        uav: Some(crate::uav::UavRoute::new(cfg)),
        ..Extras::default()
    }
}

#[test]
fn without_its_switch_the_uav_route_does_not_exist() {
    let port = start(
        Setup {
            extras: Extras::default(),
            sim: false,
            collision: false,
        },
        None,
        "uav-off",
    );
    assert_eq!(request(port, "GET", "/twin/uav", &auth(), "").0, 404);
    assert_eq!(
        request(
            port,
            "POST",
            "/twin/uav",
            &auth(),
            &uav_body("u", 1, 1.0, "real")
        )
        .0,
        405
    );
}

#[test]
fn a_state_of_a_vehicle_needs_the_token_is_kept_and_listed_and_a_bad_one_is_refused() {
    let port = start(
        Setup {
            extras: uav_extras(Default::default()),
            sim: false,
            collision: false,
        },
        None,
        "uav-on",
    );
    let good = uav_body("uav-1", 1, 10.0, "real");
    let json_head = "Content-Type: application/json\r\n";
    assert_eq!(request(port, "POST", "/twin/uav", json_head, &good).0, 401);
    assert_eq!(request(port, "GET", "/twin/uav", "", "").0, 401);
    let browser = format!("{}Origin: http://evil.example\r\n", auth());
    assert_eq!(request(port, "POST", "/twin/uav", &browser, &good).0, 403);
    let (status, body) = request(port, "POST", "/twin/uav", &auth(), &good);
    assert_eq!(status, 200, "{body}");
    let answer = json_of(&body);
    assert_eq!(answer["accepted"], true);
    assert_eq!(answer["robot_id"], "uav-1");
    assert_eq!(answer["sequence"], 1);
    // the same sequence again, or an older one, is a 409 and does not replace the state
    let (status, body) = request(
        port,
        "POST",
        "/twin/uav",
        &auth(),
        &uav_body("uav-1", 1, 99.0, "real"),
    );
    assert_eq!(status, 409, "{body}");
    assert_eq!(json_of(&body)["last_sequence"], 1);
    // a simulated state is refused with a sentence in the language asked for
    let (status, body) = request(
        port,
        "POST",
        "/twin/uav?lang=de",
        &auth(),
        &uav_body("uav-2", 1, 1.0, "simulated"),
    );
    assert_eq!(status, 422, "{body}");
    assert_eq!(
        json_of(&body)["message"],
        say(Lang::De, Say::UavSimulatedRefused)
    );
    for bad in [
        "nope".to_string(),
        "{}".to_string(),
        uav_body("uav-3", 1, 1.0e9, "real"),
    ] {
        assert_eq!(
            request(port, "POST", "/twin/uav", &auth(), &bad).0,
            400,
            "{bad}"
        );
    }
    // a body above the limit is a 413
    let big = format!("{{\"pad\":\"{}\"}}", "x".repeat(20_000));
    assert_eq!(request(port, "POST", "/twin/uav", &auth(), &big).0, 413);
    let (status, body) = request(port, "GET", "/twin/uav", &auth(), "");
    assert_eq!(status, 200, "{body}");
    let list = json_of(&body);
    let vehicles = list["vehicles"].as_array().unwrap();
    assert_eq!(vehicles.len(), 1);
    assert_eq!(vehicles[0]["robot_id"], "uav-1");
    assert_eq!(vehicles[0]["stale"], false);
    assert_eq!(
        vehicles[0]["state"]["poses"][0]["pose"]["position"]["x"],
        10.0
    );
}

#[test]
fn the_state_of_a_vehicle_is_shown_as_stale_when_it_is_too_old() {
    let port = start(
        Setup {
            extras: uav_extras(crate::twincfg::UavCfg {
                max_age_ms: 50,
                ..Default::default()
            }),
            sim: false,
            collision: false,
        },
        None,
        "uav-stale",
    );
    assert_eq!(
        request(
            port,
            "POST",
            "/twin/uav",
            &auth(),
            &uav_body("u", 1, 1.0, "real")
        )
        .0,
        200
    );
    std::thread::sleep(std::time::Duration::from_millis(200));
    let (_, body) = request(port, "GET", "/twin/uav?lang=es", &auth(), "");
    let list = json_of(&body);
    assert_eq!(list["vehicles"][0]["stale"], true);
    assert_eq!(
        list["vehicles"][0]["message"],
        say(Lang::Es, Say::UavStateStale)
    );
}

/// Opens a WebSocket on `port` and returns the stream, the head of the answer and what followed it.
fn ws_open(port: u16, path: &str, extra: &str) -> (TcpStream, String, Vec<u8>) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    let text = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n{extra}\r\n"
    );
    stream.write_all(text.as_bytes()).unwrap();
    let mut raw = Vec::new();
    let mut buf = [0u8; 2048];
    loop {
        if let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            return (
                stream,
                String::from_utf8_lossy(&raw[..end]).to_string(),
                raw[end + 4..].to_vec(),
            );
        }
        let n = stream.read(&mut buf).unwrap();
        assert!(n > 0, "closed before the head: {raw:?}");
        raw.extend_from_slice(&buf[..n]);
    }
}

/// The next text message of the twin (pings and the like are skipped); None when the connection ends or nothing comes in time.
fn ws_next_text(stream: &mut TcpStream, pending: &mut Vec<u8>) -> Option<serde_json::Value> {
    let mut buf = [0u8; 8192];
    loop {
        if pending.len() >= 2 {
            let opcode = pending[0] & 0x0F;
            let (length, header) = match pending[1] & 0x7F {
                126 if pending.len() >= 4 => {
                    (u16::from_be_bytes([pending[2], pending[3]]) as usize, 4)
                }
                126 | 127 => (usize::MAX, 0),
                n => (n as usize, 2),
            };
            if length != usize::MAX && pending.len() >= header + length {
                let payload = pending[header..header + length].to_vec();
                pending.drain(..header + length);
                if opcode == 1 {
                    return Some(serde_json::from_slice(&payload).unwrap());
                }
                continue;
            }
        }
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => return None,
            Ok(n) => pending.extend_from_slice(&buf[..n]),
        }
    }
}

#[test]
fn the_websocket_pushes_the_robot_and_the_vehicles_to_a_client_with_the_token() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let ws_port = listener.local_addr().unwrap().port();
    let extras = Extras {
        poses: Some(PoseRoute::new(
            PathBuf::from("no-such-binary"),
            PathBuf::from("no-such.urdf"),
            "arm".into(),
            crate::twincfg::PosesCfg {
                joints: vec!["shoulder".into()],
                max_age_ms: 60_000,
                timeout_ms: 1_000,
            },
        )),
        uav: Some(crate::uav::UavRoute::new(Default::default())),
        ws: Some(WsRoute {
            cfg: crate::twincfg::WsCfg {
                interval_ms: 50,
                ..Default::default()
            },
            listener: std::sync::Mutex::new(Some(listener)),
        }),
        ..Extras::default()
    };
    let port = start(
        Setup {
            extras,
            sim: false,
            collision: false,
        },
        None,
        "ws",
    );
    // no token, no session
    let (_, head, _) = ws_open(ws_port, "/twin/ws", "");
    assert!(head.starts_with("HTTP/1.1 401"), "{head}");
    let (mut stream, head, mut pending) = ws_open(
        ws_port,
        "/twin/ws?lang=fr",
        &format!("Authorization: Bearer {TOKEN}\r\n"),
    );
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    // the robot has reported nothing: the client is told which joint is missing, in French
    let first = ws_next_text(&mut stream, &mut pending).unwrap();
    assert_eq!(first["type"], "status");
    assert_eq!(first["code"], "incomplete");
    assert_eq!(first["missing"], serde_json::json!(["shoulder"]));
    assert_eq!(first["message"], say(Lang::Fr, Say::PosesIncomplete));
    // a vehicle that reports is pushed as soon as the twin has it
    assert_eq!(
        request(
            port,
            "POST",
            "/twin/uav",
            &auth(),
            &uav_body("uav-1", 1, 5.0, "real")
        )
        .0,
        200
    );
    let mut seen = None;
    for _ in 0..100 {
        match ws_next_text(&mut stream, &mut pending) {
            Some(message) if message["type"] == "uav_state" => {
                seen = Some(message);
                break;
            }
            Some(_) => {}
            None => break,
        }
    }
    let message = seen.expect("the state of the vehicle was not pushed");
    assert_eq!(message["state"]["robot_id"], "uav-1");
    assert_eq!(message["state"]["poses"][0]["pose"]["position"]["x"], 5.0);
}

#[test]
fn the_websocket_route_does_not_exist_on_the_http_port() {
    // the WebSocket has its own listener: the HTTP server never upgrades a request
    let port = start(
        Setup {
            extras: uav_extras(Default::default()),
            sim: false,
            collision: false,
        },
        None,
        "ws-off",
    );
    assert_eq!(request(port, "GET", "/twin/ws", &auth(), "").0, 404);
}

#[cfg(unix)]
mod fake_child;
