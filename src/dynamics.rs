// =============================================================================
// HYDRA-UMC-TWIN - src/dynamics.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0 - see LICENSE
// =============================================================================
//! A simple physics model of a robot that does not exist: every joint follows the position it is ordered to, the way a real joint does,
//! with a top speed, a top acceleration and the two ends of its travel.
//!
//! By default this is not a rigid-body model: a joint is a point that moves on a line and obeys three limits, which is what the
//! hardware-in-the-loop bridge needs to be exercised without a robot: a command that is delivered makes the joint move over time (it does
//! not jump), it never leaves its travel, it never goes faster than its limits and it stops at the position it was sent to without
//! overshooting. The step is fixed and small, so the same orders and the same time steps give the same positions every time.
//!
//! A joint that has a `rigid` block is a rigid link instead: an inertia, a viscous damping, the torque of gravity on the link and a motor
//! that pulls the joint to its target with a proportional-derivative law that saturates at a top torque. The equation of motion is
//! `inertia * acceleration = motor torque - damping * velocity - gravity_torque * sin(position)` (the position is measured from the
//! hanging-down rest of the link), integrated with the fourth-order Runge-Kutta rule, the same equation and the same rule as one joint of
//! the articulated engine of PHYSICS-REPLICA (`rigidbody.rs`), which `compare-dynamics` of HIL-BRIDGE compares against a recording. What it
//! does not model: the coupling between joints (each rigid joint is its own link), contacts, friction other than viscous, and flexible
//! parts; the numbers are those of a model, not measurements of a robot, so anything built on it is evidence=simulator.
//!
//! The model is read from a JSON file (`--sim-config`), which is where the numbers of the robot belong:
//! `{"joints":[{"name":"elbow","lower":-1.5,"upper":1.5,"max_velocity":1.0,"max_acceleration":4.0,"initial":0.0,
//! "rigid":{"inertia":0.2,"damping":0.05,"gravity_torque":2.0,"kp":50.0,"kd":5.0,"max_torque":20.0}}]}`.

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

/// The rigid link of a joint (see the header of this file). Every number is in SI units: kg m^2, N m s/rad, N m, N m/rad, N m s/rad, N m.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RigidSpec {
    /// The inertia of the link about the joint axis (above zero).
    pub inertia: f64,
    /// The viscous damping of the joint (zero or more).
    #[serde(default)]
    pub damping: f64,
    /// The largest torque that gravity makes on the link, mass * g * distance of the centre of mass (zero for a horizontal axis).
    #[serde(default)]
    pub gravity_torque: f64,
    /// The stiffness of the motor towards the target (zero or more).
    #[serde(default)]
    pub kp: f64,
    /// The damping of the motor against the speed (zero or more).
    #[serde(default)]
    pub kd: f64,
    /// The most torque the motor gives (above zero).
    pub max_torque: f64,
}

/// A step of `MAX_SUBSTEP_S` is stable for the Runge-Kutta rule when the motor stiffness and damping per unit of inertia stay below these.
const MAX_STIFFNESS_PER_INERTIA: f64 = (2.0 / MAX_SUBSTEP_S) * (2.0 / MAX_SUBSTEP_S);
const MAX_DAMPING_PER_INERTIA: f64 = 2.0 / MAX_SUBSTEP_S;
/// A rigid joint that moves slower and accelerates less than this has come to rest.
const REST_VELOCITY: f64 = 1e-4;
const REST_ACCELERATION: f64 = 1e-3;

