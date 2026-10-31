# HYDRA-UMC-TWIN — CLI Reference

`hydra-umc-twin` is a single Rust binary (`src/main.rs`). It is the Digital
Twin's integration-parent role, not a physics/rendering engine itself yet:
`family-status`/`family-sync` run this project's actual v0 readiness checks
against its 3 real children (`src/family.rs`), reading each child's own
`hydra-umc.project.json` manifest rather than a second hand-maintained list.
Every example below was captured from a real, built release binary, run
against a real sibling-checkout workspace (`<workspace-root>` below
stands for that real absolute path, not a placeholder value the binary
itself prints) — the output shown is real, not illustrative.

## Usage

```
$ ./run.sh family-status
$ ./run.sh family-sync
```

`run.sh` execs `target/release/hydra-umc-twin` and forwards all arguments
unchanged. The examples below invoke the release binary directly, which is
equivalent.

Bare invocation (no arguments) prints identity/version/role and exits `0`:

```
$ hydra-umc-twin
HYDRA-UMC-TWIN v0.0.8
Digital Twin integration hub: family readiness and state-sync contract for PHYSICS-REPLICA, HIL-BRIDGE and SYNTHETIC-DATA-GEN (physics engine planned).
```

## Workspace resolution

Both real subcommands accept `--workspace PATH`; without it, the default is
this binary's own parent directory — the real sibling-checkout layout this
whole ecosystem uses (every repo is a sibling folder under one workspace
root), since `run.sh` `cd`s into this repo's own directory first.

## The 3 expected children

`src/family.rs`'s `EXPECTED_CHILDREN` names exactly 3 real children:
`HYDRA-UMC-PHYSICS-REPLICA`, `HYDRA-UMC-HIL-BRIDGE`, and
`HYDRA-UMC-SYNTHETIC-DATA-GEN`.

## Commands

### `family-status [--workspace PATH]`

For each expected child, reports whether it's checked out and, if so, its
real reported version/maturity/role — read live from that child's own
`hydra-umc.project.json`.

