use super::lp::{LpResult, RecipeChoice, fuel_item, fuel_per_craft, products_per_craft, solve_rates};
use crate::config::run_configuration::{ConfigSource, RunConfiguration};
use crate::config::tuning::Tuning;
use crate::data::data_model::RootData;
use crate::data::game_constants as gc;
use crate::util::json::to_pretty_string;
use anyhow::{Context, Result, anyhow};
use indexmap::{IndexMap, IndexSet};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// SINK: inserter into a chest; OUTPUT: leaves the block at a port on the edge (belt or pipe).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Role {
    Producer,
    Consumer,
    Sink,
    Source,
    Output,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineInstance {
    pub id: usize,
    pub recipe: String,
    pub machine: String,
    pub w: i32,
    pub h: i32,
    /// Fluid ingredient this machine needs on its fluid input, or None (v1 infinity-pipe mode).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_fluid: Option<String>,
    /// Items/s of the main product this instance makes.
    pub output_rate: f64,
    pub product: String,
    /// Items/s this instance consumes, per solid ingredient (fuel included).
    pub demand: IndexMap<String, f64>,
    /// Every product per second, items and fluids.
    pub products: IndexMap<String, f64>,
    /// Fluid ingredients per second.
    #[serde(default)]
    pub fluid_demand: IndexMap<String, f64>,
    /// Fluid -> 1-based input or output connection it uses (recipe fluidbox_index, else order).
    #[serde(default)]
    pub fluid_in: IndexMap<String, i32>,
    #[serde(default)]
    pub fluid_out: IndexMap<String, i32>,
    #[serde(default)]
    pub crafts: f64,
}

impl MachineInstance {
    pub fn label(&self) -> String {
        format!("{}#{}", self.recipe, self.id)
    }

    /// Items (not fluids) this machine makes.
    pub fn item_products(&self) -> impl Iterator<Item = (&String, &f64)> {
        self.products.iter().filter(|(name, _)| !self.fluid_out.contains_key(*name))
    }
}

/// Where an input enters (or an output leaves): the union of the configured fixed tiles and edge segments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Port {
    pub id: usize,
    pub item: String,
    pub specs: Vec<ConfigSource>,
    #[serde(default)]
    pub fluid: bool,
    /// Items leave the block here instead of entering.
    #[serde(default)]
    pub output: bool,
    /// Exactly one belt (a link between blocks of a split factory, so its two ends pair up).
    #[serde(default)]
    pub single: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Terminal {
    pub id: usize,
    pub role: Role,
    pub item: String,
    /// Items/s through this terminal (fluids: units/s).
    pub rate: f64,
    /// Inserters this terminal needs (1 for ports and fluids).
    pub k: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<usize>,
    #[serde(default)]
    pub fluid: bool,
}

/// All producers and consumers of one item (or fluid) in the block. Routing decides how many belts carry it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Net {
    pub id: usize,
    pub item: String,
    pub flow: f64,
    pub terminals: Vec<usize>,
    /// Belts this item needs at the configured tier.
    #[serde(default = "one")]
    pub belt_count: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane_pair: Option<i32>,
    #[serde(default)]
    pub fluid: bool,
}

fn one() -> i32 {
    1
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductionPlan {
    pub machines: Vec<MachineInstance>,
    pub ports: Vec<Port>,
    pub terminals: Vec<Terminal>,
    pub nets: Vec<Net>,
    /// Supplied through ports per second (items and fluids).
    pub raw_rates: IndexMap<String, f64>,
    /// The fluid part of `raw_rates`.
    pub fluid_rates: IndexMap<String, f64>,
    pub recipe_rates: IndexMap<String, f64>,
    pub belt_tier: String,
    pub warnings: Vec<String>,
    /// Leaving the block per second: targets plus surplus by-products.
    #[serde(default)]
    pub outputs: IndexMap<String, f64>,
}

impl ProductionPlan {
    /// Nets carried on belts (fluid nets are piped).
    pub fn belt_nets(&self) -> impl Iterator<Item = &Net> {
        self.nets.iter().filter(|n| !n.fluid)
    }

    pub fn terminal(&self, id: usize) -> &Terminal {
        &self.terminals[id]
    }

    pub fn net_terminals(&self, net: &Net) -> Vec<&Terminal> {
        net.terminals.iter().map(|&t| &self.terminals[t]).collect()
    }

    pub fn machine_terminals(&self, machine: usize) -> Vec<&Terminal> {
        self.terminals.iter().filter(|t| t.machine == Some(machine)).collect()
    }

    pub fn total_machine_area(&self) -> i32 {
        self.machines.iter().map(|m| m.w * m.h).sum()
    }

    pub fn to_json(&self) -> String {
        to_pretty_string(self)
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("Could not create {}", dir.display()))?;
        }
        std::fs::write(path, self.to_json()).with_context(|| format!("Could not write {}", path.display()))
    }
}

