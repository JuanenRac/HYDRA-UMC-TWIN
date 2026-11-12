// =============================================================================
// HYDRA-UMC-TWIN - src/coupled/child_tests.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0-or-later - see LICENSE
// =============================================================================
//! The coupled joints of the simulated robot with a stand-in for the articulated engine: a small shell program that prints a prepared answer, or fails in each way
//! the real one can. What is checked: one call to the child for each interval (and none without the `coupled` block), the command line it gets, the state that
//! is taken from its answer with the guards of the model (travel, top speed), the robot that is left untouched when the child fails, and that a joint without a
//! rigid block still follows its own limits. Unix only: the stand-in is a shell script.

use super::*;
use crate::dynamics::{SimConfig, SimulatedRobot};
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
        let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
        let dir =
            std::env::temp_dir().join(format!("twin-coupled-child-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fake-physics");
        let body = format!(
            r#"#!/bin/sh
echo "$@" >> "{dir}/args.log"
mode=ok
if [ -f "{dir}/mode" ]; then mode=$(cat "{dir}/mode"); fi
case "$mode" in
  refuse) echo "simulate: unknown joint zz" >&2; exit 2 ;;
  crash) exit 7 ;;
  garbage) echo "not json at all"; exit 0 ;;
  slow) sleep 5; exit 0 ;;
esac
cat "{dir}/answer"
"#,
            dir = dir.display()
        );
        std::fs::write(&path, body).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).unwrap();
        Fake { binary: path, dir }
    }

    fn answer(&self, text: &str) {
        std::fs::write(self.dir.join("answer"), text).unwrap();
    }

    fn mode(&self, mode: &str) {
        std::fs::write(self.dir.join("mode"), mode).unwrap();
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.join("args.log"))
            .map(|t| t.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    fn robot(&self, rigid: &str) -> SimulatedRobot {
        let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
        let text = format!(
            r#"{{"coupled":{{"urdf":"arm.urdf","binary":"{}","timeout_ms":1000}},"joints":[
                {{"name":"a","lower":-1.5,"upper":1.5,"max_velocity":2.0,"max_acceleration":50,"rigid":{rigid}}},
                {{"name":"b","lower":-1.0,"upper":1.0,"max_velocity":3.0,"max_acceleration":50,"rigid":{rigid}}},
                {{"name":"free","lower":-3.0,"upper":3.0,"max_velocity":2.0,"max_acceleration":8.0}}
            ]}}"#,
            self.binary.display()
        );
        SimulatedRobot::new(&SimConfig::parse(&text).unwrap())
    }
}

const MOTOR: &str = r#"{"inertia":0.1,"kp":20,"kd":2,"max_torque":5}"#;
const ANSWER: &str = r#"{"joints":["a","b"],"dt_s":0.005,"steps":20,"samples":[{"t_s":0,"positions":[0,0],"velocities":[0,0]},{"t_s":0.1,"positions":[0.3,-0.2],"velocities":[1.5,-0.5]}]}"#;

fn state(robot: &SimulatedRobot, joint: &str) -> (f64, f64) {
    let j = robot
        .snapshot()
        .into_iter()
        .find(|j| j.joint == joint)
        .unwrap();
    (j.position, j.velocity)
}

#[test]
fn one_call_for_each_interval_and_the_state_comes_from_its_answer() {
    let fake = Fake::new("one-call");
    fake.answer(ANSWER);
    let mut robot = fake.robot(MOTOR);
    robot.command("a", 0.4).unwrap();
    robot.advance(0.1).unwrap();
    assert_eq!(fake.calls().len(), 1);
    assert_eq!(state(&robot, "a"), (0.3, 1.5));
    assert_eq!(state(&robot, "b"), (-0.2, -0.5));
    robot.advance(0.1).unwrap();
    assert_eq!(fake.calls().len(), 2);
    assert!((robot.clock_s() - 0.2).abs() < 1e-12);
    // the command line carries the state before the interval, the target and the motor numbers
    let second = &fake.calls()[1];
    assert!(second.contains("--joints a=0.3,b=-0.2"), "{second}");
    assert!(second.contains("--velocities a=1.5,b=-0.5"), "{second}");
    assert!(second.contains("--targets a=0.4,b=0.0"), "{second}");
    assert!(second.contains("--kp a=20.0,b=20.0"), "{second}");
    assert!(second.contains("--max-torque a=5.0,b=5.0"), "{second}");
}

#[test]
fn an_interval_of_no_time_makes_no_call() {
    let fake = Fake::new("no-time");
    fake.answer(ANSWER);
    let mut robot = fake.robot(MOTOR);
    robot.advance(0.0).unwrap();
    assert!(fake.calls().is_empty());
}

