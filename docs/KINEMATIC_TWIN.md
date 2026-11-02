# The kinematic twin: poses, dry run of a trajectory and drift

Three routes that give the twin something useful without a graphics engine or a GPU. They reuse HYDRA-UMC-PHYSICS-REPLICA (forward
kinematics, the limits of the URDF, the prediction of collisions) as a process, so the kinematics are never written twice. **Not tried with a
real robot**: they work with the joint reports of the bridge and with the simulated robot of `--sim-config`.

Each route is **off by default** and has its own switch. All of them need the signing key and the token of the interlock routes
(`--signing-key-file`, `--certify-token-file`), because they show where the robot is or sign something a bridge relies on, and a
`--twin-config FILE` that holds every number (nothing tuned is in the code). A switch without the file, the file without a switch, or a
route without the token stops the start.

| flag | route |
|------|-------|
| `--twin-config FILE` | the configuration (below) |
| `--poses` | `GET /twin/poses` |
| `--dryrun` | `POST /dryrun/trajectory` |
| `--drift` | `POST /drift/observe`, `GET /drift` |
| `--drift-datalake` | also send every observation of the drift to HYDRA-UMC-DATALAKE (needs `--drift`) |
| `--physics-replica-bin PATH` | the binary of PHYSICS-REPLICA (also `HYDRA_UMC_TWIN_PHYSICS_REPLICA_BIN`; else the one built next to the workspace) |

The language of the sentences (`message`) is chosen with `?lang=xx`, `Accept-Language` or `language` in the file: `en`, `es`, `de`, `fr`, `it`,
`ja`, `zh` and `pt` (Portugal). What a program reads (`code`, `verdict`, `level`) is always English.

## The configuration file

Every field is optional and an unknown field is an error. The paths are relative to the file.

```json
{
  "robot_id": "arm-1",
  "urdf": "robot.urdf",
  "physics_config": "physics.json",
  "language": "en",
  "poses":  { "joints": ["shoulder", "elbow"], "max_age_ms": 2000, "timeout_ms": 5000 },
  "dryrun": { "timeout_ms": 10000, "max_samples": 512, "validity_ms": 5000,
              "require_collision_check": true, "fail_on_warning": true },
  "drift":  { "warn": 0.02, "alarm": 0.05, "alarm_after": 3, "max_age_ms": 2000,
              "per_joint": { "wrist": { "warn": 0.05, "alarm": 0.1 } },
              "log_file": "drift.jsonl", "max_log_bytes": 5242880,
              "datalake": { "url": "http://127.0.0.1:8095", "source_id": "twin-arm-1", "kind": "twin_drift",
                            "token_file": "datalake.token", "queue": 256, "timeout_ms": 2000, "allow_remote": false } }
}
```

`physics_config` is the file of `check-trajectory` of PHYSICS-REPLICA (the one of its `serve --config`), where the acceleration limits and
the tolerances live. `poses.joints` lists the joints the robot needs (empty: every joint that was reported).

## `GET /twin/poses`

Answers a document of the `TwinState` contract of the SDK (`schemas/twin-state.schema.json`, checked before it is sent): the joints with
their position, whether it was `commanded` or `measured` and its age, and the pose of every joint frame, in metres and as a unit
quaternion, in the base frame. `source` is `simulated` when the server runs the simulated robot and `real` otherwise.

A joint the robot needs and the twin has not heard of, or has not heard of for `poses.max_age_ms`, is a `409` that names it (`missing`,
`stale`); the pose is never computed with a zero in its place. Other answers: `422` (the kinematics refused the positions), `502` (they
could not be run), `503` (two are running), `504` (they did not end in time). The positions of the last computation are remembered, so a
client that polls while the robot stands still starts no process.

Studios read it by polling. A push over a WebSocket is not offered: the server has no WebSocket layer and adding one is an open decision.

## `POST /dryrun/trajectory`

```
POST /dryrun/trajectory
Authorization: Bearer <certify token>
Content-Type: application/json

{"samples": [{"t": 0.0, "joints": {"shoulder": 0.0}}, {"t": 0.1, "joints": {"shoulder": 0.05}}]}
```

The samples are the format of `check-trajectory` of PHYSICS-REPLICA (`t` strictly increasing, at least two, at most `dryrun.max_samples`).
The twin runs `check-trajectory` (positions, speeds and accelerations against the limits of the URDF) and, when the server was started with
`--collision-predict`, `predict-collision` on the same samples (the joints a sample leaves out come from the last reports). The answer is a
**signed verdict**:

