// =============================================================================
// HYDRA-UMC-TWIN - src/joint_state.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0 - see LICENSE
// =============================================================================
//! The state of the real robot, as the hardware-in-the-loop bridge reports it to the twin.
//!
//! The bridge sends one `JointStateReport` per joint: `{joint, position, seq, stamp_ms, source}`. `source` says whether the position is
//! what was only ordered (`commanded`) or what the controller measured (`measured`); the twin keeps the newest report of every joint
//! and the moment it arrived. A report whose `seq` is not newer than the last one of its joint is refused, so a late or replayed
//! message cannot move the twin back.
//!
//! Nothing here decides anything by itself: `POST /interlock/state` stores the report and, only when the server is started with
//! `--require-fresh-state-ms`, `POST /interlock/certify` refuses to sign for a joint whose state is missing or older than that.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// The longest joint name accepted (the same limit as the bridge).
pub const MAX_JOINT_NAME_LEN: usize = 64;
/// How many different joints the store keeps: a robot has a few dozen, so a flood of invented names cannot grow it without limit.
pub const MAX_JOINTS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StateSource {
    /// The position the bridge ordered and the controller accepted; the robot may not have got there yet.
    Commanded,
    /// The position the controller read back from the joint.
    Measured,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JointStateReport {
    pub joint: String,
    pub position: f64,
    /// Grows with every report of the joint; a report that is not newer than the last one is refused.
    pub seq: u64,
    /// When the bridge made the report, in milliseconds since the Unix epoch (informative: the age is counted from the arrival).
    pub stamp_ms: u64,
    pub source: StateSource,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StateRejection {
    Invalid(String),
    /// Not newer than the last report of the joint.
    Stale {
        last_seq: u64,
    },
    /// The store already follows `MAX_JOINTS` joints.
    TooManyJoints,
}

impl std::fmt::Display for StateRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StateRejection::Invalid(reason) => write!(f, "{reason}"),
            StateRejection::Stale { last_seq } => write!(
                f,
                "seq is not newer than the last report of this joint ({last_seq})"
            ),
            StateRejection::TooManyJoints => {
                write!(f, "the twin follows {MAX_JOINTS} joints already")
            }
        }
    }
}

impl JointStateReport {
    pub fn validate(&self) -> Result<(), StateRejection> {
        if self.joint.trim().is_empty() {
            return Err(StateRejection::Invalid(
                "joint name must not be empty".into(),
            ));
        }
        if self.joint.len() > MAX_JOINT_NAME_LEN {
            return Err(StateRejection::Invalid(format!(
                "joint name is longer than {MAX_JOINT_NAME_LEN} bytes"
            )));
        }
        if !self.position.is_finite() {
            return Err(StateRejection::Invalid(
                "position must be a finite number".into(),
            ));
        }
        Ok(())
    }
}

struct Stored {
    report: JointStateReport,
    received_at: Instant,
}

/// The newest report of every joint.
#[derive(Default)]
pub struct JointStateStore {
    joints: HashMap<String, Stored>,
}

impl JointStateStore {
    pub fn accept(&mut self, report: JointStateReport, now: Instant) -> Result<(), StateRejection> {
        report.validate()?;
        match self.joints.get(&report.joint) {
            Some(last) if report.seq <= last.report.seq => {
                return Err(StateRejection::Stale {
                    last_seq: last.report.seq,
                })
            }
            None if self.joints.len() >= MAX_JOINTS => return Err(StateRejection::TooManyJoints),
            _ => {}
        }
        self.joints.insert(
            report.joint.clone(),
            Stored {
                report,
                received_at: now,
            },
        );
        Ok(())
    }

    /// The age of the last report of `joint`, or None when the twin has never heard of it.
    pub fn age(&self, joint: &str, now: Instant) -> Option<Duration> {
        self.joints
            .get(joint)
            .map(|s| now.saturating_duration_since(s.received_at))
    }

    /// Fail closed: the state of `joint` is fresh only if it exists and arrived at most `max_age` ago.
    pub fn check_fresh(&self, joint: &str, max_age: Duration, now: Instant) -> Result<(), String> {
        match self.age(joint, now) {
            None => Err(format!(
                "the twin has no state report for joint {joint}: it will not certify a command for a joint it cannot see"
            )),
            Some(age) if age > max_age => Err(format!(
                "the last state report of joint {joint} is {} ms old (limit {} ms)",
                age.as_millis(),
                max_age.as_millis()
            )),
            Some(_) => Ok(()),
        }
    }

