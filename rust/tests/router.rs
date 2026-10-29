//! The router's own tests, ported from Kotlin's RouterTest and RouterFuzzTest: hand placements that must route and
//! validate, the options that change what gets built (undergrounds, splitters), and random instances of which every
//! success must pass the validator. They need no goldens, so they also hold for any plan the Rust planner makes.

mod common;

use factorio_optimizer::config::run_configuration::{ConfigSource, ConfigTarget, RunConfiguration};
use factorio_optimizer::config::tuning::{RouteTuning, Tuning};
use factorio_optimizer::data::data_model::RootData;
use factorio_optimizer::data::loader::load_root_data;
use factorio_optimizer::geom::{Dir, Kind, Rect};
use factorio_optimizer::place::{PlacedMachine, Placement, PlacementFixture, static_obstacles};
use factorio_optimizer::plan::{MachineInstance, Net, ProductionPlan, Role, Terminal, build_plan};
use factorio_optimizer::route::{RouteProblem, RouteResult, route};
use factorio_optimizer::search::trial::{layout_of, validation_spec};
use factorio_optimizer::util::splittable_random::SplittableRandom;
use factorio_optimizer::validate::{ValidationError, validate};
use indexmap::IndexMap;
use std::collections::HashSet;
use std::sync::LazyLock;
use std::time::Instant;

static DATA: LazyLock<RootData> = LazyLock::new(|| load_root_data("recipes.json").unwrap());

struct Routed {
    plan: ProductionPlan,
    result: RouteResult,
    errors: Vec<ValidationError>,
}

fn resource(name: &str) -> String {
    common::repo_root().join("src/test/resources").join(name).to_str().unwrap().to_owned()
}

/// Routes `placement` and, if that succeeds, validates the layout the way the search does (power off).
fn route_and_validate(config: &RunConfiguration, plan: ProductionPlan, placement: &Placement, tuning: RouteTuning) -> Routed {
    let mut problem = RouteProblem::new(&plan, config, tuning, placement);
    problem.obstacles = static_obstacles(config, &plan, placement.w, placement.h);
    let result = route(&problem);
    let spec = validation_spec(&DATA, &plan, config, placement, &result.sources, false, &HashSet::new(), &result.outputs);
    let errors = if result.success { validate(&layout_of(placement, result.entities.clone(), Vec::new(), "factorio-optimizer"), &spec) } else { Vec::new() };
    Routed { plan, result, errors }
}

fn fixture_with(name: &str, edit: impl FnOnce(&mut RunConfiguration), tuning: RouteTuning) -> Routed {
    let fx = PlacementFixture::load(&resource(&format!("placements/{name}.json"))).unwrap();
    let mut config = RunConfiguration::load(&resource(&fx.job)).unwrap();
    edit(&mut config);
    let plan = build_plan(&DATA, &config, &Tuning::default()).unwrap();
    let placement = fx.to_placement(&plan).unwrap();
    route_and_validate(&config, plan, &placement, tuning)
}

fn fixture(name: &str) -> Routed {
    fixture_with(name, |_| {}, RouteTuning::default())
}

fn assert_routed_clean(r: &Routed) {
    assert!(r.result.success, "routing failed: {:?}, failed nets {:?}", &r.result.log[r.result.log.len().saturating_sub(10)..], r.result.failed_nets);
    assert_eq!(r.errors, []);
}

fn kind_count(r: &Routed, kind: Kind) -> usize {
    r.result.entities.iter().filter(|e| e.kind == kind).count()
}

fn net_item(r: &Routed, net: i32) -> &str {
    &r.plan.nets[net as usize].item
}

#[test]
fn t1_hand_placement_routes_and_validates() {
    let r = fixture("t1");
    assert_routed_clean(&r);
    assert_eq!(r.result.entities.iter().filter(|e| e.kind == Kind::Inserter && e.net.is_some()).count(), 3, "two input inserters for 2 iron/s, one onto the gear belt");
    assert_eq!(kind_count(&r, Kind::Chest), 0, "gears leave through an output port");
    assert_eq!(r.result.outputs.iter().map(|o| o.item.as_str()).collect::<Vec<_>>(), ["iron-gear-wheel"]);
}