```json
{"verdict": "pass", "message": "...", "robot_id": "arm-1", "samples": 2,
 "trajectory_sha256": "...", "issued_at_ms": 1, "valid_until_ms": 5001, "nonce": "...32 hex...",
 "key_id": "...", "signature": "...64 bytes in hex...",
 "checks": {"trajectory": {"ok": true, "complete": true}, "collision": {"checked": true, "collision": false, "level": "clear"}}}
```

* `pass`: every check ran, every limit holds and nothing was predicted to touch a zone or the robot. **Only `pass` allows anything.**
* `fail`: a limit is broken, or a contact is predicted (a `warning` level also fails while `dryrun.fail_on_warning` is true).
* `incomplete`: a check could not run or was switched off (a limit that the URDF lacks, no collision prediction while
  `dryrun.require_collision_check` is true): the trajectory is **not** declared safe.

The signature is Ed25519 over this text, with a `\n` between the lines:

```
hydra-umc-dryrun-verdict-v1
<key_id>
<robot_id>
<trajectory_sha256>
<verdict>
<issued_at_ms>
<valid_until_ms>
<nonce>
```

`trajectory_sha256` is the SHA-256 of the line `hydra-umc-trajectory-v1` followed by one line per sample, `t=<t>` and then ` <joint>=<position>`
for each joint in name order, every number in the shortest form that reads back to the same value (the `{:?}` of Rust, `0.1` and `1e-7`); each line ends with a line feed. A bridge that is about to send
a trajectory builds the same text and compares the digest, so a verdict is bound to the exact samples, to the robot and to its time window.

A child that cannot be run, does not answer in time or refuses the input is an error (`502`, `504`, `422`) and no verdict is signed. At most two
dry runs run at once (`503`). Requiring this verdict before a command moves real hardware is a decision of the bridge and is not done there yet.
It is a simulation with finite differences and capsules that over-estimate: it does not replace the limits of the controller.

## The drift monitor: `POST /drift/observe`, `GET /drift`

```
POST /drift/observe        {"joint": "shoulder", "real": 0.53, "simulated": 0.50}
```

`real` and `simulated` may be left out: the real position then comes from the newest `measured` report of the twin (at most `drift.max_age_ms`
old, else `409`) and the simulated one from the simulated robot of `--sim-config` (else `409`). The difference is classified with `drift.warn`
and `drift.alarm` (per joint in `drift.per_joint`):

* `ok`: below the warning threshold.
* `warn`: at or above it.
* `alarm`: at or above the alarm threshold in `drift.alarm_after` observations in a row. One noisy reading is only a warning.

`GET /drift` lists every joint with its counters, its level, the largest difference seen and its thresholds. The observations that are not `ok`
and every change of level are appended to `drift.log_file` (JSON Lines; when the next line would pass `drift.max_log_bytes` the file becomes
`<name>.1` and a new one begins).

### Sending to HYDRA-UMC-DATALAKE

Only with `--drift-datalake` and `drift.datalake.url`. Every observation becomes a sample of `POST /ingest` of the data lake:

```json
{"sourceId": "twin-arm-1", "kind": "twin_drift", "timestamp": 1800000000000, "sequence": 7,
 "fields": {"shoulder_error": 0.03, "shoulder_abs_error": 0.03, "shoulder_level": 1}}
```

(`shoulder_level` is 0 for `ok`, 1 for `warn`, 2 for `alarm`.) A thread of its own sends them, so a data lake that is down or slow costs the
twin only a counter: `GET /drift` shows `sent`, `failed` and `dropped` (the queue holds `drift.datalake.queue` samples; a new one is dropped
when it is full). The token is read from `drift.datalake.token_file` (the `ingest` token), never from the configuration file itself and never
printed. Only plain HTTP is spoken, and only a loopback address is accepted unless `drift.datalake.allow_remote` is true, because the token and
the samples would travel in clear text. HTTPS is not supported yet.

## Metrics

`GET /metrics` adds `hydra_umc_twin_dryrun_pass_total`, `hydra_umc_twin_dryrun_not_pass_total`, `hydra_umc_twin_drift_observations_total` and
`hydra_umc_twin_drift_alarms_total`. They are numbers about the service: no position, token or path.
