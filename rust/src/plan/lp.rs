use crate::config::run_configuration::RunConfiguration;
use crate::config::tuning::PlannerTuning;
use crate::data::data_model::{Recipe, RootData};
use anyhow::{Result, anyhow, bail};
use indexmap::IndexMap;
use microlp::{ComparisonOp, OptimizationDirection, Problem, Variable};
use std::collections::{BTreeSet, HashMap, HashSet};

/// Resources the base game mines or pumps; they may always be supplied through a port.
pub const BASE_RAW: [&str; 9] = ["iron-ore", "copper-ore", "stone", "coal", "wood", "crude-oil", "water", "uranium-ore", "raw-fish"];

/// Categories no base crafting machine should take even if listed (placeholders, recycling).
const EXCLUDED_CATEGORIES: [&str; 3] = ["parameters", "recycling", "recycling-or-hand-crafting"];
const EXCLUDED_SUBGROUPS: [&str; 2] = ["fill-barrel", "empty-barrel"];

/// A recipe the planner may use, with the machine that will craft it.
#[derive(Debug, Clone)]
pub struct RecipeChoice<'a> {
    pub recipe: &'a Recipe,
    pub machine: String,
    pub speed: f64,
}

impl RecipeChoice<'_> {
    /// Seconds of one machine per craft.
    pub fn machine_time(&self) -> f64 {
        self.recipe.energy / self.speed
    }
}

#[derive(Debug, Clone)]
pub struct LpResult<'a> {
    /// Crafts per second per recipe used, in recipe order.
    pub crafts: Vec<(RecipeChoice<'a>, f64)>,
    /// Items/s (fluids: units/s) supplied through ports.
    pub supply: IndexMap<String, f64>,
    /// Items/s leaving the block beyond what stays inside: targets plus surplus by-products.
    pub output: IndexMap<String, f64>,
    pub targets: IndexMap<String, f64>,
    pub fluids: HashSet<String>,
}

/// The machines a job may use, in preference order (first machine able to craft a category wins).
pub fn effective_machines(config: &RunConfiguration) -> Vec<String> {
    if let Some(machines) = &config.machines {
        return machines.clone();
    }
    let mut machines = vec![config.machine.clone()];
    for m in ["electric-furnace", "chemical-plant", "oil-refinery", "centrifuge"] {
        if !machines.iter().any(|x| x == m) {
            machines.push(m.to_owned());
        }
    }
    machines
}

/// The fuel a burner machine burns: the job's `fuel` when the machine takes its category (coal for furnaces),
/// else the item named after one of the machine's fuel categories (nutrients for a biochamber), else any item
/// of such a category. None for electric machines.
pub fn fuel_item(data: &RootData, machine: &str, config: &RunConfiguration) -> Result<Option<String>> {
    let Some(e) = data.entities.get(machine).filter(|e| e.is_burner()) else { return Ok(None) };
    let default = ["chemical".to_owned()];
    let cats: &[String] = e.fuel_categories().unwrap_or(&default);
    let category_of = |item: &str| data.items.get(item).and_then(|i| i.fuel_category.as_deref());
    let in_cats = |c: Option<&str>| c.is_some_and(|c| cats.iter().any(|x| x == c));
    if in_cats(category_of(&config.fuel)) {
        return Ok(Some(config.fuel.clone()));
    }
    if let Some(c) = cats.iter().find(|c| category_of(c) == Some(c.as_str())) {
        return Ok(Some(c.clone()));
    }
    data.items
        .values()
        .filter(|i| in_cats(i.fuel_category.as_deref()) && i.fuel_value > 0)
        .min_by(|a, b| a.name.cmp(&b.name))
        .map(|i| Some(i.name.clone()))
        .ok_or_else(|| anyhow!("no fuel for '{machine}' (fuel categories {cats:?})"))
}

/// Fuel per craft for burner machines (items of `fuel_item`); 0 for electric machines.
pub fn fuel_per_craft(data: &RootData, choice: &RecipeChoice, fuel_item: Option<&str>) -> Result<f64> {
    let Some(fuel_item) = fuel_item else { return Ok(0.0) };
    let watts = data
        .entities
        .get(&choice.machine)
        .and_then(|e| e.energy_consumption())
        .ok_or_else(|| anyhow!("'{}' has no energy consumption", choice.machine))?;
    let fuel_value = data
        .items
        .get(fuel_item)
        .map(|i| i.fuel_value)
        .filter(|&v| v > 0)
        .ok_or_else(|| anyhow!("'{fuel_item}' is not a fuel"))?;
    Ok(watts * choice.machine_time() / fuel_value as f64)
}

