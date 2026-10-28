use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::Add;

mod layout;
pub use layout::*;

/// Cardinal direction. `value16` is the 2.0 blueprint value (16-way defines.direction: N 0, E 4, S 8, W 12).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Dir {
    N,
    E,
    S,
    W,
}

impl Dir {
    pub const ALL: [Dir; 4] = [Dir::N, Dir::E, Dir::S, Dir::W];

    pub fn ordinal(self) -> usize {
        self as usize
    }

    pub fn dx(self) -> i32 {
        [0, 1, 0, -1][self as usize]
    }

    pub fn dy(self) -> i32 {
        [-1, 0, 1, 0][self as usize]
    }

    pub fn value16(self) -> i32 {
        [0, 4, 8, 12][self as usize]
    }

    pub fn opposite(self) -> Dir {
        Dir::ALL[(self as usize + 2) % 4]
    }

    pub fn cw(self) -> Dir {
        Dir::ALL[(self as usize + 1) % 4]
    }

    pub fn ccw(self) -> Dir {
        Dir::ALL[(self as usize + 3) % 4]
    }

    /// 0 for E/W travel, 1 for N/S travel.
    pub fn axis(self) -> i32 {
        if self.dx() != 0 { 0 } else { 1 }
    }

    pub fn name(self) -> &'static str {
        ["N", "E", "S", "W"][self as usize]
    }

    pub fn from16(value: i32) -> Result<Dir> {
        match Dir::ALL.iter().find(|d| d.value16() == value) {
            Some(d) => Ok(*d),
            None => bail!("Direction {value} is not cardinal"),
        }
    }

    pub fn from_name(s: &str) -> Result<Dir> {
        Ok(match s.to_lowercase().as_str() {
            "n" | "north" => Dir::N,
            "e" | "east" => Dir::E,
            "s" | "south" => Dir::S,
            "w" | "west" => Dir::W,
            _ => bail!("Unknown direction '{s}'"),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Tile {
    pub x: i32,
    pub y: i32,
}

impl Tile {
    pub fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    pub fn step(self, d: Dir, n: i32) -> Tile {
        Tile::new(self.x + d.dx() * n, self.y + d.dy() * n)
    }

    pub fn manhattan(self, o: Tile) -> i32 {
        (self.x - o.x).abs() + (self.y - o.y).abs()
    }
}

impl Add<Dir> for Tile {
    type Output = Tile;

    fn add(self, d: Dir) -> Tile {
        Tile::new(self.x + d.dx(), self.y + d.dy())
    }
}

impl fmt::Display for Tile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({},{})", self.x, self.y)
    }
}

/// Axis-aligned rectangle of tiles with top-left (x, y).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub fn x2(&self) -> i32 {
        self.x + self.w - 1
    }

    pub fn y2(&self) -> i32 {
        self.y + self.h - 1
    }

    pub fn contains(&self, tx: i32, ty: i32) -> bool {
        tx >= self.x && tx <= self.x2() && ty >= self.y && ty <= self.y2()
    }

    pub fn contains_tile(&self, t: Tile) -> bool {
        self.contains(t.x, t.y)
    }

    pub fn overlaps(&self, o: &Rect) -> bool {
        self.x <= o.x2() && o.x <= self.x2() && self.y <= o.y2() && o.y <= self.y2()
    }

    /// Row by row, top to bottom.
    pub fn tiles(&self) -> Vec<Tile> {
        (self.y..=self.y2()).flat_map(|ty| (self.x..=self.x2()).map(move |tx| Tile::new(tx, ty))).collect()
    }

    /// Tiles at distance `d` outside side `side` (d = 1 is the first row outside), along the whole side.
    /// Corners are not included.
    pub fn side_tiles(&self, side: Dir, d: i32) -> Vec<Tile> {
        match side {
            Dir::N => (self.x..=self.x2()).map(|x| Tile::new(x, self.y - d)).collect(),
            Dir::S => (self.x..=self.x2()).map(|x| Tile::new(x, self.y2() + d)).collect(),
            Dir::W => (self.y..=self.y2()).map(|y| Tile::new(self.x - d, y)).collect(),
            Dir::E => (self.y..=self.y2()).map(|y| Tile::new(self.x2() + d, y)).collect(),
        }
    }

    /// The tile of this rectangle's edge row on `side` that lies next to the outside tile `outside`.
    pub fn edge_tile_facing(&self, side: Dir, outside: Tile) -> Tile {
        match side {
            Dir::N => Tile::new(outside.x, self.y),
            Dir::S => Tile::new(outside.x, self.y2()),
            Dir::W => Tile::new(self.x, outside.y),
            Dir::E => Tile::new(self.x2(), outside.y),
        }
    }

    /// Middle tile at distance `d` outside `side` (for odd sizes).
    pub fn side_middle(&self, side: Dir, d: i32) -> Tile {
        match side {
            Dir::N => Tile::new(self.x + self.w / 2, self.y - d),
            Dir::S => Tile::new(self.x + self.w / 2, self.y2() + d),
            Dir::W => Tile::new(self.x - d, self.y + self.h / 2),
            Dir::E => Tile::new(self.x2() + d, self.y + self.h / 2),
        }
    }
}

/// Index arithmetic for a box of `w` x `h` tiles surrounded by a ring of `margin` tiles. Box tiles have
/// coordinates 0 until w / 0 until h; ring tiles have negative or >= w/h coordinates.
#[derive(Clone, Copy, Debug)]
pub struct GridDims {
    pub w: i32,
    pub h: i32,
    pub margin: i32,
    pub full_w: i32,
    pub full_h: i32,
    pub size: usize,
}

impl GridDims {
    pub fn new(w: i32, h: i32, margin: i32) -> Self {
        let (full_w, full_h) = (w + 2 * margin, h + 2 * margin);
        Self { w, h, margin, full_w, full_h, size: (full_w * full_h) as usize }
    }

    pub fn in_full(&self, x: i32, y: i32) -> bool {
        x >= -self.margin && y >= -self.margin && x < self.w + self.margin && y < self.h + self.margin
    }

    pub fn in_box(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && x < self.w && y < self.h
    }

    pub fn idx(&self, x: i32, y: i32) -> usize {
        ((y + self.margin) * self.full_w + (x + self.margin)) as usize
    }

    pub fn idx_tile(&self, t: Tile) -> usize {
        self.idx(t.x, t.y)
    }

    pub fn x(&self, i: usize) -> i32 {
        i as i32 % self.full_w - self.margin
    }

    pub fn y(&self, i: usize) -> i32 {
        i as i32 / self.full_w - self.margin
    }

    pub fn tile(&self, i: usize) -> Tile {
        Tile::new(self.x(i), self.y(i))
    }
}
