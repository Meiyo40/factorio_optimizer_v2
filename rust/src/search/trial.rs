use crate::config::run_configuration::RunConfiguration;
use crate::data::data_model::RootData;
use crate::geom::{Layout, PlacedEntity, Tile};
use crate::place::Placement;
use crate::plan::{ProductionPlan, Role, inserter_rate};
use crate::util::sum_f64;
use crate::validate::{MachineSpec, OutputSpec, SourceSpec, ValidationSpec, recipe_info_from};
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// What the validator should expect of this plan on this placement.
#[allow(clippy::too_many_arguments)] // the parameters of validationSpec in Kotlin
pub fn validation_spec(
    data: &RootData,
    plan: &ProductionPlan,
    config: &RunConfiguration,
    placement: &Placement,
    sources: &[SourceSpec],
    require_power: bool,
    outside_allowed: &HashSet<Tile>,
    outputs: &[OutputSpec],
) -> ValidationSpec {
    let recipe_info = recipe_info_from(data);
    let mut machines: HashMap<Tile, MachineSpec> = HashMap::new();
    for pm in &placement.machines {
        let inst = &plan.machines[pm.id];
        let info = recipe_info(&inst.recipe).expect("a planned recipe is in the data");
        // Inserters move only item products; fluids leave through pipes.
        let items: IndexMap<String, f64> = inst.item_products().map(|(k, v)| (k.clone(), *v)).collect();
        // The first product with the largest rate (maxByOrNull keeps the first of equals).
        let mut best: Option<(&String, f64)> = None;
        for (k, &v) in &items {
            if best.is_none_or(|(_, bv)| bv < v) {
                best = Some((k, v));
            }
        }
        let sinks_here: Vec<_> = plan.terminals.iter().filter(|t| t.machine == Some(pm.id) && t.role == Role::Sink).collect();
        let mut sinks: IndexMap<String, f64> = IndexMap::new();
        for t in &sinks_here {
            sinks.entry(t.item.clone()).or_insert(0.0);
        }
        for (item, rate) in sinks.iter_mut() {
            *rate = sum_f64(sinks_here.iter().filter(|t| &t.item == item).map(|t| t.rate));
        }
        let mut ingredients = info.ingredients.clone();
        ingredients.extend(inst.demand.keys().cloned());
        let spec = MachineSpec {
            product: best.map_or_else(|| inst.product.clone(), |(k, _)| k.clone()),
            // Fuel counts as an ingredient (coal for stone furnaces, nutrients for biochambers).
            ingredients,
            output_rate: sum_f64(items.values().copied()),
            sink_rate: sum_f64(sinks_here.iter().map(|t| t.rate)),
            demand: inst.demand.clone(),
            needs_fluid: inst.needs_fluid.clone(),
            fluid_in: inst.fluid_in.clone(),
            fluid_out: inst.fluid_out.clone(),
            products: if items.len() > 1 { items } else { IndexMap::new() },
            sinks,
        };
        machines.insert(Tile::new(pm.x, pm.y), spec);
    }
    let outside = |t: &Tile| t.x < 0 || t.x >= placement.w || t.y < 0 || t.y >= placement.h;
    let mut outside_allowed = outside_allowed.clone();
    outside_allowed.extend(sources.iter().map(|s| s.tile).chain(outputs.iter().map(|o| o.tile)).filter(outside));
    let config = Arc::new(config.clone());
    let rate_config = Arc::clone(&config);
    ValidationSpec {
        machines,
        sources: sources.to_vec(),
        outputs: outputs.to_vec(),
        pole_ring: i32::from(config.power.allow_outside_ring),
        outside_allowed,
        require_power,
        fluid_mode: config.fluids.mode.clone(),
        check_rates: true,
        inserter_rate: Arc::new(move |name, from_belt| inserter_rate(name, &rate_config, from_belt)),
        recipe_info,
    }
}

pub fn layout_of(placement: &Placement, entities: Vec<PlacedEntity>, wires: Vec<(usize, usize)>, label: &str) -> Layout {
    Layout { w: placement.w, h: placement.h, entities, wires, label: label.to_owned(), tiles: Vec::new() }
}
