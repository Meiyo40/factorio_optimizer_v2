//! Refits the placer's q(k) table and the s_lo area factor from routed results: runs a short search per job,
//! collects every routed chain (terminals, HPWL, belt tiles) from all successful trials, and takes the smallest
//! feasible box area over machine area per job.

use super::search::Search;
use crate::config::run_configuration::{RunConfiguration, SearchConfig};
use crate::config::tuning::{PlaceTuning, SearchTuning, Tuning};
use crate::data::data_model::RootData;
use crate::plan::{ProductionPlan, build_plan};
use crate::route::ChainStat;
use crate::util::compare_f64;
use crate::util::text::fmt_fixed;
use anyhow::{Context, Result};
use indexmap::IndexMap;
use std::collections::BTreeMap;
use std::path::Path;

pub struct CalibrationResult {
    pub tuning: Tuning,
    pub samples: usize,
    pub area_ratios: IndexMap<String, f64>,
    pub report: Vec<String>,
}

/// Runs a search of `budget_s` seconds on each job (each in its own folder of `out`) and refits the tuning `base`.
pub fn calibrate(data: &RootData, jobs: &[(String, RunConfiguration)], base: &Tuning, budget_s: f64, out: &Path) -> Result<CalibrationResult> {
    let mut planned = Vec::new();
    for (name, job) in jobs {
        let config = RunConfiguration { search: SearchConfig { time_budget_s: budget_s, ..job.search.clone() }, ..job.clone() };
        let plan = build_plan(data, &config, base)?;
        planned.push((name.clone(), config, plan));
    }
    calibrate_plans(data, &planned, base, out)
}

/// [`calibrate`] for jobs that are planned already (the plan of a job is the same for every run, and Rust's differs
/// from Kotlin's in the last bits, so the tests hand it the plans Kotlin made). Each job's config carries its budget.
pub fn calibrate_plans(data: &RootData, jobs: &[(String, RunConfiguration, ProductionPlan)], base: &Tuning, out: &Path) -> Result<CalibrationResult> {
    let mut stats: Vec<ChainStat> = Vec::new();
    let mut ratios: IndexMap<String, f64> = IndexMap::new();
    let mut report: Vec<String> = Vec::new();
    for (name, config, plan) in jobs {
        let result = Search::new(data, config, base, plan, out.join(name), None)
            .with_on_trial(|t| {
                if let (true, Some(route)) = (t.success, t.route.as_ref()) {
                    stats.extend(route.chain_stats.iter().cloned());
                }
            })
            .run()?;
        if let Some(best) = &result.best {
            ratios.insert(name.clone(), f64::from(best.area()) / f64::from(plan.total_machine_area()));
        }
        let best = result.best.as_ref().map_or_else(|| "none".to_owned(), |b| format!("{}x{}", b.w, b.h));
        let ratio = ratios.get(name).map_or_else(|| "-".to_owned(), |r| fmt_fixed(*r, 2));
        report.push(format!("{name}: best {best}, area ratio {ratio}, {} trials", result.trials));
    }
    let tuning = refit(base, &stats, &ratios, &mut report);
    Ok(CalibrationResult { tuning, samples: stats.len(), area_ratios: ratios, report })
}

