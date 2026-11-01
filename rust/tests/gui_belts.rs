//! The in-game view's belt pictures, decided from hand-built layouts: which row of the belt sheet each tile takes, where
//! the lips of open ends go, and which way an underground's hood faces. The rows' meaning is pinned to the game's own
//! sheet by `tests/gui_sheets.rs`; this file pins the neighbour rules that choose them.

use factorio_optimizer::geom::{Dir, Kind, Layout, PlacedEntity, Tile};
use factorio_optimizer::gui::belts::{BELT_ROWS, BeltSprites, belt_sprites, side};

fn belt(x: i32, y: i32, d: Dir) -> PlacedEntity {
    PlacedEntity::new("transport-belt", Kind::Belt, x, y, d)
}

fn ug(kind: Kind, x: i32, y: i32, d: Dir) -> PlacedEntity {
    PlacedEntity::new("underground-belt", kind, x, y, d)
}

fn sprites(entities: Vec<PlacedEntity>) -> BeltSprites {
    belt_sprites(&Layout::new(10, 10, entities))
}

fn row_at(s: &BeltSprites, x: i32, y: i32) -> &str {
    let tile = Tile::new(x, y);
    let piece = s.pieces.iter().find(|p| p.tile == tile).unwrap_or_else(|| panic!("no belt piece on {tile}"));
    assert!(BELT_ROWS.contains(&piece.row.as_str()), "{} is not a row of the sheet", piece.row);
    &piece.row
}

fn caps(s: &BeltSprites) -> Vec<(i32, i32, &str)> {
    let mut caps: Vec<(i32, i32, &str)> = s.caps.iter().map(|c| (c.tile.x, c.tile.y, c.row.as_str())).collect();
    caps.sort();
    caps
}

#[test]
fn a_straight_run_has_a_lip_behind_its_first_belt_and_one_past_its_last() {
    let s = sprites(vec![belt(2, 3, Dir::E), belt(3, 3, Dir::E), belt(4, 3, Dir::E)]);
    for x in 2..=4 {
        assert_eq!(row_at(&s, x, 3), "east_index");
    }
    assert_eq!(caps(&s), vec![(1, 3, "starting_west_index"), (5, 3, "ending_east_index")]);
}

#[test]
fn a_belt_fed_from_one_side_only_curves_from_that_side() {
    for d in Dir::ALL {
        for from in [d.cw(), d.ccw()] {
            // The feeder stands on side `from` and travels into the target's tile.
            let feeder = belt(5 + from.dx(), 5 + from.dy(), from.opposite());
            let s = sprites(vec![belt(5, 5, d), feeder]);
            assert_eq!(row_at(&s, 5, 5), format!("{}_to_{}_index", side(from), side(d)), "travelling {d:?}, fed from {from:?}");
            assert!(s.caps.iter().all(|c| c.tile != Tile::new(5, 5) + d.opposite()), "a curve has no lip behind it");
        }
    }
}

#[test]
fn turning_north_from_the_west_and_from_the_east_are_two_pictures() {
    // The bug the first in-game view had: both drew one sprite per direction, so the run broke at every turn.
    let from_west = sprites(vec![belt(4, 5, Dir::E), belt(5, 5, Dir::N)]);
    let from_east = sprites(vec![belt(6, 5, Dir::W), belt(5, 5, Dir::N)]);
    assert_eq!(row_at(&from_west, 5, 5), "west_to_north_index");
    assert_eq!(row_at(&from_east, 5, 5), "east_to_north_index");
    // Neither feeder has a lip ahead of it: each runs into the curve.
    assert_eq!(caps(&from_west), vec![(3, 5, "starting_west_index"), (5, 4, "ending_north_index")]);
    assert_eq!(caps(&from_east), vec![(5, 4, "ending_north_index"), (7, 5, "starting_east_index")]);
}

#[test]
fn a_belt_fed_from_behind_stays_straight_whatever_comes_from_the_side() {
    let s = sprites(vec![belt(5, 6, Dir::N), belt(5, 5, Dir::N), belt(4, 5, Dir::E)]);
    assert_eq!(row_at(&s, 5, 5), "north_index");
    // The side-loading belt runs into the side of the other: no lip ahead of it.
    assert!(!caps(&s).contains(&(5, 5, "ending_east_index")));
}

#[test]
fn a_belt_fed_from_both_sides_and_not_from_behind_is_straight_with_an_open_back() {
    let s = sprites(vec![belt(5, 5, Dir::N), belt(4, 5, Dir::E), belt(6, 5, Dir::W)]);
    assert_eq!(row_at(&s, 5, 5), "north_index");
    assert!(caps(&s).contains(&(5, 6, "starting_south_index")));
}

#[test]
fn belts_meeting_head_on_both_end_with_a_lip() {
    let s = sprites(vec![belt(4, 5, Dir::E), belt(5, 5, Dir::W)]);
    let caps = caps(&s);
    assert!(caps.contains(&(5, 5, "ending_east_index")), "{caps:?}");
    assert!(caps.contains(&(4, 5, "ending_west_index")), "{caps:?}");
}

#[test]
fn a_belt_into_a_machine_ends_with_a_lip_on_the_machine() {
    let machine = PlacedEntity::new("assembling-machine-2", Kind::Machine, 3, 2, Dir::N);
    let s = sprites(vec![belt(4, 6, Dir::N), belt(4, 5, Dir::N), machine]);
    assert!(caps(&s).contains(&(4, 4, "ending_north_index")));
}

