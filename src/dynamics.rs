// =============================================================================
// HYDRA-UMC-TWIN - src/dynamics.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0 - see LICENSE
// =============================================================================
//! A simple physics model of a robot that does not exist: every joint follows the position it is ordered to, the way a real joint does,
//! with a top speed, a top acceleration and the two ends of its travel.
//!
//! This is not a rigid-body engine: there are no masses, no gravity, no contacts and no motor model. Each joint is a point that moves on a
//! line and obeys three limits, which is what the hardware-in-the-loop bridge needs to be exercised without a robot: a command that is
//! delivered makes the joint move over time (it does not jump), it never leaves its travel, it never goes faster than its limits and it
//! stops at the position it was sent to without overshooting. The step is fixed and small, so the same orders and the same time steps give
//! the same positions every time.
//!
//! The model is read from a JSON file (`--sim-config`), which is where the numbers of the robot belong:
//! `{"joints":[{"name":"elbow","lower":-1.5,"upper":1.5,"max_velocity":1.0,"max_acceleration":4.0,"initial":0.0}]}`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The longest a single integration step is (seconds): a longer interval is cut into steps of at most this.
pub const MAX_SUBSTEP_S: f64 = 0.005;
/// The most a single call may simulate (seconds), so that one request cannot hold a worker for long.
pub const MAX_ADVANCE_S: f64 = 60.0;
/// How many joints a model may have (the same bound as the store of the real joints).
pub const MAX_SIM_JOINTS: usize = 256;
/// Closer than this to the target, and slower than `SETTLE_VELOCITY`, a joint has arrived (radians or metres).
const SETTLE_DISTANCE: f64 = 1e-6;
const SETTLE_VELOCITY: f64 = 1e-5;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JointSpec {
    pub name: String,
    pub lower: f64,
    pub upper: f64,
    pub max_velocity: f64,
    pub max_acceleration: f64,
    #[serde(default)]
    pub initial: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimConfig {
    pub joints: Vec<JointSpec>,
}

impl SimConfig {
    pub fn parse(text: &str) -> Result<SimConfig, String> {
        let config: SimConfig = serde_json::from_str(text)
            .map_err(|e| format!("the simulation file is not valid: {e}"))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.joints.is_empty() {
            return Err("the simulation needs at least one joint".into());
        }
        if self.joints.len() > MAX_SIM_JOINTS {
            return Err(format!(
                "the simulation has more than {MAX_SIM_JOINTS} joints"
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for joint in &self.joints {
            if joint.name.is_empty() || joint.name.len() > crate::joint_state::MAX_JOINT_NAME_LEN {
                return Err("a joint name must have 1 to 64 characters".into());
            }
            if !seen.insert(joint.name.clone()) {
                return Err(format!("the joint {} is listed twice", joint.name));
            }
            let finite = [
                joint.lower,
                joint.upper,
                joint.max_velocity,
                joint.max_acceleration,
                joint.initial,
            ]
            .iter()
            .all(|v| v.is_finite());
            if !finite {
                return Err(format!(
                    "the joint {} has a number that is not finite",
                    joint.name
                ));
            }
            if joint.lower >= joint.upper {
                return Err(format!("the joint {} needs lower < upper", joint.name));
            }
            if joint.max_velocity <= 0.0 || joint.max_acceleration <= 0.0 {
                return Err(format!(
                    "the joint {} needs a positive max_velocity and max_acceleration",
                    joint.name
                ));
            }
            if joint.initial < joint.lower || joint.initial > joint.upper {
                return Err(format!(
                    "the joint {} starts outside its travel",
                    joint.name
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct JointSnapshot {
    pub joint: String,
    pub position: f64,
    pub velocity: f64,
    pub target: f64,
    /// Whether the joint is still on its way (it has not settled at the target).
    pub moving: bool,
}

#[derive(Debug, Clone)]
struct Joint {
    spec: JointSpec,
    position: f64,
    velocity: f64,
    target: f64,
}

/// What a command did: the target that was kept and whether it had to be pulled back inside the travel of the joint.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CommandOutcome {
    pub joint: String,
    pub target: f64,
    pub clamped: bool,
}

#[derive(Debug, Clone)]
pub struct SimulatedRobot {
    joints: BTreeMap<String, Joint>,
    /// Total simulated time (seconds).
    clock_s: f64,
}

impl SimulatedRobot {
    pub fn new(config: &SimConfig) -> SimulatedRobot {
        let joints = config
            .joints
            .iter()
            .map(|spec| {
                (
                    spec.name.clone(),
                    Joint {
                        spec: spec.clone(),
                        position: spec.initial,
                        velocity: 0.0,
                        target: spec.initial,
                    },
                )
            })
            .collect();
        SimulatedRobot {
            joints,
            clock_s: 0.0,
        }
    }

    pub fn clock_s(&self) -> f64 {
        self.clock_s
    }

    /// Order a joint to a position. A position outside the travel is pulled back to the nearest end (and said so); a position that is not a
    /// finite number, or a joint that does not exist, is refused.
    pub fn command(&mut self, joint: &str, position: f64) -> Result<CommandOutcome, String> {
        if !position.is_finite() {
            return Err("the position must be a finite number".into());
        }
        let Some(entry) = self.joints.get_mut(joint) else {
            return Err(format!("unknown joint: {joint}"));
        };
        let target = position.clamp(entry.spec.lower, entry.spec.upper);
        entry.target = target;
        Ok(CommandOutcome {
            joint: joint.to_string(),
            target,
            clamped: target != position,
        })
    }

    /// Let `seconds` pass (cut into steps of at most `MAX_SUBSTEP_S`).
    pub fn advance(&mut self, seconds: f64) -> Result<(), String> {
        if !seconds.is_finite() || seconds < 0.0 || seconds > MAX_ADVANCE_S {
            return Err(format!(
                "the time to advance must be between 0 and {MAX_ADVANCE_S} seconds"
            ));
        }
        let steps = (seconds / MAX_SUBSTEP_S).ceil().max(0.0) as u64;
        if steps == 0 {
            return Ok(());
        }
        let dt = seconds / steps as f64;
        for _ in 0..steps {
            for joint in self.joints.values_mut() {
                step_joint(joint, dt);
            }
        }
        self.clock_s += seconds;
        Ok(())
    }

    pub fn snapshot(&self) -> Vec<JointSnapshot> {
        self.joints
            .values()
            .map(|joint| JointSnapshot {
                joint: joint.spec.name.clone(),
                position: joint.position,
                velocity: joint.velocity,
                target: joint.target,
                moving: !settled(joint),
            })
            .collect()
    }

    /// Stop a joint where it can: the target becomes the position it reaches braking with its top deceleration (inside its travel), so the joint
    /// slows down at once and does not go back to where it was. This is what the watchdog of the bridge asks for when a command lease runs out.
    pub fn stop(&mut self, joint: &str) -> Result<CommandOutcome, String> {
        let Some(entry) = self.joints.get_mut(joint) else {
            return Err(format!("unknown joint: {joint}"));
        };
        let braking = entry.velocity * entry.velocity.abs() / (2.0 * entry.spec.max_acceleration);
        let target = (entry.position + braking).clamp(entry.spec.lower, entry.spec.upper);
        entry.target = target;
        Ok(CommandOutcome {
            joint: joint.to_string(),
            target,
            clamped: false,
        })
    }

    #[cfg(test)]
    pub fn position(&self, joint: &str) -> Option<f64> {
        self.joints.get(joint).map(|j| j.position)
    }
}

fn settled(joint: &Joint) -> bool {
    (joint.target - joint.position).abs() <= SETTLE_DISTANCE
        && joint.velocity.abs() <= SETTLE_VELOCITY
}

/// One step of one joint. The speed after the step is the fastest one toward the target from which the joint can still stop on it with its top
/// deceleration (worked out for the position it will have reached by then, with the same trapezoid rule that moves it), limited to the top
/// speed and to a change of at most the top acceleration. So the joint accelerates, cruises, brakes and stops on the target without
/// overshooting it and without a jump of speed; a new order that sends it back brakes it first. It never leaves the travel. A joint next to its
/// target that can come to rest within this very step rests on it.
fn step_joint(joint: &mut Joint, dt: f64) {
    let error = joint.target - joint.position;
    let a_max = joint.spec.max_acceleration;
    let v_max = joint.spec.max_velocity;
    // close enough to come to rest on the target within this very step: rest on it (also what stops a joint with a tiny travel from trembling)
    if error.abs() <= SETTLE_DISTANCE.max(0.5 * a_max * dt * dt)
        && joint.velocity.abs() <= a_max * dt
    {
        joint.position = joint.target;
        joint.velocity = 0.0;
        return;
    }
    // work toward the target as "forward": `sign` turns the line around when the target is below the joint
    let sign = if error >= 0.0 { 1.0 } else { -1.0 };
    let remaining = error.abs();
    let speed = joint.velocity * sign;
    let step_speed = a_max * dt;
    // the largest speed `w` after this step with  remaining - (speed + w) * dt / 2 >= w^2 / (2 * a_max)
    let discriminant =
        step_speed * step_speed - 4.0 * (step_speed * speed - 2.0 * a_max * remaining);
    let feasible = if discriminant >= 0.0 {
        (-step_speed + discriminant.sqrt()) / 2.0
    } else {
        f64::NEG_INFINITY
    };
    let next_speed = feasible
        .min(v_max)
        .clamp(speed - step_speed, speed + step_speed);
    let next_velocity = next_speed * sign;
    let next_position = joint.position + 0.5 * (joint.velocity + next_velocity) * dt;
    joint.position = next_position;
    joint.velocity = next_velocity;
    if joint.position <= joint.spec.lower {
        joint.position = joint.spec.lower;
        joint.velocity = joint.velocity.max(0.0);
    } else if joint.position >= joint.spec.upper {
        joint.position = joint.spec.upper;
        joint.velocity = joint.velocity.min(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> SimConfig {
        SimConfig::parse(
            r#"{"joints":[
                {"name":"elbow","lower":-1.5,"upper":1.5,"max_velocity":1.0,"max_acceleration":4.0},
                {"name":"base","lower":-3.0,"upper":3.0,"max_velocity":2.0,"max_acceleration":8.0,"initial":1.0}
            ]}"#,
        )
        .unwrap()
    }

    fn robot() -> SimulatedRobot {
        SimulatedRobot::new(&config())
    }

    /// The joints are kept in the order of their names ("base" comes before "elbow"), so a test looks one up by name.
    fn elbow_of(robot: &SimulatedRobot) -> JointSnapshot {
        robot
            .snapshot()
            .into_iter()
            .find(|j| j.joint == "elbow")
            .unwrap()
    }

    #[test]
    fn a_joint_starts_where_the_model_says_and_at_rest() {
        let robot = robot();
        let base = robot
            .snapshot()
            .into_iter()
            .find(|j| j.joint == "base")
            .unwrap();
        assert_eq!(
            (base.position, base.velocity, base.target, base.moving),
            (1.0, 0.0, 1.0, false)
        );
        assert_eq!(robot.position("elbow"), Some(0.0));
    }

    #[test]
    fn a_command_moves_the_joint_over_time_and_it_does_not_jump() {
        let mut robot = robot();
        robot.command("elbow", 1.0).unwrap();
        robot.advance(0.05).unwrap();
        let early = robot.position("elbow").unwrap();
        assert!(
            early > 0.0 && early < 0.05,
            "after 50 ms the joint has only started: {early}"
        );
        assert!(elbow_of(&robot).moving);
    }

    #[test]
    fn the_joint_arrives_at_the_target_without_overshooting_it() {
        let mut robot = robot();
        robot.command("elbow", 1.2).unwrap();
        let mut highest = 0.0f64;
        for _ in 0..4000 {
            robot.advance(0.005).unwrap();
            highest = highest.max(robot.position("elbow").unwrap());
        }
        assert!(highest <= 1.2 + 1e-9, "it went past the target: {highest}");
        let elbow = robot
            .snapshot()
            .into_iter()
            .find(|j| j.joint == "elbow")
            .unwrap();
        assert_eq!(elbow.position, 1.2);
        assert_eq!(elbow.velocity, 0.0);
        assert!(!elbow.moving);
    }

    #[test]
    fn the_speed_and_the_acceleration_never_pass_their_limits() {
        let mut robot = robot();
        robot.command("elbow", 1.5).unwrap();
        let (mut last_v, mut peak_v, mut peak_a) = (0.0f64, 0.0f64, 0.0f64);
        for _ in 0..2000 {
            robot.advance(0.005).unwrap();
            let v = robot
                .snapshot()
                .into_iter()
                .find(|j| j.joint == "elbow")
                .unwrap()
                .velocity;
            peak_v = peak_v.max(v.abs());
            peak_a = peak_a.max(((v - last_v) / 0.005).abs());
            last_v = v;
        }
        assert!(peak_v <= 1.0 + 1e-9, "top speed passed: {peak_v}");
        assert!(peak_a <= 4.0 + 1e-6, "top acceleration passed: {peak_a}");
        assert!(peak_v > 0.9, "a long move reaches its top speed: {peak_v}");
    }

    #[test]
    fn a_position_outside_the_travel_is_pulled_back_and_the_joint_never_leaves_it() {
        let mut robot = robot();
        let outcome = robot.command("elbow", 9.0).unwrap();
        assert_eq!(
            outcome,
            CommandOutcome {
                joint: "elbow".into(),
                target: 1.5,
                clamped: true
            }
        );
        for _ in 0..3000 {
            robot.advance(0.01).unwrap();
            let position = robot.position("elbow").unwrap();
            assert!((-1.5..=1.5).contains(&position));
        }
        assert_eq!(robot.position("elbow"), Some(1.5));
        assert!(!robot.command("elbow", 0.5).unwrap().clamped);
    }

    #[test]
    fn a_new_order_in_mid_move_turns_the_joint_around_within_its_limits() {
        let mut robot = robot();
        robot.command("elbow", 1.4).unwrap();
        robot.advance(0.4).unwrap();
        let moving_up = elbow_of(&robot).velocity;
        assert!(moving_up > 0.0);
        robot.command("elbow", -1.0).unwrap();
        let mut last = moving_up;
        for _ in 0..4000 {
            robot.advance(0.005).unwrap();
            let v = elbow_of(&robot).velocity;
            assert!(
                ((v - last) / 0.005).abs() <= 4.0 + 1e-6,
                "a turnaround must not exceed the acceleration limit"
            );
            last = v;
        }
        assert_eq!(robot.position("elbow"), Some(-1.0));
    }

    #[test]
    fn the_same_orders_and_steps_give_the_same_positions_every_time() {
        let run = || {
            let mut robot = robot();
            robot.command("elbow", 0.7).unwrap();
            robot.command("base", -2.0).unwrap();
            let mut trace = Vec::new();
            for _ in 0..300 {
                robot.advance(0.0137).unwrap();
                trace.push((
                    robot.position("elbow").unwrap(),
                    robot.position("base").unwrap(),
                ));
            }
            trace
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn the_result_does_not_depend_on_how_the_time_is_cut_into_calls() {
        let mut once = robot();
        let mut many = robot();
        once.command("base", -2.5).unwrap();
        many.command("base", -2.5).unwrap();
        once.advance(1.0).unwrap();
        for _ in 0..100 {
            many.advance(0.01).unwrap();
        }
        let (a, b) = (
            once.position("base").unwrap(),
            many.position("base").unwrap(),
        );
        assert!((a - b).abs() < 1e-3, "{a} vs {b}");
        assert!((once.clock_s() - many.clock_s()).abs() < 1e-9);
    }

    #[test]
    fn bad_orders_and_bad_times_are_refused_and_change_nothing() {
        let mut robot = robot();
        assert!(robot
            .command("nope", 0.0)
            .unwrap_err()
            .contains("unknown joint"));
        assert!(robot.command("elbow", f64::NAN).is_err());
        assert!(robot.command("elbow", f64::INFINITY).is_err());
        assert!(robot.advance(-1.0).is_err());
        assert!(robot.advance(f64::NAN).is_err());
        assert!(robot.advance(MAX_ADVANCE_S + 1.0).is_err());
        assert!(robot.advance(0.0).is_ok());
        assert_eq!(robot.clock_s(), 0.0);
        assert_eq!(robot.position("elbow"), Some(0.0));
    }

    #[test]
    fn the_model_file_is_checked_closely() {
        for (text, needle) in [
            (r#"{"joints":[]}"#, "at least one"),
            (
                r#"{"joints":[{"name":"a","lower":1,"upper":0,"max_velocity":1,"max_acceleration":1}]}"#,
                "lower < upper",
            ),
            (
                r#"{"joints":[{"name":"a","lower":0,"upper":1,"max_velocity":0,"max_acceleration":1}]}"#,
                "positive",
            ),
            (
                r#"{"joints":[{"name":"a","lower":0,"upper":1,"max_velocity":1,"max_acceleration":1,"initial":2}]}"#,
                "outside its travel",
            ),
            (
                r#"{"joints":[{"name":"a","lower":0,"upper":1,"max_velocity":1,"max_acceleration":1},{"name":"a","lower":0,"upper":1,"max_velocity":1,"max_acceleration":1}]}"#,
                "twice",
            ),
            (
                r#"{"joints":[{"name":"a","lower":0,"upper":1,"max_velocity":1,"max_acceleration":1,"mass":3}]}"#,
                "not valid",
            ),
            ("not json", "not valid"),
        ] {
            let error = SimConfig::parse(text).unwrap_err();
            assert!(error.contains(needle), "{text}: {error}");
        }
    }

    #[test]
    fn a_stop_brakes_with_the_top_deceleration_and_does_not_go_back() {
        let mut robot = robot();
        robot.command("elbow", 1.4).unwrap();
        robot.advance(0.4).unwrap();
        let (position, velocity) = {
            let j = elbow_of(&robot);
            (j.position, j.velocity)
        };
        assert!(velocity > 0.5);
        let outcome = robot.stop("elbow").unwrap();
        // the distance to stop from that speed with 4 rad/s2
        let braking = velocity * velocity / (2.0 * 4.0);
        assert!((outcome.target - (position + braking)).abs() < 1e-9);
        let mut highest = position;
        for _ in 0..2000 {
            robot.advance(0.005).unwrap();
            highest = highest.max(robot.position("elbow").unwrap());
        }
        assert!(
            highest <= outcome.target + 1e-9,
            "it went past where it can stop"
        );
        assert!(
            highest > position,
            "it keeps moving the little it needs to brake"
        );
        assert_eq!(robot.position("elbow"), Some(outcome.target));
        assert!(!elbow_of(&robot).moving);
        assert!(robot.stop("nope").is_err());
    }

    #[test]
    fn a_joint_with_a_tiny_travel_settles_instead_of_trembling() {
        let config = SimConfig::parse(r#"{"joints":[{"name":"j","lower":0,"upper":0.001,"max_velocity":5,"max_acceleration":50}]}"#).unwrap();
        let mut robot = SimulatedRobot::new(&config);
        robot.command("j", 0.001).unwrap();
        robot.advance(1.0).unwrap();
        assert_eq!(robot.position("j"), Some(0.001));
        assert!(!robot.snapshot()[0].moving);
    }
}
