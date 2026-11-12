// =============================================================================
// HYDRA-UMC-TWIN - src/poses.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0-or-later - see LICENSE
// =============================================================================
//! The minimal kinematic twin: `GET /twin/poses`.
//!
//! The twin keeps the last report of every joint (`joint_state.rs`). This route turns those positions into the pose of every joint frame of
//! the robot, with the forward kinematics of HYDRA-UMC-PHYSICS-REPLICA (`fk`, started as a process like the collision prediction, so the
//! kinematics are never written twice), and answers a document of the `TwinState` contract of the SDK (checked against the vendored schema
//! before it is sent). There is no graphics engine and no GPU: a studio that draws the robot polls this route.
//!
//! Nothing is guessed. A joint the robot needs and the twin has not heard of, or has not heard of lately (`poses.max_age_ms`), is a `409`
//! that names it, never a position of zero; a child that cannot be run is a `502`. The `source` of the state is `simulated` when the server
//! runs the simulated robot and `real` otherwise, and each joint carries whether its position was `commanded` or `measured`.
//!
//! The route is off unless the server is started with `--poses` and a `--twin-config`. The binary and the URDF come from the configuration
//! and never from the request.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};

use crate::collision::{run_with_timeout, CollisionError};
use crate::joint_state::{JointStateReport, StateSource};
use crate::twincfg::PosesCfg;

/// Poses computed at the same time: each one starts a process.
pub const MAX_CONCURRENT: usize = 2;

#[derive(Debug, PartialEq)]
pub enum PoseError {
    /// Joints the twin needs and has no report for, or whose report is too old (`409`).
    Incomplete {
        missing: Vec<String>,
        stale: Vec<String>,
    },
    /// Two computations are running already (`503`).
    Busy,
    /// The child refused the positions (`422`).
    Refused(String),
    /// The child could not be run or answered something unusable (`502`).
    Unavailable(String),
    /// The child did not end in time (`504`).
    Timeout(u64),
}

type Cache = Option<(Vec<(String, u64)>, Vec<Value>)>;

pub struct PoseRoute {
    pub binary: PathBuf,
    pub urdf: PathBuf,
    pub robot_id: String,
    pub cfg: PosesCfg,
    running: AtomicUsize,
    /// The positions of the last computation and its poses: a client that polls while the robot stands still starts no process.
    cache: Mutex<Cache>,
}

impl PoseRoute {
    pub fn new(binary: PathBuf, urdf: PathBuf, robot_id: String, cfg: PosesCfg) -> Self {
        Self {
            binary,
            urdf,
            robot_id,
            cfg,
            running: AtomicUsize::new(0),
            cache: Mutex::new(None),
        }
    }

    /// The TwinState document for these reports (`simulated`: whether the reports come from the simulated robot).
    pub fn twin_state(
        &self,
        reports: &[(JointStateReport, Duration)],
        simulated: bool,
        now_ms: u64,
    ) -> Result<Value, PoseError> {
        let max_age = Duration::from_millis(self.cfg.max_age_ms);
        let wanted: Vec<String> = if self.cfg.joints.is_empty() {
            reports.iter().map(|(r, _)| r.joint.clone()).collect()
        } else {
            self.cfg.joints.clone()
        };
        if wanted.is_empty() {
            // nothing reported and nothing required: there is no state to give
            return Err(PoseError::Incomplete {
                missing: vec![],
                stale: vec![],
            });
        }
        let (mut missing, mut stale) = (Vec::new(), Vec::new());
        let mut used: Vec<&(JointStateReport, Duration)> = Vec::new();
        for name in &wanted {
            match reports.iter().find(|(r, _)| &r.joint == name) {
                None => missing.push(name.clone()),
                Some(entry) if entry.1 > max_age => stale.push(name.clone()),
                Some(entry) => used.push(entry),
            }
        }
        if !missing.is_empty() || !stale.is_empty() {
            return Err(PoseError::Incomplete { missing, stale });
        }
        let positions: BTreeMap<String, f64> = used
            .iter()
            .map(|(r, _)| (r.joint.clone(), r.position))
            .collect();
        let poses = self.poses(&positions)?;
        Ok(json!({
            "schema_version": "1.0",
            "source": if simulated { "simulated" } else { "real" },
            "robot_id": self.robot_id,
            "frame": "base",
            "timestamp_utc": rfc3339_utc(now_ms),
            "sequence": used.iter().map(|(r, _)| r.seq).max().unwrap_or(0),
            "joint_states": used.iter().map(|(r, age)| json!({
                "name": r.joint,
                "position": r.position,
                "reading": match r.source { StateSource::Commanded => "commanded", StateSource::Measured => "measured" },
                "age_ms": age.as_millis() as u64,
            })).collect::<Vec<_>>(),
            "poses": poses,
        }))
    }

    fn poses(&self, positions: &BTreeMap<String, f64>) -> Result<Vec<Value>, PoseError> {
        let key: Vec<(String, u64)> = positions
            .iter()
            .map(|(k, v)| (k.clone(), v.to_bits()))
            .collect();
        if let Some((cached_key, cached)) = self
            .cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
        {
            if *cached_key == key {
                return Ok(cached.clone());
            }
        }
        if self.running.fetch_add(1, Ordering::SeqCst) >= MAX_CONCURRENT {
            self.running.fetch_sub(1, Ordering::SeqCst);
            return Err(PoseError::Busy);
        }
        let result = self.run_child(positions);
        self.running.fetch_sub(1, Ordering::SeqCst);
        let poses = result?;
        *self.cache.lock().unwrap_or_else(|p| p.into_inner()) = Some((key, poses.clone()));
        Ok(poses)
    }

