// =============================================================================
// HYDRA-UMC-TWIN - src/dryrun.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0-or-later - see LICENSE
// =============================================================================
//! The dry run of a trajectory: `POST /dryrun/trajectory`.
//!
//! A route planned by HYDRA-UMC-PATH-PLANNER is a list of samples `{t, joints}`. Before it moves real hardware the twin runs it in simulation:
//! HYDRA-UMC-PHYSICS-REPLICA checks the positions, speeds and accelerations against the limits of the URDF (`check-trajectory`), and, when
//! the collision prediction is on (`--collision-predict`), the same samples are predicted against the zones of HYDRA-UMC-SAFETY-ZONES and
//! against the robot itself. The answer is a **verdict**, signed with the Ed25519 key of the twin, that a bridge can ask for before it moves:
//!
//! ```text
//! hydra-umc-dryrun-verdict-v1
//! <key_id>
//! <robot_id>
//! <trajectory_sha256>   hexadecimal SHA-256 of the canonical text of the samples (see `canonical_trajectory`)
//! <verdict>             pass | fail | incomplete
//! <issued_at_ms>
//! <valid_until_ms>
//! <nonce>               32 hexadecimal characters
//! ```
//!
//! The verdict is bound to the exact samples (a different trajectory has a different digest), to the robot and to a time window
//! (`dryrun.validity_ms`). Only `pass` allows anything. `incomplete` means a check could not run or was switched off (a limit of the URDF
//! that is missing, no collision prediction while the configuration requires it): such a trajectory is **not** declared safe. A child that
//! cannot be run, does not answer in time or refuses the input is an error status and no verdict at all.
//!
//! The route is off unless the server is started with `--dryrun`, the signing key and the token, and a `--twin-config` that names the URDF.
//! This is a simulation with finite differences and over-estimating capsules: it does not replace the limits of the controller.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::collision::{run_with_timeout, CollisionError, CollisionRoute, Scratch};
use crate::twincfg::{say, DryRunCfg, Lang, Say};

pub const DOMAIN: &str = "hydra-umc-dryrun-verdict-v1";
/// Dry runs at the same time: each one starts processes.
pub const MAX_CONCURRENT: usize = 2;
/// A request body is a list of samples: up to about a megabyte for the largest allowed.
pub const MAX_BODY_BYTES: u64 = 1 << 20;

#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub t: f64,
    pub joints: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Fail,
    Incomplete,
}

