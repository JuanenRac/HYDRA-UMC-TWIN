// =============================================================================
// HYDRA-UMC-TWIN - src/uav.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0-or-later - see LICENSE
// =============================================================================
//! The input for the state of an unmanned vehicle: where it is and how it is turned.
//!
//! HYDRA-UMC-BRIDGE-UAV joins the position and the attitude that the vehicle reports into a `TwinState` (one pose, `vehicle` by default, in
//! the frame `base`, its position in metres east, north and up of the home point, its orientation a unit quaternion) and hands it over. This is
//! the receiving end: `POST /twin/uav` takes that document, and `GET /twin/uav` shows the last state of every vehicle and how old it is.
//!
//! What it checks before it keeps a state: the document satisfies the `TwinState` contract, it says `real` (a `simulated` one is refused unless
//! `uav.accept_simulated` allows it, because a consumer must never take a simulated vehicle for a real one), it names its `robot_id`, its
//! `sequence` is newer than the last one of that vehicle (so an old one that arrives late is dropped; a vehicle that has been silent for longer
//! than `uav.max_age_ms` may start again from any number, which is what a restarted bridge does), it carries the pose named `uav.pose_name` in
//! the frame `base` and the position is finite and within `uav.max_distance_m` of home on every axis. At most `uav.max_vehicles` vehicles are
//! followed. A state older than `uav.max_age_ms` is shown as stale and is not pushed through the WebSocket as if it were current.
//!
//! Information only: nothing here reads a state to decide anything about a motion, and it has not been tried with a real vehicle.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::twincfg::UavCfg;

#[derive(Debug, Clone, PartialEq)]
pub enum UavError {
    /// The document is not acceptable: what is wrong with it.
    Invalid(String),
    /// The state says `simulated` and the configuration does not accept that.
    Simulated,
    /// Its `sequence` is not newer than the last of this vehicle.
    Stale { last_sequence: u64 },
    /// The twin follows `uav.max_vehicles` vehicles already.
    TooManyVehicles,
}

impl std::fmt::Display for UavError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UavError::Invalid(reason) => write!(f, "{reason}"),
            UavError::Simulated => write!(
                f,
                "a simulated state is not accepted (uav.accept_simulated)"
            ),
            UavError::Stale { last_sequence } => write!(
                f,
                "sequence is not newer than the last state of this vehicle ({last_sequence})"
            ),
            UavError::TooManyVehicles => write!(
                f,
                "the twin follows the most vehicles it may (uav.max_vehicles)"
            ),
        }
    }
}

struct Stored {
    state: Value,
    sequence: u64,
    received_at: Instant,
}

/// The last state of a vehicle and how old it is.
#[derive(Debug, Clone, PartialEq)]
pub struct Vehicle {
    pub robot_id: String,
    pub state: Value,
    pub age_ms: u64,
    pub stale: bool,
}

pub struct UavRoute {
    pub cfg: UavCfg,
    vehicles: Mutex<BTreeMap<String, Stored>>,
}

fn number(value: &Value) -> Option<f64> {
    value.as_f64().filter(|n| n.is_finite())
}

impl UavRoute {
    pub fn new(cfg: UavCfg) -> Self {
        Self {
            cfg,
            vehicles: Mutex::new(BTreeMap::new()),
        }
    }

    fn max_age(&self) -> Duration {
        Duration::from_millis(self.cfg.max_age_ms)
    }

