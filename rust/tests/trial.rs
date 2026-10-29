//! Full trials, ported from Kotlin's TrialTest and the layout half of SpaceAgeTest: jobs that must place, route,
//! get power and validate, and what their layouts must contain (infinity pipes, chests, ports on the ring, lanes,
//! filters). They need no goldens, so they also hold for any plan the Rust planner makes.

mod common;

use factorio_optimizer::config::run_configuration::{ConfigSource, ConfigTarget, FluidConfig, ReservedArea, ResearchConfig, RunConfiguration};
use factorio_optimizer::config::tuning::Tuning;
use factorio_optimizer::data::data_model::RootData;
use factorio_optimizer::data::loader::load_root_data;
use factorio_optimizer::geom::{Kind, Rect, Tile};
use factorio_optimizer::plan::build_plan;
use factorio_optimizer::search::search::port_manifest;
use factorio_optimizer::search::trial::{TrialOptions, TrialResult, run_trial, validation_spec};
use factorio_optimizer::validate::{ErrorType, validate};
use std::collections::HashSet;
use std::sync::LazyLock;

static DATA: LazyLock<RootData> = LazyLock::new(|| load_root_data("recipes.json").unwrap());

fn job(name: &str) -> RunConfiguration {
    RunConfiguration::load(common::repo_root().join(format!("src/test/resources/jobs/{name}.json")).to_str().unwrap()).unwrap()
}

fn source(name: &str, edge: &str) -> ConfigSource {
    ConfigSource { name: name.to_owned(), edge: Some(edge.to_owned()), ..ConfigSource::default() }
}

fn target(name: &str, amount: f64) -> ConfigTarget {
    ConfigTarget { name: name.to_owned(), amount }
}

fn trial(config: &RunConfiguration, size: i32, seed: i64) -> TrialResult {
    let tuning = Tuning::default();
    let plan = build_plan(&DATA, config, &tuning).unwrap();
    run_trial(&DATA, &plan, config, &tuning, size, size, seed, TrialOptions::default()).unwrap()
}

/// Like the search: several seeds, the first success counts.
fn any_seed(config: &RunConfiguration, size: i32, seeds: std::ops::RangeInclusive<i64>) -> TrialResult {
    seeds.map(|seed| trial(config, size, seed)).find(|r| r.success).unwrap_or_else(|| trial(config, size, 1))
}

fn why(r: &TrialResult) -> String {
    format!("failed at {}: {:?} {:?}", r.stage, r.errors.iter().take(5).collect::<Vec<_>>(), r.notes.iter().take(2).collect::<Vec<_>>())
}

fn count(r: &TrialResult, kinds: &[Kind]) -> usize {
    r.layout.as_ref().unwrap().entities.iter().filter(|e| kinds.contains(&e.kind)).count()
}

#[test]
fn t1_to_t3_complete_a_full_trial_at_a_comfortable_size() {
    for (name, size) in [("t1", 10), ("t2", 14), ("t3", 24)] {
        let r = trial(&job(name), size, 1);
        assert!(r.success, "{name} {}", why(&r));
    }
}

#[test]
fn t4_validates_with_an_infinity_pipe_on_every_acid_consumer() {
    let config = RunConfiguration { fluids: FluidConfig { mode: "infinity-pipe".to_owned() }, ..job("t4") };
    let r = any_seed(&config, 24, 1..=8);
    assert!(r.success, "{}", why(&r));
    let layout = r.layout.as_ref().unwrap();
    let pipes: Vec<_> = layout.entities.iter().filter(|e| e.kind == Kind::InfinityPipe).collect();
    let acid: Vec<_> = layout.entities.iter().filter(|e| e.kind == Kind::Machine && e.recipe.as_deref() == Some("processing-unit")).collect();
    assert_eq!(acid.len(), pipes.len());
    assert!(pipes.iter().all(|p| p.fluid.as_deref() == Some("sulfuric-acid")));
    for m in &acid {
        assert!(pipes.iter().any(|p| p.tile() == m.rect().side_middle(m.dir, 1)), "pipe on the input side of {}", m.tile());
    }
    for (i, a) in pipes.iter().enumerate() {
        for b in &pipes[i + 1..] {
            assert!(a.tile().manhattan(b.tile()) > 1);
        }
    }
}

