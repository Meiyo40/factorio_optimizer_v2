//! Ports of the Kotlin PlacerTest: invariants of the incremental cost, legality of fluid connections, and the
//! statistical behaviour of the annealer. Exact same-seed behaviour is checked in place_golden.rs.

mod common;

use factorio_optimizer::config::run_configuration::{ConfigTarget, RunConfiguration};
use factorio_optimizer::config::tuning::{PlaceTuning, Tuning};
use factorio_optimizer::data::data_model::RootData;
use factorio_optimizer::data::loader::load_root_data;
use factorio_optimizer::geom::Tile;
use factorio_optimizer::place::{AnnealOptions, Annealer, PlaceModel, static_obstacles};
use factorio_optimizer::plan::{ProductionPlan, build_plan};
use factorio_optimizer::util::splittable_random::SplittableRandom;
use std::collections::HashSet;
use std::sync::LazyLock;

static DATA: LazyLock<RootData> = LazyLock::new(|| load_root_data("recipes.json").unwrap());

struct Job {
    config: RunConfiguration,
    plan: ProductionPlan,
}

fn job(name: &str) -> Job {
    let path = common::repo_root().join(format!("src/test/resources/jobs/{name}.json"));
    let config = RunConfiguration::load(path.to_str().unwrap()).unwrap();
    let plan = build_plan(&DATA, &config, &Tuning::default()).unwrap();
    Job { config, plan }
}

impl Job {
    fn annealer<'a>(&'a self, tun: &'a PlaceTuning, size: i32, seed: i64) -> Annealer<'a> {
        let obstacles = static_obstacles(&self.config, &self.plan, size, size);
        let model = PlaceModel::new(&self.plan, &self.config, tun, size, size, &obstacles).unwrap();
        Annealer::new(model, SplittableRandom::new(seed))
    }
}

fn terms(m: &PlaceModel) -> [f64; 6] {
    [m.wl, m.cong, m.pin, m.di, m.fb, m.cost()]
}

fn assert_incremental_matches_full(m: &mut PlaceModel, what: &str) {
    let incremental = terms(m);
    m.recompute_all();
    let full = terms(m);
    for i in 0..incremental.len() {
        assert!((incremental[i] - full[i]).abs() <= 1e-6 * full[i].abs().max(1.0), "{what} term {i}: incremental {} vs full {}", incremental[i], full[i]);
    }
}

#[test]
fn incremental_cost_matches_a_full_recompute_after_10000_random_moves() {
    let tun = PlaceTuning::default();
    for name in ["t3", "t4"] {
        let j = job(name);
        let mut a = j.annealer(&tun, 22, 99);
        a.model.set_history(&[(Tile::new(5, 5), 3.0), (Tile::new(10, 12), 1.5)], 0.7);
        assert!(a.initial_random());
        // A high fixed temperature, so the moves are a mix of accepts and reverts.
        a.anneal(AnnealOptions { t0: Some(1e6), max_moves: Some(10_000), ..Default::default() });
        assert_incremental_matches_full(&mut a.model, name);
    }
}

#[test]
fn annealing_t3_halves_the_random_initial_cost_for_5_seeds() {
    let tun = PlaceTuning::default();
    let j = job("t3");
    for seed in 1..=5 {
        let mut a = j.annealer(&tun, 24, seed);
        assert!(a.initial_random());
        let initial = a.model.cost();
        let stats = a.anneal(AnnealOptions::default());
        println!("T3 seed {seed}: {initial:.1} -> {:.1} in {} moves, {} temperatures", a.model.cost(), stats.moves, stats.temperatures);
        assert!(a.model.cost() < 0.5 * initial, "seed {seed}: {} vs initial {initial}", a.model.cost());
    }
}

#[test]
fn t2_places_the_circuit_machine_next_to_a_cable_machine_for_direct_insertion() {
    let tun = PlaceTuning::default();
    let j = job("t2");
    let mut hits = 0;
    for seed in 1..=5 {
        let mut a = j.annealer(&tun, 14, seed);
        assert!(a.initial_random());
        a.anneal(AnnealOptions::default());
        let recipe = |m: usize| j.plan.machines[m].recipe.as_str();
        let fits = a.model.di_pairs_fitting();
        println!("T2 seed {seed}: DI pairs {:?}", fits.iter().map(|&(p, c)| format!("{}->{}", recipe(p), recipe(c))).collect::<Vec<_>>());
        if fits.iter().any(|&(p, c)| recipe(p) == "copper-cable" && recipe(c) == "electronic-circuit") {
            hits += 1;
        }
    }
    assert!(hits >= 4, "direct insertion in only {hits} of 5 seeds");
}

