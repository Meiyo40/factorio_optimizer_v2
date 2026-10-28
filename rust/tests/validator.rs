//! Ports of the Kotlin ValidatorTest: hand-built layouts with a known verdict. The same layouts are validated by
//! Kotlin and Rust in validate_golden.rs; these check the verdicts a person can predict.

use factorio_optimizer::data::data_model::RootData;
use factorio_optimizer::data::game_constants as gc;
use factorio_optimizer::data::loader::load_root_data;
use factorio_optimizer::geom::{Dir, Kind, Layout, PlacedEntity, Tile};
use factorio_optimizer::validate::{ErrorType, MachineSpec, SourceSpec, ValidationError, ValidationSpec, recipe_info_from, validate};
use std::sync::LazyLock;

static DATA: LazyLock<RootData> = LazyLock::new(|| load_root_data("recipes.json").unwrap());

fn e(name: &str, x: i32, y: i32, dir: Dir) -> PlacedEntity {
    PlacedEntity::new(name, PlacedEntity::kind_of(name, None).unwrap(), x, y, dir)
}

fn at(name: &str, x: i32, y: i32) -> PlacedEntity {
    e(name, x, y, Dir::N)
}

fn recipe(mut ent: PlacedEntity, r: &str) -> PlacedEntity {
    ent.recipe = Some(r.to_owned());
    ent
}

fn item(mut ent: PlacedEntity, i: &str) -> PlacedEntity {
    ent.item = Some(i.to_owned());
    ent
}

fn fluid(mut ent: PlacedEntity, f: &str) -> PlacedEntity {
    ent.fluid = Some(f.to_owned());
    ent
}

fn net(mut ent: PlacedEntity, n: i32) -> PlacedEntity {
    ent.net = Some(n);
    ent
}

fn belt(x: i32, y: i32, d: Dir, n: i32) -> PlacedEntity {
    net(e("fast-transport-belt", x, y, d), n)
}

fn layout(w: i32, h: i32, entities: Vec<PlacedEntity>) -> Layout {
    Layout::new(w, h, entities)
}

fn base() -> ValidationSpec {
    ValidationSpec { recipe_info: recipe_info_from(&DATA), require_power: false, check_rates: false, ..Default::default() }
}

fn source(x: i32, y: i32, item: &str, rate: f64, lane: i32) -> SourceSpec {
    SourceSpec { tile: Tile::new(x, y), item: item.to_owned(), rate, fluid: false, lane, port: -1 }
}

/// MachineSpec(product, ingredients, outputRate, sinkRate, demand)
fn machine(product: &str, ingredients: &[&str], output_rate: f64, sink_rate: f64, demand: &[(&str, f64)]) -> MachineSpec {
    MachineSpec {
        product: product.to_owned(),
        ingredients: ingredients.iter().map(|s| (*s).to_owned()).collect(),
        output_rate,
        sink_rate,
        demand: demand.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect(),
        ..Default::default()
    }
}

fn assert_has(errors: &[ValidationError], kind: ErrorType, tile: Tile) {
    assert!(errors.iter().any(|x| x.kind == kind && x.tile == tile), "expected {kind} at {tile}, got {}", dump(errors));
}

fn dump(errors: &[ValidationError]) -> String {
    errors.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ")
}

/// The hand-written T1 layout of the Kotlin tests (VerifyKit.t1Hand).
fn t1_hand() -> Layout {
    let ug = |x, kind| PlacedEntity::new("fast-underground-belt", PlacedEntity::kind_of("fast-underground-belt", Some(kind)).unwrap(), x, 5, Dir::E);
    let mut l = layout(
        8,
        6,
        vec![
            recipe(at("assembling-machine-3", 3, 1), "iron-gear-wheel"),
            e("fast-transport-belt", 0, 5, Dir::E),
            ug(1, "input"),
            ug(3, "output"),
            e("fast-transport-belt", 4, 5, Dir::E),
            e("fast-transport-belt", 5, 5, Dir::E), // dead end
            e("fast-inserter", 4, 4, Dir::S),       // picks from the belt at (4,5), drops into the machine
            e("fast-inserter", 5, 4, Dir::S),
            e("fast-inserter", 6, 2, Dir::W), // picks from the machine at (5,2), drops into the chest
            at("steel-chest", 7, 2),
            at("medium-electric-pole", 6, 4),
        ],
    );
    l.label = "T1 hand layout".to_owned();
    l
}

