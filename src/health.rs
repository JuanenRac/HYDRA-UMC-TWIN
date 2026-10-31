// =============================================================================
// HYDRA-UMC-TWIN - src/health.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0-or-later - see LICENSE
// =============================================================================
//! An active health check of the children, instead of believing their manifests.
//!
//! `family-status` and `family-sync` read what each child says about itself. This module asks the child to do something and checks the
//! answer:
//!
//! * HYDRA-UMC-PHYSICS-REPLICA: its built binary is run on a two-joint probe URDF (a revolute joint turned by 0.5 rad, then a fixed joint
//!   one metre along x) and the pose of the second joint must be (cos 0.5, sin 0.5, 0). A child that starts but computes something else
//!   is unhealthy.
//! * HYDRA-UMC-HIL-BRIDGE: `GET /stats` of the running bridge must answer 200 with the role it declares.
//! * HYDRA-UMC-SYNTHETIC-DATA-GEN is a Python project with nothing to run or call: it is reported as not probed, never as healthy.
//!
//! Nothing here believes a manifest, and nothing here comes from a request: the binary that is run and the address that is called are
//! configuration (`--physics-replica-bin`, `--hil-bridge-addr`), never a parameter of an HTTP request.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

/// The children that can be probed, in the order `family::EXPECTED_CHILDREN` lists them.
pub const PHYSICS_REPLICA: &str = "HYDRA-UMC-PHYSICS-REPLICA";
pub const HIL_BRIDGE: &str = "HYDRA-UMC-HIL-BRIDGE";
pub const SYNTHETIC_DATA_GEN: &str = "HYDRA-UMC-SYNTHETIC-DATA-GEN";

/// The address the HIL bridge listens on by default.
pub const DEFAULT_HIL_ADDR: &str = "127.0.0.1:8113";
/// How long a probe waits for its child.
pub const DEFAULT_TIMEOUT_MS: u64 = 5_000;

const PROBE_URDF: &str = r#"<robot name="twin_probe">
  <link name="base"/><link name="arm"/><link name="tip"/>
  <joint name="turn" type="revolute"><parent link="base"/><child link="arm"/>
    <origin xyz="0 0 0" rpy="0 0 0"/><axis xyz="0 0 1"/>
    <limit lower="-3.14" upper="3.14" effort="1" velocity="1"/></joint>
  <joint name="reach" type="fixed"><parent link="arm"/><child link="tip"/>
    <origin xyz="1 0 0" rpy="0 0 0"/></joint>
</robot>
"#;
const PROBE_ANGLE: f64 = 0.5;
const PROBE_TOLERANCE: f64 = 1.0e-6;

/// What the probes use. Nothing here is read from a request.
#[derive(Debug, Clone)]
pub struct HealthConfig {
    /// The built binary of PHYSICS-REPLICA. None: the one under the child's `target/` in the workspace.
    pub physics_replica_bin: Option<PathBuf>,
    /// `host:port` of the running HIL bridge.
    pub hil_bridge_addr: String,
    /// The longest a probe waits.
    pub timeout: Duration,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            physics_replica_bin: None,
            hil_bridge_addr: DEFAULT_HIL_ADDR.to_string(),
            timeout: Duration::from_millis(DEFAULT_TIMEOUT_MS),
        }
    }
}

/// The result of one probe.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Probe {
    /// The child did what was asked and gave the right answer.
    Healthy { detail: String },
    /// The child ran and answered, and the answer was wrong.
    Unhealthy { reason: String },
    /// The child could not be reached or run (no binary, nothing listening, no answer in time).
    Unreachable { reason: String },
    /// There is nothing to run or call, so nothing was checked.
    NotProbed { reason: String },
}

impl Probe {
    #[cfg(test)]
    pub fn is_healthy(&self) -> bool {
        matches!(self, Probe::Healthy { .. })
    }

    /// Something was checked and it was not right, or could not be checked. `NotProbed` is neither: there was nothing to run.
    pub fn is_failure(&self) -> bool {
        matches!(self, Probe::Unhealthy { .. } | Probe::Unreachable { .. })
    }
}