/// Checks a placement against the fluid connection rules.
fn assert_connections_legal(m: &PlaceModel) {
    let p = m.placement();
    let rects: Vec<_> = p.machines.iter().map(|pm| p.rect(m.plan, pm)).collect();
    for i in 0..rects.len() {
        for j in i + 1..rects.len() {
            assert!(!rects[i].overlaps(&rects[j]), "machines {i} and {j} overlap");
        }
    }
    let all: Vec<_> = p.machines.iter().flat_map(|pm| p.connections(m.plan, pm)).collect();
    let used: Vec<_> = all.iter().filter(|c| c.fluid.is_some()).collect();
    let unused: HashSet<Tile> = all.iter().filter(|c| c.fluid.is_none()).map(|c| c.tile).collect();
    for c in &used {
        assert!(c.tile.x >= 0 && c.tile.x < p.w && c.tile.y >= 0 && c.tile.y < p.h, "pipe end {} outside", c.tile);
        assert!(rects.iter().all(|r| !r.contains_tile(c.tile)), "pipe end {} under a machine", c.tile);
        assert!(!unused.contains(&c.tile), "pipe end {} on an unused connection", c.tile);
    }
    for a in &used {
        for b in &used {
            if a.fluid != b.fluid {
                assert!(a.tile.manhattan(b.tile) > 1, "{:?} at {} touches {:?} at {}", a.fluid, a.tile, b.fluid, b.tile);
            }
        }
    }
}

#[test]
fn legality_keeps_fluid_connections_inside_the_box_and_apart_from_other_fluids() {
    let tun = PlaceTuning::default();
    let j = job("t4");
    let mut a = j.annealer(&tun, 20, 3);
    assert!(a.initial_random());
    a.anneal(AnnealOptions { max_moves: Some(20_000), ..Default::default() });
    assert_connections_legal(&a.model);
    assert_eq!(a.model.placement().machines.len(), j.plan.machines.len());
}

#[test]
fn refineries_and_chemical_plants_rotate_and_mirror_legally() {
    let tun = PlaceTuning::default();
    let config = RunConfiguration { targets: vec![ConfigTarget { name: "sulfur".to_owned(), amount: 3.0 }], ..Default::default() };
    let plan = build_plan(&DATA, &config, &Tuning::default()).unwrap();
    assert!(plan.machines.iter().any(|m| m.machine == "oil-refinery") && plan.machines.iter().any(|m| m.machine == "chemical-plant"));
    let mut orientations = HashSet::new();
    for seed in 1..=3 {
        let model = PlaceModel::new(&plan, &config, &tun, 30, 30, &HashSet::new()).unwrap();
        let mut a = Annealer::new(model, SplittableRandom::new(seed));
        assert!(a.initial_random());
        a.anneal(AnnealOptions { max_moves: Some(30_000), ..Default::default() });
        assert_connections_legal(&a.model);
        orientations.extend(a.model.rot.iter().copied());
        assert_incremental_matches_full(&mut a.model, "sulfur");
    }
    assert!(orientations.iter().any(|&r| r >= 4) && orientations.iter().any(|&r| r % 4 != 0), "{orientations:?}: both mirroring and rotation are used");
}

#[test]
fn incremental_cost_stays_exact_with_the_path_cost_and_column_moves() {
    let tun = PlaceTuning { belt_cost: "path".to_owned(), initial: "columns".to_owned(), column_share: 0.2, ..Default::default() };
    for name in ["t3", "t4"] {
        let j = job(name);
        let mut a = j.annealer(&tun, 24, 5);
        assert!(a.initial_random());
        let hot = AnnealOptions { t0: Some(1e6), max_moves: Some(10_000), ..Default::default() };
        a.anneal(hot);
        assert!(a.initial_columns(&j.plan));
        a.anneal(hot);
        assert_incremental_matches_full(&mut a.model, name);
    }
}

#[test]
fn the_path_cost_charges_a_scattered_net_for_its_detours_a_column_only_for_its_length() {
    // The three machines of T2: flush in one column, then at three corners of the same box.
    let j = job("t2");
    let wl = |tun: &PlaceTuning, xs: [i32; 3], ys: [i32; 3]| {
        let mut a = j.annealer(tun, 20, 1);
        a.model.set_all(&xs, &ys, &[0; 3]);
        a.model.wl
    };
    let path = PlaceTuning { belt_cost: "path".to_owned(), ..Default::default() };
    let hpwl = PlaceTuning { belt_cost: "hpwl".to_owned(), q_table: vec![vec![1.0, 1.0], vec![50.0, 1.0]], ..Default::default() };
    let (column_xs, column_ys) = ([8; 3], [2, 5, 8]);
    let (corner_xs, corner_ys) = ([2, 15, 2], [2, 2, 15]);
    let column = wl(&path, column_xs, column_ys);
    let scattered = wl(&path, corner_xs, corner_ys);
    let hp_column = wl(&hpwl, column_xs, column_ys);
    let hp_scattered = wl(&hpwl, corner_xs, corner_ys);
    println!("path: column {column} scattered {scattered}; hpwl: column {hp_column} scattered {hp_scattered}");
    assert!(column < scattered);
    // The column's machines share a corridor: no detours, so the path cost adds nothing between them.
    assert!(scattered - column > hp_scattered - hp_column, "the path cost should separate them more than the bounding box");
}

