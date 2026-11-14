// =============================================================================
// HYDRA-UMC-TWIN - src/server/twin/tests/fake_child.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0-or-later - see LICENSE
// =============================================================================
//! The poses route with a stand-in for the forward kinematics child, so that it is run where the real binary of HYDRA-UMC-PHYSICS-REPLICA is not built (the
//! continuous integration): the answer for a good child, the status each failure of the child becomes, a state that does not satisfy the contract, and the
//! language of the answer. Unix only: the stand-in is a shell script.

use super::*;
use std::os::unix::fs::PermissionsExt;

/// A program that answers `fk` like the real one (`mode` ok) or fails in one way.
fn fake_physics(tag: &str, mode: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("twin-fake-physics-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("fake-physics");
    let body = format!(
        r#"#!/bin/sh
mode={mode}
case "$mode" in
  refuse) echo "fk: unknown joint zz" >&2; exit 2 ;;
  crash) exit 9 ;;
  slow) sleep 5; exit 0 ;;
  badtype) echo '[{{"joint":"a","position":{{"x":"far","y":0,"z":0}},"quaternion":{{"w":1,"x":0,"y":0,"z":0}}}}]'; exit 0 ;;
esac
out="["
sep=""
IFS=,
for kv in $5; do
  name=${{kv%%=*}}
  val=${{kv#*=}}
  out="$out$sep{{\"joint\":\"$name\",\"position\":{{\"x\":$val,\"y\":0.0,\"z\":0.5}},\"quaternion\":{{\"w\":1.0,\"x\":0.0,\"y\":0.0,\"z\":0.0}}}}"
  sep=","
done
printf '%s]\n' "$out"
"#
    );
    std::fs::write(&path, body).unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&path, permissions).unwrap();
    path
}

fn poses_server(tag: &str, mode: &str, timeout_ms: u64) -> u16 {
    let route = PoseRoute::new(
        fake_physics(tag, mode),
        urdf_file(tag),
        "arm".into(),
        crate::twincfg::PosesCfg {
            joints: vec!["a".into(), "b".into()],
            max_age_ms: 60_000,
            timeout_ms,
        },
    );
    start(
        Setup {
            extras: Extras {
                poses: Some(route),
                ..Extras::default()
            },
            sim: false,
            collision: false,
        },
        None,
        tag,
    )
}

fn both_reported(port: u16, a: f64, b: f64, seq: u64) {
    report(port, "a", a, seq, "measured");
    report(port, "b", b, seq, "commanded");
}

#[test]
fn a_good_child_gives_a_state_that_satisfies_the_contract_and_follows_the_reports() {
    let port = poses_server("fc-ok", "ok", 20_000);
    both_reported(port, 0.5, 1.5, 1);
    let (status, body) = request(port, "GET", "/twin/poses", &auth(), "");
    assert_eq!(status, 200, "{body}");
    let state = json_of(&body);
    assert!(
        crate::schema::state_problems(&state).is_empty(),
        "{:?}",
        crate::schema::state_problems(&state)
    );
    assert_eq!(state["source"], "real");
    assert_eq!(state["robot_id"], "arm");
    assert_eq!(state["joint_states"][0]["reading"], "measured");
    assert_eq!(state["joint_states"][1]["reading"], "commanded");
    let x_of = |state: &serde_json::Value, name: &str| {
        state["poses"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == name)
            .unwrap_or_else(|| panic!("{name} in {state}"))["pose"]["position"]["x"]
            .as_f64()
            .unwrap()
    };
    assert_eq!(x_of(&state, "a"), 0.5);
    assert_eq!(x_of(&state, "b"), 1.5);
    // asked again for the same positions the answer has the same poses; moved, the poses move
    let (_, again) = request(port, "GET", "/twin/poses", &auth(), "");
    assert_eq!(json_of(&again)["poses"], state["poses"]);
    both_reported(port, -0.25, 1.5, 2);
    let (_, moved) = request(port, "GET", "/twin/poses", &auth(), "");
    assert_eq!(x_of(&json_of(&moved), "a"), -0.25);
}

#[test]
fn the_route_needs_the_token_and_a_get() {
    let port = poses_server("fc-auth", "ok", 20_000);
    both_reported(port, 0.5, 1.5, 1);
    assert_eq!(request(port, "GET", "/twin/poses", "", "").0, 401);
    assert_eq!(
        request(
            port,
            "GET",
            "/twin/poses",
            "Authorization: Bearer not-the-token\r\n",
            ""
        )
        .0,
        401
    );
    assert_ne!(request(port, "POST", "/twin/poses", &auth(), "{}").0, 200);
}