/// Every child that can be probed was found healthy (a child with nothing to probe does not count against the family).
pub fn all_probed_healthy(children: &[ChildHealth]) -> bool {
    !children.iter().any(|c| c.probe.is_failure())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChildHealth {
    pub name: String,
    pub probe: Probe,
}

/// Probes every child. The order is the one of `EXPECTED_CHILDREN`.
pub fn check_family_health(workspace: &Path, config: &HealthConfig) -> Vec<ChildHealth> {
    vec![
        ChildHealth {
            name: PHYSICS_REPLICA.to_string(),
            probe: probe_physics_replica(workspace, config),
        },
        ChildHealth {
            name: HIL_BRIDGE.to_string(),
            probe: probe_hil_bridge(&config.hil_bridge_addr, config.timeout),
        },
        ChildHealth {
            name: SYNTHETIC_DATA_GEN.to_string(),
            probe: Probe::NotProbed {
                reason: "a Python project with no service or binary to run; only its manifest is read (family-status)".to_string(),
            },
        },
    ]
}

pub fn built_binary(workspace: &Path) -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "hydra-umc-physics-replica.exe"
    } else {
        "hydra-umc-physics-replica"
    };
    ["release", "debug"]
        .iter()
        .map(|profile| {
            workspace
                .join(PHYSICS_REPLICA)
                .join("target")
                .join(profile)
                .join(name)
        })
        .find(|path| path.is_file())
}

/// A file name for the probe URDF that two probes at the same time do not share.
fn probe_urdf_path() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "hydra-umc-twin-probe-{}-{}.urdf",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ))
}