**All 3 children present** (this machine's real workspace; exit `0`):

```
$ hydra-umc-twin family-status
Digital Twin family status (workspace: <workspace-root>):
  HYDRA-UMC-PHYSICS-REPLICA: v0.0.6, maturity=functional, role=library
  HYDRA-UMC-HIL-BRIDGE: v0.0.8, maturity=functional, role=service
  HYDRA-UMC-SYNTHETIC-DATA-GEN: v0.0.9, maturity=established, role=tool

All 3 children present.
```

**No children checked out** — a real empty `--workspace` directory (exit
`1`):

```
$ hydra-umc-twin family-status --workspace /empty/workspace
Digital Twin family status (workspace: /empty/workspace):
  HYDRA-UMC-PHYSICS-REPLICA: NOT FOUND (expected at /empty/workspace/HYDRA-UMC-PHYSICS-REPLICA)
  HYDRA-UMC-HIL-BRIDGE: NOT FOUND (expected at /empty/workspace/HYDRA-UMC-HIL-BRIDGE)
  HYDRA-UMC-SYNTHETIC-DATA-GEN: NOT FOUND (expected at /empty/workspace/HYDRA-UMC-SYNTHETIC-DATA-GEN)

Some children are missing - see NOT FOUND lines above.
```

### `family-sync [--workspace PATH]`

The next real question beyond `family-status`: for each checked-out child,
would this Twin actually trust its manifest enough to sync state with it —
the real `contract.rs` gate (`src/contract.rs`). Two gates, both required:
maturity must be at least `functional`, and the child's reported major
version must not exceed `0` (`MAX_COMPATIBLE_MAJOR`, since every child in
this ecosystem is still pre-1.0).

**All 3 children sync-ready** (this machine's real workspace; exit `0`):

```
$ hydra-umc-twin family-sync
Digital Twin family sync contract (workspace: <workspace-root>):
  HYDRA-UMC-PHYSICS-REPLICA: READY (v0.0.6, maturity=functional)
  HYDRA-UMC-HIL-BRIDGE: READY (v0.0.8, maturity=functional)
  HYDRA-UMC-SYNTHETIC-DATA-GEN: READY (v0.0.9, maturity=established)

All 3 children are sync-ready.
```

**No children checked out** — `family-sync` reports `MISSING`, a distinct
outcome from a checked-out-but-rejected child (exit `1`):

```
$ hydra-umc-twin family-sync --workspace /empty/workspace
Digital Twin family sync contract (workspace: /empty/workspace):
  HYDRA-UMC-PHYSICS-REPLICA: MISSING (not checked out)
  HYDRA-UMC-HIL-BRIDGE: MISSING (not checked out)
  HYDRA-UMC-SYNTHETIC-DATA-GEN: MISSING (not checked out)

Not every child is sync-ready - see the lines above.
```

A child that *is* checked out but fails a contract gate is reported
differently — `REJECTED (immature) - ...`, `REJECTED (incompatible
version) - ...`, or `REJECTED (unparseable manifest) - ...` — naming the
specific gate that failed (see `src/contract.rs`'s `assess()`); this
machine's real siblings are all at least `functional` at major version `0`, so none
of those rejection paths could be captured from a real run today.

### `family-health [--workspace PATH] [--physics-replica-bin PATH] [--hil-bridge-addr HOST:PORT] [--timeout-ms N]`

The active check: instead of reading what each child says about itself it asks the child to do something and checks the answer.

- **HYDRA-UMC-PHYSICS-REPLICA** — its built binary (`--physics-replica-bin`, or `HYDRA_UMC_TWIN_PHYSICS_REPLICA_BIN`, or the one under `target/release` or `target/debug` of the child in the workspace) is run as `fk --urdf <probe> --joints turn=0.5 --json` on a two-joint probe URDF; the pose of the second joint must be (cos 0.5, sin 0.5, 0) within 1e-6.
- **HYDRA-UMC-HIL-BRIDGE** — `GET /stats` at `--hil-bridge-addr` (default `127.0.0.1:8113`, or `HYDRA_UMC_TWIN_HIL_ADDR`) must answer `200` with a JSON body whose `role` is the bridge's own.
- **HYDRA-UMC-SYNTHETIC-DATA-GEN** — a Python project with no service or binary to run: `NOT PROBED`, never `HEALTHY`.

Each line is `HEALTHY`, `UNHEALTHY` (it ran or answered, and the answer was wrong), `UNREACHABLE` (no binary, nothing listening, no answer within `--timeout-ms`, 100 to 60000, default 5000; a child that does not end is killed) or `NOT PROBED`. Exit `0` when no probed child is unhealthy or unreachable, `1` otherwise, `2` for a bad option.

## `serve [--addr ADDR] [--port PORT] [--workspace PATH]`

Starts a real, blocking HTTP JSON server (`src/server.rs`, `tiny_http`, no
async runtime) that exposes the exact same `check_family_status()`/
`assess_family_sync()` logic the `family-status`/`family-sync`
subcommands run above — closes the "only reachable as a one-shot CLI
invocation" gap. `ADDR` defaults to `127.0.0.1`, `PORT` to `8111`,
`WORKSPACE` to this repo's own parent directory (same default as the
other two subcommands). This is the same binary
`systemd/hydra-umc-twin.service` runs on a deployed CM5
(`hydra-umc-twin serve --addr 127.0.0.1 --port 8111 --workspace /opt/hydra-umc/twin/workspace`).

```
$ hydra-umc-twin serve --addr 127.0.0.1 --port 8111
[twin] HTTP API listening on 127.0.0.1:8111 (workspace=<workspace-root>)
[twin] GET /family-status, GET /family-sync, GET /stats
```

**`GET /family-status`** and **`GET /family-sync`** — same real check as
the CLI subcommands, as JSON; both accept an optional `?workspace=` query
parameter that overrides the server's own default for that one request:

```bash
$ curl http://127.0.0.1:8111/family-status
{"allPresent":true,"children":[{"manifest":{"maturity":"functional","name":"HYDRA-UMC-PHYSICS-REPLICA","role":"library","version":"0.0.3"},"name":"HYDRA-UMC-PHYSICS-REPLICA"},...],"workspace":"<workspace-root>"}
```

Both always answer HTTP `200` for a well-formed `GET` — a child being
missing or contract-rejected is real data in the response body
(`allPresent`/`allReady: false`), not an HTTP-level error.

**`GET /stats`** — `{"workspace": "<the server's own default workspace>"}`, `200`.

**`GET /family-health`** (only with `serve --health-probes`, or `HYDRA_UMC_TWIN_HEALTH_PROBES=1`; otherwise `404`) — the report of `family-health` as `{"workspace", "children": [{"name", "probe": {"state": "healthy|unhealthy|unreachable|not_probed", ...}}], "allHealthy", "cached"}`. It runs a binary and calls a service, so the binary and the address are the ones of the configuration (`--physics-replica-bin`, `--hil-bridge-addr`, `--timeout-ms`), a `?workspace=` of the request is ignored, and the answer is cached for `--health-cache-ms` (default 5000) so a client that asks in a loop cannot make the server run a process in a loop. `allHealthy` is true when no probed child failed.

Any other path is `404`, and any non-`GET` request (but the certify route) is `405`.

`serve --workers N` (default 8, from 1 to 256) is how many requests are served at once: a request that is slow (a
`?workspace=` on a network file system that does not answer, a client that sends half a request and stays) holds
one worker, not the whole server.

There is no
authentication — same as every other loopback-only internal API on the
CM5 (see the systemd unit's own hardening for what it relies on instead).

### `--strict-workspace-root PATH` and `--allow-any-workspace`

By default every `?workspace=` override must resolve inside the served
workspace (the one `--workspace` sets, the root itself or a real
subdirectory of it) or the request gets a `400` naming exactly that, so the
answers cannot be used to probe which directories exist elsewhere on the
machine. `--strict-workspace-root PATH` bounds the overrides to `PATH`
instead; a `PATH` that doesn't exist is a real, loud startup error (exit
`2`), not a silent always-reject once the server is already listening.
`--allow-any-workspace` turns the check off: an override can then point at
any real, existing directory the process can read (for comparing two
working trees during development). The two flags cannot be combined. The
server's own default workspace is never affected, only what a
caller-supplied override is allowed to point at.

```bash
$ hydra-umc-twin serve --workspace /opt/hydra-umc/twin/workspace --strict-workspace-root /opt/hydra-umc/twin/workspace
[twin] --strict-workspace-root set: ?workspace= overrides are bounded to /opt/hydra-umc/twin/workspace
```

## `validate-state FILE`

Checks a twin state (the `TwinState` contract of the SDK) against its JSON Schema: `source` (`real` or `simulated`, so that a reading of the
robot and one of a simulation are never taken for each other), `frame`, `timestamp_utc`, the `joint_states` and the optional named `poses`. Besides
the schema it refuses a pose whose quaternion is not of length 1 and a joint named twice. It prints each problem and `TWIN_STATE=PASS` or
`TWIN_STATE=FAIL`; the exit status is `0` (valid), `1` (invalid) or `2` (the file cannot be read or is not JSON).

```
$ hydra-umc-twin validate-state state.json
TWIN_STATE=PASS source=simulated
```

The manifests of the children are validated the same way: a `hydra-umc.project.json` that declares a `schema_version` must match the project
manifest schema of the SDK as a whole (the schemas are vendored under `schemas/`), and `family-status` / `family-sync` report the violations as
`UNUSABLE`. A manifest without `schema_version` keeps the three fields this Twin has always needed.

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | ok — `family-status`: every expected child is present; `family-sync`: every expected child is sync-ready |
| `1` | `family-status`: at least one expected child is missing; `family-sync`: at least one expected child is missing or rejected by the sync contract |
| `2` | `serve`: the HTTP server failed to bind the requested address/port, or an option is invalid |

`family-health` exits `0` when no probed child is unhealthy or unreachable and `1` otherwise.

## Not yet wired in

`family-status`/`family-sync` are real local manifest checks, not a live
network sync — there is no real Bevy engine or physics backend, and no
actual state-sync transport between this Twin and its children yet (see
`src/family.rs`'s and `src/contract.rs`'s own module docs). `SyncSnapshot`
is a real, typed fixture representing what a future sync transport would
exchange, not live simulation tick data.

## `keygen --key-file PATH` and `POST /interlock/certify`

`keygen` writes a new Ed25519 signing key (one line of 64 hexadecimal characters, mode 0600 on Unix) and prints its key id and public key. It never
overwrites a file. `serve --signing-key-file PATH --certify-token-file PATH` (or `HYDRA_UMC_TWIN_SIGNING_KEY_FILE` and `HYDRA_UMC_TWIN_CERTIFY_TOKEN`) turns on
`POST /interlock/certify`; see `docs/INTERLOCK_CERTIFICATE.md` for the format, the vector and what the bridge checks.

## `POST /interlock/state`, `GET /interlock/state` and `--require-fresh-state-ms`

The twin keeps the newest report of every real joint that HYDRA-UMC-HIL-BRIDGE sends it. Both routes use the token of `--certify-token-file` and exist only when
the signing is configured (otherwise `404`).

```json
{"joint":"shoulder","position":0.5,"seq":7,"stamp_ms":1700000000000,"source":"measured"}
```

`source` is `commanded` (what the bridge ordered) or `measured` (what the controller read back). `seq` must be newer than the last one of that joint (`409` with `last_seq`
otherwise, so a late or replayed message cannot move the twin back); an empty or over-long joint name or a position that is not finite is a `400`; the twin follows at most
256 joints; a body is at most 2 KiB; a request with an `Origin` header or not declared as JSON is refused as on `/interlock/certify`. `GET /interlock/state` answers the
last report of every joint with its `age_ms` counted from its arrival, and `requireFreshStateMs`.

`serve --require-fresh-state-ms N` (1 to 3600000; off by default) makes `POST /interlock/certify` answer `409` for a joint with no report or whose last report is older
than N ms, so the real mode of the bridge blocks while the twin is blind. It needs the signing key and the token.
