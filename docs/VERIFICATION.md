# Verifying a grasp and a placement, sweeping the sensitivity, and the rule of safe degradation

Not tried with a real robot: the evidence of everything here is the simulator and the unit tests.

## The rule: degrade towards the safer side, never towards a pass

* A check that could not run, or whose data is missing or not a finite number, is **never** a pass.
* A planner or a retiming that fails with its constraints is a failure to report. The code never retries without the
  constraints, never widens a tolerance after an error, and never reports success because a step "did not complain".
* A report that cannot be completed says it is incomplete and which case stopped it; it does not leave out the runs that broke.
* What a controller says about itself ("done", a phase counter) is not evidence. Only what the twin measured is.

The tests that keep the rule: `no_missing_or_worse_finding_ever_improves_the_verdict` (the dry run, over every combination of
findings and switches), the `Unverified` cases of `src/verify.rs`, the aborting sweep of `src/sweep.rs`, and the exit codes of
`verify-grasp` in `tests/cli_modes.rs`. A change that relaxes a constraint after an error, or that reads a missing measurement
as a success, makes one of them fail.

## `verify-grasp`

```
hydra-umc-twin verify-grasp <observation.json> [--config <verify.json>] [--check grasp|placement|pick-and-place]
```

It judges the task only from the measured poses of the object and of the gripper (`src/verify.rs`).

| output | exit code | meaning |
|--------|-----------|---------|
| `VERIFY=VERIFIED` | `0` | every check ran on good data and passed |
| `VERIFY=FAILED reasons=...` | `1` | a check ran and the measurement says the task was not done |
| `VERIFY=UNVERIFIED reasons=...` | `3` | the data cannot support a verdict: this is not a pass |
| a message on stderr | `2` | the command line, a file or the configuration is wrong |

The grasp is judged after the object was carried along the transport trajectory: at every sample the object is within
`hold_radius_mm` of the gripper, at the end it is at least `min_lift_mm` above where it rested, and it moved at least
`min_travel_mm`. The placement is judged by the distance between the measured final pose and the destination
(`dest_tolerance_mm`). `controller_reports_done` may be in the file so a log can be replayed as it is; it never changes a verdict.

Reasons are stable codes: `config_invalid`, `missing_rest_pose`, `missing_transport`, `too_few_samples`,
`non_finite_measurement`, `time_not_increasing`, `missing_destination`, `missing_final_pose` (unverified) and `not_held`,
`lift_too_low`, `travel_too_short`, `off_target` (failed). A failure found in the data wins over a missing piece elsewhere.

### The observation

```json
{
  "rest_object": [0, 0, 0],
  "transport": [{"t_s": 0.0, "object": [0, 0, 50], "gripper": [0, 0, 52]}],
  "destination": [200, 0, 0],
  "final_object": [200, 0, 0]
}
```

Positions are `[x, y, z]` in millimetres, `z` up; the times of `transport` grow strictly; unknown fields are refused.

### The configuration file

Every field is optional and every number must be finite and above zero; a field that is not known is an error.

| key | meaning | default |
|-----|---------|---------|
| `min_lift_mm` | the object is at least this high above where it rested at the end of the carry | `20` |
| `hold_radius_mm` | the object stays within this distance of the gripper at every sample | `15` |
| `min_travel_mm` | the object moved at least this far during the carry | `100` |
| `dest_tolerance_mm` | a placed object ends within this distance of the destination | `5` |
| `min_samples` | fewer samples cannot show the object was held (2 to 100000) | `5` |
| `transport.distance_mm` | how far the object is carried | `200` |
| `transport.max_speed_mm_s` | top speed of the carry | `250` |
| `transport.max_accel_mm_s2` | top acceleration and deceleration of the carry | `1000` |
| `transport.steps` | equal time steps of the carry (2 to 100000) | `8` |

`transport_profile` turns the `transport` block into the checkpoints of a trapezoid profile (a triangle when the distance is too
short to reach the top speed): the speed and the acceleration never pass their tops and the last checkpoint is exactly the
distance. A simulator that drives the carry samples the profile at those checkpoints.

## Sweeps of sensitivity and of mounting misalignment

`src/sweep.rs` runs one scenario at the nominal values, then once per factor of every physical parameter (for example mass or
friction from 0.5 to 2) and once per offset of the mounting of the arm (`forward` and `lateral` in millimetres, `yaw` in
degrees), each with a number of seeds counted from `base_seed`, and writes the table as Markdown or CSV:

```json
{
  "parameters": [{"name": "mass", "nominal": 0.2, "factors": [0.5, 2.0]}],
  "mount_offsets": [{"axis": "forward", "values": [-5, 5]}],
  "seeds": 5,
  "base_seed": 0
}
```

The scenario is a function the caller gives. It answers verified, not verified, or an error. An error is never counted as a pass:
the sweep stops at the first one, keeps the rows that ran, and the report says `ABORTED` with the case. A sweep is limited to
64 values per list, 1000 seeds per row and 100000 runs in all. The same configuration gives the same table.

The sweep is a library: the twin runs no task controller yet, so nothing in the server calls it. Connecting it to the simulated
robot (a scenario that runs the checks of `verify-grasp` over a simulated carry) is future work.