/// Runs `command` for at most `timeout` and returns its standard output. A child that does not end in time is killed.
fn run_with_timeout(mut command: Command, timeout: Duration) -> Result<(bool, String), String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not start it: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("no standard output")?;
    // read in another thread so a child that writes a lot cannot fill the pipe and stall
    let reader = std::thread::spawn(move || {
        let mut raw = Vec::new();
        let _ = stdout.by_ref().take(1 << 20).read_to_end(&mut raw);
        raw
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                // the reader is not joined: a grandchild that inherited the pipe (a script's `sleep`) would keep it open after the kill
                drop(reader);
                return Err(format!("no answer within {} ms", timeout.as_millis()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => return Err(format!("could not wait for it: {e}")),
        }
    };
    let raw = reader.join().unwrap_or_default();
    Ok((status.success(), String::from_utf8_lossy(&raw).into_owned()))
}

pub fn probe_physics_replica(workspace: &Path, config: &HealthConfig) -> Probe {
    let binary = match &config.physics_replica_bin {
        Some(path) => path.clone(),
        None => match built_binary(workspace) {
            Some(path) => path,
            None => {
                return Probe::Unreachable {
                    reason: format!(
                        "no built binary under {} (build the child, or pass --physics-replica-bin)",
                        workspace.join(PHYSICS_REPLICA).join("target").display()
                    ),
                }
            }
        },
    };
    if !binary.is_file() {
        return Probe::Unreachable {
            reason: format!("{} is not a file", binary.display()),
        };
    }
    let urdf = probe_urdf_path();
    if let Err(e) = std::fs::write(&urdf, PROBE_URDF) {
        return Probe::Unreachable {
            reason: format!("could not write the probe URDF: {e}"),
        };
    }
    let mut command = Command::new(&binary);
    command
        .arg("fk")
        .arg("--urdf")
        .arg(&urdf)
        .arg("--joints")
        .arg(format!("turn={PROBE_ANGLE}"))
        .arg("--json");
    let result = run_with_timeout(command, config.timeout);
    let _ = std::fs::remove_file(&urdf);
    let (success, output) = match result {
        Ok(pair) => pair,
        Err(reason) => return Probe::Unreachable { reason },
    };
    if !success {
        return Probe::Unhealthy {
            reason: "the forward kinematics of the probe URDF ended with an error".to_string(),
        };
    }
    judge_forward_kinematics(&output)
}

/// Whether the output of `fk --json` is the pose the probe URDF must give.
fn judge_forward_kinematics(output: &str) -> Probe {
    let poses: serde_json::Value = match serde_json::from_str(output.trim()) {
        Ok(value) => value,
        Err(e) => {
            return Probe::Unhealthy {
                reason: format!("the answer is not JSON: {e}"),
            }
        }
    };
    let Some(tip) = poses
        .as_array()
        .and_then(|a| a.iter().find(|p| p["joint"] == "reach"))
    else {
        return Probe::Unhealthy {
            reason: "the answer has no pose for the joint \"reach\"".to_string(),
        };
    };
    let read = |axis: &str| tip["position"][axis].as_f64();
    let (Some(x), Some(y), Some(z)) = (read("x"), read("y"), read("z")) else {
        return Probe::Unhealthy {
            reason: "the pose of \"reach\" has no numeric x, y and z".to_string(),
        };
    };
    let (want_x, want_y) = (PROBE_ANGLE.cos(), PROBE_ANGLE.sin());
    if (x - want_x).abs() > PROBE_TOLERANCE
        || (y - want_y).abs() > PROBE_TOLERANCE
        || z.abs() > PROBE_TOLERANCE
    {
        return Probe::Unhealthy {
            reason: format!(
                "the pose of \"reach\" is ({x:.6}, {y:.6}, {z:.6}), expected ({want_x:.6}, {want_y:.6}, 0)"
            ),
        };
    }
    Probe::Healthy {
        detail: format!("forward kinematics of the probe URDF gives ({x:.6}, {y:.6}, {z:.6})"),
    }
}

pub fn probe_hil_bridge(addr: &str, timeout: Duration) -> Probe {
    let socket: SocketAddr = match addr.to_socket_addrs().ok().and_then(|mut a| a.next()) {
        Some(socket) => socket,
        None => {
            return Probe::Unreachable {
                reason: format!("{addr:?} is not a host:port"),
            }
        }
    };
    let mut stream = match TcpStream::connect_timeout(&socket, timeout) {
        Ok(stream) => stream,
        Err(e) => {
            return Probe::Unreachable {
                reason: format!("nothing answers at {addr}: {e}"),
            }
        }
    };
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    let request = format!("GET /stats HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    if let Err(e) = stream.write_all(request.as_bytes()) {
        return Probe::Unreachable {
            reason: format!("could not send the request to {addr}: {e}"),
        };
    }
    let mut raw = Vec::new();
    // a child that stops answering half way is reported with what it had sent
    let read = stream.take(1 << 20).read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw).into_owned();
    let Some((head, body)) = text.split_once("\r\n\r\n") else {
        return Probe::Unreachable {
            reason: match read {
                Err(e) => format!("no answer from {addr}: {e}"),
                Ok(_) => format!("{addr} did not answer with HTTP"),
            },
        };
    };
    let status: u16 = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    if status != 200 {
        return Probe::Unhealthy {
            reason: format!("GET /stats answered {status}"),
        };
    }
    let stats: serde_json::Value = match serde_json::from_str(body.trim()) {
        Ok(value) => value,
        Err(e) => {
            return Probe::Unhealthy {
                reason: format!("GET /stats is not JSON: {e}"),
            }
        }
    };
    match stats["role"].as_str() {
        Some(role) if role.to_ascii_lowercase().contains("hardware-in-the-loop") => {
            Probe::Healthy {
                detail: format!("GET /stats at {addr} answers as \"{role}\""),
            }
        }
        Some(role) => Probe::Unhealthy {
            reason: format!("the service at {addr} says it is \"{role}\", not the HIL bridge"),
        },
        None => Probe::Unhealthy {
            reason: format!("GET /stats at {addr} has no role"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn short() -> Duration {
        Duration::from_millis(600)
    }

    /// An address where nothing usable answers, and that no other test of this run can be handed: a port that was only bound and then released
    /// can be given to another test's server a moment later, and the probe would find a live service. The port stays taken by a listener that
    /// hangs up on every connection, so the probe always meets the same thing.
    fn address_that_answers_nothing() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                drop(stream);
            }
        });
        addr
    }

    /// A one-shot HTTP server on an OS-assigned port that answers every connection with `reply` (raw bytes).
    fn serve_raw(reply: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer);
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        addr
    }

    fn json_reply(status_line: &str, body: &str) -> &'static str {
        Box::leak(
            format!(
                "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .into_boxed_str(),
        )
    }

    #[test]
    fn a_hil_bridge_that_answers_with_its_role_is_healthy() {
        let addr = serve_raw(json_reply(
            "HTTP/1.1 200 OK",
            r#"{"role":"Hardware-in-the-loop real-vs-virtual command bridge","routedReal":0}"#,
        ));
        let probe = probe_hil_bridge(&addr, short());
        assert!(probe.is_healthy(), "{probe:?}");
    }

    #[test]
    fn another_service_on_the_port_is_unhealthy_not_healthy() {
        let addr = serve_raw(json_reply(
            "HTTP/1.1 200 OK",
            r#"{"role":"something else"}"#,
        ));
        assert!(matches!(
            probe_hil_bridge(&addr, short()),
            Probe::Unhealthy { .. }
        ));
        let addr = serve_raw(json_reply("HTTP/1.1 200 OK", r#"{"workspace":"x"}"#));
        assert!(matches!(
            probe_hil_bridge(&addr, short()),
            Probe::Unhealthy { .. }
        ));
    }

    #[test]
    fn an_error_status_or_a_body_that_is_not_json_is_unhealthy() {
        let addr = serve_raw(json_reply("HTTP/1.1 503 Service Unavailable", "{}"));
        match probe_hil_bridge(&addr, short()) {
            Probe::Unhealthy { reason } => assert!(reason.contains("503"), "{reason}"),
            other => panic!("{other:?}"),
        }
        let addr = serve_raw(json_reply("HTTP/1.1 200 OK", "<html>nope</html>"));
        assert!(matches!(
            probe_hil_bridge(&addr, short()),
            Probe::Unhealthy { .. }
        ));
    }

    #[test]
    fn nothing_listening_or_a_bad_address_is_unreachable() {
        let closed = address_that_answers_nothing();
        assert!(matches!(
            probe_hil_bridge(&closed, short()),
            Probe::Unreachable { .. }
        ));
        assert!(matches!(
            probe_hil_bridge("not an address", short()),
            Probe::Unreachable { .. }
        ));
    }

    #[test]
    fn a_service_that_never_answers_is_unreachable_after_the_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let held = std::thread::spawn(move || {
            let accepted = listener.accept();
            std::thread::sleep(Duration::from_millis(1500));
            drop(accepted);
        });
        let started = Instant::now();
        let probe = probe_hil_bridge(&addr, Duration::from_millis(300));
        assert!(matches!(probe, Probe::Unreachable { .. }), "{probe:?}");
        assert!(started.elapsed() < Duration::from_millis(1400));
        held.join().unwrap();
    }

    const GOOD_FK: &str = r#"[{"joint":"turn","position":{"x":0.0,"y":0.0,"z":0.0}},{"joint":"reach","position":{"x":0.877582562,"y":0.479425539,"z":0.0}}]"#;

    #[test]
    fn the_right_pose_is_healthy_and_a_wrong_one_names_what_it_found() {
        assert!(judge_forward_kinematics(GOOD_FK).is_healthy());
        let wrong = GOOD_FK.replace("0.877582562", "0.9");
        match judge_forward_kinematics(&wrong) {
            Probe::Unhealthy { reason } => assert!(reason.contains("0.900000"), "{reason}"),
            other => panic!("{other:?}"),
        }
        for bad in [
            "",
            "nope",
            "[]",
            r#"[{"joint":"reach"}]"#,
            r#"[{"joint":"reach","position":{"x":"a","y":0,"z":0}}]"#,
        ] {
            assert!(
                matches!(judge_forward_kinematics(bad), Probe::Unhealthy { .. }),
                "{bad:?}"
            );
        }
    }

    /// A stand-in for the binary of PHYSICS-REPLICA: a script that prints `output` and ends with `exit_code`, or one that does not end.
    fn fake_binary(dir: &Path, output: &str, exit_code: i32, hang: bool) -> PathBuf {
        let (name, body) = if cfg!(windows) {
            let mut script = String::from("@echo off\r\n");
            if hang {
                script.push_str("ping -n 30 127.0.0.1 > nul\r\n");
            }
            for line in output.lines() {
                script.push_str(&format!("echo {}\r\n", line.replace('%', "%%")));
            }
            script.push_str(&format!("exit /b {exit_code}\r\n"));
            ("fake-physics.bat", script)
        } else {
            let mut script = String::from("#!/bin/sh\n");
            if hang {
                script.push_str("sleep 30\n");
            }
            script.push_str(&format!("cat <<'END'\n{output}\nEND\nexit {exit_code}\n"));
            ("fake-physics.sh", script)
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

    fn scratch(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "hydra-umc-twin-health-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The stand-in is a `.bat` on Windows, and starting `cmd.exe` under a loaded machine can take seconds; a verdict test must never be
    /// decided by that start-up time, so the limit is generous. Only the test that wants the limit to fire sets its own short one.
    fn config_with(bin: PathBuf) -> HealthConfig {
        HealthConfig {
            physics_replica_bin: Some(bin),
            timeout: Duration::from_secs(60),
            ..HealthConfig::default()
        }
    }

    #[test]
    fn a_binary_that_prints_the_right_pose_is_healthy() {
        let dir = scratch("good");
        let bin = fake_binary(&dir, GOOD_FK, 0, false);
        let probe = probe_physics_replica(&dir, &config_with(bin));
        assert!(probe.is_healthy(), "{probe:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_binary_that_prints_the_wrong_pose_or_fails_is_unhealthy() {
        let dir = scratch("bad");
        let wrong = fake_binary(&dir, &GOOD_FK.replace("0.479425539", "0.1"), 0, false);
        assert!(matches!(
            probe_physics_replica(&dir, &config_with(wrong)),
            Probe::Unhealthy { .. }
        ));
        let failing = fake_binary(&dir, "", 3, false);
        assert!(matches!(
            probe_physics_replica(&dir, &config_with(failing)),
            Probe::Unhealthy { .. }
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_binary_that_does_not_end_is_killed_and_unreachable() {
        let dir = scratch("hang");
        let bin = fake_binary(&dir, GOOD_FK, 0, true);
        let config = HealthConfig {
            timeout: Duration::from_millis(300),
            ..config_with(bin)
        };
        let started = Instant::now();
        let probe = probe_physics_replica(&dir, &config);
        assert!(matches!(probe, Probe::Unreachable { .. }), "{probe:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn no_built_binary_is_unreachable_and_says_where_it_looked() {
        let dir = scratch("none");
        match probe_physics_replica(&dir, &HealthConfig::default()) {
            Probe::Unreachable { reason } => assert!(reason.contains("target"), "{reason}"),
            other => panic!("{other:?}"),
        }
        let missing = HealthConfig {
            physics_replica_bin: Some(dir.join("nope")),
            ..HealthConfig::default()
        };
        assert!(matches!(
            probe_physics_replica(&dir, &missing),
            Probe::Unreachable { .. }
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_real_binary_of_a_sibling_checkout_is_healthy_when_it_is_built() {
        // the sibling workspace of this repository; the test only means something where PHYSICS-REPLICA was built
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        if built_binary(&workspace).is_none() {
            eprintln!("skipped: HYDRA-UMC-PHYSICS-REPLICA is not built next to this repository");
            return;
        }
        let probe = probe_physics_replica(&workspace, &HealthConfig::default());
        assert!(probe.is_healthy(), "{probe:?}");
    }

    #[test]
    fn a_child_with_nothing_to_probe_does_not_count_against_the_family() {
        let child = |probe| ChildHealth {
            name: "x".to_string(),
            probe,
        };
        let healthy = Probe::Healthy {
            detail: String::new(),
        };
        let not_probed = Probe::NotProbed {
            reason: String::new(),
        };
        assert!(all_probed_healthy(&[
            child(healthy.clone()),
            child(not_probed)
        ]));
        for bad in [
            Probe::Unhealthy {
                reason: String::new(),
            },
            Probe::Unreachable {
                reason: String::new(),
            },
        ] {
            assert!(!all_probed_healthy(&[child(healthy.clone()), child(bad)]));
        }
    }

    #[test]
    fn the_family_report_lists_the_three_children_and_never_calls_the_python_one_healthy() {
        let dir = scratch("family");
        let report = check_family_health(&dir, &HealthConfig::default());
        let names: Vec<_> = report.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, [PHYSICS_REPLICA, HIL_BRIDGE, SYNTHETIC_DATA_GEN]);
        assert!(matches!(report[2].probe, Probe::NotProbed { .. }));
        assert!(!report[2].probe.is_healthy());
        let _ = std::fs::remove_dir_all(dir);
    }
}
