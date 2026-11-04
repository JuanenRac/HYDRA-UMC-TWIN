// =============================================================================
// HYDRA-UMC-TWIN - src/server/twin.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0-or-later - see LICENSE
// =============================================================================
//! The routes of the kinematic twin: `GET /twin/poses` (`poses.rs`), `POST /dryrun/trajectory` (`dryrun.rs`), `POST /drift/observe` and
//! `GET /drift` (`drift.rs`). Each exists only when the server is started with its switch (`--poses`, `--dryrun`, `--drift`); without it the
//! path is like any other (404 for a `GET`, 405 for a `POST`). All of them use the token of the interlock routes, because they show where the
//! robot is or sign something a bridge relies on.

use std::time::Instant;

use serde_json::json;
use tiny_http::Method;

use super::*;
use crate::drift::DriftMonitor;
use crate::dryrun::{DryRunError, DryRunRoute};
use crate::poses::{PoseError, PoseRoute};
use crate::twincfg::{say, Lang, Say};

/// The routes of the kinematic twin that are on, and the language of the answers when a request does not ask for one.
pub struct Extras {
    pub poses: Option<PoseRoute>,
    pub dryrun: Option<DryRunRoute>,
    pub drift: Option<DriftRoute>,
    pub lang: Lang,
    /// `--health-open`: opens `GET /health`. Closed (the default) the route answers 404 like a route that does not exist; open it answers a
    /// caller on loopback, and any other caller needs the certify token (a twin without one never answers it).
    pub health_open: bool,
}

impl Default for Extras {
    fn default() -> Self {
        Self {
            poses: None,
            dryrun: None,
            drift: None,
            lang: Lang::En,
            health_open: false,
        }
    }
}