#[test]
fn the_hand_written_t1_layout_passes_with_rates_and_power() {
    let mut spec = base();
    spec.require_power = true;
    spec.check_rates = true;
    spec.machines.insert(Tile::new(3, 1), machine("iron-gear-wheel", &["iron-plate"], 1.0, 1.0, &[("iron-plate", 2.0)]));
    spec.sources = vec![source(0, 5, "iron-plate", 2.0, -1)];
    let errors = validate(&t1_hand(), &spec);
    assert!(errors.is_empty(), "{}", dump(&errors));
}

#[test]
fn a_dead_end_into_another_nets_belt_contaminates_it() {
    let l = layout(
        8,
        8,
        vec![
            belt(3, 0, Dir::S, 1),
            belt(3, 1, Dir::S, 1),
            belt(3, 2, Dir::S, 1),
            belt(3, 3, Dir::S, 1),
            belt(0, 1, Dir::E, 2),
            belt(1, 1, Dir::E, 2),
            belt(2, 1, Dir::E, 2), // dead end points into (3,1)
        ],
    );
    let mut spec = base();
    spec.sources = vec![source(3, 0, "copper-plate", 1.0, -1), source(0, 1, "iron-plate", 1.0, -1)];
    assert_has(&validate(&l, &spec), ErrorType::Contamination, Tile::new(3, 1));
}

#[test]
fn a_dead_end_that_turns_a_straight_belt_into_a_curve_is_caught() {
    let l = layout(
        8,
        8,
        vec![
            belt(3, 1, Dir::S, 1),
            belt(3, 2, Dir::S, 1),
            belt(3, 3, Dir::S, 1), // starts at (3,1), no feeder
            belt(1, 1, Dir::E, 2),
            belt(2, 1, Dir::E, 2), // dead end into (3,1) from the side
        ],
    );
    let mut spec = base();
    spec.sources = vec![source(3, 1, "copper-plate", 1.0, -1), source(1, 1, "iron-plate", 1.0, -1)];
    let errors = validate(&l, &spec);
    assert_has(&errors, ErrorType::ShapeChanged, Tile::new(3, 1));
    assert_has(&errors, ErrorType::Contamination, Tile::new(3, 1));
}

#[test]
fn two_items_share_a_belt_on_separate_lanes_but_not_on_one_lane() {
    // Iron on the left lane, cable on the right, past a circuit assembler that takes both.
    let l = layout(
        8,
        6,
        vec![
            recipe(at("assembling-machine-3", 2, 0), "electronic-circuit"),
            e("fast-inserter", 3, 3, Dir::S),
            belt(0, 4, Dir::E, 1),
            belt(1, 4, Dir::E, 1),
            belt(2, 4, Dir::E, 1),
            belt(3, 4, Dir::E, 1),
            belt(4, 4, Dir::E, 1),
        ],
    );
    let mut shared = base();
    shared.sources = vec![source(0, 4, "iron-plate", 1.0, 0), source(0, 4, "copper-cable", 3.0, 1)];
    assert!(validate(&l, &shared).is_empty());
    // Both on the left lane: contamination.
    let mut one_lane = base();
    one_lane.sources = vec![source(0, 4, "iron-plate", 1.0, 0), source(0, 4, "copper-cable", 3.0, 0)];
    assert_has(&validate(&l, &one_lane), ErrorType::Contamination, Tile::new(0, 4));
    // An inserter dropping cable onto the iron lane is caught; onto the cable lane it is fine.
    let mut iron_only = base();
    iron_only.sources = vec![source(0, 4, "iron-plate", 1.0, 0)];
    let with_drop = |from_north_side: bool| {
        let mut l = l.clone();
        let y = |north, south| if from_north_side { north } else { south };
        l.entities.push(item(at("infinity-chest", 1, y(2, 6)), "copper-cable"));
        l.entities.push(e("fast-inserter", 1, y(3, 5), if from_north_side { Dir::N } else { Dir::S }));
        l.h = 7;
        l
    };
    // From the north side (the belt's left) an inserter drops on the far, right lane: no clash with iron.
    assert!(validate(&with_drop(true), &iron_only).iter().all(|x| !matches!(x.kind, ErrorType::InserterWrongItem | ErrorType::Contamination)));
    assert_has(&validate(&with_drop(false), &iron_only), ErrorType::InserterWrongItem, Tile::new(1, 5));
}

