// =============================================================================
// HYDRA-UMC-TWIN - src/synthetic.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0-or-later - see LICENSE
// =============================================================================
//! A synthetic pick-and-place scenario for the sweep: a small model, not a measurement of a robot.
//!
//! The scenario builds a measured carry from a few physical numbers and judges it with the same checks as `verify-grasp`
//! (`verify::verify_pick_and_place`), so a sweep over it shows how the verdict moves with the physics and with the mounting.
//!
//! - The grip holds while the friction of two fingers (`2 * friction * grip_force`) covers the weight plus the push of the top
//!   acceleration of the carry. Under that, the object slips down the gripper in proportion to the shortfall, up to 100 mm at the
//!   end of the carry.
//! - A misaligned mounting does not change the carry; it moves where the object ends: `forward` and `lateral` in millimetres, and
//!   `yaw` in degrees across a lever arm.
//! - The sensors add a bounded noise drawn from the seed of the run, so the same seed gives the same carry.
//!
//! Everything is simulation: the evidence of a result here is the simulator, never a robot.

use crate::sweep::{Axis, Case, Change, OffsetSweep, ParamSweep, SweepConfig};
use crate::verify::{
    transport_profile, verify_pick_and_place, Observation, Sample, Verdict, VerifyConfig,
};

/// The acceleration of gravity, in metres per second squared.
const GRAVITY: f64 = 9.81;
/// How far an object slips down the gripper when the grip gives no hold at all, in millimetres.
const FULL_SLIP_MM: f64 = 100.0;
/// How high the carry lifts the object, in millimetres.
const LIFT_MM: f64 = 50.0;

/// The physical numbers of the scenario.
#[derive(Debug, Clone, PartialEq)]
pub struct Scenario {
    /// The mass of the object, in kilograms.
    pub mass_kg: f64,
    /// The friction coefficient between a finger and the object.
    pub friction: f64,
    /// The force of each finger, in newtons.
    pub grip_force_n: f64,
    /// The bound of the noise of a sensor reading, in millimetres.
    pub noise_mm: f64,
    /// The distance between the mounting of the arm and the object at the destination, in millimetres.
    pub lever_arm_mm: f64,
}

impl Default for Scenario {
    fn default() -> Self {
        Scenario {
            mass_kg: 0.6,
            friction: 0.5,
            grip_force_n: 8.0,
            noise_mm: 1.0,
            lever_arm_mm: 300.0,
        }
    }
}

impl Scenario {
    /// Sets one named parameter; a name the scenario does not know, or a value out of range, is an error and never ignored.
    pub fn set(&mut self, name: &str, value: f64) -> Result<(), String> {
        let ok = match name {
            "mass" | "friction" | "grip_force" => value.is_finite() && value > 0.0,
            "noise" => value.is_finite() && value >= 0.0,
            _ => return Err(format!("the synthetic scenario has no parameter {name:?}")),
        };
        if !ok {
            return Err(format!("parameter {name}: {value} is out of range"));
        }
        match name {
            "mass" => self.mass_kg = value,
            "friction" => self.friction = value,
            "grip_force" => self.grip_force_n = value,
            _ => self.noise_mm = value,
        }
        Ok(())
    }

    /// How many times the friction of the fingers covers what the carry asks of it (1 or more holds).
    pub fn hold_ratio(&self, max_accel_mm_s2: f64) -> f64 {
        let capacity = 2.0 * self.friction * self.grip_force_n;
        let required = self.mass_kg * (GRAVITY + max_accel_mm_s2 / 1000.0);
        capacity / required
    }
}

