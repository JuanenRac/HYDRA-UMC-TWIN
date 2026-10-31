// =============================================================================
// HYDRA-UMC-TWIN - src/collision.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0-or-later - see LICENSE
// =============================================================================
//! Collision prediction for a short trajectory: `POST /predict/collision`.
//!
//! The twin does not compute the geometry itself. It asks HYDRA-UMC-PHYSICS-REPLICA (`predict-collision`), which places the links of the
//! robot with the forward kinematics of the URDF and compares them with the zones of the zones file of HYDRA-UMC-SAFETY-ZONES and with
//! each other, always over-estimating (see `docs/COLLISION_PREDICTION.md`). What the twin adds is what only it knows: where the real joints
//! were last reported to be, so a trajectory can name only the joints that move and the rest are taken from the last reports.
//!
//! Everything that decides the answer comes from the configuration of the server, never from the request: the binary, the URDF, the zones,
//! the sizes of the robot and the time limit. The request only carries the states. The route is off unless the server is started with
//! `--collision-predict`, and the answer is information: nothing in the twin (the certificates, `/interlock/*`) reads it.
//!
//! An answer is only ever "clear" when the child said so. A child that cannot be run, does not answer in time or refuses the input is an
//! error (`502`, `504`, `422`), never "no collision".

use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;

/// How many predictions run at the same time: each one starts a process, so a flood of requests cannot start a flood of processes.
pub const MAX_CONCURRENT: usize = 2;
/// The default and the largest number of states a request may carry.
pub const DEFAULT_MAX_STATES: usize = 64;
pub const HARD_MAX_STATES: usize = 256;
pub const DEFAULT_TIMEOUT_MS: u64 = 5_000;
/// A request body is a list of states: a few kilobytes.
pub const MAX_BODY_BYTES: u64 = 64 * 1024;
/// The longest answer read from the child.
const MAX_ANSWER_BYTES: u64 = 1 << 20;

/// What the prediction runs with. Nothing here is read from a request.
#[derive(Debug)]
pub struct CollisionRoute {
    pub binary: PathBuf,
    pub urdf: PathBuf,
    pub zones: PathBuf,
    pub config: PathBuf,
    pub timeout: Duration,
    pub max_states: usize,
    running: AtomicUsize,
}

#[derive(Debug, PartialEq)]
pub enum CollisionError {
    /// The request is not a list of states (`400`).
    BadRequest(String),
    /// Too many predictions are running (`503`).
    Busy,
    /// The child refused the input: a joint it does not have, a position that is missing... (`422`).
    Refused(String),
    /// The child could not be run or did not answer sensibly (`502`).
    Unavailable(String),
    /// The child did not end in time (`504`).
    Timeout(u64),
}

impl CollisionRoute {
    pub fn new(
        binary: PathBuf,
        urdf: PathBuf,
        zones: PathBuf,
        config: PathBuf,
        timeout: Duration,
        max_states: usize,
    ) -> Self {
        Self {
            binary,
            urdf,
            zones,
            config,
            timeout,
            max_states,
            running: AtomicUsize::new(0),
        }
    }