/// Productivity of a recipe in its machine: the machine's own bonus (+50% in a foundry, electromagnetic plant or
/// biochamber) plus research bonuses from the job, capped at the recipe's maximum; none if the recipe allows none.
pub fn productivity(data: &RootData, c: &RecipeChoice, config: &RunConfiguration) -> f64 {
    if !c.recipe.allowed_effects.productivity {
        return 0.0;
    }
    let base = data
        .entities
        .get(&c.machine)
        .and_then(|e| e.effect_receiver())
        .and_then(|r| r.base_effect.get("productivity"))
        .copied()
        .unwrap_or(0.0);
    let research = config.research.productivity.get(&c.recipe.name).copied().unwrap_or(0.0);
    (base + research).min(f64::from(c.recipe.maximum_productivity))
}

/// Products per craft with productivity. The bonus applies to the part of a product above the same item's
/// ingredient amount, so catalysts (Kovarex, asteroid chunks) gain nothing from it. [P] VERIFY
pub fn products_per_craft(data: &RootData, c: &RecipeChoice, config: &RunConfiguration) -> IndexMap<String, f64> {
    let p = productivity(data, c, config);
    let mut ingredients: HashMap<&str, f64> = HashMap::new();
    for i in &c.recipe.ingredients {
        *ingredients.entry(&i.name).or_insert(0.0) += i.amount;
    }
    let mut amounts: IndexMap<String, f64> = IndexMap::new();
    for product in &c.recipe.products {
        *amounts.entry(product.name.clone()).or_insert(0.0) += product.expected_amount();
    }
    for (name, amount) in amounts.iter_mut() {
        *amount += p * (*amount - ingredients.get(name.as_str()).copied().unwrap_or(0.0)).max(0.0);
    }
    amounts
}

fn add(m: &mut HashMap<String, f64>, key: &str, value: f64) {
    m.entry(key.to_owned()).and_modify(|v| *v += value).or_insert(value);
}