#[test]
fn test_mode_feeds_every_source_from_infinity_chests() {
    let config = RunConfiguration { test_mode: true, fluids: FluidConfig { mode: "infinity-pipe".to_owned() }, ..job("t4") };
    let r = any_seed(&config, 24, 1..=8);
    assert!(r.success, "{}", why(&r));
    let items: HashSet<&str> = r.layout.as_ref().unwrap().entities.iter().filter(|e| e.kind == Kind::InfinityChest).filter_map(|e| e.item.as_deref()).collect();
    assert_eq!(items, HashSet::from(["iron-plate", "copper-plate", "plastic-bar"]));
}

#[test]
fn t4_routes_acid_in_pipes_from_its_port() {
    let r = any_seed(&job("t4"), 26, 1..=8);
    assert!(r.success, "{}", why(&r));
    assert_eq!(count(&r, &[Kind::InfinityPipe]), 0);
    assert!(count(&r, &[Kind::Pipe, Kind::PipeUg]) >= 2);
}

#[test]
fn a_plastic_block_pipes_petroleum_from_refineries_to_chemical_plants() {
    let config = RunConfiguration { targets: vec![target("plastic-bar", 2.0)], sources: vec![source("crude-oil", "west"), source("coal", "north")], ..RunConfiguration::default() };
    let r = any_seed(&config, 24, 1..=8);
    assert!(r.success, "no seed routed the plastic block: {}", why(&r));
    assert!(count(&r, &[Kind::Pipe, Kind::PipeUg]) > 0);
    assert_eq!(count(&r, &[Kind::InfinityPipe]), 0);
}

#[test]
fn an_item_above_one_belt_is_carried_on_several_independent_belts() {
    // Inserter capacity research 2 (hand size 2): two fast inserters per gear machine instead of four.
    let config = RunConfiguration {
        targets: vec![target("iron-gear-wheel", 20.0)],
        sources: vec![source("iron-plate", "west")],
        research: ResearchConfig { inserter_capacity: 2, ..ResearchConfig::default() },
        ..RunConfiguration::default()
    };
    let plan = build_plan(&DATA, &config, &Tuning::default()).unwrap();
    assert!((plan.raw_rates["iron-plate"] - 40.0).abs() < 1e-9);
    assert!(plan.terminals.iter().filter(|t| t.item == "iron-plate" && t.machine.is_some()).all(|t| t.k == 2));
    let r = any_seed(&config, 28, 1..=6);
    assert!(r.success, "no seed routed 40 iron/s: {}", why(&r));
    let chains = r.route.as_ref().unwrap().chain_stats.iter().filter(|c| c.item == "iron-plate").count();
    assert!(chains >= 2, "iron on {chains} belts");
    assert_eq!(chains, r.sources.iter().filter(|s| s.item == "iron-plate").count(), "each belt has its own port tile");
}

#[test]
fn products_leave_through_an_output_port_and_the_test_harness_empties_it() {
    let config = RunConfiguration { test_mode: true, ..job("t1") };
    let r = any_seed(&config, 10, 1..=8);
    assert!(r.success, "{}", why(&r));
    assert_eq!(r.outputs.len(), 1);
    let out = &r.outputs[0];
    assert_eq!(out.item, "iron-gear-wheel");
    assert!((out.rate - 1.0).abs() < 1e-9);
    let layout = r.layout.as_ref().unwrap();
    let belts: Vec<_> = layout.entities.iter().filter(|e| e.kind.belt_family() && e.tile() == out.tile).collect();
    assert_eq!(belts.len(), 1);
    let beyond = belts[0].tile() + belts[0].dir;
    assert!(beyond.x < 0 || beyond.x >= r.w || beyond.y < 0 || beyond.y >= r.h, "the port belt leads out of the box");
    assert!(layout.entities.iter().any(|e| e.kind == Kind::InfinityChest && e.item.is_none()), "void chests take the gears");
    assert_eq!(count(&r, &[Kind::Chest]), 0);
    assert_eq!(port_manifest(&r)["outputs"][0]["item"], "iron-gear-wheel");
}

