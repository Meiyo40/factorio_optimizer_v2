//! Ports of the Kotlin PlanTest: expectations are hand calculations, so they hold for any solver.

mod common;

use factorio_optimizer::config::run_configuration::{ConfigSource, ConfigTarget, ResearchConfig, RunConfiguration, SinkConfig};
use factorio_optimizer::config::tuning::Tuning;
use factorio_optimizer::data::data_model::RootData;
use factorio_optimizer::data::loader::load_root_data;
use factorio_optimizer::plan::lp::{RecipeChoice, fuel_item, productivity, products_per_craft};
use factorio_optimizer::plan::rates::machine_count;
use factorio_optimizer::plan::{ProductionPlan, Role, best_inserter_rate, build_plan, inserter_count, inserter_rate};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::LazyLock;

static DATA: LazyLock<RootData> = LazyLock::new(|| load_root_data("recipes.json").unwrap());

fn job(name: &str) -> RunConfiguration {
    let path = common::repo_root().join(format!("src/test/resources/jobs/{name}.json"));
    RunConfiguration::load(path.to_str().unwrap()).unwrap()
}

fn wanting(targets: &[(&str, f64)]) -> RunConfiguration {
    RunConfiguration {
        targets: targets.iter().map(|(name, amount)| ConfigTarget { name: (*name).to_owned(), amount: *amount }).collect(),
        ..Default::default()
    }
}

fn plan(config: &RunConfiguration) -> ProductionPlan {
    build_plan(&DATA, config, &Tuning::default()).unwrap()
}

fn count(plan: &ProductionPlan, recipe: &str) -> usize {
    plan.machines.iter().filter(|m| m.recipe == recipe).count()
}

fn roles(plan: &ProductionPlan, item: &str, role: Role) -> usize {
    let net = plan.nets.iter().find(|n| n.item == item).unwrap_or_else(|| panic!("no net for {item}"));
    plan.net_terminals(net).iter().filter(|t| t.role == role).count()
}

fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-9, "{a} != {b}");
}

#[test]
fn machine_count_uses_crafting_speed() {
    // 2 circuits/s: 2 * 0.5 / (1 * 0.75) = 1.33 -> 2 with AM2; / 1.25 = 0.8 -> 1 with AM3.
    let circuit = &DATA.recipes["electronic-circuit"];
    assert_eq!(DATA.machine_count(2.0, circuit, "assembling-machine-2", "normal").unwrap(), 2);
    assert_eq!(DATA.machine_count(2.0, circuit, "assembling-machine-3", "normal").unwrap(), 1);
    // Exactly 2.0 machines must not round up to 3.
    let red = &DATA.recipes["automation-science-pack"];
    assert_eq!(DATA.machine_count(0.5, red, "assembling-machine-3", "normal").unwrap(), 2);
    assert_eq!(machine_count(0.0, red, 1.25), 1, "at least one machine");
}

#[test]
fn t2_gives_one_circuit_machine_two_cable_machines_and_one_cable_net() {
    let p = plan(&job("t2"));
    assert_eq!(count(&p, "electronic-circuit"), 1);
    assert_eq!(count(&p, "copper-cable"), 2);
    assert_eq!(p.machines.len(), 3);

    let cable_nets: Vec<_> = p.nets.iter().filter(|n| n.item == "copper-cable").collect();
    assert_eq!(cable_nets.len(), 1);
    assert_eq!(roles(&p, "copper-cable", Role::Producer), 2);
    assert_eq!(roles(&p, "copper-cable", Role::Consumer), 1);
    close(cable_nets[0].flow, 6.0);

    let inputs: Vec<&str> = p.ports.iter().filter(|x| !x.output).map(|x| x.item.as_str()).collect();
    assert_eq!(inputs, ["copper-plate", "iron-plate"]);
    let outputs: Vec<&str> = p.ports.iter().filter(|x| x.output).map(|x| x.item.as_str()).collect();
    assert_eq!(outputs, ["electronic-circuit"]);
    close(p.raw_rates["iron-plate"], 2.0);
    close(p.raw_rates["copper-plate"], 3.0);
    // The product leaves on a belt through its output port.
    assert_eq!(roles(&p, "electronic-circuit", Role::Producer), 1);
    let circuit = p.nets.iter().find(|n| n.item == "electronic-circuit").unwrap();
    let out: Vec<_> = p.net_terminals(circuit).into_iter().filter(|t| t.role == Role::Output).collect();
    assert_eq!(out.len(), 1);
    close(out[0].rate, 2.0);
    assert!(p.terminals.iter().all(|t| t.role != Role::Sink));

    // Chest mode: no product net, one chest sink instead.
    let chests = plan(&RunConfiguration { sinks: SinkConfig { mode: "chest".to_owned(), ..Default::default() }, ..job("t2") });
    assert!(chests.nets.iter().all(|n| n.item != "electronic-circuit"), "the final product has no consumers, so no net");
    let sinks: Vec<_> = chests.terminals.iter().filter(|t| t.role == Role::Sink).collect();
    assert_eq!(sinks.len(), 1);
    close(sinks[0].rate, 2.0);
}

