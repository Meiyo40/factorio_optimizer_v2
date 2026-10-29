use crate::data::game_constants as gc;
use crate::geom::{Dir, Kind, PlacedEntity, Tile};
use crate::validate::{OutputSpec, SourceSpec};
use indexmap::IndexMap;
use std::collections::HashMap;

/// A routed layout with its ports moved onto the ring around the box: the entities and the new port tiles.
pub struct RingedPorts {
    pub entities: Vec<PlacedEntity>,
    pub sources: Vec<SourceSpec>,
    pub outputs: Vec<OutputSpec>,
}

fn is_pipe(kind: Kind) -> bool {
    matches!(kind, Kind::Pipe | Kind::PipeUg | Kind::InfinityPipe)
}

/// Moves every port on the box edge one tile out, onto the ring around the box, so inputs and outputs stand out:
/// an input belt gets one more belt behind it (inputs enter the box heading straight in, so lanes stay as they
/// are), an output belt one more in front, a fluid port one more pipe unless that pipe would touch a pipe of
/// another fluid. Ports already outside the box, or whose ring tile is taken, stay where they are, and so does a
/// fluid port on a pipe-to-ground (it sits beside a port of another fluid and is fed through its open side).
pub fn ring_ports(entities: &[PlacedEntity], sources: &[SourceSpec], outputs: &[OutputSpec], w: i32, h: i32, belt_name: &str) -> RingedPorts {
    let in_box = |t: Tile| t.x >= 0 && t.x < w && t.y >= 0 && t.y < h;
    let mut list: Vec<PlacedEntity> = entities.to_vec();
    // The tile map holds an index into `list`, so a later entity overwrites an earlier one on a shared tile.
    let mut occ: HashMap<Tile, usize> = HashMap::new();
    for (i, e) in entities.iter().enumerate() {
        for t in e.rect().tiles() {
            occ.insert(t, i);
        }
    }
    let add = |e: PlacedEntity, list: &mut Vec<PlacedEntity>, occ: &mut HashMap<Tile, usize>| {
        let i = list.len();
        for t in e.rect().tiles() {
            occ.insert(t, i);
        }
        list.push(e);
    };
    // A ring pipe joins every neighbour: none may hold a pipe of another fluid.
    let pipe_safe = |r: Tile, fluid: &str, list: &[PlacedEntity], occ: &HashMap<Tile, usize>| {
        Dir::ALL.iter().all(|&d| occ.get(&(r + d)).is_none_or(|&i| !is_pipe(list[i].kind) || list[i].fluid.as_deref() == Some(fluid)))
    };
    let pipe_out = |t: Tile, fluid: &str, net: Option<i32>, list: &mut Vec<PlacedEntity>, occ: &mut HashMap<Tile, usize>| -> Option<Tile> {
        let d = Dir::ALL.into_iter().filter(|&d| !in_box(t + d)).find(|&d| !occ.contains_key(&(t + d)) && pipe_safe(t + d, fluid, list, occ))?;
        let r = t + d;
        let mut pipe = PlacedEntity::new(gc::PIPE, Kind::Pipe, r.x, r.y, Dir::N);
        pipe.fluid = Some(fluid.to_owned());
        pipe.net = net;
        add(pipe, list, occ);
        Some(r)
    };

    // Inputs, one ring tile per port tile (two items sharing a belt move together).
    let mut moved: HashMap<Tile, Tile> = HashMap::new();
    let mut groups: IndexMap<Tile, Vec<&SourceSpec>> = IndexMap::new();
    for s in sources {
        groups.entry(s.tile).or_default().push(s);
    }
    for (&t, group) in &groups {
        if !in_box(t) {
            continue;
        }
        let Some(&ei) = occ.get(&t) else { continue };
        let e = list[ei].clone();
        if group[0].fluid {
            if e.kind == Kind::Pipe
                && let Some(r) = pipe_out(t, &group[0].item, e.net, &mut list, &mut occ)
            {
                moved.insert(t, r);
            }
            continue;
        }
        if !e.kind.belt_family() || e.kind == Kind::Splitter {
            continue;
        }
        let r = t + e.dir.opposite();
        if in_box(r) || occ.contains_key(&r) {
            continue;
        }
        let mut belt = PlacedEntity::new(belt_name, Kind::Belt, r.x, r.y, e.dir);
        belt.net = e.net;
        add(belt, &mut list, &mut occ);
        moved.insert(t, r);
    }
    let new_sources: Vec<SourceSpec> = sources.iter().map(|s| moved.get(&s.tile).map_or_else(|| s.clone(), |&t| SourceSpec { tile: t, ..s.clone() })).collect();
    let new_outputs: Vec<OutputSpec> = outputs
        .iter()
        .map(|o| {
            if !in_box(o.tile) {
                return o.clone();
            }
            let Some(&ei) = occ.get(&o.tile) else { return o.clone() };
            let e = list[ei].clone();
            if o.fluid {
                let r = if e.kind == Kind::Pipe { pipe_out(o.tile, &o.item, e.net, &mut list, &mut occ) } else { None };
                return r.map_or_else(|| o.clone(), |t| OutputSpec { tile: t, ..o.clone() });
            }
            if e.kind != Kind::Belt && e.kind != Kind::UgOut {
                return o.clone();
            }
            let r = o.tile + e.dir;
            if in_box(r) || occ.contains_key(&r) {
                return o.clone();
            }
            let mut belt = PlacedEntity::new(belt_name, Kind::Belt, r.x, r.y, e.dir);
            belt.net = e.net;
            add(belt, &mut list, &mut occ);
            OutputSpec { tile: r, ..o.clone() }
        })
        .collect();
    RingedPorts { entities: list, sources: new_sources, outputs: new_outputs }
}
