// =============================================================================
// HYDRA-UMC-TWIN - src/server.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0 - see LICENSE
// =============================================================================
//! Plain JSON/HTTP surface (`tiny_http`, blocking, no async runtime) -
//! same convention as this ecosystem's other real internal-only APIs
//! (Python's own `api.py` files use stdlib `ThreadingHTTPServer`; this
//! is the closest Rust equivalent). GET /family-status and GET
//! /family-sync reach the exact same `check_family_status()`/
//! `assess_family_sync()` functions the CLI's own subcommands already
//! run - real gap this closes: this project's own readiness/sync-
//! contract check was only ever reachable as a one-shot CLI.

use std::path::PathBuf;

use serde_json::json;
use tiny_http::{Header, Method, Response, Server};

use crate::family::{assess_family_sync, check_family_status};

fn json_header() -> Header {
    Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap()
}

fn write_json(request: tiny_http::Request, status: u16, body: &serde_json::Value) {
    let text = body.to_string();
    let response = Response::from_string(text)
        .with_status_code(status)
        .with_header(json_header());
    let _ = request.respond(response);
}

fn query_param(url: &str, key: &str) -> Option<String> {
    let (_, query) = url.split_once('?')?;
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=')?;
        if k == key {
            return Some(v.to_string());
        }
    }
    None
}

/// Resolves a caller-supplied `?workspace=` override, or `None` if the
/// request didn't pass one at all (the real default `workspace` applies
/// then, unchanged). Found in an ecosystem-wide software-improvements
/// audit: this override used to go straight to the filesystem reader
/// with no validation at all - a typo'd or bogus path silently produced
/// the exact same response as a real, empty family ("allPresent":
/// false), with no way for an operator to tell "you gave me a bad path"
/// from "the family genuinely isn't checked out here". Pointing this
/// loopback-only internal API at a whole other real checkout root is a
/// deliberate, tested feature (see the test below) - not something this
/// closes - so this canonicalizes the override (resolving `..`/symlinks,
/// same as the rest of this ecosystem's path-handling) and requires it
/// to actually exist and be a real directory, returning a clear error
/// instead of a misleading "everything missing" for anything else.
fn resolve_workspace_override(url: &str) -> Result<Option<PathBuf>, String> {
    let Some(raw) = query_param(url, "workspace") else {
        return Ok(None);
    };
    let canonical = PathBuf::from(&raw)
        .canonicalize()
        .map_err(|_| format!("workspace override does not exist or is not accessible: {raw}"))?;
    if !canonical.is_dir() {
        return Err(format!("workspace override is not a directory: {raw}"));
    }
    Ok(Some(canonical))
}

/// Binds the real listening socket, split out from `run()` so a test can
/// bind an OS-assigned port (`"127.0.0.1:0"`), read the real port back
/// via `Server::server_addr()`, and only then start serving - without
/// this split, a test would have no way to discover which port a
/// `serve()`-all-in-one call actually bound.
pub fn bind(addr: &str) -> std::io::Result<Server> {
    Server::http(addr).map_err(std::io::Error::other)
}