#[test]
fn t3_nets_match_the_hand_calculation() {
    let p = plan(&job("t3"));
    let expected: Value = serde_json::from_str(&std::fs::read_to_string(common::repo_root().join("src/test/resources/jobs/t3_expected.json")).unwrap()).unwrap();

    let machines = expected["machines"].as_object().unwrap();
    for (recipe, n) in machines {
        assert_eq!(count(&p, recipe), n.as_u64().unwrap() as usize, "machines for {recipe}");
    }
    assert_eq!(p.machines.len(), machines.values().map(|n| n.as_u64().unwrap() as usize).sum::<usize>());

    let nets = expected["nets"].as_object().unwrap();
    let mut planned: Vec<&str> = p.nets.iter().map(|n| n.item.as_str()).collect();
    planned.sort();
    let mut wanted: Vec<&str> = nets.keys().map(String::as_str).collect();
    wanted.sort();
    assert_eq!(planned, wanted);
    for (item, e) in nets {
        let net = p.nets.iter().find(|n| n.item == *item).unwrap();
        close(net.flow, e["flow"].as_f64().unwrap());
        assert_eq!(roles(&p, item, Role::Producer), e["producers"].as_u64().unwrap() as usize, "producers of {item}");
        assert_eq!(roles(&p, item, Role::Source), e["sources"].as_u64().unwrap() as usize, "sources of {item}");
        assert_eq!(roles(&p, item, Role::Consumer), e["consumers"].as_u64().unwrap() as usize, "consumers of {item}");
        assert_eq!(net.belt_count, 1);
    }
    for (item, n) in expected["outputs"].as_object().unwrap() {
        let outputs = p.terminals.iter().filter(|t| t.role == Role::Output && t.item == *item).count();
        assert_eq!(outputs, n.as_u64().unwrap() as usize);
    }
    assert!(p.terminals.iter().all(|t| t.role != Role::Sink), "products leave through ports, not chests");
    for (item, rate) in expected["raw"].as_object().unwrap() {
        close(p.raw_rates[item], rate.as_f64().unwrap());
    }
}

#[test]
fn t4_takes_acid_through_a_fluid_port_into_both_processing_unit_machines() {
    let p = plan(&job("t4"));
    let pu: Vec<_> = p.machines.iter().filter(|m| m.recipe == "processing-unit").collect();
    assert_eq!(pu.len(), 2);
    assert!(pu.iter().all(|m| m.needs_fluid.as_deref() == Some("sulfuric-acid") && m.fluid_in.len() == 1 && m.fluid_in["sulfuric-acid"] == 1));
    let acid = p.nets.iter().find(|n| n.item == "sulfuric-acid").unwrap();
    assert!(acid.fluid);
    assert_eq!(roles(&p, "sulfuric-acid", Role::Source), 1);
    assert_eq!(roles(&p, "sulfuric-acid", Role::Consumer), 2);
    assert!(p.ports.iter().find(|x| x.item == "sulfuric-acid").unwrap().fluid);
    close(p.fluid_rates["sulfuric-acid"], 1.0);
    assert_eq!(count(&p, "copper-cable"), 4);
}

