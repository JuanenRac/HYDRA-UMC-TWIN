# HYDRA-UMC-TWIN — CLI Reference

`hydra-umc-twin` is a single Rust binary (`src/main.rs`). It is the Digital
Twin's integration-parent role, not a physics/rendering engine itself yet:
`family-status`/`family-sync` run this project's actual v0 readiness checks
against its 3 real children (`src/family.rs`), reading each child's own
`hydra-umc.project.json` manifest rather than a second hand-maintained list.
Every example below was captured from a real, built release binary, run
against this machine's real sibling checkouts under
`C:\Users\juane\Documents\GitHub` — the output shown is real, not
illustrative.

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
HYDRA-UMC-TWIN v0.0.3
Physics-based Digital Twin engine for safe robotic simulation (integration parent).
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
Digital Twin family status (workspace: C:\Users\juane\Documents\GitHub):
  HYDRA-UMC-PHYSICS-REPLICA: v0.0.3, maturity=established, role=library
  HYDRA-UMC-HIL-BRIDGE: v0.0.3, maturity=established, role=service
  HYDRA-UMC-SYNTHETIC-DATA-GEN: v0.0.5, maturity=established, role=tool

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
Digital Twin family sync contract (workspace: C:\Users\juane\Documents\GitHub):
  HYDRA-UMC-PHYSICS-REPLICA: READY (v0.0.3, maturity=established)
  HYDRA-UMC-HIL-BRIDGE: READY (v0.0.3, maturity=established)
  HYDRA-UMC-SYNTHETIC-DATA-GEN: READY (v0.0.5, maturity=established)

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
machine's real siblings are all `established` at major version `0`, so none
of those rejection paths could be captured from a real run today.

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | ok — `family-status`: every expected child is present; `family-sync`: every expected child is sync-ready |
| `1` | `family-status`: at least one expected child is missing; `family-sync`: at least one expected child is missing or rejected by the sync contract |

## Not yet wired in

`family-status`/`family-sync` are real local manifest checks, not a live
network sync — there is no real Bevy engine or physics backend, and no
actual state-sync transport between this Twin and its children yet (see
`src/family.rs`'s and `src/contract.rs`'s own module docs). `SyncSnapshot`
is a real, typed fixture representing what a future sync transport would
exchange, not live simulation tick data.
