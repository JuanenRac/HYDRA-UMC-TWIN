// =============================================================================
// HYDRA-UMC-TWIN - src/schema.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0 - see LICENSE
// =============================================================================
//! JSON Schema validation of the two documents this Twin trusts: the project manifest of a
//! child and the state of a twin.
//!
//! Both schemas are the published contracts of the SDK, vendored under `schemas/` and
//! compiled with the `jsonschema` crate (Draft 2020-12, with `format` checked), so the Twin
//! enforces the real contract and not a hand-written subset of it. A schema cannot say that
//! a quaternion has length 1, so `state_problems` checks that besides the schema.

use std::sync::OnceLock;

use jsonschema::Validator;
use serde_json::Value;

const PROJECT_MANIFEST_SCHEMA: &str = include_str!("../schemas/project-manifest.schema.json");
const TWIN_STATE_SCHEMA: &str = include_str!("../schemas/twin-state.schema.json");

/// How far from 1 the length of a unit quaternion may be (the same tolerance as the SDK clients).
const QUATERNION_TOLERANCE: f64 = 1e-6;

fn compile(name: &str, text: &str) -> Validator {
    let schema: Value = serde_json::from_str(text)
        .unwrap_or_else(|error| panic!("the vendored {name} schema is not JSON: {error}"));
    jsonschema::draft202012::options()
        .should_validate_formats(true)
        .build(&schema)
        .unwrap_or_else(|error| panic!("the vendored {name} schema does not compile: {error}"))
}

fn manifest_validator() -> &'static Validator {
    static VALIDATOR: OnceLock<Validator> = OnceLock::new();
    VALIDATOR.get_or_init(|| compile("project manifest", PROJECT_MANIFEST_SCHEMA))
}

fn state_validator() -> &'static Validator {
    static VALIDATOR: OnceLock<Validator> = OnceLock::new();
    VALIDATOR.get_or_init(|| compile("twin state", TWIN_STATE_SCHEMA))
}

fn violations(validator: &Validator, value: &Value) -> Vec<String> {
    let mut found: Vec<String> = validator
        .iter_errors(value)
        .map(|error| {
            let at = error.instance_path().to_string();
            if at.is_empty() {
                error.to_string()
            } else {
                format!("{at}: {error}")
            }
        })
        .collect();
    found.sort();
    found.dedup();
    found
}

/// What is wrong with a project manifest, as the schema of the SDK sees it; empty when it is valid.
pub fn manifest_problems(manifest: &Value) -> Vec<String> {
    violations(manifest_validator(), manifest)
}

fn quaternion_length(orientation: &Value) -> Option<f64> {
    let part = |key: &str| orientation.get(key).and_then(Value::as_f64);
    let (w, x, y, z) = (part("w")?, part("x")?, part("y")?, part("z")?);
    Some((w * w + x * x + y * y + z * z).sqrt())
}

