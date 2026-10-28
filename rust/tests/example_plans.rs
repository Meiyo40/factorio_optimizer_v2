//! The planner against what the Kotlin generator committed for each example job: the rate of every input and
//! output port (`ports.json`) and the machines in the blueprint. The planner has not changed since the first
//! commit, so these must still match.

mod common;

use factorio_optimizer::config::run_configuration::RunConfiguration;
use factorio_optimizer::config::tuning::Tuning;
use factorio_optimizer::data::data_model::RootData;
use factorio_optimizer::data::game_constants::takes_recipe;
use factorio_optimizer::data::loader::load_root_data;
use factorio_optimizer::emit::blueprint::decode_blueprint;
use factorio_optimizer::geom::Kind;
use factorio_optimizer::plan::{ProductionPlan, build_plan};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::LazyLock;

static DATA: LazyLock<RootData> = LazyLock::new(|| load_root_data("recipes.json").unwrap());

type Ports = BTreeMap<String, (bool, f64)>;
type Machines = BTreeMap<(String, Option<String>), usize>;

fn plan_of(dir: &str) -> ProductionPlan {
    let job = common::repo_root().join(format!("{dir}.json"));
    let config = RunConfiguration::load(job.to_str().unwrap()).unwrap();
    // As Main.kt does: the measured defaults, then the job's own overrides.
    let tuning = Tuning::defaults().with_overrides(config.tuning.as_ref()).unwrap();
    build_plan(&DATA, &config, &tuning).unwrap()
}

/// Per item: whether it is a fluid, and the sum of the rates of its ports (a busy item enters on several belts).
fn committed_ports(dir: &str, key: &str) -> Ports {
    let text = std::fs::read_to_string(common::repo_root().join(dir).join("ports.json")).unwrap();
    let json: Value = serde_json::from_str(&text).unwrap();
    let mut ports = Ports::new();
    for entry in json[key].as_array().unwrap() {
        let slot = ports.entry(entry["item"].as_str().unwrap().to_owned()).or_insert((entry["fluid"].as_bool().unwrap(), 0.0));
        slot.1 += entry["rate"].as_f64().unwrap();
    }
    ports
}

fn planned_ports(plan: &ProductionPlan, output: bool) -> Ports {
    let rates = if output { &plan.outputs } else { &plan.raw_rates };
    plan.ports.iter().filter(|p| p.output == output).map(|p| (p.item.clone(), (p.fluid, rates[&p.item]))).collect()
}

fn assert_ports(what: &str, planned: &Ports, committed: &Ports) {
    let names = |p: &Ports| p.iter().map(|(item, (fluid, _))| (item.clone(), *fluid)).collect::<Vec<_>>();
    assert_eq!(names(planned), names(committed), "{what}: items");
    for ((item, (_, got)), (_, (_, want))) in planned.iter().zip(committed) {
        assert!((got - want).abs() <= 1e-6 * want.abs().max(1.0), "{what} {item}: planned {got}, committed {want}");
    }
}

/// Machines by (machine, recipe); furnaces and the recycler carry no recipe in a blueprint.
fn committed_machines(dir: &str) -> Machines {
    let text = std::fs::read_to_string(common::repo_root().join(dir).join("best_output_string.txt")).unwrap();
    let layout = decode_blueprint(&text).unwrap().to_layout(0, 0).unwrap();
    let mut machines = Machines::new();
    for e in layout.entities.iter().filter(|e| e.kind == Kind::Machine) {
        *machines.entry((e.name.clone(), e.recipe.clone().filter(|_| takes_recipe(&e.name)))).or_default() += 1;
    }
    machines
}

fn planned_machines(plan: &ProductionPlan) -> Machines {
    let mut machines = Machines::new();
    for m in &plan.machines {
        *machines.entry((m.machine.clone(), takes_recipe(&m.machine).then(|| m.recipe.clone()))).or_default() += 1;
    }
    machines
}

fn check_example(dir: &str, machines: usize) {
    let plan = plan_of(dir);
    assert_eq!(plan.machines.len(), machines, "machines in the plan (the README's count)");
    assert_ports("inputs", &planned_ports(&plan, false), &committed_ports(dir, "inputs"));
    assert_ports("outputs", &planned_ports(&plan, true), &committed_ports(dir, "outputs"));
    assert_eq!(planned_machines(&plan), committed_machines(dir), "machines by (machine, recipe)");
}

macro_rules! example {
    ($name:ident, $dir:expr, $machines:expr) => {
        #[test]
        fn $name() {
            check_example($dir, $machines);
        }
    };
}

example!(red_green_science, "docs/examples/red_green_science", 10);
example!(red_green_science_with_shared_belts, "docs/examples/red_green_science_shared", 10);
example!(chemical_science_from_ore_and_crude_oil, "docs/examples/chemical_science", 43);
example!(electromagnetic_science_on_fulgora, "docs/examples/electromagnetic_science", 36);
example!(metallurgic_science_on_vulcanus, "docs/examples/metallurgic_science", 38);
example!(agricultural_science_and_carbon_fiber_on_gleba, "docs/examples/gleba_agricultural_carbon_fiber", 43);
example!(six_science_as_a_factory_of_three_blocks, "docs/examples/six_science_factory", 177);
example!(six_science_from_ore_as_a_factory_of_ten_blocks, "docs/status/six_science_9_blocks", 618);