/// The tuning `base` with q(k) taken from `stats` and the area factor from `ratios`; what it found goes to `report`.
fn refit(base: &Tuning, stats: &[ChainStat], ratios: &IndexMap<String, f64>, report: &mut Vec<String>) -> Tuning {
    // q(k): median tiles / HPWL per terminal count, made non-decreasing in k.
    let mut by_k: BTreeMap<usize, Vec<&ChainStat>> = BTreeMap::new();
    for s in stats.iter().filter(|s| s.hpwl > 0) {
        by_k.entry(s.terminals).or_default().push(s);
    }
    let mut points: Vec<Vec<f64>> = Vec::new();
    let mut last = 0.0f64;
    for (k, list) in &by_k {
        if list.len() < 3 {
            continue;
        }
        let mut sorted: Vec<f64> = list.iter().map(|s| s.tiles as f64 / f64::from(s.hpwl)).collect();
        sorted.sort_by(|a, b| compare_f64(*a, *b));
        last = last.max(sorted[sorted.len() / 2]);
        points.push(vec![*k as f64, last]);
        report.push(format!("q({k}) = {} from {} chains", fmt_fixed(last, 3), list.len()));
    }
    let q_table = if points.len() >= 2 {
        // Extend beyond the largest observed k with the old table's slope.
        let (last_k, last_q) = (points[points.len() - 1][0], points[points.len() - 1][1]);
        let tail = base.place.q_table.iter().filter(|p| p[0] > last_k).map(|p| vec![p[0], p[1] - base.place.q(last_k as i32) + last_q]);
        let mut table = vec![vec![1.0, points[0][1]]];
        table.extend(points.iter().filter(|p| p[0] > 1.0).cloned());
        table.extend(tail);
        table
    } else {
        base.place.q_table.clone()
    };
    // Area factor: s_lo must stay a lower bound, so use the tightest job (with a small margin).
    let factor = ratios.values().copied().reduce(f64::min).map_or(base.search.area_factor, |r| r * 0.95);
    report.push(format!("area factor {} (min ratio over {} jobs), {} routed chains", fmt_fixed(factor, 3), ratios.len(), stats.len()));
    Tuning { place: PlaceTuning { q_table, ..base.place.clone() }, search: SearchTuning { area_factor: factor, ..base.search.clone() }, ..base.clone() }
}

/// Writes `t` as a tuning file (and the folders above it).
pub fn write_tuning(t: &Tuning, file: &Path) -> Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("Could not create {}", dir.display()))?;
    }
    std::fs::write(file, t.to_json()).with_context(|| format!("Could not write {}", file.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(terminals: usize, hpwl: i32, tiles: usize) -> ChainStat {
        ChainStat { item: "iron-plate".to_owned(), terminals, hpwl, tiles }
    }

    #[test]
    fn q_is_the_median_per_terminal_count_made_non_decreasing_and_extended_with_the_old_slope() {
        let base = Tuning::default();
        // k = 2: ratios 1.0, 2.0, 3.0 -> median 2.0; k = 3: only two chains, skipped; k = 4: 0.5, 0.5, 0.5 -> 0.5,
        // lifted to 2.0 by the non-decreasing rule; a chain with no extent is ignored.
        let mut stats = vec![chain(2, 10, 10), chain(2, 10, 20), chain(2, 10, 30), chain(3, 10, 10), chain(3, 10, 10), chain(2, 0, 5)];
        stats.extend([chain(4, 10, 5), chain(4, 10, 5), chain(4, 10, 5)]);
        let mut report = Vec::new();
        let tuned = refit(&base, &stats, &IndexMap::new(), &mut report);
        let table = &tuned.place.q_table;
        assert_eq!(table[0], vec![1.0, 2.0], "starts at k = 1 with the first point");
        assert_eq!(table[1], vec![2.0, 2.0]);
        assert_eq!(table[2], vec![4.0, 2.0], "non-decreasing");
        // Past k = 4 the old table's points follow, shifted so the curve is continuous.
        assert_eq!(table[3][0], 5.0);
        assert!((table[3][1] - (1.1536 - base.place.q(4) + 2.0)).abs() < 1e-12);
        assert_eq!(table.len(), 3 + base.place.q_table.iter().filter(|p| p[0] > 4.0).count());
        assert_eq!(report[0], "q(2) = 2.000 from 3 chains");
        assert_eq!(report[1], "q(4) = 2.000 from 3 chains");
    }

    #[test]
    fn with_fewer_than_two_points_the_old_table_stays_and_the_area_factor_is_the_tightest_ratio_less_a_margin() {
        let base = Tuning::default();
        let ratios: IndexMap<String, f64> = [("a".to_owned(), 3.0), ("b".to_owned(), 2.0)].into_iter().collect();
        let mut report = Vec::new();
        let tuned = refit(&base, &[chain(2, 10, 10)], &ratios, &mut report);
        assert_eq!(tuned.place.q_table, base.place.q_table);
        assert!((tuned.search.area_factor - 1.9).abs() < 1e-12);
        assert_eq!(report, vec!["area factor 1.900 (min ratio over 2 jobs), 1 routed chains".to_owned()]);
        // No job found a layout: the old factor stays.
        assert_eq!(refit(&base, &[], &IndexMap::new(), &mut Vec::new()).search.area_factor, base.search.area_factor);
    }
}
