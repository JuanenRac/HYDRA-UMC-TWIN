// =============================================================================
// HYDRA-UMC-TWIN - src/verify.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0-or-later - see LICENSE
// =============================================================================
//! Checks of a grasp and of a placement against the pose the twin MEASURED, never against what a controller says.
//!
//! A controller that reports "done" proves nothing: its phase counter can reach the end while the object never left the table. The
//! functions here judge only from measured poses of the object and of the gripper, with tolerances read from a configuration file
//! (`VerifyConfig`), and answer one of three verdicts:
//!
//! - `Verified`: every check ran on good data and passed.
//! - `Failed`: a check ran and the measurement says the task was not done (the object is not held, not lifted, did not travel or
//!   is off its target).
//! - `Unverified`: the data cannot support a verdict (a pose is missing or not finite, too few samples, time that does not grow,
//!   a configuration that does not validate). This is NOT a pass: nothing here ever turns a missing measurement into a success.
//!
//! The grasp is scored after the object was carried along a transport trajectory (a trapezoid profile with a top speed and a top
//! acceleration, both configurable): the object must still be within `hold_radius_mm` of the gripper at every sample, lifted at
//! least `min_lift_mm` above where it rested, and moved at least `min_travel_mm`.
//!
//! The reasons are stable machine codes (`Reason::code`), not sentences, so every front end can say them in its own language.
//! The twin does not run a task controller yet: feeding the samples from the simulated robot is future wiring, and the command
//! `verify-grasp` takes the samples from a file.

use serde::{Deserialize, Serialize};

/// A position in millimetres: `[x, y, z]`, `z` up.
pub type Vec3 = [f64; 3];

/// The most samples one observation may carry (a bound on what a file can make the twin read).
pub const MAX_SAMPLES: usize = 100_000;

/// The shape of the transport the object is carried along.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct TransportConfig {
    /// How far the object is carried, in millimetres.
    pub distance_mm: f64,
    /// The top speed of the carry, in millimetres per second.
    pub max_speed_mm_s: f64,
    /// The top acceleration (and deceleration) of the carry, in millimetres per second squared.
    pub max_accel_mm_s2: f64,
    /// How many equal time steps the carry is divided into (the profile has one checkpoint more than this).
    pub steps: usize,
}

impl Default for TransportConfig {
    fn default() -> Self {
        TransportConfig {
            distance_mm: 200.0,
            max_speed_mm_s: 250.0,
            max_accel_mm_s2: 1000.0,
            steps: 8,
        }
    }
}

/// The tolerances of the checks. Every default is the conservative one; a caller changes them in its configuration file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct VerifyConfig {
    /// The object must be at least this high above where it rested when the carry ends.
    pub min_lift_mm: f64,
    /// The object must stay within this distance of the gripper at every sample of the carry.
    pub hold_radius_mm: f64,
    /// The object must have moved at least this far between the first and the last sample of the carry.
    pub min_travel_mm: f64,
    /// A placed object must end within this distance of its destination.
    pub dest_tolerance_mm: f64,
    /// Fewer samples than this cannot show that the object was held through the carry.
    pub min_samples: usize,
    /// The transport trajectory.
    pub transport: TransportConfig,
}

impl Default for VerifyConfig {
    fn default() -> Self {
        VerifyConfig {
            min_lift_mm: 20.0,
            hold_radius_mm: 15.0,
            min_travel_mm: 100.0,
            dest_tolerance_mm: 5.0,
            min_samples: 5,
            transport: TransportConfig::default(),
        }
    }
}

fn positive(value: f64) -> bool {
    value.is_finite() && value > 0.0
}

impl VerifyConfig {
    /// Reads a configuration from JSON; a field left out keeps its default, a field that is not known is an error.
    pub fn parse(text: &str) -> Result<VerifyConfig, String> {
        let config: VerifyConfig =
            serde_json::from_str(text).map_err(|e| format!("verify config: {e}"))?;
        config.validate()?;
        Ok(config)
    }

