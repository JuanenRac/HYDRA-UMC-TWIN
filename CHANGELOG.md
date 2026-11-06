# Changelog

All notable changes to this project are documented here.

## [Unreleased]

- **No date-and-time crate left in the dependency tree:** the throw-away certificate of the HTTPS tests came from a certificate-generating crate that pulled in `time`, which has a known denial-of-service advisory fixed only in a release that needs a newer Rust than the 1.85.1 of Debian trixie. The test certificate is now built by hand and signed with `ring` (the provider rustls already uses), the crate and everything it brought are gone from `Cargo.lock`, and CI fails if `time` comes back. The binary, the HTTPS send and its switches are unchanged.

## [0.1.7] - A WebSocket that pushes the state, off by default (serve --ws, with --poses and/or --uav-input): the twin pushes the TwinState of the robot and the state of the vehicles to the clients that watch it, instead of being polled.... (and 2 more changes)

- **A WebSocket that pushes the state, off by default (`serve --ws`, with `--poses` and/or `--uav-input`):** the twin pushes the `TwinState` of the robot and the state of the vehicles to the clients that watch it, instead of being polled. RFC 6455 is written in the twin itself (the handshake, the frames, ping, pong and close, the limits) and adds no dependency. It has its own listener (`ws.listen`, loopback unless `ws.allow_remote`), needs the same token as the other routes, refuses a browser `Origin` that is not in `ws.allowed_origins`, takes at most `ws.max_clients` sessions, drops a client that stays silent, closes a session at `ws.max_session_s` and limits what a client may send. The sentences are in the eight languages. Every number is in the configuration file.

- **The HTTPS send to the data lake (`drift.datalake.url` with `https://`), over `--drift-datalake`:** TLS with rustls; it trusts only the certificate (or authority) in `drift.datalake.ca_file`, valid for the host or for `drift.datalake.server_name`, with no system store and no way to skip the check. An `http://` address stays plain and loopback-only unless `drift.datalake.allow_remote` says otherwise.

- **An input for the state of a vehicle, off by default (`serve --uav-input`):** `POST /twin/uav` takes the `TwinState` that HYDRA-UMC-BRIDGE-UAV builds (position in metres from home and a unit quaternion), checks it against the contract and the limits of `uav.*` (a newer `sequence`, the pose `uav.pose_name` in the frame `base`, a bounded distance, a `simulated` state refused unless `uav.accept_simulated`, at most `uav.max_vehicles` vehicles) and keeps the last state of each vehicle; `GET /twin/uav` lists them with their age and marks a stale one. `GET /metrics` counts the sessions and the states.

## [0.1.6] - The build test of the repository compiles only on CI or when allowed: for the stacks that compile (rust, go, android, flutter, firmware-c), tools/buildtest.py prints BUILDTEST=SKIPPED with exit status 2 on a plain PC, never a... (and 2 more changes)

- **The build test of the repository compiles only on CI or when allowed:** for the stacks that compile (`rust`, `go`, `android`, `flutter`, `firmware-c`), `tools/build_test.py` prints `BUILD_TEST=SKIPPED` with exit status 2 on a plain PC, never a pass; `ALLOW_LOCAL_BUILD=1` allows it and a CI runner always may. The `python` and `node` stacks are not locked.

- **The identity of HYDRA-UMC-SERVER, off by default (`serve --identity-key-file FILE`, or `HYDRA_UMC_TWIN_IDENTITY_KEY_FILE`):** the twin verifies the signed token (HS256 JWT with `exp`, `username` and `role`) that the server issues at login and accepts it next to the certify token. A viewer may read but not change, and a request with no credential is refused even on loopback, `GET /health` and `GET /metrics` included: nothing is open. The routes of the interlock keep asking for the certify token. The key is read from a file, at least 32 characters; without the switch nothing changes.

- **A uniform `GET /health`, closed by default, and the manifest declares it:** `serve --health-open` (or `HYDRA_UMC_TWIN_HEALTH_OPEN=1`) makes the twin answer `GET /health` with `{"status": "ok", "service": ..., "version": ...}`, the same body in every service of the family; without the switch the route answers `404` like a route that does not exist. A caller that is not on loopback must present the certify token (`401` otherwise), and a service without a token never answers it. The body carries no path, configuration or secret, and `service.health_path` of the manifest is `/health` now (the health checks ask `/stats` after a `404`). 2 tests.