#[test]
fn a_column_start_keeps_its_columns_through_annealing() {
    let tun = PlaceTuning { initial: "columns".to_owned(), column_moves_only: true, ..Default::default() };
    let j = job("t4");
    let mut a = j.annealer(&tun, 20, 3);
    assert!(a.initial_columns(&j.plan));
    assert!(a.structured());
    let xs: HashSet<i32> = a.model.px.iter().copied().collect();
    assert!(xs.len() >= 2, "T4 should need two columns in 20 x 20: {xs:?}");
    a.anneal(AnnealOptions { max_moves: Some(50_000), start_range: Some(3), refine: true, t0_scale: tun.column_t0_scale, ..Default::default() });
    // Whole columns move together, so the machines still stand on as many distinct x positions as columns.
    let after: HashSet<i32> = a.model.px.iter().copied().collect();
    assert_eq!(xs.len(), after.len(), "columns before {xs:?} after {after:?}");
}

#[test]
fn the_row_start_places_every_machine_legally_in_dependency_order() {
    let tun = PlaceTuning::default();
    let j = job("t3");
    let mut a = j.annealer(&tun, 24, 1);
    assert!(a.initial_rows(&j.plan));
    assert!(a.model.placed());
    let p = a.model.placement();
    assert_eq!(p.machines.len(), j.plan.machines.len());
    // Rows are filled in recipe-graph depth order: a machine never sits above one that is further upstream.
    let depth = factorio_optimizer::place::recipe_depth(&j.plan);
    let d = |id: usize| depth.get(&j.plan.machines[id].product).copied().unwrap_or(0);
    for a_ in &p.machines {
        for b in &p.machines {
            if d(a_.id) < d(b.id) {
                assert!(a_.y <= b.y, "machine {} (depth {}) is below machine {} (depth {})", a_.id, d(a_.id), b.id, d(b.id));
            }
        }
    }
}

#[test]
fn recipe_depth_counts_the_longest_chain_to_a_raw_input() {
    let plan = job("t3").plan;
    let depth = factorio_optimizer::place::recipe_depth(&plan);
    // Plates are raw (no machine), gears and cable are made from plates, circuits from cable, science from those.
    assert_eq!(depth["iron-gear-wheel"], 1);
    assert_eq!(depth["copper-cable"], 1);
    assert_eq!(depth["electronic-circuit"], 2);
    assert_eq!(depth["inserter"], 3);
    assert_eq!(depth["automation-science-pack"], 2);
    assert_eq!(depth["logistic-science-pack"], 4);
    assert!(!depth.contains_key("iron-plate"));
}

#[test]
fn a_placement_can_be_restored_and_an_unplaceable_one_is_refused() {
    let tun = PlaceTuning::default();
    let j = job("t3");
    let mut a = j.annealer(&tun, 24, 4);
    assert!(a.initial_random());
    let first = a.model.placement();
    let cost = a.model.cost();
    assert!(a.initial_random());
    assert_ne!(a.model.placement(), first, "a second random start lands elsewhere");
    assert!(a.initial_from(&first));
    assert_eq!(a.model.placement(), first);
    assert!((a.model.cost() - cost).abs() <= 1e-9 * cost.abs().max(1.0));
    // Put two machines on top of each other: not legal for the model.
    let mut broken = first.clone();
    broken.machines[1].x = broken.machines[0].x;
    broken.machines[1].y = broken.machines[0].y;
    assert!(!a.initial_from(&broken));
    assert!(!a.model.placed(), "a refused start leaves the model cleared");
}

#[test]
fn the_history_feedback_raises_the_cost_of_busy_bins() {
    let tun = PlaceTuning::default();
    let j = job("t3");
    let mut a = j.annealer(&tun, 24, 2);
    assert!(a.initial_random());
    let before = a.model.cost();
    assert_eq!(a.model.fb, 0.0);
    let history: Vec<(Tile, f64)> = (0..24).flat_map(|x| (0..24).map(move |y| (Tile::new(x, y), 1.0))).collect();
    a.model.set_history(&history, 1.0);
    assert!(a.model.fb > 0.0 && a.model.cost() > before, "{} vs {before}", a.model.cost());
    a.model.set_history(&history, 0.0);
    assert!((a.model.cost() - before).abs() <= 1e-9 * before.abs().max(1.0), "weight 0 turns the feedback off");
}
