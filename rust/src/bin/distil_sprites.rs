//! Turns the game's `data-raw-dump.json` into the sprite table the window draws with (`sprite-table.json`).
//!
//! The table holds paths and geometry only — no image data — so it can live in the repository while the pictures stay
//! in the player's install. Frame layout is not in the PNGs: how many frames a sheet holds, how they are laid out and
//! how far each sprite is shifted are all in the prototype data, which is why this needs the dump at all.
//!
//! ```sh
//! factorio.exe --dump-data          # writes %APPDATA%/Factorio/script-output/data-raw-dump.json
//! cargo run --bin distil_sprites -- ../data-raw-dump.json ../sprite-table.json
//! cargo run --bin distil_sprites -- ../data-raw-dump.json --keys=chemical-plant   # what a prototype carries
//! ```
//!
//! What the dump says, for the record:
//! - a sheet is `columns` frames wide (`line_length`, defaulting to `frame_count`) and one row per direction; frame
//!   `n` of direction `d` sits at column `(d * frame_count + n) % columns`, row `(d * frame_count + n) / columns`;
//! - `width`/`height` (or `size`) are one frame in sheet pixels, `scale` shrinks it and `shift` moves it in tiles
//!   from the entity's centre;
//! - belts carry 16 baked directions (north 0, east 4, south 8, west 12) plus four ending variants, so a belt sheet
//!   has 20 rows; poles and other four-way sprites bake directions 0..3 across one row;
//! - an inserter's platform and a pipe's stub carry a single frame that the game rotates, hence `rotate` in the table;
//! - a machine's animation is either flat (`layers`) or keyed by side (`north: {layers}`), which is why an entity's
//!   sprites are stored as four side lists.

use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// Entities the layouts can place, by prototype name: the belts of every tier, the inserters, the machines this port
/// plans with, the poles, chests and pipes.
fn names() -> Vec<String> {
    let mut names = Vec::new();
    for tier in ["", "fast-", "express-", "turbo-"] {
        for kind in ["transport-belt", "underground-belt", "splitter"] {
            names.push(format!("{tier}{kind}"));
        }
    }
    for inserter in ["inserter", "long-handed-inserter", "fast-inserter", "bulk-inserter"] {
        names.push(inserter.to_owned());
    }
    for machine in [
        "assembling-machine-1",
        "assembling-machine-2",
        "assembling-machine-3",
        "stone-furnace",
        "steel-furnace",
        "electric-furnace",
        "chemical-plant",
        "oil-refinery",
        "centrifuge",
        "recycler",
        "foundry",
        "electromagnetic-plant",
        "cryogenic-plant",
        "biolab",
    ] {
        names.push(machine.to_owned());
    }
    for pole in ["small-electric-pole", "medium-electric-pole", "big-electric-pole", "substation"] {
        names.push(pole.to_owned());
    }
    for chest in ["wooden-chest", "iron-chest", "steel-chest"] {
        names.push(chest.to_owned());
    }
    for pipe in ["pipe", "pipe-to-ground"] {
        names.push(pipe.to_owned());
    }
    names
}