## [0.1.5] - A kinematic twin of the robot poses, a signed dry run of a trajectory before it is played and a log of the drift between the real and the simulated robot

- **A minimal kinematic twin, with no graphics engine.** `serve --poses --twin-config FILE` adds `GET /twin/poses`: the pose of every joint frame of the robot, computed from the last reports of its joints with the forward kinematics of HYDRA-UMC-PHYSICS-REPLICA (started as a process; the kinematics are not written twice) and answered as a `TwinState` document that is checked against the contract before it is sent, with the `source` (`real` or `simulated`) and whether each joint was `commanded` or `measured`. A joint the robot needs that was never reported, or not lately (`poses.max_age_ms`), is a `409` that names it and never a zero. Off by default; see `docs/KINEMATIC_TWIN.md`.

- **A dry run of a trajectory with a signed verdict.** `serve --dryrun` adds `POST /dryrun/trajectory`: the samples of a route are checked against the limits of the URDF (`check-trajectory` of the physics replica) and, with `--collision-predict`, against the zones and the robot itself, and the answer is a verdict (`pass`, `fail` or `incomplete`) signed with the Ed25519 key of the twin, bound to the SHA-256 of the exact samples, to the robot and to a time window (`dryrun.validity_ms`). Only `pass` allows anything: a check that could not run, or a collision prediction that is off while the configuration requires it, is `incomplete`, and a child that cannot be run or refuses the input gives an error status and no verdict. Off by default. Not tried with a real robot.

- **A drift monitor between the real robot and the simulated one.** `serve --drift` adds `POST /drift/observe` and `GET /drift`: the difference of a real joint and its simulated position (given, or taken from the newest measured report and from the simulated robot) is classified as `ok`, `warn` or `alarm` with thresholds per joint in the configuration; an alarm needs `drift.alarm_after` observations in a row, so one noisy reading is only a warning. What is not `ok` and every change of level go to a log of bounded size. With `--drift-datalake` (off by default) every observation is also sent to HYDRA-UMC-DATALAKE as a sample of its `POST /ingest`, by a thread of its own that never holds the twin back (a full queue drops and counts); only plain HTTP and, unless `drift.datalake.allow_remote` says otherwise, only a loopback address, with the token read from a file.

- **One configuration file for the new routes** (`serve --twin-config FILE`): the URDF, the freshness of the reports, the limits of a request, the thresholds of the drift, the log and the data lake are all in it, a field it does not know is an error, and the sentences of the answers exist in the eight languages (`?lang=`, `Accept-Language` or `language`). `GET /metrics` counts the dry runs and the drift observations.

## [0.1.4] - A simulated robot and GET /metrics in the Prometheus format

- **A simulated robot and `GET /metrics`.** `serve --sim-config FILE` adds `POST /sim/command`, `POST /sim/step` and `GET /sim/state`: a simple physics model (top speed, top acceleration and travel of each joint; no masses, gravity or contacts) that moves over time, brakes without overshooting, keeps to its travel and is deterministic, and reports its joints to the twin as `measured`, so the hardware-in-the-loop bridge can be run with no robot (`--sim-tick-ms` makes the time follow the clock). `GET /metrics` gives the counters of the service as Prometheus text, with no position, token, key or path in it.

## [0.1.3] - Steadier tests of the health probes

- **Steadier tests of the health probes:** the tests that need a port where nothing answers keep that port taken by a listener that hangs up (a port that was only released can be given to another test of the same run), and the stand-in script of the physics replica gets a limit that start-up time on a loaded machine cannot reach.

## [0.1.2] - Collision prediction, a twin that follows the real joints and a contract for the state of the twin