#[test]
fn a_fluid_product_leaves_through_a_pipe_to_its_output_port() {
    let r = any_seed(&job("acid"), 14, 1..=8);
    assert!(r.success, "{}", why(&r));
    assert_eq!(r.outputs.len(), 1);
    let out = &r.outputs[0];
    assert!(out.fluid);
    assert_eq!(out.item, "sulfuric-acid");
    assert!(r.layout.as_ref().unwrap().entities.iter().any(|e| e.kind == Kind::Pipe && e.tile() == out.tile));
    let t = any_seed(&RunConfiguration { test_mode: true, ..job("acid") }, 14, 1..=8);
    assert!(t.success, "test mode {}", why(&t));
    assert!(t.layout.as_ref().unwrap().entities.iter().any(|e| e.kind == Kind::InfinityPipe && e.drain && e.fluid.as_deref() == Some("sulfuric-acid")));
}

#[test]
fn two_inputs_share_one_belt_a_lane_each_and_the_harness_feeds_each_lane() {
    let shared = RunConfiguration { test_mode: true, lane_sharing: true, ..job("pair") };
    let r = any_seed(&shared, 14, 1..=8);
    assert!(r.success, "{}", why(&r));
    let tiles: HashSet<Tile> = r.sources.iter().map(|s| s.tile).collect();
    assert_eq!(tiles.len(), 1, "one shared input belt: {:?}", r.sources);
    assert_eq!(r.sources.iter().map(|s| s.lane).collect::<HashSet<_>>(), HashSet::from([0, 1]));
    let lanes = port_manifest(&r)["inputs"].to_string();
    assert!(lanes.contains("\"lane\":\"left\"") && lanes.contains("\"lane\":\"right\""), "{lanes}");
    // Without lane sharing the two items need two belts.
    let apart = any_seed(&RunConfiguration { test_mode: true, lane_sharing: false, ..job("pair") }, 14, 1..=8);
    assert!(apart.success);
    assert_eq!(apart.sources.iter().map(|s| s.tile).collect::<HashSet<_>>().len(), 2);
}

#[test]
fn ports_sit_on_the_ring_outside_the_box_and_reserved_areas_stay_empty_and_marked() {
    let config = RunConfiguration { reserved: vec![ReservedArea { w: 5, h: 4, x: None, y: None, corner: None }], ..job("t3") };
    let r = any_seed(&config, 24, 1..=8);
    assert!(r.success, "{}", why(&r));
    let inside = |t: Tile| t.x >= 0 && t.x < r.w && t.y >= 0 && t.y < r.h;
    assert!(!r.sources.is_empty() && r.sources.iter().all(|s| !inside(s.tile)), "inputs on the ring: {:?}", r.sources);
    assert!(!r.outputs.is_empty() && r.outputs.iter().all(|o| !inside(o.tile)), "outputs on the ring: {:?}", r.outputs);
    // The reserved area (south-east corner by default) holds nothing and has a marker border.
    let area = Rect::new(r.w - 5, r.h - 4, 5, 4);
    let layout = r.layout.as_ref().unwrap();
    assert!(layout.entities.iter().all(|e| e.rect().tiles().iter().all(|t| !area.contains_tile(*t))), "reserved area is empty");
    let border = area.tiles().iter().filter(|t| t.x == area.x || t.y == area.y || t.x == area.x2() || t.y == area.y2()).count();
    assert_eq!(border, layout.tiles.len());
    // Marker tiles survive a blueprint round trip.
    let back = layout.to_blueprint(&config.quality).to_layout(r.w, r.h).unwrap();
    let (mine, theirs): (HashSet<_>, HashSet<_>) = (layout.tiles.iter().collect(), back.tiles.iter().collect());
    assert_eq!(mine, theirs);
}

