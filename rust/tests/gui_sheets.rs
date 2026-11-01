//! The game's belt and underground sheets read the way `gui::belts` says they do, measured on the pictures themselves:
//! the first in-game view took the belt rows for sixteen rotations and no test said otherwise. These need the game
//! (`FACTORIO_DIR`), so they are ignored by default: `cargo test --features gui --test gui_sheets -- --ignored`.
//!
//! A belt frame is a 128 px canvas, two tiles at 64 px a tile, centred on the belt's tile, which spans 32..96. The
//! texture of every frame moves two pixels a frame the way items travel, which is what tells a row's direction.
#![cfg(feature = "gui")]

use factorio_optimizer::geom::Dir;
use factorio_optimizer::gui::belts::{BELT_ROWS, side};
use factorio_optimizer::gui::sprites::{asset, install_dir};
use image::RgbaImage;

const CELL: u32 = 128;
const T0: u32 = 32;
const T1: u32 = 96;
const FRAMES: u32 = 16;

fn sheet(file: &str) -> RgbaImage {
    let path = asset(&install_dir(), file);
    image::open(&path).unwrap_or_else(|e| panic!("{}: {e} (set FACTORIO_DIR)", path.display())).to_rgba8()
}

fn belt_sheet() -> RgbaImage {
    sheet("__base__/graphics/entity/transport-belt/transport-belt.png")
}

fn row(name: &str) -> u32 {
    BELT_ROWS.iter().position(|row| *row == name).unwrap_or_else(|| panic!("{name} is not a row")) as u32
}

/// The yellow chevrons the basic belt carries.
fn chevron(s: &RgbaImage, row: u32, frame: u32, x: u32, y: u32) -> bool {
    let p = s.get_pixel(frame * CELL + x, row * CELL + y);
    p[3] > 128 && p[0] > 150 && p[1] > 90 && p[2] < 100
}

/// How far the chevrons inside `[x0, y0, x1, y1)` move a frame: the shift that best carries each frame onto the next,
/// summed over the whole cycle.
fn motion(s: &RgbaImage, row: u32, [x0, y0, x1, y1]: [u32; 4]) -> (i32, i32) {
    let mut best = ((0, 0), 0);
    for dy in -3i32..=3 {
        for dx in -3i32..=3 {
            let mut score = 0;
            for frame in 0..FRAMES - 1 {
                for y in y0..y1 {
                    for x in x0..x1 {
                        let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                        if chevron(s, row, frame, x, y) && (0..CELL as i32).contains(&nx) && (0..CELL as i32).contains(&ny) && chevron(s, row, frame + 1, nx as u32, ny as u32) {
                            score += 1;
                        }
                    }
                }
            }
            if score > best.1 {
                best = ((dx, dy), score);
            }
        }
    }
    assert!(best.0 != (0, 0), "row {row}: the chevrons in {x0},{y0}..{x1},{y1} do not move");
    best.0
}

/// The way a motion mostly goes. Only for a straight run: along a bend the chevrons also move sideways.
fn heading((dx, dy): (i32, i32)) -> Dir {
    if dx.abs() > dy.abs() { if dx > 0 { Dir::E } else { Dir::W } } else if dy > 0 { Dir::S } else { Dir::N }
}

/// How much of a motion goes toward `d`.
fn along((dx, dy): (i32, i32), d: Dir) -> i32 {
    dx * d.dx() + dy * d.dy()
}

/// The strip of the belt's tile along side `d`.
fn strip(d: Dir) -> [u32; 4] {
    match d {
        Dir::N => [T0, T0, T1, T0 + 18],
        Dir::S => [T0, T1 - 18, T1, T1],
        Dir::W => [T0, T0, T0 + 18, T1],
        Dir::E => [T1 - 18, T0, T1, T1],
    }
}

/// How much of the 8 px square just inside the tile's corner between sides `a` and `b` has ink.
fn corner_ink(s: &RgbaImage, row: u32, a: Dir, b: Dir) -> f32 {
    let (vertical, horizontal) = if a.axis() == 1 { (a, b) } else { (b, a) };
    let x0 = if horizontal == Dir::W { T0 + 3 } else { T1 - 11 };
    let y0 = if vertical == Dir::N { T0 + 3 } else { T1 - 11 };
    let mut inked = 0;
    for y in y0..y0 + 8 {
        for x in x0..x0 + 8 {
            inked += u32::from(s.get_pixel(x, row * CELL + y)[3] > 64);
        }
    }
    inked as f32 / 64.0
}