    /// The reports with the age of each in milliseconds, ordered by joint name.
    pub fn snapshot(&self, now: Instant) -> Vec<serde_json::Value> {
        let mut names: Vec<&String> = self.joints.keys().collect();
        names.sort();
        names
            .into_iter()
            .map(|name| {
                let stored = &self.joints[name];
                let mut value = serde_json::to_value(&stored.report).unwrap();
                value["age_ms"] = serde_json::json!(now
                    .saturating_duration_since(stored.received_at)
                    .as_millis() as u64);
                value
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(joint: &str, position: f64, seq: u64, source: StateSource) -> JointStateReport {
        JointStateReport {
            joint: joint.to_string(),
            position,
            seq,
            stamp_ms: 1_700_000_000_000,
            source,
        }
    }

    #[test]
    fn the_shape_on_the_wire_is_the_one_the_bridge_sends() {
        // The same text is checked in HYDRA-UMC-HIL-BRIDGE: the two repositories agree on it.
        let wire = r#"{"joint":"shoulder","position":0.5,"seq":7,"stamp_ms":1700000000000,"source":"measured"}"#;
        let parsed: JointStateReport = serde_json::from_str(wire).unwrap();
        assert_eq!(parsed, report("shoulder", 0.5, 7, StateSource::Measured));
        assert_eq!(serde_json::to_string(&parsed).unwrap(), wire);
        let commanded = wire.replace("measured", "commanded");
        assert_eq!(
            serde_json::from_str::<JointStateReport>(&commanded)
                .unwrap()
                .source,
            StateSource::Commanded
        );
        assert!(
            serde_json::from_str::<JointStateReport>(&wire.replace("measured", "guessed")).is_err()
        );
    }

    #[test]
    fn a_report_that_is_not_newer_is_refused_and_does_not_replace_the_last() {
        let now = Instant::now();
        let mut store = JointStateStore::default();
        store
            .accept(report("j1", 1.0, 5, StateSource::Measured), now)
            .unwrap();
        for seq in [5, 4, 0] {
            assert_eq!(
                store.accept(report("j1", 9.0, seq, StateSource::Measured), now),
                Err(StateRejection::Stale { last_seq: 5 })
            );
        }
        store
            .accept(report("j1", 2.0, 6, StateSource::Commanded), now)
            .unwrap();
        let snap = store.snapshot(now);
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0]["position"], 2.0);
        assert_eq!(snap[0]["source"], "commanded");
    }

    #[test]
    fn each_joint_has_its_own_sequence() {
        let now = Instant::now();
        let mut store = JointStateStore::default();
        store
            .accept(report("j1", 1.0, 10, StateSource::Measured), now)
            .unwrap();
        store
            .accept(report("j2", 1.0, 1, StateSource::Measured), now)
            .unwrap();
    }

    #[test]
    fn invalid_reports_are_refused() {
        let now = Instant::now();
        let mut store = JointStateStore::default();
        for bad in [
            report("", 1.0, 1, StateSource::Measured),
            report("  ", 1.0, 1, StateSource::Measured),
            report(
                &"j".repeat(MAX_JOINT_NAME_LEN + 1),
                1.0,
                1,
                StateSource::Measured,
            ),
            report("j", f64::NAN, 1, StateSource::Measured),
            report("j", f64::INFINITY, 1, StateSource::Measured),
        ] {
            assert!(matches!(
                store.accept(bad, now),
                Err(StateRejection::Invalid(_))
            ));
        }
        assert!(store.snapshot(now).is_empty());
    }

    #[test]
    fn the_store_does_not_grow_without_limit() {
        let now = Instant::now();
        let mut store = JointStateStore::default();
        for i in 0..MAX_JOINTS {
            store
                .accept(report(&format!("j{i}"), 0.0, 1, StateSource::Measured), now)
                .unwrap();
        }
        assert_eq!(
            store.accept(report("one-more", 0.0, 1, StateSource::Measured), now),
            Err(StateRejection::TooManyJoints)
        );
        // a joint that is already followed can still be updated
        store
            .accept(report("j0", 1.0, 2, StateSource::Measured), now)
            .unwrap();
    }

    #[test]
    fn freshness_fails_closed_for_an_unknown_or_old_joint() {
        let start = Instant::now();
        let mut store = JointStateStore::default();
        store
            .accept(report("j1", 1.0, 1, StateSource::Measured), start)
            .unwrap();
        let limit = Duration::from_millis(500);
        assert!(store.check_fresh("j1", limit, start).is_ok());
        assert!(store
            .check_fresh("j1", limit, start + Duration::from_millis(500))
            .is_ok());
        let old = store
            .check_fresh("j1", limit, start + Duration::from_millis(501))
            .unwrap_err();
        assert!(old.contains("501 ms old"), "{old}");
        let unknown = store.check_fresh("j2", limit, start).unwrap_err();
        assert!(unknown.contains("no state report"), "{unknown}");
    }
}
