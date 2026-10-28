//! The branches of pole placement the Kotlin goldens never reach: machines too far apart for one wire. The
//! validator, a separate port, is the judge of what the placer built.

use factorio_optimizer::data::game_constants as gc;
use factorio_optimizer::geom::{Dir, Kind, Layout, PlacedEntity, Tile};
use factorio_optimizer::power::{PowerResult, place_poles, pole_candidates};
use factorio_optimizer::validate::{ErrorType, ValidationSpec, validate};

const POLE: &str = "medium-electric-pole";
const W: i32 = 40;
const H: i32 = 5;

/// Two machines at opposite ends of a long thin box.
fn machines() -> Vec<PlacedEntity> {
    [0, W - 3]
        .into_iter()
        .map(|x| {
            let mut m = PlacedEntity::new("assembling-machine-1", Kind::Machine, x, 1, Dir::N);
            m.recipe = Some("iron-gear-wheel".to_owned());
            m
        })
        .collect()
}

fn power_errors(entities: &[PlacedEntity], placed: &PowerResult) -> Vec<ErrorType> {
    let mut layout = Layout::new(W, H, entities.to_vec());
    let first_pole = layout.entities.len();
    layout.entities.extend(placed.poles.iter().cloned());
    layout.wires = placed.wires.iter().map(|&(a, b)| (a + first_pole, b + first_pole)).collect();
    let spec = ValidationSpec { require_power: true, check_rates: false, ..Default::default() };
    validate(&layout, &spec)
        .into_iter()
        .map(|e| e.kind)
        .filter(|k| matches!(k, ErrorType::Unpowered | ErrorType::PolesDisconnected | ErrorType::WireTooLong))
        .collect()
}

#[test]
fn machines_out_of_wire_reach_are_joined_by_bridge_poles() {
    let entities = machines();
    let placed = place_poles(&entities, &pole_candidates(W, H, false, &[]), POLE, &[]);
    let reach = gc::pole(POLE).unwrap().wire_reach;
    assert!(f64::from(W - 6) > reach, "the machines really are out of reach of each other");
    assert!(placed.poles.len() > 2, "one pole per machine plus bridges, got {}", placed.poles.len());
    assert!(placed.ok && !placed.disconnected && placed.uncovered.is_empty());
    assert_eq!(placed.wires.len(), placed.poles.len() - 1, "one connected tree");
    assert_eq!(power_errors(&entities, &placed), []);
}

#[test]
fn without_room_to_bridge_the_poles_stay_disconnected() {
    let entities = machines();
    // Candidates only beside each machine: nothing in between to bridge with.
    let candidates = [Tile::new(3, 1), Tile::new(W - 4, 1)];
    let placed = place_poles(&entities, &candidates, POLE, &[]);
    assert_eq!(placed.poles.len(), 2);
    assert!(placed.disconnected && !placed.ok);
    assert!(placed.uncovered.is_empty(), "both machines are covered, only the wiring fails");
    assert!(placed.wires.is_empty(), "no wire reaches across the gap");
    assert_eq!(power_errors(&entities, &placed), [ErrorType::PolesDisconnected]);
}
