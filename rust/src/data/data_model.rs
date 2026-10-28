//! recipes.json. Maps keep the file's order (Gson read them into insertion-ordered maps, and the planner's
//! variable order follows it). Unlike the Kotlin model, products and ingredients do not hold links to their
//! item or fluid: look them up in `RootData::items` / `RootData::fluids` by name.

use indexmap::IndexMap;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Deserialize)]
pub struct RootData {
    pub game_version: String,
    pub groups: IndexMap<String, Group>,
    pub quality: IndexMap<String, Quality>,
    #[serde(deserialize_with = "list_or_empty")]
    pub quality_names: Vec<String>,
    pub recipes: IndexMap<String, Recipe>,
    pub items: IndexMap<String, Item>,
    pub fluids: IndexMap<String, Fluid>,
    pub entities: IndexMap<String, Entity>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Group {
    pub name: String,
    pub r#type: String,
    pub order: String,
    pub order_in_recipe: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Quality {
    pub name: String,
    pub level: i32,
    pub next_probability: f64,
    pub beacon_power_usage_multiplier: f64,
    pub mining_drill_resource_drain_multiplier: f64,
    pub group: String,
    pub subgroup: String,
    pub next: Option<String>,
    pub translated_name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Recipe {
    pub name: String,
    pub category: String,
    #[serde(default, deserialize_with = "list_or_empty")]
    pub ingredients: Vec<Ingredient>,
    #[serde(default, deserialize_with = "list_or_empty")]
    pub products: Vec<Product>,
    pub main_product: Option<Product>,
    pub allowed_effects: AllowedEffects,
    pub maximum_productivity: i32,
    pub energy: f64,
    pub order: String,
    pub group: String,
    pub subgroup: String,
    pub enabled: bool,
    pub productivity_bonus: i32,
    pub translated_name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Ingredient {
    pub r#type: String,
    pub name: String,
    pub amount: f64,
    pub fluidbox_multiplier: Option<i32>,
    pub ignored_by_stats: Option<f64>,
    pub fluidbox_index: Option<i32>,
}

impl Ingredient {
    pub fn is_fluid(&self) -> bool {
        self.r#type == "fluid"
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Product {
    pub r#type: String,
    pub name: String,
    pub amount: f64,
    pub probability: Option<f64>,
    pub temperature: Option<f64>,
    pub extra_count_fraction: Option<f64>,
    pub fluidbox_index: Option<i32>,
    pub fluidbox_multiplier: Option<i32>,
    pub percent_spoiled: Option<f64>,
    pub ignored_by_stats: Option<f64>,
    pub ignored_by_productivity: Option<f64>,
}

impl Product {
    /// Expected items per craft, counting probability and extra fraction.
    pub fn expected_amount(&self) -> f64 {
        self.amount * self.probability.unwrap_or(1.0) + self.extra_count_fraction.unwrap_or(0.0)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AllowedEffects {
    pub consumption: bool,
    pub speed: bool,
    pub productivity: bool,
    pub pollution: bool,
    pub quality: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Item {
    pub name: String,
    pub r#type: String,
    pub order: String,
    pub group: String,
    pub subgroup: String,
    pub stack_size: i32,
    pub weight: f64,
    pub fuel_value: i64,
    #[serde(default)]
    pub rocket_launch_products: Value,
    pub translated_name: String,
    pub fuel_category: Option<String>,
    #[serde(default, deserialize_with = "opt_list_or_empty")]
    pub flags: Option<Vec<String>>,
    pub module_effects: Option<ModuleEffects>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModuleEffects {
    pub consumption: Option<f64>,
    pub speed: Option<f64>,
    pub productivity: Option<f64>,
    pub pollution: Option<f64>,
    pub quality: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Fluid {
    pub name: String,
    pub order: String,
    pub group: String,
    pub subgroup: String,
    pub fuel_value: i64,
    pub translated_name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EntityBase {
    pub name: String,
    pub r#type: String,
    pub order: String,
    pub group: String,
    pub subgroup: String,
    pub module_inventory_size: i32,
    pub width: i32,
    pub height: i32,
    #[serde(default, deserialize_with = "list_or_empty")]
    pub flags: Vec<String>,
    pub translated_name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EffectReceiver {
    pub base_effect: IndexMap<String, f64>,
    pub uses_module_effects: bool,
    pub uses_beacon_effects: bool,
    pub uses_surface_effects: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BoilerEntity {
    #[serde(flatten)]
    pub base: EntityBase,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FurnaceEntity {
    #[serde(flatten)]
    pub base: EntityBase,
    pub crafting_speed: IndexMap<String, f64>,
    #[serde(default, deserialize_with = "list_or_empty")]
    pub crafting_categories: Vec<String>,
    #[serde(default, deserialize_with = "list_or_empty")]
    pub allowed_effects: Vec<String>,
    pub effect_receiver: EffectReceiver,
    pub energy_consumption: f64,
    pub drain: f64,
    pub energy_source: String,
    #[serde(default, deserialize_with = "opt_list_or_empty")]
    pub fuel_categories: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssemblingMachineEntity {
    #[serde(flatten)]
    pub base: EntityBase,
    pub crafting_speed: IndexMap<String, f64>,
    #[serde(default, deserialize_with = "list_or_empty")]
    pub crafting_categories: Vec<String>,
    #[serde(default, deserialize_with = "list_or_empty")]
    pub allowed_effects: Vec<String>,
    pub effect_receiver: EffectReceiver,
    pub energy_consumption: f64,
    pub drain: f64,
    pub energy_source: String,
    #[serde(default, deserialize_with = "opt_list_or_empty")]
    pub fuel_categories: Option<Vec<String>>,
    pub fixed_recipe: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BeaconEntity {
    #[serde(flatten)]
    pub base: EntityBase,
    #[serde(default, deserialize_with = "list_or_empty")]
    pub allowed_effects: Vec<String>,
    pub distribution_effectivity: f64,
    pub distribution_effectivity_bonus_per_quality_level: f64,
    pub supply_area_distance: IndexMap<String, f64>,
    pub energy_consumption: f64,
    pub drain: f64,
    pub energy_source: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RocketSiloEntity {
    #[serde(flatten)]
    pub base: EntityBase,
    pub crafting_speed: IndexMap<String, f64>,
    #[serde(default, deserialize_with = "list_or_empty")]
    pub crafting_categories: Vec<String>,
    #[serde(default, deserialize_with = "list_or_empty")]
    pub allowed_effects: Vec<String>,
    pub effect_receiver: EffectReceiver,
    pub rocket_parts_required: i32,
    pub energy_consumption: f64,
    pub drain: f64,
    pub energy_source: String,
    pub fixed_recipe: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Entity {
    Boiler(BoilerEntity),
    Furnace(FurnaceEntity),
    AssemblingMachine(AssemblingMachineEntity),
    Beacon(BeaconEntity),
    RocketSilo(RocketSiloEntity),
}

impl Entity {
    pub fn base(&self) -> &EntityBase {
        match self {
            Entity::Boiler(e) => &e.base,
            Entity::Furnace(e) => &e.base,
            Entity::AssemblingMachine(e) => &e.base,
            Entity::Beacon(e) => &e.base,
            Entity::RocketSilo(e) => &e.base,
        }
    }

    pub fn name(&self) -> &str {
        &self.base().name
    }

    pub fn width(&self) -> i32 {
        self.base().width
    }

    pub fn height(&self) -> i32 {
        self.base().height
    }

    /// Crafting speed per quality, for the entities that craft.
    pub fn crafting_speed(&self) -> Option<&IndexMap<String, f64>> {
        match self {
            Entity::Furnace(e) => Some(&e.crafting_speed),
            Entity::AssemblingMachine(e) => Some(&e.crafting_speed),
            Entity::RocketSilo(e) => Some(&e.crafting_speed),
            _ => None,
        }
    }

    pub fn crafting_categories(&self) -> &[String] {
        match self {
            Entity::Furnace(e) => &e.crafting_categories,
            Entity::AssemblingMachine(e) => &e.crafting_categories,
            Entity::RocketSilo(e) => &e.crafting_categories,
            _ => &[],
        }
    }

    /// The energy source of a furnace or assembling machine (the only entities the planner reads it from).
    pub fn energy_source(&self) -> Option<&str> {
        match self {
            Entity::Furnace(e) => Some(&e.energy_source),
            Entity::AssemblingMachine(e) => Some(&e.energy_source),
            _ => None,
        }
    }

    /// Watts, for a furnace or assembling machine.
    pub fn energy_consumption(&self) -> Option<f64> {
        match self {
            Entity::Furnace(e) => Some(e.energy_consumption),
            Entity::AssemblingMachine(e) => Some(e.energy_consumption),
            _ => None,
        }
    }

    pub fn fuel_categories(&self) -> Option<&[String]> {
        match self {
            Entity::Furnace(e) => e.fuel_categories.as_deref(),
            Entity::AssemblingMachine(e) => e.fuel_categories.as_deref(),
            _ => None,
        }
    }

    pub fn effect_receiver(&self) -> Option<&EffectReceiver> {
        match self {
            Entity::Furnace(e) => Some(&e.effect_receiver),
            Entity::AssemblingMachine(e) => Some(&e.effect_receiver),
            _ => None,
        }
    }

    /// A furnace or assembling machine that burns fuel.
    pub fn is_burner(&self) -> bool {
        self.energy_source() == Some("burner")
    }
}

impl<'de> Deserialize<'de> for Entity {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let v = Value::deserialize(d)?;
        let kind = v.get("type").and_then(Value::as_str).ok_or_else(|| D::Error::custom("entity without a type"))?.to_owned();
        let bad = |e: serde_json::Error| D::Error::custom(e);
        Ok(match kind.as_str() {
            "boiler" => Entity::Boiler(serde_json::from_value(v).map_err(bad)?),
            "furnace" => Entity::Furnace(serde_json::from_value(v).map_err(bad)?),
            "assembling-machine" => Entity::AssemblingMachine(serde_json::from_value(v).map_err(bad)?),
            "beacon" => Entity::Beacon(serde_json::from_value(v).map_err(bad)?),
            "rocket-silo" => Entity::RocketSilo(serde_json::from_value(v).map_err(bad)?),
            other => return Err(D::Error::custom(format!("Unknown entity type: {other}"))),
        })
    }
}

/// Lua writes an empty table as `{}`, so a list in recipes.json may be an empty object.
#[derive(Deserialize)]
#[serde(untagged)]
enum ListOrObject<T> {
    List(Vec<T>),
    /// Only here so that `{}` matches; whatever it holds is dropped, as Gson's adapter did.
    Object(#[allow(dead_code)] Map<String, Value>),
}

impl<T> ListOrObject<T> {
    fn into_vec(self) -> Vec<T> {
        match self {
            ListOrObject::List(v) => v,
            ListOrObject::Object(_) => Vec::new(),
        }
    }
}

fn list_or_empty<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Vec<T>, D::Error> {
    Ok(ListOrObject::<T>::deserialize(d)?.into_vec())
}

fn opt_list_or_empty<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<Vec<T>>, D::Error> {
    Ok(Option::<ListOrObject<T>>::deserialize(d)?.map(ListOrObject::into_vec))
}
