//! Same-seed parity with the Kotlin placer. Each golden in tests/golden/place was produced by rust/tools/kotlin/
//! GoldenPlaceTest.kt from the Kotlin implementation (upstream commit in the file): the plan Kotlin built, the
//! parameters, and the exact result. The Rust placer must reproduce the placement machine for machine, the
//! `moves` and `temperatures` counts, and the cost terms to within rounding.

mod common;

use factorio_optimizer::config::run_configuration::RunConfiguration;
use factorio_optimizer::config::tuning::PlaceTuning;
use factorio_optimizer::geom::{Dir, Tile};
use factorio_optimizer::place::{AnnealOptions, Annealer, PlaceModel, PlacedMachine, Placement, static_obstacles};
use factorio_optimizer::plan::ProductionPlan;
use factorio_optimizer::util::splittable_random::SplittableRandom;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize)]
struct Cell {
    id: usize,
    x: i32,
    y: i32,
    code: i32,
}

#[derive(Deserialize)]
struct Terms {
    wl: f64,
    cong: f64,
    pin: f64,
    di: f64,
    fb: f64,
    cost: f64,
}

#[derive(Deserialize)]
struct Stats {
    initial_cost: f64,
    final_cost: f64,
    t0: f64,
    moves: i64,
    temperatures: i32,
}

#[derive(Deserialize)]
struct Hist {
    x: i32,
    y: i32,
    v: f64,
}

#[derive(Deserialize)]
struct Step {
    t0: Option<f64>,
    t0_last_divisor: Option<f64>,
    max_moves: i64,
    start_range: Option<i32>,
    refine: bool,
    t0_scale: f64,
    beta2: f64,
    history: Option<Vec<Hist>>,
}

#[derive(Deserialize)]
struct Snapshot {
    placement: Vec<Cell>,
    terms: Terms,
}

#[derive(Deserialize)]
struct Outcome {
    placement: Vec<Cell>,
    terms: Terms,
    stats: Vec<Stats>,
    last_t0: f64,
}

#[derive(Deserialize)]
struct Golden {
    upstream: String,
    job: String,
    plan: String,
    w: i32,
    h: i32,
    seed: i64,
    start: String,
    from_placement: Option<Vec<Cell>>,
    place_tuning: PlaceTuning,
    steps: Vec<Step>,
    initial: Snapshot,
    result: Outcome,
}

fn dir() -> PathBuf {
    common::repo_root().join("rust/tests/golden/place")
}

fn close(what: &str, got: f64, want: f64) {
    assert!((got - want).abs() <= 1e-9 * want.abs().max(1.0), "{what}: got {got}, want {want}");
}

fn assert_terms(what: &str, m: &PlaceModel, want: &Terms) {
    close(&format!("{what} wl"), m.wl, want.wl);
    close(&format!("{what} cong"), m.cong, want.cong);
    close(&format!("{what} pin"), m.pin, want.pin);
    close(&format!("{what} di"), m.di, want.di);
    close(&format!("{what} fb"), m.fb, want.fb);
    close(&format!("{what} cost"), m.cost(), want.cost);
}

fn assert_cells(what: &str, m: &PlaceModel, want: &[Cell]) {
    assert_eq!(want.len(), m.n_m, "{what}: machine count");
    for c in want {
        assert_eq!((m.px[c.id], m.py[c.id], m.rot[c.id]), (c.x, c.y, c.code), "{what}: machine {} (x, y, orientation)", c.id);
    }
}

