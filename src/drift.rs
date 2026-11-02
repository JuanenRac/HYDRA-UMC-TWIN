// =============================================================================
// HYDRA-UMC-TWIN - src/drift.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0-or-later - see LICENSE
// =============================================================================
//! The drift monitor: how far the real robot is from the simulated one, joint by joint.
//!
//! A real joint and its simulated twin should stay together. When they part and stay parted, something is out of calibration (a slipped
//! encoder, a loose belt, a model that no longer describes the machine). `POST /drift/observe` takes one pair of positions, the real one and
//! the simulated one (either may be left out: the real one then comes from the newest `measured` report of the twin, the simulated one from
//! the simulated robot), and classifies the difference with the thresholds of the configuration (`drift.warn`, `drift.alarm`, per joint if
//! needed): `ok`, `warn`, and `alarm` only after `drift.alarm_after` observations in a row above the alarm threshold, so a single noisy
//! reading is not an alarm.
//!
//! What is kept: the counters of every joint (`GET /drift`), a JSON Lines log of what is not `ok` and of each change of level (a file of
//! bounded size that is rotated once), and, only if the server was started with `--drift-datalake`, every observation sent to
//! HYDRA-UMC-DATALAKE as a sample of `POST /ingest` (`sourceId`, `kind`, `timestamp`, `sequence`, `fields`) by a thread of its own: a data
//! lake that is down or slow costs the twin nothing but a counter (`dropped`, `failed`). The send speaks plain HTTP and accepts only a loopback
//! address unless `drift.datalake.allow_remote` says otherwise; the token is read from a file and never printed or logged.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::twincfg::{say, DatalakeCfg, DriftCfg, Lang, Say, Threshold};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ok,
    Warn,
    Alarm,
}