impl RigidSpec {
    fn validate(&self, joint: &str) -> Result<(), String> {
        let finite = [
            self.inertia,
            self.damping,
            self.gravity_torque,
            self.kp,
            self.kd,
            self.max_torque,
        ]
        .iter()
        .all(|v| v.is_finite());
        if !finite {
            return Err(format!(
                "the rigid link of the joint {joint} has a number that is not finite"
            ));
        }
        if self.inertia <= 0.0 || self.max_torque <= 0.0 {
            return Err(format!(
                "the rigid link of the joint {joint} needs a positive inertia and max_torque"
            ));
        }
        if self.damping < 0.0 || self.gravity_torque < 0.0 || self.kp < 0.0 || self.kd < 0.0 {
            return Err(format!(
                "the rigid link of the joint {joint} needs damping, gravity_torque, kp and kd of zero or more"
            ));
        }
        if self.kp / self.inertia > MAX_STIFFNESS_PER_INERTIA
            || (self.kd + self.damping) / self.inertia > MAX_DAMPING_PER_INERTIA
        {
            return Err(format!(
                "the rigid link of the joint {joint} is too stiff for the step of the simulation (kp / inertia at most {MAX_STIFFNESS_PER_INERTIA}, (kd + damping) / inertia at most {MAX_DAMPING_PER_INERTIA})"
            ));
        }
        Ok(())
    }

    /// The angular acceleration of the link for a position, a speed and the target the motor pulls to.
    fn acceleration(&self, target: f64, position: f64, velocity: f64) -> f64 {
        let motor = (self.kp * (target - position) - self.kd * velocity)
            .clamp(-self.max_torque, self.max_torque);
        (motor - self.damping * velocity - self.gravity_torque * position.sin()) / self.inertia
    }
}

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
    /// A rigid link in place of the kinematic joint (None: the joint follows its limits only).
    #[serde(default)]
    pub rigid: Option<RigidSpec>,
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
            if let Some(rigid) = &joint.rigid {
                rigid.validate(&joint.name)?;
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
    /// The acceleration of the last step (only a rigid joint keeps it: it tells a joint at rest from one that is about to move).
    acceleration: f64,
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
                        acceleration: 0.0,
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
        if !seconds.is_finite() || !(0.0..=MAX_ADVANCE_S).contains(&seconds) {
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
                if joint.spec.rigid.is_some() {
                    step_rigid(joint, dt);
                } else {
                    step_joint(joint, dt);
                }
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
        // a rigid joint is held where it is: the damping of its motor brakes it, as the inertia and the torque decide
        let braking = if entry.spec.rigid.is_some() {
            0.0
        } else {
            entry.velocity * entry.velocity.abs() / (2.0 * entry.spec.max_acceleration)
        };
        let target = (entry.position + braking).clamp(entry.spec.lower, entry.spec.upper);
        entry.target = target;
        Ok(CommandOutcome {
            joint: joint.to_string(),
            target,
            clamped: false,
        })
    }

    pub fn position(&self, joint: &str) -> Option<f64> {
        self.joints.get(joint).map(|j| j.position)
    }
}

fn settled(joint: &Joint) -> bool {
    if joint.spec.rigid.is_some() {
        // gravity bends the link under its motor, so a rigid joint rests where it stops, not necessarily on its target
        return joint.velocity.abs() <= REST_VELOCITY
            && joint.acceleration.abs() <= REST_ACCELERATION;
    }
    (joint.target - joint.position).abs() <= SETTLE_DISTANCE
        && joint.velocity.abs() <= SETTLE_VELOCITY
}

