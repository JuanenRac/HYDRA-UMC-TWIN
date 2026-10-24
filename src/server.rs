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

use crate::certificate::{random_nonce, RiskInput, Signer};
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

fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(b) => {
                        out.push(b);
                        i += 3;
                    }
                    None => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query_param(url: &str, key: &str) -> Option<String> {
    let (_, query) = url.split_once('?')?;
    for pair in query.split('&') {
        // A bare parameter without '=' is skipped, not a reason to stop
        // looking for the one that was asked for.
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        if percent_decode(k) == key {
            return Some(percent_decode(v));
        }
    }
    None
}

/// Resolves a caller-supplied `?workspace=` override, or `None` if the
/// request didn't pass one at all (the real default `workspace` applies
/// then, unchanged). Found while auditing the code: this override used to
/// go straight to the filesystem reader
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
/// `strict_root`, when set, additionally requires the resolved override
/// to be the root itself or a real subdirectory of it - a deliberate,
/// OPT-IN production hardening flag (`--strict-workspace-root`, see
/// main.rs), never the default. Unset, `?workspace=` keeps pointing at
/// any real, existing directory the process can read, exactly the
/// tested, documented behavior above - this never changes that default.
fn resolve_workspace_override(
    url: &str,
    strict_root: Option<&std::path::Path>,
) -> Result<Option<PathBuf>, String> {
    let Some(raw) = query_param(url, "workspace") else {
        return Ok(None);
    };
    let canonical = PathBuf::from(&raw)
        .canonicalize()
        .map_err(|_| format!("workspace override does not exist or is not accessible: {raw}"))?;
    if !canonical.is_dir() {
        return Err(format!("workspace override is not a directory: {raw}"));
    }
    if let Some(root) = strict_root {
        if !canonical.starts_with(root) {
            return Err(format!(
                "workspace override {} is outside the configured --strict-workspace-root {}",
                canonical.display(),
                root.display()
            ));
        }
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
/// `?workspace=`. `strict_root`, when set, also bounds every
/// `?workspace=` override to that root - see `resolve_workspace_override()`'s
/// own doc comment.
#[cfg(test)]
pub fn run(server: Server, workspace: PathBuf, strict_root: Option<PathBuf>) {
    run_with_certifier(server, workspace, strict_root, None);
}

/// What `POST /interlock/certify` needs: the key that signs and the token a caller must hold to ask for a signature.
pub struct Certifier {
    pub signer: Signer,
    pub token: String,
}

/// The largest body `POST /interlock/certify` reads: a report is a few hundred bytes of JSON.
const MAX_CERTIFY_BODY_BYTES: u64 = 4 * 1024;

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= (x ^ y) as usize;
    }
    diff == 0
}

fn bearer_matches(request: &tiny_http::Request, expected: &str) -> bool {
    request
        .headers()
        .iter()
        .find(|h| {
            h.field
                .as_str()
                .as_str()
                .eq_ignore_ascii_case("authorization")
        })
        .and_then(|h| h.value.as_str().split_once(' '))
        .map(|(scheme, presented)| {
            scheme == "Bearer" && constant_time_eq(presented.trim().as_bytes(), expected.as_bytes())
        })
        .unwrap_or(false)
}

fn handle_certify(mut request: tiny_http::Request, certifier: Option<&Certifier>) {
    use std::io::Read;
    let Some(certifier) = certifier else {
        write_json(
            request,
            404,
            &json!({"error": "signing is not configured on this server"}),
        );
        return;
    };
    if !bearer_matches(&request, &certifier.token) {
        let response = tiny_http::Response::from_string(
            json!({"error": "a valid bearer token is required to ask for a certificate"})
                .to_string(),
        )
        .with_status_code(401)
        .with_header(json_header())
        .with_header(Header::from_bytes(&b"WWW-Authenticate"[..], &b"Bearer"[..]).unwrap());
        let _ = request.respond(response);
        return;
    }
    // a web page can send a "simple" cross-site POST to a loopback port: refuse anything from a browser, and anything not declared as JSON
    let mut json_content_type = false;
    for h in request.headers() {
        let name = h.field.as_str().as_str();
        if name.eq_ignore_ascii_case("origin") {
            write_json(
                request,
                403,
                &json!({"error": "requests carrying an Origin header are refused"}),
            );
            return;
        }
        if name.eq_ignore_ascii_case("content-type") {
            json_content_type = h
                .value
                .as_str()
                .to_ascii_lowercase()
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                == "application/json";
        }
    }
    if !json_content_type {
        write_json(
            request,
            415,
            &json!({"error": "Content-Type must be application/json"}),
        );
        return;
    }
    let mut raw = Vec::new();
    if request
        .as_reader()
        .take(MAX_CERTIFY_BODY_BYTES + 1)
        .read_to_end(&mut raw)
        .is_err()
    {
        write_json(
            request,
            400,
            &json!({"error": "could not read the request body"}),
        );
        return;
    }
    if raw.len() as u64 > MAX_CERTIFY_BODY_BYTES {
        write_json(
            request,
            413,
            &json!({"error": format!("request body is larger than {MAX_CERTIFY_BODY_BYTES} bytes")}),
        );
        return;
    }
    let input: RiskInput = match serde_json::from_slice(&raw) {
        Ok(input) => input,
        Err(e) => {
            write_json(
                request,
                400,
                &json!({"error": format!("malformed risk report: {e}")}),
            );
            return;
        }
    };
    let issued_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let nonce = match random_nonce() {
        Ok(nonce) => nonce,
        Err(e) => {
            write_json(request, 500, &json!({"error": e}));
            return;
        }
    };
    match certifier.signer.certify(&input, issued_at_ms, &nonce) {
        Ok(certificate) => write_json(request, 200, &serde_json::to_value(&certificate).unwrap()),
        Err(e) => write_json(request, 400, &json!({"error": e.to_string()})),
    }
}