/// Effective items/s of one inserter of `name`, given hand size research and whether it picks from a belt.
pub fn inserter_rate(name: &str, config: &RunConfiguration, from_belt: bool) -> f64 {
    let spec = gc::inserter(name).expect("inserters.allowed is validated");
    let (normal, bulk) = gc::hand_sizes(config.research.inserter_capacity);
    let hand = if spec.bulk { bulk } else { normal };
    spec.base_rate * f64::from(hand) * if from_belt { config.inserters.belt_pickup_derate } else { 1.0 }
}

/// Best rate among the allowed inserters that sit at d = 1 (long-handed only serves trick options).
pub fn best_inserter_rate(config: &RunConfiguration, from_belt: bool) -> f64 {
    config.inserters.allowed.iter().map(|n| inserter_rate(n, config, from_belt)).fold(f64::NEG_INFINITY, f64::max)
}

pub fn inserter_count(rate: f64, per_inserter: f64) -> i32 {
    ((rate / per_inserter - 1e-9).ceil() as i32).max(1)
}

fn warn(warnings: &mut Vec<String>, message: String) {
    println!("WARNING: {message}");
    warnings.push(message);
}

/// Stage 1: targets -> recipe rates (linear program) -> machine instances, ports, terminals and nets.
/// Runs once per job.
pub fn build_plan(data: &RootData, config: &RunConfiguration, tuning: &Tuning) -> Result<ProductionPlan> {
    let mut warnings = Vec::new();
    let lp = solve_rates(data, config, &tuning.planner, &mut |m| warn(&mut warnings, m))?;
    let belt_tier = gc::belt_tier(&config.belt_tier)?;

    let machines = machine_instances(data, config, tuning, &lp, &mut warnings)?;
    let ports = ports(config, &lp, &mut warnings);
    let (terminals, nets) = terminals_and_nets(config, tuning, belt_tier, &lp, &machines, &ports);

    Ok(ProductionPlan {
        machines,
        ports,
        terminals,
        nets,
        raw_rates: lp.supply.clone(),
        fluid_rates: lp.supply.iter().filter(|(k, _)| lp.fluids.contains(*k)).map(|(k, v)| (k.clone(), *v)).collect(),
        recipe_rates: lp
            .crafts
            .iter()
            .map(|(c, x)| (c.recipe.name.clone(), x * c.recipe.main_product.as_ref().map_or(1.0, |p| p.expected_amount())))
            .collect(),
        belt_tier: config.belt_tier.clone(),
        warnings,
        outputs: lp.output,
    })
}

