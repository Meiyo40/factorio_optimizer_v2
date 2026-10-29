//! The Rust trial against the Kotlin one. Every case is a job, a box, a seed and the options of `runTrial`; Rust
//! must reproduce the whole `TrialResult`: where it stopped, the notes of every feedback round and retry, the score
//! and annealing temperature, the layout entity by entity with its poles and wires, the placement, the ports, the
//! validator's errors and the summary of how routing went. The goldens are dumped by
//! rust/tools/kotlin/GoldenTrialTest.kt (TRIAL_OUT); every route ran with the endgame bounded by its round count
//! instead of the clock. TRIAL_CASE=<name> runs one case.

mod common;

use common::{EntityJson, compare, compare_list, entity_json, files_in, read};
use factorio_optimizer::config::run_configuration::RunConfiguration;
use factorio_optimizer::config::tuning::Tuning;
use factorio_optimizer::data::data_model::RootData;
use factorio_optimizer::data::loader::load_root_data;
use factorio_optimizer::geom::{Dir, Tile};
use factorio_optimizer::place::{PlacedMachine, Placement};
use factorio_optimizer::plan::ProductionPlan;
use factorio_optimizer::search::trial::{TrialOptions, TrialResult, run_trial};
use factorio_optimizer::validate::{OutputSpec, SourceSpec};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize)]
struct MachineJson {
    id: usize,
    x: i32,
    y: i32,
    rot: usize,
    mirror: bool,
}

#[derive(Deserialize)]
struct PlacementJson {
    w: i32,
    h: i32,
    machines: Vec<MachineJson>,
}

#[derive(Deserialize)]
struct ErrorJson {
    #[serde(rename = "type")]
    kind: String,
    x: i32,
    y: i32,
    message: String,
}

#[derive(Deserialize)]
struct HotspotJson {
    tile: Tile,
    nets: Vec<i32>,
}

#[derive(Deserialize)]
struct SummaryJson {
    rounds: i32,
    overuse: i32,
    failed_nets: Vec<i32>,
    hotspots: Vec<HotspotJson>,
}

#[derive(Deserialize)]
struct LayoutJson {
    w: i32,
    h: i32,
    label: String,
    entities: Vec<EntityJson>,
    wires: Vec<(usize, usize)>,
    tiles: Vec<(String, i32, i32)>,
}

#[derive(Deserialize)]
struct RouteJson {
    success: bool,
    iterations: i32,
    overuse: i32,
    chains: usize,
    failed_nets: Vec<i32>,
    entities: usize,
    conflicts: Vec<Tile>,
    log: Vec<String>,
}

#[derive(Deserialize)]
struct ResultJson {
    success: bool,
    stage: String,
    w: i32,
    h: i32,
    seed: i64,
    score: f64,
    anneal_t0: f64,
    notes: Vec<String>,
    errors: Vec<ErrorJson>,
    sources: Vec<SourceSpec>,
    outputs: Vec<OutputSpec>,
    summary: Option<SummaryJson>,
    layout: Option<LayoutJson>,
    placement: Option<PlacementJson>,
    route: Option<RouteJson>,
}

#[derive(Deserialize)]
struct Golden {
    plan: String,
    config: String,
    tuning: String,
    w: i32,
    h: i32,
    seed: i64,
    warm: Option<PlacementJson>,
    warm_t0: Option<f64>,
    lattice: Option<bool>,
    result: ResultJson,
}

fn golden_dir() -> PathBuf {
    common::repo_root().join("rust/tests/golden/trial")
}

fn placement_of(p: &PlacementJson) -> Placement {
    Placement { w: p.w, h: p.h, machines: p.machines.iter().map(|m| PlacedMachine { id: m.id, x: m.x, y: m.y, rot: Dir::ALL[m.rot], mirror: m.mirror }).collect() }
}

