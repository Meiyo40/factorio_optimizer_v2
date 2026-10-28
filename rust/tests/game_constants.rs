use factorio_optimizer::data::game_constants::*;
use factorio_optimizer::geom::{Dir, Kind, PlacedEntity, Tile};

#[test]
fn blueprint_version_is_2_0_10_1() {
    assert_eq!(BLUEPRINT_VERSION, 562_949_954_076_673);
}

#[test]
fn a_straight_belt_takes_the_far_lane() {
    // Belt heading north: an inserter on its west side swings east and lands on the right lane.
    assert_eq!(drop_lane(Dir::E, Dir::N, Dir::N), 1);
    assert_eq!(drop_lane(Dir::W, Dir::N, Dir::N), 0);
    // In line with the belt, either way.
    assert_eq!(drop_lane(Dir::N, Dir::N, Dir::N), PARALLEL_DROP_LANE);
    assert_eq!(drop_lane(Dir::S, Dir::N, Dir::N), PARALLEL_DROP_LANE);
}

#[test]
fn a_right_turn_puts_the_inside_lane_on_the_right() {
    // Items enter heading north and leave heading east. From the side straight ahead of the entry (swinging
    // south) or from behind the exit (swinging east) the far side is the inside of the bend.
    let (into, out) = (Dir::N, Dir::E);
    assert_eq!(drop_lane(Dir::S, into, out), 1);
    assert_eq!(drop_lane(Dir::E, into, out), 1);
    assert_eq!(drop_lane(Dir::N, into, out), 0);
    assert_eq!(drop_lane(Dir::W, into, out), 0);
}

#[test]
fn a_left_turn_puts_the_inside_lane_on_the_left() {
    let (into, out) = (Dir::N, Dir::W);
    assert_eq!(drop_lane(Dir::S, into, out), 0);
    assert_eq!(drop_lane(Dir::W, into, out), 0);
    assert_eq!(drop_lane(Dir::N, into, out), 1);
    assert_eq!(drop_lane(Dir::E, into, out), 1);
}

#[test]
fn verify_kit_v2_row_3_expects_the_inside_lane_of_a_left_turn() {
    // docs/verify/v2: an inserter drops onto a left turn from behind it, expected on the inside (west) lane.
    // Items enter heading east and leave heading north, so behind the exit means swinging north.
    assert_eq!(drop_lane(Dir::N, Dir::E, Dir::N), 0, "lane 0 is the left (west) lane of a belt heading north");
}

#[test]
fn belt_tiers_know_their_entities() {
    assert_eq!(belt_tier("fast").unwrap().belt, "fast-transport-belt");
    assert!(belt_tier("ludicrous").is_err());
    assert_eq!(tier_of_entity("express-splitter").unwrap().key, "express");
    assert_eq!(tier_of_entity("turbo-underground-belt").unwrap().max_underground_gap, 10);
    assert!(tier_of_entity("pipe").is_none());
}

#[test]
fn inserter_hand_sizes_clamp_to_the_research_levels() {
    assert_eq!(hand_sizes(0), (1, 2));
    assert_eq!(hand_sizes(7), (3, 12));
    assert_eq!(hand_sizes(-4), (1, 2));
    assert_eq!(hand_sizes(99), (3, 12));
}

#[test]
fn entity_kinds_follow_the_name() {
    let kind = |name| PlacedEntity::kind_of(name, None).unwrap();
    assert_eq!(kind("fast-transport-belt"), Kind::Belt);
    assert_eq!(kind("fast-underground-belt"), Kind::UgIn);
    assert_eq!(PlacedEntity::kind_of("fast-underground-belt", Some("output")).unwrap(), Kind::UgOut);
    assert_eq!(kind("express-splitter"), Kind::Splitter);
    assert_eq!(kind("long-handed-inserter"), Kind::Inserter);
    assert_eq!(kind("steel-chest"), Kind::Chest);
    assert_eq!(kind("infinity-chest"), Kind::InfinityChest);
    assert_eq!(kind("medium-electric-pole"), Kind::Pole);
    assert_eq!(kind("pipe"), Kind::Pipe);
    assert_eq!(kind("pipe-to-ground"), Kind::PipeUg);
    assert_eq!(kind("infinity-pipe"), Kind::InfinityPipe);
    assert_eq!(kind("foundry"), Kind::Machine);
    assert!(PlacedEntity::kind_of("rocket-silo", None).is_err());
}