/// 1. Machine instances: ceil(machine-seconds), each with an equal share of its recipe's crafts; more when one
///    machine would need more inserters than fit around it (a foundry or electromagnetic plant is fast).
fn machine_instances(
    data: &RootData,
    config: &RunConfiguration,
    tuning: &Tuning,
    lp: &LpResult,
    warnings: &mut Vec<String>,
) -> Result<Vec<MachineInstance>> {
    let mut machines: Vec<MachineInstance> = Vec::new();
    let pick_rate = best_inserter_rate(config, true);
    let drop_rate = best_inserter_rate(config, false);
    let mut crafts: Vec<&(RecipeChoice, f64)> = lp.crafts.iter().collect();
    crafts.sort_by(|a, b| a.0.recipe.name.cmp(&b.0.recipe.name));
    for (c, x) in crafts.into_iter().map(|(c, x)| (c, *x)) {
        let r = c.recipe;
        let e = data.entities.get(&c.machine).ok_or_else(|| anyhow!("Unknown machine '{}'", c.machine))?;
        let fluid_ingredients: Vec<_> = r.ingredients.iter().filter(|i| i.is_fluid()).collect();
        let fluid_products: Vec<_> = r.products.iter().filter(|p| p.r#type == "fluid").collect();
        let fuel_item = fuel_item(data, &c.machine, config)?;
        let fuel = fuel_per_craft(data, c, fuel_item.as_deref())?;
        let main = r.main_product.as_ref().unwrap_or(&r.products[0]).name.clone();
        let products_per = products_per_craft(data, c, config);
        let mut demand_per: IndexMap<String, f64> = IndexMap::new();
        for ing in r.ingredients.iter().filter(|i| !i.is_fluid()) {
            *demand_per.entry(ing.name.clone()).or_insert(0.0) += ing.amount;
        }
        if fuel > 0.0 {
            let fuel_item = fuel_item.as_deref().expect("a fuel use has a fuel item");
            let current = demand_per.get(fuel_item).copied().unwrap_or(0.0);
            demand_per.insert(fuel_item.to_owned(), current + fuel);
        }
        let item_out: IndexMap<&String, f64> = products_per
            .iter()
            .filter(|(p, v)| fluid_products.iter().all(|fp| fp.name != **p) && **v > 1e-12)
            .map(|(p, v)| (p, *v))
            .collect();
        let inserters = |n: i32| -> i32 {
            let n = f64::from(n);
            demand_per.values().map(|d| inserter_count(x * d / n, pick_rate)).sum::<i32>()
                + item_out.values().map(|d| inserter_count(x * d / n, drop_rate)).sum::<i32>()
        };
        let room = (demand_per.len() + item_out.len()).max(
            (tuning.planner.access_share * f64::from(2 * (e.width() + e.height()) - fluid_ingredients.len() as i32 - fluid_products.len() as i32))
                as usize,
        ) as i32;
        let base = ((x * c.machine_time() - 1e-6).ceil() as i32).max(1);
        let mut n = base;
        while inserters(n) > room && n < base * 20 {
            n += 1;
        }
        if n > base {
            warn(warnings, format!("{}: {} more {} than its rate needs, so each one's inserters fit around it", r.name, n - base, c.machine));
        }
        let crafts_each = x / f64::from(n);
        let products: IndexMap<String, f64> = products_per.iter().map(|(k, v)| (k.clone(), crafts_each * v)).collect();
        let demand: IndexMap<String, f64> = demand_per.iter().map(|(k, v)| (k.clone(), crafts_each * v)).collect();
        let mut fluid_demand: IndexMap<String, f64> = IndexMap::new();
        for ing in &fluid_ingredients {
            *fluid_demand.entry(ing.name.clone()).or_insert(0.0) += ing.amount;
        }
        fluid_demand.values_mut().for_each(|v| *v *= crafts_each);
        let fluid_in: IndexMap<String, i32> =
            fluid_ingredients.iter().enumerate().map(|(i, ing)| (ing.name.clone(), ing.fluidbox_index.unwrap_or(i as i32 + 1))).collect();
        let fluid_out: IndexMap<String, i32> =
            fluid_products.iter().enumerate().map(|(i, p)| (p.name.clone(), p.fluidbox_index.unwrap_or(i as i32 + 1))).collect();
        let output_rate = *products.get(&main).ok_or_else(|| anyhow!("{}: main product '{main}' is not a product", r.name))?;
        for _ in 0..n {
            machines.push(MachineInstance {
                id: machines.len(),
                recipe: r.name.clone(),
                machine: c.machine.clone(),
                w: e.width(),
                h: e.height(),
                needs_fluid: fluid_ingredients.first().map(|i| i.name.clone()),
                output_rate,
                product: main.clone(),
                demand: demand.clone(),
                products: products.clone(),
                fluid_demand: fluid_demand.clone(),
                fluid_in: fluid_in.clone(),
                fluid_out: fluid_out.clone(),
                crafts: crafts_each,
            });
        }
    }
    Ok(machines)
}

/// 2. Ports: one per supplied item or fluid, the union of its configured sources (whole perimeter if none);
///    and, in port mode, one per item or fluid leaving the block (fluids always leave through a port).
fn ports(config: &RunConfiguration, lp: &LpResult, warnings: &mut Vec<String>) -> Vec<Port> {
    let anywhere = |item: &str| vec![ConfigSource { name: item.to_owned(), edge: Some("any".to_owned()), ..Default::default() }];
    let mut ports: Vec<Port> = Vec::new();
    let mut supplied: Vec<&String> = lp.supply.keys().collect();
    supplied.sort();
    for item in supplied {
        let mut specs: Vec<ConfigSource> = config.sources.iter().filter(|s| s.name == *item).cloned().collect();
        if specs.is_empty() {
            warn(warnings, format!("no source configured for input '{item}'; it may enter anywhere on the box edge"));
            specs = anywhere(item);
        }
        ports.push(Port { id: ports.len(), item: item.clone(), specs, fluid: lp.fluids.contains(item), output: false, single: false });
    }
    let port_mode = config.sinks.mode == "port";
    let mut leaving: Vec<&String> = lp.output.keys().filter(|i| port_mode || lp.fluids.contains(*i)).collect();
    leaving.sort();
    for item in leaving {
        let mut specs: Vec<ConfigSource> = config.outputs.iter().filter(|s| s.name == *item).cloned().collect();
        if specs.is_empty() {
            specs = anywhere(item);
        }
        ports.push(Port { id: ports.len(), item: item.clone(), specs, fluid: lp.fluids.contains(item), output: true, single: false });
    }
    let configured: IndexSet<&String> = config.sources.iter().map(|s| &s.name).collect();
    for name in configured.into_iter().filter(|n| !lp.supply.contains_key(*n)) {
        warn(warnings, format!("source '{name}' is configured but not an input of this job"));
    }
    ports
}

/// 3. Terminals and nets per item and fluid.
fn terminals_and_nets(
    config: &RunConfiguration,
    tuning: &Tuning,
    belt_tier: &gc::BeltTier,
    lp: &LpResult,
    machines: &[MachineInstance],
    ports: &[Port],
) -> (Vec<Terminal>, Vec<Net>) {
    let mut terminals: Vec<Terminal> = Vec::new();
    let mut nets: Vec<Net> = Vec::new();
    let drop_rate = best_inserter_rate(config, false);
    let pick_rate = best_inserter_rate(config, true);

    let mut items: IndexSet<&String> = IndexSet::new();
    items.extend(machines.iter().flat_map(|m| m.products.keys()));
    items.extend(machines.iter().flat_map(|m| m.demand.keys().chain(m.fluid_demand.keys())));
    items.extend(lp.supply.keys());

    for item in items {
        let fluid = lp.fluids.contains(item);
        let producers: Vec<&MachineInstance> = machines.iter().filter(|m| m.products.get(item).copied().unwrap_or(0.0) > 1e-12).collect();
        let demand_of = |m: &MachineInstance| m.demand.get(item).or_else(|| m.fluid_demand.get(item)).copied();
        let consumers: Vec<&MachineInstance> = machines.iter().filter(|m| demand_of(m).unwrap_or(0.0) > 1e-12).collect();
        let out_total = lp.output.get(item).copied().unwrap_or(0.0);
        let produced: f64 = producers.iter().map(|m| m.products[item]).sum();
        let out_port = ports.iter().find(|p| p.item == *item && p.output);
        let mut net_terminals: Vec<Terminal> = Vec::new();
        let mut add_terminal = |role: Role, rate: f64, k: i32, machine: Option<usize>, port: Option<usize>| -> Terminal {
            let t = Terminal { id: terminals.len(), role, item: item.clone(), rate, k, machine, port, fluid };
            terminals.push(t.clone());
            t
        };

        for m in &producers {
            let made = m.products[item];
            // Chest mode: what leaves the block is split over producers in proportion to what they make.
            let chest_share = if out_port.is_none() && produced > 0.0 { out_total * made / produced } else { 0.0 };
            if chest_share > 1e-9 {
                add_terminal(Role::Sink, chest_share, inserter_count(chest_share, drop_rate), Some(m.id), None);
            }
            let belt_share = made - chest_share;
            if (!consumers.is_empty() || out_port.is_some()) && belt_share > 1e-9 {
                let k = if fluid { 1 } else { inserter_count(belt_share, drop_rate) };
                net_terminals.push(add_terminal(Role::Producer, belt_share, k, Some(m.id), None));
            }
        }
        if let Some(port) = ports.iter().find(|p| p.item == *item && !p.output) {
            net_terminals.push(add_terminal(Role::Source, lp.supply[item], 1, None, Some(port.id)));
        }
        if let Some(port) = out_port.filter(|_| out_total > 1e-9) {
            net_terminals.push(add_terminal(Role::Output, out_total, 1, None, Some(port.id)));
        }
        for m in &consumers {
            let need = demand_of(m).expect("a consumer has demand");
            let k = if fluid { 1 } else { inserter_count(need, pick_rate) };
            net_terminals.push(add_terminal(Role::Consumer, need, k, Some(m.id), None));
        }

        let takes = |t: &&Terminal| matches!(t.role, Role::Consumer | Role::Output);
        if net_terminals.len() >= 2 && net_terminals.iter().any(|t| takes(&t)) {
            let flow: f64 = net_terminals.iter().filter(takes).map(|t| t.rate).sum();
            let belts = if fluid { 1 } else { ((flow / (tuning.net.belt_utilisation * belt_tier.throughput) - 1e-9).ceil() as i32).max(1) };
            nets.push(Net {
                id: nets.len(),
                item: item.clone(),
                flow,
                terminals: net_terminals.iter().map(|t| t.id).collect(),
                belt_count: belts,
                lane_pair: None,
                fluid,
            });
        }
    }
    (terminals, nets)
}
