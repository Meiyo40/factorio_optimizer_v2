//! The Java emulations against real java.util and Formatter output, dumped by rust/tools/kotlin/GoldenValidateTest.kt.

mod common;

use factorio_optimizer_v2_alias::*;
use serde::Deserialize;

mod factorio_optimizer_v2_alias {
    pub use factorio_optimizer::data::data_model::RootData;
    pub use factorio_optimizer::data::loader::load_root_data;
    pub use factorio_optimizer::geom::Tile;
    pub use factorio_optimizer::util::java_hash::{JavaHash, java_order};
    pub use factorio_optimizer::util::text::fmt2;
    pub use factorio_optimizer::validate::{MachineSpec, recipe_info_from};
}

fn golden(name: &str) -> String {
    std::fs::read_to_string(common::repo_root().join("rust/tests/golden").join(name)).unwrap()
}

/// Positions in iteration order, applied to the keys in insertion order.
fn reordered<K: JavaHash + Clone>(keys: &[K]) -> Vec<K> {
    java_order(keys).into_iter().map(|i| keys[i].clone()).collect()
}

#[derive(Deserialize)]
struct IntOrder {
    keys: Vec<i32>,
    set_order: Vec<i32>,
    map_order: Vec<i32>,
}

#[derive(Deserialize)]
struct StringOrder {
    keys: Vec<String>,
    order: Vec<String>,
}

#[derive(Deserialize)]
struct StringHash {
    s: String,
    hash: i32,
}

#[derive(Deserialize)]
struct PairOrder {
    keys: Vec<(i32, String)>,
    order: Vec<(i32, String)>,
}

#[derive(Deserialize)]
struct TileOrder {
    keys: Vec<Tile>,
    order: Vec<Tile>,
}

#[derive(Deserialize)]
struct Fmt {
    value: f64,
    text: String,
}

#[derive(Deserialize)]
struct Samples {
    int_orders: Vec<IntOrder>,
    string_orders: Vec<StringOrder>,
    string_hashes: Vec<StringHash>,
    pair_orders: Vec<PairOrder>,
    tile_orders: Vec<TileOrder>,
    fmt2: Vec<Fmt>,
}

fn samples() -> Samples {
    serde_json::from_str(&golden("java_samples.json")).unwrap()
}

/// Multiples of 64 all share bucket 0 of a 64-slot table: the dumper's probe of the treeify rule.
fn is_collision_probe(o: &IntOrder) -> bool {
    o.keys.iter().all(|k| k % 64 == 0)
}

#[test]
fn integer_keys_iterate_like_java_hash_sets_and_maps() {
    let s = samples();
    let spread: Vec<&IntOrder> = s.int_orders.iter().filter(|o| !is_collision_probe(o)).collect();
    assert_eq!(spread.len(), s.int_orders.len() - 1, "exactly one probe");
    for o in spread {
        assert_eq!(o.set_order, o.map_order, "a HashSet is a HashMap, so they agree");
        assert_eq!(reordered(&o.keys), o.set_order, "{} keys", o.keys.len());
    }
}

/// Java lists a treeified bucket in a different order than a chain; the model refuses such keys rather than guess.
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "not modelled")]
fn colliding_keys_are_rejected() {
    let s = samples();
    let probe = s.int_orders.iter().find(|o| is_collision_probe(o)).unwrap();
    java_order(&probe.keys);
}

#[test]
fn string_hash_codes_match_java() {
    for h in &samples().string_hashes {
        assert_eq!(h.s.java_hash(), h.hash, "{}", h.s);
    }
}

#[test]
fn string_keys_iterate_like_java() {
    for o in &samples().string_orders {
        assert_eq!(reordered(&o.keys), o.order, "{} strings", o.keys.len());
    }
}

#[test]
fn pair_keys_iterate_like_java() {
    for o in &samples().pair_orders {
        let keys: Vec<(usize, String)> = o.keys.iter().map(|(a, b)| (*a as usize, b.clone())).collect();
        let order: Vec<(usize, String)> = o.order.iter().map(|(a, b)| (*a as usize, b.clone())).collect();
        assert_eq!(reordered(&keys), order, "{} pairs", keys.len());
    }
}

#[test]
fn tile_keys_iterate_like_java() {
    for o in &samples().tile_orders {
        assert_eq!(reordered(&o.keys), o.order, "{} tiles", o.keys.len());
    }
}

#[test]
fn fmt2_matches_java_string_format() {
    let s = samples();
    assert!(s.fmt2.len() > 150);
    for f in &s.fmt2 {
        assert_eq!(fmt2(f.value), f.text, "%.2f of {}", f.value);
    }
}

#[test]
fn recipe_info_matches_kotlin_for_every_recipe() {
    let data = load_root_data("recipes.json").unwrap();
    let info = recipe_info_from(&data);
    let kotlin: indexmap::IndexMap<String, MachineSpec> = serde_json::from_str(&golden("recipe_info.json")).unwrap();
    assert!(kotlin.len() > 600);
    for (name, want) in &kotlin {
        let got = info(name).unwrap_or_else(|| panic!("no recipe info for {name}"));
        assert_eq!(&got, want, "{name}");
    }
    assert!(info("no-such-recipe").is_none());
    let _: &RootData = &data;
}
