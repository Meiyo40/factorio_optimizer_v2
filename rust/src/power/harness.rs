use crate::config::run_configuration::RunConfiguration;
use crate::data::game_constants as gc;
use crate::geom::{Dir, Kind, PlacedEntity, Tile};
use crate::plan::{inserter_count, inserter_rate};
use crate::util::text::fmt_list;
use crate::validate::{OutputSpec, SourceSpec};
use anyhow::Result;
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct Harness {
    pub entities: Vec<PlacedEntity>,
    pub tiles: HashSet<Tile>,
    pub warnings: Vec<String>,
}

/// H1 (test mode): feed every source belt from outside the box with a straight feeder belt whose sides
/// carry inserters taking from infinity chests, enough inserters for the source's rate. Every output belt
/// runs on outside the box past inserters that empty it into void infinity chests; fluid ports get an
/// infinity pipe (a drain for outputs).
pub fn build_harness(
    sources: &[SourceSpec],
    entities: &[PlacedEntity],
    w: i32,
    h: i32,
    config: &RunConfiguration,
    outputs: &[OutputSpec],
) -> Result<Harness> {
    let tier = gc::belt_tier(&config.belt_tier)?;
    let mut occupied: HashSet<Tile> = HashSet::new();
    for e in entities {
        occupied.extend(e.rect().tiles());
    }
    let by_tile: HashMap<Tile, &PlacedEntity> = entities.iter().map(|e| (e.tile(), e)).collect();
    // The fastest allowed inserter that is not long-handed (the first of equals, like maxByOrNull).
    let mut ins_name = "inserter".to_owned();
    let mut best_rate = f64::NEG_INFINITY;
    for name in config.inserters.allowed.iter().filter(|n| *n != "long-handed-inserter") {
        let rate = inserter_rate(name, config, false);
        if rate > best_rate {
            best_rate = rate;
            ins_name = name.clone();
        }
    }
    let cap = inserter_rate(&ins_name, config, false);
    let mut out: Vec<PlacedEntity> = Vec::new();
    let mut tiles: HashSet<Tile> = HashSet::new();
    let mut warnings: Vec<String> = Vec::new();
    let in_box = |t: Tile| t.x >= 0 && t.x < w && t.y >= 0 && t.y < h;

    let fluid_port = |at: Tile, fluid: &str, drain: bool, out: &mut Vec<PlacedEntity>, tiles: &mut HashSet<Tile>, warnings: &mut Vec<String>, occupied: &HashSet<Tile>| {
        // A fluid port gets an infinity pipe just outside the box, next to its pipe.
        let free = |t: Tile, tiles: &HashSet<Tile>| !in_box(t) && !occupied.contains(&t) && !tiles.contains(&t);
        let Some(o) = Dir::ALL.into_iter().find(|&d| !in_box(at + d) && free(at + d, tiles)) else {
            warnings.push(format!("no room for a fluid harness at {at} ({fluid})"));
            return;
        };
        let t = at + o;
        let mut pipe = PlacedEntity::new(gc::INFINITY_PIPE, Kind::InfinityPipe, t.x, t.y, Dir::N);
        pipe.fluid = Some(fluid.to_owned());
        pipe.drain = drain;
        out.push(pipe);
        tiles.insert(t);
    };
    for s in sources.iter().filter(|s| s.fluid) {
        fluid_port(s.tile, &s.item, false, &mut out, &mut tiles, &mut warnings, &occupied);
    }
    for o in outputs.iter().filter(|o| o.fluid) {
        fluid_port(o.tile, &o.item, true, &mut out, &mut tiles, &mut warnings, &occupied);
    }
    let free = |t: Tile, tiles: &HashSet<Tile>| !in_box(t) && !occupied.contains(&t) && !tiles.contains(&t);

    // Item sources, grouped by tile: a shared belt gets one feeder with each item's inserters on the side that
    // drops onto its lane (an inserter drops on the far lane).
    let mut groups: IndexMap<Tile, Vec<&SourceSpec>> = IndexMap::new();
    for s in sources.iter().filter(|s| !s.fluid) {
        groups.entry(s.tile).or_default().push(s);
    }
    for (tile, group) in &groups {
        let tile = *tile;
        let first = by_tile.get(&tile).copied();
        let laned = group.iter().any(|s| s.lane >= 0);
        // Outward direction: off the box, and not the way the first belt faces (it cannot take items head-on);
        // a laned belt must be fed straight from behind so its lanes stay as they are.
        let dirs: Vec<Dir> = match first {
            Some(f) if laned => [f.dir.opposite()].into_iter().filter(|&d| !in_box(tile + d)).collect(),
            _ => Dir::ALL.into_iter().filter(|&d| !in_box(tile + d) && first.map(|f| f.dir) != Some(d)).collect(),
        };
        let Some(o) = dirs.into_iter().find(|&d| free(tile + d, &tiles)) else {
            warnings.push(format!("no room for a harness at {tile} ({})", fmt_list(group.iter().map(|s| s.item.as_str()))));
            continue;
        };
        // Feeder heading o.opposite: inserters on its o.ccw side drop on the left lane, on o.cw the right lane.
        let mut pending: Vec<(&SourceSpec, i32)> = group.iter().map(|s| (*s, inserter_count(s.rate, cap))).collect();
        let sides_for = |s: &SourceSpec| match s.lane {
            0 => vec![o.ccw()],
            1 => vec![o.cw()],
            _ => vec![o.cw(), o.ccw()],
        };
        let total: i32 = pending.iter().map(|p| p.1).sum();
        let mut i = 1;
        while pending.iter().any(|p| p.1 > 0) && i <= total + 4 {
            let f = tile.step(o, i);
            if !free(f, &tiles) {
                warnings.push(format!("harness at {tile} blocked at {f}"));
                break;
            }
            out.push(PlacedEntity::new(tier.belt, Kind::Belt, f.x, f.y, o.opposite()));
            tiles.insert(f);
            for side in [o.cw(), o.ccw()] {
                let Some(k) = pending.iter().position(|(s, n)| *n > 0 && sides_for(s).contains(&side)) else { continue };
                let ins_t = f + side;
                let chest_t = f.step(side, 2);
                if !free(ins_t, &tiles) || !free(chest_t, &tiles) {
                    continue;
                }
                out.push(PlacedEntity::new(&ins_name, Kind::Inserter, ins_t.x, ins_t.y, side)); // picks from the chest side
                let mut chest = PlacedEntity::new(gc::INFINITY_CHEST, Kind::InfinityChest, chest_t.x, chest_t.y, Dir::N);
                chest.item = Some(pending[k].0.item.clone());
                out.push(chest);
                tiles.insert(ins_t);
                tiles.insert(chest_t);
                pending[k].1 -= 1;
            }
            i += 1;
        }
        for (s, n) in pending.iter().filter(|p| p.1 > 0) {
            warnings.push(format!("harness for {} is short of {n} inserters", s.item));
        }
    }
    // Outputs after inputs: an input's feeder must run straight out, an output's void inserters can go either side.
    let pick_cap = inserter_rate(&ins_name, config, true);
    for o in outputs.iter().filter(|o| !o.fluid) {
        let d = by_tile.get(&o.tile).map(|e| e.dir);
        let Some(d) = d.filter(|&d| !in_box(o.tile + d)) else {
            warnings.push(format!("output of {} at {} does not leave the box", o.item, o.tile));
            continue;
        };
        let k = inserter_count(o.rate, pick_cap);
        let mut placed_ins = 0;
        let mut i = 1;
        while placed_ins < k && i <= k + 4 {
            let f = o.tile.step(d, i);
            if !free(f, &tiles) {
                warnings.push(format!("output harness for {} blocked at {f}", o.item));
                break;
            }
            out.push(PlacedEntity::new(tier.belt, Kind::Belt, f.x, f.y, d));
            tiles.insert(f);
            for side in [d.cw(), d.ccw()] {
                if placed_ins >= k {
                    break;
                }
                let ins_t = f + side;
                let chest_t = f.step(side, 2);
                if !free(ins_t, &tiles) || !free(chest_t, &tiles) {
                    continue;
                }
                out.push(PlacedEntity::new(&ins_name, Kind::Inserter, ins_t.x, ins_t.y, side.opposite())); // picks from the belt
                out.push(PlacedEntity::new(gc::INFINITY_CHEST, Kind::InfinityChest, chest_t.x, chest_t.y, Dir::N));
                tiles.insert(ins_t);
                tiles.insert(chest_t);
                placed_ins += 1;
            }
            i += 1;
        }
        if placed_ins < k {
            warnings.push(format!("output harness for {} has {placed_ins} of {k} inserters", o.item));
        }
    }
    Ok(Harness { entities: out, tiles, warnings })
}