fn check(name: &str) {
    let g: Golden = serde_json::from_str(&std::fs::read_to_string(dir().join(format!("{name}.json"))).unwrap()).unwrap();
    let plan: ProductionPlan = serde_json::from_str(&std::fs::read_to_string(dir().join(&g.plan)).unwrap()).unwrap();
    let config = RunConfiguration::load(common::repo_root().join(&g.job).to_str().unwrap()).unwrap();
    let obstacles = static_obstacles(&config, &plan, g.w, g.h);
    let model = PlaceModel::new(&plan, &config, &g.place_tuning, g.w, g.h, &obstacles).unwrap();
    let mut a = Annealer::new(model, SplittableRandom::new(g.seed));
    let started = match g.start.as_str() {
        "random" => a.initial_random(),
        "columns" => a.initial_columns(&plan),
        "rows" => a.initial_rows(&plan),
        "from" => {
            let from = g.from_placement.as_ref().expect("a warm start carries its placement");
            let machines = from
                .iter()
                .map(|c| PlacedMachine { id: c.id, x: c.x, y: c.y, rot: Dir::ALL[(c.code % 4) as usize], mirror: c.code >= 4 })
                .collect();
            a.initial_from(&Placement { w: g.w, h: g.h, machines })
        }
        other => panic!("unknown start {other}"),
    };
    assert!(started, "{name} (upstream {}): no initial placement", g.upstream);
    assert_cells(&format!("{name} initial"), &a.model, &g.initial.placement);
    assert_terms(&format!("{name} initial"), &a.model, &g.initial.terms);

    assert_eq!(g.steps.len(), g.result.stats.len());
    for (i, (step, want)) in g.steps.iter().zip(&g.result.stats).enumerate() {
        if let Some(history) = &step.history {
            let tiles: Vec<(Tile, f64)> = history.iter().map(|h| (Tile::new(h.x, h.y), h.v)).collect();
            a.model.set_history(&tiles, step.beta2);
        }
        let t0 = step.t0.or(step.t0_last_divisor.map(|d| a.last_t0 / d));
        let got = a.anneal(AnnealOptions { t0, max_moves: Some(step.max_moves), start_range: step.start_range, refine: step.refine, t0_scale: step.t0_scale });
        let what = format!("{name} step {i}");
        assert_eq!((got.moves, got.temperatures), (want.moves, want.temperatures), "{what}: moves and temperatures");
        close(&format!("{what} t0"), got.t0, want.t0);
        close(&format!("{what} initial cost"), got.initial_cost, want.initial_cost);
        close(&format!("{what} final cost"), got.final_cost, want.final_cost);
    }
    assert_cells(&format!("{name} result"), &a.model, &g.result.placement);
    assert_terms(&format!("{name} result"), &a.model, &g.result.terms);
    close(&format!("{name} last t0"), a.last_t0, g.result.last_t0);
}

macro_rules! golden {
    ($($case:ident),+ $(,)?) => {
        $(#[test] fn $case() { check(stringify!($case)); })+

        /// Adding a case to the Kotlin dump without a test here would otherwise go unnoticed.
        #[test]
        fn every_golden_file_has_a_test() {
            let mut tested = vec![$(stringify!($case)),+];
            tested.sort();
            let mut files: Vec<String> = common::files_in("rust/tests/golden/place", "json")
                .iter()
                .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
                .filter(|n| n != "per_temp_x10")
                .collect();
            files.sort();
            assert_eq!(files, tested);
        }
    };
}

golden!(
    t2_random_14_s1,
    t3_random_24_s2,
    t4_random_20_s3,
    chem_random_36_s7,
    em_random_41_s2,
    t3_columns_path_24_s5,
    chem_columns_40_s4,
    t3_rows_24,
    t3_feedback_24_s6,
    t3_warm_24_s8,
);

#[test]
fn moves_per_temperature_match_java_math_pow_for_every_machine_count() {
    // `(10 * n^(4/3)).toInt()`: at a perfect cube (8, 27, 64, ...) one ulp decides between 160 and 159.
    let want: Vec<i32> = serde_json::from_str(&std::fs::read_to_string(dir().join("per_temp_x10.json")).unwrap()).unwrap();
    assert_eq!(want.len(), 700);
    for (i, w) in want.iter().enumerate() {
        let n = (i + 1) as f64;
        assert_eq!((10.0 * n.powf(4.0 / 3.0)) as i32, *w, "{} machines", i + 1);
    }
}