#[test]
fn terminal_inserter_counts_cover_their_rate() {
    let config = job("t4");
    let p = plan(&config);
    for t in p.terminals.iter().filter(|t| t.machine.is_some()) {
        let per_inserter = best_inserter_rate(&config, t.role == Role::Consumer);
        assert!(f64::from(t.k) * per_inserter >= t.rate - 1e-9, "terminal {} ({}, {:?}) k={}", t.id, t.item, t.role, t.k);
        assert!(f64::from(t.k - 1) * per_inserter < t.rate, "terminal {} uses more inserters than needed", t.id);
    }
    // 14.4 cable/s into 2 circuit machines is 7.2/s each; fast inserters from a belt give 2.31 * 0.7.
    let circuit_cable: Vec<_> = p
        .terminals
        .iter()
        .filter(|t| t.role == Role::Consumer && t.item == "copper-cable" && p.machines[t.machine.unwrap()].recipe == "electronic-circuit")
        .collect();
    assert!(!circuit_cable.is_empty());
    assert!(circuit_cable.iter().all(|t| t.k == 5), "k = {:?}", circuit_cable.iter().map(|t| t.k).collect::<Vec<_>>());
}

/// Every item and fluid balances: made + supplied = used + leaving.
fn assert_balanced(p: &ProductionPlan) {
    let (mut made, mut used): (HashMap<&str, f64>, HashMap<&str, f64>) = Default::default();
    for m in &p.machines {
        for (k, v) in &m.products {
            *made.entry(k).or_default() += v;
        }
        for (k, v) in m.demand.iter().chain(&m.fluid_demand) {
            *used.entry(k).or_default() += v;
        }
    }
    let mut all: Vec<&str> = made.keys().chain(used.keys()).copied().collect();
    all.extend(p.raw_rates.keys().chain(p.outputs.keys()).map(String::as_str));
    for item in all {
        let get = |m: &HashMap<&str, f64>| m.get(item).copied().unwrap_or(0.0);
        let balance = get(&made) + p.raw_rates.get(item).copied().unwrap_or(0.0) - get(&used) - p.outputs.get(item).copied().unwrap_or(0.0);
        assert!(balance.abs() < 1e-6 * get(&made).max(1.0), "{item} off by {balance}");
    }
}

#[test]
fn plastic_from_crude_oil_balances_with_refineries_and_chemical_plants() {
    let p = plan(&wanting(&[("plastic-bar", 5.0)]));
    assert_balanced(&p);
    assert!(p.machines.iter().any(|m| m.machine == "oil-refinery"));
    assert!(p.machines.iter().filter(|m| m.recipe == "plastic-bar").all(|m| m.machine == "chemical-plant" && m.fluid_in.len() == 1 && m.fluid_in["petroleum-gas"] == 1));
    assert!(p.raw_rates.contains_key("coal") && p.raw_rates.contains_key("crude-oil"));
    assert!(p.nets.iter().any(|n| n.item == "petroleum-gas" && n.fluid));
}

#[test]
fn basic_oil_processing_puts_crude_on_input_2_and_petroleum_on_output_3() {
    // With everything else forbidden it is the only way to oil. (By default the planner prefers advanced oil
    // processing with cracking: 12.46 against 17.95 in its cost, machine area plus small supply costs.)
    let config = RunConfiguration {
        forbidden_recipes: ["advanced-oil-processing", "heavy-oil-cracking", "light-oil-cracking", "coal-liquefaction", "simple-coal-liquefaction"]
            .map(String::from)
            .to_vec(),
        ..wanting(&[("plastic-bar", 5.0)])
    };
    let p = plan(&config);
    assert_balanced(&p);
    let basic: Vec<_> = p.machines.iter().filter(|m| m.recipe == "basic-oil-processing").collect();
    assert_eq!(basic.len(), 6, "5.56 refinery-seconds per second");
    for m in basic {
        assert_eq!(m.fluid_in.iter().collect::<Vec<_>>(), [(&"crude-oil".to_owned(), &2)]);
        assert_eq!(m.fluid_out.iter().collect::<Vec<_>>(), [(&"petroleum-gas".to_owned(), &3)]);
    }
}