    /// Reads `{"states": [{"joint": position, ...}, ...]}`.
    pub fn parse_request(&self, raw: &[u8]) -> Result<Vec<BTreeMap<String, f64>>, CollisionError> {
        let value: Value = serde_json::from_slice(raw)
            .map_err(|e| CollisionError::BadRequest(format!("the body is not JSON: {e}")))?;
        let list = value
            .get("states")
            .and_then(Value::as_array)
            .ok_or_else(|| CollisionError::BadRequest("the body needs a \"states\" list".into()))?;
        if list.is_empty() {
            return Err(CollisionError::BadRequest("\"states\" is empty".into()));
        }
        if list.len() > self.max_states {
            return Err(CollisionError::BadRequest(format!(
                "more than {} states (--collision-max-states)",
                self.max_states
            )));
        }
        let mut states = Vec::with_capacity(list.len());
        for (i, entry) in list.iter().enumerate() {
            let object = entry.as_object().ok_or_else(|| {
                CollisionError::BadRequest(format!(
                    "state {i} must be an object of joint to position"
                ))
            })?;
            if object.len() > crate::joint_state::MAX_JOINTS {
                return Err(CollisionError::BadRequest(format!(
                    "state {i} has too many joints"
                )));
            }
            let mut state = BTreeMap::new();
            for (joint, position) in object {
                if joint.is_empty() || joint.len() > crate::joint_state::MAX_JOINT_NAME_LEN {
                    return Err(CollisionError::BadRequest(format!(
                        "state {i}: a joint name must have 1 to {} bytes",
                        crate::joint_state::MAX_JOINT_NAME_LEN
                    )));
                }
                match position.as_f64() {
                    Some(p) if p.is_finite() => {
                        state.insert(joint.clone(), p);
                    }
                    _ => {
                        return Err(CollisionError::BadRequest(format!(
                            "state {i}: the position of {joint} must be a finite number"
                        )))
                    }
                }
            }
            states.push(state);
        }
        Ok(states)
    }

    /// Asks the child. `reported` is the last position of every joint the bridge reported: it gives each state the joints the request left
    /// out.
    pub fn predict(
        &self,
        states: &[BTreeMap<String, f64>],
        reported: &BTreeMap<String, f64>,
    ) -> Result<Value, CollisionError> {
        if self.running.fetch_add(1, Ordering::SeqCst) >= MAX_CONCURRENT {
            self.running.fetch_sub(1, Ordering::SeqCst);
            return Err(CollisionError::Busy);
        }
        let result = self.run_child(states, reported);
        self.running.fetch_sub(1, Ordering::SeqCst);
        result
    }

    fn run_child(
        &self,
        states: &[BTreeMap<String, f64>],
        reported: &BTreeMap<String, f64>,
    ) -> Result<Value, CollisionError> {
        let scratch = Scratch::new()?;
        let trajectory =
            scratch.write("trajectory.json", &serde_json::json!({ "states": states }))?;
        let base = scratch.write("base.json", &serde_json::json!(reported))?;
        let mut command = Command::new(&self.binary);
        command
            .arg("predict-collision")
            .arg("--urdf")
            .arg(&self.urdf)
            .arg("--zones")
            .arg(&self.zones)
            .arg("--config")
            .arg(&self.config)
            .arg("--trajectory")
            .arg(&trajectory)
            .arg("--base")
            .arg(&base)
            .arg("--json");
        let (code, stdout, stderr) = run_with_timeout(command, self.timeout)?;
        match code {
            // 0: no contact predicted, 1: a contact is predicted; both carry the JSON answer
            Some(0) | Some(1) => {
                let answer: Value = serde_json::from_str(stdout.trim()).map_err(|e| {
                    CollisionError::Unavailable(format!("the answer of the child is not JSON: {e}"))
                })?;
                let consistent =
                    answer.get("collision").and_then(Value::as_bool) == Some(code == Some(1));
                if !consistent {
                    return Err(CollisionError::Unavailable(
                        "the answer of the child does not agree with its exit status".into(),
                    ));
                }
                Ok(answer)
            }
            Some(2) => Err(CollisionError::Refused(first_line(&stderr))),
            other => Err(CollisionError::Unavailable(format!(
                "the child ended with status {other:?}"
            ))),
        }
    }
}

fn first_line(text: &str) -> String {
    text.lines()
        .find(|l| l.contains("predict-collision"))
        .or_else(|| text.lines().next())
        .unwrap_or("the child refused the input")
        .chars()
        .take(300)
        .collect()
}