pub struct DriftRoute {
    pub monitor: std::sync::Mutex<DriftMonitor>,
    /// The age a `measured` report may have to be taken as the real position of an observation.
    pub max_age_ms: u64,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Whether this request is for one of the routes that are on.
pub(super) fn owns(request: &tiny_http::Request, context: &Context) -> bool {
    let url = request.url();
    let path = url.split('?').next().unwrap_or("");
    let get = request.method() == &Method::Get;
    let post = request.method() == &Method::Post;
    (get && path == "/twin/poses" && context.extras.poses.is_some())
        || (post && path == "/dryrun/trajectory" && context.extras.dryrun.is_some())
        || (post && path == "/drift/observe" && context.extras.drift.is_some())
        || (get && path == "/drift" && context.extras.drift.is_some())
}

pub(super) fn dispatch(request: tiny_http::Request, context: &Context) {
    let path = request.url().split('?').next().unwrap_or("").to_string();
    match path.as_str() {
        "/twin/poses" => handle_poses(request, context),
        "/dryrun/trajectory" => handle_dryrun(request, context),
        "/drift/observe" => handle_drift_observe(request, context),
        _ => handle_drift_summary(request, context),
    }
}

fn request_lang(request: &tiny_http::Request, context: &Context) -> Lang {
    query_param(request.url(), "lang")
        .and_then(|tag| Lang::from_tag(&tag))
        .or_else(|| {
            request
                .headers()
                .iter()
                .find(|h| {
                    h.field
                        .as_str()
                        .as_str()
                        .eq_ignore_ascii_case("accept-language")
                })
                .and_then(|h| Lang::from_accept_language(h.value.as_str()))
        })
        .unwrap_or(context.extras.lang)
}

/// The token check of a `GET`; the answer is written and None returned when it fails.
fn guard_get(request: tiny_http::Request, context: &Context) -> Option<tiny_http::Request> {
    let Some(certifier) = context.certifier.as_ref() else {
        write_json(
            request,
            404,
            &json!({"error": "signing is not configured on this server"}),
        );
        return None;
    };
    if !bearer_matches(&request, &certifier.token) {
        crate::metrics::count(&crate::metrics::UNAUTHORIZED);
        crate::metrics::note_status(401);
        let response = tiny_http::Response::from_string(
            json!({"error": "a valid bearer token is required on the interlock routes"})
                .to_string(),
        )
        .with_status_code(401)
        .with_header(json_header())
        .with_header(Header::from_bytes(&b"WWW-Authenticate"[..], &b"Bearer"[..]).unwrap());
        let _ = request.respond(response);
        return None;
    }
    Some(request)
}

/// `GET /twin/poses`: the TwinState of the robot, with the pose of every joint frame.
fn handle_poses(request: tiny_http::Request, context: &Context) {
    let Some(request) = guard_get(request, context) else {
        return;
    };
    let lang = request_lang(&request, context);
    let route = context.extras.poses.as_ref().expect("checked by owns");
    let reports = context
        .states
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .reports(Instant::now());
    match route.twin_state(&reports, context.sim.is_some(), now_ms()) {
        Ok(state) => {
            // the document is checked against the contract before it is sent: what does not satisfy it is a failure, not an answer
            let problems = crate::schema::state_problems(&state);
            if problems.is_empty() {
                write_json(request, 200, &state);
            } else {
                write_json(
                    request,
                    502,
                    &json!({"error": "the state that was computed does not satisfy the TwinState contract", "problems": problems}),
                );
            }
        }
        Err(PoseError::Incomplete { missing, stale }) => write_json(
            request,
            409,
            &json!({
                "error": "the twin has no fresh report for every joint it needs",
                "code": "incomplete",
                "message": say(lang, Say::PosesIncomplete),
                "missing": missing,
                "stale": stale,
            }),
        ),
        Err(PoseError::Busy) => write_json(
            request,
            503,
            &json!({"error": "too many computations are running; try again"}),
        ),
        Err(PoseError::Refused(reason)) => write_json(
            request,
            422,
            &json!({"error": reason, "note": "no pose was computed: an unknown position is never read as zero"}),
        ),
        Err(PoseError::Timeout(ms)) => write_json(
            request,
            504,
            &json!({"error": format!("the computation did not end within {ms} ms")}),
        ),
        Err(PoseError::Unavailable(reason)) => write_json(request, 502, &json!({"error": reason})),
    }
}

/// `POST /dryrun/trajectory`: run the samples in simulation and answer with a signed verdict.
fn handle_dryrun(request: tiny_http::Request, context: &Context) {
    let lang = request_lang(&request, context);
    let Some((request, raw)) = read_guarded_post(
        request,
        context.certifier.as_ref(),
        crate::dryrun::MAX_BODY_BYTES,
    ) else {
        return;
    };
    let route = context.extras.dryrun.as_ref().expect("checked by owns");
    let certifier = context.certifier.as_ref().expect("checked by the guard");
    let samples = match route.parse_request(&raw) {
        Ok(samples) => samples,
        Err(DryRunError::BadRequest(message)) => {
            write_json(request, 400, &json!({"error": message}));
            return;
        }
        Err(_) => unreachable!("parsing only fails as a bad request"),
    };
    let reported = context
        .states
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .positions();
    let checks = route.check(&samples, context.collision.as_ref().map(|c| (c, &reported)));
    match checks {
        Ok(checks) => {
            let nonce = match random_nonce() {
                Ok(nonce) => nonce,
                Err(e) => {
                    write_json(request, 500, &json!({"error": e}));
                    return;
                }
            };
            let answer =
                route.verdict(&samples, &checks, &certifier.signer, now_ms(), &nonce, lang);
            if answer["verdict"] == "pass" {
                crate::metrics::count(&crate::metrics::DRYRUN_PASS);
            } else {
                crate::metrics::count(&crate::metrics::DRYRUN_NOT_PASS);
            }
            write_json(request, 200, &answer);
        }
        Err(DryRunError::BadRequest(message)) => {
            write_json(request, 400, &json!({"error": message}))
        }
        Err(DryRunError::Busy) => write_json(
            request,
            503,
            &json!({"error": "too many dry runs are running; try again"}),
        ),
        Err(DryRunError::Refused(reason)) => write_json(
            request,
            422,
            &json!({"error": reason, "note": "no verdict was given: an input that cannot be checked is never read as safe"}),
        ),
        Err(DryRunError::Timeout(ms)) => write_json(
            request,
            504,
            &json!({"error": format!("the check did not end within {ms} ms")}),
        ),
        Err(DryRunError::Unavailable(reason)) => {
            write_json(request, 502, &json!({"error": reason}))
        }
    }
}

const MAX_DRIFT_BODY_BYTES: u64 = 1024;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ObserveBody {
    joint: String,
    real: Option<f64>,
    simulated: Option<f64>,
}

/// `POST /drift/observe`: compare one real position with its simulated one.
fn handle_drift_observe(request: tiny_http::Request, context: &Context) {
    let lang = request_lang(&request, context);
    let Some((request, raw)) =
        read_guarded_post(request, context.certifier.as_ref(), MAX_DRIFT_BODY_BYTES)
    else {
        return;
    };
    let route = context.extras.drift.as_ref().expect("checked by owns");
    let body: ObserveBody = match serde_json::from_slice(&raw) {
        Ok(body) => body,
        Err(e) => {
            write_json(
                request,
                400,
                &json!({"error": format!("malformed observation: {e}")}),
            );
            return;
        }
    };
    let real = match body.real {
        Some(real) => real,
        None => {
            let reports = context
                .states
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .reports(Instant::now());
            match reports.iter().find(|(r, _)| r.joint == body.joint) {
                Some((r, age))
                    if r.source == crate::joint_state::StateSource::Measured
                        && age.as_millis() as u64 <= route.max_age_ms =>
                {
                    r.position
                }
                _ => {
                    write_json(
                        request,
                        409,
                        &json!({"error": format!("the twin has no fresh measured report of joint {} (drift.max_age_ms)", body.joint)}),
                    );
                    return;
                }
            }
        }
    };
    let simulated = match body.simulated {
        Some(simulated) => simulated,
        None => match context.sim.as_ref().and_then(|s| {
            s.robot
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .position(&body.joint)
        }) {
            Some(position) => position,
            None => {
                write_json(
                    request,
                    409,
                    &json!({"error": format!("there is no simulated position of joint {}: give \"simulated\" or start with --sim-config", body.joint)}),
                );
                return;
            }
        },
    };
    let result = route
        .monitor
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .observe(&body.joint, real, simulated, now_ms());
    match result {
        Ok(o) => write_json(
            request,
            200,
            &json!({
                "joint": o.joint,
                "real": o.real,
                "simulated": o.simulated,
                "error": o.error,
                "level": o.level.name(),
                "run": o.run,
                "message": say(lang, o.level.sentence()),
            }),
        ),
        Err(message) => write_json(request, 400, &json!({"error": message})),
    }
}

/// `GET /drift`: the state of every joint the monitor has seen.
fn handle_drift_summary(request: tiny_http::Request, context: &Context) {
    let Some(request) = guard_get(request, context) else {
        return;
    };
    let lang = request_lang(&request, context);
    let route = context.extras.drift.as_ref().expect("checked by owns");
    let summary = route
        .monitor
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .summary(lang);
    write_json(request, 200, &summary);
}

#[cfg(test)]
mod tests;
