//! The Rust router against the Kotlin one. Every case is a placement of a plan plus the router's inputs; Rust must
//! reproduce what Kotlin made of it: first the router's setup (chains, terminals, access options), then the whole
//! result (entities in order, iterations, the score after every iteration, A* expansions, conflicts, history in the
//! order Kotlin iterates it), the validation spec built from it, the validator's verdict and the ring ports.
//! The goldens are dumped by rust/tools/kotlin/GoldenRouteTest.kt; every route ran with the endgame bounded by its
//! round count instead of the clock. ROUTE_CASE=<name> runs one case.

mod common;

use common::{EntityJson, compare, compare_list, dir_of, entity_json, read};

use factorio_optimizer::config::run_configuration::RunConfiguration;
use factorio_optimizer::config::tuning::RouteTuning;
use factorio_optimizer::data::data_model::RootData;
use factorio_optimizer::data::game_constants as gc;
use factorio_optimizer::data::loader::load_root_data;
use factorio_optimizer::geom::{Dir, Tile};
use factorio_optimizer::place::{PlacedMachine, Placement};
use factorio_optimizer::plan::ProductionPlan;
use factorio_optimizer::route::access::{AccessOption, DiLink};
use factorio_optimizer::route::ring_ports::ring_ports;
use factorio_optimizer::route::{PathFinder, RNet, RouteProblem};
use factorio_optimizer::search::trial::{layout_of, validation_spec};
use factorio_optimizer::validate::{MachineSpec, OutputSpec, SourceSpec, validate};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(Deserialize)]
struct PlacedJson {
    id: usize,
    x: i32,
    y: i32,
    rot: usize,
    mirror: bool,
}

#[derive(Deserialize)]
struct Bias {
    x: i32,
    y: i32,
    v: f64,
}

/// A float array as its non-zero entries, with the bits of each (Kotlin's `Float.toRawBits`).
#[derive(Deserialize)]
struct Bits {
    len: usize,
    nonzero: Vec<(usize, i32)>,
}

#[derive(Deserialize)]
struct NetJson {
    idx: i32,
    plan_id: i32,
    item: String,
    terms: Vec<i32>,
    port_tiles: Vec<i32>,
    port_out: Vec<i32>,
    hpwl: i32,
    source_rate: f64,
    out_tiles: Vec<i32>,
    out_out: Vec<i32>,
    out_rate: f64,
    item2: Option<String>,
    source_rate2: f64,
    src_port: i32,
    out_port: i32,
}

#[derive(Deserialize)]
struct VtJson {
    id: usize,
    terminal: usize,
    rate: f64,
    k: i32,
    net: i32,
    slot: i32,
}

/// [terminal, belt x, belt y, inserter x, inserter y, inserter dir, long, trick, side]
type OptionJson = (usize, i32, i32, i32, i32, String, bool, String, String);

#[derive(Deserialize)]
struct SinkOptionsJson {
    terminal: usize,
    options: Vec<OptionJson>,
}

#[derive(Deserialize)]
struct SetupJson {
    nets: Vec<NetJson>,
    vts: Vec<VtJson>,
    options: Vec<OptionJson>,
    sink_options: Vec<SinkOptionsJson>,
}

#[derive(Deserialize)]
struct ConflictNetsJson {
    tile: Tile,
    nets: Vec<i32>,
}

#[derive(Deserialize)]
struct DiJson {
    producer: usize,
    consumer: usize,
    item: String,
    flow: f64,
    tiles: Vec<Tile>,
    long: bool,
    ins_dir: String,
}

#[derive(Deserialize)]
struct StatsJson {
    belts: usize,
    undergrounds: usize,
    inserters: usize,
}

#[derive(Deserialize)]
struct ChainStatJson {
    item: String,
    terminals: usize,
    hpwl: i32,
    tiles: usize,
}

#[derive(Deserialize)]
struct HistoryJson {
    x: i32,
    y: i32,
    v: f64,
}

#[derive(Deserialize)]
struct ResultJson {
    success: bool,
    iterations: i32,
    overuse: i32,
    chains: usize,
    expansions: u64,
    failed_nets: Vec<i32>,
    trace: Vec<i32>,
    log: Vec<String>,
    entities: Vec<EntityJson>,
    sources: Vec<SourceSpec>,
    outputs: Vec<OutputSpec>,
    conflicts: Vec<Tile>,
    conflict_nets: Vec<ConflictNetsJson>,
    di: Vec<DiJson>,
    stats: StatsJson,
    chain_stats: Vec<ChainStatJson>,
    history: Vec<HistoryJson>,
    raw_history: Bits,
}