#[test]
fn a_child_that_refuses_the_positions_is_a_422_that_says_so() {
    let port = poses_server("fc-refuse", "refuse", 20_000);
    both_reported(port, 0.5, 1.5, 1);
    let (status, body) = request(port, "GET", "/twin/poses", &auth(), "");
    assert_eq!(status, 422, "{body}");
    let answer = json_of(&body);
    assert_eq!(answer["error"], "fk: unknown joint zz");
    assert!(answer["note"]
        .as_str()
        .unwrap()
        .contains("never read as zero"));
}

#[test]
fn a_child_that_crashes_is_a_502_and_one_that_hangs_is_a_504_that_names_the_limit() {
    let port = poses_server("fc-crash", "crash", 20_000);
    both_reported(port, 0.5, 1.5, 1);
    let (status, body) = request(port, "GET", "/twin/poses", &auth(), "");
    assert_eq!(status, 502, "{body}");
    assert!(json_of(&body)["error"]
        .as_str()
        .unwrap()
        .contains("status Some(9)"));
    let port = poses_server("fc-slow", "slow", 150);
    both_reported(port, 0.5, 1.5, 1);
    let (status, body) = request(port, "GET", "/twin/poses", &auth(), "");
    assert_eq!(status, 504, "{body}");
    assert_eq!(
        json_of(&body)["error"],
        "the computation did not end within 150 ms"
    );
}

#[test]
fn an_answer_of_the_child_that_does_not_satisfy_the_contract_is_a_502_with_its_problems_never_an_answer(
) {
    let port = poses_server("fc-badtype", "badtype", 20_000);
    both_reported(port, 0.5, 1.5, 1);
    let (status, body) = request(port, "GET", "/twin/poses", &auth(), "");
    assert_eq!(status, 502, "{body}");
    let answer = json_of(&body);
    assert!(answer["error"]
        .as_str()
        .unwrap()
        .contains("is not a JointPoses document"));
}

#[test]
fn the_incomplete_answer_is_in_the_language_asked_for_by_the_query_then_the_header_then_the_default(
) {
    let port = poses_server("fc-lang", "ok", 20_000);
    let message = |path: &str, headers: &str| {
        let (status, body) = request(port, "GET", path, &format!("{}{headers}", auth()), "");
        assert_eq!(status, 409, "{body}");
        let answer = json_of(&body);
        assert_eq!(answer["code"], "incomplete");
        assert_eq!(answer["missing"], serde_json::json!(["a", "b"]));
        answer["message"].as_str().unwrap().to_string()
    };
    let mut seen = std::collections::HashSet::new();
    for lang in Lang::ALL {
        let text = message(&format!("/twin/poses?lang={}", lang.code()), "");
        assert_eq!(text, say(lang, Say::PosesIncomplete), "{}", lang.code());
        seen.insert(text);
    }
    assert_eq!(seen.len(), 8, "eight different sentences");
    let default = say(Extras::default().lang, Say::PosesIncomplete);
    assert_eq!(message("/twin/poses", ""), default);
    assert_eq!(
        message("/twin/poses", "Accept-Language: fr-FR,en;q=0.8\r\n"),
        say(Lang::Fr, Say::PosesIncomplete)
    );
    assert_eq!(
        message(
            "/twin/poses",
            "Accept-Language: xx-XX, pt-PT;q=0.9, de;q=0.5\r\n"
        ),
        say(Lang::Pt, Say::PosesIncomplete),
        "the first language of the header that the twin has"
    );
    // the query wins over the header, and a tag the twin does not have falls back to the header, then to the default
    assert_eq!(
        message("/twin/poses?lang=ja", "Accept-Language: de\r\n"),
        say(Lang::Ja, Say::PosesIncomplete)
    );
    assert_eq!(
        message("/twin/poses?lang=klingon", "Accept-Language: it\r\n"),
        say(Lang::It, Say::PosesIncomplete)
    );
    assert_eq!(message("/twin/poses?lang=klingon", ""), default);
    assert_eq!(
        message("/twin/poses?lang=ZH_cn", ""),
        say(Lang::Zh, Say::PosesIncomplete),
        "the tag is read in any case with either separator"
    );
}
