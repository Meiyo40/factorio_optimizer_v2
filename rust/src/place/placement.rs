use crate::config::run_configuration::{ConfigSource, RunConfiguration};
use crate::data::data_model::RootData;
use crate::data::game_constants as gc;
use crate::data::loader::read_resource_or_file;
use crate::geom::{Dir, Rect, Tile};
use crate::plan::{MachineInstance, ProductionPlan};
use anyhow::{Context, Result, anyhow, ensure};
use indexmap::IndexMap;
use serde::Deserialize;
use std::collections::{HashMap, HashSet, VecDeque};

/// Machine `id` (index into the plan's machines) with top-left (x, y), rotated `rot` (north = as in the
/// connection table) and optionally mirrored. Rotation only matters for machines with fluid connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlacedMachine {
    pub id: usize,
    pub x: i32,
    pub y: i32,
    pub rot: Dir,
    pub mirror: bool,
}

impl PlacedMachine {
    pub fn new(id: usize, x: i32, y: i32) -> Self {
        Self { id, x, y, rot: Dir::N, mirror: false }
    }
}

/// A placed machine's fluid connection: the pipe tile outside it, the machine side it is on, and the fluid
/// the recipe sends through it (None: the connection exists but this recipe does not use it, so no foreign
/// pipe may touch it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnTile {
    pub tile: Tile,
    pub side: Dir,
    pub flow: gc::Flow,
    pub index: i32,
    pub fluid: Option<String>,
}

/// Fluid connections of `inst` at (x, y) with rotation `rot` (0..3) and `mirror`.
pub fn connections(inst: &MachineInstance, x: i32, y: i32, rot: i32, mirror: bool) -> Vec<ConnTile> {
    connections_of(&inst.machine, inst.w, inst.h, &inst.fluid_in, &inst.fluid_out, x, y, rot, mirror)
}

/// Fluid connections of a `machine` whose recipe maps fluids to connection indices as given.
#[allow(clippy::too_many_arguments)]
pub fn connections_of(
    machine: &str,
    w: i32,
    h: i32,
    fluid_in: &IndexMap<String, i32>,
    fluid_out: &IndexMap<String, i32>,
    x: i32,
    y: i32,
    rot: i32,
    mirror: bool,
) -> Vec<ConnTile> {
    let base = gc::base_fluid_connections(machine);
    if base.is_empty() || (gc::connections_only_with_fluid_recipe(machine) && fluid_in.is_empty() && fluid_out.is_empty()) {
        return Vec::new();
    }
    base.iter()
        .map(|c| {
            let (dx, dy) = gc::place_connection(c, w, h, rot, mirror);
            let by_index = if c.flow == gc::Flow::Input { fluid_in } else { fluid_out };
            ConnTile {
                tile: Tile::new(x + dx, y + dy),
                side: gc::place_side(c, rot, mirror),
                flow: c.flow,
                index: c.index,
                fluid: by_index.iter().find(|(_, v)| **v == c.index).map(|(k, _)| k.clone()),
            }
        })
        .collect()
}

/// Whether rotating or mirroring `inst` changes anything (it has fluid connections).
pub fn orientable(inst: &MachineInstance) -> bool {
    !connections(inst, 0, 0, 0, false).is_empty()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub w: i32,
    pub h: i32,
    pub machines: Vec<PlacedMachine>,
}

impl Placement {
    pub fn rect(&self, plan: &ProductionPlan, m: &PlacedMachine) -> Rect {
        let inst = &plan.machines[m.id];
        Rect::new(m.x, m.y, inst.w, inst.h)
    }

    pub fn connections(&self, plan: &ProductionPlan, m: &PlacedMachine) -> Vec<ConnTile> {
        connections(&plan.machines[m.id], m.x, m.y, m.rot.ordinal() as i32, m.mirror)
    }
}

/// Static tiles the placer and router must keep free: forbidden tiles and fixed port tiles inside the box.
pub fn static_obstacles(config: &RunConfiguration, plan: &ProductionPlan, w: i32, h: i32) -> HashSet<Tile> {
    let in_box = |t: &Tile| t.x >= 0 && t.x < w && t.y >= 0 && t.y < h;
    let mut out = HashSet::new();
    for r in &config.forbidden_tiles {
        out.extend(Rect::new(r.x, r.y, r.w, r.h).tiles().into_iter().filter(in_box));
    }
    for r in config.reserved_rects(w, h) {
        out.extend(r.tiles().into_iter().filter(in_box));
    }
    for p in &plan.ports {
        for s in &p.specs {
            if let (Some(x), Some(y)) = (s.x, s.y) {
                let t = Tile::new(x, y);
                if in_box(&t) {
                    out.insert(t);
                }
            }
        }
    }
    out
}