#[test]
fn removing_a_pole_from_a_valid_result_fails_validation() {
    let config = job("t3");
    let tuning = Tuning::default();
    let plan = build_plan(&DATA, &config, &tuning).unwrap();
    let r = run_trial(&DATA, &plan, &config, &tuning, 24, 24, 2, TrialOptions::default()).unwrap();
    assert!(r.success, "failed at {}", r.stage);
    let layout = r.layout.as_ref().unwrap();
    let poles: Vec<usize> = (0..layout.entities.len()).filter(|&i| layout.entities[i].kind == Kind::Pole).collect();
    assert!(!poles.is_empty());
    let outside: HashSet<Tile> = layout.entities.iter().flat_map(|e| e.rect().tiles()).filter(|t| t.x < 0 || t.x >= 24 || t.y < 0 || t.y >= 24).collect();
    let spec = validation_spec(&DATA, &plan, &config, r.placement.as_ref().unwrap(), &r.sources, true, &outside, &r.outputs);
    assert!(validate(layout, &spec).is_empty());
    // Every pole is needed: dropping any single one leaves something unpowered or the rest disconnected.
    let failing = poles
        .iter()
        .filter(|&&drop| {
            let keep: Vec<usize> = (0..layout.entities.len()).filter(|&i| i != drop).collect();
            let remap: std::collections::HashMap<usize, usize> = keep.iter().enumerate().map(|(new, &old)| (old, new)).collect();
            let wires = layout.wires.iter().filter(|w| w.0 != drop && w.1 != drop).map(|w| (remap[&w.0], remap[&w.1])).collect();
            let smaller = factorio_optimizer::geom::Layout { entities: keep.iter().map(|&i| layout.entities[i].clone()).collect(), wires, ..layout.clone() };
            validate(&smaller, &spec).iter().any(|e| e.kind == ErrorType::Unpowered || e.kind == ErrorType::PolesDisconnected)
        })
        .count();
    assert!(failing >= 1, "removing a pole should break power");
}

#[test]
fn fluid_inputs_on_touching_edge_tiles_enter_through_pipe_to_ground() {
    // Pipes on touching tiles join, so the second and third fluid start with a pipe-to-ground open to the outside.
    let config = RunConfiguration::parse(
        r#"{"targets": [{"name": "sulfur", "amount": 2.0}, {"name": "lubricant", "amount": 10.0}],
        "sources": [{"name": "petroleum-gas", "edge": "west", "from": 0, "to": 0}, {"name": "heavy-oil", "edge": "west", "from": 1, "to": 1},
        {"name": "water", "edge": "west", "from": 2, "to": 2}]}"#,
    )
    .unwrap();
    let r = any_seed(&config, 20, 1..=4);
    assert!(r.success, "{}", why(&r));
    let ports = r.layout.as_ref().unwrap().entities.iter().filter(|e| matches!(e.kind, Kind::Pipe | Kind::PipeUg) && e.x == 0 && (0..=2).contains(&e.y)).filter(|e| e.kind == Kind::PipeUg).count();
    assert!(ports >= 1, "pipe-to-ground ports: {ports}");
}

// The layout half of SpaceAgeTest.

#[test]
fn foundries_cast_gears_from_a_molten_iron_pipe() {
    let config = RunConfiguration {
        targets: vec![target("iron-gear-wheel", 4.0)],
        machines: Some(vec!["foundry".to_owned()]),
        sources: vec![source("molten-iron", "west")],
        ..RunConfiguration::default()
    };
    let r = any_seed(&config, 16, 1..=8);
    assert!(r.success, "{}", why(&r));
    let layout = r.layout.as_ref().unwrap();
    assert!(layout.entities.iter().any(|e| e.kind == Kind::Machine && e.name == "foundry"));
    assert!(layout.entities.iter().any(|e| matches!(e.kind, Kind::Pipe | Kind::PipeUg) && e.fluid.as_deref() == Some("molten-iron")));
}

