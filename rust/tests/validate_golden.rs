//! The Rust validator against the Kotlin one: every committed blueprint, as committed and with seeded mutations
//! (dropped, turned and moved entities, refiltered inserters), must give the same errors in the same order, message
//! text included. The goldens are dumped by rust/tools/kotlin/GoldenValidateTest.kt.

mod common;

use factorio_optimizer::data::data_model::RootData;
use factorio_optimizer::data::loader::load_root_data;
use factorio_optimizer::emit::blueprint::decode_blueprint;
use factorio_optimizer::geom::{Layout, Tile};
use factorio_optimizer::validate::{MachineSpec, OutputSpec, SourceSpec, ValidationSpec, recipe_info_from, validate};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

static DATA: LazyLock<RootData> = LazyLock::new(|| load_root_data("recipes.json").unwrap());

#[derive(Deserialize)]
struct MachineJson {
    tile: Tile,
    spec: MachineSpec,
}

#[derive(Deserialize)]
struct SpecJson {
    pole_ring: i32,
    require_power: bool,
    fluid_mode: String,
    check_rates: bool,
    /// name -> [not from a belt, from a belt]
    inserter_rate: HashMap<String, [f64; 2]>,
    machines: Vec<MachineJson>,
    sources: Vec<SourceSpec>,
    outputs: Vec<OutputSpec>,
    outside_allowed: Vec<Tile>,
}

#[derive(Deserialize)]
struct Op {
    op: String,
    index: usize,
    dx: i32,
    dy: i32,
    item: Option<String>,
}

#[derive(Deserialize)]
struct ErrorJson {
    #[serde(rename = "type")]
    kind: String,
    x: i32,
    y: i32,
    message: String,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    ops: Vec<Op>,
    errors: Vec<ErrorJson>,
}

#[derive(Deserialize)]
struct Golden {
    example: String,
    w: i32,
    h: i32,
    spec: SpecJson,
    cases: Vec<Case>,
}

fn spec_of(s: &SpecJson) -> ValidationSpec {
    let rates = s.inserter_rate.clone();
    ValidationSpec {
        machines: s.machines.iter().map(|m| (m.tile, m.spec.clone())).collect(),
        sources: s.sources.clone(),
        outputs: s.outputs.clone(),
        pole_ring: s.pole_ring,
        outside_allowed: s.outside_allowed.iter().copied().collect(),
        require_power: s.require_power,
        fluid_mode: s.fluid_mode.clone(),
        check_rates: s.check_rates,
        inserter_rate: Arc::new(move |name, from_belt| rates[name][usize::from(from_belt)]),
        recipe_info: recipe_info_from(&DATA),
    }
}

/// The dumper's operations: remove drops the entity and its wires and renumbers the rest, rotate is a clockwise
/// quarter turn, move shifts by (dx, dy), set_item refilters.
fn apply(mut l: Layout, o: &Op) -> Layout {
    match o.op.as_str() {
        "remove" => {
            l.entities.remove(o.index);
            let renumber = |i: usize| if i > o.index { i - 1 } else { i };
            l.wires = l.wires.iter().filter(|&&(a, b)| a != o.index && b != o.index).map(|&(a, b)| (renumber(a), renumber(b))).collect();
        }
        "rotate" => l.entities[o.index].dir = l.entities[o.index].dir.cw(),
        "move" => {
            l.entities[o.index].x += o.dx;
            l.entities[o.index].y += o.dy;
        }
        "set_item" => l.entities[o.index].item = o.item.clone(),
        other => panic!("unknown operation {other}"),
    }
    l
}

fn load_golden(name: &str) -> Golden {
    let path = common::repo_root().join("rust/tests/golden/validate").join(format!("{name}.json"));
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))).unwrap()
}

fn committed_layout(g: &Golden) -> Layout {
    let text = std::fs::read_to_string(common::repo_root().join(&g.example).join("best_output_string.txt")).unwrap();
    decode_blueprint(text.trim()).unwrap().to_layout(g.w, g.h).unwrap()
}

fn check(name: &str) {
    let g = load_golden(name);
    let base = committed_layout(&g);
    let spec = spec_of(&g.spec);
    let mut mismatches: Vec<String> = Vec::new();
    for case in &g.cases {
        let layout = case.ops.iter().fold(base.clone(), apply);
        let got: Vec<String> = validate(&layout, &spec).iter().map(|e| format!("{} {},{} {}", e.kind.name(), e.tile.x, e.tile.y, e.message)).collect();
        let want: Vec<String> = case.errors.iter().map(|e| format!("{} {},{} {}", e.kind, e.x, e.y, e.message)).collect();
        if got != want {
            let first = got.iter().zip(&want).position(|(a, b)| a != b).unwrap_or(got.len().min(want.len()));
            mismatches.push(format!(
                "{}: rust {} errors, kotlin {}; first difference at #{first}\n    rust:   {}\n    kotlin: {}",
                case.name,
                got.len(),
                want.len(),
                got.get(first).map_or("(none)", String::as_str),
                want.get(first).map_or("(none)", String::as_str),
            ));
        }
    }
    assert!(mismatches.is_empty(), "{name}: {} of {} cases differ\n{}", mismatches.len(), g.cases.len(), mismatches.iter().take(5).cloned().collect::<Vec<_>>().join("\n"));
    // The corpus must tell mutations from the original, or agreeing on it proves little.
    if g.cases.len() > 1 {
        let committed = &g.cases[0].errors;
        assert!(g.cases[1..].iter().any(|c| c.errors.len() != committed.len()), "{name}: no mutation changed the verdict");
    }
}

#[test]
fn red_green_science() {
    check("red_green_science");
}

#[test]
fn red_green_science_shared() {
    check("red_green_science_shared");
}

#[test]
fn chemical_science() {
    check("chemical_science");
}

#[test]
fn electromagnetic_science() {
    check("electromagnetic_science");
}

#[test]
fn metallurgic_science() {
    check("metallurgic_science");
}

#[test]
fn gleba_agricultural_carbon_fiber() {
    check("gleba_agricultural_carbon_fiber");
}

#[test]
fn six_science_factory() {
    check("six_science_factory");
}

/// Predates the curve-lane fix, so Kotlin reports 1433 errors on it: a large negative case.
#[test]
fn six_science_9_blocks() {
    check("six_science_9_blocks");
}