#[test]
fn the_planner_prefers_advanced_oil_with_cracking_for_plastic() {
    let p = plan(&wanting(&[("plastic-bar", 5.0)]));
    assert!(count(&p, "advanced-oil-processing") > 0 && count(&p, "basic-oil-processing") == 0);
    assert!(p.raw_rates.contains_key("water"), "advanced oil processing takes water");
}

#[test]
fn advanced_oil_processing_with_cracking_leaves_no_by_products() {
    let config = RunConfiguration { forbidden_recipes: vec!["basic-oil-processing".to_owned()], ..wanting(&[("plastic-bar", 5.0)]) };
    let p = plan(&config);
    assert_balanced(&p);
    assert!(p.machines.iter().any(|m| m.recipe == "advanced-oil-processing"));
    assert!(p.machines.iter().any(|m| m.recipe == "heavy-oil-cracking") && p.machines.iter().any(|m| m.recipe == "light-oil-cracking"));
    assert!(p.outputs.iter().filter(|(k, _)| *k != "plastic-bar").all(|(_, v)| *v < 1e-6), "surplus {:?}", p.outputs);
}

#[test]
fn by_products_without_a_use_leave_the_block() {
    let config = RunConfiguration { forbidden_recipes: vec!["kovarex-enrichment-process".to_owned()], ..wanting(&[("uranium-238", 0.5)]) };
    let p = plan(&config);
    assert_balanced(&p);
    assert!(p.outputs.get("uranium-235").copied().unwrap_or(0.0) > 0.0, "U-235 must leave the block: {:?}", p.outputs);
    assert!(p.machines.iter().all(|m| m.machine == "centrifuge"));
}

#[test]
fn listed_sources_are_not_made_inside_the_block() {
    let config = RunConfiguration {
        sources: vec![ConfigSource { name: "iron-plate".to_owned(), edge: Some("west".to_owned()), ..Default::default() }],
        ..wanting(&[("electronic-circuit", 1.0)])
    };
    let p = plan(&config);
    assert_balanced(&p);
    assert!(p.machines.iter().all(|m| m.product != "iron-plate"));
    assert!(p.machines.iter().any(|m| m.product == "copper-plate" && m.machine == "electric-furnace"), "copper is smelted from ore");
    assert!(p.raw_rates.contains_key("copper-ore"));
}

#[test]
fn burner_furnaces_get_coal_as_an_input() {
    let config = RunConfiguration { machines: Some(vec!["stone-furnace".to_owned()]), ..wanting(&[("iron-plate", 2.0)]) };
    let p = plan(&config);
    assert_balanced(&p);
    let f = &p.machines[0];
    assert_eq!(f.machine, "stone-furnace");
    assert!(f.demand.get("coal").copied().unwrap_or(0.0) > 0.0, "fuel: {:?}", f.demand);
}

#[test]
fn plan_json_round_trips() {
    let p = plan(&job("t3"));
    let again: ProductionPlan = serde_json::from_str(&p.to_json()).unwrap();
    assert_eq!(p, again);
}

#[test]
fn plan_json_uses_the_kotlin_key_names_and_omits_nulls() {
    let json: Value = serde_json::from_str(&plan(&job("t4")).to_json()).unwrap();
    for key in ["rawRates", "fluidRates", "recipeRates", "beltTier", "warnings", "outputs"] {
        assert!(json.get(key).is_some(), "missing {key}");
    }
    let machine = &json["machines"][0];
    assert!(machine.get("outputRate").is_some() && machine.get("fluidDemand").is_some());
    assert!(!json.to_string().contains("null"), "kotlinx omits nulls");
    let roles: Vec<&str> = json["terminals"].as_array().unwrap().iter().map(|t| t["role"].as_str().unwrap()).collect();
    assert!(roles.iter().all(|r| ["PRODUCER", "CONSUMER", "SINK", "SOURCE", "OUTPUT"].contains(r)), "{roles:?}");
    let source = json["terminals"].as_array().unwrap().iter().find(|t| t["role"] == "SOURCE").unwrap();
    assert!(source.get("machine").is_none(), "a source terminal has no machine: {source}");
    let spec = &json["ports"][0]["specs"][0];
    assert!(spec.get("x").is_none() && spec.get("force").is_some(), "{spec}");
}

