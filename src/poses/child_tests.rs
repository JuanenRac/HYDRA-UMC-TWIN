// =============================================================================
// HYDRA-UMC-TWIN - src/poses/child_tests.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0-or-later - see LICENSE
// =============================================================================
//! The poses of the twin with a stand-in for the forward kinematics child: a small shell program that answers like the real one, or fails in each way the real
//! one can. What is checked: the command line the child gets, the document that is built from its answer, the cache that spares a process while the robot stands
//! still (and that a failure never poisons), and the status each kind of failure becomes. Unix only: the stand-in is a shell script.

use super::*;
use std::os::unix::fs::PermissionsExt;
use std::sync::Mutex as StdMutex;

/// The stand-ins are written and started one at a time: a program that was just written can be refused ("text file busy") while another thread forks.
static ONE_AT_A_TIME: StdMutex<()> = StdMutex::new(());

struct Fake {
    binary: PathBuf,
    dir: PathBuf,
}

impl Fake {
    fn new(tag: &str) -> Fake {
        let dir =
            std::env::temp_dir().join(format!("twin-poses-child-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fake-physics");
        let body = format!(
            r#"#!/bin/sh
echo "$@" >> "{dir}/args.log"
echo run >> "{dir}/count.log"
mode=ok
if [ -f "{dir}/mode" ]; then mode=$(cat "{dir}/mode"); fi
case "$mode" in
  refuse) echo "some noise first" >&2; echo "fk: unknown joint zz" >&2; echo "fk: a second line" >&2; exit 2 ;;
  refuse_silent) exit 2 ;;
  refuse_long) printf 'fk: %0400d\n' 0 >&2; exit 2 ;;
  crash) exit 7 ;;
  garbage) echo "not json at all"; exit 0 ;;
  empty) echo "[]"; exit 0 ;;
  object) echo '{{"joint":"a"}}'; exit 0 ;;
  no_quaternion) echo '[{{"joint":"a","position":{{"x":1,"y":2,"z":3}}}}]'; exit 0 ;;
  no_joint) echo '[{{"position":{{"x":1,"y":2,"z":3}},"quaternion":{{"w":1,"x":0,"y":0,"z":0}}}}]'; exit 0 ;;
  slow) sleep 5; exit 0 ;;
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
"#,
            dir = dir.display()
        );
        std::fs::write(&path, body).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).unwrap();
        Fake { binary: path, dir }
    }

    fn mode(&self, mode: &str) {
        std::fs::write(self.dir.join("mode"), mode).unwrap();
    }

    fn runs(&self) -> usize {
        std::fs::read_to_string(self.dir.join("count.log"))
            .map(|t| t.lines().count())
            .unwrap_or(0)
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.join("args.log"))
            .map(|t| t.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    fn route(&self, joints: &[&str], timeout_ms: u64) -> PoseRoute {
        PoseRoute::new(
            self.binary.clone(),
            PathBuf::from("robot.urdf"),
            "arm".into(),
            PosesCfg {
                joints: joints.iter().map(|j| j.to_string()).collect(),
                max_age_ms: 1000,
                timeout_ms,
            },
        )
    }
}

fn report(
    joint: &str,
    position: f64,
    seq: u64,
    age_ms: u64,
    source: StateSource,
) -> (JointStateReport, Duration) {
    (
        JointStateReport {
            joint: joint.into(),
            position,
            seq,
            stamp_ms: 0,
            source,
        },
        Duration::from_millis(age_ms),
    )
}

fn measured(joint: &str, position: f64, seq: u64) -> (JointStateReport, Duration) {
    report(joint, position, seq, 10, StateSource::Measured)
}

#[test]
fn the_state_is_built_from_the_answer_of_the_child_and_says_where_it_came_from() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let fake = Fake::new("state");
    let route = fake.route(&["shoulder", "elbow"], 20_000);
    let reports = vec![
        report("shoulder", 0.25, 4, 30, StateSource::Measured),
        report("elbow", -1.5, 9, 120, StateSource::Commanded),
    ];
    let state = route
        .twin_state(&reports, false, 1_709_210_096_789)
        .unwrap();
    assert_eq!(state["schema_version"], "1.0");
    assert_eq!(state["source"], "real");
    assert_eq!(state["robot_id"], "arm");
    assert_eq!(state["frame"], "base");
    assert_eq!(state["timestamp_utc"], "2024-02-29T12:34:56.789Z");
    assert_eq!(state["sequence"], 9, "the newest report of the joints used");
    assert_eq!(
        state["joint_states"],
        json!([
            {"name": "shoulder", "position": 0.25, "reading": "measured", "age_ms": 30},
            {"name": "elbow", "position": -1.5, "reading": "commanded", "age_ms": 120},
        ])
    );
    let names: Vec<&str> = state["poses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["elbow", "shoulder"],
        "the child answers in the order of the sorted joints"
    );
    let elbow = &state["poses"][0]["pose"];
    assert_eq!(elbow["frame"], "base");
    assert_eq!(elbow["schema_version"], "1.0");
    assert_eq!(elbow["position"], json!({"x": -1.5, "y": 0.0, "z": 0.5}));
    assert_eq!(
        elbow["orientation"],
        json!({"w": 1.0, "x": 0.0, "y": 0.0, "z": 0.0})
    );
    assert!(
        crate::schema::state_problems(&state).is_empty(),
        "{:?}",
        crate::schema::state_problems(&state)
    );
    // the simulated robot is said to be simulated
    assert_eq!(
        route.twin_state(&reports, true, 0).unwrap()["source"],
        "simulated"
    );
}