#[test]
fn entity_sizes() {
    assert_eq!(PlacedEntity::default_size("oil-refinery", Kind::Machine, Dir::N), (5, 5));
    assert_eq!(PlacedEntity::default_size("crusher", Kind::Machine, Dir::N), (2, 3));
    assert_eq!(PlacedEntity::default_size("big-electric-pole", Kind::Pole, Dir::N), (2, 2));
    assert_eq!(PlacedEntity::default_size("fast-splitter", Kind::Splitter, Dir::N), (2, 1));
    assert_eq!(PlacedEntity::default_size("fast-splitter", Kind::Splitter, Dir::E), (1, 2));
    assert_eq!(PlacedEntity::default_size("pipe", Kind::Pipe, Dir::N), (1, 1));
}

#[test]
fn an_inserter_picks_and_drops_along_its_direction() {
    // From the Kotlin BlueprintTest: the direction points at the pickup side.
    let ins = PlacedEntity::new("long-handed-inserter", Kind::Inserter, 5, 5, Dir::N);
    assert_eq!(ins.pickup_tile(), Tile::new(5, 3));
    assert_eq!(ins.drop_tile(), Tile::new(5, 7));
}

#[test]
fn belts_push_ahead_and_underground_entrances_do_not() {
    let belt = PlacedEntity::new("transport-belt", Kind::Belt, 1, 1, Dir::E);
    assert_eq!(belt.output_tile(), Some(Tile::new(2, 1)));
    assert_eq!(PlacedEntity::new("underground-belt", Kind::UgIn, 1, 1, Dir::E).output_tile(), None);
    assert_eq!(PlacedEntity::new("underground-belt", Kind::UgOut, 1, 1, Dir::S).output_tile(), Some(Tile::new(1, 2)));
}

#[test]
fn only_machines_that_choose_a_recipe_carry_one() {
    assert!(takes_recipe("assembling-machine-3"));
    assert!(takes_recipe("chemical-plant"));
    assert!(!takes_recipe("electric-furnace"));
    assert!(!takes_recipe("stone-furnace"));
    assert!(!takes_recipe("recycler"));
}

#[test]
fn fluid_connections_per_machine() {
    assert_eq!(base_fluid_connections("chemical-plant").len(), 4);
    assert_eq!(base_fluid_connections("oil-refinery").len(), 5);
    assert_eq!(base_fluid_connections("cryogenic-plant").len(), 6);
    assert!(base_fluid_connections("electric-furnace").is_empty());
    assert!(connections_only_with_fluid_recipe("assembling-machine-3"));
    assert!(!connections_only_with_fluid_recipe("chemical-plant"));
}

#[test]
fn a_connection_follows_the_machine_it_is_rotated_with() {
    // Chemical plant, first input: the tile above its top-left corner when facing north.
    let c = base_fluid_connections("chemical-plant")[0];
    assert_eq!(place_connection(&c, 3, 3, 0, false), (0, -1));
    assert_eq!(place_connection(&c, 3, 3, 1, false), (3, 0), "a quarter turn clockwise: right of the top row");
    assert_eq!(place_connection(&c, 3, 3, 2, false), (2, 3), "half a turn: below the bottom-right tile");
    assert_eq!(place_connection(&c, 3, 3, 3, false), (-1, 2));
    assert_eq!(place_connection(&c, 3, 3, 4, false), (0, -1), "four turns are none");
    assert_eq!(place_connection(&c, 3, 3, -1, false), (-1, 2), "a negative rotation counts as its remainder");
    assert_eq!(place_connection(&c, 3, 3, 0, true), (2, -1), "mirrored: the top-right corner");
}

#[test]
fn an_even_sized_machine_rotates_its_connections_exactly() {
    // Electromagnetic plant (4x4): input 1 sits west of the machine; a quarter turn puts it north.
    let c = base_fluid_connections("electromagnetic-plant")[0];
    assert_eq!(place_connection(&c, 4, 4, 0, false), (-1, 2));
    assert_eq!(place_connection(&c, 4, 4, 1, false), (1, -1));
    assert_eq!(place_side(&c, 1, false), Dir::N);
}

#[test]
fn a_connection_side_turns_and_mirrors() {
    let north = base_fluid_connections("chemical-plant")[0];
    assert_eq!(north.side, Dir::N);
    assert_eq!(place_side(&north, 1, false), Dir::E);
    assert_eq!(place_side(&north, 2, false), Dir::S);
    assert_eq!(place_side(&north, 0, true), Dir::N, "mirroring swaps east and west only");
    let west = base_fluid_connections("electromagnetic-plant")[0];
    assert_eq!(place_side(&west, 0, true), Dir::E);
}

#[test]
fn supported_machines_are_base_then_space_age() {
    let all: Vec<_> = supported_machines().collect();
    assert_eq!(all.len(), 15);
    assert_eq!(all[0], "assembling-machine-1");
    assert_eq!(all[14], "recycler");
    assert!(all.iter().all(|m| machine_size(m).is_some()), "every supported machine has a footprint");
}