#[test]
fn ids_are_positions_and_lists_follow_the_kotlin_ordering() {
    let p = plan(&job("t3"));
    assert!(p.machines.iter().enumerate().all(|(i, m)| m.id == i));
    assert!(p.ports.iter().enumerate().all(|(i, x)| x.id == i));
    assert!(p.terminals.iter().enumerate().all(|(i, t)| t.id == i));
    assert!(p.nets.iter().enumerate().all(|(i, n)| n.id == i));
    // Machines are sorted by recipe name.
    assert!(p.machines.windows(2).all(|w| w[0].recipe <= w[1].recipe));
    // Input ports by item, then output ports by item.
    let items = |output| p.ports.iter().filter(|x| x.output == output).map(|x| x.item.clone()).collect::<Vec<_>>();
    for list in [items(false), items(true)] {
        assert!(list.windows(2).all(|w| w[0] < w[1]), "{list:?}");
    }
    assert!(p.ports.iter().take_while(|x| !x.output).count() == items(false).len());
    // Nets come in the order items are first seen: every machine's products first, so the first recipe's product leads.
    assert_eq!(p.nets[0].item, "automation-science-pack");
    // Within a net: producers, then the source, then the output port, then consumers.
    let order = |item: &str| {
        let net = p.nets.iter().find(|n| n.item == item).unwrap();
        p.net_terminals(net).iter().map(|t| t.role).collect::<Vec<_>>()
    };
    assert_eq!(order("automation-science-pack"), [Role::Producer, Role::Producer, Role::Output]);
    assert_eq!(order("iron-plate"), [Role::Source, Role::Consumer, Role::Consumer, Role::Consumer, Role::Consumer]);
    assert_eq!(order("inserter"), [Role::Producer, Role::Consumer, Role::Consumer, Role::Consumer]);
}