/// What is wrong with a twin state: what the schema finds, the unit length of every pose
/// quaternion and a joint named twice. Empty when the state is valid.
pub fn state_problems(state: &Value) -> Vec<String> {
    let mut found = violations(state_validator(), state);
    if let Some(poses) = state.get("poses").and_then(Value::as_array) {
        for (index, entry) in poses.iter().enumerate() {
            let length = entry
                .get("pose")
                .and_then(|pose| pose.get("orientation"))
                .and_then(quaternion_length);
            if let Some(length) = length {
                if !length.is_finite() || (length - 1.0).abs() > QUATERNION_TOLERANCE {
                    found.push(format!(
                        "/poses/{index}/pose/orientation: the quaternion has length {length}, not 1"
                    ));
                }
            }
        }
    }
    if let Some(joints) = state.get("joint_states").and_then(Value::as_array) {
        let mut seen = std::collections::HashSet::new();
        for (index, joint) in joints.iter().enumerate() {
            if let Some(name) = joint.get("name").and_then(Value::as_str) {
                if !seen.insert(name.to_string()) {
                    found.push(format!(
                        "/joint_states/{index}/name: the joint {name:?} appears twice"
                    ));
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn manifest() -> Value {
        json!({
            "schema_version": "1.0", "ecosystem": "HYDRA-UMC", "name": "HYDRA-UMC-HIL-BRIDGE",
            "version": "0.1.2", "role": "service", "stack": "rust", "technologies": ["Rust"],
            "deployment_target": "cm5", "maturity": "functional", "family": "Digital Twin & Simulation",
            "parent": null, "build": "cargo build --release.", "notes": "n",
            "native_version": {"file": "Cargo.toml", "pattern": "^version"}
        })
    }

    fn state() -> Value {
        json!({
            "schema_version": "1.0", "source": "simulated", "frame": "base",
            "timestamp_utc": "2024-01-01T00:00:00Z", "sequence": 3,
            "joint_states": [{"name": "shoulder", "position": 0.5}, {"name": "elbow", "position": -1.0, "velocity": 0.1}],
            "poses": [{"name": "tcp", "pose": {
                "schema_version": "1.0", "frame": "base",
                "position": {"x": 0.1, "y": 0.2, "z": 0.3},
                "orientation": {"w": 1.0, "x": 0.0, "y": 0.0, "z": 0.0}
            }}]
        })
    }

    #[test]
    fn a_valid_manifest_has_no_problems() {
        assert_eq!(manifest_problems(&manifest()), Vec::<String>::new());
    }

    #[test]
    fn a_manifest_that_breaks_the_schema_is_described() {
        let mut broken = manifest();
        broken["version"] = json!("one");
        broken["maturity"] = json!("great");
        broken["unexpected"] = json!(1);
        broken.as_object_mut().unwrap().remove("notes");
        let problems = manifest_problems(&broken).join("\n");
        assert!(problems.contains("/version"), "{problems}");
        assert!(problems.contains("/maturity"), "{problems}");
        assert!(problems.contains("notes"), "{problems}");
        assert!(problems.contains("unexpected"), "{problems}");
    }

    #[test]
    fn a_valid_state_has_no_problems_and_the_optional_parts_may_be_missing() {
        assert_eq!(state_problems(&state()), Vec::<String>::new());
        let mut minimal = state();
        let object = minimal.as_object_mut().unwrap();
        object.remove("poses");
        object.remove("sequence");
        assert_eq!(state_problems(&minimal), Vec::<String>::new());
    }

    #[test]
    fn real_and_simulated_are_the_only_sources() {
        for source in ["real", "simulated"] {
            let mut ok = state();
            ok["source"] = json!(source);
            assert!(state_problems(&ok).is_empty());
        }
        let mut bad = state();
        bad["source"] = json!("imagined");
        assert!(state_problems(&bad).join("\n").contains("/source"));
        bad.as_object_mut().unwrap().remove("source");
        assert!(state_problems(&bad).join("\n").contains("source"));
    }

    #[test]
    fn a_state_with_a_wrong_part_is_refused() {
        for (key, value) in [
            ("frame", json!("world")),
            ("timestamp_utc", json!("yesterday")),
            ("sequence", json!(-1)),
            ("joint_states", json!({"shoulder": 0.5})),
            ("joint_states", json!([{"position": 0.5}])),
            ("joint_states", json!([{"name": "a", "position": "0.5"}])),
            ("poses", json!([{"name": "tcp"}])),
        ] {
            let mut bad = state();
            bad[key] = value.clone();
            assert!(!state_problems(&bad).is_empty(), "{key} = {value}");
        }
    }

    #[test]
    fn a_pose_quaternion_must_have_unit_length() {
        let mut bad = state();
        bad["poses"][0]["pose"]["orientation"] = json!({"w": 1.0, "x": 1.0, "y": 0.0, "z": 0.0});
        assert!(state_problems(&bad).join("\n").contains("not 1"));
        let mut near = state();
        near["poses"][0]["pose"]["orientation"] = json!({"w": std::f64::consts::FRAC_1_SQRT_2, "x": 0.0, "y": 0.0, "z": std::f64::consts::FRAC_1_SQRT_2});
        assert!(state_problems(&near).is_empty());
    }

    #[test]
    fn a_joint_named_twice_is_refused() {
        let mut bad = state();
        bad["joint_states"] =
            json!([{"name": "a", "position": 1.0}, {"name": "a", "position": 2.0}]);
        assert!(state_problems(&bad).join("\n").contains("appears twice"));
    }
}
