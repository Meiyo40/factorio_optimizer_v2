mod common;

use factorio_optimizer::data::game_constants as gc;
use factorio_optimizer::emit::blueprint::{decode_blueprint, encode_blueprint};
use factorio_optimizer::emit::encode::{compress_and_encode, decode_to_json};
use factorio_optimizer::geom::{Dir, GroundTile, Kind, Layout, PlacedEntity};
use serde_json::Value;

fn entity(name: &str, x: i32, y: i32, dir: Dir) -> PlacedEntity {
    PlacedEntity::new(name, PlacedEntity::kind_of(name, None).unwrap(), x, y, dir)
}

fn underground(name: &str, x: i32, y: i32, kind_type: &str) -> PlacedEntity {
    PlacedEntity::new(name, PlacedEntity::kind_of(name, Some(kind_type)).unwrap(), x, y, Dir::E)
}

fn with_recipe(mut e: PlacedEntity, recipe: &str) -> PlacedEntity {
    e.recipe = Some(recipe.to_owned());
    e
}

fn with_item(mut e: PlacedEntity, item: &str) -> PlacedEntity {
    e.item = Some(item.to_owned());
    e
}

/// Every entity type the tool emits (the Kotlin BlueprintTest's layout).
fn all_types() -> Layout {
    let mut list = vec![
        with_recipe(entity("assembling-machine-3", 0, 0, Dir::E), "processing-unit"),
        with_recipe(entity("assembling-machine-2", 4, 0, Dir::N), "iron-gear-wheel"),
    ];
    for (i, tier) in ["basic", "fast", "express", "turbo"].iter().enumerate() {
        let (i, t) = (i as i32, gc::belt_tier(tier).unwrap());
        for (j, d) in Dir::ALL.into_iter().enumerate() {
            list.push(entity(t.belt, j as i32, 4 + i, d));
        }
        list.push(underground(t.underground, 5, 4 + i, "input"));
        list.push(underground(t.underground, 7, 4 + i, "output"));
        list.push(entity(t.splitter, 9, 4 + i, Dir::N));
        list.push(entity(t.splitter, 12, 2 * i, Dir::E));
    }
    for (i, spec) in gc::INSERTERS.iter().enumerate() {
        for (j, d) in Dir::ALL.into_iter().enumerate() {
            list.push(entity(spec.name, 14 + j as i32, i as i32, d));
        }
    }
    for (i, name) in gc::CHESTS.iter().filter(|c| **c != "infinity-chest").enumerate() {
        list.push(entity(name, i as i32, 9, Dir::N));
    }
    list.push(with_item(entity("steel-chest", 0, 10, Dir::N), "iron-gear-wheel"));
    list.push(with_item(entity("infinity-chest", 1, 10, Dir::N), "iron-plate"));
    let mut pipe = entity("infinity-pipe", 2, 10, Dir::N);
    pipe.fluid = Some("sulfuric-acid".to_owned());
    list.push(pipe);
    let pole_start = list.len();
    for (name, x) in [("small-electric-pole", 0), ("medium-electric-pole", 5), ("big-electric-pole", 10), ("substation", 14)] {
        list.push(entity(name, x, 12, Dir::N));
    }
    let mut layout = Layout::new(20, 15, list);
    layout.wires = vec![(pole_start, pole_start + 1), (pole_start + 2, pole_start + 1), (pole_start + 3, pole_start + 2)];
    layout.label = "all types".to_owned();
    layout
}

fn stripped(l: &Layout) -> Vec<PlacedEntity> {
    l.entities.iter().map(PlacedEntity::stripped).collect()
}

fn unordered(wires: &[(usize, usize)]) -> Vec<(usize, usize)> {
    wires.iter().map(|&(a, b)| (a.min(b), a.max(b))).collect()
}

#[test]
fn round_trip_keeps_every_entity_type() {
    let layout = all_types();
    let decoded = decode_blueprint(&layout.to_blueprint_string("normal")).unwrap().to_layout(layout.w, layout.h).unwrap();
    assert_eq!(stripped(&decoded), stripped(&layout));
    assert_eq!(unordered(&decoded.wires), unordered(&layout.wires));
    assert_eq!(decoded.label, layout.label);
}