#[test]
fn t2_with_adjacent_machines_uses_direct_insertion_for_cable() {
    let r = fixture("t2_di");
    assert_routed_clean(&r);
    assert_eq!(r.result.di.len(), 2, "both cable machines feed the circuit machine directly");
    assert!(r.result.entities.iter().all(|e| e.net.is_none_or(|n| net_item(&r, n) != "copper-cable")), "no cable belt is needed");
}

#[test]
fn t2_with_spread_machines_routes_a_cable_belt() {
    let r = fixture("t2_belt");
    assert_routed_clean(&r);
    assert!(r.result.entities.iter().any(|e| e.kind == Kind::Belt && e.net.is_some_and(|n| net_item(&r, n) == "copper-cable")));
}

#[test]
fn t3_hand_placement_routes_and_validates() {
    assert_routed_clean(&fixture("t3"));
}

#[test]
fn two_nets_that_must_cross_succeed_through_negotiation_with_an_underground() {
    // Expensive undergrounds and the fast 1.5 present-cost schedule, under which a detour does not win.
    let tuning = RouteTuning { ug_penalty: 8.0, p_mult: 1.5, max_iterations: 60, stagnation_stop: 25, ..Default::default() };
    // Plain A*: one pass, other nets cost nothing and no endgame, so the two shortest paths overlap.
    let plain = fixture_with("cross", |_| {}, RouteTuning { p_fac_first: 0.0, max_iterations: 1, endgame_rounds: 0, ..tuning.clone() });
    assert!(!plain.result.success, "plain A* should overlap");
    let r = fixture_with("cross", |_| {}, tuning);
    assert_routed_clean(&r);
    assert!(kind_count(&r, Kind::UgIn) > 0, "an underground carries one net under the other");
    assert!(r.result.iterations > 1, "negotiation needed more than one pass");
}

#[test]
fn consumers_far_apart_are_reached_by_a_splitter_branch() {
    let r = fixture("fork");
    assert_routed_clean(&r);
    assert!(kind_count(&r, Kind::Splitter) >= 1, "expected a branch");
    // Without B9 the same fixture still routes, as one longer chain.
    let chain = fixture_with("fork", |c| { c.tricks.insert("B9".to_owned(), false); }, RouteTuning::default());
    assert_routed_clean(&chain);
    assert_eq!(kind_count(&chain, Kind::Splitter), 0);
}

#[test]
fn sources_start_on_their_configured_edge_tiles() {
    let r = fixture("t3");
    for s in &r.result.sources {
        assert_eq!(s.tile.x, 0, "west edge source {} at {}", s.item, s.tile);
    }
    let items: HashSet<&str> = r.result.sources.iter().map(|s| s.item.as_str()).collect();
    assert_eq!(items, HashSet::from(["iron-plate", "copper-plate"]));
}

// ---- random instances ----

const FUZZ_ITEMS: [&str; 11] = [
    "iron-gear-wheel", "copper-cable", "electronic-circuit", "pipe", "iron-stick", "inserter",
    "transport-belt", "automation-science-pack", "firearm-magazine", "pipe-to-ground", "burner-inserter",
];

/// Random legal placement, or None if the machines do not fit after a few tries.
fn random_placement(plan: &ProductionPlan, w: i32, h: i32, rnd: &mut SplittableRandom, spacing: i32) -> Option<Placement> {
    let mut placed: Vec<PlacedMachine> = Vec::new();
    let mut rects: Vec<Rect> = Vec::new();
    for m in &plan.machines {
        let mut ok = false;
        for _ in 0..200 {
            if ok {
                break;
            }
            let x = rnd.next_int_range(0, w - m.w + 1);
            let y = rnd.next_int_range(0, h - m.h + 1);
            let grown = Rect::new(x - spacing, y - spacing, m.w + 2 * spacing, m.h + 2 * spacing);
            if !rects.iter().any(|r| r.overlaps(&grown)) {
                rects.push(Rect::new(x, y, m.w, m.h));
                placed.push(PlacedMachine { id: m.id, x, y, rot: Dir::ALL[rnd.next_int_bound(4) as usize], mirror: false });
                ok = true;
            }
        }
        if !ok {
            return None;
        }
    }
    Some(Placement { w, h, machines: placed })
}

