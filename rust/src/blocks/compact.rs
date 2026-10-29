use crate::geom::{Dir, GroundTile, Kind, PlacedEntity, Tile};
use std::collections::HashMap;

/// A layout with some rows and columns of its box taken out, and where every old coordinate went. Coordinates
/// outside the box keep their side: before the box unchanged, after it shifted by what was removed.
#[derive(Debug)]
pub struct Compaction {
    pub entities: Vec<PlacedEntity>,
    pub tiles: Vec<GroundTile>,
    pub w: i32,
    pub h: i32,
    new_x: Vec<i32>,
    new_y: Vec<i32>,
    pub rows_removed: i32,
    pub cols_removed: i32,
}

impl Compaction {
    pub fn x(&self, x: i32) -> i32 {
        if x < 0 {
            x
        } else if x >= self.new_x.len() as i32 {
            x - self.cols_removed
        } else {
            self.new_x[x as usize]
        }
    }

    pub fn y(&self, y: i32) -> i32 {
        if y < 0 {
            y
        } else if y >= self.new_y.len() as i32 {
            y - self.rows_removed
        } else {
            self.new_y[y as usize]
        }
    }

    pub fn tile(&self, t: Tile) -> Tile {
        Tile::new(self.x(t.x), self.y(t.y))
    }
}

fn is_pipe(kind: Kind) -> bool {
    matches!(kind, Kind::Pipe | Kind::PipeUg | Kind::InfinityPipe)
}

fn occupancy(ents: &[PlacedEntity]) -> HashMap<Tile, usize> {
    let mut m = HashMap::new();
    for (i, e) in ents.iter().enumerate() {
        for t in e.rect().tiles() {
            m.insert(t, i);
        }
    }
    m
}

/// Can line `c` go? `row`: the row y = c (crossed vertically), else the column x = c (crossed horizontally).
fn removable(ents: &[PlacedEntity], occ: &HashMap<Tile, usize>, cw: i32, ch: i32, c: i32, row: bool) -> bool {
    let len = if row { cw } else { ch };
    let at = |i: i32, off: i32| if row { Tile::new(i, c + off) } else { Tile::new(c + off, i) };
    let ent = |t: Tile| occ.get(&t).map(|&i| &ents[i]);
    let across = |d: Dir| if row { d == Dir::N || d == Dir::S } else { d == Dir::E || d == Dir::W };
    // No inserter may pick or drop across the line (its pickup, itself and its drop span the line).
    for e in ents {
        if e.kind != Kind::Inserter || !across(e.dir) {
            continue;
        }
        let (p, d) = (e.pickup_tile(), e.drop_tile());
        let (lo, hi) = if row { (p.y.min(d.y), p.y.max(d.y)) } else { (p.x.min(d.x), p.x.max(d.x)) };
        if (lo..=hi).contains(&c) {
            return false;
        }
    }
    for i in 0..len {
        let t = at(i, 0);
        let a = ent(at(i, -1));
        let b = ent(at(i, 1));
        let Some(e) = ent(t) else {
            // The tiles before and after the gap come to touch: neither may push into the other, and no
            // pipe may meet another pipe or a machine's connection.
            let (Some(a), Some(b)) = (a, b) else { continue };
            let points_into = |x: &PlacedEntity| {
                x.output_tile() == Some(t)
                    || (x.kind == Kind::UgIn && x.tile() + x.dir == t)
                    || (x.kind == Kind::Splitter && x.rect().tiles().iter().any(|&q| q + x.dir == t))
            };
            if points_into(a) || points_into(b) {
                return false;
            }
            if is_pipe(a.kind) && (is_pipe(b.kind) || b.kind == Kind::Machine) {
                return false;
            }
            if is_pipe(b.kind) && a.kind == Kind::Machine {
                return false;
            }
            continue;
        };
        match e.kind {
            Kind::Belt => {
                if !across(e.dir) {
                    return false;
                }
                let behind = ent(t + e.dir.opposite());
                let ahead = ent(t + e.dir);
                let Some(behind) = behind else { return false };
                if !behind.kind.belt_family() || behind.dir != e.dir || (behind.kind != Kind::Splitter && behind.output_tile() != Some(t)) {
                    return false;
                }
                let Some(ahead) = ahead else { return false };
                if (ahead.kind != Kind::Belt && ahead.kind != Kind::UgIn) || ahead.dir != e.dir {
                    return false;
                }
            }
            Kind::Pipe => {
                let (Some(a), Some(b)) = (a, b) else { return false };
                if a.kind != Kind::Pipe || b.kind != Kind::Pipe {
                    return false;
                }
                // Nothing beside it along the line that it could be joined to.
                for s in [at(i - 1, 0), at(i + 1, 0)] {
                    if ent(s).is_some_and(|it| is_pipe(it.kind) || it.kind == Kind::Machine) {
                        return false;
                    }
                }
            }
            _ => return false,
        }
    }
    true
}

/// Removes every row and column of the box that only straight belts, straight pipes or underground spans cross,
/// one at a time and checking again after each: a line goes when every entity on it is a belt heading straight
/// across it (fed straight from behind, feeding straight on), or a plain pipe joined only to plain pipes before
/// and after it, and no inserter reaches across it; and on its empty tiles no belt points into the gap at
/// something, and no pipe would come to touch a pipe or a machine. Nothing changes but lengths, so whatever
/// worked still works (callers validate anyway). Poles must be removed first and placed again afterwards.
pub fn compact_layout(entities: &[PlacedEntity], tiles: &[GroundTile], w: i32, h: i32) -> Compaction {
    let mut ents = entities.to_vec();
    let mut ground = tiles.to_vec();
    let (mut cw, mut ch) = (w, h);
    let mut new_x: Vec<i32> = (0..w).collect();
    let mut new_y: Vec<i32> = (0..h).collect();
    let (mut rows, mut cols) = (0, 0);

    let mut changed = true;
    while changed {
        changed = false;
        for row in [true, false] {
            let mut c = 0;
            let mut occ = occupancy(&ents);
            while c < if row { ch } else { cw } {
                if !removable(&ents, &occ, cw, ch, c, row) {
                    c += 1;
                    continue;
                }
                // Entities on the line are taken out (a line only has one-tile-thick ones), the rest move up.
                ents.retain(|e| !(if row { e.y == c && e.h == 1 } else { e.x == c && e.w == 1 }));
                for e in &mut ents {
                    if row && e.y > c {
                        e.y -= 1;
                    } else if !row && e.x > c {
                        e.x -= 1;
                    }
                }
                ground.retain(|g| if row { g.y != c } else { g.x != c });
                for g in &mut ground {
                    if row && g.y > c {
                        g.y -= 1;
                    } else if !row && g.x > c {
                        g.x -= 1;
                    }
                }
                if row {
                    new_y.iter_mut().for_each(|y| *y -= i32::from(*y > c));
                    ch -= 1;
                    rows += 1;
                } else {
                    new_x.iter_mut().for_each(|x| *x -= i32::from(*x > c));
                    cw -= 1;
                    cols += 1;
                }
                occ = occupancy(&ents);
                changed = true;
            }
        }
    }
    Compaction { entities: ents, tiles: ground, w: cw, h: ch, new_x, new_y, rows_removed: rows, cols_removed: cols }
}