    /// Every tolerance must be a finite number above zero, and the counts must be usable.
    pub fn validate(&self) -> Result<(), String> {
        let numbers = [
            ("min_lift_mm", self.min_lift_mm),
            ("hold_radius_mm", self.hold_radius_mm),
            ("min_travel_mm", self.min_travel_mm),
            ("dest_tolerance_mm", self.dest_tolerance_mm),
            ("transport.distance_mm", self.transport.distance_mm),
            ("transport.max_speed_mm_s", self.transport.max_speed_mm_s),
            ("transport.max_accel_mm_s2", self.transport.max_accel_mm_s2),
        ];
        for (name, value) in numbers {
            if !positive(value) {
                return Err(format!("{name} must be a finite number above zero"));
            }
        }
        if !(2..=MAX_SAMPLES).contains(&self.min_samples) {
            return Err(format!("min_samples must be from 2 to {MAX_SAMPLES}"));
        }
        if !(2..=MAX_SAMPLES).contains(&self.transport.steps) {
            return Err(format!("transport.steps must be from 2 to {MAX_SAMPLES}"));
        }
        Ok(())
    }
}

/// One point of the transport profile: how far along the carry the object should be at a moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Checkpoint {
    pub t_s: f64,
    pub distance_mm: f64,
}

/// The trapezoid profile of the carry: accelerate at `max_accel_mm_s2` up to `max_speed_mm_s`, cruise, decelerate. When the distance
/// is too short to reach the top speed the profile is a triangle. The result has `steps + 1` checkpoints at equal times, the first
/// at distance 0 and the last at exactly `distance_mm`; the speed never passes the top speed nor the acceleration the top one.
pub fn transport_profile(config: &TransportConfig) -> Result<Vec<Checkpoint>, String> {
    let probe = VerifyConfig {
        transport: config.clone(),
        ..VerifyConfig::default()
    };
    probe.validate()?;
    let d = config.distance_mm;
    let v = config.max_speed_mm_s;
    let a = config.max_accel_mm_s2;
    // the peak speed actually reached, the time to reach it and the distance covered while doing so
    let (peak, t_acc, d_acc) = if v * v / a >= d {
        let peak = (a * d).sqrt();
        (peak, peak / a, d / 2.0)
    } else {
        (v, v / a, v * v / (2.0 * a))
    };
    let t_cruise = ((d - 2.0 * d_acc) / peak).max(0.0);
    let total = 2.0 * t_acc + t_cruise;
    let at = |t: f64| -> f64 {
        if t <= t_acc {
            0.5 * a * t * t
        } else if t <= t_acc + t_cruise {
            d_acc + peak * (t - t_acc)
        } else {
            let left = (total - t).max(0.0);
            d - 0.5 * a * left * left
        }
    };
    let steps = config.steps;
    let mut points = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        let t = total * (i as f64) / (steps as f64);
        let distance = if i == steps { d } else { at(t).clamp(0.0, d) };
        points.push(Checkpoint {
            t_s: t,
            distance_mm: distance,
        });
    }
    Ok(points)
}

/// One measured moment of the carry.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sample {
    /// Seconds since the start of the carry.
    pub t_s: f64,
    /// The measured position of the object.
    pub object: Vec3,
    /// The measured position of the gripper.
    pub gripper: Vec3,
}

/// What the twin measured. `controller_reports_done` is accepted so a log can be replayed as it is, and is ignored: it never
/// changes a verdict.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Observation {
    pub controller_reports_done: bool,
    /// Where the object rested before the grasp.
    pub rest_object: Option<Vec3>,
    /// The measured carry, in time order.
    pub transport: Vec<Sample>,
    /// Where the object was meant to end (for the placement check).
    pub destination: Option<Vec3>,
    /// Where the object was measured at the end of the placement.
    pub final_object: Option<Vec3>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Verified,
    Failed,
    Unverified,
}

/// Why a verdict is not `Verified`. The codes are stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    // -- the data cannot support a verdict (Unverified)
    ConfigInvalid,
    MissingRestPose,
    MissingTransport,
    TooFewSamples,
    NonFiniteMeasurement,
    TimeNotIncreasing,
    MissingDestination,
    MissingFinalPose,
    // -- the measurement says the task was not done (Failed)
    NotHeld,
    LiftTooLow,
    TravelTooShort,
    OffTarget,
}

