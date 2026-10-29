//! Calibration against Kotlin: five short searches (one thread, a budget that is never reached, `extra_rounds` of 2)
//! whose routed chains refit the q(k) table and the area factor. Rust must collect the same chains and so report the
//! same lines (`q(k) = ... from n chains`, the area ratios), find the same sample count and area ratios, and end
//! with the same tuning, bit for bit. A second end-to-end check of the trial loop: any earlier divergence shows up in
//! the chains. The goldens are dumped by rust/tools/kotlin/GoldenVizTest.kt (CALIB_OUT).

mod common;

use common::{assert_no_differences, compare, compare_list, read};
use factorio_optimizer::config::run_configuration::{RunConfiguration, SearchConfig};
use factorio_optimizer::config::tuning::Tuning;
use factorio_optimizer::data::loader::load_root_data;
use factorio_optimizer::plan::ProductionPlan;
use factorio_optimizer::search::calibrate::{calibrate_plans, write_tuning};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize)]
struct Expected {
    budget: f64,
    samples: usize,
    jobs: Vec<String>,
    report: Vec<String>,
    tuned: String,
    ratios: BTreeMap<String, f64>,
}

#[test]
fn calibration_matches_kotlin() {
    let dir = common::repo_root().join("rust/tests/golden/calibrate");
    let want: Expected = serde_json::from_str(&read(dir.join("expected.json"))).unwrap();
    let base = Tuning::load(dir.join("base_tuning.json").to_str().unwrap()).unwrap();
    // The plans Kotlin made: Rust's LP solver (microlp) answers in slightly different last bits than Kotlin's (OR-Tools).
    let jobs: Vec<(String, RunConfiguration, ProductionPlan)> = want
        .jobs
        .iter()
        .map(|name| {
            let job = RunConfiguration::load(dir.join(format!("configs/{name}.json")).to_str().unwrap()).unwrap();
            let config = RunConfiguration { search: SearchConfig { time_budget_s: want.budget, ..job.search.clone() }, ..job };
            let plan: ProductionPlan = serde_json::from_str(&read(dir.join(format!("plans/{name}.plan.json")))).unwrap();
            (name.clone(), config, plan)
        })
        .collect();
    let data = load_root_data("recipes.json").unwrap();
    let out = std::env::temp_dir().join(format!("factorio-calibrate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let got = calibrate_plans(&data, &jobs, &base, &out).unwrap();

    let mut diffs = Vec::new();
    compare("routed chains", &got.samples, &want.samples, &mut diffs);
    compare_list("report", &got.report, &want.report, &mut diffs);
    let ratios: BTreeMap<String, f64> = got.area_ratios.iter().map(|(k, v)| (k.clone(), *v)).collect();
    compare("area ratios", &ratios, &want.ratios, &mut diffs);
    let tuned = Tuning::parse(&want.tuned).unwrap();
    compare("q table", &got.tuning.place.q_table, &tuned.place.q_table, &mut diffs);
    compare("area factor", &got.tuning.search.area_factor, &tuned.search.area_factor, &mut diffs);
    compare("everything else in the tuning is the base", &(got.tuning == tuned), &true, &mut diffs);
    assert_no_differences("calibration", diffs);

    // The corpus is not vacuous: several terminal counts made it into the table, and its curve rises.
    assert!(want.report.iter().filter(|l| l.starts_with("q(")).count() >= 4, "{:?}", want.report);
    assert!(got.tuning.place.q_table.len() > base.place.q_table.len() - 3 && got.tuning.place.q_table != base.place.q_table);

    // What the CLI writes with it.
    let file = out.join("tuning.json");
    write_tuning(&got.tuning, &file).unwrap();
    assert_eq!(Tuning::load(file.to_str().unwrap()).unwrap(), got.tuning);
    let _ = std::fs::remove_dir_all(&out);
}