#[test]
fn json_uses_2_0_conventions() {
    let layout = Layout::new(
        10,
        4,
        vec![
            with_recipe(entity("assembling-machine-3", 3, 1, Dir::N), "iron-gear-wheel"),
            entity("transport-belt", 0, 0, Dir::E),
            underground("underground-belt", 1, 0, "input"),
            underground("underground-belt", 4, 0, "output"),
            entity("inserter", 2, 2, Dir::S),
            entity("inserter", 2, 3, Dir::W),
            entity("steel-chest", 5, 3, Dir::N),
        ],
    );
    let json: Value = serde_json::from_str(&decode_to_json(&layout.to_blueprint_string("normal")).unwrap()).unwrap();
    let bp = &json["blueprint"];
    assert_eq!(bp["version"], 562_949_954_076_673u64);
    assert_eq!(bp["item"], "blueprint");
    let ents = bp["entities"].as_array().unwrap();
    assert_eq!(ents[0]["position"], serde_json::json!({"x": 4.5, "y": 2.5}), "positions are centres");
    assert_eq!(ents[0]["recipe"], "iron-gear-wheel");
    assert_eq!(ents[0]["recipe_quality"], "normal");
    assert!(ents[0].get("direction").is_none(), "north is omitted");
    assert_eq!(ents[1]["direction"], 4, "east belt");
    assert_eq!(ents[2]["type"], "input");
    assert_eq!(ents[3]["type"], "output");
    assert_eq!(ents[4]["direction"], 8, "inserter picking from the south");
    assert_eq!(ents[5]["direction"], 12);
    assert!(ents[6].get("direction").is_none(), "chests have no direction");
    assert_eq!(ents[0]["entity_number"], 1);
    assert_eq!(bp["icons"][0]["signal"], serde_json::json!({"name": "iron-gear-wheel"}), "2.0 item signals have no type");
    assert!(bp.get("tiles").is_none(), "no floor tiles, no key");
}

#[test]
fn the_icon_is_the_first_machines_recipe_else_the_first_entity() {
    let belt_first = Layout::new(5, 5, vec![entity("transport-belt", 0, 0, Dir::N), with_recipe(entity("assembling-machine-2", 1, 1, Dir::N), "iron-gear-wheel")]);
    assert_eq!(belt_first.to_blueprint("normal").icons[0].signal.name, "iron-gear-wheel");
    // A furnace carries no recipe, so the first entity gives the icon.
    let furnace = Layout::new(5, 5, vec![entity("transport-belt", 0, 0, Dir::N), entity("electric-furnace", 1, 1, Dir::N)]);
    assert_eq!(furnace.to_blueprint("normal").icons[0].signal.name, "transport-belt");
    assert!(Layout::new(5, 5, vec![]).to_blueprint("normal").icons.is_empty());
}

#[test]
fn a_furnaces_layout_recipe_still_picks_the_icon() {
    // The blueprint entity leaves a furnace's recipe out, but the layout's recipe gives the icon.
    let furnace = with_recipe(entity("electric-furnace", 0, 0, Dir::N), "iron-plate");
    let bp = Layout::new(3, 3, vec![entity("transport-belt", 5, 5, Dir::N), furnace]).to_blueprint("normal");
    assert_eq!(bp.icons[0].signal.name, "iron-plate");
    assert_eq!(bp.entities[1].recipe, None);
}

#[test]
fn wires_are_copper_tuples_with_the_lower_entity_first() {
    let mut l = Layout::new(10, 3, vec![entity("medium-electric-pole", 0, 0, Dir::N), entity("medium-electric-pole", 6, 0, Dir::N)]);
    l.wires = vec![(1, 0)];
    assert_eq!(l.to_blueprint("normal").wires, vec![vec![1, 5, 2, 5]]);
}

#[test]
fn furnaces_carry_no_recipe_and_machines_carry_the_quality() {
    let mut furnace = entity("electric-furnace", 0, 0, Dir::N);
    furnace.recipe = Some("steel-plate".to_owned());
    let bp = Layout::new(3, 3, vec![furnace, with_recipe(entity("assembling-machine-3", 4, 0, Dir::N), "iron-gear-wheel")]).to_blueprint("rare");
    assert_eq!((bp.entities[0].recipe.as_deref(), bp.entities[0].recipe_quality.as_deref()), (None, None));
    assert_eq!(bp.entities[1].recipe_quality.as_deref(), Some("rare"));
}

#[test]
fn a_mirrored_machine_keeps_its_mirror() {
    let mut machine = with_recipe(entity("chemical-plant", 0, 0, Dir::E), "plastic-bar");
    machine.mirror = true;
    let layout = Layout::new(5, 5, vec![machine.clone(), entity("assembling-machine-3", 4, 0, Dir::N)]);
    let back = decode_blueprint(&layout.to_blueprint_string("normal")).unwrap().to_layout(5, 5).unwrap();
    assert!(back.entities[0].mirror && !back.entities[1].mirror);
    assert_eq!(back.entities[0], machine);
}

