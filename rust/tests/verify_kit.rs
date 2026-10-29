//! The in-game verification kit: the layouts are the ones Kotlin builds (entity by entity, from a dump), they are valid
//! enough to import (no overlaps, inserters that pick from something) and survive a blueprint round trip, and the
//! strings written for them decode to the committed docs/verify files and to what Kotlin wrote.

mod common;

use common::{LayoutJson, assert_no_differences, compare, compare_layout, read};
use factorio_optimizer::data::loader::load_root_data;
use factorio_optimizer::emit::blueprint::decode_blueprint;
use factorio_optimizer::emit::encode::decode_to_json;
use factorio_optimizer::emit::verify_kit::{cases, t1_hand, write};
use factorio_optimizer::validate::{ErrorType, ValidationSpec, recipe_info_from, validate};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;

#[derive(Deserialize)]
struct CaseJson {
    id: String,
    title: String,
    check: String,
    layout: LayoutJson,
}

#[derive(Deserialize)]
struct Golden {
    cases: Vec<CaseJson>,
    t1_hand: LayoutJson,
}

fn golden() -> Golden {
    serde_json::from_str(&read(common::repo_root().join("rust/tests/golden/verify/verify_kit.json"))).unwrap()
}

#[test]
fn the_kit_layouts_are_the_ones_kotlin_builds() {
    let want = golden();
    let got = cases();
    assert_eq!(got.len(), 18);
    let mut out = Vec::new();
    compare("case ids in order", &got.iter().map(|c| c.id).collect::<Vec<_>>(), &want.cases.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), &mut out);
    for (g, w) in got.iter().zip(&want.cases) {
        compare(&format!("{} title", g.id), &g.title, &w.title.as_str(), &mut out);
        compare(&format!("{} check", g.id), &g.check, &w.check.as_str(), &mut out);
        let mut diffs = Vec::new();
        compare_layout(&g.layout, &w.layout, &mut diffs);
        out.extend(diffs.into_iter().map(|d| format!("{}: {d}", g.id)));
    }
    compare_layout(&t1_hand(), &want.t1_hand, &mut out);
    assert_no_differences("the verify kit", out);
}

#[test]
fn the_kit_layouts_have_no_overlaps_and_round_trip() {
    let data = load_root_data("recipes.json").unwrap();
    for c in cases() {
        let l = &c.layout;
        let spec = ValidationSpec {
            recipe_info: recipe_info_from(&data),
            require_power: false,
            check_rates: false,
            outside_allowed: l.entities.iter().flat_map(|e| e.rect().tiles()).collect::<HashSet<_>>(),
            ..Default::default()
        };
        let errors = validate(l, &spec);
        assert!(errors.iter().all(|e| e.kind != ErrorType::Overlap && e.kind != ErrorType::InserterBadPickup), "{}: {errors:?}", c.id);
        let back = l.to_blueprint("normal").to_layout(l.w, l.h).unwrap();
        let strip = |l: &factorio_optimizer::geom::Layout| l.entities.iter().map(|e| e.stripped()).collect::<Vec<_>>();
        assert_eq!(strip(l), strip(&back), "{}", c.id);
    }
}

fn blueprint_json(text: &str) -> Value {
    serde_json::from_str(&decode_to_json(text.trim()).unwrap()).unwrap()
}

#[test]
fn the_strings_it_writes_are_the_ones_in_docs_verify_and_the_ones_kotlin_wrote() {
    let dir = std::env::temp_dir().join(format!("factorio-verify-kit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(write(&dir).unwrap(), 18);
    let mut files: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    files.sort();
    assert_eq!(files.len(), 19, "{files:?}");
    for f in &files {
        let mine = blueprint_json(&read(dir.join(f)));
        let docs = common::repo_root().join("docs/verify").join(f);
        assert_eq!(mine, blueprint_json(&read(docs)), "{f} against docs/verify");
        let kotlin = common::repo_root().join("rust/tests/golden/verify/written").join(f);
        assert_eq!(mine, blueprint_json(&read(kotlin)), "{f} against Kotlin's output");
        // And each one is a blueprint the codec reads back.
        assert!(decode_blueprint(read(dir.join(f)).trim()).is_ok());
    }
    let _ = std::fs::remove_dir_all(&dir);
}
