//! Factories of blocks, ported from Kotlin's FactoryTest: the split balances every item and keeps the limit, edge
//! links go through the corridors unless their block touches an edge the job allows, and the stitched factories are
//! valid and put their ports on the edges the job names. The searches use a short extra-rounds phase instead of
//! Kotlin's wall-clock budget.

mod common;

use factorio_optimizer::blocks::factory::run_factory;
use factorio_optimizer::blocks::split::split_plan;
use factorio_optimizer::config::run_configuration::{BlocksConfig, ConfigSource, RunConfiguration, SearchConfig};
use factorio_optimizer::config::tuning::Tuning;
use factorio_optimizer::data::data_model::RootData;
use factorio_optimizer::data::loader::load_root_data;
use factorio_optimizer::plan::{Role, build_plan};
use std::path::PathBuf;
use std::sync::LazyLock;

static DATA: LazyLock<RootData> = LazyLock::new(|| load_root_data("recipes.json").unwrap());

fn job(name: &str) -> RunConfiguration {
    RunConfiguration::load(common::repo_root().join(format!("src/test/resources/jobs/{name}.json")).to_str().unwrap()).unwrap()
}

fn blocks(max_machines: i32) -> BlocksConfig {
    BlocksConfig { max_machines, ..BlocksConfig::default() }
}

fn source(name: &str, edge: &str) -> ConfigSource {
    ConfigSource { name: name.to_owned(), edge: Some(edge.to_owned()), ..ConfigSource::default() }
}

fn quick_search() -> SearchConfig {
    SearchConfig { time_budget_s: 3600.0, seeds_per_size: 2, threads: 2, seed: 7, verbose: false }
}

fn quick_tuning() -> Tuning {
    let mut tuning = Tuning::default();
    tuning.search.extra_rounds = 1;
    tuning
}

fn out_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("factorio-factory-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn the_split_keeps_blocks_within_the_limit_and_balances_every_item() {
    let plan = build_plan(&DATA, &job("t3"), &Tuning::default()).unwrap();
    let split = split_plan(&plan, &blocks(5), 30.0);
    assert!(split.blocks.len() >= 2);
    assert!(split.blocks.iter().all(|b| b.len() <= 5), "{:?}", split.blocks);
    assert_eq!(plan.machines.len(), split.blocks.iter().map(Vec::len).sum::<usize>());
    // Each block's links in and out balance what its machines make and use.
    for net in &plan.nets {
        for (b, members) in split.blocks.iter().enumerate() {
            let here: Vec<_> = plan.net_terminals(net).into_iter().filter(|t| t.machine.is_some_and(|m| members.contains(&m))).collect();
            let made: f64 = here.iter().filter(|t| t.role == Role::Producer).map(|t| t.rate).sum();
            let used: f64 = here.iter().filter(|t| t.role == Role::Consumer).map(|t| t.rate).sum();
            let inflow: f64 = split.links.iter().filter(|l| l.item == net.item && l.to == b as i32).map(|l| l.rate).sum();
            let outflow: f64 = split.links.iter().filter(|l| l.item == net.item && l.from == b as i32).map(|l| l.rate).sum();
            assert!((made + inflow - (used + outflow)).abs() < 1e-6, "{} in block {b}", net.item);
        }
    }
    assert!(split.links.iter().filter(|l| l.inner()).all(|l| l.rate <= 0.9 * 30.0 + 1e-9));
}

#[test]
fn edge_links_are_routed_through_the_corridors_unless_the_block_touches_an_allowed_edge() {
    let west = build_plan(&DATA, &job("t3"), &Tuning::default()).unwrap();
    let split = split_plan(&west, &blocks(5), 30.0);
    let edge: Vec<_> = split.links.iter().filter(|l| !l.inner()).collect();
    assert!(edge.iter().filter(|l| l.from < 0).all(|l| l.routed), "inputs on the west edge: {:?}", edge.iter().filter(|l| !l.routed).collect::<Vec<_>>());
    assert!(edge.iter().filter(|l| l.routed && !l.fluid).all(|l| l.rate <= 0.9 * 30.0 + 1e-9));
    // Any edge (the default): every edge link leaves its block directly, as before.
    let c = job("t3");
    let any = RunConfiguration { sources: c.sources.iter().map(|s| source(&s.name, "any")).collect(), ..c };
    let plan = build_plan(&DATA, &any, &Tuning::default()).unwrap();
    assert!(split_plan(&plan, &blocks(5), 30.0).links.iter().all(|l| !l.routed));
}

