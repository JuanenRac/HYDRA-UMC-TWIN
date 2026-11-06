// =============================================================================
// HYDRA-UMC-TWIN - src/metrics.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0 - see LICENSE
// =============================================================================
//! What the twin counts about itself, in the text format of Prometheus (`GET /metrics`, version 0.0.4 of the exposition format).
//!
//! Only numbers about the service are published: how many requests it took, how they ended, how many certificates and state reports it
//! handled, how many joints it follows and the clock of the simulated robot. No joint position, no token, no key, no path and no address ever
//! goes in a metric, so the route can be read by a monitor that is not trusted with the robot. The counters live for the life of the process.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

pub static REQUESTS: AtomicU64 = AtomicU64::new(0);
pub static RESPONSES_2XX: AtomicU64 = AtomicU64::new(0);
pub static RESPONSES_4XX: AtomicU64 = AtomicU64::new(0);
pub static RESPONSES_5XX: AtomicU64 = AtomicU64::new(0);
pub static UNAUTHORIZED: AtomicU64 = AtomicU64::new(0);
pub static CERTIFICATES_SIGNED: AtomicU64 = AtomicU64::new(0);
pub static STATE_REPORTS_ACCEPTED: AtomicU64 = AtomicU64::new(0);
pub static STATE_REPORTS_REFUSED: AtomicU64 = AtomicU64::new(0);
pub static SIM_COMMANDS: AtomicU64 = AtomicU64::new(0);
pub static SIM_STEPS: AtomicU64 = AtomicU64::new(0);
pub static DRYRUN_PASS: AtomicU64 = AtomicU64::new(0);
pub static DRYRUN_NOT_PASS: AtomicU64 = AtomicU64::new(0);
pub static DRIFT_OBSERVATIONS: AtomicU64 = AtomicU64::new(0);
pub static DRIFT_ALARMS: AtomicU64 = AtomicU64::new(0);
pub static WS_SESSIONS: AtomicU64 = AtomicU64::new(0);
pub static WS_REFUSED: AtomicU64 = AtomicU64::new(0);
pub static UAV_ACCEPTED: AtomicU64 = AtomicU64::new(0);
pub static UAV_REFUSED: AtomicU64 = AtomicU64::new(0);

static STARTED: OnceLock<Instant> = OnceLock::new();

/// Mark the start of the process (the first call wins).
pub fn start() {
    let _ = STARTED.get_or_init(Instant::now);
}

pub fn count(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}

/// Count the answer of a request by the class of its status.
pub fn note_status(status: u16) {
    match status {
        200..=299 => count(&RESPONSES_2XX),
        400..=499 => count(&RESPONSES_4XX),
        500..=599 => count(&RESPONSES_5XX),
        _ => {}
    }
}

/// What the server reports about its own state when it is asked: the joints it follows and, when there is a simulated robot, its joints and clock.
pub struct Gauges {
    pub version: &'static str,
    pub joints_tracked: usize,
    pub sim: Option<(usize, f64)>,
}

fn metric(out: &mut String, name: &str, kind: &str, help: &str, value: impl std::fmt::Display) {
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} {kind}");
    let _ = writeln!(out, "{name} {value}");
}