#[test]
fn infinity_chests_and_pipes_keep_their_settings() {
    let mut drain = entity("infinity-pipe", 0, 0, Dir::N);
    drain.fluid = Some("water".to_owned());
    drain.drain = true;
    let mut full = entity("infinity-pipe", 1, 0, Dir::N);
    full.fluid = Some("crude-oil".to_owned());
    let void = entity("infinity-chest", 2, 0, Dir::N);
    let supply = with_item(entity("infinity-chest", 3, 0, Dir::N), "iron-plate");
    let layout = Layout::new(4, 1, vec![drain, full, void, supply]);
    let back = decode_blueprint(&layout.to_blueprint_string("normal")).unwrap().to_layout(4, 1).unwrap();
    assert_eq!(back.entities, layout.entities);
    assert!(back.entities[0].drain && !back.entities[1].drain);
    assert_eq!(back.entities[2].item, None, "a chest without an item voids what it is given");
}

#[test]
fn infinity_settings_match_the_2_0_format() {
    let supply = with_item(entity("infinity-chest", 0, 0, Dir::N), "iron-plate");
    let mut pipe = entity("infinity-pipe", 1, 0, Dir::N);
    pipe.fluid = Some("water".to_owned());
    let bp = Layout::new(2, 1, vec![supply, pipe]).to_blueprint("normal");
    let chest = Value::Object(bp.entities[0].infinity_settings.clone().unwrap());
    assert_eq!(
        chest,
        serde_json::json!({"remove_unfiltered_items": true, "filters": [{"index": 1, "name": "iron-plate", "count": 50, "mode": "exactly"}]})
    );
    let pipe = Value::Object(bp.entities[1].infinity_settings.clone().unwrap());
    assert_eq!(pipe, serde_json::json!({"name": "water", "percentage": 1.0, "mode": "at-least"}));
}

#[test]
fn filtered_inserters_and_chests_round_trip_their_filter() {
    let filtered = with_item(entity("fast-inserter", 0, 0, Dir::N), "iron-plate");
    let chest = with_item(entity("steel-chest", 1, 0, Dir::N), "copper-plate");
    let plain = entity("inserter", 2, 0, Dir::N);
    let layout = Layout::new(3, 1, vec![filtered, chest, plain]);
    let bp = layout.to_blueprint("normal");
    assert_eq!(bp.entities[0].use_filters, Some(true));
    assert_eq!(bp.entities[1].use_filters, None, "a chest has filters but no use_filters");
    assert_eq!(bp.entities[2].filters, None);
    let back = bp.to_layout(3, 1).unwrap();
    assert_eq!(back.entities, layout.entities);
}

#[test]
fn a_blacklist_inserter_has_no_item() {
    let mut bp = Layout::new(1, 1, vec![with_item(entity("fast-inserter", 0, 0, Dir::N), "iron-plate")]).to_blueprint("normal");
    bp.entities[0].filter_mode = Some("blacklist".to_owned());
    assert_eq!(bp.to_layout(1, 1).unwrap().entities[0].item, None);
}

#[test]
fn floor_tiles_round_trip_and_sit_on_their_corner() {
    let mut layout = Layout::new(3, 3, vec![entity("transport-belt", 0, 0, Dir::N)]);
    layout.tiles = vec![GroundTile { name: "hazard-concrete-left".to_owned(), x: -1, y: 2 }];
    let bp = layout.to_blueprint("normal");
    let json: Value = serde_json::from_str(&bp.to_json()).unwrap();
    assert_eq!(json["blueprint"]["tiles"][0], serde_json::json!({"name": "hazard-concrete-left", "position": {"x": -1, "y": 2}}));
    assert_eq!(bp.to_layout(3, 3).unwrap().tiles, layout.tiles);
}

#[test]
fn a_position_maps_back_to_the_top_left_tile() {
    let bp = Layout::new(9, 9, vec![entity("oil-refinery", 2, 3, Dir::N), entity("splitter", 6, 6, Dir::E), entity("big-electric-pole", 0, 7, Dir::N)]).to_blueprint("normal");
    assert_eq!((bp.entities[0].position.x, bp.entities[0].position.y), (4.5, 5.5));
    assert_eq!((bp.entities[1].position.x, bp.entities[1].position.y), (6.5, 7.0), "an east splitter is 1 wide, 2 tall");
    let back = bp.to_layout(9, 9).unwrap();
    assert_eq!((back.entities[0].x, back.entities[0].y), (2, 3));
    assert_eq!((back.entities[1].x, back.entities[1].y, back.entities[1].w, back.entities[1].h), (6, 6, 1, 2));
    assert_eq!((back.entities[2].x, back.entities[2].y), (0, 7));
}