#[test]
fn the_child_gets_the_sorted_positions_of_the_joints_it_was_asked_for_and_nothing_else() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let fake = Fake::new("args");
    let route = fake.route(&["b", "a"], 20_000);
    let reports = vec![
        measured("b", 2.0, 1),
        measured("a", 0.1, 2),
        measured("extra", 9.0, 3),
    ];
    route.twin_state(&reports, false, 0).unwrap();
    assert_eq!(
        fake.calls(),
        vec!["fk --urdf robot.urdf --joints a=0.1,b=2.0 --json".to_string()]
    );
}

#[test]
fn without_a_list_of_joints_every_reported_joint_is_used() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let fake = Fake::new("all");
    let route = fake.route(&[], 20_000);
    let reports = vec![measured("z", 1.0, 1), measured("y", 2.0, 2)];
    let state = route.twin_state(&reports, false, 0).unwrap();
    assert_eq!(state["joint_states"].as_array().unwrap().len(), 2);
    assert_eq!(
        fake.calls(),
        vec!["fk --urdf robot.urdf --joints y=2.0,z=1.0 --json".to_string()]
    );
}

#[test]
fn the_same_positions_start_no_second_process_and_other_positions_start_one() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let fake = Fake::new("cache");
    let route = fake.route(&["a", "b"], 20_000);
    let first = route
        .twin_state(&[measured("a", 0.5, 1), measured("b", 1.5, 2)], false, 1)
        .unwrap();
    assert_eq!(fake.runs(), 1);
    // the same positions, in another order, with other sequence numbers, ages and times: the cache answers
    let second = route
        .twin_state(
            &[
                report("b", 1.5, 8, 700, StateSource::Commanded),
                report("a", 0.5, 7, 3, StateSource::Measured),
            ],
            false,
            99,
        )
        .unwrap();
    assert_eq!(fake.runs(), 1);
    assert_eq!(first["poses"], second["poses"]);
    assert_ne!(first["timestamp_utc"], second["timestamp_utc"]);
    assert_eq!(second["sequence"], 8);
    // a position that moved by the least representable step is another position
    let nudged = f64::from_bits(1.5f64.to_bits() + 1);
    route
        .twin_state(
            &[measured("a", 0.5, 9), measured("b", nudged, 10)],
            false,
            1,
        )
        .unwrap();
    assert_eq!(fake.runs(), 2);
    // and the new positions are the ones that are cached now
    route
        .twin_state(
            &[measured("a", 0.5, 11), measured("b", nudged, 12)],
            false,
            1,
        )
        .unwrap();
    assert_eq!(fake.runs(), 2);
}

#[test]
fn a_failure_is_never_cached_and_the_next_call_works_again() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let fake = Fake::new("fail-then-ok");
    let route = fake.route(&["a"], 20_000);
    let reports = vec![measured("a", 0.5, 1)];
    fake.mode("refuse");
    assert!(matches!(
        route.twin_state(&reports, false, 0),
        Err(PoseError::Refused(_))
    ));
    assert_eq!(
        route.running.load(Ordering::SeqCst),
        0,
        "the place for a computation is given back after a failure"
    );
    fake.mode("ok");
    assert!(route.twin_state(&reports, false, 0).is_ok());
    assert_eq!(fake.runs(), 2, "the failed call left nothing in the cache");
}