#[test]
fn a_plan_can_be_written_to_disk() {
    let dir = std::env::temp_dir().join(format!("factorio_plan_test_{}", std::process::id()));
    let file = dir.join("nested").join("plan.json");
    let p = plan(&job("t2"));
    p.write(&file).unwrap();
    assert_eq!(serde_json::from_str::<ProductionPlan>(&std::fs::read_to_string(&file).unwrap()).unwrap(), p);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_target_nothing_can_make_is_refused_and_named() {
    let err = build_plan(&DATA, &wanting(&[("flux-capacitor", 1.0)]), &Tuning::default()).unwrap_err().to_string();
    assert!(err.contains("No production plan"), "{err}");
    assert!(err.contains("nothing can make [flux-capacitor]"), "{err}");
}

#[test]
fn a_target_the_block_can_make_is_not_passed_through_from_a_raw_input() {
    // Sulfuric acid is allowed in as a raw resource and is also the target. Passing it straight through would be
    // the cheapest plan (no machines at all), so the planner must make it inside instead.
    let config = RunConfiguration {
        raw_resources: Some(["sulfuric-acid", "iron-plate", "sulfur", "water"].map(String::from).to_vec()),
        ..wanting(&[("sulfuric-acid", 1.0)])
    };
    let p = plan(&config);
    assert!(count(&p, "sulfuric-acid") >= 1, "the acid is made by machines");
    assert!(!p.raw_rates.contains_key("sulfuric-acid"), "and not taken in: {:?}", p.raw_rates);
    assert!(p.raw_rates.contains_key("sulfur") && p.raw_rates.contains_key("iron-plate"));
    close(p.outputs["sulfuric-acid"], 1.0);
}

#[test]
fn inserter_rates_follow_hand_size_and_belt_derating() {
    let config = RunConfiguration::default();
    assert!((inserter_rate("fast-inserter", &config, false) - 2.31).abs() < 1e-12);
    assert!((inserter_rate("fast-inserter", &config, true) - 2.31 * 0.7).abs() < 1e-12);
    assert!((best_inserter_rate(&config, true) - 2.31 * 0.7).abs() < 1e-12, "fast beats long-handed (1.2) and basic (0.83)");
    let researched = RunConfiguration { research: ResearchConfig { inserter_capacity: 7, ..Default::default() }, ..Default::default() };
    assert!((inserter_rate("fast-inserter", &researched, false) - 2.31 * 3.0).abs() < 1e-12, "hand size 3 at level 7");
    assert!((inserter_rate("bulk-inserter", &researched, false) - 2.31 * 12.0).abs() < 1e-12, "bulk hand 12 at level 7");
}

#[test]
fn an_inserter_count_rounds_up_but_forgives_float_noise() {
    assert_eq!(inserter_count(3.0, 1.5), 2, "exactly two inserters' worth");
    assert_eq!(inserter_count(3.0 + 1e-12, 1.5), 2, "noise does not buy a third");
    assert_eq!(inserter_count(3.1, 1.5), 3);
    assert_eq!(inserter_count(0.0001, 2.0), 1, "at least one");
}

#[test]
fn burner_machines_take_the_job_fuel_or_the_item_of_their_fuel_category() {
    let config = RunConfiguration::default();
    assert_eq!(fuel_item(&DATA, "stone-furnace", &config).unwrap().as_deref(), Some("coal"));
    assert_eq!(fuel_item(&DATA, "electric-furnace", &config).unwrap(), None);
    assert_eq!(fuel_item(&DATA, "assembling-machine-3", &config).unwrap(), None);
    let wood = RunConfiguration { fuel: "wood".to_owned(), ..Default::default() };
    assert_eq!(fuel_item(&DATA, "stone-furnace", &wood).unwrap().as_deref(), Some("wood"));
    // A biochamber burns nutrients, not coal.
    assert_eq!(fuel_item(&DATA, "biochamber", &config).unwrap().as_deref(), Some("nutrients"));
}

fn choice(recipe: &str, machine: &str) -> RecipeChoice<'static> {
    RecipeChoice { recipe: &DATA.recipes[recipe], machine: machine.to_owned(), speed: 1.0 }
}

#[test]
fn productivity_adds_research_to_the_machines_own_bonus_up_to_the_recipes_cap() {
    let mut config = RunConfiguration::default();
    let gear = choice("iron-gear-wheel", "assembling-machine-3");
    assert_eq!(productivity(&DATA, &gear, &config), 0.0);
    config.research.productivity.insert("iron-gear-wheel".to_owned(), 0.2);
    close(productivity(&DATA, &gear, &config), 0.2);
    config.research.productivity.insert("iron-gear-wheel".to_owned(), 50.0);
    close(productivity(&DATA, &gear, &config), f64::from(gear.recipe.maximum_productivity));
    // Space Age machines have a built-in bonus.
    let categories = DATA.crafting_categories("foundry");
    let cast = DATA.recipes.values().find(|r| categories.contains(&r.category) && r.allowed_effects.productivity).expect("a foundry recipe takes productivity");
    close(productivity(&DATA, &choice(&cast.name, "foundry"), &RunConfiguration::default()), 0.5);
}

#[test]
fn productivity_only_multiplies_the_part_of_a_product_above_its_ingredient() {
    // Kovarex: 40 U-235 and 5 U-238 make 41 U-235 and 2 U-238; only the one extra U-235 gains.
    let mut config = RunConfiguration::default();
    config.research.productivity.insert("kovarex-enrichment-process".to_owned(), 0.5);
    let kovarex = choice("kovarex-enrichment-process", "centrifuge");
    assert!(kovarex.recipe.allowed_effects.productivity, "the data allows it");
    let made = products_per_craft(&DATA, &kovarex, &config);
    close(made["uranium-235"], 41.0 + 0.5 * 1.0);
    close(made["uranium-238"], 2.0);
}