#[test]
fn an_underground_hood_stands_ahead_of_its_entrance_and_behind_its_exit() {
    for d in Dir::ALL {
        // Entrance at the origin, exit three tiles further on, a belt into the entrance and nothing after the exit.
        let exit = Tile::new(5, 5).step(d, 3);
        let s = sprites(vec![belt(5 - d.dx(), 5 - d.dy(), d), ug(Kind::UgIn, 5, 5, d), ug(Kind::UgOut, exit.x, exit.y, d)]);
        let entrance = s.hoods.iter().find(|h| h.entity == 1).expect("an entrance hood");
        let out = s.hoods.iter().find(|h| h.entity == 2).expect("an exit hood");
        assert_eq!(entrance.column, d, "travelling {d:?}");
        assert_eq!(out.column, d.opposite(), "travelling {d:?}");
        assert!(!entrance.side_loading && !out.side_loading);
        // Each draws the half of the belt on its open side: behind an entrance, ahead of an exit.
        let half = |tile: Tile| s.pieces.iter().find(|p| p.tile == tile).and_then(|p| p.half);
        assert_eq!(half(Tile::new(5, 5)), Some(d.opposite()));
        assert_eq!(half(exit), Some(d));
        assert_eq!(row_at(&s, 5, 5), format!("{}_index", side(d)));
        // The fed entrance has no lip; the belt feeding it starts with one, and the exit runs into nothing.
        let first = Tile::new(5, 5) + d.opposite() + d.opposite();
        let ahead = exit + d;
        let start = format!("starting_{}_index", side(d.opposite()));
        let end = format!("ending_{}_index", side(d));
        let mut want = vec![(first.x, first.y, start.as_str()), (ahead.x, ahead.y, end.as_str())];
        want.sort();
        assert_eq!(caps(&s), want, "travelling {d:?}");
    }
}

#[test]
fn an_unfed_entrance_gets_a_lip_behind_it_and_an_exit_never_does() {
    let s = sprites(vec![ug(Kind::UgIn, 5, 5, Dir::S), ug(Kind::UgOut, 5, 8, Dir::S), belt(5, 9, Dir::S)]);
    assert_eq!(caps(&s), vec![(5, 4, "starting_north_index"), (5, 10, "ending_south_index")]);
}

#[test]
fn a_belt_into_the_side_of_an_east_west_entrance_uses_the_side_loading_hood() {
    let s = sprites(vec![belt(5, 4, Dir::S), ug(Kind::UgIn, 5, 5, Dir::E), ug(Kind::UgOut, 8, 5, Dir::E)]);
    let entrance = s.hoods.iter().find(|h| h.entity == 1).expect("an entrance hood");
    assert!(entrance.side_loading);
    // Its back is walled in, so no lip behind it; the feeder runs into its side, so none ahead of the feeder either.
    assert!(caps(&s).iter().all(|c| c.0 != 4 && (c.0, c.1) != (5, 5)), "{:?}", caps(&s));
    // North and south hoods have no such variant.
    let s = sprites(vec![belt(4, 5, Dir::E), ug(Kind::UgIn, 5, 5, Dir::N), ug(Kind::UgOut, 5, 2, Dir::N)]);
    assert!(!s.hoods.iter().any(|h| h.side_loading));
}

#[test]
fn a_splitter_draws_two_lanes_with_lips_only_where_a_lane_is_open() {
    // East-facing splitter on (5,4)-(5,5): the top lane is fed and its output taken, the bottom one neither.
    let splitter = PlacedEntity::new("splitter", Kind::Splitter, 5, 4, Dir::E);
    let s = sprites(vec![belt(4, 4, Dir::E), splitter, belt(6, 4, Dir::E)]);
    assert_eq!(row_at(&s, 5, 4), "east_index");
    assert_eq!(row_at(&s, 5, 5), "east_index");
    assert_eq!(caps(&s), vec![(3, 4, "starting_west_index"), (4, 5, "starting_west_index"), (6, 5, "ending_east_index"), (7, 4, "ending_east_index")]);
}

#[test]
fn an_exit_or_a_splitter_lane_into_a_belt_side_makes_it_curve() {
    let s = sprites(vec![ug(Kind::UgIn, 1, 5, Dir::E), ug(Kind::UgOut, 4, 5, Dir::E), belt(5, 5, Dir::S)]);
    assert_eq!(row_at(&s, 5, 5), "west_to_south_index");
    let splitter = PlacedEntity::new("splitter", Kind::Splitter, 4, 5, Dir::N);
    let s = sprites(vec![splitter, belt(5, 4, Dir::W)]);
    assert_eq!(row_at(&s, 5, 4), "south_to_west_index");
}

#[test]
fn an_entrance_feeds_nothing_above_ground() {
    // A belt ahead of an entrance is not fed by it: it starts there, with its own lip.
    let s = sprites(vec![ug(Kind::UgIn, 5, 5, Dir::E), belt(6, 5, Dir::N)]);
    assert_eq!(row_at(&s, 6, 5), "north_index");
    assert!(caps(&s).contains(&(6, 6, "starting_south_index")));
}