#[test]
fn an_entrance_pairs_with_the_nearest_same_tier_underground_on_its_axis() {
    let ug = |x, kind: &str, n| {
        net(PlacedEntity::new("fast-underground-belt", PlacedEntity::kind_of("fast-underground-belt", Some(kind)).unwrap(), x, 4, Dir::E), n)
    };
    let l = layout(10, 8, vec![belt(0, 4, Dir::E, 1), ug(1, "input", 1), ug(6, "output", 1), belt(7, 4, Dir::E, 1), ug(2, "input", 2), ug(4, "output", 2)]);
    let errors = validate(&l, &base());
    assert_has(&errors, ErrorType::UgUnpaired, Tile::new(1, 4));
    assert_has(&errors, ErrorType::UgUnpaired, Tile::new(6, 4));
    assert!(errors.iter().all(|x| x.tile != Tile::new(2, 4)));
    // Different tiers ignore each other: making net 2 basic tier fixes net 1.
    let mut woven = l.clone();
    for ent in woven.entities.iter_mut().filter(|x| x.net == Some(2)) {
        ent.name = "underground-belt".to_owned();
    }
    let errors = validate(&woven, &base());
    assert!(errors.iter().all(|x| x.kind != ErrorType::UgUnpaired), "{}", dump(&errors));
}

#[test]
fn an_inserter_dropping_onto_another_nets_belt_is_caught() {
    let l = layout(
        10,
        8,
        vec![
            recipe(at("assembling-machine-3", 2, 0), "iron-gear-wheel"),
            belt(0, 4, Dir::E, 1),
            belt(1, 4, Dir::E, 1),
            belt(2, 4, Dir::E, 1),
            belt(3, 4, Dir::E, 1),
            belt(4, 4, Dir::E, 1),
            e("inserter", 3, 3, Dir::N), // picks gears from the machine, drops onto the copper belt at (3,4)
        ],
    );
    let mut spec = base();
    spec.sources = vec![source(0, 4, "copper-plate", 1.0, -1)];
    let errors = validate(&l, &spec);
    assert_has(&errors, ErrorType::InserterWrongItem, Tile::new(3, 3));
    assert_has(&errors, ErrorType::Contamination, Tile::new(3, 4));
}

#[test]
fn an_inserter_outside_every_supply_area_is_unpowered() {
    let mut l = t1_hand();
    for ent in l.entities.iter_mut().filter(|x| x.name == "medium-electric-pole") {
        ent.x = 0;
        ent.y = 2;
    }
    let mut spec = base();
    spec.require_power = true;
    let errors = validate(&l, &spec);
    // Pole at (0,2): supply x in (-3, 4), so the inserters at x = 4, 5 and 6 are outside.
    assert_has(&errors, ErrorType::Unpowered, Tile::new(6, 2));
    assert_has(&errors, ErrorType::Unpowered, Tile::new(5, 4));
    assert!(errors.iter().all(|x| !(x.kind == ErrorType::Unpowered && x.tile == Tile::new(3, 1))), "the machine overlaps the area");
}

#[test]
fn removing_the_only_pole_fails_the_power_check() {
    let mut l = t1_hand();
    l.entities.retain(|x| x.name != "medium-electric-pole");
    let mut spec = base();
    spec.require_power = true;
    assert!(validate(&l, &spec).iter().any(|x| x.kind == ErrorType::Unpowered));
}