- **Collision prediction, off by default.** `serve --collision-predict` (with `--collision-urdf`, `--collision-zones`, `--collision-config`, and the signing key and token) adds `POST /predict/collision`: a list of future states is answered with whether it is predicted to enter a zone of the zones file or touch the robot itself, computed by `predict-collision` of HYDRA-UMC-PHYSICS-REPLICA, which over-estimates by construction. The joints a state leaves out come from the last reports of the real joints; a joint nobody knows is a `422`, never zero. The files and the binary come from the configuration, never from the request; at most two predictions run at once (`503`), one that does not end is killed (`504`), and a failure is never read as "clear". Information only: the certificates and `/interlock/*` do not read it, and without the switch the path is an ordinary `POST` path (`405`) and nothing else changes. Not tried with a real robot; see `docs/COLLISION_PREDICTION.md`.

- **The twin follows the real joints.** `POST /interlock/state` takes the report `{joint, position, seq, stamp_ms, source}` that the hardware-in-the-loop bridge sends (`commanded` or `measured`; behind the certify token; a `seq` that is not newer is a `409`, at most 256 joints), and `GET /interlock/state` lists the last report of every joint with its age (`src/joint_state.rs`). `serve --require-fresh-state-ms N` (off by default) makes `POST /interlock/certify` refuse a joint with no report or an old one, so the real mode of the bridge blocks while the twin is blind.