fn distinct_targets(targets: Vec<ConfigTarget>) -> Vec<ConfigTarget> {
    let mut seen: HashSet<String> = HashSet::new();
    targets.into_iter().filter(|t| seen.insert(t.name.clone())).collect()
}

#[test]
fn random_small_instances_never_panic_and_every_success_validates() {
    let mut rnd = SplittableRandom::new(4242);
    let edges = ["west", "north", "east", "south", "any"];
    let (mut instances, mut successes, mut shared) = (0, 0, 0);
    while instances < 200 {
        let count = rnd.next_int_range(1, 4);
        let targets = distinct_targets(
            (0..count)
                .map(|_| {
                    let name = FUZZ_ITEMS[rnd.next_int_bound(FUZZ_ITEMS.len() as i32) as usize].to_owned();
                    ConfigTarget { name, amount: 0.1 + rnd.next_double() * 1.2 }
                })
                .collect(),
        );
        let sources: Vec<ConfigSource> = ["iron-plate", "copper-plate"]
            .iter()
            .map(|name| ConfigSource { name: (*name).to_owned(), edge: Some(edges[rnd.next_int_bound(edges.len() as i32) as usize].to_owned()), ..Default::default() })
            .collect();
        let config = RunConfiguration { targets, sources, lane_sharing: rnd.next_boolean(), ..Default::default() };
        let plan = build_plan(&DATA, &config, &Tuning::default()).unwrap();
        if !(2..=8).contains(&plan.machines.len()) {
            continue;
        }
        let size = rnd.next_int_range(12, 31);
        let spacing = rnd.next_int_range(0, 3);
        let Some(placement) = random_placement(&plan, size, size, &mut rnd, spacing) else { continue };
        instances += 1;
        let r = route_and_validate(&config, plan, &placement, RouteTuning { max_iterations: 30, ..Default::default() });
        if r.result.success {
            successes += 1;
            shared += r.result.log.iter().filter(|l| l.starts_with("shared belt")).count();
            assert_eq!(r.errors, [], "instance {instances} reported success but fails validation");
        }
    }
    assert!(successes >= instances / 3, "only {successes} of {instances} routed");
    assert!(shared > 0, "no instance shared a belt between two items, so lane sharing went untested");
}

#[test]
fn random_fluid_instances_never_panic_and_every_success_validates() {
    let mut rnd = SplittableRandom::new(777);
    let fluid_items = ["plastic-bar", "sulfur", "sulfuric-acid", "lubricant", "solid-fuel", "explosives"];
    let (mut instances, mut successes) = (0, 0);
    while instances < 40 {
        let count = rnd.next_int_range(1, 3);
        let targets = distinct_targets(
            (0..count)
                .map(|_| {
                    let name = fluid_items[rnd.next_int_bound(fluid_items.len() as i32) as usize].to_owned();
                    ConfigTarget { name, amount: 0.2 + rnd.next_double() * 2.0 }
                })
                .collect(),
        );
        let config = RunConfiguration { targets, ..Default::default() };
        let plan = build_plan(&DATA, &config, &Tuning::default()).unwrap();
        if !(2..=14).contains(&plan.machines.len()) {
            continue;
        }
        let size = rnd.next_int_range(24, 41);
        let spacing = rnd.next_int_range(1, 4);
        let Some(placement) = random_placement(&plan, size, size, &mut rnd, spacing) else { continue };
        instances += 1;
        let r = route_and_validate(&config, plan, &placement, RouteTuning { max_iterations: 30, ..Default::default() });
        if r.result.success {
            successes += 1;
            assert_eq!(r.errors, [], "fluid instance {instances} reported success but fails validation");
        }
    }
    assert!(successes > 0, "no fluid instance routed at all");
}