    /// The checks that need only the document. Returns the `robot_id` and the `sequence`.
    fn check(&self, state: &Value) -> Result<(String, u64), UavError> {
        let problems = crate::schema::state_problems(state);
        if !problems.is_empty() {
            return Err(UavError::Invalid(format!(
                "not a TwinState: {}",
                problems.join("; ")
            )));
        }
        if state["source"] == "simulated" && !self.cfg.accept_simulated {
            return Err(UavError::Simulated);
        }
        let robot_id = match state["robot_id"].as_str() {
            Some(id) if !id.is_empty() && id.len() <= 64 && !id.chars().any(|c| c.is_control()) => {
                id.to_string()
            }
            _ => {
                return Err(UavError::Invalid(
                    "robot_id is needed: 1 to 64 characters and no control character".into(),
                ))
            }
        };
        let Some(sequence) = state["sequence"].as_u64() else {
            return Err(UavError::Invalid(
                "sequence is needed: a whole number that grows with every state".into(),
            ));
        };
        if state["frame"] != "base" {
            return Err(UavError::Invalid("frame must be base".into()));
        }
        if state["joint_states"].as_array().map_or(0, Vec::len) > 64
            || state["poses"].as_array().map_or(0, Vec::len) > 16
        {
            return Err(UavError::Invalid(
                "too many joint states or poses for a vehicle".into(),
            ));
        }
        let pose = state["poses"]
            .as_array()
            .and_then(|poses| {
                poses
                    .iter()
                    .find(|p| p["name"] == self.cfg.pose_name.as_str())
            })
            .ok_or_else(|| {
                UavError::Invalid(format!(
                    "the pose named {} (uav.pose_name) is missing",
                    self.cfg.pose_name
                ))
            })?;
        if pose["pose"]["frame"] != "base" {
            return Err(UavError::Invalid(
                "the pose of the vehicle must be in the frame base".into(),
            ));
        }
        for axis in ["x", "y", "z"] {
            match number(&pose["pose"]["position"][axis]) {
                Some(n) if n.abs() <= self.cfg.max_distance_m => {}
                _ => {
                    return Err(UavError::Invalid(format!(
                        "position.{axis} must be a finite number within {} m of home (uav.max_distance_m)",
                        self.cfg.max_distance_m
                    )))
                }
            }
        }
        Ok((robot_id, sequence))
    }

    /// Keeps the state when it passes every check; returns the `robot_id` and the `sequence`.
    pub fn accept(&self, state: Value, now: Instant) -> Result<(String, u64), UavError> {
        let (robot_id, sequence) = self.check(&state)?;
        let mut vehicles = self.vehicles.lock().unwrap_or_else(|p| p.into_inner());
        match vehicles.get(&robot_id) {
            Some(last)
                if sequence <= last.sequence
                    && now.duration_since(last.received_at) <= self.max_age() =>
            {
                return Err(UavError::Stale {
                    last_sequence: last.sequence,
                });
            }
            Some(_) => {}
            None => {
                if vehicles.len() >= self.cfg.max_vehicles {
                    // a vehicle that has been silent for longer than the age limit gives way to a new one
                    let silent = vehicles
                        .iter()
                        .filter(|(_, v)| now.duration_since(v.received_at) > self.max_age())
                        .min_by_key(|(_, v)| v.received_at)
                        .map(|(id, _)| id.clone());
                    match silent {
                        Some(id) => {
                            vehicles.remove(&id);
                        }
                        None => return Err(UavError::TooManyVehicles),
                    }
                }
            }
        }
        vehicles.insert(
            robot_id.clone(),
            Stored {
                state,
                sequence,
                received_at: now,
            },
        );
        Ok((robot_id, sequence))
    }