#[test]
fn touching_pipes_of_two_fluids_are_caught() {
    let l = layout(
        10,
        8,
        vec![
            recipe(at("assembling-machine-3", 0, 1), "processing-unit"),
            recipe(at("assembling-machine-3", 4, 1), "processing-unit"),
            fluid(at("infinity-pipe", 1, 0), "sulfuric-acid"),
            fluid(at("infinity-pipe", 2, 0), "lubricant"), // joins the acid pipe next to it
            fluid(at("infinity-pipe", 5, 0), "sulfuric-acid"),
        ],
    );
    let errors = validate(&l, &base());
    assert_has(&errors, ErrorType::FluidMix, Tile::new(1, 0));
    assert!(errors.iter().all(|x| !(x.kind == ErrorType::FluidMissing && x.tile == Tile::new(5, 0))), "{}", dump(&errors));
}

#[test]
fn pipes_carry_a_fluid_from_a_refinery_to_a_chemical_plant() {
    // Refinery facing north at (0,2): basic oil takes crude on input 2 at (3,7), petroleum leaves output 3 at (4,1).
    // Chemical plant facing north at (10,2) makes plastic from petroleum on input 1 at (10,1).
    let with = |line: Vec<PlacedEntity>| {
        let mut entities = vec![
            recipe(at("oil-refinery", 0, 2), "basic-oil-processing"),
            fluid(at("infinity-pipe", 3, 7), "crude-oil"),
            recipe(at("chemical-plant", 10, 2), "plastic-bar"),
        ];
        entities.extend(line);
        layout(14, 10, entities)
    };
    let no_fluid_errors = |l: &Layout| validate(l, &base()).iter().all(|x| !x.kind.name().starts_with("FLUID"));
    let pipes = with((4..=10).map(|x| at("pipe", x, 1)).collect());
    assert!(no_fluid_errors(&pipes), "{}", dump(&validate(&pipes, &base())));

    let underground = with(vec![at("pipe", 4, 1), e("pipe-to-ground", 5, 1, Dir::W), e("pipe-to-ground", 9, 1, Dir::E), at("pipe", 10, 1)]);
    assert!(no_fluid_errors(&underground), "{}", dump(&validate(&underground, &base())));

    // Another pipe-to-ground facing the wrong way inside the span breaks the pair.
    let mut broken = underground.clone();
    broken.entities.push(e("pipe-to-ground", 7, 1, Dir::W));
    let be = validate(&broken, &base());
    assert!(be.iter().any(|x| x.kind == ErrorType::FluidMissing) && be.iter().any(|x| x.kind == ErrorType::FluidStuck), "{}", dump(&be));

    // A lubricant pipe touching the petroleum line mixes them.
    let mut mixed = pipes.clone();
    mixed.entities.push(fluid(at("infinity-pipe", 6, 0), "lubricant"));
    assert!(validate(&mixed, &base()).iter().any(|x| x.kind == ErrorType::FluidMix));

    // Without the plant, the refinery's petroleum has nowhere to go.
    let mut stuck = pipes.clone();
    stuck.entities.retain(|x| x.name != "chemical-plant");
    assert_has(&validate(&stuck, &base()), ErrorType::FluidStuck, Tile::new(4, 1));

    // A pipe on the refinery's unused heavy-oil output leaks.
    let mut leak = pipes.clone();
    leak.entities.push(at("pipe", 0, 1));
    assert_has(&validate(&leak, &base()), ErrorType::FluidLeak, Tile::new(0, 1));
}