/// Stage 1 rates as a linear program: crafts/s per recipe, supply per input, output per item, minimising
/// machine area plus small costs for inputs and larger ones for surplus by-products. Handles recipes with
/// several products (oil processing, cracking, Kovarex) and several recipes per item.
pub fn solve_rates<'a>(
    data: &'a RootData,
    config: &RunConfiguration,
    tun: &PlannerTuning,
    warn: &mut dyn FnMut(String),
) -> Result<LpResult<'a>> {
    let machines = effective_machines(config);
    let sourced: HashSet<&str> = config.sources.iter().map(|s| s.name.as_str()).collect();
    let category_filter: Option<HashSet<&str>> = config.recipe_categories.as_ref().map(|c| c.iter().map(String::as_str).collect());

    let mut choices: Vec<RecipeChoice<'a>> = Vec::new();
    for r in data.recipes.values() {
        if EXCLUDED_CATEGORIES.contains(&r.category.as_str())
            || EXCLUDED_SUBGROUPS.contains(&r.subgroup.as_str())
            || config.forbidden_recipes.contains(&r.name)
            || category_filter.as_ref().is_some_and(|f| !f.contains(r.category.as_str()))
            || r.products.is_empty()
        {
            continue;
        }
        // Items the job supplies are not made inside the block.
        if r.main_product.as_ref().is_some_and(|m| sourced.contains(m.name.as_str())) {
            continue;
        }
        // recipe_overrides: only the chosen recipe may make that item.
        if config.recipe_overrides.iter().any(|(item, chosen)| *chosen != r.name && r.products.iter().any(|p| p.name == *item)) {
            continue;
        }
        let Some(m) = machines.iter().find(|m| data.crafting_categories(m).contains(&r.category)) else { continue };
        choices.push(RecipeChoice { recipe: r, machine: m.clone(), speed: data.crafting_speed(m, &config.quality)? });
    }

    let mut fluids = HashSet::new();
    for r in data.recipes.values() {
        fluids.extend(r.ingredients.iter().filter(|i| i.is_fluid()).map(|i| i.name.clone()));
        fluids.extend(r.products.iter().filter(|p| p.r#type == "fluid").map(|p| p.name.clone()));
    }
    let mut targets: IndexMap<String, f64> = IndexMap::new();
    for t in &config.targets {
        *targets.entry(t.name.clone()).or_insert(0.0) += t.amount;
    }

    // Net amount per craft for every item a recipe touches (catalysts cancel).
    let mut net: Vec<HashMap<String, f64>> = Vec::with_capacity(choices.len());
    for c in &choices {
        let mut m = HashMap::new();
        for (item, amount) in products_per_craft(data, c, config) {
            add(&mut m, &item, amount);
        }
        for ing in &c.recipe.ingredients {
            add(&mut m, &ing.name, -ing.amount);
        }
        let fuel_item = fuel_item(data, &c.machine, config)?;
        let fuel = fuel_per_craft(data, c, fuel_item.as_deref())?;
        if fuel > 0.0 {
            add(&mut m, fuel_item.as_deref().expect("a fuel use has a fuel item"), -fuel);
        }
        net.push(m);
    }
    let items: BTreeSet<String> = net
        .iter()
        .flat_map(|m| m.keys().cloned())
        .chain(targets.keys().cloned())
        .chain(sourced.iter().map(|s| (*s).to_owned()))
        .collect();

    let mut problem = Problem::new(OptimizationDirection::Minimize);
    let free = (0.0, f64::INFINITY);
    let x: Vec<Variable> = choices
        .iter()
        .map(|c| {
            let e = &data.entities[&c.machine];
            problem.add_var(c.machine_time() * f64::from(e.width()) * f64::from(e.height()) / 9.0, free)
        })
        .collect();
    let raw: HashSet<&str> = match &config.raw_resources {
        Some(r) => r.iter().map(String::as_str).collect(),
        None => BASE_RAW.into_iter().collect(),
    };
    // A target the block can make is made, not passed through from a raw input (sulfuric acid on Vulcanus).
    let makeable: HashSet<&str> = choices.iter().flat_map(|c| c.recipe.products.iter().map(|p| p.name.as_str())).collect();
    let supply_allowed: HashSet<&str> = items
        .iter()
        .map(String::as_str)
        .filter(|i| sourced.contains(i) || (raw.contains(i) && !(targets.contains_key(*i) && makeable.contains(i))))
        .collect();
    let s: HashMap<&str, Variable> = items
        .iter()
        .map(String::as_str)
        .filter(|i| supply_allowed.contains(i))
        .map(|i| (i, problem.add_var(if fluids.contains(i) { tun.supply_cost_fluid } else { tun.supply_cost_item }, free)))
        .collect();
    let out: Vec<(&str, Variable)> = items
        .iter()
        .map(|i| {
            let cost = if targets.contains_key(i) {
                0.0
            } else if fluids.contains(i) {
                tun.surplus_cost_fluid
            } else {
                tun.surplus_cost_item
            };
            (i.as_str(), problem.add_var(cost, free))
        })
        .collect();
    for (item, out_var) in &out {
        let t = targets.get(*item).copied().unwrap_or(0.0);
        let mut row: Vec<(Variable, f64)> = Vec::new();
        for (c, v) in net.iter().zip(&x) {
            if let Some(&coefficient) = c.get(*item).filter(|k| **k != 0.0) {
                row.push((*v, coefficient));
            }
        }
        if let Some(&v) = s.get(item) {
            row.push((v, 1.0));
        }
        row.push((*out_var, -1.0));
        problem.add_constraint(row, ComparisonOp::Eq, t);
    }

    let solution = match problem.solve().map_err(|e| e.to_string()).and_then(|o| o.into_solution().map_err(|_| "interrupted".to_owned())) {
        Ok(solution) => solution,
        Err(status) => {
            let missing: Vec<&str> = targets
                .keys()
                .map(String::as_str)
                .filter(|t| !makeable.contains(t) && !supply_allowed.contains(t))
                .collect();
            let hint = if missing.is_empty() { String::new() } else { format!("; nothing can make [{}]", missing.join(", ")) };
            bail!("No production plan: LP status {status}{hint}");
        }
    };

    let eps = 1e-9;
    let crafts: Vec<(RecipeChoice<'a>, f64)> =
        choices.into_iter().zip(&x).map(|(c, v)| (c, solution.var_value(*v))).filter(|(_, v)| *v > eps).collect();
    let supply: IndexMap<String, f64> = items
        .iter()
        .filter_map(|i| s.get(i.as_str()).map(|v| (i.clone(), solution.var_value(*v))))
        .filter(|(_, v)| *v > eps)
        .collect();
    // What leaves the block: each target plus any surplus.
    let output: IndexMap<String, f64> = out
        .iter()
        .map(|(i, v)| ((*i).to_owned(), solution.var_value(*v) + targets.get(*i).copied().unwrap_or(0.0)))
        .filter(|(_, v)| *v > eps)
        .collect();
    for (item, rate) in &output {
        if !targets.contains_key(item) && *rate > 1e-6 {
            warn(format!("by-product {item} leaves the block at {rate:.3}/s"));
        }
    }
    Ok(LpResult { crafts, supply, output, targets, fluids })
}