fn placement_json(p: &Placement) -> Vec<(usize, i32, i32, usize, bool)> {
    p.machines.iter().map(|m| (m.id, m.x, m.y, m.rot.ordinal(), m.mirror)).collect()
}

fn errors_of(errors: &[factorio_optimizer::validate::ValidationError]) -> Vec<(String, i32, i32, String)> {
    errors.iter().map(|e| (e.kind.name().to_owned(), e.tile.x, e.tile.y, e.message.clone())).collect()
}

fn compare_result(got: &TrialResult, want: &ResultJson, out: &mut Vec<String>) {
    compare("stage", &got.stage, &want.stage.as_str(), out);
    compare("success", &got.success, &want.success, out);
    compare("size", &(got.w, got.h, got.seed), &(want.w, want.h, want.seed), out);
    compare("score", &got.score, &want.score, out);
    compare("annealing t0", &got.anneal_t0, &want.anneal_t0, out);
    compare_list("notes", &got.notes, &want.notes, out);
    let want_errors: Vec<_> = want.errors.iter().map(|e| (e.kind.clone(), e.x, e.y, e.message.clone())).collect();
    compare_list("errors", &errors_of(&got.errors), &want_errors, out);
    compare_list("sources", &got.sources, &want.sources, out);
    compare_list("outputs", &got.outputs, &want.outputs, out);

    let got_summary = got.summary.as_ref().map(|s| {
        let hot: Vec<(Tile, Vec<i32>)> = s.hotspots.iter().map(|(t, n)| (*t, n.clone())).collect();
        (s.rounds, s.overuse, s.failed_nets.clone(), hot)
    });
    let want_summary = want.summary.as_ref().map(|s| (s.rounds, s.overuse, s.failed_nets.clone(), s.hotspots.iter().map(|h| (h.tile, h.nets.clone())).collect::<Vec<_>>()));
    compare("summary", &got_summary, &want_summary, out);

    match (&got.layout, &want.layout) {
        (Some(g), Some(w)) => {
            compare("layout size and label", &(g.w, g.h, g.label.as_str()), &(w.w, w.h, w.label.as_str()), out);
            let entities: Vec<EntityJson> = g.entities.iter().map(entity_json).collect();
            compare_list("layout entities", &entities, &w.entities, out);
            compare_list("layout wires", &g.wires, &w.wires, out);
            let tiles: Vec<(String, i32, i32)> = g.tiles.iter().map(|t| (t.name.clone(), t.x, t.y)).collect();
            compare_list("layout tiles", &tiles, &w.tiles, out);
        }
        (None, None) => {}
        (g, w) => out.push(format!("layout: rust {}, kotlin {}", g.is_some(), w.is_some())),
    }
    match (&got.placement, &want.placement) {
        (Some(g), Some(w)) => {
            compare("placement size", &(g.w, g.h), &(w.w, w.h), out);
            compare_list("placement machines", &placement_json(g), &placement_json(&placement_of(w)), out);
        }
        (None, None) => {}
        (g, w) => out.push(format!("placement: rust {}, kotlin {}", g.is_some(), w.is_some())),
    }
    match (&got.route, &want.route) {
        (Some(g), Some(w)) => {
            compare("route success", &g.success, &w.success, out);
            compare("route iterations", &g.iterations, &w.iterations, out);
            compare("route overuse", &g.overuse, &w.overuse, out);
            compare("route chains", &g.chains, &w.chains, out);
            compare("route failed nets", &g.failed_nets, &w.failed_nets, out);
            compare("route entities", &g.entities.len(), &w.entities, out);
            compare_list("route conflicts", &g.conflicts, &w.conflicts, out);
            compare_list("route log", &g.log, &w.log, out);
        }
        (None, None) => {}
        (g, w) => out.push(format!("route: rust {}, kotlin {}", g.is_some(), w.is_some())),
    }
}