pub fn render(gauges: &Gauges) -> String {
    let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# HELP hydra_umc_twin_info The version of the running service."
    );
    let _ = writeln!(out, "# TYPE hydra_umc_twin_info gauge");
    let _ = writeln!(
        out,
        "hydra_umc_twin_info{{version=\"{}\"}} 1",
        gauges.version
    );
    metric(
        &mut out,
        "hydra_umc_twin_up",
        "gauge",
        "1 while the service answers.",
        1,
    );
    metric(
        &mut out,
        "hydra_umc_twin_uptime_seconds",
        "gauge",
        "Seconds since the process started.",
        STARTED.get().map(|t| t.elapsed().as_secs()).unwrap_or(0),
    );
    metric(
        &mut out,
        "hydra_umc_twin_requests_total",
        "counter",
        "Requests taken.",
        load(&REQUESTS),
    );
    let _ = writeln!(
        out,
        "# HELP hydra_umc_twin_responses_total Answers sent, by the class of the status."
    );
    let _ = writeln!(out, "# TYPE hydra_umc_twin_responses_total counter");
    for (class, counter) in [
        ("2xx", &RESPONSES_2XX),
        ("4xx", &RESPONSES_4XX),
        ("5xx", &RESPONSES_5XX),
    ] {
        let _ = writeln!(
            out,
            "hydra_umc_twin_responses_total{{class=\"{class}\"}} {}",
            load(counter)
        );
    }
    metric(
        &mut out,
        "hydra_umc_twin_unauthorized_total",
        "counter",
        "Requests refused for a missing or wrong token.",
        load(&UNAUTHORIZED),
    );
    metric(
        &mut out,
        "hydra_umc_twin_certificates_signed_total",
        "counter",
        "Risk certificates signed.",
        load(&CERTIFICATES_SIGNED),
    );
    metric(
        &mut out,
        "hydra_umc_twin_state_reports_accepted_total",
        "counter",
        "State reports of the real joints accepted.",
        load(&STATE_REPORTS_ACCEPTED),
    );
    metric(
        &mut out,
        "hydra_umc_twin_state_reports_refused_total",
        "counter",
        "State reports refused (stale, malformed or too many joints).",
        load(&STATE_REPORTS_REFUSED),
    );
    metric(
        &mut out,
        "hydra_umc_twin_dryrun_pass_total",
        "counter",
        "Dry runs of a trajectory whose verdict was pass.",
        load(&DRYRUN_PASS),
    );
    metric(
        &mut out,
        "hydra_umc_twin_dryrun_not_pass_total",
        "counter",
        "Dry runs of a trajectory whose verdict was fail or incomplete.",
        load(&DRYRUN_NOT_PASS),
    );
    metric(
        &mut out,
        "hydra_umc_twin_drift_observations_total",
        "counter",
        "Pairs of a real and a simulated joint compared.",
        load(&DRIFT_OBSERVATIONS),
    );
    metric(
        &mut out,
        "hydra_umc_twin_drift_alarms_total",
        "counter",
        "Times a joint went into the drift alarm.",
        load(&DRIFT_ALARMS),
    );
    metric(
        &mut out,
        "hydra_umc_twin_ws_sessions_total",
        "counter",
        "WebSocket sessions opened.",
        load(&WS_SESSIONS),
    );
    metric(
        &mut out,
        "hydra_umc_twin_ws_refused_total",
        "counter",
        "WebSocket sessions refused (token, origin or too many sessions).",
        load(&WS_REFUSED),
    );
    metric(
        &mut out,
        "hydra_umc_twin_uav_states_accepted_total",
        "counter",
        "States of a vehicle accepted.",
        load(&UAV_ACCEPTED),
    );
    metric(
        &mut out,
        "hydra_umc_twin_uav_states_refused_total",
        "counter",
        "States of a vehicle refused.",
        load(&UAV_REFUSED),
    );
    metric(
        &mut out,
        "hydra_umc_twin_joints_tracked",
        "gauge",
        "Joints whose state the twin follows.",
        gauges.joints_tracked,
    );
    if let Some((joints, clock_s)) = gauges.sim {
        metric(
            &mut out,
            "hydra_umc_twin_sim_joints",
            "gauge",
            "Joints of the simulated robot.",
            joints,
        );
        metric(
            &mut out,
            "hydra_umc_twin_sim_clock_seconds",
            "gauge",
            "Simulated time that has passed.",
            clock_s,
        );
        metric(
            &mut out,
            "hydra_umc_twin_sim_commands_total",
            "counter",
            "Orders given to the simulated robot.",
            load(&SIM_COMMANDS),
        );
        metric(
            &mut out,
            "hydra_umc_twin_sim_steps_total",
            "counter",
            "Times the simulated time moved.",
            load(&SIM_STEPS),
        );
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The shape the exposition format asks for: every line is a comment (`# HELP`/`# TYPE`) or `name{labels} value`, a name is declared
    /// before it is used and every value is a number.
    pub(crate) fn assert_valid_exposition(text: &str) {
        let mut declared = std::collections::BTreeSet::new();
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("# TYPE ") {
                let mut parts = rest.split_whitespace();
                let name = parts.next().expect("a name");
                let kind = parts.next().expect("a type");
                assert!(
                    ["counter", "gauge", "histogram", "summary", "untyped"].contains(&kind),
                    "{line}"
                );
                declared.insert(name.to_string());
            } else if line.starts_with("# HELP ") {
                assert!(line.split_whitespace().count() >= 4, "{line}");
            } else if !line.is_empty() {
                let (series, value) = line.rsplit_once(' ').unwrap_or_else(|| panic!("{line}"));
                let name = series.split('{').next().unwrap();
                assert!(
                    name.chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':'),
                    "{line}"
                );
                assert!(declared.contains(name), "{name} used before its TYPE line");
                assert!(value.parse::<f64>().is_ok(), "{line}");
                if let Some(labels) = series.split_once('{') {
                    assert!(labels.1.ends_with('}'), "{line}");
                }
            }
        }
    }

    #[test]
    fn the_text_is_valid_and_has_no_position_token_or_path() {
        start();
        count(&REQUESTS);
        note_status(200);
        note_status(404);
        note_status(503);
        let text = render(&Gauges {
            version: "9.9.9",
            joints_tracked: 3,
            sim: Some((2, 1.25)),
        });
        assert_valid_exposition(&text);
        assert!(text.contains("hydra_umc_twin_info{version=\"9.9.9\"} 1"));
        assert!(text.contains("hydra_umc_twin_joints_tracked 3"));
        assert!(text.contains("hydra_umc_twin_sim_clock_seconds 1.25"));
        assert!(text.contains("hydra_umc_twin_responses_total{class=\"4xx\"}"));
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            for forbidden in ["token", "position", "key", "/", "\\"] {
                assert!(
                    !line.to_lowercase().contains(forbidden),
                    "{forbidden} in the metric line {line}"
                );
            }
        }
    }

    #[test]
    fn the_simulation_series_exist_only_with_a_simulated_robot() {
        let without = render(&Gauges {
            version: "1.0.0",
            joints_tracked: 0,
            sim: None,
        });
        assert!(!without.contains("hydra_umc_twin_sim_"));
        assert_valid_exposition(&without);
    }

    #[test]
    fn a_status_is_counted_in_its_class_and_a_counter_only_grows() {
        let before = RESPONSES_4XX.load(Ordering::Relaxed);
        note_status(409);
        note_status(101);
        assert!(RESPONSES_4XX.load(Ordering::Relaxed) > before);
    }
}
