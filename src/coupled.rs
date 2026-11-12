// =============================================================================
// HYDRA-UMC-TWIN - src/coupled.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0-or-later - see LICENSE
// =============================================================================
//! The joints of the simulated robot that move together: the articulated rigid-body engine of HYDRA-UMC-PHYSICS-REPLICA, started as a process.
//!
//! By default every `rigid` joint of the simulation is a link of its own (`dynamics.rs`): its inertia and its gravity torque are numbers of the
//! model file and the others do not move it. With a `coupled` block in the model file (`{"coupled": {"urdf": "robot.urdf", "binary": "physics-replica"}}`)
//! the rigid joints are instead the joints of the robot of that URDF: the engine computes the mass matrix and the bias of the whole arm (the inertial
//! blocks of the URDF and gravity), so the motion of one joint pulls on the others. The motor of each joint stays what the file says (`kp`, `kd`, `max_torque`);
//! its `inertia`, `damping` and `gravity_torque` are not used then (the URDF and the engine give them). The joints without a `rigid` block follow their
//! limits as before.
//!
//! One call to the child is made for each interval the simulation advances (`simulate` with the targets and the motor numbers, once, for the whole
//! interval), never one per step. Nothing is guessed: a child that fails, does not end in time, answers something that is not the state of exactly the
//! rigid joints, or answers a number that is not finite is an error, and the simulated robot is left where it was. The binary and the URDF come from the
//! model file and never from a request. The numbers are those of a model, not measurements of a robot, so anything built on it is evidence=simulator.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use crate::collision::{run_with_timeout, CollisionError};

/// The time the child may take for one interval when the file does not say (milliseconds).
const DEFAULT_TIMEOUT_MS: u64 = 5000;
const MIN_TIMEOUT_MS: u64 = 100;
const MAX_TIMEOUT_MS: u64 = 60_000;
/// The longest a step of the child's integration is (seconds): the same as the step of the rest of the simulation.
const CHILD_STEP_S: f64 = crate::dynamics::MAX_SUBSTEP_S;

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_MS
}

/// The `coupled` block of the model file.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoupledSpec {
    /// The URDF of the robot (the names of its movable joints are the names of the `rigid` joints of the model).
    pub urdf: PathBuf,
    /// The `hydra-umc-physics-replica` program.
    pub binary: PathBuf,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

impl CoupledSpec {
    pub fn validate(&self) -> Result<(), String> {
        if self.urdf.as_os_str().is_empty() || self.binary.as_os_str().is_empty() {
            return Err("the coupled block needs a urdf and a binary".into());
        }
        if !(MIN_TIMEOUT_MS..=MAX_TIMEOUT_MS).contains(&self.timeout_ms) {
            return Err(format!(
                "the coupled timeout_ms must be between {MIN_TIMEOUT_MS} and {MAX_TIMEOUT_MS}"
            ));
        }
        Ok(())
    }
}

/// One rigid joint going into an interval.
#[derive(Debug, Clone, PartialEq)]
pub struct JointIn {
    pub name: String,
    pub position: f64,
    pub velocity: f64,
    pub target: f64,
    pub kp: f64,
    pub kd: f64,
    pub max_torque: f64,
}

/// One rigid joint at the end of an interval.
#[derive(Debug, Clone, PartialEq)]
pub struct JointOut {
    pub name: String,
    pub position: f64,
    pub velocity: f64,
}

/// A number list as the `name=value` pairs the child reads (`{:?}` writes a float so that it reads back as the same number).
fn pairs(joints: &[JointIn], value: impl Fn(&JointIn) -> f64) -> String {
    joints
        .iter()
        .map(|j| format!("{}={:?}", j.name, value(j)))
        .collect::<Vec<_>>()
        .join(",")
}

/// The command line for an interval of `seconds`, cut into `steps` equal integration steps.
fn command_for(spec: &CoupledSpec, joints: &[JointIn], seconds: f64, steps: u64) -> Command {
    let mut command = Command::new(&spec.binary);
    command
        .arg("simulate")
        .arg("--urdf")
        .arg(&spec.urdf)
        .arg("--joints")
        .arg(pairs(joints, |j| j.position))
        .arg("--velocities")
        .arg(pairs(joints, |j| j.velocity))
        .arg("--targets")
        .arg(pairs(joints, |j| j.target))
        .arg("--kp")
        .arg(pairs(joints, |j| j.kp))
        .arg("--kd")
        .arg(pairs(joints, |j| j.kd))
        .arg("--max-torque")
        .arg(pairs(joints, |j| j.max_torque))
        .arg("--duration")
        .arg(format!("{seconds:?}"))
        .arg("--dt")
        .arg(format!("{:?}", seconds / steps as f64))
        // only the first and the last sample are wanted
        .arg("--every")
        .arg(steps.to_string())
        .arg("--json");
    command
}

/// Let `seconds` pass for these joints with one call to the child. The joints must be exactly the movable joints of the URDF.
pub fn advance(
    spec: &CoupledSpec,
    joints: &[JointIn],
    seconds: f64,
) -> Result<Vec<JointOut>, String> {
    let steps = (seconds / CHILD_STEP_S).ceil().max(1.0) as u64;
    let command = command_for(spec, joints, seconds, steps);
    let (code, stdout, stderr) = run_with_timeout(command, Duration::from_millis(spec.timeout_ms))
        .map_err(|e| match e {
            CollisionError::Timeout(ms) => {
                format!("the coupled dynamics did not answer in {ms} ms")
            }
            CollisionError::Unavailable(reason) => format!("the coupled dynamics: {reason}"),
            other => format!("the coupled dynamics: {other:?}"),
        })?;
    match code {
        Some(0) => {}
        Some(2) => {
            let reason: String = stderr
                .lines()
                .find(|l| l.starts_with("simulate:"))
                .unwrap_or("the child refused the state")
                .chars()
                .take(300)
                .collect();
            return Err(format!("the coupled dynamics refused the state: {reason}"));
        }
        other => return Err(format!("the coupled dynamics ended with status {other:?}")),
    }
    parse_answer(stdout.trim(), joints)
}

