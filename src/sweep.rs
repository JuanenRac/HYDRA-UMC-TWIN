// =============================================================================
// HYDRA-UMC-TWIN - src/sweep.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0-or-later - see LICENSE
// =============================================================================
//! Automatic sweeps of how much a result depends on a physical parameter and on how well the arm is mounted.
//!
//! A sweep runs one scenario many times: once at the nominal values, then once per factor of every physical parameter (for
//! example mass or friction from half to double), and once per offset of the mounting of the arm (forward, lateral, yaw), each
//! with several seeds. The scenario itself is a function the caller gives (`Evaluator`): it receives the `Case` (what is changed
//! and which seed) and answers `Ok(true)` when the task was verified, `Ok(false)` when it was not, or `Err` when it could not be
//! evaluated at all.
//!
//! An evaluation that fails is never a pass and never skipped: the sweep stops at the first one, marks the report as aborted and
//! says which case it was, because a table that quietly leaves out the runs that broke would look better than the truth.
//!
//! The report is a table, as Markdown (`SweepReport::to_markdown`) and as CSV (`SweepReport::to_csv`). The seeds are counted from
//! `base_seed`, so the same configuration gives the same table. Nothing here talks to the simulator: the twin does not run a task
//! controller yet, so a caller wires its own scenario, for instance the checks of `verify.rs` over a simulated run.

use serde::{Deserialize, Serialize};

/// The most values one list of a sweep may carry, and the most seeds per row.
pub const MAX_VALUES: usize = 64;
pub const MAX_SEEDS: u32 = 1000;
/// The most rows one sweep may produce, so a configuration cannot make the twin run for ever.
pub const MAX_RUNS: u64 = 100_000;

/// One physical parameter and the factors it is multiplied by.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParamSweep {
    pub name: String,
    /// The nominal value of the parameter.
    pub nominal: f64,
    /// The factors applied to the nominal value.
    #[serde(default = "default_factors")]
    pub factors: Vec<f64>,
}

fn default_factors() -> Vec<f64> {
    vec![0.5, 0.75, 1.5, 2.0]
}

/// The direction in which the mounting of the arm is moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Axis {
    Forward,
    Lateral,
    Yaw,
}

impl Axis {
    fn label(self) -> &'static str {
        match self {
            Axis::Forward => "forward",
            Axis::Lateral => "lateral",
            Axis::Yaw => "yaw",
        }
    }
}

/// An offset of the mounting: millimetres for `forward` and `lateral`, degrees for `yaw`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OffsetSweep {
    pub axis: Axis,
    pub values: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SweepConfig {
    pub parameters: Vec<ParamSweep>,
    pub mount_offsets: Vec<OffsetSweep>,
    /// How many seeds every row runs.
    pub seeds: u32,
    /// The first seed; the others follow it one by one.
    pub base_seed: u64,
}

impl Default for SweepConfig {
    fn default() -> Self {
        SweepConfig {
            parameters: Vec::new(),
            mount_offsets: Vec::new(),
            seeds: 5,
            base_seed: 0,
        }
    }
}

fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

impl SweepConfig {
    /// Reads a configuration from JSON and validates it.
    pub fn parse(text: &str) -> Result<SweepConfig, String> {
        let config: SweepConfig =
            serde_json::from_str(text).map_err(|e| format!("sweep config: {e}"))?;
        config.validate()?;
        Ok(config)
    }

    /// Names are plain (letters, digits, `_`, `-`, `.`) so a name cannot break a table; numbers are finite; factors are above zero;
    /// every list and the whole sweep are bounded.
    pub fn validate(&self) -> Result<(), String> {
        if self.seeds == 0 || self.seeds > MAX_SEEDS {
            return Err(format!("seeds must be from 1 to {MAX_SEEDS}"));
        }
        if self.parameters.len() > MAX_VALUES || self.mount_offsets.len() > MAX_VALUES {
            return Err(format!("at most {MAX_VALUES} parameters and offsets"));
        }
        for p in &self.parameters {
            if !plain_name(&p.name) {
                return Err(format!("parameter name {:?} is not a plain name", p.name));
            }
            if !p.nominal.is_finite() {
                return Err(format!("parameter {}: nominal must be finite", p.name));
            }
            if p.factors.is_empty() || p.factors.len() > MAX_VALUES {
                return Err(format!(
                    "parameter {}: from 1 to {MAX_VALUES} factors",
                    p.name
                ));
            }
            if p.factors.iter().any(|f| !f.is_finite() || *f <= 0.0) {
                return Err(format!(
                    "parameter {}: every factor must be a finite number above zero",
                    p.name
                ));
            }
        }
        for o in &self.mount_offsets {
            if o.values.is_empty() || o.values.len() > MAX_VALUES {
                return Err(format!(
                    "offset {}: from 1 to {MAX_VALUES} values",
                    o.axis.label()
                ));
            }
            if o.values.iter().any(|v| !v.is_finite()) {
                return Err(format!(
                    "offset {}: every value must be finite",
                    o.axis.label()
                ));
            }
        }
        if self.total_runs() > MAX_RUNS {
            return Err(format!("a sweep may run at most {MAX_RUNS} times"));
        }
        Ok(())
    }