/// A directory for the files of one prediction, removed when it is dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self, CollisionError> {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "hydra-umc-twin-collision-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir)
            .map_err(|e| CollisionError::Unavailable(format!("no scratch directory: {e}")))?;
        Ok(Self(dir))
    }

    fn write(&self, name: &str, value: &Value) -> Result<PathBuf, CollisionError> {
        let path = self.0.join(name);
        std::fs::write(&path, value.to_string())
            .map_err(|e| CollisionError::Unavailable(format!("cannot write {name}: {e}")))?;
        Ok(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runs `command` for at most `timeout`; returns its exit code, its standard output and its standard error. A child that does not end in
/// time is killed.
fn run_with_timeout(
    mut command: Command,
    timeout: Duration,
) -> Result<(Option<i32>, String, String), CollisionError> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| CollisionError::Unavailable(format!("could not start the child: {e}")))?;
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let out_reader = std::thread::spawn(move || {
        let mut raw = Vec::new();
        let _ = stdout.by_ref().take(MAX_ANSWER_BYTES).read_to_end(&mut raw);
        raw
    });
    let err_reader = std::thread::spawn(move || {
        let mut raw = Vec::new();
        let _ = stderr.by_ref().take(16 * 1024).read_to_end(&mut raw);
        raw
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                drop(out_reader);
                drop(err_reader);
                return Err(CollisionError::Timeout(timeout.as_millis() as u64));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => {
                return Err(CollisionError::Unavailable(format!(
                    "could not wait for the child: {e}"
                )))
            }
        }
    };
    let out = out_reader.join().unwrap_or_default();
    let err = err_reader.join().unwrap_or_default();
    Ok((
        status.code(),
        String::from_utf8_lossy(&out).into_owned(),
        String::from_utf8_lossy(&err).into_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route_with_timeout(binary: &str, timeout_ms: u64) -> CollisionRoute {
        CollisionRoute::new(
            PathBuf::from(binary),
            PathBuf::from("robot.urdf"),
            PathBuf::from("zones.json"),
            PathBuf::from("config.json"),
            Duration::from_millis(timeout_ms),
            4,
        )
    }

    /// A generous limit: starting a script can take seconds on a busy machine, and these tests are not about time.
    fn route(binary: &str) -> CollisionRoute {
        route_with_timeout(binary, 20_000)
    }

    #[test]
    fn the_request_is_a_list_of_states_and_nothing_else_is_accepted() {
        let r = route("x");
        let ok = r
            .parse_request(br#"{"states":[{"shoulder":0.5},{"shoulder":1.0,"elbow":-1}]}"#)
            .unwrap();
        assert_eq!(ok.len(), 2);
        assert_eq!(ok[1]["elbow"], -1.0);
        for bad in [
            &br#"nope"#[..],
            br#"{}"#,
            br#"{"states":[]}"#,
            br#"{"states":[1]}"#,
            br#"{"states":[{"j":"a"}]}"#,
            br#"{"states":[{"j":null}]}"#,
            br#"{"states":[{"":1}]}"#,
            br#"{"states":[{},{},{},{},{}]}"#,
        ] {
            assert!(
                matches!(r.parse_request(bad), Err(CollisionError::BadRequest(_))),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
        // the paths of the files are not something a request can choose
        let sneaky = r
            .parse_request(br#"{"states":[{"a":1}],"urdf":"/etc/passwd","bin":"sh"}"#)
            .unwrap();
        assert_eq!(sneaky.len(), 1);
    }

    #[test]
    fn a_child_that_does_not_exist_is_unavailable_never_clear() {
        let r = route("definitely-not-a-binary-9f2c");
        let states = vec![BTreeMap::from([("a".to_string(), 1.0)])];
        assert!(matches!(
            r.predict(&states, &BTreeMap::new()),
            Err(CollisionError::Unavailable(_))
        ));
    }

    #[test]
    fn at_most_two_predictions_run_at_once() {
        let r = route("x");
        r.running.store(MAX_CONCURRENT, Ordering::SeqCst);
        let states = vec![BTreeMap::new()];
        assert_eq!(
            r.predict(&states, &BTreeMap::new()),
            Err(CollisionError::Busy)
        );
        // a refused request does not leave the counter higher
        assert_eq!(r.running.load(Ordering::SeqCst), MAX_CONCURRENT);
    }

    fn fake(dir: &std::path::Path, output: &str, exit_code: i32, hang: bool) -> PathBuf {
        let (name, body) = if cfg!(windows) {
            let mut script = String::from("@echo off\r\n");
            if hang {
                script.push_str("ping -n 30 127.0.0.1 > nul\r\n");
            }
            for line in output.lines() {
                script.push_str(&format!("echo {}\r\n", line.replace('%', "%%")));
            }
            if exit_code == 2 {
                script.push_str("echo predict-collision: unknown joint wrist in a state 1>&2\r\n");
            }
            script.push_str(&format!("exit /b {exit_code}\r\n"));
            ("fake-predict.bat", script)
        } else {
            let mut script = String::from("#!/bin/sh\n");
            if hang {
                script.push_str("sleep 30\n");
            }
            if exit_code == 2 {
                script.push_str("echo 'predict-collision: unknown joint wrist in a state' >&2\n");
            }
            script.push_str(&format!("cat <<'END'\n{output}\nEND\nexit {exit_code}\n"));
            ("fake-predict.sh", script)
        };
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    fn scratch_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "hydra-umc-twin-collision-test-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn states() -> Vec<BTreeMap<String, f64>> {
        vec![BTreeMap::from([("a".to_string(), 1.0)])]
    }

    #[test]
    fn the_answer_of_the_child_is_passed_on_and_must_agree_with_its_exit_status() {
        let dir = scratch_dir("answers");
        let clear = fake(&dir, r#"{"collision":false,"level":"clear"}"#, 0, false);
        let answer = route(clear.to_str().unwrap())
            .predict(&states(), &BTreeMap::new())
            .unwrap();
        assert_eq!(answer["collision"], false);
        let hit = fake(&dir, r#"{"collision":true,"level":"danger"}"#, 1, false);
        let answer = route(hit.to_str().unwrap())
            .predict(&states(), &BTreeMap::new())
            .unwrap();
        assert_eq!(answer["level"], "danger");
        // a child that says "collision: false" with the status of a contact (or the other way round) is not believed
        let liar = fake(&dir, r#"{"collision":false}"#, 1, false);
        assert!(matches!(
            route(liar.to_str().unwrap()).predict(&states(), &BTreeMap::new()),
            Err(CollisionError::Unavailable(_))
        ));
        let garbage = fake(&dir, "not json", 0, false);
        assert!(matches!(
            route(garbage.to_str().unwrap()).predict(&states(), &BTreeMap::new()),
            Err(CollisionError::Unavailable(_))
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_refusal_of_the_child_is_a_refusal_with_its_reason() {
        let dir = scratch_dir("refusal");
        let refuses = fake(&dir, "", 2, false);
        match route(refuses.to_str().unwrap()).predict(&states(), &BTreeMap::new()) {
            Err(CollisionError::Refused(reason)) => {
                assert!(reason.contains("unknown joint"), "{reason}")
            }
            other => panic!("{other:?}"),
        }
        let crashes = fake(&dir, "", 101, false);
        assert!(matches!(
            route(crashes.to_str().unwrap()).predict(&states(), &BTreeMap::new()),
            Err(CollisionError::Unavailable(_))
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_child_that_does_not_end_is_killed_and_the_answer_is_a_timeout() {
        let dir = scratch_dir("hang");
        let hangs = fake(&dir, r#"{"collision":false}"#, 0, true);
        let started = Instant::now();
        let result =
            route_with_timeout(hangs.to_str().unwrap(), 1500).predict(&states(), &BTreeMap::new());
        assert_eq!(result, Err(CollisionError::Timeout(1500)));
        assert!(started.elapsed() < Duration::from_secs(10));
        let _ = std::fs::remove_dir_all(dir);
    }
}
