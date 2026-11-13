// =============================================================================
// HYDRA-UMC-TWIN - src/simulation_bench.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0-or-later - see LICENSE
// =============================================================================
//! The simulation bench of the twin: a simulated robot stands for the real machine, a second simulated robot is the twin, both receive the
//! same seeded orders, and the drift monitor watches the difference while faults are injected into the "real" side (a joint that sticks, an
//! encoder that jumps, a lost order, an actuator that is too slow, a single bad reading). Nothing here touches a robot: a pass means "passed in
//! simulation". The numbers of the robot are those of a model.

use crate::drift::{DriftMonitor, Level};
use crate::dynamics::{SimConfig, SimulatedRobot};
use crate::twincfg::{DriftCfg, Lang};

const TICK_S: f64 = 0.02;
const TICKS: usize = 2000;
const ORDER_EVERY: usize = 75;
const JOINTS: [&str; 3] = ["shoulder", "elbow", "wrist"];

fn model(max_velocity: f64) -> SimConfig {
    let joints: Vec<String> = JOINTS
        .iter()
        .map(|name| {
            format!(
                r#"{{"name":"{name}","lower":-1.5,"upper":1.5,"max_velocity":{max_velocity},"max_acceleration":4.0,"initial":0.0}}"#
            )
        })
        .collect();
    SimConfig::parse(&format!(r#"{{"joints":[{}]}}"#, joints.join(","))).unwrap()
}

/// A small generator with a fixed seed.
struct Lcg(u64);

impl Lcg {
    fn unit(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
    }
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }
}

#[derive(Default, Clone)]
struct Fault {
    /// The elbow of the real robot stops following its orders at this tick.
    stuck_elbow_at: Option<usize>,
    /// The reading of the wrist is this much off from this tick on.
    wrist_offset_from: Option<(usize, f64)>,
    /// The shoulder of the real robot never hears an order.
    shoulder_deaf: bool,
    /// One reading of the elbow is this much off, at this tick, once.
    elbow_spike: Option<(usize, f64)>,
    /// The real actuators have this top speed (the twin has 1.0).
    real_max_velocity: Option<f64>,
}

struct Report {
    first_alarm: [Option<usize>; 3],
    alarms: [u64; 3],
    worst: Level,
    summary: serde_json::Value,
}

fn run(seed: u64, fault: &Fault) -> Report {
    let mut rng = Lcg(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut twin = SimulatedRobot::new(&model(1.0));
    let mut real = SimulatedRobot::new(&model(fault.real_max_velocity.unwrap_or(1.0)));
    let mut monitor = DriftMonitor::new(
        DriftCfg {
            warn: 0.02,
            alarm: 0.05,
            alarm_after: 3,
            ..DriftCfg::default()
        },
        None,
        None,
    );
    let mut first_alarm = [None; 3];
    for tick in 0..TICKS {
        if tick % ORDER_EVERY == 0 {
            for (index, joint) in JOINTS.iter().enumerate() {
                let target = if tick == 0 {
                    [1.0, -1.0, 0.5][index]
                } else {
                    rng.range(-1.4, 1.4)
                };
                twin.command(joint, target).unwrap();
                let stuck = fault.stuck_elbow_at.is_some_and(|at| tick >= at) && *joint == "elbow";
                let deaf = fault.shoulder_deaf && *joint == "shoulder";
                if !stuck && !deaf {
                    real.command(joint, target).unwrap();
                }
            }
        }
        if fault.stuck_elbow_at == Some(tick) {
            real.stop("elbow").unwrap();
        }
        twin.advance(TICK_S).unwrap();
        real.advance(TICK_S).unwrap();
        for (index, joint) in JOINTS.iter().enumerate() {
            let mut reading = real.position(joint).unwrap() + rng.range(-0.01, 0.01);
            if let Some((from, offset)) = fault.wrist_offset_from {
                if *joint == "wrist" && tick >= from {
                    reading += offset;
                }
            }
            if let Some((at, spike)) = fault.elbow_spike {
                if *joint == "elbow" && tick == at {
                    reading += spike;
                }
            }
            let seen = monitor
                .observe(
                    joint,
                    reading,
                    twin.position(joint).unwrap(),
                    tick as u64 * 20,
                )
                .unwrap();
            if seen.level == Level::Alarm && first_alarm[index].is_none() {
                first_alarm[index] = Some(tick);
            }
        }
    }
    let summary = monitor.summary(Lang::En);
    let mut alarms = [0u64; 3];
    for entry in summary["joints"].as_array().unwrap() {
        let name = entry["joint"].as_str().unwrap();
        let index = JOINTS.iter().position(|j| *j == name).unwrap();
        alarms[index] = entry["alarms"].as_u64().unwrap();
    }
    let worst = match summary["level"].as_str().unwrap() {
        "alarm" => Level::Alarm,
        "warn" => Level::Warn,
        _ => Level::Ok,
    };
    Report {
        first_alarm,
        alarms,
        worst,
        summary,
    }
}

#[test]
fn a_healthy_machine_with_a_noisy_sensor_never_raises_an_alarm() {
    for seed in 1..=5 {
        let report = run(seed, &Fault::default());
        assert_eq!(report.worst, Level::Ok, "seed {seed}: {}", report.summary);
        assert_eq!(report.alarms, [0, 0, 0], "seed {seed}");
    }
}

#[test]
fn the_same_seed_and_the_same_fault_give_the_same_report() {
    let fault = Fault {
        wrist_offset_from: Some((300, 0.08)),
        ..Fault::default()
    };
    assert_eq!(run(3, &fault).summary, run(3, &fault).summary);
}

#[test]
fn an_encoder_that_jumps_is_an_alarm_on_that_joint_after_three_readings_and_on_no_other() {
    let fault = Fault {
        wrist_offset_from: Some((300, 0.08)),
        ..Fault::default()
    };
    for seed in 1..=3 {
        let report = run(seed, &fault);
        assert_eq!(report.first_alarm[2], Some(302), "seed {seed}");
        assert_eq!(report.first_alarm[0], None, "seed {seed}");
        assert_eq!(report.first_alarm[1], None, "seed {seed}");
        assert_eq!(report.worst, Level::Alarm);
    }
}

#[test]
fn a_joint_that_sticks_is_found_and_only_that_joint() {
    let fault = Fault {
        stuck_elbow_at: Some(500),
        ..Fault::default()
    };
    for seed in 1..=3 {
        let report = run(seed, &fault);
        assert!(report.alarms[1] >= 1, "seed {seed}: {}", report.summary);
        let found = report.first_alarm[1].expect("the sticking elbow is found");
        assert!(found >= 500, "no alarm before the fault: {found}");
        assert_eq!(report.alarms[0], 0, "seed {seed}");
        assert_eq!(report.alarms[2], 0, "seed {seed}");
    }
}

#[test]
fn a_lost_order_is_found_on_the_joint_that_did_not_hear_it() {
    let fault = Fault {
        shoulder_deaf: true,
        ..Fault::default()
    };
    let report = run(2, &fault);
    assert!(report.first_alarm[0].is_some(), "{}", report.summary);
    assert_eq!(report.alarms[1], 0);
    assert_eq!(report.alarms[2], 0);
}

#[test]
fn an_actuator_that_is_too_slow_drifts_and_is_found() {
    let fault = Fault {
        real_max_velocity: Some(0.25),
        ..Fault::default()
    };
    let report = run(4, &fault);
    assert_eq!(report.worst, Level::Alarm, "{}", report.summary);
    assert!(report.alarms.iter().any(|a| *a >= 1));
}

#[test]
fn one_bad_reading_is_a_warning_and_never_an_alarm() {
    let fault = Fault {
        elbow_spike: Some((900, 0.5)),
        ..Fault::default()
    };
    let report = run(5, &fault);
    assert_eq!(report.worst, Level::Warn, "{}", report.summary);
    assert_eq!(report.alarms, [0, 0, 0]);
}

#[test]
fn orders_far_outside_the_travel_never_take_a_joint_past_its_limits_or_its_top_speed() {
    let config = model(1.0);
    let mut robot = SimulatedRobot::new(&config);
    let mut rng = Lcg(77);
    for tick in 0..TICKS {
        if tick % 25 == 0 {
            for joint in JOINTS {
                let outcome = robot.command(joint, rng.range(-50.0, 50.0)).unwrap();
                assert!((-1.5..=1.5).contains(&outcome.target));
            }
        }
        robot.advance(TICK_S).unwrap();
        for snapshot in robot.snapshot() {
            assert!(
                (-1.5 - 1e-9..=1.5 + 1e-9).contains(&snapshot.position),
                "{}: {}",
                snapshot.joint,
                snapshot.position
            );
            assert!(snapshot.velocity.abs() <= 1.0 + 1e-9, "{}", snapshot.joint);
        }
    }
}

#[test]
fn a_robot_given_an_order_that_cannot_be_obeyed_says_so_and_stays_where_it_is() {
    let mut robot = SimulatedRobot::new(&model(1.0));
    assert!(robot.command("elbow", f64::NAN).is_err());
    assert!(robot.command("no-such-joint", 0.1).is_err());
    assert!(robot.stop("no-such-joint").is_err());
    assert!(robot.advance(-1.0).is_err());
    assert!(robot.advance(f64::INFINITY).is_err());
    assert!(robot.advance(1.0e6).is_err());
    assert_eq!(robot.position("elbow"), Some(0.0));
    assert_eq!(robot.position("no-such-joint"), None);
}