#[test]
fn a_splitter_branch_feeds_consumers_on_both_sides() {
    let gear = machine("iron-gear-wheel", &["iron-plate"], 0.0, 0.0, &[("iron-plate", 1.5)]);
    let mut entities = vec![belt(0, 4, Dir::E, 1), belt(1, 4, Dir::E, 1), net(e("fast-splitter", 2, 4, Dir::E), 1)];
    entities.extend((3..=7).map(|x| belt(x, 4, Dir::E, 1)));
    entities.extend((3..=7).map(|x| belt(x, 5, Dir::E, 1)));
    entities.extend([
        recipe(at("assembling-machine-3", 5, 0), "iron-gear-wheel"),
        e("fast-inserter", 6, 3, Dir::S),
        recipe(at("assembling-machine-3", 5, 7), "iron-gear-wheel"),
        e("fast-inserter", 6, 6, Dir::N),
    ]);
    let l = layout(10, 10, entities);
    let mut spec = base();
    spec.check_rates = true;
    spec.sources = vec![source(0, 4, "iron-plate", 3.0, -1)];
    spec.machines = [(Tile::new(5, 0), gear.clone()), (Tile::new(5, 7), gear)].into();
    let errors = validate(&l, &spec);
    assert!(errors.is_empty(), "{}", dump(&errors));
    // Without the splitter the lower belt gets nothing.
    let mut no_split = l.clone();
    for ent in no_split.entities.iter_mut().filter(|x| x.kind == Kind::Splitter) {
        *ent = belt(2, 4, Dir::E, 1);
    }
    assert!(validate(&no_split, &spec).iter().any(|x| matches!(x.kind, ErrorType::Unreachable | ErrorType::MissingInput)));
}

#[test]
fn one_lane_overflows_when_every_producer_drops_from_the_same_side() {
    let basic = |x| net(e("transport-belt", x, 2, Dir::E), 1);
    let belts: Vec<PlacedEntity> = (0..=6).map(basic).collect();
    let from_north = |x| vec![item(at("infinity-chest", x, 0), "iron-plate"), e("fast-inserter", x, 1, Dir::N)];
    let from_south = |x| vec![item(at("infinity-chest", x, 4), "iron-plate"), e("fast-inserter", x, 3, Dir::S)];
    let mut spec = base();
    spec.check_rates = true;
    let one_side = layout(8, 5, belts.iter().cloned().chain((1..=4).flat_map(from_north)).collect());
    let errors = validate(&one_side, &spec);
    assert!(errors.iter().any(|x| x.kind == ErrorType::BeltOverCapacity), "{}", dump(&errors));
    let both: Vec<PlacedEntity> = belts.into_iter().chain(from_north(1)).chain(from_north(2)).chain(from_south(3)).chain(from_south(4)).collect();
    let errors = validate(&layout(8, 5, both), &spec);
    assert!(errors.iter().all(|x| x.kind != ErrorType::BeltOverCapacity), "{}", dump(&errors));
}

#[test]
fn a_missing_infinity_pipe_and_overlaps_are_reported() {
    let l = layout(10, 8, vec![recipe(e("assembling-machine-3", 0, 1, Dir::E), "processing-unit"), at("steel-chest", 2, 2)]);
    let errors = validate(&l, &base());
    assert_has(&errors, ErrorType::FluidMissing, Tile::new(3, 2));
    assert_has(&errors, ErrorType::Overlap, Tile::new(2, 2));
}

#[test]
fn a_consumer_upstream_of_every_producer_is_unreachable() {
    let mut entities = vec![recipe(at("assembling-machine-3", 1, 5), "automation-science-pack"), recipe(at("assembling-machine-3", 6, 5), "iron-gear-wheel")];
    entities.extend((0..=7).map(|x| belt(x, 3, Dir::E, 1)));
    entities.extend([
        e("fast-inserter", 2, 4, Dir::N), // belt -> science, before any gear is on the belt
        e("fast-inserter", 7, 4, Dir::S), // gears -> belt
    ]);
    let mut spec = base();
    spec.check_rates = true;
    spec.machines = [
        (Tile::new(1, 5), machine("automation-science-pack", &["copper-plate", "iron-gear-wheel"], 0.0, 0.0, &[("iron-gear-wheel", 0.5)])),
        (Tile::new(6, 5), machine("iron-gear-wheel", &["iron-plate"], 0.5, 0.0, &[])),
    ]
    .into();
    let errors = validate(&layout(12, 8, entities), &spec);
    assert_has(&errors, ErrorType::Unreachable, Tile::new(2, 3));
    assert_has(&errors, ErrorType::MissingInput, Tile::new(1, 5));
}