fn main() -> anyhow::Result<()> {
    use anyhow::Context;

    let mut args = std::env::args().skip(1);
    let dump_path = args.next().unwrap_or_else(|| "../data-raw-dump.json".to_owned());
    let out_path = args.next().unwrap_or_else(|| "../sprite-table.json".to_owned());
    let text = std::fs::read_to_string(&dump_path).with_context(|| format!("Could not read {dump_path}"))?;
    let dump: Value = serde_json::from_str(&text).with_context(|| format!("Could not parse {dump_path}"))?;
    let top = dump.as_object().context("the dump is an object")?;

    // Prototypes are grouped by type, and a name can appear more than once: "pipe" is an item, "fast-inserter" is a
    // technology, "steel-chest" is an item too. The entity group is the one with the sprites, so it wins.
    let entity_groups = [
        "assembling-machine",
        "furnace",
        "chemical-plant",
        "oil-refinery",
        "centrifuge",
        "recycler",
        "foundry",
        "electromagnetic-plant",
        "cryogenic-plant",
        "biolab",
        "lab",
        "inserter",
        "transport-belt",
        "underground-belt",
        "splitter",
        "electric-pole",
        "container",
        "pipe",
        "pipe-to-ground",
    ];
    let mut index: BTreeMap<&str, (&str, &Value)> = BTreeMap::new();
    for (group, value) in top {
        if let Some(prototypes) = value.as_object() {
            let is_entity = entity_groups.contains(&group.as_str());
            for (name, prototype) in prototypes {
                match index.get(name.as_str()) {
                    Some((seen, _)) if !is_entity || entity_groups.contains(seen) => {}
                    _ => {
                        index.insert(name.as_str(), (group.as_str(), prototype));
                    }
                }
            }
        }
    }

    // `--keys=name` prints the fields of one prototype that carry pictures, which is how the shapes above were found.
    if let Some(spec) = std::env::args().find_map(|a| a.strip_prefix("--keys=").map(str::to_owned)) {
        let name = spec.rsplit(':').next().unwrap_or(&spec);
        let (group, prototype) = index.get(name).with_context(|| format!("no prototype named {name}"))?;
        println!("{name} is in group {group}");
        if let Some(map) = prototype.as_object() {
            for (key, value) in map {
                let text = serde_json::to_string(value).unwrap_or_default();
                if text.contains("\"filename\"") {
                    let cut = text.len().min(700);
                    println!("{key}: {}{}", &text[..cut], if cut < text.len() { " …" } else { "" });
                }
            }
        }
        return Ok(());
    }

    let mut entities = Map::new();
    let mut missing = Vec::new();
    for name in names() {
        let Some((group, prototype)) = index.get(name.as_str()) else {
            missing.push(name);
            continue;
        };
        let sides = layers_of(group, prototype);
        if sides.iter().all(Vec::is_empty) {
            missing.push(format!("{name} ({group})"));
            continue;
        }
        entities.insert(name, json!({ "type": group, "sides": sides }));
    }

    let table = json!({
        "game": top.get("game-version").or_else(|| top.get("version")).and_then(|v| v.as_str()).unwrap_or("unknown"),
        "entities": entities,
    });
    std::fs::write(&out_path, serde_json::to_string_pretty(&table)?).with_context(|| format!("Could not write {out_path}"))?;
    println!(
        "wrote {out_path} ({} entities); no sprites for {}",
        table["entities"].as_object().map_or(0, Map::len),
        if missing.is_empty() { "none".to_owned() } else { missing.join(", ") }
    );
    Ok(())
}