/// 60 nets of one producer and two consumers on a 100 x 100 box: one negotiation pass must route them all, quickly.
#[test]
fn one_iteration_on_100_x_100_with_60_nets_routes_every_net_in_time() {
    let mut rnd = SplittableRandom::new(7);
    let mut machines: Vec<MachineInstance> = Vec::new();
    let mut terminals: Vec<Terminal> = Vec::new();
    let mut nets: Vec<Net> = Vec::new();
    for n in 0..60 {
        let first = machines.len();
        for i in 0..3 {
            machines.push(MachineInstance {
                id: first + i,
                recipe: "iron-gear-wheel".to_owned(),
                machine: "assembling-machine-3".to_owned(),
                w: 3,
                h: 3,
                needs_fluid: None,
                output_rate: 1.0,
                product: "iron-gear-wheel".to_owned(),
                demand: IndexMap::new(),
                products: IndexMap::new(),
                fluid_demand: IndexMap::new(),
                fluid_in: IndexMap::new(),
                fluid_out: IndexMap::new(),
                crafts: 0.0,
            });
            // One producer at 1.0/s feeding two consumers at 0.5/s each.
            terminals.push(Terminal {
                id: terminals.len(),
                role: if i == 0 { Role::Producer } else { Role::Consumer },
                item: format!("item-{n}"),
                rate: if i == 0 { 1.0 } else { 0.5 },
                k: 1,
                machine: Some(first + i),
                port: None,
                fluid: false,
            });
        }
        let ids = terminals[terminals.len() - 3..].iter().map(|t| t.id).collect();
        nets.push(Net { id: n, item: format!("item-{n}"), flow: 1.0, terminals: ids, belt_count: 1, lane_pair: None, fluid: false });
    }
    let plan = ProductionPlan {
        machines,
        ports: Vec::new(),
        terminals,
        nets,
        raw_rates: IndexMap::new(),
        fluid_rates: IndexMap::new(),
        recipe_rates: IndexMap::new(),
        belt_tier: "fast".to_owned(),
        warnings: Vec::new(),
        outputs: IndexMap::new(),
    };
    // 14 x 14 lattice of 7-tile cells; machines shuffled into cells.
    let mut cells: Vec<usize> = (0..196).collect();
    for i in (0..cells.len()).rev() {
        let j = rnd.next_int_bound(i as i32 + 1) as usize;
        cells.swap(i, j);
    }
    let placement = Placement {
        w: 100,
        h: 100,
        machines: plan.machines.iter().enumerate().map(|(i, m)| PlacedMachine::new(m.id, 1 + (cells[i] % 14) as i32 * 7 + 2, 1 + (cells[i] / 14) as i32 * 7 + 2)).collect(),
    };
    let mut config = RunConfiguration { targets: Vec::new(), ..Default::default() };
    config.tricks.insert("I5".to_owned(), false);
    config.tricks.insert("I6".to_owned(), false);
    // One negotiation iteration, no endgame.
    let tuning = RouteTuning { max_iterations: 1, endgame_rounds: 0, ..Default::default() };
    let run = || {
        let problem = RouteProblem::new(&plan, &config, tuning.clone(), &placement);
        let t = Instant::now();
        (route(&problem), t.elapsed().as_millis())
    };
    for _ in 0..2 {
        run();
    }
    let runs: Vec<(RouteResult, u128)> = (0..3).map(|_| run()).collect();
    assert!(runs[0].0.failed_nets.is_empty(), "every net routes in the first pass");
    let fastest = runs.iter().map(|r| r.1).min().unwrap();
    // Kotlin's bound is 200 ms on a warm JVM; this one only catches a blow-up, not a slow machine.
    assert!(fastest < 1000, "one iteration took {fastest} ms");
}