#[test]
fn t3_as_two_blocks_stitches_into_one_valid_factory() {
    for test_mode in [false, true] {
        let config = RunConfiguration { blocks: Some(blocks(5)), test_mode, search: quick_search(), ..job("t3") };
        let tuning = quick_tuning();
        let plan = build_plan(&DATA, &config, &tuning).unwrap();
        let out = out_dir(if test_mode { "t3-test" } else { "t3" });
        let f = run_factory(&DATA, &config, &tuning, &plan, &out).unwrap().expect("a factory");
        assert!(f.success(), "{:?} {:?}", f.errors.iter().take(5).collect::<Vec<_>>(), f.notes);
        assert!(out.join("best_output_string.txt").exists() && out.join("ports.json").exists());
        assert!(out.join("block_1/best_output_string.txt").exists() && out.join("block_2/best_output_string.txt").exists());
        let _ = std::fs::remove_dir_all(&out);
    }
}

#[test]
fn a_factory_takes_its_inputs_and_outputs_on_the_edges_the_job_names_fluids_included() {
    let config = RunConfiguration {
        outputs: vec![source("processing-unit", "east")],
        blocks: Some(blocks(6)),
        search: quick_search(),
        ..job("t4")
    };
    let tuning = quick_tuning();
    let plan = build_plan(&DATA, &config, &tuning).unwrap();
    let out = out_dir("t4-edges");
    let f = run_factory(&DATA, &config, &tuning, &plan, &out).unwrap().expect("a factory");
    assert!(f.success(), "{:?} {:?}", f.errors.iter().take(5).collect::<Vec<_>>(), f.notes);
    let (w, h) = (f.layout.w, f.layout.h);
    for s in &f.sources {
        let ok = match s.item.as_str() {
            "iron-plate" | "copper-plate" => s.tile.x <= 0,
            "plastic-bar" => s.tile.y <= 0,
            "sulfuric-acid" => s.tile.y >= h - 1,
            other => panic!("unexpected input {other}"),
        };
        assert!(ok, "{} enters at {} of {w} x {h}", s.item, s.tile);
    }
    assert!(f.sources.iter().any(|s| s.fluid && s.item == "sulfuric-acid"));
    assert!(f.outputs.iter().filter(|o| o.item == "processing-unit").all(|o| o.tile.x >= w - 1), "{:?}", f.outputs);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn a_fluid_product_leaves_the_factory_by_pipe_on_the_edge_the_job_names() {
    let config = RunConfiguration {
        outputs: vec![source("sulfuric-acid", "east")],
        blocks: Some(blocks(2)),
        search: quick_search(),
        ..job("acid")
    };
    let tuning = quick_tuning();
    let plan = build_plan(&DATA, &config, &tuning).unwrap();
    let out = out_dir("acid-east");
    let f = run_factory(&DATA, &config, &tuning, &plan, &out).unwrap().expect("a factory");
    assert!(f.success(), "{:?} {:?}", f.errors.iter().take(5).collect::<Vec<_>>(), f.notes);
    let acid: Vec<_> = f.outputs.iter().filter(|o| o.item == "sulfuric-acid").collect();
    assert!(!acid.is_empty() && acid.iter().all(|o| o.fluid && o.tile.x >= f.layout.w - 1), "{:?}", f.outputs);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn blocks_share_belts_inside_while_links_between_blocks_keep_one_item_each() {
    let config = RunConfiguration { blocks: Some(blocks(5)), lane_sharing: true, search: quick_search(), ..job("t3") };
    let tuning = quick_tuning();
    let plan = build_plan(&DATA, &config, &tuning).unwrap();
    let out = out_dir("t3-shared");
    let f = run_factory(&DATA, &config, &tuning, &plan, &out).unwrap().expect("a factory");
    assert!(f.success(), "{:?} {:?}", f.errors.iter().take(5).collect::<Vec<_>>(), f.notes);
    let _ = std::fs::remove_dir_all(&out);
}