#[derive(Deserialize)]
struct MachineJson {
    tile: Tile,
    spec: MachineSpec,
}

#[derive(Deserialize)]
struct SpecJson {
    pole_ring: i32,
    require_power: bool,
    fluid_mode: String,
    check_rates: bool,
    inserter_rate: HashMap<String, [f64; 2]>,
    machines: Vec<MachineJson>,
    outside_allowed: Vec<Tile>,
}

#[derive(Deserialize)]
struct RingedJson {
    added: Vec<EntityJson>,
    sources: Vec<SourceSpec>,
    outputs: Vec<OutputSpec>,
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
struct Golden {
    config: String,
    plan: String,
    w: i32,
    h: i32,
    placement: Vec<PlacedJson>,
    obstacles: Vec<Tile>,
    tuning: RouteTuning,
    tile_bias: Vec<Bias>,
    warm_history: Option<Bits>,
    endgame_seed: i64,
    no_tunnel: Vec<Tile>,
    setup: SetupJson,
    result: ResultJson,
    spec: SpecJson,
    ringed: RingedJson,
    errors: Vec<ErrorJson>,
}

fn golden_dir() -> PathBuf {
    common::repo_root().join("rust/tests/golden/route")
}

fn option_json(o: &AccessOption) -> OptionJson {
    (o.terminal, o.belt_tile.x, o.belt_tile.y, o.ins_tile.x, o.ins_tile.y, format!("{:?}", o.ins_dir), o.long, o.trick.to_owned(), format!("{:?}", o.side))
}

/// The nonzero entries of a float array with their bits, like the dump.
fn bits_of(values: &[f32]) -> Bits {
    Bits { len: values.len(), nonzero: values.iter().enumerate().filter(|(_, v)| **v != 0.0).map(|(i, v)| (i, v.to_bits() as i32)).collect() }
}

fn run_case(name: &str, data: &RootData) -> Vec<String> {
    let dir = golden_dir();
    let g: Golden = serde_json::from_str(&read(dir.join("cases").join(format!("{name}.json")))).unwrap_or_else(|e| panic!("{name}: {e}"));
    let plan: ProductionPlan = serde_json::from_str(&read(dir.join(&g.plan))).unwrap();
    let config = RunConfiguration::load(dir.join(&g.config).to_str().unwrap()).unwrap();
    let placement = Placement {
        w: g.w,
        h: g.h,
        machines: g.placement.iter().map(|m| PlacedMachine { id: m.id, x: m.x, y: m.y, rot: Dir::ALL[m.rot], mirror: m.mirror }).collect(),
    };
    let mut problem = RouteProblem::new(&plan, &config, g.tuning.clone(), &placement);
    problem.obstacles = g.obstacles.iter().copied().collect();
    problem.tile_bias = g.tile_bias.iter().map(|b| (Tile::new(b.x, b.y), b.v)).collect();
    problem.warm_history = g.warm_history.as_ref().map(|w| {
        let mut h = vec![0f32; w.len];
        for &(i, bits) in &w.nonzero {
            h[i] = f32::from_bits(bits as u32);
        }
        h
    });
    problem.endgame_seed = g.endgame_seed;
    problem.no_tunnel = g.no_tunnel.iter().copied().collect();

    let mut out: Vec<String> = Vec::new();
    let mut pf = PathFinder::new(&problem);

    // Setup first: a mistake here would otherwise show up as a mystery in the routes.
    let nets: Vec<RNet> = g
        .setup
        .nets
        .iter()
        .map(|n| RNet {
            idx: n.idx,
            plan_id: n.plan_id,
            item: n.item.clone(),
            terms: n.terms.clone(),
            port_tiles: n.port_tiles.clone(),
            port_out: n.port_out.clone(),
            hpwl: n.hpwl,
            source_rate: n.source_rate,
            out_tiles: n.out_tiles.clone(),
            out_out: n.out_out.clone(),
            out_rate: n.out_rate,
            item2: n.item2.clone(),
            source_rate2: n.source_rate2,
            src_port: n.src_port,
            out_port: n.out_port,
        })
        .collect();
    compare_list("setup: chains", pf.nets(), &nets, &mut out);
    let terminals: Vec<(usize, usize, f64, i32, i32, i32)> = pf.chain_terminals().iter().enumerate().map(|(i, v)| (i, v.base.id, v.rate, v.k, v.net, v.slot)).collect();
    let want_terminals: Vec<(usize, usize, f64, i32, i32, i32)> = g.setup.vts.iter().map(|v| (v.id, v.terminal, v.rate, v.k, v.net, v.slot)).collect();
    compare_list("setup: chain terminals (id, terminal, rate, k, net, slot)", &terminals, &want_terminals, &mut out);
    let options: Vec<OptionJson> = pf.options().iter().map(option_json).collect();
    compare_list("setup: access options", &options, &g.setup.options, &mut out);
    let mut sinks: Vec<(usize, Vec<OptionJson>)> = pf.sink_options().into_iter().map(|(t, o)| (t, o.iter().map(option_json).collect())).collect();
    sinks.sort_by_key(|s| s.0);
    let mut want_sinks: Vec<(usize, Vec<OptionJson>)> = g.setup.sink_options.iter().map(|s| (s.terminal, s.options.clone())).collect();
    want_sinks.sort_by_key(|s| s.0);
    compare_list("setup: sink options", &sinks, &want_sinks, &mut out);
    if !out.is_empty() {
        return out; // everything after this follows from the setup
    }

    let r = pf.run();
    let want = &g.result;
    compare("success", &r.success, &want.success, &mut out);
    compare("iterations", &r.iterations, &want.iterations, &mut out);
    compare_list("trace (score after each iteration)", &r.trace, &want.trace, &mut out);
    compare("expansions", &r.expansions, &want.expansions, &mut out);
    compare("overuse", &r.overuse, &want.overuse, &mut out);
    compare("chains", &r.chains, &want.chains, &mut out);
    compare_list("failed nets", &r.failed_nets, &want.failed_nets, &mut out);
    let entities: Vec<EntityJson> = r.entities.iter().map(entity_json).collect();
    compare_list("entities", &entities, &want.entities, &mut out);
    compare_list("sources", &r.sources, &want.sources, &mut out);
    compare_list("outputs", &r.outputs, &want.outputs, &mut out);
    compare_list("conflicts", &r.conflicts, &want.conflicts, &mut out);
    let conflict_nets: Vec<(Tile, Vec<i32>)> = r.conflict_nets.iter().map(|(t, ids)| (*t, ids.clone())).collect();
    let want_conflict_nets: Vec<(Tile, Vec<i32>)> = want.conflict_nets.iter().map(|c| (c.tile, c.nets.clone())).collect();
    compare_list("conflict nets", &conflict_nets, &want_conflict_nets, &mut out);
    let di: Vec<DiLink> = want
        .di
        .iter()
        .map(|d| DiLink { producer: d.producer, consumer: d.consumer, item: d.item.clone(), flow: d.flow, tiles: d.tiles.clone(), long: d.long, ins_dir: dir_of(&d.ins_dir) })
        .collect();
    compare_list("direct insertion", &r.di, &di, &mut out);
    compare("stats (belts, undergrounds, inserters)", &(r.stats.belts, r.stats.undergrounds, r.stats.inserters), &(want.stats.belts, want.stats.undergrounds, want.stats.inserters), &mut out);
    let chain_stats: Vec<(String, usize, i32, usize)> = r.chain_stats.iter().map(|c| (c.item.clone(), c.terminals, c.hpwl, c.tiles)).collect();
    let want_chain_stats: Vec<(String, usize, i32, usize)> = want.chain_stats.iter().map(|c| (c.item.clone(), c.terminals, c.hpwl, c.tiles)).collect();
    compare_list("chain stats", &chain_stats, &want_chain_stats, &mut out);
    let history: Vec<(i32, i32, f64)> = r.history.iter_java().map(|(t, v)| (t.x, t.y, *v)).collect();
    let want_history: Vec<(i32, i32, f64)> = want.history.iter().map(|h| (h.x, h.y, h.v)).collect();
    compare_list("history (in Kotlin's hash map order)", &history, &want_history, &mut out);
    let raw = bits_of(&r.raw_history);
    compare("raw history length", &raw.len, &want.raw_history.len, &mut out);
    compare_list("raw history (index, bits)", &raw.nonzero, &want.raw_history.nonzero, &mut out);
    compare_list("log", &r.log, &want.log, &mut out);

    // The spec the validator is given and what it says about the result.
    let spec = validation_spec(data, &plan, &config, &placement, &r.sources, false, &HashSet::new(), &r.outputs);
    let ws = &g.spec;
    compare("spec: pole ring", &spec.pole_ring, &ws.pole_ring, &mut out);
    compare("spec: require power", &spec.require_power, &ws.require_power, &mut out);
    compare("spec: fluid mode", &spec.fluid_mode, &ws.fluid_mode, &mut out);
    compare("spec: check rates", &spec.check_rates, &ws.check_rates, &mut out);
    let machines: HashMap<Tile, MachineSpec> = ws.machines.iter().map(|m| (m.tile, m.spec.clone())).collect();
    if spec.machines != machines {
        let bad: Vec<Tile> = machines.keys().filter(|t| spec.machines.get(t) != machines.get(t)).copied().collect();
        out.push(format!("spec: machines differ at {bad:?} ({} vs {})", spec.machines.len(), machines.len()));
    }
    let mut outside: Vec<Tile> = spec.outside_allowed.iter().copied().collect();
    outside.sort_by_key(|t| (t.y, t.x));
    compare_list("spec: outside allowed", &outside, &ws.outside_allowed, &mut out);
    for (inserter, [plain, from_belt]) in &ws.inserter_rate {
        compare(&format!("spec: {inserter} rate"), &[(spec.inserter_rate)(inserter, false), (spec.inserter_rate)(inserter, true)], &[*plain, *from_belt], &mut out);
    }
    let errors: Vec<String> = if r.success {
        let layout = layout_of(&placement, r.entities.clone(), Vec::new(), "factorio-optimizer");
        validate(&layout, &spec).iter().map(|e| format!("{} {},{} {}", e.kind.name(), e.tile.x, e.tile.y, e.message)).collect()
    } else {
        Vec::new()
    };
    let want_errors: Vec<String> = g.errors.iter().map(|e| format!("{} {},{} {}", e.kind, e.x, e.y, e.message)).collect();
    compare_list("validator errors", &errors, &want_errors, &mut out);

    // Ports moved onto the ring.
    let tier = gc::belt_tier(&config.belt_tier).unwrap();
    let ringed = ring_ports(&r.entities, &r.sources, &r.outputs, placement.w, placement.h, tier.belt);
    let added: Vec<EntityJson> = ringed.entities[r.entities.len()..].iter().map(entity_json).collect();
    compare_list("ring ports: added entities", &added, &g.ringed.added, &mut out);
    compare_list("ring ports: sources", &ringed.sources, &g.ringed.sources, &mut out);
    compare_list("ring ports: outputs", &ringed.outputs, &g.ringed.outputs, &mut out);
    out
}

fn case_names() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(golden_dir().join("cases"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Every `shards`-th case, so that the shards run side by side.
fn run_shard(shard: usize, shards: usize) {
    let data = load_root_data("recipes.json").unwrap();
    let names = case_names();
    assert!(names.len() >= 77, "only {} route cases in the goldens", names.len());
    let only = std::env::var("ROUTE_CASE").ok();
    let mut failures: Vec<String> = Vec::new();
    let mut ran = 0;
    for (i, name) in names.iter().enumerate() {
        let mine = match &only {
            Some(o) => o == name,
            None => i % shards == shard,
        };
        if !mine {
            continue;
        }
        ran += 1;
        let diffs = run_case(name, &data);
        if !diffs.is_empty() {
            failures.push(format!("{name}:\n    {}", diffs.join("\n    ")));
        }
    }
    assert!(failures.is_empty(), "{} of {ran} cases differ from Kotlin:\n{}", failures.len(), failures.join("\n"));
}

#[test]
fn route_cases_0() {
    run_shard(0, 6);
}

#[test]
fn route_cases_1() {
    run_shard(1, 6);
}

#[test]
fn route_cases_2() {
    run_shard(2, 6);
}

#[test]
fn route_cases_3() {
    run_shard(3, 6);
}

#[test]
fn route_cases_4() {
    run_shard(4, 6);
}

#[test]
fn route_cases_5() {
    run_shard(5, 6);
}