/// The sprites that make one entity, per side (north, east, south, west), in drawing order. Shadows and glows are
/// left out: the dark backdrop and the tiles already carry the contrast.
fn layers_of(group: &str, prototype: &Value) -> [Vec<Value>; 4] {
    // Belts, undergrounds and splitters draw their surface from the tier's belt sheet. An underground also has a
    // structure sprite (the hole in the ground); its entrance one is used for every side, which reads better than a
    // plain belt in its place.
    if let Some(set) = prototype.get("belt_animation_set").and_then(|belt| belt.get("animation_set")) {
        let mut surface: Vec<Value> = layer(set, false).into_iter().collect();
        if let Some(entry) = prototype.get("structure").and_then(|s| s.get("direction_in")).and_then(|d| d.get("sheet")) {
            surface.extend(layer(entry, false));
        }
        return [surface.clone(), surface.clone(), surface.clone(), surface];
    }
    // Inserters: a platform the game rotates with the arm's direction. The arm needs the swing animation's pivot,
    // which is not worth guessing, so it is left out.
    if group == "inserter" {
        let platform: Vec<Value> = prototype
            .get("platform_picture")
            .and_then(|picture| picture.get("sheet"))
            .and_then(|sheet| layer(sheet, true))
            .into_iter()
            .collect();
        return [platform.clone(), platform.clone(), platform.clone(), platform];
    }
    // A pipe carries one stub per shape; the single straight stub reads correctly anywhere.
    if group == "pipe" || group == "pipe-to-ground" {
        let stub = prototype
            .get("pictures")
            .and_then(|pictures| pictures.get("straight_vertical_single").or_else(|| pictures.get("straight_vertical")))
            .and_then(|stub| layer(stub, true));
        if let Some(stub) = stub {
            return [vec![stub.clone()], vec![stub.clone()], vec![stub.clone()], vec![stub]];
        }
    }
    // Machines, furnaces, chests, poles.
    for path in ["graphics_set.animation", "graphics_set.pictures", "animation", "picture", "pictures"] {
        let Some(value) = get_path(prototype, path) else { continue };
        if let Some(list) = value.get("layers").and_then(Value::as_array) {
            let flat: Vec<Value> = list.iter().filter_map(|item| layer(item, false)).collect();
            if !flat.is_empty() {
                return [flat.clone(), flat.clone(), flat.clone(), flat];
            }
        }
        let sides = ["north", "east", "south", "west"].map(|side| {
            value
                .get(side)
                .and_then(|s| s.get("layers"))
                .and_then(Value::as_array)
                .map(|list| list.iter().filter_map(|item| layer(item, false)).collect::<Vec<_>>())
                .unwrap_or_default()
        });
        if sides.iter().all(|list| !list.is_empty()) {
            return sides;
        }
        if value.get("filename").is_some() {
            if let Some(single) = layer(value, false) {
                return [vec![single.clone()], vec![single.clone()], vec![single.clone()], vec![single]];
            }
        }
    }
    [Vec::new(), Vec::new(), Vec::new(), Vec::new()]
}

fn get_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for part in path.split('.') {
        current = current.get(part)?;
    }
    Some(current)
}

/// One sheet, as the renderer wants it. `rotate` marks a single frame the game turns to face a direction; `directions`
/// says how many directions are baked into the sheet (1, 4 or 20).
fn layer(sprite: &Value, rotate: bool) -> Option<Value> {
    if sprite.get("draw_as_shadow").and_then(Value::as_bool).unwrap_or(false) {
        return None;
    }
    if sprite.get("draw_as_glow").and_then(Value::as_bool).unwrap_or(false) {
        return None;
    }
    let file = sprite.get("filename")?.as_str()?;
    if file.contains("circuit-connector") || file.contains("/frozen/") {
        return None;
    }
    let number = |key: &str| sprite.get(key).and_then(Value::as_f64);
    let width = number("width").or_else(|| number("size")).unwrap_or(0.0);
    let height = number("height").or_else(|| number("size")).unwrap_or(0.0);
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let frames = number("frame_count").unwrap_or(1.0).max(1.0);
    let dirs = number("direction_count").unwrap_or(1.0).max(1.0);
    let columns = number("line_length").unwrap_or(frames).max(1.0);
    let shift = sprite.get("shift").and_then(Value::as_array);
    let shift = |index: usize| shift.and_then(|s| s.get(index)).and_then(Value::as_f64).unwrap_or(0.0);
    Some(json!({
        "file": asset_path(file),
        "w": width,
        "h": height,
        "columns": columns,
        "frame_count": frames,
        "directions": dirs,
        "shift": [shift(0), shift(1)],
        "scale": number("scale").unwrap_or(1.0),
        "offset": [number("x").unwrap_or(0.0), number("y").unwrap_or(0.0)],
        "rotate": rotate,
    }))
}

/// `__base__/graphics/entity/x.png` → `base/graphics/entity/x.png`, the path under the install's `data` directory.
fn asset_path(file: &str) -> String {
    file.strip_prefix("__").map_or_else(|| file.to_owned(), |rest| rest.replacen("__/", "/", 1))
}