    /// The last state of every vehicle, by `robot_id`.
    pub fn snapshot(&self, now: Instant) -> Vec<Vehicle> {
        let max_age = self.max_age();
        self.vehicles
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .map(|(id, v)| {
                let age = now.duration_since(v.received_at);
                Vehicle {
                    robot_id: id.clone(),
                    state: v.state.clone(),
                    age_ms: age.as_millis() as u64,
                    stale: age > max_age,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// What `twin_telemetry.py` of HYDRA-UMC-BRIDGE-UAV builds, with a unit quaternion.
    fn state(robot: &str, sequence: u64, east: f64, source: &str) -> Value {
        json!({
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
    }

    fn route(max_vehicles: usize, max_age_ms: u64) -> UavRoute {
        UavRoute::new(UavCfg {
            max_vehicles,
            max_age_ms,
            ..UavCfg::default()
        })
    }

    #[test]
    fn a_state_of_the_bridge_is_kept_and_shown_with_its_age() {
        let r = route(4, 5_000);
        let t0 = Instant::now();
        assert_eq!(
            r.accept(state("uav-1", 1, 10.0, "real"), t0).unwrap(),
            ("uav-1".to_string(), 1)
        );
        let seen = r.snapshot(t0 + Duration::from_millis(100));
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].robot_id, "uav-1");
        assert_eq!(seen[0].age_ms, 100);
        assert!(!seen[0].stale);
        assert_eq!(seen[0].state["poses"][0]["pose"]["position"]["x"], 10.0);
        let later = r.snapshot(t0 + Duration::from_secs(6));
        assert!(later[0].stale);
    }

    #[test]
    fn a_simulated_state_is_refused_unless_the_configuration_accepts_it() {
        let t0 = Instant::now();
        let r = route(4, 5_000);
        assert_eq!(
            r.accept(state("uav-1", 1, 1.0, "simulated"), t0),
            Err(UavError::Simulated)
        );
        let permissive = UavRoute::new(UavCfg {
            accept_simulated: true,
            ..UavCfg::default()
        });
        assert!(permissive
            .accept(state("uav-1", 1, 1.0, "simulated"), t0)
            .is_ok());
        // what is kept says what it is
        assert_eq!(permissive.snapshot(t0)[0].state["source"], "simulated");
    }

    #[test]
    fn an_old_sequence_is_dropped_but_a_vehicle_that_was_silent_may_start_again() {
        let r = route(4, 1_000);
        let t0 = Instant::now();
        r.accept(state("uav-1", 5, 1.0, "real"), t0).unwrap();
        for sequence in [5, 4, 0] {
            assert_eq!(
                r.accept(
                    state("uav-1", sequence, 2.0, "real"),
                    t0 + Duration::from_millis(100)
                ),
                Err(UavError::Stale { last_sequence: 5 })
            );
        }
        r.accept(
            state("uav-1", 6, 3.0, "real"),
            t0 + Duration::from_millis(200),
        )
        .unwrap();
        // a bridge that restarted counts from zero again after the silence
        r.accept(state("uav-1", 0, 4.0, "real"), t0 + Duration::from_secs(5))
            .unwrap();
        assert_eq!(
            r.snapshot(t0 + Duration::from_secs(5))[0].state["poses"][0]["pose"]["position"]["x"],
            4.0
        );
    }

    #[test]
    fn the_number_of_vehicles_is_bounded_and_a_silent_one_gives_way() {
        let r = route(2, 1_000);
        let t0 = Instant::now();
        r.accept(state("a", 1, 0.0, "real"), t0).unwrap();
        r.accept(state("b", 1, 0.0, "real"), t0 + Duration::from_millis(10))
            .unwrap();
        assert_eq!(
            r.accept(state("c", 1, 0.0, "real"), t0 + Duration::from_millis(20)),
            Err(UavError::TooManyVehicles)
        );
        // the vehicles that are known still update
        r.accept(state("a", 2, 0.0, "real"), t0 + Duration::from_millis(30))
            .unwrap();
        // later "b" has been silent for longer than the limit: "c" takes its place, "a" is kept
        r.accept(
            state("a", 3, 0.0, "real"),
            t0 + Duration::from_millis(1_500),
        )
        .unwrap();
        r.accept(
            state("c", 1, 0.0, "real"),
            t0 + Duration::from_millis(1_600),
        )
        .unwrap();
        let ids: Vec<String> = r
            .snapshot(t0 + Duration::from_millis(1_600))
            .into_iter()
            .map(|v| v.robot_id)
            .collect();
        assert_eq!(ids, vec!["a".to_string(), "c".to_string()]);
    }

    #[test]
    fn what_is_not_a_state_of_a_vehicle_in_the_frame_of_home_is_refused() {
        let r = route(4, 5_000);
        let t0 = Instant::now();
        let mut cases: Vec<(&str, Value)> = Vec::new();
        let mut v = state("u", 1, 1.0, "real");
        v["schema_version"] = json!("2.0");
        cases.push(("schema version", v));
        let mut v = state("u", 1, 1.0, "real");
        v.as_object_mut().unwrap().remove("robot_id");
        cases.push(("no robot id", v));
        let mut v = state("u", 1, 1.0, "real");
        v.as_object_mut().unwrap().remove("sequence");
        cases.push(("no sequence", v));
        let mut v = state("u", 1, 1.0, "real");
        v["poses"][0]["name"] = json!("camera");
        cases.push(("no vehicle pose", v));
        let mut v = state("u", 1, 1.0, "real");
        v["frame"] = json!("tool");
        cases.push(("frame tool", v));
        let mut v = state("u", 1, 1.0, "real");
        v["poses"][0]["pose"]["frame"] = json!("tool");
        cases.push(("pose frame tool", v));
        cases.push(("too far", state("u", 1, 1.0e7, "real")));
        let mut v = state("u", 1, 1.0, "real");
        v["poses"][0]["pose"]["orientation"] = json!({"w": 2.0, "x": 0.0, "y": 0.0, "z": 0.0});
        cases.push(("not a unit quaternion", v));
        cases.push(("long id", state(&"x".repeat(65), 1, 1.0, "real")));
        cases.push(("not an object", json!([1, 2, 3])));
        for (why, document) in cases {
            assert!(
                matches!(r.accept(document, t0), Err(UavError::Invalid(_))),
                "{why}"
            );
        }
        assert!(r.snapshot(t0).is_empty());
    }
}