impl Reason {
    /// The stable code of the reason.
    pub fn code(self) -> &'static str {
        match self {
            Reason::ConfigInvalid => "config_invalid",
            Reason::MissingRestPose => "missing_rest_pose",
            Reason::MissingTransport => "missing_transport",
            Reason::TooFewSamples => "too_few_samples",
            Reason::NonFiniteMeasurement => "non_finite_measurement",
            Reason::TimeNotIncreasing => "time_not_increasing",
            Reason::MissingDestination => "missing_destination",
            Reason::MissingFinalPose => "missing_final_pose",
            Reason::NotHeld => "not_held",
            Reason::LiftTooLow => "lift_too_low",
            Reason::TravelTooShort => "travel_too_short",
            Reason::OffTarget => "off_target",
        }
    }

    /// True for the reasons that mean the measurement itself shows the task was not done.
    pub fn is_failure(self) -> bool {
        matches!(
            self,
            Reason::NotHeld | Reason::LiftTooLow | Reason::TravelTooShort | Reason::OffTarget
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Outcome {
    pub verdict: Verdict,
    pub reasons: Vec<Reason>,
}

impl Outcome {
    fn verified() -> Outcome {
        Outcome {
            verdict: Verdict::Verified,
            reasons: Vec::new(),
        }
    }

    fn unverified(reason: Reason) -> Outcome {
        Outcome {
            verdict: Verdict::Unverified,
            reasons: vec![reason],
        }
    }

    fn from_reasons(reasons: Vec<Reason>) -> Outcome {
        let verdict = if reasons.is_empty() {
            Verdict::Verified
        } else if reasons.iter().any(|r| r.is_failure()) {
            Verdict::Failed
        } else {
            Verdict::Unverified
        };
        Outcome { verdict, reasons }
    }

    /// True only for `Verified`.
    pub fn is_verified(&self) -> bool {
        self.verdict == Verdict::Verified
    }
}

fn finite3(p: &Vec3) -> bool {
    p.iter().all(|c| c.is_finite())
}

fn distance(a: &Vec3, b: &Vec3) -> f64 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// Judges a grasp, after the carry, from the measured poses only.
pub fn verify_grasp(config: &VerifyConfig, observation: &Observation) -> Outcome {
    if config.validate().is_err() {
        return Outcome::unverified(Reason::ConfigInvalid);
    }
    let Some(rest) = observation.rest_object else {
        return Outcome::unverified(Reason::MissingRestPose);
    };
    let samples = &observation.transport;
    if samples.is_empty() {
        return Outcome::unverified(Reason::MissingTransport);
    }
    if !(config.min_samples..=MAX_SAMPLES).contains(&samples.len()) {
        return Outcome::unverified(Reason::TooFewSamples);
    }
    let finite = finite3(&rest)
        && samples
            .iter()
            .all(|s| s.t_s.is_finite() && finite3(&s.object) && finite3(&s.gripper));
    if !finite {
        return Outcome::unverified(Reason::NonFiniteMeasurement);
    }
    if samples.windows(2).any(|w| w[1].t_s <= w[0].t_s) {
        return Outcome::unverified(Reason::TimeNotIncreasing);
    }

    let mut reasons = Vec::new();
    if samples
        .iter()
        .any(|s| distance(&s.object, &s.gripper) > config.hold_radius_mm)
    {
        reasons.push(Reason::NotHeld);
    }
    let first = &samples[0];
    let last = &samples[samples.len() - 1];
    if last.object[2] - rest[2] < config.min_lift_mm {
        reasons.push(Reason::LiftTooLow);
    }
    if distance(&first.object, &last.object) < config.min_travel_mm {
        reasons.push(Reason::TravelTooShort);
    }
    Outcome::from_reasons(reasons)
}

/// Judges a placement from the measured final pose of the object and its destination.
pub fn verify_placement(config: &VerifyConfig, observation: &Observation) -> Outcome {
    if config.validate().is_err() {
        return Outcome::unverified(Reason::ConfigInvalid);
    }
    let Some(destination) = observation.destination else {
        return Outcome::unverified(Reason::MissingDestination);
    };
    let Some(final_pose) = observation.final_object else {
        return Outcome::unverified(Reason::MissingFinalPose);
    };
    if !finite3(&destination) || !finite3(&final_pose) {
        return Outcome::unverified(Reason::NonFiniteMeasurement);
    }
    if distance(&destination, &final_pose) > config.dest_tolerance_mm {
        return Outcome::from_reasons(vec![Reason::OffTarget]);
    }
    Outcome::verified()
}

/// Judges a whole pick and place: the worse of the grasp and the placement (`Failed` over `Unverified` over `Verified`).
pub fn verify_pick_and_place(config: &VerifyConfig, observation: &Observation) -> Outcome {
    let grasp = verify_grasp(config, observation);
    let placement = verify_placement(config, observation);
    let mut reasons = grasp.reasons;
    reasons.extend(placement.reasons);
    Outcome::from_reasons(reasons)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A good carry: the object rests at z=0, is lifted 50 mm and carried 200 mm along x, held right at the gripper.
    fn good() -> Observation {
        let samples = (0..8)
            .map(|i| {
                let x = 200.0 * (i as f64) / 7.0;
                Sample {
                    t_s: 0.1 * (i as f64),
                    object: [x, 0.0, 50.0],
                    gripper: [x, 0.0, 52.0],
                }
            })
            .collect();
        Observation {
            controller_reports_done: true,
            rest_object: Some([0.0, 0.0, 0.0]),
            transport: samples,
            destination: Some([200.0, 0.0, 0.0]),
            final_object: Some([201.0, 0.0, 0.0]),
        }
    }

    #[test]
    fn a_held_lifted_and_carried_object_is_verified() {
        let outcome = verify_grasp(&VerifyConfig::default(), &good());
        assert_eq!(outcome.verdict, Verdict::Verified);
        assert!(outcome.reasons.is_empty());
        assert!(verify_pick_and_place(&VerifyConfig::default(), &good()).is_verified());
    }

    #[test]
    fn a_controller_that_says_done_while_the_object_never_moved_fails() {
        let mut o = good();
        o.controller_reports_done = true;
        for s in &mut o.transport {
            s.object = [0.0, 0.0, 0.0];
            s.gripper = [0.0, 0.0, 2.0];
        }
        let outcome = verify_grasp(&VerifyConfig::default(), &o);
        assert_eq!(outcome.verdict, Verdict::Failed);
        assert!(outcome.reasons.contains(&Reason::LiftTooLow));
        assert!(outcome.reasons.contains(&Reason::TravelTooShort));
    }

    #[test]
    fn the_claim_of_the_controller_never_changes_the_verdict() {
        for claim in [true, false] {
            let mut o = good();
            o.controller_reports_done = claim;
            assert!(verify_grasp(&VerifyConfig::default(), &o).is_verified());
            for s in &mut o.transport {
                s.object[2] = 0.0;
            }
            assert_eq!(
                verify_grasp(&VerifyConfig::default(), &o).verdict,
                Verdict::Failed
            );
        }
    }

    #[test]
    fn an_object_that_slips_out_of_the_gripper_during_the_carry_fails() {
        let mut o = good();
        // from the fifth sample on the object lags 40 mm behind the gripper
        for s in o.transport.iter_mut().skip(4) {
            s.object[0] -= 40.0;
        }
        let outcome = verify_grasp(&VerifyConfig::default(), &o);
        assert_eq!(outcome.verdict, Verdict::Failed);
        assert!(outcome.reasons.contains(&Reason::NotHeld));
    }

    #[test]
    fn an_object_dropped_at_the_end_of_the_carry_fails_even_if_it_was_held_before() {
        let mut o = good();
        let last = o.transport.len() - 1;
        o.transport[last].object[2] = 3.0;
        o.transport[last].gripper[2] = 52.0;
        let outcome = verify_grasp(&VerifyConfig::default(), &o);
        assert_eq!(outcome.verdict, Verdict::Failed);
        assert!(outcome.reasons.contains(&Reason::NotHeld));
        assert!(outcome.reasons.contains(&Reason::LiftTooLow));
    }

    #[test]
    fn the_tolerances_are_the_ones_of_the_configuration() {
        let mut o = good();
        for s in &mut o.transport {
            s.object[2] = 12.0;
            s.gripper[2] = 12.0;
        }
        assert_eq!(
            verify_grasp(&VerifyConfig::default(), &o).verdict,
            Verdict::Failed
        );
        let loose = VerifyConfig::parse(r#"{"min_lift_mm": 10}"#).unwrap();
        assert!(verify_grasp(&loose, &o).is_verified());
        let exact = VerifyConfig {
            min_lift_mm: 12.0,
            ..VerifyConfig::default()
        };
        // the edge is a pass: lifted exactly the demanded height
        assert!(verify_grasp(&exact, &o).is_verified());
    }

    #[test]
    fn missing_or_unusable_data_is_unverified_and_never_a_pass() {
        let config = VerifyConfig::default();
        let mut cases: Vec<(Observation, Reason)> = Vec::new();

        let mut o = good();
        o.rest_object = None;
        cases.push((o, Reason::MissingRestPose));

        let mut o = good();
        o.transport.clear();
        cases.push((o, Reason::MissingTransport));

        let mut o = good();
        o.transport.truncate(3);
        cases.push((o, Reason::TooFewSamples));

        let mut o = good();
        o.transport[2].object[1] = f64::NAN;
        cases.push((o, Reason::NonFiniteMeasurement));

        let mut o = good();
        o.rest_object = Some([0.0, f64::INFINITY, 0.0]);
        cases.push((o, Reason::NonFiniteMeasurement));

        let mut o = good();
        o.transport[3].t_s = o.transport[2].t_s;
        cases.push((o, Reason::TimeNotIncreasing));

        let mut o = good();
        o.transport[5].t_s = -1.0;
        cases.push((o, Reason::TimeNotIncreasing));

        for (observation, reason) in cases {
            let outcome = verify_grasp(&config, &observation);
            assert_eq!(outcome.verdict, Verdict::Unverified, "{reason:?}");
            assert_eq!(outcome.reasons, vec![reason]);
            assert!(!outcome.is_verified());
        }
    }

    #[test]
    fn a_configuration_that_does_not_validate_is_unverified() {
        let bad = VerifyConfig {
            hold_radius_mm: f64::NAN,
            ..VerifyConfig::default()
        };
        let outcome = verify_grasp(&bad, &good());
        assert_eq!(outcome.verdict, Verdict::Unverified);
        assert_eq!(outcome.reasons, vec![Reason::ConfigInvalid]);
        assert_eq!(
            verify_placement(&bad, &good()).reasons,
            vec![Reason::ConfigInvalid]
        );
    }

    #[test]
    fn a_failure_found_in_the_data_wins_over_a_missing_piece_elsewhere() {
        let mut o = good();
        o.final_object = None;
        for s in &mut o.transport {
            s.object[2] = 0.0;
        }
        let outcome = verify_pick_and_place(&VerifyConfig::default(), &o);
        assert_eq!(outcome.verdict, Verdict::Failed);
        assert!(outcome.reasons.contains(&Reason::MissingFinalPose));
        // a good grasp with no final pose is unverified, not verified
        let mut o = good();
        o.final_object = None;
        let outcome = verify_pick_and_place(&VerifyConfig::default(), &o);
        assert_eq!(outcome.verdict, Verdict::Unverified);
    }

    #[test]
    fn a_placement_is_judged_on_the_measured_final_pose() {
        let config = VerifyConfig::default();
        let mut o = good();
        o.final_object = Some([200.0, 3.0, 4.0]);
        assert!(verify_placement(&config, &o).is_verified());
        o.final_object = Some([200.0, 0.0, 5.5]);
        let outcome = verify_placement(&config, &o);
        assert_eq!(outcome.verdict, Verdict::Failed);
        assert_eq!(outcome.reasons, vec![Reason::OffTarget]);
        o.final_object = None;
        assert_eq!(verify_placement(&config, &o).verdict, Verdict::Unverified);
        o.final_object = Some([200.0, 0.0, 0.0]);
        o.destination = None;
        assert_eq!(
            verify_placement(&config, &o).reasons,
            vec![Reason::MissingDestination]
        );
        o.destination = Some([f64::NAN, 0.0, 0.0]);
        assert_eq!(
            verify_placement(&config, &o).reasons,
            vec![Reason::NonFiniteMeasurement]
        );
    }

    #[test]
    fn the_empty_observation_verifies_nothing() {
        let o = Observation::default();
        let config = VerifyConfig::default();
        for outcome in [
            verify_grasp(&config, &o),
            verify_placement(&config, &o),
            verify_pick_and_place(&config, &o),
        ] {
            assert_eq!(outcome.verdict, Verdict::Unverified);
        }
    }

    #[test]
    fn the_configuration_refuses_what_makes_no_sense() {
        assert!(VerifyConfig::parse("{}").is_ok());
        assert!(VerifyConfig::parse(r#"{"unknown": 1}"#).is_err());
        assert!(VerifyConfig::parse(r#"{"min_lift_mm": 0}"#).is_err());
        assert!(VerifyConfig::parse(r#"{"min_lift_mm": -3}"#).is_err());
        assert!(VerifyConfig::parse(r#"{"min_samples": 1}"#).is_err());
        assert!(VerifyConfig::parse(r#"{"transport": {"steps": 1}}"#).is_err());
        assert!(VerifyConfig::parse(r#"{"transport": {"max_accel_mm_s2": 0}}"#).is_err());
        assert!(VerifyConfig::parse("not json").is_err());
        let parsed = VerifyConfig::parse(r#"{"transport": {"distance_mm": 80}}"#).unwrap();
        assert_eq!(parsed.transport.distance_mm, 80.0);
        assert_eq!(parsed.transport.max_speed_mm_s, 250.0);
    }

    #[test]
    fn the_transport_profile_is_a_trapezoid_that_keeps_its_limits() {
        let config = TransportConfig::default();
        let profile = transport_profile(&config).unwrap();
        assert_eq!(profile.len(), config.steps + 1);
        assert_eq!(profile[0].distance_mm, 0.0);
        assert_eq!(profile[0].t_s, 0.0);
        assert_eq!(profile[config.steps].distance_mm, 200.0);
        // 250 mm/s at 1000 mm/s2: 0.25 s up, 0.25 s down, 31.25 mm each, cruise (200 - 62.5) / 250 = 0.55 s
        assert!((profile[config.steps].t_s - 1.05).abs() < 1e-9);
        let dt = profile[1].t_s - profile[0].t_s;
        let mut last_speed = 0.0_f64;
        for w in profile.windows(2) {
            assert!(w[1].distance_mm >= w[0].distance_mm, "monotonic");
            assert!(w[1].distance_mm <= 200.0 + 1e-9, "never overshoots");
            let speed = (w[1].distance_mm - w[0].distance_mm) / dt;
            assert!(speed <= config.max_speed_mm_s + 1e-6, "speed {speed}");
            last_speed = speed;
        }
        assert!(last_speed <= config.max_speed_mm_s);
    }

    #[test]
    fn a_short_carry_is_a_triangle_and_never_reaches_the_top_speed() {
        let config = TransportConfig {
            distance_mm: 20.0,
            steps: 10,
            ..TransportConfig::default()
        };
        let profile = transport_profile(&config).unwrap();
        // v*v/a = 62.5 >= 20: the peak speed is sqrt(a*d) ~ 141 mm/s and the carry lasts 2*sqrt(d/a)
        let total = 2.0 * (20.0_f64 / 1000.0).sqrt();
        assert!((profile[10].t_s - total).abs() < 1e-9);
        assert_eq!(profile[10].distance_mm, 20.0);
        let dt = profile[1].t_s;
        for w in profile.windows(2) {
            let speed = (w[1].distance_mm - w[0].distance_mm) / dt;
            assert!(speed <= 250.0, "speed {speed}");
        }
        // halfway in time is halfway in distance
        assert!((profile[5].distance_mm - 10.0).abs() < 1e-9);
    }

    #[test]
    fn a_transport_profile_with_bad_numbers_is_refused() {
        for bad in [
            TransportConfig {
                distance_mm: 0.0,
                ..TransportConfig::default()
            },
            TransportConfig {
                max_speed_mm_s: f64::NAN,
                ..TransportConfig::default()
            },
            TransportConfig {
                max_accel_mm_s2: -1.0,
                ..TransportConfig::default()
            },
            TransportConfig {
                steps: 0,
                ..TransportConfig::default()
            },
        ] {
            assert!(transport_profile(&bad).is_err());
        }
    }

    #[test]
    fn reasons_have_stable_unique_codes_and_the_right_kind() {
        let all = [
            Reason::ConfigInvalid,
            Reason::MissingRestPose,
            Reason::MissingTransport,
            Reason::TooFewSamples,
            Reason::NonFiniteMeasurement,
            Reason::TimeNotIncreasing,
            Reason::MissingDestination,
            Reason::MissingFinalPose,
            Reason::NotHeld,
            Reason::LiftTooLow,
            Reason::TravelTooShort,
            Reason::OffTarget,
        ];
        let mut codes: Vec<&str> = all.iter().map(|r| r.code()).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), all.len());
        let failures: Vec<&str> = all
            .iter()
            .filter(|r| r.is_failure())
            .map(|r| r.code())
            .collect();
        assert_eq!(
            failures,
            vec!["not_held", "lift_too_low", "travel_too_short", "off_target"]
        );
    }

    #[test]
    fn an_observation_with_an_unknown_field_is_refused() {
        assert!(serde_json::from_str::<Observation>(r#"{"success": true}"#).is_err());
        let o: Observation = serde_json::from_str(r#"{"controller_reports_done": true}"#).unwrap();
        assert_eq!(
            verify_grasp(&VerifyConfig::default(), &o).verdict,
            Verdict::Unverified
        );
    }
}