impl Level {
    pub fn name(self) -> &'static str {
        match self {
            Level::Ok => "ok",
            Level::Warn => "warn",
            Level::Alarm => "alarm",
        }
    }

    fn number(self) -> u8 {
        match self {
            Level::Ok => 0,
            Level::Warn => 1,
            Level::Alarm => 2,
        }
    }

    pub fn sentence(self) -> Say {
        match self {
            Level::Ok => Say::DriftOk,
            Level::Warn => Say::DriftWarn,
            Level::Alarm => Say::DriftAlarm,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    pub joint: String,
    pub real: f64,
    pub simulated: f64,
    /// `real - simulated`.
    pub error: f64,
    pub level: Level,
    /// Consecutive observations above the alarm threshold, this one included.
    pub run: u32,
    pub stamp_ms: u64,
}

#[derive(Debug, Clone, Default)]
struct JointDrift {
    count: u64,
    max_abs: f64,
    last_error: f64,
    run: u32,
    level: Option<Level>,
    alarms: u64,
}

/// The JSON Lines file of the observations that matter. When the next line would pass `max_bytes` the file becomes `<name>.1` (the previous
/// `.1` is replaced) and a new one begins: the log never grows without limit.
struct LogFile {
    path: PathBuf,
    max_bytes: u64,
}

impl LogFile {
    fn append(&self, line: &str) -> Result<(), String> {
        let size = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        if size > 0 && size + line.len() as u64 + 1 > self.max_bytes {
            let mut rotated = self.path.clone().into_os_string();
            rotated.push(".1");
            let rotated = PathBuf::from(rotated);
            let _ = std::fs::remove_file(&rotated);
            std::fs::rename(&self.path, &rotated)
                .map_err(|e| format!("cannot rotate the drift log: {e}"))?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| format!("cannot open the drift log: {e}"))?;
        writeln!(file, "{line}").map_err(|e| format!("cannot write the drift log: {e}"))
    }
}

pub struct DriftMonitor {
    cfg: DriftCfg,
    joints: BTreeMap<String, JointDrift>,
    log: Option<LogFile>,
    log_errors: u64,
    sink: Option<DatalakeSink>,
    sequence: u64,
}

impl DriftMonitor {
    /// `log_file` is already resolved (an empty name in the configuration means no file).
    pub fn new(cfg: DriftCfg, log_file: Option<PathBuf>, sink: Option<DatalakeSink>) -> Self {
        let log = log_file.map(|path| LogFile {
            path,
            max_bytes: cfg.max_log_bytes,
        });
        Self {
            cfg,
            joints: BTreeMap::new(),
            log,
            log_errors: 0,
            sink,
            sequence: 0,
        }
    }

    pub fn threshold(&self, joint: &str) -> Threshold {
        self.cfg.per_joint.get(joint).copied().unwrap_or(Threshold {
            warn: self.cfg.warn,
            alarm: self.cfg.alarm,
        })
    }

    pub fn observe(
        &mut self,
        joint: &str,
        real: f64,
        simulated: f64,
        now_ms: u64,
    ) -> Result<Observation, String> {
        if !real.is_finite() || !simulated.is_finite() {
            return Err("real and simulated must be finite numbers".into());
        }
        crate::twincfg::check_joint_name("drift", joint)?;
        if !self.joints.contains_key(joint) && self.joints.len() >= crate::joint_state::MAX_JOINTS {
            return Err(format!(
                "the monitor follows {} joints already",
                crate::joint_state::MAX_JOINTS
            ));
        }
        let t = self.threshold(joint);
        let error = real - simulated;
        let abs = error.abs();
        let alarm_after = self.cfg.alarm_after;
        let state = self.joints.entry(joint.to_string()).or_default();
        if abs >= t.alarm {
            state.run = state.run.saturating_add(1);
        } else {
            state.run = 0;
        }
        let level = if state.run >= alarm_after {
            Level::Alarm
        } else if abs >= t.warn {
            Level::Warn
        } else {
            Level::Ok
        };
        let changed = state.level != Some(level) && !(state.level.is_none() && level == Level::Ok);
        state.count += 1;
        state.max_abs = state.max_abs.max(abs);
        state.last_error = error;
        state.level = Some(level);
        if level == Level::Alarm && changed {
            state.alarms += 1;
            crate::metrics::count(&crate::metrics::DRIFT_ALARMS);
        }
        let observation = Observation {
            joint: joint.to_string(),
            real,
            simulated,
            error,
            level,
            run: state.run,
            stamp_ms: now_ms,
        };
        crate::metrics::count(&crate::metrics::DRIFT_OBSERVATIONS);
        if level != Level::Ok || changed {
            if let Some(log) = &self.log {
                let line = json!({
                    "stamp_ms": now_ms,
                    "joint": joint,
                    "real": real,
                    "simulated": simulated,
                    "error": error,
                    "level": level.name(),
                    "run": observation.run,
                })
                .to_string();
                if log.append(&line).is_err() {
                    self.log_errors += 1;
                }
            }
        }
        if let Some(sink) = &self.sink {
            self.sequence += 1;
            sink.send(&observation, self.sequence);
        }
        Ok(observation)
    }

    pub fn summary(&self, lang: Lang) -> Value {
        let joints: Vec<Value> = self
            .joints
            .iter()
            .map(|(name, s)| {
                let level = s.level.unwrap_or(Level::Ok);
                let t = self.threshold(name);
                json!({
                    "joint": name,
                    "observations": s.count,
                    "last_error": s.last_error,
                    "max_abs_error": s.max_abs,
                    "level": level.name(),
                    "run": s.run,
                    "alarms": s.alarms,
                    "warn": t.warn,
                    "alarm": t.alarm,
                    "message": say(lang, level.sentence()),
                })
            })
            .collect();
        let worst = self
            .joints
            .values()
            .filter_map(|s| s.level)
            .max()
            .unwrap_or(Level::Ok);
        json!({
            "level": worst.name(),
            "joints": joints,
            "log_errors": self.log_errors,
            "datalake": match &self.sink {
                Some(sink) => json!({
                    "enabled": true,
                    "sent": sink.sent.load(Ordering::Relaxed),
                    "failed": sink.failed.load(Ordering::Relaxed),
                    "dropped": sink.dropped.load(Ordering::Relaxed),
                }),
                None => json!({"enabled": false}),
            },
        })
    }
}

// ---- the data lake ----

pub struct DatalakeSink {
    tx: SyncSender<String>,
    source_id: String,
    kind: String,
    pub sent: Arc<AtomicU64>,
    pub failed: Arc<AtomicU64>,
    pub dropped: Arc<AtomicU64>,
}

/// `http://host:port` split into the address to connect to and the `Host` header.
fn parse_url(url: &str, allow_remote: bool) -> Result<String, String> {
    let rest = url.trim().strip_prefix("http://").ok_or(
        "drift.datalake.url must start with http:// (only plain HTTP is spoken; use a loopback or a trusted LAN address)",
    )?;
    let authority = rest.trim_end_matches('/');
    if authority.is_empty() || authority.contains('/') || authority.contains('@') {
        return Err(
            "drift.datalake.url must be http://host:port with no path or credentials".into(),
        );
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(']') || p.chars().all(|c| c.is_ascii_digit()) => (h, p),
        _ => (authority, "80"),
    };
    if port.parse::<u16>().is_err() {
        return Err("drift.datalake.url has no valid port".into());
    }
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let loopback = bare == "localhost"
        || bare
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false);
    if !loopback && !allow_remote {
        return Err("drift.datalake.url is not a loopback address: the token and the samples would travel in clear text; set drift.datalake.allow_remote to accept that".into());
    }
    Ok(format!("{host}:{port}"))
}