/// How many requests are served at once. One request that is slow (a `?workspace=` on a network file system that does not answer, a
/// client that sends its body one byte a minute) holds one worker, never the whole server.
pub const DEFAULT_WORKERS: usize = 8;

/// Everything a request needs, shared by the workers.
struct Context {
    workspace: PathBuf,
    strict_root: Option<PathBuf>,
    certifier: Option<Certifier>,
}

/// Like `run`, with the key and token that let `POST /interlock/certify` sign a risk report (None: the route answers 404).
#[cfg(test)]
pub fn run_with_certifier(
    server: Server,
    workspace: PathBuf,
    strict_root: Option<PathBuf>,
    certifier: Option<Certifier>,
) {
    run_with_workers(server, workspace, strict_root, certifier, DEFAULT_WORKERS);
}

/// The same server with `workers` threads (at least one) taking requests from the one listening socket.
pub fn run_with_workers(
    server: Server,
    workspace: PathBuf,
    strict_root: Option<PathBuf>,
    certifier: Option<Certifier>,
    workers: usize,
) {
    let server = std::sync::Arc::new(server);
    let context = std::sync::Arc::new(Context {
        workspace,
        strict_root,
        certifier,
    });
    let mut handles = Vec::new();
    for _ in 1..workers.max(1) {
        let (server, context) = (server.clone(), context.clone());
        handles.push(std::thread::spawn(move || serve_loop(&server, &context)));
    }
    serve_loop(&server, &context);
    for handle in handles {
        let _ = handle.join();
    }
}

fn serve_loop(server: &Server, context: &Context) {
    for request in server.incoming_requests() {
        handle_request(request, context);
    }
}