/// A repeatable reading in -1..1 from a seed and a place in the run (SplitMix64).
fn unit(seed: u64, salt: u64) -> f64 {
    let mut z = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(salt.wrapping_mul(0xBF58_476D_1CE4_E5B9))
        .wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    ((z >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
}

/// The offset of the mounting of one run, if any.
fn mount_of(change: &Change) -> Option<(Axis, f64)> {
    match change {
        Change::Mount { axis, value } => Some((*axis, *value)),
        _ => None,
    }
}

/// Builds the measured carry of one run. `mount` is the offset of the mounting (axis and value), if the run has one.
pub fn observe(
    scenario: &Scenario,
    verify: &VerifyConfig,
    mount: Option<(Axis, f64)>,
    seed: u64,
) -> Result<Observation, String> {
    let profile = transport_profile(&verify.transport)?;
    let ratio = scenario.hold_ratio(verify.transport.max_accel_mm_s2);
    let slip_total = if ratio >= 1.0 {
        0.0
    } else {
        (1.0 - ratio) * FULL_SLIP_MM
    };
    let distance = verify.transport.distance_mm;
    let noise = scenario.noise_mm;
    let jitter = |salt: u64| unit(seed, salt) * noise;

    let mut transport = Vec::with_capacity(profile.len());
    for (i, point) in profile.iter().enumerate() {
        let salt = 8 * i as u64;
        let slip = slip_total * point.distance_mm / distance;
        transport.push(Sample {
            t_s: point.t_s,
            object: [
                point.distance_mm + jitter(salt),
                jitter(salt + 1),
                LIFT_MM - slip + jitter(salt + 2),
            ],
            gripper: [
                point.distance_mm + jitter(salt + 3),
                jitter(salt + 4),
                LIFT_MM + 2.0 + jitter(salt + 5),
            ],
        });
    }

    let (mut dx, mut dy) = (0.0, 0.0);
    match mount {
        Some((Axis::Forward, v)) => dx = v,
        Some((Axis::Lateral, v)) => dy = v,
        Some((Axis::Yaw, degrees)) => dy = scenario.lever_arm_mm * degrees.to_radians().sin(),
        None => {}
    }
    let tail = 8 * profile.len() as u64;
    Ok(Observation {
        controller_reports_done: true,
        rest_object: Some([0.0, 0.0, 0.0]),
        transport,
        destination: Some([distance, 0.0, 0.0]),
        final_object: Some([
            distance + dx + jitter(tail),
            dy + jitter(tail + 1),
            jitter(tail + 2),
        ]),
    })
}

/// The scenario of the sweep: applies the change of the case to the nominal scenario, builds the carry from the seed and
/// answers `Ok(true)` when it is verified and `Ok(false)` when the measurement says the task was not done. A verdict that the data
/// cannot support (`Unverified`) is an error, so the sweep stops there and never counts it as a pass.
pub fn evaluate(base: &Scenario, verify: &VerifyConfig, case: &Case) -> Result<bool, String> {
    let mut scenario = base.clone();
    if let Change::Parameter { name, value, .. } = &case.change {
        scenario.set(name, *value)?;
    }
    let observation = observe(&scenario, verify, mount_of(&case.change), case.seed)?;
    let outcome = verify_pick_and_place(verify, &observation);
    match outcome.verdict {
        Verdict::Verified => Ok(true),
        Verdict::Failed => Ok(false),
        Verdict::Unverified => {
            let codes: Vec<&str> = outcome.reasons.iter().map(|r| r.code()).collect();
            Err(format!("unverified: {}", codes.join(",")))
        }
    }
}

/// The sweep that runs when none is given: the three physical parameters at half to double, and the mounting off by a few
/// millimetres or degrees.
pub fn default_sweep() -> SweepConfig {
    let scenario = Scenario::default();
    let parameter = |name: &str, nominal: f64| ParamSweep {
        name: name.to_string(),
        nominal,
        factors: vec![0.5, 0.75, 1.5, 2.0],
    };
    SweepConfig {
        parameters: vec![
            parameter("mass", scenario.mass_kg),
            parameter("friction", scenario.friction),
            parameter("grip_force", scenario.grip_force_n),
        ],
        mount_offsets: vec![
            OffsetSweep {
                axis: Axis::Forward,
                values: vec![-6.0, -3.0, 3.0, 6.0],
            },
            OffsetSweep {
                axis: Axis::Lateral,
                values: vec![-6.0, -3.0, 3.0, 6.0],
            },
            OffsetSweep {
                axis: Axis::Yaw,
                values: vec![-1.0, 1.0, 2.0],
            },
        ],
        seeds: 5,
        base_seed: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sweep::run_sweep;

    fn case(change: Change, seed: u64) -> Case {
        Case { change, seed }
    }

    fn param(name: &str, value: f64) -> Change {
        Change::Parameter {
            name: name.to_string(),
            factor: 1.0,
            value,
        }
    }

    #[test]
    fn the_nominal_scenario_is_verified_for_every_seed() {
        let verify = VerifyConfig::default();
        for seed in 0..50 {
            assert_eq!(
                evaluate(&Scenario::default(), &verify, &case(Change::Nominal, seed)),
                Ok(true),
                "seed {seed}"
            );
        }
    }

    #[test]
    fn the_same_seed_gives_the_same_carry_and_another_gives_another() {
        let (s, v) = (Scenario::default(), VerifyConfig::default());
        let a = observe(&s, &v, None, 7).unwrap();
        assert_eq!(a, observe(&s, &v, None, 7).unwrap());
        assert_ne!(a, observe(&s, &v, None, 8).unwrap());
        // the noise stays within its bound
        for sample in &a.transport {
            assert!((sample.object[1]).abs() <= s.noise_mm);
        }
    }

    #[test]
    fn a_grip_that_cannot_hold_the_weight_fails_and_a_stronger_one_holds() {
        let verify = VerifyConfig::default();
        let base = Scenario::default();
        // twice the mass: the friction covers only 0.6 of what is asked
        assert!(base.hold_ratio(verify.transport.max_accel_mm_s2) > 1.0);
        let heavy = case(param("mass", 1.2), 3);
        assert_eq!(evaluate(&base, &verify, &heavy), Ok(false));
        let firm = case(param("grip_force", 16.0), 3);
        assert_eq!(evaluate(&base, &verify, &firm), Ok(true));
    }

    #[test]
    fn a_misaligned_mounting_moves_the_end_of_the_carry() {
        let (base, verify) = (Scenario::default(), VerifyConfig::default());
        let mount = |axis, value| case(Change::Mount { axis, value }, 1);
        assert_eq!(
            evaluate(&base, &verify, &mount(Axis::Forward, 2.0)),
            Ok(true)
        );
        assert_eq!(
            evaluate(&base, &verify, &mount(Axis::Forward, 6.0)),
            Ok(false)
        );
        assert_eq!(
            evaluate(&base, &verify, &mount(Axis::Lateral, -6.0)),
            Ok(false)
        );
        // 2 degrees across 300 mm is about 10 mm, 0.5 degrees about 2.6 mm
        assert_eq!(evaluate(&base, &verify, &mount(Axis::Yaw, 2.0)), Ok(false));
        assert_eq!(evaluate(&base, &verify, &mount(Axis::Yaw, 0.5)), Ok(true));
    }

    #[test]
    fn a_parameter_the_scenario_does_not_know_or_out_of_range_is_an_error() {
        let (base, verify) = (Scenario::default(), VerifyConfig::default());
        let unknown = evaluate(&base, &verify, &case(param("colour", 1.0), 0));
        assert!(unknown.unwrap_err().contains("colour"));
        assert!(evaluate(&base, &verify, &case(param("mass", -1.0), 0)).is_err());
        assert!(evaluate(&base, &verify, &case(param("noise", f64::NAN), 0)).is_err());
        assert!(evaluate(&base, &verify, &case(param("noise", 0.0), 0)).is_ok());
    }

    #[test]
    fn data_that_cannot_support_a_verdict_stops_the_sweep_and_is_never_a_pass() {
        let base = Scenario::default();
        // a carry shorter than the fewest samples the checks accept
        let verify = VerifyConfig {
            min_samples: 50,
            ..VerifyConfig::default()
        };
        let err = evaluate(&base, &verify, &case(Change::Nominal, 0)).unwrap_err();
        assert!(err.contains("too_few_samples"), "{err}");
        let report = run_sweep(&default_sweep(), &mut |c: &Case| {
            evaluate(&base, &verify, c)
        })
        .unwrap();
        assert!(!report.is_complete());
        assert_eq!(report.rows.len(), 1);
        assert_eq!(report.rows[0].passed, 0);
    }

    #[test]
    fn the_default_sweep_is_valid_and_shows_where_the_task_breaks() {
        let config = default_sweep();
        assert!(config.validate().is_ok());
        assert_eq!(config.total_runs(), 24 * 5);
        let (base, verify) = (Scenario::default(), VerifyConfig::default());
        let report = run_sweep(&config, &mut |c: &Case| evaluate(&base, &verify, c)).unwrap();
        assert!(report.is_complete());
        assert_eq!(report.rows[0].pass_rate(), 1.0);
        let row = |name: &str, factor: f64| {
            report
                .rows
                .iter()
                .find(|r| r.name == name && r.factor == Some(factor))
                .unwrap()
        };
        assert_eq!(row("mass", 0.5).pass_rate(), 1.0);
        assert_eq!(row("mass", 2.0).pass_rate(), 0.0);
        assert_eq!(row("friction", 0.5).pass_rate(), 0.0);
        assert_eq!(row("grip_force", 2.0).pass_rate(), 1.0);
        // the same configuration gives the same table
        let again = run_sweep(&config, &mut |c: &Case| evaluate(&base, &verify, c)).unwrap();
        assert_eq!(report.to_csv(), again.to_csv());
    }
}