/// One POST, `Connection: close`, and the status of the answer.
fn http_post(
    address: &str,
    path: &str,
    token: Option<&str>,
    body: &str,
    timeout: Duration,
) -> Result<u16, String> {
    let target = address
        .to_socket_addrs()
        .map_err(|e| format!("cannot resolve {address}: {e}"))?
        .next()
        .ok_or("no address")?;
    let mut stream =
        TcpStream::connect_timeout(&target, timeout).map_err(|e| format!("cannot connect: {e}"))?;
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    let auth = token
        .map(|t| format!("Authorization: Bearer {t}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("cannot send: {e}"))?;
    let mut head = [0u8; 64];
    let mut got = 0;
    while got < head.len() {
        match stream.read(&mut head[got..]) {
            Ok(0) => break,
            Ok(n) => {
                got += n;
                if head[..got].contains(&b'\n') {
                    break;
                }
            }
            Err(e) => return Err(format!("no answer: {e}")),
        }
    }
    String::from_utf8_lossy(&head[..got])
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| "the answer is not HTTP".to_string())
}

impl DatalakeSink {
    /// Starts the thread that sends. `token` is the content of the token file, if there is one.
    pub fn start(cfg: &DatalakeCfg, token: Option<String>) -> Result<Self, String> {
        let address = parse_url(&cfg.url, cfg.allow_remote)?;
        let (tx, rx) = sync_channel::<String>(cfg.queue);
        let (sent, failed, dropped) = (
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
        );
        let (s, f) = (sent.clone(), failed.clone());
        let timeout = Duration::from_millis(cfg.timeout_ms);
        std::thread::spawn(move || {
            for body in rx {
                match http_post(&address, "/ingest", token.as_deref(), &body, timeout) {
                    // 202 is the answer of the data lake to an accepted sample
                    Ok(202) => s.fetch_add(1, Ordering::Relaxed),
                    _ => f.fetch_add(1, Ordering::Relaxed),
                };
            }
        });
        Ok(Self {
            tx,
            source_id: cfg.source_id.clone(),
            kind: cfg.kind.clone(),
            sent,
            failed,
            dropped,
        })
    }

    /// Queues the observation; never waits. A full queue drops it and counts it.
    fn send(&self, o: &Observation, sequence: u64) {
        let abs = o.error.abs();
        let body = json!({
            "sourceId": self.source_id,
            "kind": self.kind,
            "timestamp": o.stamp_ms,
            "sequence": sequence,
            "fields": {
                format!("{}_error", o.joint): o.error,
                format!("{}_abs_error", o.joint): abs,
                format!("{}_level", o.joint): o.level.number(),
            },
        })
        .to_string();
        match self.tx.try_send(body) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn cfg() -> DriftCfg {
        DriftCfg {
            warn: 0.02,
            alarm: 0.05,
            alarm_after: 3,
            ..DriftCfg::default()
        }
    }

    fn monitor() -> DriftMonitor {
        DriftMonitor::new(cfg(), None, None)
    }

    #[test]
    fn a_small_difference_is_ok_a_larger_one_a_warning() {
        let mut m = monitor();
        assert_eq!(m.observe("a", 1.0, 1.01, 0).unwrap().level, Level::Ok);
        let o = m.observe("a", 1.03, 1.0, 1).unwrap();
        assert_eq!(o.level, Level::Warn);
        assert!((o.error - 0.03).abs() < 1e-12);
        assert_eq!(m.observe("a", 1.0, 1.0, 2).unwrap().level, Level::Ok);
    }

    #[test]
    fn one_noisy_reading_is_not_an_alarm_but_a_sustained_one_is() {
        let mut m = monitor();
        assert_eq!(m.observe("a", 1.2, 1.0, 0).unwrap().level, Level::Warn);
        assert_eq!(m.observe("a", 1.0, 1.0, 1).unwrap().level, Level::Ok);
        assert_eq!(m.observe("a", 1.2, 1.0, 2).unwrap().level, Level::Warn);
        assert_eq!(m.observe("a", -1.2, 1.0, 3).unwrap().level, Level::Warn);
        let third = m.observe("a", 1.2, 1.0, 4).unwrap();
        assert_eq!((third.level, third.run), (Level::Alarm, 3));
        // it stays an alarm while the difference lasts and ends when it falls below the warning threshold
        assert_eq!(m.observe("a", 1.2, 1.0, 5).unwrap().level, Level::Alarm);
        assert_eq!(m.observe("a", 1.001, 1.0, 6).unwrap().level, Level::Ok);
        assert_eq!(m.summary(Lang::En)["joints"][0]["alarms"], 1);
    }

    #[test]
    fn a_joint_may_have_its_own_thresholds() {
        let mut c = cfg();
        c.per_joint.insert(
            "wrist".into(),
            Threshold {
                warn: 0.5,
                alarm: 1.0,
            },
        );
        let mut m = DriftMonitor::new(c, None, None);
        assert_eq!(m.observe("wrist", 0.3, 0.0, 0).unwrap().level, Level::Ok);
        assert_eq!(m.observe("elbow", 0.3, 0.0, 0).unwrap().level, Level::Warn);
    }

    #[test]
    fn what_is_not_a_number_is_refused() {
        let mut m = monitor();
        assert!(m.observe("a", f64::NAN, 0.0, 0).is_err());
        assert!(m.observe("a", 0.0, f64::INFINITY, 0).is_err());
        assert!(m.observe("a,b", 0.0, 0.0, 0).is_err());
        assert!(m.observe("", 0.0, 0.0, 0).is_err());
    }

    #[test]
    fn the_summary_speaks_the_language_asked_for() {
        let mut m = monitor();
        m.observe("a", 1.0, 1.0, 0).unwrap();
        let en = m.summary(Lang::En)["joints"][0]["message"].clone();
        let pt = m.summary(Lang::Pt)["joints"][0]["message"].clone();
        assert_ne!(en, pt);
        assert_eq!(m.summary(Lang::En)["level"], "ok");
        assert_eq!(m.summary(Lang::En)["datalake"]["enabled"], false);
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("twin-drift-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_log_keeps_what_matters_and_rotates_at_its_limit() {
        let dir = scratch("log");
        let path = dir.join("drift.jsonl");
        let mut c = cfg();
        c.max_log_bytes = 1_024;
        let mut m = DriftMonitor::new(c, Some(path.clone()), None);
        m.observe("a", 1.0, 1.0, 1).unwrap(); // ok, first: not logged
        assert!(!path.exists());
        m.observe("a", 2.0, 1.0, 2).unwrap(); // warn: logged
        let text = std::fs::read_to_string(&path).unwrap();
        let line: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(line["level"], "warn");
        assert_eq!(line["joint"], "a");
        for i in 0..40 {
            m.observe("a", 2.0, 1.0, 10 + i).unwrap();
        }
        let mut rotated = path.clone().into_os_string();
        rotated.push(".1");
        assert!(PathBuf::from(&rotated).exists(), "the old log is kept once");
        assert!(std::fs::metadata(&path).unwrap().len() <= 1_024);
        assert!(std::fs::metadata(PathBuf::from(&rotated)).unwrap().len() <= 1_024);
        assert_eq!(m.summary(Lang::En)["log_errors"], 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_address_of_the_data_lake_is_plain_http_and_loopback_unless_allowed() {
        assert_eq!(
            parse_url("http://127.0.0.1:8095", false).unwrap(),
            "127.0.0.1:8095"
        );
        assert_eq!(
            parse_url("http://localhost:8095/", false).unwrap(),
            "localhost:8095"
        );
        assert_eq!(parse_url("http://[::1]:8095", false).unwrap(), "[::1]:8095");
        for bad in [
            "https://127.0.0.1:8095",
            "127.0.0.1:8095",
            "http://192.168.0.10:8095",
            "http://127.0.0.1:8095/ingest",
            "http://user@127.0.0.1:8095",
            "http://127.0.0.1:99999",
            "http://",
        ] {
            assert!(parse_url(bad, false).is_err(), "{bad}");
        }
        assert_eq!(
            parse_url("http://192.168.0.10:8095", true).unwrap(),
            "192.168.0.10:8095"
        );
    }

    /// A stand-in for the data lake: answers each request with `status` and hands the requests to the test.
    fn fake_datalake(status: u16, requests: usize) -> (String, std::sync::mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for _ in 0..requests {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut raw = Vec::new();
                let mut buf = [0u8; 2048];
                // read the head, then the body of the declared length
                loop {
                    let n = stream.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&raw).to_string();
                    if let Some((head, body)) = text.split_once("\r\n\r\n") {
                        let length: usize = head
                            .lines()
                            .find_map(|l| l.strip_prefix("Content-Length: "))
                            .and_then(|v| v.trim().parse().ok())
                            .unwrap_or(0);
                        if body.len() >= length {
                            break;
                        }
                    }
                }
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = tx.send(String::from_utf8_lossy(&raw).to_string());
            }
        });
        (url, rx)
    }

    fn wait_for(counter: &AtomicU64, at_least: u64) {
        for _ in 0..400 {
            if counter.load(Ordering::Relaxed) >= at_least {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the counter stayed at {}", counter.load(Ordering::Relaxed));
    }

    #[test]
    fn a_sample_reaches_the_data_lake_in_the_form_of_its_contract_with_the_token() {
        let (url, requests) = fake_datalake(202, 1);
        let sink = DatalakeSink::start(
            &DatalakeCfg {
                url,
                source_id: "twin-arm".into(),
                ..DatalakeCfg::default()
            },
            Some("0123456789abcdef".into()),
        )
        .unwrap();
        let mut m = DriftMonitor::new(cfg(), None, Some(sink));
        let stamp = 1_800_000_000_000;
        m.observe("shoulder", 1.1, 1.0, stamp).unwrap();
        let request = requests.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(
            request.starts_with("POST /ingest HTTP/1.1\r\n"),
            "{request}"
        );
        assert!(request.contains("Authorization: Bearer 0123456789abcdef\r\n"));
        let body: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        // the shape that POST /ingest of the data lake accepts: strings, an integer in milliseconds, numeric fields (no booleans)
        assert_eq!(body["sourceId"], "twin-arm");
        assert_eq!(body["kind"], "twin_drift");
        assert_eq!(body["timestamp"], stamp);
        assert_eq!(body["sequence"], 1);
        let fields = body["fields"].as_object().unwrap();
        assert_eq!(fields.len(), 3);
        assert!(fields.values().all(|v| v.is_number()));
        assert!((fields["shoulder_error"].as_f64().unwrap() - 0.1).abs() < 1e-9);
        assert_eq!(fields["shoulder_level"], 1);
        let sent = m.sink.as_ref().unwrap().sent.clone();
        wait_for(&sent, 1);
        assert_eq!(m.summary(Lang::En)["datalake"]["enabled"], true);
        assert_eq!(m.summary(Lang::En)["datalake"]["failed"], 0);
    }

    #[test]
    fn a_data_lake_that_refuses_or_is_down_costs_a_counter_not_the_twin() {
        // refuses (401)
        let (url, _requests) = fake_datalake(401, 1);
        let sink = DatalakeSink::start(
            &DatalakeCfg {
                url,
                ..DatalakeCfg::default()
            },
            None,
        )
        .unwrap();
        let failed = sink.failed.clone();
        let mut m = DriftMonitor::new(cfg(), None, Some(sink));
        assert!(m.observe("a", 1.0, 1.0, 1_800_000_000_000).is_ok());
        wait_for(&failed, 1);
        // nothing listens at all: the observation still succeeds
        let closed = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", closed.local_addr().unwrap());
        drop(closed);
        let sink = DatalakeSink::start(
            &DatalakeCfg {
                url,
                timeout_ms: 200,
                queue: 1,
                ..DatalakeCfg::default()
            },
            None,
        )
        .unwrap();
        let (failed, dropped) = (sink.failed.clone(), sink.dropped.clone());
        let mut m = DriftMonitor::new(cfg(), None, Some(sink));
        for i in 0..20 {
            assert!(m.observe("a", 1.0, 1.0, 1_800_000_000_000 + i).is_ok());
        }
        wait_for(&failed, 1);
        assert!(failed.load(Ordering::Relaxed) + dropped.load(Ordering::Relaxed) >= 2);
    }

    #[test]
    fn the_sink_refuses_an_address_that_is_not_allowed() {
        assert!(DatalakeSink::start(
            &DatalakeCfg {
                url: "http://10.1.2.3:8095".into(),
                ..DatalakeCfg::default()
            },
            Some("0123456789abcdef".into())
        )
        .is_err());
    }
}