impl Verdict {
    pub fn name(self) -> &'static str {
        match self {
            Verdict::Pass => "pass",
            Verdict::Fail => "fail",
            Verdict::Incomplete => "incomplete",
        }
    }

    fn sentence(self) -> Say {
        match self {
            Verdict::Pass => Say::VerdictPass,
            Verdict::Fail => Say::VerdictFail,
            Verdict::Incomplete => Say::VerdictIncomplete,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum DryRunError {
    /// The body is not a list of samples (`400`).
    BadRequest(String),
    /// Two dry runs are running already (`503`).
    Busy,
    /// The child refused the input: a joint the robot does not have... (`422`).
    Refused(String),
    /// The child could not be run or did not answer sensibly (`502`).
    Unavailable(String),
    /// The child did not end in time (`504`).
    Timeout(u64),
}

/// What the two checks said.
#[derive(Debug, Clone, PartialEq)]
pub struct Checks {
    pub trajectory_ok: bool,
    pub trajectory_complete: bool,
    /// None: the collision prediction is not on.
    pub collision: Option<CollisionFinding>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CollisionFinding {
    pub collision: bool,
    pub level: String,
}

pub struct DryRunRoute {
    pub binary: PathBuf,
    pub urdf: PathBuf,
    pub physics_config: Option<PathBuf>,
    pub robot_id: String,
    pub cfg: DryRunCfg,
    running: AtomicUsize,
}

/// The text that is hashed: a version line, then one line per sample with the time and the joints in name order, every number in the shortest
/// form that reads back to the same value (`{:?}` of Rust). The bridge builds the same text from the samples it is about to send.
pub fn canonical_trajectory(samples: &[Sample]) -> String {
    let mut text = String::from("hydra-umc-trajectory-v1\n");
    for s in samples {
        text.push_str(&format!("t={:?}", s.t));
        for (name, position) in &s.joints {
            text.push_str(&format!(" {name}={position:?}"));
        }
        text.push('\n');
    }
    text
}

pub fn trajectory_sha256(samples: &[Sample]) -> String {
    crate::certificate::encode_hex(&Sha256::digest(canonical_trajectory(samples).as_bytes()))
}

/// The verdict for what the checks found. Fail closed: only a complete, clean answer is a pass.
pub fn decide(checks: &Checks, cfg: &DryRunCfg) -> Verdict {
    if !checks.trajectory_ok {
        return Verdict::Fail;
    }
    if let Some(found) = &checks.collision {
        if found.collision || (cfg.fail_on_warning && found.level != "clear") {
            return Verdict::Fail;
        }
    }
    if !checks.trajectory_complete {
        return Verdict::Incomplete;
    }
    if checks.collision.is_none() && cfg.require_collision_check {
        return Verdict::Incomplete;
    }
    Verdict::Pass
}

impl DryRunRoute {
    pub fn new(
        binary: PathBuf,
        urdf: PathBuf,
        physics_config: Option<PathBuf>,
        robot_id: String,
        cfg: DryRunCfg,
    ) -> Self {
        Self {
            binary,
            urdf,
            physics_config,
            robot_id,
            cfg,
            running: AtomicUsize::new(0),
        }
    }

    /// Reads `{"samples": [{"t": seconds, "joints": {"joint": position, ...}}, ...]}`.
    pub fn parse_request(&self, raw: &[u8]) -> Result<Vec<Sample>, DryRunError> {
        let bad = |m: String| DryRunError::BadRequest(m);
        let value: Value =
            serde_json::from_slice(raw).map_err(|e| bad(format!("the body is not JSON: {e}")))?;
        let list = value
            .get("samples")
            .and_then(Value::as_array)
            .ok_or_else(|| bad("the body needs a \"samples\" list".into()))?;
        if list.len() < 2 {
            return Err(bad("a trajectory needs at least two samples".into()));
        }
        if list.len() > self.cfg.max_samples {
            return Err(bad(format!(
                "more than {} samples (dryrun.max_samples)",
                self.cfg.max_samples
            )));
        }
        let mut samples: Vec<Sample> = Vec::with_capacity(list.len());
        for (i, entry) in list.iter().enumerate() {
            let t = entry
                .get("t")
                .and_then(Value::as_f64)
                .filter(|t| t.is_finite())
                .ok_or_else(|| bad(format!("sample {i} needs a finite \"t\" in seconds")))?;
            if samples.last().is_some_and(|p| t <= p.t) {
                return Err(bad(format!(
                    "the time of sample {i} must be above that of the sample before"
                )));
            }
            let object = entry
                .get("joints")
                .and_then(Value::as_object)
                .filter(|o| !o.is_empty())
                .ok_or_else(|| bad(format!("sample {i} needs a \"joints\" object")))?;
            if object.len() > crate::joint_state::MAX_JOINTS {
                return Err(bad(format!("sample {i} has too many joints")));
            }
            let mut joints = BTreeMap::new();
            for (name, position) in object {
                crate::twincfg::check_joint_name(&format!("sample {i}"), name).map_err(bad)?;
                let p = position.as_f64().filter(|p| p.is_finite()).ok_or_else(|| {
                    bad(format!(
                        "sample {i}: the position of {name} must be a finite number"
                    ))
                })?;
                joints.insert(name.clone(), p);
            }
            samples.push(Sample { t, joints });
        }
        Ok(samples)
    }

    /// Runs the checks. `collision` is the prediction route of the server and the last reported positions, when the prediction is on.
    pub fn check(
        &self,
        samples: &[Sample],
        collision: Option<(&CollisionRoute, &BTreeMap<String, f64>)>,
    ) -> Result<Checks, DryRunError> {
        if self.running.fetch_add(1, Ordering::SeqCst) >= MAX_CONCURRENT {
            self.running.fetch_sub(1, Ordering::SeqCst);
            return Err(DryRunError::Busy);
        }
        let result = self.check_inner(samples, collision);
        self.running.fetch_sub(1, Ordering::SeqCst);
        result
    }

    fn check_inner(
        &self,
        samples: &[Sample],
        collision: Option<(&CollisionRoute, &BTreeMap<String, f64>)>,
    ) -> Result<Checks, DryRunError> {
        let (trajectory_ok, trajectory_complete) = self.run_check_trajectory(samples)?;
        let collision = match collision {
            None => None,
            Some((route, reported)) => {
                let states: Vec<BTreeMap<String, f64>> =
                    samples.iter().map(|s| s.joints.clone()).collect();
                if states.len() > route.max_states {
                    return Err(DryRunError::BadRequest(format!(
                        "more than {} samples for the collision prediction (--collision-max-states)",
                        route.max_states
                    )));
                }
                match route.predict(&states, reported) {
                    Ok(answer) => Some(CollisionFinding {
                        collision: answer
                            .get("collision")
                            .and_then(Value::as_bool)
                            .ok_or_else(|| {
                                DryRunError::Unavailable(
                                    "the collision prediction answered no verdict".into(),
                                )
                            })?,
                        level: answer
                            .get("level")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                            .to_string(),
                    }),
                    Err(CollisionError::Busy) => return Err(DryRunError::Busy),
                    Err(CollisionError::Refused(reason)) => {
                        return Err(DryRunError::Refused(reason))
                    }
                    Err(CollisionError::Timeout(ms)) => return Err(DryRunError::Timeout(ms)),
                    Err(CollisionError::Unavailable(reason)) => {
                        return Err(DryRunError::Unavailable(reason))
                    }
                    Err(CollisionError::BadRequest(reason)) => {
                        return Err(DryRunError::BadRequest(reason))
                    }
                }
            }
        };
        Ok(Checks {
            trajectory_ok,
            trajectory_complete,
            collision,
        })
    }

    /// `check-trajectory` of the replica: (ok, complete). Exit status 0: within the limits, 1: a limit is broken, 2: the input is wrong.
    fn run_check_trajectory(&self, samples: &[Sample]) -> Result<(bool, bool), DryRunError> {
        let scratch = Scratch::new().map_err(|e| DryRunError::Unavailable(format!("{e:?}")))?;
        let list: Vec<Value> = samples
            .iter()
            .map(|s| json!({"t": s.t, "joints": s.joints}))
            .collect();
        let file = scratch
            .write("samples.json", &json!({ "samples": list }))
            .map_err(|e| DryRunError::Unavailable(format!("{e:?}")))?;
        let mut command = Command::new(&self.binary);
        command
            .arg("check-trajectory")
            .arg("--urdf")
            .arg(&self.urdf)
            .arg("--samples")
            .arg(&file);
        if let Some(config) = &self.physics_config {
            command.arg("--config").arg(config);
        }
        let (code, stdout, stderr) =
            run_with_timeout(command, Duration::from_millis(self.cfg.timeout_ms)).map_err(|e| {
                match e {
                    CollisionError::Timeout(ms) => DryRunError::Timeout(ms),
                    CollisionError::Unavailable(reason) => DryRunError::Unavailable(reason),
                    other => DryRunError::Unavailable(format!("{other:?}")),
                }
            })?;
        match code {
            Some(0) | Some(1) => {
                let answer: Value = serde_json::from_str(stdout.trim()).map_err(|e| {
                    DryRunError::Unavailable(format!("the answer of the child is not JSON: {e}"))
                })?;
                let ok = answer.get("ok").and_then(Value::as_bool);
                let complete = answer.get("complete").and_then(Value::as_bool);
                let (Some(ok), Some(complete)) = (ok, complete) else {
                    return Err(DryRunError::Unavailable(
                        "the answer of the child has no ok or complete".into(),
                    ));
                };
                // a child that says "ok" with the status of a broken limit (or the other way round) is not believed
                if ok != (code == Some(0)) {
                    return Err(DryRunError::Unavailable(
                        "the answer of the child does not agree with its exit status".into(),
                    ));
                }
                Ok((ok, complete))
            }
            Some(2) => Err(DryRunError::Refused(
                stderr
                    .lines()
                    .find(|l| l.contains("check-trajectory"))
                    .or_else(|| stderr.lines().next())
                    .unwrap_or("the child refused the input")
                    .chars()
                    .take(300)
                    .collect(),
            )),
            other => Err(DryRunError::Unavailable(format!(
                "the child ended with status {other:?}"
            ))),
        }
    }

    /// The text a verdict signs.
    pub fn signed_text(
        key_id: &str,
        robot_id: &str,
        sha256: &str,
        verdict: Verdict,
        issued_at_ms: u64,
        valid_until_ms: u64,
        nonce: &str,
    ) -> String {
        format!(
            "{DOMAIN}\n{key_id}\n{robot_id}\n{sha256}\n{}\n{issued_at_ms}\n{valid_until_ms}\n{nonce}",
            verdict.name()
        )
    }

    /// The signed answer for these samples and checks.
    pub fn verdict(
        &self,
        samples: &[Sample],
        checks: &Checks,
        signer: &crate::certificate::Signer,
        now_ms: u64,
        nonce: &str,
        lang: Lang,
    ) -> Value {
        let verdict = decide(checks, &self.cfg);
        let sha = trajectory_sha256(samples);
        let valid_until = now_ms + self.cfg.validity_ms;
        let text = Self::signed_text(
            signer.key_id(),
            &self.robot_id,
            &sha,
            verdict,
            now_ms,
            valid_until,
            nonce,
        );
        json!({
            "verdict": verdict.name(),
            "message": say(lang, verdict.sentence()),
            "robot_id": self.robot_id,
            "samples": samples.len(),
            "trajectory_sha256": sha,
            "issued_at_ms": now_ms,
            "valid_until_ms": valid_until,
            "nonce": nonce,
            "key_id": signer.key_id(),
            "signature": signer.sign_text(&text),
            "checks": {
                "trajectory": {"ok": checks.trajectory_ok, "complete": checks.trajectory_complete},
                "collision": match &checks.collision {
                    Some(f) => json!({"checked": true, "collision": f.collision, "level": f.level}),
                    None => json!({"checked": false}),
                },
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::certificate::{decode_hex, Signer};
    use ed25519_dalek::{Signature, Verifier as _, VerifyingKey};

    // A script that is being written while another test starts a child leaves its descriptor open in that child, and executing it then fails
    // with "Text file busy" on Linux: the tests that write and start scripts take turns.
    static SCRIPT_TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn route(binary: &str, cfg: DryRunCfg) -> DryRunRoute {
        DryRunRoute::new(
            PathBuf::from(binary),
            PathBuf::from("robot.urdf"),
            None,
            "arm".into(),
            cfg,
        )
    }

    fn samples() -> Vec<Sample> {
        vec![
            Sample {
                t: 0.0,
                joints: BTreeMap::from([("b".into(), 0.5), ("a".into(), 0.0)]),
            },
            Sample {
                t: 0.1,
                joints: BTreeMap::from([("b".into(), 0.6), ("a".into(), 0.1)]),
            },
        ]
    }

    fn checks(ok: bool, complete: bool, collision: Option<(bool, &str)>) -> Checks {
        Checks {
            trajectory_ok: ok,
            trajectory_complete: complete,
            collision: collision.map(|(c, l)| CollisionFinding {
                collision: c,
                level: l.into(),
            }),
        }
    }

    #[test]
    fn only_a_complete_and_clean_answer_is_a_pass() {
        let cfg = DryRunCfg::default();
        assert_eq!(
            decide(&checks(true, true, Some((false, "clear"))), &cfg),
            Verdict::Pass
        );
        assert_eq!(
            decide(&checks(false, true, Some((false, "clear"))), &cfg),
            Verdict::Fail
        );
        assert_eq!(
            decide(&checks(true, true, Some((true, "danger"))), &cfg),
            Verdict::Fail
        );
        assert_eq!(
            decide(&checks(true, true, Some((false, "warning"))), &cfg),
            Verdict::Fail
        );
        assert_eq!(
            decide(&checks(true, false, Some((false, "clear"))), &cfg),
            Verdict::Incomplete
        );
        // no collision prediction while the configuration requires it
        assert_eq!(decide(&checks(true, true, None), &cfg), Verdict::Incomplete);
        // a broken limit is a fail whatever else is missing
        assert_eq!(decide(&checks(false, false, None), &cfg), Verdict::Fail);
        // the configuration may relax the two switches
        let relaxed = DryRunCfg {
            require_collision_check: false,
            fail_on_warning: false,
            ..DryRunCfg::default()
        };
        assert_eq!(decide(&checks(true, true, None), &relaxed), Verdict::Pass);
        assert_eq!(
            decide(&checks(true, true, Some((false, "warning"))), &relaxed),
            Verdict::Pass
        );
        assert_eq!(
            decide(&checks(true, true, Some((true, "danger"))), &relaxed),
            Verdict::Fail
        );
    }

    #[test]
    fn the_request_is_a_list_of_samples_and_nothing_else_is_accepted() {
        let r = route(
            "x",
            DryRunCfg {
                max_samples: 3,
                ..DryRunCfg::default()
            },
        );
        let ok = r
            .parse_request(br#"{"samples":[{"t":0,"joints":{"a":0.0}},{"t":0.5,"joints":{"a":0.1,"b":1}}],"urdf":"/etc/passwd"}"#)
            .unwrap();
        assert_eq!(ok.len(), 2);
        assert_eq!(ok[1].joints["b"], 1.0);
        for bad in [
            &br#"nope"#[..],
            br#"{}"#,
            br#"{"samples":[]}"#,
            br#"{"samples":[{"t":0,"joints":{"a":0}}]}"#,
            br#"{"samples":[{"t":0,"joints":{"a":0}},{"t":0,"joints":{"a":0}}]}"#,
            br#"{"samples":[{"t":1,"joints":{"a":0}},{"t":0.5,"joints":{"a":0}}]}"#,
            br#"{"samples":[{"t":0,"joints":{}},{"t":1,"joints":{"a":0}}]}"#,
            br#"{"samples":[{"t":0,"joints":{"a":"x"}},{"t":1,"joints":{"a":0}}]}"#,
            br#"{"samples":[{"t":0,"joints":{"a":null}},{"t":1,"joints":{"a":0}}]}"#,
            br#"{"samples":[{"t":0,"joints":{"a,b":1}},{"t":1,"joints":{"a":0}}]}"#,
            br#"{"samples":[{"t":0,"joints":{"a":0}},{"t":1,"joints":{"a":0}},{"t":2,"joints":{"a":0}},{"t":3,"joints":{"a":0}}]}"#,
        ] {
            assert!(
                matches!(r.parse_request(bad), Err(DryRunError::BadRequest(_))),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[test]
    fn the_digest_follows_the_samples_and_not_the_order_of_the_keys() {
        let a = samples();
        assert!(
            canonical_trajectory(&a).starts_with("hydra-umc-trajectory-v1\nt=0.0 a=0.0 b=0.5\n")
        );
        assert_eq!(trajectory_sha256(&a).len(), 64);
        assert_eq!(trajectory_sha256(&a), trajectory_sha256(&samples()));
        let mut moved = samples();
        moved[1].joints.insert("a".into(), 0.1000001);
        assert_ne!(trajectory_sha256(&a), trajectory_sha256(&moved));
    }

    #[test]
    fn a_verdict_verifies_under_the_public_key_and_is_bound_to_its_fields() {
        let signer = Signer::from_seed([9u8; 32]);
        let r = route("x", DryRunCfg::default());
        let nonce = "00112233445566778899aabbccddeeff";
        let answer = r.verdict(
            &samples(),
            &checks(true, true, Some((false, "clear"))),
            &signer,
            1_000,
            nonce,
            Lang::Es,
        );
        assert_eq!(answer["verdict"], "pass");
        assert_eq!(answer["valid_until_ms"], 6_000);
        assert_eq!(answer["message"], say(Lang::Es, Say::VerdictPass));
        let public = VerifyingKey::from_bytes(
            &decode_hex(&signer.public_key_hex())
                .unwrap()
                .try_into()
                .unwrap(),
        )
        .unwrap();
        let verify = |text: &str| {
            let raw: [u8; 64] = decode_hex(answer["signature"].as_str().unwrap())
                .unwrap()
                .try_into()
                .unwrap();
            public
                .verify(text.as_bytes(), &Signature::from_bytes(&raw))
                .is_ok()
        };
        let text = |robot: &str, sha: &str, verdict: Verdict, until: u64| {
            DryRunRoute::signed_text(signer.key_id(), robot, sha, verdict, 1_000, until, nonce)
        };
        let sha = answer["trajectory_sha256"].as_str().unwrap().to_string();
        assert!(verify(&text("arm", &sha, Verdict::Pass, 6_000)));
        // another robot, another trajectory, another verdict or another window do not verify
        assert!(!verify(&text("other", &sha, Verdict::Pass, 6_000)));
        assert!(!verify(&text("arm", &"0".repeat(64), Verdict::Pass, 6_000)));
        assert!(!verify(&text("arm", &sha, Verdict::Fail, 6_000)));
        assert!(!verify(&text("arm", &sha, Verdict::Pass, 60_000)));
        // the answer states what was checked
        assert_eq!(answer["checks"]["collision"]["checked"], true);
        let no_collision = r.verdict(
            &samples(),
            &checks(true, true, None),
            &signer,
            1_000,
            nonce,
            Lang::En,
        );
        assert_eq!(no_collision["verdict"], "incomplete");
        assert_eq!(no_collision["checks"]["collision"]["checked"], false);
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
                script.push_str("echo check-trajectory: unknown joint wrist 1>&2\r\n");
            }
            script.push_str(&format!("exit /b {exit_code}\r\n"));
            ("fake-check.bat", script)
        } else {
            let mut script = String::from("#!/bin/sh\n");
            if hang {
                script.push_str("sleep 30\n");
            }
            if exit_code == 2 {
                script.push_str("echo 'check-trajectory: unknown joint wrist' >&2\n");
            }
            script.push_str(&format!("cat <<'END'\n{output}\nEND\nexit {exit_code}\n"));
            ("fake-check.sh", script)
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
            "hydra-umc-twin-dryrun-test-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn quick() -> DryRunCfg {
        DryRunCfg {
            timeout_ms: 20_000,
            ..DryRunCfg::default()
        }
    }

    #[test]
    fn the_answer_of_the_child_is_read_and_must_agree_with_its_exit_status() {
        let _turn = SCRIPT_TURN.lock().unwrap_or_else(|e| e.into_inner());
        let dir = scratch_dir("answers");
        let good = fake(&dir, r#"{"ok":true,"complete":true}"#, 0, false);
        let c = route(good.to_str().unwrap(), quick())
            .check(&samples(), None)
            .unwrap();
        assert_eq!((c.trajectory_ok, c.trajectory_complete), (true, true));
        let broken = fake(&dir, r#"{"ok":false,"complete":true}"#, 1, false);
        let c = route(broken.to_str().unwrap(), quick())
            .check(&samples(), None)
            .unwrap();
        assert!(!c.trajectory_ok);
        let liar = fake(&dir, r#"{"ok":true,"complete":true}"#, 1, false);
        assert!(matches!(
            route(liar.to_str().unwrap(), quick()).check(&samples(), None),
            Err(DryRunError::Unavailable(_))
        ));
        let garbage = fake(&dir, "not json", 0, false);
        assert!(matches!(
            route(garbage.to_str().unwrap(), quick()).check(&samples(), None),
            Err(DryRunError::Unavailable(_))
        ));
        let silent = fake(&dir, r#"{"fine":1}"#, 0, false);
        assert!(matches!(
            route(silent.to_str().unwrap(), quick()).check(&samples(), None),
            Err(DryRunError::Unavailable(_))
        ));
        let refuses = fake(&dir, "", 2, false);
        match route(refuses.to_str().unwrap(), quick()).check(&samples(), None) {
            Err(DryRunError::Refused(reason)) => {
                assert!(reason.contains("unknown joint"), "{reason}")
            }
            other => panic!("{other:?}"),
        }
        let crashes = fake(&dir, "", 101, false);
        assert!(matches!(
            route(crashes.to_str().unwrap(), quick()).check(&samples(), None),
            Err(DryRunError::Unavailable(_))
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_child_that_does_not_exist_or_does_not_end_is_an_error_never_a_pass() {
        let _turn = SCRIPT_TURN.lock().unwrap_or_else(|e| e.into_inner());
        assert!(matches!(
            route("definitely-not-a-binary-9f2c", quick()).check(&samples(), None),
            Err(DryRunError::Unavailable(_))
        ));
        let dir = scratch_dir("hang");
        let hangs = fake(&dir, r#"{"ok":true,"complete":true}"#, 0, true);
        let slow = DryRunCfg {
            timeout_ms: 1500,
            ..DryRunCfg::default()
        };
        let started = std::time::Instant::now();
        assert_eq!(
            route(hangs.to_str().unwrap(), slow).check(&samples(), None),
            Err(DryRunError::Timeout(1500))
        );
        assert!(started.elapsed() < Duration::from_secs(10));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn at_most_two_dry_runs_at_once() {
        let r = route("x", quick());
        r.running.store(MAX_CONCURRENT, Ordering::SeqCst);
        assert_eq!(r.check(&samples(), None), Err(DryRunError::Busy));
        assert_eq!(r.running.load(Ordering::SeqCst), MAX_CONCURRENT);
    }

    /// A child that answers `check-trajectory` as a clean run and `predict-collision` with the given text and exit status.
    #[cfg(unix)]
    fn two_faced_child(
        dir: &std::path::Path,
        predict_output: &str,
        predict_code: i32,
        predict_hangs: bool,
    ) -> PathBuf {
        let hang = if predict_hangs { "sleep 30\n" } else { "" };
        let body = format!(
            "#!/bin/sh\ncase \"$1\" in\n  check-trajectory)\n    echo '{{\"ok\":true,\"complete\":true}}'\n    exit 0 ;;\n  predict-collision)\n    {hang}    echo '{predict_output}'\n    [ {predict_code} -eq 2 ] && echo 'predict-collision: unknown joint wrist' >&2\n    exit {predict_code} ;;\nesac\nexit 64\n"
        );
        let path = dir.join("two-faced.sh");
        std::fs::write(&path, body).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    fn collision_route(
        binary: &std::path::Path,
        timeout_ms: u64,
        max_states: usize,
    ) -> CollisionRoute {
        CollisionRoute::new(
            binary.to_path_buf(),
            PathBuf::from("robot.urdf"),
            PathBuf::from("zones.json"),
            PathBuf::from("physics.cfg"),
            Duration::from_millis(timeout_ms),
            max_states,
        )
    }

    #[cfg(unix)]
    #[test]
    fn the_prediction_of_a_collision_is_part_of_the_dry_run_and_each_failure_of_it_is_named() {
        let _turn = SCRIPT_TURN.lock().unwrap_or_else(|e| e.into_inner());
        let dir = scratch_dir("collision");
        let reported = BTreeMap::from([("a".to_string(), 0.0)]);
        let run = |output: &str, code: i32, hangs: bool, timeout_ms: u64, max_states: usize| {
            let child = two_faced_child(&dir, output, code, hangs);
            let dry = route(child.to_str().unwrap(), quick());
            let collision = collision_route(&child, timeout_ms, max_states);
            dry.check(&samples(), Some((&collision, &reported)))
        };
        let clear = run(
            r#"{"collision":false,"level":"clear"}"#,
            0,
            false,
            20_000,
            10,
        )
        .unwrap();
        let finding = clear.collision.unwrap();
        assert!(!finding.collision);
        assert_eq!(finding.level, "clear");
        let contact = run(r#"{"collision":true}"#, 1, false, 20_000, 10).unwrap();
        let finding = contact.collision.unwrap();
        assert!(finding.collision);
        assert_eq!(finding.level, "unknown");
        // an answer without a verdict, or one that disagrees with the exit status, is never read as "clear"
        assert!(matches!(
            run(r#"{"level":"clear"}"#, 0, false, 20_000, 10),
            Err(DryRunError::Unavailable(_))
        ));
        assert!(matches!(
            run(r#"{"collision":false}"#, 1, false, 20_000, 10),
            Err(DryRunError::Unavailable(_))
        ));
        match run("", 2, false, 20_000, 10) {
            Err(DryRunError::Refused(reason)) => {
                assert!(reason.contains("unknown joint"), "{reason}")
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            run("", 101, false, 20_000, 10),
            Err(DryRunError::Unavailable(_))
        ));
        assert_eq!(
            run("{}", 0, true, 1_000, 10),
            Err(DryRunError::Timeout(1_000))
        );
        // more samples than the prediction accepts is a bad request, and the prediction child is not asked
        assert!(matches!(
            run(r#"{"collision":false}"#, 0, false, 20_000, 1),
            Err(DryRunError::BadRequest(_))
        ));
        let _ = std::fs::remove_dir_all(dir);
    }
}
