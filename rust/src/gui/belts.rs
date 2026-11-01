//! Which picture of the game's belt sheets each belt-family tile takes in the in-game view, worked out from the
//! layout alone so it can be tested without the game.
//!
//! A belt sheet is a `TransportBeltAnimationSetWithCorners`: 20 rows, one per named index, frame 0 in column 0, each
//! frame a 2x2-tile canvas centred on the belt's tile. The names are side names, measured on `transport-belt.png`
//! (`tests/gui_sheets.rs` checks them against the sheet):
//! - `north_index` and the like: a straight belt travelling that way;
//! - `X_to_Y_index`: a curve entering through side X and leaving through side Y, Y being the belt's direction;
//! - `starting_X_index` / `ending_X_index`: the lip at an open end, drawn on the neighbouring tile on side X, so it
//!   hangs a quarter tile past the belt. A start lip sits behind a belt travelling away from X, an end lip ahead of a
//!   belt travelling towards X.
//!
//! The shape follows Factorio's rule, the one the validator models (`validator.rs`, `check_shapes`): a belt fed from
//! behind is straight; otherwise one fed from exactly one side curves from that side; anything else is straight.
//! "Fed" means the neighbour's belt surface pushes into this tile and this entity takes it (`accepts`).

use crate::geom::{Dir, Kind, Layout, PlacedEntity, Tile};
use crate::validate::validator::accepts;
use std::collections::HashMap;

/// The 20 rows of a belt sheet, in sheet order: row `n` is index `n + 1` in the game's numbering.
pub const BELT_ROWS: [&str; 20] = [
    "east_index",
    "west_index",
    "north_index",
    "south_index",
    "east_to_north_index",
    "north_to_east_index",
    "west_to_north_index",
    "north_to_west_index",
    "south_to_east_index",
    "east_to_south_index",
    "south_to_west_index",
    "west_to_south_index",
    "starting_south_index",
    "ending_south_index",
    "starting_west_index",
    "ending_west_index",
    "starting_north_index",
    "ending_north_index",
    "starting_east_index",
    "ending_east_index",
];

/// One belt surface: `row` is a name from `BELT_ROWS`. Undergrounds draw only the `half` of the canvas on that side
/// of the tile's centre, the open side of the hood; the rest of their tile is under it.
#[derive(Debug, Clone, PartialEq)]
pub struct BeltPiece {
    pub entity: usize,
    pub tile: Tile,
    pub row: String,
    pub half: Option<Dir>,
}

/// The lip at an open belt end, on the tile next to it; `entity` is the belt it belongs to, for the tier's sheet.
#[derive(Debug, Clone, PartialEq)]
pub struct Cap {
    pub entity: usize,
    pub tile: Tile,
    pub row: String,
}

/// An underground's structure: the sheet column is the side its hood is on, `side_loading` the variant for a belt
/// that enters from the side (the game has it for east and west only).
#[derive(Debug, Clone, PartialEq)]
pub struct Hood {
    pub entity: usize,
    pub column: Dir,
    pub side_loading: bool,
}

#[derive(Debug, Default)]
pub struct BeltSprites {
    pub pieces: Vec<BeltPiece>,
    pub caps: Vec<Cap>,
    pub hoods: Vec<Hood>,
}

/// Every belt surface, lip and hood of a layout.
pub fn belt_sprites(layout: &Layout) -> BeltSprites {
    let index = layout.tile_index();
    let mut out = BeltSprites::default();
    for (i, e) in layout.entities.iter().enumerate() {
        if !e.kind.belt_family() {
            continue;
        }
        let d = e.dir;
        for tile in e.rect().tiles() {
            let fed = |side: Dir| fed_from(layout, &index, i, tile, side);
            let behind = fed(d.opposite());
            let (left, right) = (fed(d.ccw()), fed(d.cw()));
            let side_loading = matches!(e.kind, Kind::UgIn | Kind::UgOut) && d.axis() == 0 && !behind && (left || right);
            let row = if e.kind == Kind::Belt && !behind && left != right {
                format!("{}_to_{}_index", side(if left { d.ccw() } else { d.cw() }), side(d))
            } else {
                format!("{}_index", side(d))
            };
            let half = match e.kind {
                Kind::UgIn => Some(d.opposite()),
                Kind::UgOut => Some(d),
                _ => None,
            };
            out.pieces.push(BeltPiece { entity: i, tile, row: row.clone(), half });

            // An underground exit is fed from below, and a curve from its side; a hood in its side-loading variant
            // closes the back with a wall.
            let straight = row == format!("{}_index", side(d));
            if e.kind != Kind::UgOut && straight && !behind && !side_loading {
                out.caps.push(Cap { entity: i, tile: tile + d.opposite(), row: format!("starting_{}_index", side(d.opposite())) });
            }
            // An underground entrance pushes below ground, never into the tile ahead.
            let ahead = tile + d;
            let taken = index.get(&ahead).is_some_and(|&j| j != i && accepts(&layout.entities[j], d));
            if e.kind != Kind::UgIn && !taken {
                out.caps.push(Cap { entity: i, tile: ahead, row: format!("ending_{}_index", side(d)) });
            }
            if matches!(e.kind, Kind::UgIn | Kind::UgOut) {
                out.hoods.push(Hood { entity: i, column: hood_column(e), side_loading });
            }
        }
    }
    out
}

/// The side an underground's hood is on, which is its sheet column: ahead of an entrance, behind an exit. In every
/// column of the structure sheet the hood stands on that side and its mouth faces away from it.
pub fn hood_column(e: &PlacedEntity) -> Dir {
    if e.kind == Kind::UgOut { e.dir.opposite() } else { e.dir }
}

/// Does the belt-family entity on the tile next to `tile` on `side` push into `tile`, and does entity `me` take it?
fn fed_from(layout: &Layout, index: &HashMap<Tile, usize>, me: usize, tile: Tile, side: Dir) -> bool {
    let Some(&j) = index.get(&(tile + side)) else {
        return false;
    };
    let n = &layout.entities[j];
    j != me && n.kind.belt_family() && n.kind != Kind::UgIn && n.dir == side.opposite() && accepts(&layout.entities[me], n.dir)
}

/// The side as the game spells it in index names.
pub fn side(d: Dir) -> &'static str {
    ["north", "east", "south", "west"][d.ordinal()]
}
