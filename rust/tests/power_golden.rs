//! Pole placement and the test harness against Kotlin, on the committed blueprints with their poles stripped, plus a
//! check that the validator (a separate port) agrees with what the pole placer claims. The goldens are dumped by
//! rust/tools/kotlin/GoldenValidateTest.kt.

mod common;

use factorio_optimizer::config::run_configuration::RunConfiguration;
use factorio_optimizer::emit::blueprint::decode_blueprint;
use factorio_optimizer::geom::{Kind, Layout, PlacedEntity, Tile};
use factorio_optimizer::power::{build_harness, place_poles, pole_candidates, pole_lattice};
use factorio_optimizer::validate::{ErrorType, OutputSpec, SourceSpec, ValidationSpec, validate};
use serde::Deserialize;

#[derive(Deserialize)]
struct PowerCase {
    name: String,
    pole: String,
    ring: bool,
    lattice: bool,
    ok: bool,
    disconnected: bool,
    poles: Vec<Tile>,
    wires: Vec<(usize, usize)>,
    uncovered: Vec<Tile>,
}

#[derive(Debug, PartialEq, Deserialize)]
struct EntityJson {
    name: String,
    kind: String,
    x: i32,
    y: i32,
    dir: String,
    item: Option<String>,
    fluid: Option<String>,
    drain: bool,
}

#[derive(Deserialize)]
struct HarnessJson {
    entities: Vec<EntityJson>,
    /// Sorted by y, then x.
    tiles: Vec<Tile>,
    warnings: Vec<String>,
}

#[derive(Deserialize)]
struct Golden {
    example: String,
    job: String,
    w: i32,
    h: i32,
    sources: Vec<SourceSpec>,
    outputs: Vec<OutputSpec>,
    power: Vec<PowerCase>,
    harness: HarnessJson,
}

fn load_golden(name: &str) -> Golden {
    let path = common::repo_root().join("rust/tests/golden/power").join(format!("{name}.json"));
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))).unwrap()
}

/// The committed blueprint without poles and wires, as the dumper strips it.
fn bare_layout(g: &Golden) -> Layout {
    let text = std::fs::read_to_string(common::repo_root().join(&g.example).join("best_output_string.txt")).unwrap();
    let mut layout = decode_blueprint(text.trim()).unwrap().to_layout(g.w, g.h).unwrap();
    layout.entities.retain(|e| e.kind != Kind::Pole);
    layout.wires.clear();
    layout
}

/// Kotlin's enum constant for a `Kind` variant: `PipeUg` is `PIPE_UG`.
fn kotlin_kind(kind: Kind) -> String {
    let mut name = String::new();
    for (i, c) in format!("{kind:?}").chars().enumerate() {
        if i > 0 && c.is_uppercase() {
            name.push('_');
        }
        name.push(c.to_ascii_uppercase());
    }
    name
}

fn entity_json(e: &PlacedEntity) -> EntityJson {
    EntityJson { name: e.name.clone(), kind: kotlin_kind(e.kind), x: e.x, y: e.y, dir: format!("{:?}", e.dir), item: e.item.clone(), fluid: e.fluid.clone(), drain: e.drain }
}

fn tiles_of(entities: &[PlacedEntity]) -> Vec<Tile> {
    entities.iter().map(PlacedEntity::tile).collect()
}

fn check_poles(name: &str) {
    let g = load_golden(name);
    let bare = bare_layout(&g);
    assert_eq!(g.power.len(), 5, "{name}: the dumper's five placements");
    for case in &g.power {
        let what = format!("{name}, {}", case.name);
        let placed = if case.lattice {
            place_poles(&bare.entities, &[], &case.pole, &pole_lattice(g.w, g.h, &case.pole))
        } else {
            place_poles(&bare.entities, &pole_candidates(g.w, g.h, case.ring, &[]), &case.pole, &[])
        };
        assert_eq!(tiles_of(&placed.poles), case.poles, "{what}: poles");
        assert_eq!(placed.wires, case.wires, "{what}: wires");
        assert_eq!(tiles_of(&placed.uncovered), case.uncovered, "{what}: uncovered");
        assert_eq!((placed.ok, placed.disconnected), (case.ok, case.disconnected), "{what}: verdict");
        assert!(!case.poles.is_empty(), "{what}: Kotlin placed poles");

        // Independent of Kotlin: the validator finds a power problem exactly when the placer says it failed.
        let mut powered = Layout::new(g.w, g.h, bare.entities.clone());
        let first_pole = powered.entities.len();
        powered.entities.extend(placed.poles);
        powered.wires = placed.wires.iter().map(|&(a, b)| (a + first_pole, b + first_pole)).collect();
        let spec = ValidationSpec { pole_ring: i32::from(case.ring), require_power: true, check_rates: false, ..Default::default() };
        let power_errors: Vec<_> = validate(&powered, &spec)
            .into_iter()
            .filter(|e| matches!(e.kind, ErrorType::Unpowered | ErrorType::PolesDisconnected | ErrorType::WireTooLong))
            .collect();
        assert_eq!(power_errors.is_empty(), case.ok, "{what}: the validator sees {power_errors:?}");
    }
}

fn check_harness(name: &str) {
    let g = load_golden(name);
    let bare = bare_layout(&g);
    let config = RunConfiguration::load(common::repo_root().join(&g.job).to_str().unwrap()).unwrap();
    let built = build_harness(&g.sources, &bare.entities, g.w, g.h, &config, &g.outputs).unwrap();
    let entities: Vec<EntityJson> = built.entities.iter().map(entity_json).collect();
    assert!(!entities.is_empty(), "{name}: Kotlin built a harness");
    assert_eq!(entities, g.harness.entities, "{name}: entities");
    let mut tiles: Vec<Tile> = built.tiles.iter().copied().collect();
    tiles.sort_by_key(|t| (t.y, t.x));
    assert_eq!(tiles, g.harness.tiles, "{name}: tiles");
    assert_eq!(built.warnings, g.harness.warnings, "{name}: warnings");
}

#[test]
fn red_green_science_poles() {
    check_poles("red_green_science");
}

#[test]
fn chemical_science_poles() {
    check_poles("chemical_science");
}

#[test]
fn electromagnetic_science_poles() {
    check_poles("electromagnetic_science");
}

#[test]
fn red_green_science_harness() {
    check_harness("red_green_science");
}

#[test]
fn chemical_science_harness() {
    check_harness("chemical_science");
}

#[test]
fn electromagnetic_science_harness() {
    check_harness("electromagnetic_science");
}