/// One step of a rigid joint with the classical fourth-order Runge-Kutta rule. The speed never passes the top speed of the joint and the joint
/// never leaves its travel (it stops at an end, and an end that the torque pushes against is a rest).
fn step_rigid(joint: &mut Joint, dt: f64) {
    let Some(rigid) = joint.spec.rigid else {
        return;
    };
    let (x, v, target) = (joint.position, joint.velocity, joint.target);
    let a1 = rigid.acceleration(target, x, v);
    let (x1, v1) = (x + 0.5 * dt * v, v + 0.5 * dt * a1);
    let a2 = rigid.acceleration(target, x1, v1);
    let (x2, v2) = (x + 0.5 * dt * v1, v + 0.5 * dt * a2);
    let a3 = rigid.acceleration(target, x2, v2);
    let (x3, v3) = (x + dt * v2, v + dt * a3);
    let a4 = rigid.acceleration(target, x3, v3);
    joint.position = x + dt / 6.0 * (v + 2.0 * v1 + 2.0 * v2 + v3);
    joint.velocity = (v + dt / 6.0 * (a1 + 2.0 * a2 + 2.0 * a3 + a4))
        .clamp(-joint.spec.max_velocity, joint.spec.max_velocity);
    joint.acceleration = rigid.acceleration(target, joint.position, joint.velocity);
    if joint.position <= joint.spec.lower {
        joint.position = joint.spec.lower;
        joint.velocity = joint.velocity.max(0.0);
        if joint.acceleration < 0.0 {
            joint.acceleration = 0.0;
        }
    } else if joint.position >= joint.spec.upper {
        joint.position = joint.spec.upper;
        joint.velocity = joint.velocity.min(0.0);
        if joint.acceleration > 0.0 {
            joint.acceleration = 0.0;
        }
    }
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

    fn rigid_robot(rigid: &str, initial: f64, target: Option<f64>) -> SimulatedRobot {
        let config = SimConfig::parse(&format!(
            r#"{{"joints":[{{"name":"link","lower":-3.0,"upper":3.0,"max_velocity":20,"max_acceleration":50,"initial":{initial},"rigid":{rigid}}}]}}"#
        ))
        .unwrap();
        let mut robot = SimulatedRobot::new(&config);
        if let Some(target) = target {
            robot.command("link", target).unwrap();
        }
        robot
    }

    fn link(robot: &SimulatedRobot) -> JointSnapshot {
        robot.snapshot().into_iter().next().unwrap()
    }

    const FREE_LINK: &str = r#"{"inertia":0.1,"gravity_torque":1.0,"max_torque":10.0}"#;

    #[test]
    fn a_free_rigid_link_swings_with_the_period_of_a_pendulum() {
        // small oscillations: omega = sqrt(gravity_torque / inertia)
        let omega = (1.0f64 / 0.1).sqrt();
        let period = 2.0 * std::f64::consts::PI / omega;
        let mut robot = rigid_robot(FREE_LINK, 0.05, None);
        robot.advance(period / 4.0).unwrap();
        assert!(
            link(&robot).position.abs() < 1e-3,
            "a quarter period later it passes through the rest: {}",
            link(&robot).position
        );
        robot.advance(period / 4.0).unwrap();
        assert!((link(&robot).position + 0.05).abs() < 1e-3);
        robot.advance(period / 2.0).unwrap();
        assert!((link(&robot).position - 0.05).abs() < 1e-3);
    }

    #[test]
    fn a_free_rigid_link_keeps_its_energy() {
        let energy = |robot: &SimulatedRobot| {
            let joint = link(robot);
            0.5 * 0.1 * joint.velocity * joint.velocity + 1.0 * (1.0 - joint.position.cos())
        };
        let mut robot = rigid_robot(FREE_LINK, 1.0, None);
        let start = energy(&robot);
        for _ in 0..40 {
            robot.advance(0.25).unwrap();
            assert!(
                (energy(&robot) - start).abs() < 1e-6 * start,
                "the energy drifted: {} against {start}",
                energy(&robot)
            );
        }
    }

    #[test]
    fn damping_takes_energy_out_and_the_link_comes_to_rest() {
        let damped = r#"{"inertia":0.1,"damping":0.2,"gravity_torque":1.0,"max_torque":10.0}"#;
        let mut robot = rigid_robot(damped, 1.0, None);
        let mut last = f64::MAX;
        for _ in 0..4 {
            robot.advance(5.0).unwrap();
            let joint = link(&robot);
            let energy = 0.5 * 0.1 * joint.velocity * joint.velocity + (1.0 - joint.position.cos());
            assert!(
                energy < last,
                "the energy must fall: {energy} against {last}"
            );
            last = energy;
        }
        robot.advance(60.0).unwrap();
        let joint = link(&robot);
        assert!(joint.position.abs() < 1e-3 && joint.velocity.abs() < 1e-3);
        assert!(!joint.moving, "a link at rest is not moving");
    }

    #[test]
    fn a_motor_holds_the_link_under_gravity_with_the_sag_that_its_stiffness_leaves() {
        let motor = r#"{"inertia":0.2,"damping":0.05,"gravity_torque":2.0,"kp":50.0,"kd":5.0,"max_torque":20.0}"#;
        let target = 0.5;
        let mut robot = rigid_robot(motor, 0.0, Some(target));
        robot.advance(30.0).unwrap();
        // the rest where kp * (target - x) = gravity_torque * sin(x), found by bisection
        let (mut low, mut high) = (0.0f64, target);
        for _ in 0..80 {
            let mid = 0.5 * (low + high);
            if 50.0 * (target - mid) - 2.0 * mid.sin() > 0.0 {
                low = mid;
            } else {
                high = mid;
            }
        }
        let joint = link(&robot);
        assert!(
            (joint.position - low).abs() < 1e-6,
            "{} against {low}",
            joint.position
        );
        assert!(
            joint.position < target,
            "gravity sags the link below the target"
        );
        assert!(!joint.moving);
    }

    #[test]
    fn the_motor_cannot_give_more_than_its_top_torque() {
        // a stiffness that asks for far more than the motor gives: the acceleration is the top torque over the inertia (gravity is zero here)
        let weak = r#"{"inertia":0.5,"kp":1000.0,"max_torque":2.0}"#;
        let mut robot = rigid_robot(weak, 0.0, Some(2.0));
        robot.advance(0.1).unwrap();
        let expected_velocity = 2.0 / 0.5 * 0.1;
        let expected_position = 0.5 * (2.0 / 0.5) * 0.1 * 0.1;
        let joint = link(&robot);
        assert!(
            (joint.velocity - expected_velocity).abs() < 1e-9,
            "{}",
            joint.velocity
        );
        assert!(
            (joint.position - expected_position).abs() < 1e-9,
            "{}",
            joint.position
        );
    }

    #[test]
    fn a_rigid_link_stops_at_the_end_of_its_travel() {
        let strong = r#"{"inertia":0.1,"kp":100.0,"max_torque":50.0}"#;
        let config = SimConfig::parse(&format!(
            r#"{{"joints":[{{"name":"link","lower":-1.0,"upper":1.0,"max_velocity":20,"max_acceleration":50,"rigid":{strong}}}]}}"#
        ))
        .unwrap();
        let mut robot = SimulatedRobot::new(&config);
        robot.command("link", 5.0).unwrap(); // pulled back to the end of the travel
        let mut highest = 0.0f64;
        for _ in 0..800 {
            robot.advance(0.005).unwrap();
            highest = highest.max(robot.position("link").unwrap());
        }
        assert!(highest <= 1.0, "it left the travel: {highest}");
        assert_eq!(robot.position("link"), Some(1.0));
        assert!(!link(&robot).moving, "pushing against the end is a rest");
    }

    #[test]
    fn a_stop_holds_a_rigid_link_where_it_is_and_the_motor_brakes_it() {
        let motor = r#"{"inertia":0.2,"gravity_torque":0.0,"kp":50.0,"kd":10.0,"max_torque":20.0}"#;
        let mut robot = rigid_robot(motor, 0.0, Some(1.0));
        robot.advance(0.1).unwrap();
        let before = link(&robot);
        assert!(before.moving && before.velocity > 0.0);
        let outcome = robot.stop("link").unwrap();
        assert_eq!(outcome.target, before.position);
        robot.advance(10.0).unwrap();
        let after = link(&robot);
        assert!(
            after.position >= before.position - 1e-9,
            "it did not go back"
        );
        assert!(
            after.position < 1.0,
            "it did not carry on to the old target"
        );
        assert!(!after.moving);
    }

    #[test]
    fn a_rigid_joint_and_a_kinematic_one_live_in_the_same_robot() {
        let config = SimConfig::parse(
            r#"{"joints":[
                {"name":"a_kinematic","lower":-1,"upper":1,"max_velocity":1,"max_acceleration":4},
                {"name":"b_rigid","lower":-1,"upper":1,"max_velocity":5,"max_acceleration":4,"rigid":{"inertia":0.1,"kp":20,"kd":2,"max_torque":5}}
            ]}"#,
        )
        .unwrap();
        let mut robot = SimulatedRobot::new(&config);
        robot.command("a_kinematic", 0.5).unwrap();
        robot.command("b_rigid", 0.5).unwrap();
        robot.advance(20.0).unwrap();
        assert_eq!(robot.position("a_kinematic"), Some(0.5));
        assert!((robot.position("b_rigid").unwrap() - 0.5).abs() < 1e-6);
        assert!(robot.snapshot().iter().all(|j| !j.moving));
    }

    #[test]
    fn the_same_orders_give_the_same_positions_every_time() {
        let motor = r#"{"inertia":0.2,"damping":0.05,"gravity_torque":2.0,"kp":50.0,"kd":5.0,"max_torque":20.0}"#;
        let run = || {
            let mut robot = rigid_robot(motor, 0.1, Some(0.7));
            for _ in 0..200 {
                robot.advance(0.013).unwrap();
            }
            let joint = link(&robot);
            (joint.position.to_bits(), joint.velocity.to_bits())
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn a_rigid_block_with_a_wrong_number_is_refused() {
        let bad = |rigid: &str| {
            SimConfig::parse(&format!(
                r#"{{"joints":[{{"name":"j","lower":-1,"upper":1,"max_velocity":1,"max_acceleration":1,"rigid":{rigid}}}]}}"#
            ))
        };
        assert!(bad(r#"{"inertia":0.1,"max_torque":1}"#).is_ok());
        for refused in [
            r#"{"inertia":0,"max_torque":1}"#,
            r#"{"inertia":-1,"max_torque":1}"#,
            r#"{"inertia":0.1,"max_torque":0}"#,
            r#"{"inertia":0.1,"max_torque":1,"damping":-0.1}"#,
            r#"{"inertia":0.1,"max_torque":1,"gravity_torque":-1}"#,
            r#"{"inertia":0.1,"max_torque":1,"kp":-1}"#,
            r#"{"inertia":0.1,"max_torque":1,"kd":-1}"#,
            r#"{"inertia":0.1,"max_torque":1,"mass":3}"#,
            r#"{"max_torque":1}"#,
            r#"{"inertia":0.1}"#,
        ] {
            assert!(bad(refused).is_err(), "should be refused: {refused}");
        }
    }

    #[test]
    fn a_motor_too_stiff_for_the_step_is_refused_not_left_to_blow_up() {
        let stiff_limit = MAX_STIFFNESS_PER_INERTIA * 0.1;
        let inside = stiff_limit * 0.99;
        let make = |kp: f64, kd: f64| {
            SimConfig::parse(&format!(
                r#"{{"joints":[{{"name":"j","lower":-1,"upper":1,"max_velocity":1,"max_acceleration":1,"rigid":{{"inertia":0.1,"kp":{kp},"kd":{kd},"max_torque":1}}}}]}}"#
            ))
        };
        assert!(make(inside, 0.0).is_ok());
        assert!(make(stiff_limit * 1.01, 0.0).is_err());
        assert!(make(1.0, MAX_DAMPING_PER_INERTIA * 0.1 * 1.01).is_err());
        assert!(make(1.0, MAX_DAMPING_PER_INERTIA * 0.1 * 0.99).is_ok());
    }

    #[test]
    fn the_stiffest_motor_that_is_accepted_stays_stable() {
        let stiff = MAX_STIFFNESS_PER_INERTIA * 0.1 * 0.99;
        let rigid = format!(r#"{{"inertia":0.1,"kp":{stiff},"kd":0.0,"max_torque":1000}}"#);
        let mut robot = rigid_robot(&rigid, 0.0, Some(0.001));
        for _ in 0..400 {
            robot.advance(0.005).unwrap();
            let joint = link(&robot);
            assert!(joint.position.is_finite() && joint.position.abs() < 1.0);
        }
    }
}
