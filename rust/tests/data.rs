mod common;

use factorio_optimizer::data::data_model::{Entity, RootData};
use factorio_optimizer::data::game_constants::machine_geom;
use factorio_optimizer::data::loader::{load_root_data, read_resource_or_file};
use serde_json::Value;

fn data() -> RootData {
    load_root_data("recipes.json").unwrap()
}

fn raw() -> Value {
    serde_json::from_str(&read_resource_or_file("recipes.json").unwrap()).unwrap()
}

#[test]
fn every_section_has_as_many_entries_as_the_file() {
    let (d, raw) = (data(), raw());
    let len = |section: &str| raw[section].as_object().unwrap().len();
    assert_eq!(d.groups.len(), len("groups"));
    assert_eq!(d.quality.len(), len("quality"));
    assert_eq!(d.recipes.len(), len("recipes"));
    assert_eq!(d.items.len(), len("items"));
    assert_eq!(d.fluids.len(), len("fluids"));
    assert_eq!(d.entities.len(), len("entities"));
    assert_eq!(d.quality_names.len(), raw["quality_names"].as_array().unwrap().len());
}

#[test]
fn maps_keep_the_order_of_the_file() {
    // The planner's variable order follows it, as it did with Gson's insertion-ordered maps.
    let (d, raw) = (data(), raw());
    for (section, keys) in [
        ("recipes", d.recipes.keys().collect::<Vec<_>>()),
        ("items", d.items.keys().collect()),
        ("entities", d.entities.keys().collect()),
    ] {
        let in_file: Vec<&String> = raw[section].as_object().unwrap().keys().collect();
        assert_eq!(keys, in_file, "{section}");
    }
}

#[test]
fn an_empty_object_stands_for_an_empty_list() {
    let (d, raw) = (data(), raw());
    for field in ["ingredients", "products"] {
        let empty_in_file: Vec<&str> = raw["recipes"]
            .as_object()
            .unwrap()
            .iter()
            .filter(|(_, r)| r[field].as_object().is_some())
            .map(|(name, _)| name.as_str())
            .collect();
        assert!(!empty_in_file.is_empty(), "the file has recipes with `{{}}` for {field}");
        for name in empty_in_file {
            let r = &d.recipes[name];
            let list = if field == "ingredients" { r.ingredients.len() } else { r.products.len() };
            assert_eq!(list, 0, "{name}.{field}");
        }
    }
}

#[test]
fn a_recipe_reads_with_its_ingredients_and_products() {
    let d = data();
    let gear = &d.recipes["iron-gear-wheel"];
    assert_eq!(gear.category, "crafting");
    assert_eq!(gear.energy, 0.5);
    assert_eq!(gear.ingredients.len(), 1);
    assert_eq!((gear.ingredients[0].name.as_str(), gear.ingredients[0].amount), ("iron-plate", 2.0));
    assert!(!gear.ingredients[0].is_fluid());
    assert_eq!(gear.products.len(), 1);
    assert_eq!(gear.products[0].expected_amount(), 1.0);
}

#[test]
fn a_fluid_ingredient_is_a_fluid() {
    let d = data();
    let acid = &d.recipes["sulfuric-acid"];
    assert!(acid.ingredients.iter().any(|i| i.name == "sulfur" && !i.is_fluid()));
    assert!(acid.ingredients.iter().any(|i| i.name == "water" && i.is_fluid()));
    assert!(d.fluids.contains_key("water"));
}

#[test]
fn probabilities_count_towards_the_expected_amount() {
    // Uranium processing: 10 ore make 0.7% uranium-235 and 99.3% uranium-238.
    let d = data();
    let product = |name: &str| d.recipes["uranium-processing"].products.iter().find(|p| p.name == name).unwrap().expected_amount();
    assert!((product("uranium-235") - 0.007).abs() < 1e-9);
    assert!((product("uranium-238") - 0.993).abs() < 1e-9);
}

#[test]
fn items_and_fluids() {
    let d = data();
    let coal = &d.items["coal"];
    assert_eq!(coal.fuel_value, 4_000_000);
    assert_eq!(coal.fuel_category.as_deref(), Some("chemical"));
    assert!(d.items["iron-plate"].fuel_category.is_none());
    assert!(d.fluids.contains_key("crude-oil"));
}

#[test]
fn crafting_speed_by_machine_and_quality() {
    let d = data();
    assert_eq!(d.crafting_speed("assembling-machine-3", "normal").unwrap(), 1.25);
    assert_eq!(d.crafting_speed("electric-furnace", "normal").unwrap(), 2.0);
    assert!(d.crafting_speed("assembling-machine-3", "legendary").unwrap() > 1.25);
}

#[test]
fn crafting_speed_errors_name_the_problem() {
    let d = data();
    assert!(d.crafting_speed("flux-capacitor", "normal").unwrap_err().to_string().contains("Unknown machine"));
    assert!(d.crafting_speed("assembling-machine-3", "mythic").unwrap_err().to_string().contains("no crafting speed"));
    let boiler = d.entities.values().find(|e| matches!(e, Entity::Boiler(_))).unwrap().name().to_owned();
    assert!(d.crafting_speed(&boiler, "normal").unwrap_err().to_string().contains("not a crafting machine"));
}

#[test]
fn machines_craft_their_categories() {
    let d = data();
    assert!(d.crafting_categories("chemical-plant").iter().any(|c| c == "chemistry"));
    assert!(d.crafting_categories("electric-furnace").iter().any(|c| c == "smelting"));
    assert!(d.crafting_categories("flux-capacitor").is_empty());
}

#[test]
fn every_entity_kind_in_the_file_loads() {
    let d = data();
    let mut seen = [false; 5];
    for e in d.entities.values() {
        seen[match e {
            Entity::Boiler(_) => 0,
            Entity::Furnace(_) => 1,
            Entity::AssemblingMachine(_) => 2,
            Entity::Beacon(_) => 3,
            Entity::RocketSilo(_) => 4,
        }] = true;
    }
    assert_eq!(seen, [true; 5]);
}

#[test]
fn burners_are_the_furnaces_and_assemblers_that_burn() {
    let d = data();
    assert!(d.entities["stone-furnace"].is_burner());
    assert!(!d.entities["electric-furnace"].is_burner());
    assert!(!d.entities["assembling-machine-3"].is_burner());
}

#[test]
fn a_machine_geometry_combines_size_and_fluid_connections() {
    let d = data();
    let refinery = machine_geom(&d, "oil-refinery").unwrap();
    assert_eq!((refinery.w, refinery.h, refinery.connections.len()), (5, 5, 5));
    assert!(!refinery.burner);
    let furnace = machine_geom(&d, "stone-furnace").unwrap();
    assert_eq!((furnace.w, furnace.h, furnace.connections.len()), (2, 2, 0));
    assert!(furnace.burner);
    assert!(machine_geom(&d, "flux-capacitor").is_err());
}

#[test]
fn built_in_resources_are_found_by_name_and_other_names_by_path() {
    assert!(read_resource_or_file("/recipes.json").unwrap().contains("game_version"));
    let job = common::repo_root().join("src/test/resources/jobs/t3.json");
    assert!(read_resource_or_file(job.to_str().unwrap()).unwrap().contains("automation-science-pack"));
    assert!(read_resource_or_file("no/such/file.json").is_err());
}