fn handle_request(request: tiny_http::Request, context: &Context) {
    let workspace = &context.workspace;
    let url = request.url().to_string();
    let path = url.split('?').next().unwrap_or("").to_string();

    if request.method() == &Method::Post && path == "/interlock/certify" {
        handle_certify(request, context.certifier.as_ref());
        return;
    }

    if request.method() != &Method::Get {
        write_json(request, 405, &json!({"error": "method not allowed"}));
        return;
    }

    let effective_workspace = match resolve_workspace_override(&url, context.strict_root.as_deref())
    {
        Ok(Some(resolved)) => resolved,
        Ok(None) => workspace.clone(),
        Err(message) => {
            write_json(request, 400, &json!({"error": message}));
            return;
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
        start_test_server_strict(workspace, None)
    }

    fn start_test_server_strict(workspace: PathBuf, strict_root: Option<PathBuf>) -> u16 {
        let server = bind("127.0.0.1:0").expect("bind on an OS-assigned port must succeed");
        let port = server
            .server_addr()
            .to_ip()
            .expect("tiny_http always binds a real IP socket for an http:// server")
            .port();
        thread::spawn(move || run(server, workspace, strict_root));
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
        cleanup_registry().lock().unwrap().push(path.clone());
        path
    }

    /// Every directory a test creates is removed again when the test
    /// process exits, so repeated runs leave nothing behind in the OS
    /// temp directory.
    fn cleanup_registry() -> &'static std::sync::Mutex<Vec<PathBuf>> {
        static REGISTRY: std::sync::OnceLock<std::sync::Mutex<Vec<PathBuf>>> =
            std::sync::OnceLock::new();
        REGISTRY.get_or_init(|| {
            extern "C" fn remove_all() {
                if let Ok(paths) = cleanup_registry().lock() {
                    for p in paths.iter() {
                        let _ = std::fs::remove_dir_all(p);
                    }
                }
            }
            extern "C" {
                fn atexit(cb: extern "C" fn()) -> i32;
            }
            unsafe {
                atexit(remove_all);
            }
            std::sync::Mutex::new(Vec::new())
        })
    }

    #[test]
    fn workspace_param_is_found_after_a_bare_parameter_and_percent_decoded() {
        assert_eq!(
            query_param("/x?flag&workspace=a%20b%2Fc", "workspace").as_deref(),
            Some("a b/c")
        );
        assert_eq!(query_param("/x?flag", "workspace"), None);
    }

    #[test]
    fn non_get_methods_get_405() {
        let ws = tempdir();
        let port = start_test_server(ws);
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .write_all(
                b"POST /family-status HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        // The server may keep the connection open after a non-GET request, so
        // read the status line only instead of waiting for end of stream.
        let mut buffer = [0u8; 64];
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let read = stream.read(&mut buffer).unwrap();
        let text = String::from_utf8_lossy(&buffer[..read]).into_owned();
        assert!(text.starts_with("HTTP/1.1 405"), "{text}");
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
        // Found while auditing the code: a bogus/typo'd ?workspace=
        // used to silently produce the exact
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
    fn strict_workspace_root_allows_an_override_inside_the_root() {
        let root = tempdir().canonicalize().unwrap();
        let child_ws = root.join("checkout-a");
        std::fs::create_dir_all(&child_ws).unwrap();
        for name in crate::family::EXPECTED_CHILDREN {
            write_manifest(&child_ws, name, "functional");
        }
        let default_ws = tempdir();
        let port = start_test_server_strict(default_ws, Some(root));
        let (status, body) = get(
            port,
            &format!("/family-status?workspace={}", child_ws.display()),
        );
        assert_eq!(status, 200);
        assert!(body.contains("\"allPresent\":true"));
    }

    #[test]
    fn strict_workspace_root_rejects_an_override_outside_the_root() {
        let root = tempdir().canonicalize().unwrap();
        let outside_ws = tempdir(); // a real, existing directory - just not under root
        let default_ws = tempdir();
        let port = start_test_server_strict(default_ws, Some(root));
        let (status, body) = get(
            port,
            &format!("/family-status?workspace={}", outside_ws.display()),
        );
        assert_eq!(status, 400);
        assert!(body.contains("outside the configured --strict-workspace-root"));
    }

    #[test]
    fn strict_workspace_root_does_not_affect_the_default_workspace_when_unset() {
        // The override rejection above must never apply to the SERVER'S
        // OWN default workspace, only to a caller-supplied ?workspace=
        // override - a request with no override at all must still work
        // normally even when --strict-workspace-root is configured.
        let root = tempdir().canonicalize().unwrap();
        let default_ws = tempdir(); // deliberately outside root
        let port = start_test_server_strict(default_ws, Some(root));
        let (status, _body) = get(port, "/family-status");
        assert_eq!(status, 200);
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

    const CERTIFY_TOKEN: &str = "a-certify-token-of-sixteen+";

    fn start_certifying_server() -> u16 {
        let server = bind("127.0.0.1:0").expect("bind on an OS-assigned port must succeed");
        let port = server
            .server_addr()
            .to_ip()
            .expect("a real IP socket")
            .port();
        let certifier = Certifier {
            signer: Signer::from_seed([7u8; 32]),
            token: CERTIFY_TOKEN.to_string(),
        };
        let ws = tempdir();
        thread::spawn(move || run_with_certifier(server, ws, None, Some(certifier)));
        port
    }

    fn post_certify(port: u16, extra_headers: &str, body: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect must succeed");
        let request = format!(
            "POST /interlock/certify HTTP/1.1\r\nHost: 127.0.0.1\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).unwrap();
        let mut raw = String::new();
        stream.read_to_string(&mut raw).unwrap();
        let (headers, resp_body) = raw.split_once("\r\n\r\n").unwrap_or((raw.as_str(), ""));
        let status: u16 = headers
            .lines()
            .next()
            .unwrap_or("")
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        (status, resp_body.to_string())
    }

    fn auth() -> String {
        format!("Authorization: Bearer {CERTIFY_TOKEN}\r\nContent-Type: application/json\r\n")
    }

    const REPORT: &str =
        r#"{"joint":"shoulder","position":0.5,"collision_imminent":false,"distance_m":0.4}"#;

    #[test]
    fn a_caller_with_the_token_gets_a_certificate_that_verifies() {
        let port = start_certifying_server();
        let (status, body) = post_certify(port, &auth(), REPORT);
        assert_eq!(status, 200, "{body}");
        let cert: crate::certificate::Certificate = serde_json::from_str(&body).unwrap();
        let signer = Signer::from_seed([7u8; 32]);
        let public = ed25519_dalek::VerifyingKey::from_bytes(
            &crate::certificate::decode_hex(&signer.public_key_hex())
                .unwrap()
                .try_into()
                .unwrap(),
        )
        .unwrap();
        assert!(cert.verifies_under(&public));
        assert_eq!((cert.joint.as_str(), cert.position), ("shoulder", 0.5));
        assert!(
            cert.issued_at_ms > 1_600_000_000_000,
            "milliseconds, not seconds"
        );
    }

    #[test]
    fn two_certificates_never_share_a_nonce() {
        let port = start_certifying_server();
        let a: crate::certificate::Certificate =
            serde_json::from_str(&post_certify(port, &auth(), REPORT).1).unwrap();
        let b: crate::certificate::Certificate =
            serde_json::from_str(&post_certify(port, &auth(), REPORT).1).unwrap();
        assert_ne!(a.nonce, b.nonce);
    }

    #[test]
    fn without_the_token_or_with_a_wrong_one_nothing_is_signed() {
        let port = start_certifying_server();
        assert_eq!(
            post_certify(port, "Content-Type: application/json\r\n", REPORT).0,
            401
        );
        assert_eq!(
            post_certify(port, "Authorization: Bearer wrong-token-of-sixteen+\r\nContent-Type: application/json\r\n", REPORT).0,
            401
        );
        assert_eq!(
            post_certify(
                port,
                "Authorization: Basic abc\r\nContent-Type: application/json\r\n",
                REPORT
            )
            .0,
            401
        );
    }

    #[test]
    fn a_browser_request_or_a_body_that_is_not_json_is_refused() {
        let port = start_certifying_server();
        assert_eq!(
            post_certify(
                port,
                &format!("{}Origin: http://evil.example\r\n", auth()),
                REPORT
            )
            .0,
            403
        );
        assert_eq!(
            post_certify(
                port,
                &format!("Authorization: Bearer {CERTIFY_TOKEN}\r\nContent-Type: text/plain\r\n"),
                REPORT
            )
            .0,
            415
        );
    }

    #[test]
    fn malformed_and_unsignable_reports_are_400_and_oversized_is_413() {
        let port = start_certifying_server();
        assert_eq!(post_certify(port, &auth(), "not json").0, 400);
        assert_eq!(post_certify(port, &auth(), r#"{"joint":"j"}"#).0, 400);
        assert_eq!(
            post_certify(
                port,
                &auth(),
                r#"{"joint":"","position":1,"collision_imminent":false,"distance_m":1}"#
            )
            .0,
            400
        );
        assert_eq!(
            post_certify(
                port,
                &auth(),
                r#"{"joint":"j","position":1e999,"collision_imminent":false,"distance_m":1}"#
            )
            .0,
            400
        );
        let big = format!(
            r#"{{"joint":"{}","position":1,"collision_imminent":false,"distance_m":1}}"#,
            "j".repeat(5000)
        );
        assert_eq!(post_certify(port, &auth(), &big).0, 413);
    }

    #[test]
    fn a_server_without_a_key_answers_404_whatever_the_caller_holds() {
        let ws = tempdir();
        let port = start_test_server(ws);
        assert_eq!(post_certify(port, &auth(), REPORT).0, 404);
    }

    #[test]
    fn a_client_that_holds_a_connection_open_without_finishing_its_request_does_not_stop_the_others(
    ) {
        let workspace = tempdir();
        let port = start_test_server(workspace);
        // connect and send half a request line, never the rest: this connection is stuck for as long as it stays open
        let mut stuck = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stuck.write_all(b"GET /stats HT").unwrap();
        let mut other = Vec::new();
        for _ in 0..3 {
            let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
            other.push(stream);
        }
        for stream in &mut other {
            stream
                .write_all(b"GET /stats HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
                .unwrap();
        }
        for stream in &mut other {
            let mut raw = String::new();
            stream
                .read_to_string(&mut raw)
                .expect("the other clients are answered while one is stuck");
            assert!(raw.starts_with("HTTP/1.1 200"), "{raw}");
        }
        drop(stuck);
    }
}