    /// How many evaluations the whole sweep makes (the nominal row included).
    pub fn total_runs(&self) -> u64 {
        let rows: u64 = 1
            + self
                .parameters
                .iter()
                .map(|p| p.factors.len() as u64)
                .sum::<u64>()
            + self
                .mount_offsets
                .iter()
                .map(|o| o.values.len() as u64)
                .sum::<u64>();
        rows * u64::from(self.seeds)
    }
}

/// What a run changes from the nominal scenario.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// Nothing: the nominal scenario.
    Nominal,
    /// A physical parameter set to `value` (its nominal times `factor`).
    Parameter {
        name: String,
        factor: f64,
        value: f64,
    },
    /// The mounting of the arm moved along an axis.
    Mount { axis: Axis, value: f64 },
}

/// One run: what changes and which seed.
#[derive(Debug, Clone, PartialEq)]
pub struct Case {
    pub change: Change,
    pub seed: u64,
}

/// A scenario: `Ok(true)` verified, `Ok(false)` not verified, `Err` it could not be evaluated.
pub type Evaluator<'a> = dyn FnMut(&Case) -> Result<bool, String> + 'a;

/// The result of one row (one change, all its seeds).
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub kind: &'static str,
    pub name: String,
    /// The factor for a parameter, 1 for the nominal row, empty for a mounting offset.
    pub factor: Option<f64>,
    /// The value of the parameter or the offset.
    pub value: Option<f64>,
    pub runs: u32,
    pub passed: u32,
}

impl Row {
    /// The share of the runs that passed, from 0 to 1 (0 for a row with no run).
    pub fn pass_rate(&self) -> f64 {
        if self.runs == 0 {
            0.0
        } else {
            f64::from(self.passed) / f64::from(self.runs)
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SweepReport {
    pub rows: Vec<Row>,
    /// Set when an evaluation could not be made: the case and the reason. The rows after it were not run.
    pub aborted: Option<String>,
}

impl SweepReport {
    /// True only when every row ran every seed.
    pub fn is_complete(&self) -> bool {
        self.aborted.is_none()
    }

    /// The table as CSV, with a header line. Numbers use a dot and no thousands separator.
    pub fn to_csv(&self) -> String {
        let mut out = String::from("kind,name,factor,value,runs,passed,pass_rate\n");
        for r in &self.rows {
            out.push_str(&format!(
                "{},{},{},{},{},{},{:.3}\n",
                r.kind,
                r.name,
                opt(r.factor),
                opt(r.value),
                r.runs,
                r.passed,
                r.pass_rate()
            ));
        }
        out
    }

    /// The table as Markdown; an aborted sweep says so under the table.
    pub fn to_markdown(&self) -> String {
        let mut out = String::from(
            "| kind | name | factor | value | runs | passed | pass rate |\n|---|---|---|---|---|---|---|\n",
        );
        for r in &self.rows {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {:.3} |\n",
                r.kind,
                r.name,
                opt(r.factor),
                opt(r.value),
                r.runs,
                r.passed,
                r.pass_rate()
            ));
        }
        if let Some(why) = &self.aborted {
            out.push_str(&format!("\nABORTED: {why}\n"));
        }
        out
    }
}

fn opt(value: Option<f64>) -> String {
    match value {
        Some(v) => v.to_string(),
        None => String::new(),
    }
}

/// Runs the sweep. The nominal row comes first, then the parameters in the order given, then the offsets. The first evaluation
/// that fails stops the sweep: its row keeps what ran before it and the report says why it stopped.
pub fn run_sweep(
    config: &SweepConfig,
    evaluate: &mut Evaluator<'_>,
) -> Result<SweepReport, String> {
    config.validate()?;
    let fresh = |kind: &'static str, name: &str, factor: Option<f64>, value: Option<f64>| Row {
        kind,
        name: name.to_string(),
        factor,
        value,
        runs: 0,
        passed: 0,
    };
    let mut plan: Vec<(Row, Change)> = vec![(
        fresh("nominal", "nominal", Some(1.0), None),
        Change::Nominal,
    )];
    for p in &config.parameters {
        for factor in &p.factors {
            let value = p.nominal * factor;
            plan.push((
                fresh("parameter", &p.name, Some(*factor), Some(value)),
                Change::Parameter {
                    name: p.name.clone(),
                    factor: *factor,
                    value,
                },
            ));
        }
    }
    for o in &config.mount_offsets {
        for value in &o.values {
            plan.push((
                fresh("mount", o.axis.label(), None, Some(*value)),
                Change::Mount {
                    axis: o.axis,
                    value: *value,
                },
            ));
        }
    }