#[test]
fn the_travel_and_the_top_speed_still_hold() {
    let fake = Fake::new("guards");
    // the engine says: a beyond its upper end and too fast, b below its lower end and moving down
    fake.answer(r#"{"joints":["a","b"],"samples":[{"positions":[9,-9],"velocities":[7,-7]}]}"#);
    let mut robot = fake.robot(MOTOR);
    robot.advance(0.1).unwrap();
    assert_eq!(state(&robot, "a"), (1.5, 0.0)); // at the upper end it cannot keep going up
    assert_eq!(state(&robot, "b"), (-1.0, 0.0));
    fake.answer(r#"{"joints":["a","b"],"samples":[{"positions":[0.5,0.5],"velocities":[7,-7]}]}"#);
    robot.advance(0.1).unwrap();
    assert_eq!(state(&robot, "a"), (0.5, 2.0)); // top speed of a
    assert_eq!(state(&robot, "b"), (0.5, -3.0)); // top speed of b
}

#[test]
fn a_joint_without_a_rigid_block_follows_its_own_limits() {
    let fake = Fake::new("kinematic");
    fake.answer(ANSWER);
    let mut robot = fake.robot(MOTOR);
    robot.command("free", 1.0).unwrap();
    robot.advance(0.1).unwrap();
    let (position, velocity) = state(&robot, "free");
    assert!(
        position > 0.0 && position < 1.0 && velocity > 0.0,
        "{position} {velocity}"
    );
}

#[test]
fn a_failing_child_is_an_error_and_leaves_the_robot_where_it_was() {
    let fake = Fake::new("failures");
    fake.answer(ANSWER);
    let mut robot = fake.robot(MOTOR);
    robot.command("free", 1.0).unwrap();
    robot.advance(0.1).unwrap();
    let before = robot.snapshot();
    let clock = robot.clock_s();
    for mode in ["refuse", "crash", "garbage", "slow"] {
        fake.mode(mode);
        let error = robot.advance(0.1).unwrap_err();
        assert!(error.contains("coupled dynamics"), "{mode}: {error}");
        assert_eq!(robot.snapshot(), before, "{mode}");
        assert_eq!(robot.clock_s(), clock, "{mode}");
    }
    // and it works again when the child does
    fake.mode("ok");
    robot.advance(0.1).unwrap();
    assert!(robot.clock_s() > clock);
}

#[test]
fn an_answer_for_other_joints_than_the_rigid_ones_is_an_error() {
    let fake = Fake::new("wrong-joints");
    fake.answer(
        r#"{"joints":["a","b","c"],"samples":[{"positions":[0,0,0],"velocities":[0,0,0]}]}"#,
    );
    let mut robot = fake.robot(MOTOR);
    let error = robot.advance(0.1).unwrap_err();
    assert!(error.contains("exactly the movable joints"), "{error}");
    assert_eq!(robot.clock_s(), 0.0);
}

#[test]
fn without_the_block_the_rigid_joints_move_alone_and_no_child_is_started() {
    let fake = Fake::new("off");
    fake.answer(ANSWER);
    let text = r#"{"joints":[{"name":"a","lower":-3,"upper":3,"max_velocity":20,"max_acceleration":50,"rigid":{"inertia":0.1,"kp":20,"kd":2,"max_torque":5}}]}"#;
    let mut alone = SimulatedRobot::new(&SimConfig::parse(text).unwrap());
    alone.command("a", 0.5).unwrap();
    alone.advance(0.2).unwrap();
    let mut nulled = SimulatedRobot::new(
        &SimConfig::parse(&text.replace(r#"{"joints""#, r#"{"coupled":null,"joints""#)).unwrap(),
    );
    nulled.command("a", 0.5).unwrap();
    nulled.advance(0.2).unwrap();
    assert_eq!(alone.snapshot(), nulled.snapshot());
    assert!(fake.calls().is_empty());
    assert!(state(&alone, "a").0 > 0.0);
}

#[test]
fn a_link_with_nothing_to_couple_gives_what_the_model_gives() {
    // a single link without gravity or damping: the engine and the model of one link integrate the same equation, so when the child returns the state that the
    // model reaches, the coupled robot is that robot
    let text = r#"{"joints":[{"name":"a","lower":-3,"upper":3,"max_velocity":20,"max_acceleration":50,"rigid":{"inertia":0.1,"kp":20,"kd":2,"max_torque":5}}]}"#;
    let mut alone = SimulatedRobot::new(&SimConfig::parse(text).unwrap());
    alone.command("a", 0.5).unwrap();
    alone.advance(0.2).unwrap();
    let (position, velocity) = state(&alone, "a");

    let fake = Fake::new("equivalence");
    fake.answer(&format!(
        r#"{{"joints":["a"],"samples":[{{"positions":[{position:?}],"velocities":[{velocity:?}]}}]}}"#
    ));
    let coupled_text = text.replace(
        r#"{"joints""#,
        &format!(
            r#"{{"coupled":{{"urdf":"arm.urdf","binary":"{}"}},"joints""#,
            fake.binary.display()
        ),
    );
    let mut coupled = SimulatedRobot::new(&SimConfig::parse(&coupled_text).unwrap());
    coupled.command("a", 0.5).unwrap();
    coupled.advance(0.2).unwrap();
    assert_eq!(state(&coupled, "a"), (position, velocity));
    assert_eq!(fake.calls().len(), 1);
}

#[test]
fn a_coupled_block_needs_a_rigid_joint_and_a_valid_timeout() {
    let none = r#"{"coupled":{"urdf":"arm.urdf","binary":"x"},"joints":[{"name":"a","lower":-1,"upper":1,"max_velocity":1,"max_acceleration":1}]}"#;
    assert!(SimConfig::parse(none).is_err());
    let slow = r#"{"coupled":{"urdf":"arm.urdf","binary":"x","timeout_ms":5},"joints":[{"name":"a","lower":-1,"upper":1,"max_velocity":1,"max_acceleration":1,"rigid":{"inertia":0.1,"max_torque":1}}]}"#;
    assert!(SimConfig::parse(slow).is_err());
    let unknown = r#"{"coupled":{"urdf":"arm.urdf","binary":"x","other":1},"joints":[{"name":"a","lower":-1,"upper":1,"max_velocity":1,"max_acceleration":1,"rigid":{"inertia":0.1,"max_torque":1}}]}"#;
    assert!(SimConfig::parse(unknown).is_err());
}