/// Runs the real, blocking HTTP server forever against an already-bound
/// `server`. `workspace` is the default `check_family_status()`/
/// `assess_family_sync()` target when a request doesn't override it via
/// `?workspace=`.
pub fn run(server: Server, workspace: PathBuf) {
    for request in server.incoming_requests() {
        let url = request.url().to_string();
        let path = url.split('?').next().unwrap_or("").to_string();

        if request.method() != &Method::Get {
            write_json(request, 404, &json!({"error": "not found"}));
            continue;
        }

        let effective_workspace = match resolve_workspace_override(&url) {
            Ok(Some(resolved)) => resolved,
            Ok(None) => workspace.clone(),
            Err(message) => {
                write_json(request, 400, &json!({"error": message}));
                continue;
            }
        };

        match path.as_str() {
            "/family-status" => {
                let statuses = check_family_status(&effective_workspace);
                let all_present = statuses.iter().all(|s| s.is_present());
                write_json(
                    request,
                    200,
                    &json!({
                        "workspace": effective_workspace.display().to_string(),
                        "children": statuses,
                        "allPresent": all_present,
                    }),
                );
            }
            "/family-sync" => {
                let statuses = assess_family_sync(&effective_workspace);
                let all_ready = statuses.iter().all(|s| s.outcome.is_ready());
                write_json(
                    request,
                    200,
                    &json!({
                        "workspace": effective_workspace.display().to_string(),
                        "children": statuses,
                        "allReady": all_ready,
                    }),
                );
            }
            "/stats" => {
                write_json(
                    request,
                    200,
                    &json!({"workspace": workspace.display().to_string()}),
                );
            }
            _ => {
                write_json(request, 404, &json!({"error": "not found"}));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::thread;

    /// Real end-to-end HTTP test helper: binds on an OS-assigned port,
    /// runs the real server in a background thread, and returns the real
    /// port to connect to - same "spin up the real server, hit it with a
    /// real socket" convention as this ecosystem's Python `test_api.py`
    /// files, just over a raw `TcpStream` instead of `urllib` since no
    /// HTTP client crate is a dependency here (matching this module's
    /// own "minimal, no extra deps" reasoning for picking `tiny_http`).
    fn start_test_server(workspace: PathBuf) -> u16 {
        let server = bind("127.0.0.1:0").expect("bind on an OS-assigned port must succeed");
        let port = server
            .server_addr()
            .to_ip()
            .expect("tiny_http always binds a real IP socket for an http:// server")
            .port();
        thread::spawn(move || run(server, workspace));
        port
    }

    /// Sends a real minimal HTTP/1.1 GET request over a real TCP socket
    /// and returns (status code, body). No keep-alive - closes right
    /// after reading the response, which is all a test needs.
    fn get(port: u16, path: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect must succeed");
        let request =
            format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).unwrap();
        let mut raw = String::new();
        stream.read_to_string(&mut raw).unwrap();
        let (headers, body) = raw.split_once("\r\n\r\n").unwrap_or((raw.as_str(), ""));
        let status_line = headers.lines().next().unwrap_or("");
        let status: u16 = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        (status, body.to_string())
    }

    fn write_manifest(workspace: &std::path::Path, repo: &str, maturity: &str) {
        let dir = workspace.join(repo);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("hydra-umc.project.json"),
            format!(r#"{{"name": "{repo}", "version": "0.0.1", "maturity": "{maturity}", "role": "library"}}"#),
        )
        .unwrap();
    }

    fn tempdir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "hydra-umc-twin-server-test-{}-{}",
            std::process::id(),
            n
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn family_status_reports_missing_children_on_an_empty_workspace() {
        let ws = tempdir();
        let port = start_test_server(ws);
        let (status, body) = get(port, "/family-status");
        assert_eq!(status, 200);
        assert!(body.contains("\"allPresent\":false"));
    }

    #[test]
    fn family_status_reports_all_present() {
        let ws = tempdir();
        for name in crate::family::EXPECTED_CHILDREN {
            write_manifest(&ws, name, "functional");
        }
        let port = start_test_server(ws);
        let (status, body) = get(port, "/family-status");
        assert_eq!(status, 200);
        assert!(body.contains("\"allPresent\":true"));
    }

    #[test]
    fn family_status_workspace_query_param_overrides_default() {
        let default_ws = tempdir();
        let other_ws = tempdir();
        for name in crate::family::EXPECTED_CHILDREN {
            write_manifest(&other_ws, name, "functional");
        }
        let port = start_test_server(default_ws);
        let (status, body) = get(
            port,
            &format!("/family-status?workspace={}", other_ws.display()),
        );
        assert_eq!(status, 200);
        assert!(body.contains("\"allPresent\":true"));
    }

    #[test]
    fn family_status_rejects_a_nonexistent_workspace_override_with_a_clear_error() {
        // Found in an ecosystem-wide software-improvements audit: a
        // bogus/typo'd ?workspace= used to silently produce the exact
        // same response as a real, empty family - "allPresent": false -
        // with no way to tell a bad path from a genuinely missing family.
        let default_ws = tempdir();
        let port = start_test_server(default_ws);
        let bogus = std::env::temp_dir().join("hydra-umc-twin-this-path-does-not-exist-at-all");
        let (status, body) = get(
            port,
            &format!("/family-status?workspace={}", bogus.display()),
        );
        assert_eq!(status, 400);
        assert!(body.contains("does not exist"));
    }

    #[test]
    fn family_status_rejects_a_workspace_override_that_is_a_file_not_a_directory() {
        let default_ws = tempdir();
        let file_path = default_ws.join("not-a-directory.txt");
        std::fs::write(&file_path, b"just a file").unwrap();
        let port = start_test_server(default_ws);
        let (status, body) = get(
            port,
            &format!("/family-status?workspace={}", file_path.display()),
        );
        assert_eq!(status, 400);
        assert!(body.contains("not a directory"));
    }

    #[test]
    fn family_sync_reports_ready_for_a_functional_child() {
        let ws = tempdir();
        write_manifest(&ws, "HYDRA-UMC-PHYSICS-REPLICA", "functional");
        let port = start_test_server(ws);
        let (status, body) = get(port, "/family-sync");
        assert_eq!(status, 200);
        assert!(body.contains("\"Ready\""));
    }

    #[test]
    fn family_sync_rejects_a_scaffolding_child() {
        let ws = tempdir();
        write_manifest(&ws, "HYDRA-UMC-PHYSICS-REPLICA", "scaffolding");
        let port = start_test_server(ws);
        let (status, body) = get(port, "/family-sync");
        assert_eq!(status, 200);
        assert!(body.contains("ImmatureMaturity"));
        assert!(body.contains("\"allReady\":false"));
    }

    #[test]
    fn stats_reports_a_workspace_field() {
        let ws = tempdir();
        let port = start_test_server(ws);
        let (status, body) = get(port, "/stats");
        assert_eq!(status, 200);
        assert!(body.contains("\"workspace\""));
    }

    #[test]
    fn unknown_path_is_404() {
        let ws = tempdir();
        let port = start_test_server(ws);
        let (status, _) = get(port, "/nope");
        assert_eq!(status, 404);
    }
}