- **The state of the twin has a contract, and the manifests are validated with JSON Schema.** `schemas/twin-state.schema.json` (the SDK's `TwinState`: joint states, named poses, instant, frame and whether the state is `real` or `simulated`) is checked by the new `validate-state` command, which also refuses a pose quaternion that is not of unit length and a joint named twice. The manifest of a child that declares a `schema_version` is validated against the project manifest schema of the SDK with the `jsonschema` crate (Draft 2020-12, formats checked) instead of a hand-written subset; the schemas are vendored under `schemas/`.

## [0.1.1] - The workspace override bounded to the served workspace, the real interface of the hardware-in-the-loop bridge in the compose blueprint, and the universal README

- **`?workspace=` overrides are bounded to the served workspace by default:** a request can no longer make the API probe other directories of the machine; `--strict-workspace-root PATH` picks another root and the new `--allow-any-workspace` brings back the earlier behaviour.
- **The compose blueprint states the real interface of the hardware-in-the-loop bridge:** the service now publishes the HTTP JSON API on `127.0.0.1:8113` (the port and protocol the bridge itself serves and this project's health check asks), not a planned gRPC port 50051.
- **The notes of the project manifest no longer carry a hand-written test count:** the counts went out of date at every change; the number of tests is stated by the README, which is checked against the code.
- **README in the universal layout:** the README and its seven translations follow the layout that the four families share (22 sections in a fixed order), with an architecture picture and a repository-structure picture drawn from `docs/architecture.json` and `docs/structure.json`; the CI validator checks both pictures and the file tree.

- **The layout of the README is checked in the CI:** the validator runs `tools/universal_readme_check.py` and lists, as a warning, what differs from the universal layout of the README.

## [0.1.0] - The server takes requests on several threads and interlock certificates

- **The children are checked by asking them to work, not by reading their manifests.** `family-health` (`src/health.rs`) runs the built binary of HYDRA-UMC-PHYSICS-REPLICA on a two-joint probe URDF and requires the pose of the second joint to be (cos 0.5, sin 0.5, 0) within 1e-6, and calls `GET /stats` of the running HIL bridge, which must answer 200 with its own role. A child that cannot be run or reached (no built binary, nothing listening, no answer within `--timeout-ms`, a child that does not end is killed) is `UNREACHABLE`, one that answers wrongly is `UNHEALTHY`, and SYNTHETIC-DATA-GEN, a Python project with nothing to run, is `NOT PROBED` and never reported as healthy. `serve --health-probes` adds `GET /family-health` (off by default; the binary and the address come from the configuration, never from the request, a `?workspace=` is ignored; answers are cached for `--health-cache-ms`, default 5000). 16 new tests (77 in all: fake children, a service that never answers, a binary that never ends, and the real binary of the sibling checkout when it is built), and run by hand against the real HIL bridge.

- **The server takes requests on several threads** (`serve --workers N`, default 8): one request that is slow, or a client that connects and never finishes its request, held every other client; now it holds one worker. A test keeps a connection stuck and checks the others are answered.

- **Interlock certificates.** `POST /interlock/certify` signs a risk report (joint, position, `collision_imminent`, `distance_m`) with the Ed25519 key of the twin and returns the certificate that HYDRA-UMC-HIL-BRIDGE requires before it forwards a command to real hardware (`src/certificate.rs`, `docs/INTERLOCK_CERTIFICATE.md`). The route exists only with `--signing-key-file` and `--certify-token-file` (or `HYDRA_UMC_TWIN_CERTIFY_TOKEN`, at least 16 characters): without the token nobody can ask for a signature, without a key it answers 404, and one without the other does not start. `keygen --key-file PATH` makes a key, never over an existing file, and prints the public key for the bridge. Each certificate carries a fresh random nonce and the time it was issued. A browser request (an `Origin` header), a body that is not JSON and a body over 4 KiB are refused. Until the physics engine exists the twin signs the report of the caller holding the token. 6 new tests (60 in all) and a vector shared with the bridge.
- **A manifest that is there but wrong is reported as what it is, not as "NOT FOUND"**: `family-status` and `family-sync` read each child's manifest with `read_child_manifest_checked`, which tells a missing checkout (still `NOT FOUND` / `MISSING`) from a file that cannot be read or is not a manifest (malformed JSON, a missing field, a `schema_version` whose major is not 1) and from one that is not the project's own (a manifest of another project copied into the directory, or another `ecosystem`). The first two print `UNUSABLE - <reason>`; the status JSON carries a `problem` for them and the sync JSON an `Unusable { reason }` outcome, and neither counts as ready. A manifest without `ecosystem` or `schema_version` (an older one) is still accepted. 6 new tests (44 in all).
- The manifest declares the health route of the service (`/stats`), so the service registry, the ecosystem status of the server and the watchdog know where to probe it.
- Building no longer changes the version; raising it is the explicit `bump` command (`python tools/hydra_project_tool.py bump`). The build scripts only report the version they built, and CI fails if a build script runs a bump.
- The banner follows the animated style of the ecosystem banners, with the name, the kind and the technologies of the project.
- **The licence declared in the package metadata is GPL-3.0-or-later,** like the headers of the code and the rest of the ecosystem (it said GPL-3.0-only).
- Added `tools/hydra_project_tool.py`: the same `validate`, `test`, `version`, `status` and `bump` commands in every repository. The checks run with `HYDRA_UMC_CI=1` and fail if the version changes; only `bump` changes it.

## [0.0.9]

- Maturity is functional: nothing here has been verified on real hardware yet.
- The READMEs say, in the eight languages, what is planned and what has only been tested against fakes, emulators or fixtures, and carry a short safety and security status note.

## [0.0.8] - Sync contract accepts four-part versions and production maturity

- **Four-component versions are accepted:** the sync contract rejected any child version with four components (`0.8.0.0`) as unparseable, which would have declared the whole family not syncable as soon as one child crossed 0.8.0. Three or four numeric components are now accepted.
- **`production` maturity is recognised:** the highest maturity level a child can declare was refused as unrecognised; it is now accepted and ranks above `established`.
- **`?workspace=` parsing:** a bare parameter before `workspace` (`?x&workspace=...`) silently made the request use the default workspace, and the value was never percent-decoded. Parameters without `=` are skipped and values are decoded.
- **HTTP methods:** anything other than `GET` now answers `405` instead of `404`.
- **Service hardening:** the systemd unit now starts the API with `--strict-workspace-root` bounded to its own workspace.
- **Tests:** the HTTP tests remove the temporary directories they create; `Cargo.lock` is now tracked for reproducible builds (38 tests).
- **Docs:** the README (all languages), badges, crate description and compose blueprint no longer present a Bevy/physics engine as existing: today the binary reads the children's manifests and checks sync readiness. Sample versions and the published port are corrected.

## [0.0.7] - Opt-in --strict-workspace-root, plus strict child manifest versions

- **`--strict-workspace-root PATH`** (new, opt-in) - unset, `?workspace=`
  keeps pointing at any real, existing directory the process can read
  (the deliberate, tested behavior above, unchanged). Set, every
  override must resolve inside `PATH` or the request gets a `400` naming
  exactly that - a real production-hardening option for a deployment
  that wants this loopback-only API bounded to one real checkout root,
  without removing the override's own dev/test flexibility as the
  default. A `PATH` that doesn't exist is a loud startup error, not a
  silent always-reject. 3 new tests.
- **`server.rs`'s `?workspace=` override is now validated** (new
  `resolve_workspace_override()`) - found on code review: it used to go straight to the filesystem
  reader with no validation at all, so a typo'd or bogus path silently
  produced the exact same response as a real, empty family
  (`"allPresent": false`) - no way for an operator to tell "you gave me
  a bad path" from "the family genuinely isn't checked out here".
  Pointing this loopback-only internal API at a whole other real
  checkout root stays a deliberate, tested feature - this closes the gap
  by canonicalizing the override (resolving `..`/symlinks) and requiring
  it to actually exist and be a real directory, returning a clear 400
  for anything else instead of a misleading "everything missing". New
  regression tests cover both a nonexistent path and one that's a file,
  not a directory.
- The state-sync gate now requires a complete numeric `MAJOR.MINOR.PATCH`
  child version before it assesses compatibility. Incomplete, extra-component
  or non-numeric versions fail as unparseable instead of being accepted from
  their first component alone.

---

## [0.0.6] - No more personal absolute paths in public docs

- `docs/CLI_REFERENCE.md`'s own real captured CLI/HTTP output examples
  named this machine's real local checkout path outright
  (`C:\Users\juane\Documents\GitHub`) - replaced with `<workspace-root>`
  throughout, keeping every example's actual output format unchanged.

## [0.0.5]

- **Fixed CI**: `cargo fmt --check` was failing on `src/main.rs`/
  `src/server.rs` (unwrapped lines over the 100-column limit), and
  `cargo clippy -- -D warnings` was failing on `std::io::Error::new(
  ErrorKind::Other, e)` (now `std::io::Error::other`, clippy's own
  suggested idiom). No behavior change - `cargo test`: 29/29 passing
  throughout.

## [0.0.4] - Real v0: JSON/HTTP server mode, plus CM5 deployment

- **`server.rs`** (new) - `GET /family-status` and `GET /family-sync`
  reach the exact same `check_family_status()`/`assess_family_sync()`
  functions the CLI's own subcommands already run, over a real
  `tiny_http` server (blocking, no async runtime - the closest Rust
  equivalent to this ecosystem's other services' stdlib
  `ThreadingHTTPServer`, first used in this ecosystem here). Real gap
  this closes: this project's own readiness/sync-contract check was only
  ever reachable as a one-shot CLI.
- **`ChildManifest`/`ChildStatus`/`FamilySyncOutcome`/`FamilySyncStatus`/
  `SyncReadiness`/`SyncSnapshot`** (manifest.rs, family.rs, contract.rs)
  - all gained a `Serialize` derive (behavior-preserving, additive only)
  so `server.rs` can hand them straight to `serde_json::json!` without a
  second, parallel JSON shape to keep in sync with the CLI's own
  human-readable printer.
- **`main.rs`** - new `serve` subcommand (`--addr`/`--port`/
  `--workspace`, default `127.0.0.1:8111`).
- **`systemd/hydra-umc-twin.service`** (new) - unit for
  `HYDRA-UMC-OS/provisioning/install_twin.sh` (new, that repo) - first
  Rust service installed on this CM5 as a compiled release binary rather
  than built on-device (unlike the Go services). `--workspace` points at
  a real `root:root 0755` tree holding just this node's real children's
  own `hydra-umc.project.json` files - same real permission lesson as
  `HYDRA-UMC-VISION-NODE`'s own install (a symlink to the real sibling-
  checkout root would be unreadable by this service's own unprivileged
  account, since that root lives under the operator's home directory).
- 7 new tests (`server.rs`'s own `#[cfg(test)]` module, real end-to-end
  HTTP over a raw `TcpStream` since no HTTP client crate is a dependency
  here) - 29 total.

## [0.0.3] - Real v0: state-sync contract with incompatible-version rejection

- **`src/contract.rs`** (new) - the real state-sync contract this Twin enforces before treating a child as sync-ready: `MaturityLevel` (ordered `Scaffolding < Functional < Established`) gates on `MIN_MATURITY = Functional`, and a per-child major version is gated against `MAX_COMPATIBLE_MAJOR` (0 today, every child is still pre-1.0) - a child reporting a higher major is refused as an incompatible simulator version, since its own sync contract may have changed in a way this Twin hasn't verified. `assess()` checks maturity before version so the reported rejection reason always names the most fundamental gate that failed. A child that clears both gates gets a real `SyncSnapshot` fixture (child/version/maturity) - a concrete, typed value for a future sync transport to serialize, not a loose blob.
- **`src/family.rs`** - new `FamilySyncStatus`/`FamilySyncOutcome`/`assess_family_sync()`: combines `check_family_status()`'s presence check with `contract::assess()` per child, so "missing", "immature", "incompatible version", and "ready" are each their own distinct, real outcome.
- **`main.rs`** - new `family-sync [--workspace PATH]` subcommand prints every child's real sync outcome and exits `0` only if all are `READY`, `1` otherwise.
- 12 new tests (7 in `contract.rs` covering both gates and their boundaries - maturity exactly at the minimum, major version exactly at the compatibility ceiling and one past it, maturity checked before version; 3 new integration tests in `family.rs` covering missing/ready/rejected through the real combined path). 21 tests total.
- Real verification beyond the test suite: ran the compiled binary against a real temp workspace with a compatible child (`READY`), an incompatible major version (`REJECTED (incompatible version)`), a missing child (`MISSING`), and a `scaffolding` child (`REJECTED (immature)`) - all 4 outcomes confirmed live.

## [0.0.2] - Real v0 family-readiness check
- `manifest.rs` - a real, defensive reader for a sibling repo's own `hydra-umc.project.json` (via `serde`/`serde_json`, this project's only dependencies), returning `None` for every real failure mode (missing checkout, missing file, malformed JSON, missing field) instead of panicking.
- `family.rs` - `check_family_status()`: a real check of this hub's 3 real children (`HYDRA-UMC-PHYSICS-REPLICA`/`HYDRA-UMC-HIL-BRIDGE`/`HYDRA-UMC-SYNTHETIC-DATA-GEN`) against a real local workspace, reading each one's own manifest rather than a second hand-maintained list.
- `main.rs` - new `family-status [--workspace PATH]` subcommand, defaulting to this repo's own parent directory (the real sibling-checkout layout this whole ecosystem already uses). Bare invocation unchanged.
- 9 new real tests (`manifest.rs`/`family.rs`) - manifest reading for every real failure mode, family-status coverage for all-present/some-missing/none-present, and real maturity reporting.
- Real verification beyond the test suite: ran `family-status` against the actual local ecosystem checkout on this machine - correctly reported `HYDRA-UMC-PHYSICS-REPLICA` and `HYDRA-UMC-SYNTHETIC-DATA-GEN` as `functional` and `HYDRA-UMC-HIL-BRIDGE` as still `scaffolding`, matching their real, independently-verified state.

- `build.sh` called `bump_manifest_version.py` (no `--sync`) as its very first line, before also calling `bump_version.py` later, while `build.bat` already had the correct order - the same double-bump inconsistency found in other projects this session. Rewritten to bump the native version first, then sync the manifest. `build.sh`/`build.bat` now also run `cargo test` and use the ecosystem's no-autoclose pattern for the first time in this project; `run.sh`/`run.bat` now forward arguments.

## [0.0.1] - Initial scaffolding

- **`src/main.rs`** - minimal real entry point. No engine logic yet - the Bevy-based physics/rendering digital twin (the same renderer HYDRA-UMC-SYNTHETIC-DATA-GEN integrates with) lands in a later pass.
- **`Cargo.toml`** - crate metadata, no runtime dependencies yet.
- **`build.sh` / `build.bat`**, **`run.sh` / `run.bat`** - `cargo build --release` and run the resulting binary.
