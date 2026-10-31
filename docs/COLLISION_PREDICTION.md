# Collision prediction: `POST /predict/collision`

The twin can say, for a short list of future states of the robot, whether any part
of it would enter a protected zone or touch another part of the robot. It is
information for an operator and for the bridge in shadow mode. **Nothing reads
it to decide anything**: the certificates and `/interlock/*` behave exactly as
they do without it.

**Not tried with a real robot.** The model is described, with its assumptions and
its limits, in the documentation of HYDRA-UMC-PHYSICS-REPLICA
(`docs/COLLISION_PREDICTION.md`), which does the computation; the twin starts it
and passes the answer on.

## Off by default

The route exists only when the server is started with `--collision-predict`.
Without it the path is like any other `POST` path (`405`), and no other route
changes. The switch needs the signing key and the certify token (the route shows
where the robot is and starts a process, so it uses the same token as
`/interlock/*`) and these files, none of which a request can choose:

| flag | meaning | default |
|------|---------|---------|
| `--collision-predict` | switch the route on | off |
| `--collision-urdf PATH` | the robot | required |
| `--collision-zones PATH` | a zones file of HYDRA-UMC-SAFETY-ZONES | required |
| `--collision-config PATH` | the sizes of the robot (`link_radius_m`, `tool_extent_m`, margins) | required |
| `--physics-replica-bin PATH` | the binary of HYDRA-UMC-PHYSICS-REPLICA (also `HYDRA_UMC_TWIN_PHYSICS_REPLICA_BIN`; else the one built next to the workspace) | built one |
| `--collision-timeout-ms N` | how long a prediction may take (100 to 60000) | `5000` |
| `--collision-max-states N` | most states in a request (1 to 256) | `64` |

The options of the prediction without `--collision-predict` stop the start, so a
half-configured server never runs.

## The request

```
POST /predict/collision
Authorization: Bearer <certify token>
Content-Type: application/json

{"states": [{"shoulder": 0.5}, {"shoulder": 0.9, "elbow": -0.2}]}
```

A state names the joints that move. The joints it leaves out are taken from the
last positions that the bridge reported to `POST /interlock/state` (commanded or
measured). A joint that neither the state nor the reports have is a `422`: an
unknown position is never read as zero. The same rules as the other `POST`
routes apply: a request with an `Origin` header, one not declared as JSON and a
body over 64 KiB are refused.

## The answer

`200` with the object of the prediction (`collision`, `level` of `clear`,
`warning` or `danger`, `self_collision`, the first `hits`, `min_clearance_m`,
`samples`, `outside_limits`; see the documentation of PHYSICS-REPLICA) plus
`reported_joints`, the number of joints the twin had a report for.

| status | meaning |
|--------|---------|
| `200` | the prediction was made; `collision` says what it found |
| `400` | the body is not a list of states |
| `401`, `403`, `413`, `415` | token, browser origin, size, content type (as on `/interlock/*`) |
| `422` | the computation refused the input (a joint the robot does not have, a position missing) |
| `502` | the computation could not be run or answered something unusable |
| `503` | two predictions are running already |
| `504` | the computation did not end within `--collision-timeout-ms` and was stopped |

A `200` with `collision: false` is only ever the word of the computation; every
failure is an error status and never reads as "clear".

## In shadow mode in the bridge

HYDRA-UMC-HIL-BRIDGE started with `--collision-shadow` (and `--twin-url`) asks
this route after each real command it delivered, naming the joint and the
position of the command. It records the answer and never blocks. See its
documentation.

## Limits

Those of the computation (declared radii, straight-line paths between states,
zones only as in the file, no dynamics), and:

- the positions of the other joints come from the last reports, which may be
  old; `GET /interlock/state` shows their age and `--require-fresh-state-ms`
  keeps the certificates from being signed on old ones, but the prediction itself
  does not refuse an old report;
- the process is started per request, at most two at a time.