#[test]
fn a_child_that_refuses_the_positions_is_a_refusal_with_its_own_line() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let fake = Fake::new("refuse");
    let route = fake.route(&["a"], 20_000);
    let reports = vec![measured("a", 0.5, 1)];
    fake.mode("refuse");
    assert_eq!(
        route.twin_state(&reports, false, 0),
        Err(PoseError::Refused("fk: unknown joint zz".into())),
        "the first line that starts with fk:"
    );
    fake.mode("refuse_silent");
    assert_eq!(
        route.twin_state(&reports, false, 0),
        Err(PoseError::Refused("the child refused the positions".into()))
    );
    fake.mode("refuse_long");
    match route.twin_state(&reports, false, 0) {
        Err(PoseError::Refused(reason)) => {
            assert_eq!(reason.chars().count(), 300, "a long line is cut")
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn any_other_exit_status_is_the_child_being_unavailable() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let fake = Fake::new("crash");
    let route = fake.route(&["a"], 20_000);
    fake.mode("crash");
    assert_eq!(
        route.twin_state(&[measured("a", 0.5, 1)], false, 0),
        Err(PoseError::Unavailable(
            "the child ended with status Some(7)".into()
        ))
    );
}

#[test]
fn an_answer_that_is_not_a_list_of_poses_is_unavailable_never_an_empty_pose() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let fake = Fake::new("garbage");
    let route = fake.route(&["a"], 20_000);
    let reports = vec![measured("a", 0.5, 1)];
    for (mode, expected) in [
        ("garbage", "the answer of the child is not JSON"),
        ("empty", "the child answered no pose"),
        ("object", "the child answered no pose"),
        (
            "no_quaternion",
            "a pose of the child has no joint, position or quaternion",
        ),
        (
            "no_joint",
            "a pose of the child has no joint, position or quaternion",
        ),
    ] {
        fake.mode(mode);
        match route.twin_state(&reports, false, 0) {
            Err(PoseError::Unavailable(reason)) => {
                assert!(reason.contains(expected), "{mode}: {reason}")
            }
            other => panic!("{mode}: {other:?}"),
        }
    }
}

#[test]
fn a_child_that_does_not_end_in_time_is_a_timeout_that_names_the_limit() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let fake = Fake::new("slow");
    let route = fake.route(&["a"], 150);
    fake.mode("slow");
    let started = std::time::Instant::now();
    assert_eq!(
        route.twin_state(&[measured("a", 0.5, 1)], false, 0),
        Err(PoseError::Timeout(150))
    );
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "the child was killed, not waited for"
    );
    assert_eq!(route.running.load(Ordering::SeqCst), 0);
}

#[test]
fn a_report_exactly_as_old_as_the_limit_still_counts_and_one_millisecond_older_does_not() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let fake = Fake::new("age");
    let route = fake.route(&["a"], 20_000); // max_age_ms is 1000
    assert!(route
        .twin_state(
            &[report("a", 0.5, 1, 1000, StateSource::Measured)],
            false,
            0
        )
        .is_ok());
    assert_eq!(
        route.twin_state(
            &[report("a", 0.5, 1, 1001, StateSource::Measured)],
            false,
            0
        ),
        Err(PoseError::Incomplete {
            missing: vec![],
            stale: vec!["a".into()]
        })
    );
}

#[test]
fn what_is_missing_and_what_is_stale_are_listed_in_the_order_of_the_wanted_joints() {
    let route = PoseRoute::new(
        PathBuf::from("never-started"),
        PathBuf::from("robot.urdf"),
        "arm".into(),
        PosesCfg {
            joints: vec!["c".into(), "a".into(), "d".into(), "b".into()],
            max_age_ms: 100,
            timeout_ms: 1000,
        },
    );
    let reports = vec![
        report("b", 0.0, 1, 500, StateSource::Measured),
        report("a", 0.0, 1, 5, StateSource::Measured),
        report("e", 0.0, 1, 5, StateSource::Measured),
    ];
    assert_eq!(
        route.twin_state(&reports, false, 0),
        Err(PoseError::Incomplete {
            missing: vec!["c".into(), "d".into()],
            stale: vec!["b".into()]
        })
    );
}

#[test]
fn the_dates_around_the_leap_days_and_the_ends_of_the_years_are_the_calendar_dates() {
    for (ms, expected) in [
        (951_868_800_000u64, "2000-03-01T00:00:00.000Z"),
        (4_107_542_399_999, "2100-02-28T23:59:59.999Z"),
        (4_107_542_400_000, "2100-03-01T00:00:00.000Z"),
        (1_735_689_599_999, "2024-12-31T23:59:59.999Z"),
        (946_684_799_001, "1999-12-31T23:59:59.001Z"),
        (2_147_483_648_000, "2038-01-19T03:14:08.000Z"),
        (13_574_606_400_500, "2400-02-29T12:00:00.500Z"),
        (999, "1970-01-01T00:00:00.999Z"),
    ] {
        assert_eq!(rfc3339_utc(ms), expected);
    }
}

#[test]
fn every_date_has_the_same_shape_and_grows_with_the_time() {
    // a fixed seed walks a few thousand moments across a century: always 24 characters, and a later moment never sorts earlier
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    let mut moments: Vec<u64> = (0..2000)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % 4_102_444_800_000
        })
        .collect();
    moments.sort_unstable();
    let mut previous = String::new();
    for ms in moments {
        let text = rfc3339_utc(ms);
        assert_eq!(text.len(), 24, "{text}");
        assert!(text.ends_with('Z') && text.as_bytes()[10] == b'T', "{text}");
        assert!(text >= previous, "{previous} then {text}");
        previous = text;
    }
}
