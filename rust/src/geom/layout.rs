use super::{Dir, Rect, Tile};
use crate::data::game_constants as gc;
use anyhow::{Result, bail};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Machine,
    Belt,
    UgIn,
    UgOut,
    Splitter,
    Inserter,
    Chest,
    Pole,
    InfinityPipe,
    InfinityChest,
    Pipe,
    PipeUg,
}

impl Kind {
    pub fn belt_family(self) -> bool {
        matches!(self, Kind::Belt | Kind::UgIn | Kind::UgOut | Kind::Splitter)
    }
}

/// One entity of a layout, in tile coordinates with top-left (x, y).
/// `dir` means: travel direction for belts, undergrounds and splitters; pickup side for inserters (as in the
/// blueprint); fluid input side for machines.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PlacedEntity {
    pub name: String,
    pub kind: Kind,
    pub x: i32,
    pub y: i32,
    pub dir: Dir,
    pub recipe: Option<String>,
    /// Item for infinity chests (what they supply), filtered sink chests and filtered inserters.
    pub item: Option<String>,
    /// Fluid for infinity pipes.
    pub fluid: Option<String>,
    /// Net id, for renders and validator messages only; not written to blueprints.
    pub net: Option<i32>,
    /// Machines: mirrored (2.0 blueprint "mirror"); applied before the rotation in `dir`.
    pub mirror: bool,
    /// Infinity pipes: keep the pipe empty (a sink for `fluid`) instead of full.
    pub drain: bool,
    pub w: i32,
    pub h: i32,
}

impl PlacedEntity {
    pub fn new(name: &str, kind: Kind, x: i32, y: i32, dir: Dir) -> Self {
        let (w, h) = Self::default_size(name, kind, dir);
        Self {
            name: name.to_owned(),
            kind,
            x,
            y,
            dir,
            recipe: None,
            item: None,
            fluid: None,
            net: None,
            mirror: false,
            drain: false,
            w,
            h,
        }
    }

    pub fn rect(&self) -> Rect {
        Rect::new(self.x, self.y, self.w, self.h)
    }

    pub fn tile(&self) -> Tile {
        Tile::new(self.x, self.y)
    }

    /// Comparable identity without debug-only fields.
    pub fn stripped(&self) -> Self {
        Self { net: None, ..self.clone() }
    }

    /// Inserter reach from its name (1, or 2 for long-handed).
    pub fn reach(&self) -> i32 {
        gc::inserter(&self.name).expect("an inserter entity has a known name").reach
    }

    /// Tile an inserter picks from.
    pub fn pickup_tile(&self) -> Tile {
        self.tile().step(self.dir, self.reach())
    }

    /// Tile an inserter drops into.
    pub fn drop_tile(&self) -> Tile {
        self.tile().step(self.dir.opposite(), self.reach())
    }

    /// Tile a belt-family entity pushes items into (None for underground entrances, which go below ground).
    pub fn output_tile(&self) -> Option<Tile> {
        match self.kind {
            Kind::Belt | Kind::UgOut => Some(self.tile() + self.dir),
            _ => None,
        }
    }

    pub fn default_size(name: &str, kind: Kind, dir: Dir) -> (i32, i32) {
        match kind {
            Kind::Machine => gc::machine_size(name).unwrap_or((3, 3)),
            Kind::Splitter => if dir.axis() == 1 { (2, 1) } else { (1, 2) },
            Kind::Pole => gc::footprint(name),
            _ => (1, 1),
        }
    }

    pub fn kind_of(name: &str, ug_type: Option<&str>) -> Result<Kind> {
        let tier = gc::tier_of_entity(name);
        Ok(if tier.is_some_and(|t| name == t.belt) {
            Kind::Belt
        } else if tier.is_some_and(|t| name == t.underground) {
            if ug_type == Some("output") { Kind::UgOut } else { Kind::UgIn }
        } else if tier.is_some_and(|t| name == t.splitter) {
            Kind::Splitter
        } else if gc::inserter_spec(name).is_some() {
            Kind::Inserter
        } else if name == gc::INFINITY_CHEST {
            Kind::InfinityChest
        } else if gc::CHESTS.contains(&name) {
            Kind::Chest
        } else if gc::pole_spec(name).is_some() {
            Kind::Pole
        } else if name == gc::INFINITY_PIPE {
            Kind::InfinityPipe
        } else if gc::machine_size(name).is_some() {
            Kind::Machine
        } else if name == gc::PIPE {
            Kind::Pipe
        } else if name == gc::PIPE_TO_GROUND {
            Kind::PipeUg
        } else {
            bail!("Unknown entity '{name}'")
        })
    }
}

/// A floor tile of the blueprint (`name` is a tile prototype, e.g. hazard-concrete-left).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GroundTile {
    pub name: String,
    pub x: i32,
    pub y: i32,
}

/// The ring of tiles just inside `r`, as floor tiles named `name`.
pub fn border_tiles(r: &Rect, name: &str) -> Vec<GroundTile> {
    r.tiles()
        .into_iter()
        .filter(|t| t.x == r.x || t.y == r.y || t.x == r.x2() || t.y == r.y2())
        .map(|t| GroundTile { name: name.to_owned(), x: t.x, y: t.y })
        .collect()
}

/// The final format consumed by validate, emit and viz.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// Box size; entities may also lie in the ring around it (ports, harness, outside poles).
    pub w: i32,
    pub h: i32,
    pub entities: Vec<PlacedEntity>,
    /// Copper wires between poles, as entity indices.
    pub wires: Vec<(usize, usize)>,
    pub label: String,
    /// Floor tiles (markers: block borders, reserved areas); entities stand on top of them.
    pub tiles: Vec<GroundTile>,
}

impl Layout {
    pub fn new(w: i32, h: i32, entities: Vec<PlacedEntity>) -> Self {
        Self { w, h, entities, wires: Vec::new(), label: "factorio-optimizer layout".to_owned(), tiles: Vec::new() }
    }

    pub fn count(&self, kind: Kind) -> usize {
        self.entities.iter().filter(|e| e.kind == kind).count()
    }

    /// Map tile -> entity index; later entities overwrite earlier ones (the validator reports overlaps separately).
    pub fn tile_index(&self) -> HashMap<Tile, usize> {
        let mut m = HashMap::new();
        for (i, e) in self.entities.iter().enumerate() {
            for t in e.rect().tiles() {
                m.insert(t, i);
            }
        }
        m
    }

    pub fn bounds(&self) -> Rect {
        if self.entities.is_empty() {
            return Rect::new(0, 0, self.w, self.h);
        }
        let x0 = self.entities.iter().map(|e| e.x).min().unwrap().min(0);
        let y0 = self.entities.iter().map(|e| e.y).min().unwrap().min(0);
        let x1 = self.entities.iter().map(|e| e.rect().x2()).max().unwrap().max(self.w - 1);
        let y1 = self.entities.iter().map(|e| e.rect().y2()).max().unwrap().max(self.h - 1);
        Rect::new(x0, y0, x1 - x0 + 1, y1 - y0 + 1)
    }
}