#[test]
fn entities_come_back_in_entity_number_order() {
    let layout = Layout::new(4, 1, (0..4).map(|x| entity("transport-belt", x, 0, Dir::E)).collect());
    let mut bp = layout.to_blueprint("normal");
    bp.entities.reverse();
    let back = bp.to_layout(4, 1).unwrap();
    assert_eq!(back.entities.iter().map(|e| e.x).collect::<Vec<_>>(), [0, 1, 2, 3]);
}

#[test]
fn a_wire_to_an_unknown_entity_is_an_error() {
    let mut bp = Layout::new(1, 1, vec![entity("small-electric-pole", 0, 0, Dir::N)]).to_blueprint("normal");
    bp.wires = vec![vec![1, 5, 9, 5]];
    assert!(bp.to_layout(1, 1).unwrap_err().to_string().contains("unknown entity 9"));
}

#[test]
fn wires_on_other_connectors_are_ignored() {
    let mut bp = Layout::new(2, 1, vec![entity("small-electric-pole", 0, 0, Dir::N), entity("small-electric-pole", 1, 0, Dir::N)]).to_blueprint("normal");
    bp.wires = vec![vec![1, 1, 2, 1]];
    assert!(bp.to_layout(2, 1).unwrap().wires.is_empty());
}

#[test]
fn bad_strings_are_errors() {
    assert!(decode_blueprint("1eJwDAAAAAAE=").unwrap_err().to_string().contains("version"));
    assert!(decode_blueprint("0!!!not base64").is_err());
    assert!(decode_blueprint(&format!("0{}", "AAAA")).is_err(), "valid base64, not zlib");
    assert!(decode_blueprint(&compress_and_encode("{\"nope\": 1}")).is_err(), "JSON, not a blueprint");
}

#[test]
fn encoding_inflates_back_to_the_same_json() {
    let json = "{\"blueprint\":{\"label\":\"x\"}}".repeat(50);
    let s = compress_and_encode(&json);
    assert!(s.starts_with('0'));
    assert!(s.len() < json.len(), "it is compressed");
    assert_eq!(decode_to_json(&s).unwrap(), json);
    assert_eq!(decode_to_json(&format!("  {s}\n")).unwrap(), json, "surrounding whitespace is trimmed");
}

/// Every blueprint string committed under docs/ came from the Kotlin generator (and several were pasted into
/// Factorio), so the Rust codec must read and write them back unchanged.
fn committed_blueprints() -> Vec<(String, String)> {
    let mut found = Vec::new();
    for parent in ["docs/examples", "docs/status"] {
        for dir in std::fs::read_dir(common::repo_root().join(parent)).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()) {
            let file = dir.join("best_output_string.txt");
            if file.exists() {
                found.push((dir.file_name().unwrap().to_string_lossy().into_owned(), std::fs::read_to_string(file).unwrap()));
            }
        }
    }
    assert!(found.len() >= 8, "found only {} committed blueprints", found.len());
    found
}

#[test]
fn committed_blueprints_serialize_to_the_same_json() {
    for (name, string) in committed_blueprints() {
        let original = decode_to_json(&string).unwrap();
        let bp = decode_blueprint(&string).unwrap();
        assert_eq!(bp.to_json(), original, "{name}");
    }
}

#[test]
fn committed_blueprints_survive_layout_and_back() {
    for (name, string) in committed_blueprints() {
        let original = decode_blueprint(&string).unwrap();
        let layout = original.to_layout(0, 0).unwrap();
        assert!(!layout.entities.is_empty(), "{name}");
        // The icon comes from the generator's layout, where furnaces still carry their recipe; a blueprint does
        // not store it, so a decoded layout can pick another icon (six_science_9_blocks does).
        let back = factorio_optimizer::emit::blueprint::Blueprint { icons: original.icons.clone(), ..layout.to_blueprint("normal") };
        assert_eq!(back, original, "{name}");
    }
}

#[test]
fn committed_blueprints_survive_a_new_encoding() {
    for (name, string) in committed_blueprints() {
        let original = decode_blueprint(&string).unwrap();
        assert_eq!(decode_blueprint(&encode_blueprint(&original)).unwrap(), original, "{name}");
    }
}

#[test]
fn committed_blueprints_contain_what_the_examples_promise() {
    // The README's six-science factory marks its block borders with floor tiles.
    let factory = committed_blueprints().into_iter().find(|(n, _)| n == "six_science_factory").unwrap().1;
    let layout = decode_blueprint(&factory).unwrap().to_layout(0, 0).unwrap();
    assert!(!layout.tiles.is_empty());
    assert!(layout.count(Kind::Machine) >= 177, "177 machines in 3 blocks");
}