#[test]
fn electromagnetic_plants_make_circuits_and_their_cable() {
    let config = RunConfiguration {
        targets: vec![target("electronic-circuit", 4.0)],
        machines: Some(vec!["electromagnetic-plant".to_owned()]),
        sources: vec![source("iron-plate", "west"), source("copper-plate", "west")],
        ..RunConfiguration::default()
    };
    let r = any_seed(&config, 16, 1..=8);
    assert!(r.success, "{}", why(&r));
    let layout = r.layout.as_ref().unwrap();
    let plants: Vec<_> = layout.entities.iter().filter(|e| e.kind == Kind::Machine).collect();
    assert!(plants.iter().all(|e| e.name == "electromagnetic-plant" && e.w == 4 && e.h == 4));
    // Even sizes sit on whole-tile centres and come back where they were.
    let back = layout.to_blueprint(&config.quality).to_layout(layout.w, layout.h).unwrap();
    let before: HashSet<Tile> = plants.iter().map(|e| e.tile()).collect();
    let after: HashSet<Tile> = back.entities.iter().filter(|e| e.kind == Kind::Machine).map(|e| e.tile()).collect();
    assert_eq!(before, after);
}

#[test]
fn a_crushers_two_products_leave_through_filtered_inserters() {
    let config = RunConfiguration {
        targets: vec![target("iron-ore", 10.0)],
        machines: Some(vec!["crusher".to_owned()]),
        raw_resources: Some(Vec::new()),
        sources: vec![source("metallic-asteroid-chunk", "west")],
        forbidden_recipes: vec!["advanced-metallic-asteroid-crushing".to_owned(), "metallic-asteroid-reprocessing".to_owned()],
        ..RunConfiguration::default()
    };
    let r = any_seed(&config, 12, 1..=8);
    assert!(r.success, "{}", why(&r));
    let layout = r.layout.as_ref().unwrap();
    let crushers: Vec<_> = layout.entities.iter().filter(|e| e.kind == Kind::Machine).collect();
    assert_eq!(crushers.len(), 1);
    let crusher = crushers[0];
    assert_eq!((crusher.w, crusher.h), (2, 3));
    let items = |l: &factorio_optimizer::geom::Layout| -> Vec<Option<String>> {
        let tiles = crusher.rect().tiles();
        let mut found: Vec<Option<String>> = l.entities.iter().filter(|e| e.kind == Kind::Inserter && tiles.contains(&e.pickup_tile())).map(|e| e.item.clone()).collect();
        found.sort();
        found
    };
    let mine = items(layout);
    assert_eq!(mine.iter().flatten().cloned().collect::<HashSet<_>>(), HashSet::from(["iron-ore".to_owned(), "metallic-asteroid-chunk".to_owned()]));
    // The filters survive the blueprint.
    let back = layout.to_blueprint(&config.quality).to_layout(layout.w, layout.h).unwrap();
    assert_eq!(mine, items(&back));
}

#[test]
fn direct_insertion_from_a_machine_with_several_products_counts_each_products_own_output() {
    // Found by JobFuzzTest: the advanced crusher's copper and chunks counted as spare iron ore, so direct
    // insertion promised its foundry more ore than it makes and the second crusher's ore was never routed.
    let config = RunConfiguration {
        targets: vec![target("iron-plate", 0.5), target("electronic-circuit", 0.5)],
        machines: Some(["crusher", "assembling-machine-3", "electric-furnace", "chemical-plant", "foundry"].map(String::from).to_vec()),
        raw_resources: Some(Vec::new()),
        sources: vec![source("metallic-asteroid-chunk", "west"), source("oxide-asteroid-chunk", "north")],
        forbidden_recipes: vec!["metallic-asteroid-reprocessing".to_owned(), "oxide-asteroid-reprocessing".to_owned()],
        ..RunConfiguration::default()
    };
    for seed in 1..=3 {
        let r = trial(&config, 24, seed);
        assert!(r.stage != "validate", "seed {seed}: {:?}", r.errors.iter().take(3).collect::<Vec<_>>());
    }
}

#[test]
fn the_platform_job_builds() {
    let config = RunConfiguration {
        targets: vec![target("iron-gear-wheel", 2.0)],
        machines: Some(["crusher", "electric-furnace", "assembling-machine-3"].map(String::from).to_vec()),
        raw_resources: Some(Vec::new()),
        sources: vec![source("metallic-asteroid-chunk", "west")],
        forbidden_recipes: vec!["advanced-metallic-asteroid-crushing".to_owned(), "metallic-asteroid-reprocessing".to_owned()],
        ..RunConfiguration::default()
    };
    let r = any_seed(&config, 22, 1..=8);
    assert!(r.success, "{}", why(&r));
}