#[test]
#[ignore = "reads the game's sheets from FACTORIO_DIR"]
fn a_straight_row_moves_the_way_it_is_named() {
    let s = belt_sheet();
    for d in Dir::ALL {
        let name = format!("{}_index", side(d));
        assert_eq!(heading(motion(&s, row(&name), [T0, T0, T1, T1])), d, "{name}");
    }
}

#[test]
#[ignore = "reads the game's sheets from FACTORIO_DIR"]
fn a_corner_row_bends_from_its_first_side_out_through_its_second() {
    let s = belt_sheet();
    for d in Dir::ALL {
        for from in [d.cw(), d.ccw()] {
            let name = format!("{}_to_{}_index", side(from), side(d));
            let r = row(&name);
            // The belt hugs the corner between its two open sides, so only the opposite corner is bare.
            let bare = corner_ink(&s, r, from.opposite(), d.opposite());
            assert!(bare < 0.15, "{name}: the corner away from the bend has {bare:.2} ink");
            for (a, b) in [(from, d), (from, d.opposite()), (from.opposite(), d)] {
                let full = corner_ink(&s, r, a, b);
                assert!(full > 0.5, "{name}: the {a:?}{b:?} corner has only {full:.2} ink");
            }
            // Along the bend the chevrons also drift sideways, but at each open side they cross it the right way: in
            // through the first, out through the second. The two rows of one corner differ only in this.
            let out = motion(&s, r, strip(d));
            assert!(along(out, d) > 0, "{name}: at the {d:?} side the chevrons move {out:?}, not out");
            let entry = motion(&s, r, strip(from));
            assert!(along(entry, from.opposite()) > 0, "{name}: at the {from:?} side the chevrons move {entry:?}, not in");
        }
    }
}

#[test]
#[ignore = "reads the game's sheets from FACTORIO_DIR"]
fn a_lip_sits_on_the_edge_it_shares_with_its_belt_and_moves_with_it() {
    let s = belt_sheet();
    for x in Dir::ALL {
        // Drawn on the tile on side X of the belt, a lip touches that tile's edge facing the belt.
        let facing = x.opposite();
        for (kind, travel) in [("starting", facing), ("ending", x)] {
            let name = format!("{kind}_{}_index", side(x));
            let r = row(&name);
            let (mut n, mut sx, mut sy, mut lo, mut hi) = (0u32, 0u32, 0u32, u32::MAX, 0u32);
            for y in 0..CELL {
                for px in 0..CELL {
                    if s.get_pixel(px, r * CELL + y)[3] > 64 {
                        let across = if facing.axis() == 1 { y } else { px };
                        (n, sx, sy, lo, hi) = (n + 1, sx + px, sy + y, lo.min(across), hi.max(across));
                    }
                }
            }
            assert!(n > 0, "{name} is empty");
            let centre = if facing.axis() == 1 { sy / n } else { sx / n };
            let edge = if matches!(facing, Dir::N | Dir::W) { T0 } else { T1 };
            assert!(centre.abs_diff(edge) < 16 && hi - lo < 26, "{name}: ink {lo}..{hi} around {centre}, not a strip on the {facing:?} edge");
            assert_eq!(heading(motion(&s, r, [0, 0, CELL, CELL])), travel, "{name}");
        }
    }
}

#[test]
#[ignore = "reads the game's sheets from FACTORIO_DIR"]
fn an_underground_column_stands_its_hood_on_its_own_side() {
    // 192 px cells, three tiles, the underground's tile spanning 64..128; exits on the first row, entrances on the second.
    let s = sheet("__base__/graphics/entity/underground-belt/underground-belt-structure.png");
    for (row, what) in [(0, "direction_out"), (1, "direction_in")] {
        for d in Dir::ALL {
            let (mut n, mut sx, mut sy) = (0i64, 0i64, 0i64);
            for y in 64..128 {
                for x in 64..128 {
                    if s.get_pixel(d.ordinal() as u32 * 192 + x, row * 192 + y)[3] > 230 {
                        (n, sx, sy) = (n + 1, sx + i64::from(x), sy + i64::from(y));
                    }
                }
            }
            assert!(n > 0, "{what} {d:?} is empty");
            let toward = (sx / n - 96) * i64::from(d.dx()) + (sy / n - 96) * i64::from(d.dy());
            assert!(toward > 3, "{what} column {d:?}: the hood sits {toward} px toward {d:?}, not on that side");
        }
    }
}
