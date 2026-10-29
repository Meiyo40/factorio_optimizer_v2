#![allow(dead_code)]

use factorio_optimizer::geom::{Dir, Kind, Layout, PlacedEntity};
use factorio_optimizer::validate::ValidationError;
use serde::Deserialize;
use std::fmt::Debug;
use std::path::{Path, PathBuf};

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("the crate lives in the repo").to_path_buf()
}

/// Files with extension `ext` directly inside `dir` (relative to the repo root), sorted.
pub fn files_in(dir: &str, ext: &str) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(repo_root().join(dir))
        .unwrap_or_else(|e| panic!("cannot read {dir}: {e}"))
        .map(|entry| entry.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == ext))
        .collect();
    found.sort();
    found
}

/// name, kind, x, y, dir, recipe, mirror, w, h, item, fluid, drain, net (a tuple struct: tuples of 13 have no Debug)
#[derive(Deserialize, Debug, PartialEq)]
pub struct EntityJson(
    pub String,
    pub String,
    pub i32,
    pub i32,
    pub String,
    pub Option<String>,
    pub bool,
    pub i32,
    pub i32,
    pub Option<String>,
    pub Option<String>,
    pub bool,
    pub Option<i32>,
);

pub fn read(path: PathBuf) -> String {
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

pub fn dir_of(s: &str) -> Dir {
    match s {
        "N" => Dir::N,
        "E" => Dir::E,
        "S" => Dir::S,
        "W" => Dir::W,
        other => panic!("unknown direction {other}"),
    }
}

/// Kotlin's enum constant for a `Kind` variant: `PipeUg` is `PIPE_UG`.
pub fn kotlin_kind(kind: factorio_optimizer::geom::Kind) -> String {
    let mut name = String::new();
    for (i, c) in format!("{kind:?}").chars().enumerate() {
        if i > 0 && c.is_uppercase() {
            name.push('_');
        }
        name.push(c.to_ascii_uppercase());
    }
    name
}

pub fn entity_json(e: &PlacedEntity) -> EntityJson {
    EntityJson(e.name.clone(), kotlin_kind(e.kind), e.x, e.y, format!("{:?}", e.dir), e.recipe.clone(), e.mirror, e.w, e.h, e.item.clone(), e.fluid.clone(), e.drain, e.net)
}

pub fn compare<T: PartialEq + Debug>(what: &str, got: &T, want: &T, out: &mut Vec<String>) {
    if got != want {
        out.push(format!("{what}: rust {got:?}, kotlin {want:?}"));
    }
}

/// Lists: the length, and the first element that differs.
pub fn compare_list<T: PartialEq + Debug>(what: &str, got: &[T], want: &[T], out: &mut Vec<String>) {
    if got == want {
        return;
    }
    let first = got.iter().zip(want).position(|(a, b)| a != b);
    match first {
        Some(i) => out.push(format!("{what}: {} vs {} entries, first difference at #{i}\n      rust:   {:?}\n      kotlin: {:?}", got.len(), want.len(), got[i], want[i])),
        None => out.push(format!("{what}: {} entries in rust, {} in kotlin (the shorter is a prefix); next: {:?}", got.len(), want.len(), got.get(want.len()).or(want.get(got.len())))),
    }
}

const ALL_KINDS: [Kind; 12] = [
    Kind::Machine,
    Kind::Belt,
    Kind::UgIn,
    Kind::UgOut,
    Kind::Splitter,
    Kind::Inserter,
    Kind::Chest,
    Kind::Pole,
    Kind::InfinityPipe,
    Kind::InfinityChest,
    Kind::Pipe,
    Kind::PipeUg,
];

/// The `Kind` of Kotlin's enum constant (`PIPE_UG`).
pub fn kind_from_kotlin(name: &str) -> Kind {
    ALL_KINDS.iter().copied().find(|&k| kotlin_kind(k) == name).unwrap_or_else(|| panic!("unknown kind {name}"))
}

pub fn entity_from_json(j: &EntityJson) -> PlacedEntity {
    PlacedEntity {
        name: j.0.clone(),
        kind: kind_from_kotlin(&j.1),
        x: j.2,
        y: j.3,
        dir: dir_of(&j.4),
        recipe: j.5.clone(),
        item: j.9.clone(),
        fluid: j.10.clone(),
        net: j.12,
        mirror: j.6,
        drain: j.11,
        w: j.7,
        h: j.8,
    }
}

/// The differences of a golden run as one panic message, or nothing.
pub fn assert_no_differences(case: &str, out: Vec<String>) {
    assert!(out.is_empty(), "{case} differs from Kotlin:\n  {}", out.join("\n  "));
}


#[derive(Deserialize)]
pub struct ErrorJson {
    #[serde(rename = "type")]
    pub kind: String,
    pub x: i32,
    pub y: i32,
    pub message: String,
}

/// A layout as the Kotlin dumpers write it.
#[derive(Deserialize)]
pub struct LayoutJson {
    pub w: i32,
    pub h: i32,
    pub label: String,
    pub entities: Vec<EntityJson>,
    pub wires: Vec<(usize, usize)>,
    pub tiles: Vec<(String, i32, i32)>,
}

pub fn compare_errors(got: &[ValidationError], want: &[ErrorJson], out: &mut Vec<String>) {
    let got: Vec<(String, i32, i32, String)> = got.iter().map(|e| (e.kind.name().to_owned(), e.tile.x, e.tile.y, e.message.clone())).collect();
    let want: Vec<(String, i32, i32, String)> = want.iter().map(|e| (e.kind.clone(), e.x, e.y, e.message.clone())).collect();
    compare_list("errors", &got, &want, out);
}

pub fn compare_layout(got: &Layout, want: &LayoutJson, out: &mut Vec<String>) {
    compare("layout size and label", &(got.w, got.h, got.label.as_str()), &(want.w, want.h, want.label.as_str()), out);
    let entities: Vec<EntityJson> = got.entities.iter().map(entity_json).collect();
    compare_list("layout entities", &entities, &want.entities, out);
    compare_list("layout wires", &got.wires, &want.wires, out);
    let tiles: Vec<(String, i32, i32)> = got.tiles.iter().map(|t| (t.name.clone(), t.x, t.y)).collect();
    compare_list("layout tiles", &tiles, &want.tiles, out);
}