    fn run_child(&self, positions: &BTreeMap<String, f64>) -> Result<Vec<Value>, PoseError> {
        let spec = positions
            .iter()
            .map(|(name, position)| format!("{name}={position:?}"))
            .collect::<Vec<_>>()
            .join(",");
        let mut command = Command::new(&self.binary);
        command
            .arg("fk")
            .arg("--urdf")
            .arg(&self.urdf)
            .arg("--joints")
            .arg(&spec)
            .arg("--json");
        let (code, stdout, stderr) =
            run_with_timeout(command, Duration::from_millis(self.cfg.timeout_ms)).map_err(|e| {
                match e {
                    CollisionError::Timeout(ms) => PoseError::Timeout(ms),
                    CollisionError::Unavailable(reason) => PoseError::Unavailable(reason),
                    other => PoseError::Unavailable(format!("{other:?}")),
                }
            })?;
        match code {
            Some(0) => {}
            Some(2) => {
                return Err(PoseError::Refused(
                    stderr
                        .lines()
                        .find(|l| l.starts_with("fk:"))
                        .unwrap_or("the child refused the positions")
                        .chars()
                        .take(300)
                        .collect(),
                ))
            }
            other => {
                return Err(PoseError::Unavailable(format!(
                    "the child ended with status {other:?}"
                )))
            }
        }
        let list: Value = serde_json::from_str(stdout.trim()).map_err(|e| {
            PoseError::Unavailable(format!("the answer of the child is not JSON: {e}"))
        })?;
        let list = list
            .as_array()
            .filter(|l| !l.is_empty())
            .ok_or_else(|| PoseError::Unavailable("the child answered no pose".into()))?;
        let mut poses = Vec::with_capacity(list.len());
        for entry in list {
            let (name, p, q) = (
                entry.get("joint").and_then(Value::as_str),
                entry.get("position"),
                entry.get("quaternion"),
            );
            let (Some(name), Some(p), Some(q)) = (name, p, q) else {
                return Err(PoseError::Unavailable(
                    "a pose of the child has no joint, position or quaternion".into(),
                ));
            };
            poses.push(json!({
                "name": name,
                "pose": {
                    "schema_version": "1.0",
                    "frame": "base",
                    "position": {"x": p["x"], "y": p["y"], "z": p["z"]},
                    "orientation": {"w": q["w"], "x": q["x"], "y": q["y"], "z": q["z"]},
                }
            }));
        }
        Ok(poses)
    }
}

/// `2026-01-31T12:00:00.123Z` for a time in milliseconds since the Unix epoch (the civil date algorithm of H. Hinnant; no calendar crate).
pub fn rfc3339_utc(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60,
        ms % 1000
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(joint: &str, position: f64, seq: u64, age_ms: u64) -> (JointStateReport, Duration) {
        (
            JointStateReport {
                joint: joint.into(),
                position,
                seq,
                stamp_ms: 0,
                source: StateSource::Measured,
            },
            Duration::from_millis(age_ms),
        )
    }

    fn route(binary: &str, joints: &[&str], max_age_ms: u64) -> PoseRoute {
        PoseRoute::new(
            PathBuf::from(binary),
            PathBuf::from("robot.urdf"),
            "arm".into(),
            PosesCfg {
                joints: joints.iter().map(|j| j.to_string()).collect(),
                max_age_ms,
                timeout_ms: 20_000,
            },
        )
    }

    #[test]
    fn the_dates_are_the_calendar_dates() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339_utc(951_782_400_000), "2000-02-29T00:00:00.000Z");
        assert_eq!(rfc3339_utc(1_709_210_096_789), "2024-02-29T12:34:56.789Z");
        assert_eq!(rfc3339_utc(4_102_444_799_999), "2099-12-31T23:59:59.999Z");
    }

    #[test]
    fn a_joint_nobody_reported_or_that_is_old_is_named_and_nothing_is_computed() {
        let r = route("definitely-not-a-binary-9f2c", &["a", "b", "c"], 1000);
        let reports = vec![report("a", 0.1, 1, 10), report("b", 0.2, 2, 5000)];
        assert_eq!(
            r.twin_state(&reports, false, 0),
            Err(PoseError::Incomplete {
                missing: vec!["c".into()],
                stale: vec!["b".into()]
            })
        );
        // with no reports at all there is nothing to place
        assert!(matches!(
            route("x", &[], 1000).twin_state(&[], false, 0),
            Err(PoseError::Incomplete { .. })
        ));
    }

    #[test]
    fn a_child_that_does_not_exist_is_unavailable_never_an_empty_pose() {
        let r = route("definitely-not-a-binary-9f2c", &[], 1000);
        let reports = vec![report("a", 0.1, 1, 10)];
        assert!(matches!(
            r.twin_state(&reports, false, 0),
            Err(PoseError::Unavailable(_))
        ));
    }

    #[test]
    fn at_most_two_computations_run_at_once() {
        let r = route("x", &[], 1000);
        r.running.store(MAX_CONCURRENT, Ordering::SeqCst);
        let reports = vec![report("a", 0.1, 1, 10)];
        assert_eq!(r.twin_state(&reports, false, 0), Err(PoseError::Busy));
        assert_eq!(r.running.load(Ordering::SeqCst), MAX_CONCURRENT);
    }
}

#[cfg(all(test, unix))]
mod child_tests;