#[test]
fn a_consumer_after_the_supply_runs_out_is_starved() {
    let mut entities = vec![
        recipe(at("assembling-machine-3", 1, 5), "iron-gear-wheel"),
        recipe(at("assembling-machine-3", 5, 5), "automation-science-pack"),
        recipe(at("assembling-machine-3", 9, 5), "automation-science-pack"),
        e("fast-inserter", 2, 4, Dir::S),  // gears -> belt
        e("fast-inserter", 6, 4, Dir::N),  // belt -> science A
        e("fast-inserter", 10, 4, Dir::N), // belt -> science B
    ];
    entities.extend((0..=11).map(|x| belt(x, 3, Dir::E, 1)));
    let science = machine("automation-science-pack", &["copper-plate", "iron-gear-wheel"], 0.0, 0.0, &[("iron-gear-wheel", 0.5)]);
    let mut spec = base();
    spec.check_rates = true;
    spec.machines = [
        (Tile::new(1, 5), machine("iron-gear-wheel", &["iron-plate"], 0.5, 0.0, &[])),
        (Tile::new(5, 5), science.clone()),
        (Tile::new(9, 5), science),
    ]
    .into();
    let errors = validate(&layout(13, 8, entities), &spec);
    assert_has(&errors, ErrorType::Starved, Tile::new(10, 3));
    assert!(errors.iter().all(|x| !(x.kind == ErrorType::Starved && x.tile == Tile::new(6, 3))), "{}", dump(&errors));
}

#[test]
fn an_inserter_drops_onto_the_far_side_of_a_curve() {
    // Straight belts: far lane, or the right lane in line with the belt.
    assert_eq!(gc::drop_lane(Dir::E, Dir::N, Dir::N), 1);
    assert_eq!(gc::drop_lane(Dir::W, Dir::N, Dir::N), 0);
    assert_eq!(gc::drop_lane(Dir::N, Dir::N, Dir::N), gc::PARALLEL_DROP_LANE);
    // Left turn (east, then north): from outside the bend (south or east of it) onto the inside lane, the left.
    assert_eq!(gc::drop_lane(Dir::N, Dir::E, Dir::N), 0);
    assert_eq!(gc::drop_lane(Dir::W, Dir::E, Dir::N), 0);
    // From inside the bend (long-handed, across the belt before or after it) onto the outside lane.
    assert_eq!(gc::drop_lane(Dir::E, Dir::E, Dir::N), 1);
    assert_eq!(gc::drop_lane(Dir::S, Dir::E, Dir::N), 1);
    // Right turn (west, then north): the inside lane is the right one.
    assert_eq!(gc::drop_lane(Dir::N, Dir::W, Dir::N), 1);
    assert_eq!(gc::drop_lane(Dir::E, Dir::W, Dir::N), 1);
}

#[test]
fn cable_dropped_onto_a_left_turn_from_behind_lands_on_the_inside_lane() {
    // Iron on one lane runs east and turns north at (2,3); a cable assembler below drops cable onto the curve
    // from the south, onto the inside (left) lane.
    let l = || {
        layout(
            6,
            8,
            vec![
                belt(0, 3, Dir::E, 1),
                belt(1, 3, Dir::E, 1),
                belt(2, 3, Dir::N, 1),
                belt(2, 2, Dir::N, 1),
                belt(2, 1, Dir::N, 1),
                e("fast-inserter", 2, 4, Dir::S),
                recipe(at("assembling-machine-3", 1, 5), "copper-cable"),
            ],
        )
    };
    let with_lane = |lane| {
        let mut spec = base();
        spec.sources = vec![source(0, 3, "iron-plate", 1.0, lane)];
        validate(&l(), &spec)
    };
    assert_has(&with_lane(0), ErrorType::InserterWrongItem, Tile::new(2, 4));
    let right = with_lane(1);
    assert!(right.iter().all(|x| x.kind != ErrorType::InserterWrongItem), "{}", dump(&right));
}

#[test]
fn errors_print_like_kotlin() {
    let err = ValidationError { kind: ErrorType::UgUnpaired, tile: Tile::new(1, 4), message: "no exit".to_owned() };
    assert_eq!(err.to_string(), "UG_UNPAIRED at (1,4): no exit");
}