fn run_case(name: &str, data: &RootData) -> Vec<String> {
    let dir = golden_dir();
    let g: Golden = serde_json::from_str(&read(dir.join("cases").join(format!("{name}.json")))).unwrap_or_else(|e| panic!("{name}: {e}"));
    let plan: ProductionPlan = serde_json::from_str(&read(dir.join(&g.plan))).unwrap();
    let config = RunConfiguration::load(dir.join(&g.config).to_str().unwrap()).unwrap();
    let tuning = Tuning::load(dir.join(&g.tuning).to_str().unwrap()).unwrap();
    let warm = g.warm.as_ref().map(placement_of);
    let opts = TrialOptions { warm: warm.as_ref(), warm_t0: g.warm_t0, deadline: i64::MAX, lattice: g.lattice };
    let got = run_trial(data, &plan, &config, &tuning, g.w, g.h, g.seed, opts).unwrap();
    let mut out = Vec::new();
    compare_result(&got, &g.result, &mut out);
    out
}

fn case_names() -> Vec<String> {
    let mut names: Vec<String> = files_in("rust/tests/golden/trial/cases", "json").iter().map(|p| p.file_stem().unwrap().to_string_lossy().into_owned()).collect();
    if let Ok(one) = std::env::var("TRIAL_CASE") {
        names.retain(|n| *n == one);
        assert!(!names.is_empty(), "no case named {one}");
    }
    names
}

fn run_shard(shard: usize, shards: usize) {
    let data = load_root_data("recipes.json").unwrap();
    let names = case_names();
    assert!(names.len() >= 80 || std::env::var("TRIAL_CASE").is_ok(), "the trial goldens are missing: {} cases", names.len());
    let mut failures = Vec::new();
    for name in names.iter().skip(shard).step_by(shards) {
        let diffs = run_case(name, &data);
        if !diffs.is_empty() {
            failures.push(format!("{name}:\n  {}", diffs.join("\n  ")));
        }
    }
    assert!(failures.is_empty(), "{} case(s) differ from Kotlin:\n{}", failures.len(), failures.join("\n"));
}

#[test]
fn trial_cases_0() {
    run_shard(0, 6);
}

#[test]
fn trial_cases_1() {
    run_shard(1, 6);
}

#[test]
fn trial_cases_2() {
    run_shard(2, 6);
}

#[test]
fn trial_cases_3() {
    run_shard(3, 6);
}

#[test]
fn trial_cases_4() {
    run_shard(4, 6);
}

#[test]
fn trial_cases_5() {
    run_shard(5, 6);
}

/// The corpus has to reach every branch of the trial, or agreeing on it would prove little.
#[test]
fn the_cases_reach_every_branch() {
    let mut reached = std::collections::BTreeMap::<&str, usize>::new();
    let probes = ["column start", "round ", "spread retry", "near miss", "power attempt", "moving machine"];
    for name in case_names() {
        let g: Golden = serde_json::from_str(&read(golden_dir().join("cases").join(format!("{name}.json")))).unwrap();
        *reached.entry(Box::leak(g.result.stage.clone().into_boxed_str())).or_default() += 1;
        for p in probes {
            if g.result.notes.iter().any(|n| n.contains(p)) {
                *reached.entry(p).or_default() += 1;
            }
        }
        if g.warm.is_some() {
            *reached.entry("warm start").or_default() += 1;
        }
        if g.result.layout.as_ref().is_some_and(|l| !l.tiles.is_empty()) {
            *reached.entry("marker tiles").or_default() += 1;
        }
        if g.result.layout.as_ref().is_some_and(|l| !l.wires.is_empty()) {
            *reached.entry("poles").or_default() += 1;
        }
    }
    for what in ["ok", "route", "column start", "round ", "spread retry", "near miss", "moving machine", "power attempt", "warm start", "marker tiles", "poles"] {
        assert!(reached.get(what).copied().unwrap_or(0) >= 1, "no case reaches {what}: {reached:?}");
    }
}