/// The last sample of the child's answer, checked against the joints that were sent.
fn parse_answer(text: &str, joints: &[JointIn]) -> Result<Vec<JointOut>, String> {
    let bad = |what: &str| format!("the answer of the coupled dynamics is unusable: {what}");
    let value: Value = serde_json::from_str(text).map_err(|e| bad(&format!("not JSON ({e})")))?;
    let names = value
        .get("joints")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("no joint list"))?;
    let names: Vec<&str> = names
        .iter()
        .map(|n| n.as_str().ok_or_else(|| bad("a joint name is not text")))
        .collect::<Result<_, _>>()?;
    let sent: BTreeSet<&str> = joints.iter().map(|j| j.name.as_str()).collect();
    let got: BTreeSet<&str> = names.iter().copied().collect();
    if got.len() != names.len() || got != sent {
        return Err(bad(
            "the rigid joints of the model must be exactly the movable joints of the URDF",
        ));
    }
    let last = value
        .get("samples")
        .and_then(Value::as_array)
        .and_then(|s| s.last())
        .ok_or_else(|| bad("no samples"))?;
    let numbers = |key: &str| -> Result<Vec<f64>, String> {
        let list = last
            .get(key)
            .and_then(Value::as_array)
            .ok_or_else(|| bad(&format!("no {key}")))?;
        if list.len() != names.len() {
            return Err(bad(&format!("{key} does not match the joints")));
        }
        list.iter()
            .map(|v| {
                v.as_f64()
                    .filter(|n| n.is_finite())
                    .ok_or_else(|| bad(&format!("a number of {key} is not finite")))
            })
            .collect()
    };
    let (positions, velocities) = (numbers("positions")?, numbers("velocities")?);
    Ok(names
        .iter()
        .enumerate()
        .map(|(i, name)| JointOut {
            name: (*name).to_string(),
            position: positions[i],
            velocity: velocities[i],
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joint(name: &str) -> JointIn {
        JointIn {
            name: name.into(),
            position: 0.1,
            velocity: 0.0,
            target: 0.5,
            kp: 20.0,
            kd: 2.0,
            max_torque: 5.0,
        }
    }

    fn spec() -> CoupledSpec {
        CoupledSpec {
            urdf: "r.urdf".into(),
            binary: "bin".into(),
            timeout_ms: 1000,
        }
    }

    #[test]
    fn the_block_is_checked() {
        assert!(spec().validate().is_ok());
        for bad in [
            CoupledSpec {
                urdf: "".into(),
                ..spec()
            },
            CoupledSpec {
                binary: "".into(),
                ..spec()
            },
            CoupledSpec {
                timeout_ms: 99,
                ..spec()
            },
            CoupledSpec {
                timeout_ms: 60_001,
                ..spec()
            },
        ] {
            assert!(bad.validate().is_err());
        }
    }

    #[test]
    fn the_command_carries_the_state_the_targets_and_the_motor_once() {
        let command = command_for(&spec(), &[joint("a"), joint("b")], 0.02, 4);
        let args: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let after = |flag: &str| args[args.iter().position(|a| a == flag).unwrap() + 1].clone();
        assert_eq!(args[0], "simulate");
        assert_eq!(after("--joints"), "a=0.1,b=0.1");
        assert_eq!(after("--targets"), "a=0.5,b=0.5");
        assert_eq!(after("--kp"), "a=20.0,b=20.0");
        assert_eq!(after("--max-torque"), "a=5.0,b=5.0");
        assert_eq!(after("--duration"), "0.02");
        assert_eq!(after("--dt"), "0.005");
        assert_eq!(after("--every"), "4");
        assert!(args.contains(&"--json".to_string()));
    }

    #[test]
    fn the_last_sample_is_taken_and_checked() {
        let answer = r#"{"joints":["a","b"],"dt_s":0.005,"steps":4,"samples":[{"t_s":0,"positions":[0.1,0.1],"velocities":[0,0]},{"t_s":0.02,"positions":[0.2,0.3],"velocities":[1.0,-1.0]}]}"#;
        let out = parse_answer(answer, &[joint("a"), joint("b")]).unwrap();
        assert_eq!(
            out[1],
            JointOut {
                name: "b".into(),
                position: 0.3,
                velocity: -1.0
            }
        );
    }

    #[test]
    fn an_answer_that_does_not_match_the_joints_is_refused() {
        let two = [joint("a"), joint("b")];
        for answer in [
            "not json",
            r#"{"samples":[]}"#,
            r#"{"joints":["a","b"],"samples":[]}"#,
            r#"{"joints":["a"],"samples":[{"positions":[0],"velocities":[0]}]}"#,
            r#"{"joints":["a","b","c"],"samples":[{"positions":[0,0,0],"velocities":[0,0,0]}]}"#,
            r#"{"joints":["a","a"],"samples":[{"positions":[0,0],"velocities":[0,0]}]}"#,
            r#"{"joints":["a","b"],"samples":[{"positions":[0],"velocities":[0,0]}]}"#,
            r#"{"joints":["a","b"],"samples":[{"positions":[0,null],"velocities":[0,0]}]}"#,
            r#"{"joints":["a",1],"samples":[{"positions":[0,0],"velocities":[0,0]}]}"#,
        ] {
            assert!(parse_answer(answer, &two).is_err(), "{answer}");
        }
    }
}

#[cfg(all(test, unix))]
mod child_tests;
