# Changelog

All notable changes to this project are documented here.

## [Unreleased]

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
