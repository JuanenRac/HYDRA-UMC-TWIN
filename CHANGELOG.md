# Changelog

All notable work on **HYDRA-UMC-TWIN** is summarized here, newest first. Full
session-by-session detail (including dates) lives in a private,
unpublished internal log - this file is public, so it intentionally
omits calendar dates.

## Versioning scheme

`Cargo.toml`'s `version` field is bumped automatically by `bump_version.py`
(stdlib-only, no `cargo` plugin needed) before a real release build
(`cargo build --release`), invoked from `build.sh`/`build.bat`.

It follows the ecosystem-wide base-10 "odometer" rule rather than
semantic-versioning judgment calls:

- `PATCH` +1 on every build
- when `PATCH` would exceed 9, it resets to 0 and `MINOR` +1 instead (e.g. `0.0.9` -> `0.1.0`, never `0.0.10`)
- the same carry cascades into `MAJOR` if `MINOR` would exceed 9

---

## Unreleased - strict child manifest versions

- The state-sync gate now requires a complete numeric `MAJOR.MINOR.PATCH`
  child version before it assesses compatibility. Incomplete, extra-component
  or non-numeric versions fail as unparseable instead of being accepted from
  their first component alone.

---

## [0.0.3] - Real v0: state-sync contract with incompatible-version rejection

- **`src/contract.rs`** (new) - the real state-sync contract this Twin enforces before treating a child as sync-ready: `MaturityLevel` (ordered `Scaffolding < Functional < Established`) gates on `MIN_MATURITY = Functional`, and a per-child major version is gated against `MAX_COMPATIBLE_MAJOR` (0 today, every child is still pre-1.0) - a child reporting a higher major is refused as an incompatible simulator version, since its own sync contract may have changed in a way this Twin hasn't verified. `assess()` checks maturity before version so the reported rejection reason always names the most fundamental gate that failed. A child that clears both gates gets a real `SyncSnapshot` fixture (child/version/maturity) - a concrete, typed value for a future sync transport to serialize, not a loose blob.
- **`src/family.rs`** - new `FamilySyncStatus`/`FamilySyncOutcome`/`assess_family_sync()`: combines `check_family_status()`'s presence check with `contract::assess()` per child, so "missing", "immature", "incompatible version", and "ready" are each their own distinct, real outcome.
- **`main.rs`** - new `family-sync [--workspace PATH]` subcommand prints every child's real sync outcome and exits `0` only if all are `READY`, `1` otherwise.
- 12 new tests (7 in `contract.rs` covering both gates and their boundaries - maturity exactly at the minimum, major version exactly at the compatibility ceiling and one past it, maturity checked before version; 3 new integration tests in `family.rs` covering missing/ready/rejected through the real combined path). 21 tests total.
- Real verification beyond the test suite: ran the compiled binary against a real temp workspace with a compatible child (`READY`), an incompatible major version (`REJECTED (incompatible version)`), a missing child (`MISSING`), and a `scaffolding` child (`REJECTED (immature)`) - all 4 outcomes confirmed live.

## [0.0.2] - Real v0 family-readiness check
### Added
- `manifest.rs` - a real, defensive reader for a sibling repo's own `hydra-umc.project.json` (via `serde`/`serde_json`, this project's only dependencies), returning `None` for every real failure mode (missing checkout, missing file, malformed JSON, missing field) instead of panicking.
- `family.rs` - `check_family_status()`: a real check of this hub's 3 real children (`HYDRA-UMC-PHYSICS-REPLICA`/`HYDRA-UMC-HIL-BRIDGE`/`HYDRA-UMC-SYNTHETIC-DATA-GEN`) against a real local workspace, reading each one's own manifest rather than a second hand-maintained list.
- `main.rs` - new `family-status [--workspace PATH]` subcommand, defaulting to this repo's own parent directory (the real sibling-checkout layout this whole ecosystem already uses). Bare invocation unchanged.
- 9 new real tests (`manifest.rs`/`family.rs`) - manifest reading for every real failure mode, family-status coverage for all-present/some-missing/none-present, and real maturity reporting.
- Real verification beyond the test suite: ran `family-status` against the actual local ecosystem checkout on this machine - correctly reported `HYDRA-UMC-PHYSICS-REPLICA` and `HYDRA-UMC-SYNTHETIC-DATA-GEN` as `functional` and `HYDRA-UMC-HIL-BRIDGE` as still `scaffolding`, matching their real, independently-verified state.

### Fixed
- `build.sh` called `bump_manifest_version.py` (no `--sync`) as its very first line, before also calling `bump_version.py` later, while `build.bat` already had the correct order - the same double-bump inconsistency found in other projects this session. Rewritten to bump the native version first, then sync the manifest. `build.sh`/`build.bat` now also run `cargo test` and use the ecosystem's no-autoclose pattern for the first time in this project; `run.sh`/`run.bat` now forward arguments.

## [0.0.1] - Initial scaffolding

- **`src/main.rs`** - minimal real entry point. No engine logic yet - the Bevy-based physics/rendering digital twin (the same renderer HYDRA-UMC-SYNTHETIC-DATA-GEN integrates with) lands in a later pass.
- **`Cargo.toml`** - crate metadata, no runtime dependencies yet.
- **`build.sh` / `build.bat`**, **`run.sh` / `run.bat`** - `cargo build --release` and run the resulting binary.