/// Candidate start tiles of a port in a W x H box, each with the direction that points out of the box (or None).
pub fn port_candidates(specs: &[ConfigSource], w: i32, h: i32) -> Vec<(Tile, Option<Dir>)> {
    fn range(from: Option<i32>, to: Option<i32>, size: i32) -> std::ops::RangeInclusive<i32> {
        from.unwrap_or(0).max(0)..=to.unwrap_or(size - 1).min(size - 1)
    }
    fn edge(out: &mut IndexMap<Tile, Option<Dir>>, e: &str, from: Option<i32>, to: Option<i32>, w: i32, h: i32) {
        match e {
            "north" => range(from, to, w).for_each(|x| _ = out.entry(Tile::new(x, 0)).or_insert(Some(Dir::N))),
            "south" => range(from, to, w).for_each(|x| _ = out.entry(Tile::new(x, h - 1)).or_insert(Some(Dir::S))),
            "west" => range(from, to, h).for_each(|y| _ = out.entry(Tile::new(0, y)).or_insert(Some(Dir::W))),
            "east" => range(from, to, h).for_each(|y| _ = out.entry(Tile::new(w - 1, y)).or_insert(Some(Dir::E))),
            "any" => {
                for side in ["north", "east", "south", "west"] {
                    edge(out, side, None, None, w, h);
                }
            }
            _ => {}
        }
    }
    let mut out = IndexMap::new();
    for s in specs {
        if let Some(e) = &s.edge {
            edge(&mut out, e, s.from, s.to, w, h);
        } else {
            let dir = s.dir.as_deref().map(|d| Dir::from_name(d).expect("a validated port direction"));
            out.entry(Tile::new(s.x.expect("a port without an edge has x"), s.y.expect("a port without an edge has y"))).or_insert(dir);
        }
    }
    out.into_iter().collect()
}

// ---- fixture files: hand placements for router tests ----

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureMachine {
    pub recipe: String,
    pub x: i32,
    pub y: i32,
    #[serde(default = "north")]
    pub rot: String,
}

fn north() -> String {
    "N".to_owned()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementFixture {
    pub job: String,
    pub w: i32,
    pub h: i32,
    pub machines: Vec<FixtureMachine>,
}

impl PlacementFixture {
    pub fn load(name: &str) -> Result<Self> {
        serde_json::from_str(&read_resource_or_file(name)?).with_context(|| format!("Bad placement fixture {name}"))
    }

    /// Matches fixture machines to plan instances by recipe, in order.
    pub fn to_placement(&self, plan: &ProductionPlan) -> Result<Placement> {
        let mut pool: HashMap<&str, VecDeque<usize>> = HashMap::new();
        for m in &plan.machines {
            pool.entry(&m.recipe).or_default().push_back(m.id);
        }
        let mut placed = Vec::with_capacity(self.machines.len());
        for fm in &self.machines {
            let id = pool
                .get_mut(fm.recipe.as_str())
                .and_then(VecDeque::pop_front)
                .ok_or_else(|| anyhow!("Fixture places more {} machines than the plan has", fm.recipe))?;
            placed.push(PlacedMachine { id, x: fm.x, y: fm.y, rot: Dir::from_name(&fm.rot)?, mirror: false });
        }
        let missing: Vec<String> = pool.iter().filter(|(_, ids)| !ids.is_empty()).map(|(r, ids)| format!("{r}: {}", ids.len())).collect();
        ensure!(missing.is_empty(), "Fixture leaves machines unplaced: {missing:?}");
        Ok(Placement { w: self.w, h: self.h, machines: placed })
    }
}

/// `RootData` is only needed to spell out that fixtures name recipes, not machines.
#[allow(dead_code)]
fn _recipe_names_exist(data: &RootData, fixture: &PlacementFixture) -> bool {
    fixture.machines.iter().all(|m| data.recipes.contains_key(&m.recipe))
}