    let mut report = SweepReport {
        rows: Vec::new(),
        aborted: None,
    };
    for (mut row, change) in plan {
        for i in 0..config.seeds {
            let case = Case {
                change: change.clone(),
                seed: config.base_seed + u64::from(i),
            };
            match evaluate(&case) {
                Ok(ok) => {
                    row.runs += 1;
                    if ok {
                        row.passed += 1;
                    }
                }
                Err(reason) => {
                    report.aborted = Some(format!(
                        "{} {} seed {}: {reason}",
                        row.kind, row.name, case.seed
                    ));
                    report.rows.push(row);
                    return Ok(report);
                }
            }
        }
        report.rows.push(row);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> SweepConfig {
        SweepConfig::parse(
            r#"{
              "parameters": [{"name": "mass", "nominal": 0.2, "factors": [0.5, 2.0]}],
              "mount_offsets": [{"axis": "forward", "values": [-5, 5]}, {"axis": "yaw", "values": [3]}],
              "seeds": 4,
              "base_seed": 10
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn the_sweep_runs_the_nominal_row_then_every_factor_and_offset_with_every_seed() {
        let config = config();
        assert_eq!(config.total_runs(), 6 * 4);
        let mut seen: Vec<Case> = Vec::new();
        let report = run_sweep(&config, &mut |case: &Case| {
            seen.push(case.clone());
            Ok(true)
        })
        .unwrap();
        assert!(report.is_complete());
        assert_eq!(report.rows.len(), 6);
        assert_eq!(seen.len(), 24);
        assert_eq!(report.rows[0].kind, "nominal");
        assert_eq!(report.rows[1].name, "mass");
        assert_eq!(report.rows[1].value, Some(0.1));
        assert_eq!(report.rows[2].value, Some(0.4));
        assert_eq!(report.rows[3].name, "forward");
        assert_eq!(report.rows[5].name, "yaw");
        // the seeds count from the base seed, one by one, in every row
        let seeds: Vec<u64> = seen.iter().take(4).map(|c| c.seed).collect();
        assert_eq!(seeds, vec![10, 11, 12, 13]);
        assert!(report.rows.iter().all(|r| r.runs == 4 && r.passed == 4));
    }

    #[test]
    fn the_pass_rate_counts_only_what_the_scenario_verified() {
        let config = config();
        let report = run_sweep(&config, &mut |case: &Case| match &case.change {
            Change::Parameter { factor, .. } if *factor > 1.0 => Ok((case.seed & 1) == 0),
            Change::Mount {
                axis: Axis::Forward,
                value,
            } if *value < 0.0 => Ok(false),
            _ => Ok(true),
        })
        .unwrap();
        assert_eq!(report.rows[0].pass_rate(), 1.0);
        assert_eq!(report.rows[1].pass_rate(), 1.0);
        // seeds 10..13: the even ones pass
        assert_eq!(report.rows[2].passed, 2);
        assert_eq!(report.rows[2].pass_rate(), 0.5);
        assert_eq!(report.rows[3].passed, 0);
        assert_eq!(report.rows[4].passed, 4);
    }

    #[test]
    fn the_same_configuration_gives_the_same_table() {
        let config = config();
        let mut scenario = |case: &Case| -> Result<bool, String> { Ok(matches!(case.seed % 3, 1 | 2)) };
        let a = run_sweep(&config, &mut scenario).unwrap();
        let b = run_sweep(&config, &mut scenario).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.to_csv(), b.to_csv());
        assert_eq!(a.to_markdown(), b.to_markdown());
    }

    #[test]
    fn an_evaluation_that_fails_aborts_the_sweep_and_is_never_a_pass() {
        let config = config();
        let mut calls = 0;
        let report = run_sweep(&config, &mut |case: &Case| {
            calls += 1;
            match &case.change {
                Change::Parameter { factor, .. } if *factor > 1.0 && case.seed == 11 => {
                    Err("planner failed".to_string())
                }
                _ => Ok(true),
            }
        })
        .unwrap();
        assert!(!report.is_complete());
        let why = report.aborted.clone().unwrap();
        assert!(why.contains("parameter mass seed 11"), "{why}");
        assert!(why.contains("planner failed"), "{why}");
        // the nominal row, the first factor, and the row that broke with the one run that went before it
        assert_eq!(report.rows.len(), 3);
        let broken = &report.rows[2];
        assert_eq!((broken.runs, broken.passed), (1, 1));
        // nothing ran after the failure: 4 + 4 + 2 evaluations, the last one failing
        assert_eq!(calls, 10);
        assert!(report.to_markdown().contains("ABORTED: "));
    }

    #[test]
    fn an_evaluator_that_always_fails_never_produces_a_pass() {
        let config = config();
        let report = run_sweep(&config, &mut |_: &Case| Err("no".to_string())).unwrap();
        assert!(!report.is_complete());
        assert_eq!(report.rows.len(), 1);
        assert_eq!(report.rows[0].passed, 0);
        assert_eq!(report.rows[0].runs, 0);
        assert_eq!(report.rows[0].pass_rate(), 0.0);
    }

    #[test]
    fn the_csv_and_the_markdown_carry_the_rows() {
        let config = SweepConfig::parse(
            r#"{"parameters": [{"name": "friction", "nominal": 0.5, "factors": [2.0]}], "seeds": 2}"#,
        )
        .unwrap();
        let report = run_sweep(&config, &mut |case: &Case| Ok(case.seed == 0)).unwrap();
        assert_eq!(
            report.to_csv(),
            "kind,name,factor,value,runs,passed,pass_rate\n\
             nominal,nominal,1,,2,1,0.500\n\
             parameter,friction,2,1,2,1,0.500\n"
        );
        let md = report.to_markdown();
        assert!(md.starts_with("| kind | name | factor | value | runs | passed | pass rate |\n"));
        assert!(md.contains("| parameter | friction | 2 | 1 | 2 | 1 | 0.500 |\n"));
        assert!(!md.contains("ABORTED"));
    }

    #[test]
    fn the_configuration_refuses_what_makes_no_sense() {
        let bad = [
            r#"{"seeds": 0}"#,
            r#"{"seeds": 1001}"#,
            r#"{"unknown": 1}"#,
            r#"{"parameters": [{"name": "a,b", "nominal": 1}]}"#,
            r#"{"parameters": [{"name": "", "nominal": 1}]}"#,
            r#"{"parameters": [{"name": "m", "nominal": 1, "factors": []}]}"#,
            r#"{"parameters": [{"name": "m", "nominal": 1, "factors": [0]}]}"#,
            r#"{"parameters": [{"name": "m", "nominal": 1, "factors": [-1]}]}"#,
            r#"{"mount_offsets": [{"axis": "forward", "values": []}]}"#,
            r#"{"mount_offsets": [{"axis": "up", "values": [1]}]}"#,
            "not json",
        ];
        for text in bad {
            assert!(SweepConfig::parse(text).is_err(), "{text}");
        }
        // a sweep that would run for ever is refused
        let factors = vec!["1.5"; 64].join(",");
        let params: Vec<String> = (0..64)
            .map(|i| format!(r#"{{"name": "p{i}", "nominal": 1, "factors": [{factors}]}}"#))
            .collect();
        let text = format!(r#"{{"parameters": [{}], "seeds": 1000}}"#, params.join(","));
        assert!(SweepConfig::parse(&text).is_err());
        let ok = SweepConfig::parse("{}").unwrap();
        assert_eq!(ok.seeds, 5);
        assert!(run_sweep(&ok, &mut |_: &Case| Ok(true))
            .unwrap()
            .is_complete());
    }

    #[test]
    fn a_configuration_that_does_not_validate_runs_nothing() {
        let config = SweepConfig {
            seeds: 0,
            ..SweepConfig::default()
        };
        let mut called = false;
        let result = run_sweep(&config, &mut |_: &Case| {
            called = true;
            Ok(true)
        });
        assert!(result.is_err());
        assert!(!called);
    }
}
