//! Factorio 2.0 (Space Age data set) facts the generator relies on, keyed by entity name so tiers can be
//! swapped by config. Values the Kotlin source tags VERIFY have not been confirmed in game yet; see
//! docs/VERIFY.md. Sources:
//!  - [W1] https://lua-api.factorio.com/latest/defines.html (16-way defines.direction)
//!  - [W2] https://wiki.factorio.com/Talk:Blueprint_string_format (wires tuple, wire_connector_id, version example)
//!  - [W3] https://lua-api.factorio.com/latest/concepts/BlueprintEntity.html (entity fields)
//!  - [W6] https://wiki.factorio.com/Inserter_capacity_bonus_(research) (hand sizes per research level)
//!  - [P] base-game prototype data as remembered; VERIFY.

use super::data_model::RootData;
use crate::geom::Dir;
use anyhow::{Result, anyhow};

/// Floor tiles usable as markers (block borders, reserved areas).
pub const MARKER_TILES: [&str; 7] = [
    "stone-path",
    "concrete",
    "hazard-concrete-left",
    "hazard-concrete-right",
    "refined-concrete",
    "refined-hazard-concrete-left",
    "refined-hazard-concrete-right",
];

/// major << 48 | minor << 32 | patch << 16 | build. 2.0.10.1 is the example in [W2]; newer games migrate it.
pub const BLUEPRINT_VERSION: u64 = (2 << 48) | (10 << 16) | 1;

/// Copper wire connector id for poles in the 2.0 "wires" tuple [a, 5, b, 5]. [W2]
pub const WIRE_POLE_COPPER: i32 = 5;

#[derive(Debug, Clone, Copy)]
pub struct BeltTier {
    pub key: &'static str,
    pub belt: &'static str,
    pub underground: &'static str,
    pub splitter: &'static str,
    /// Tiles allowed between an underground entrance and its exit (prototype max_distance - 1). [P]
    pub max_underground_gap: i32,
    /// Items per second for the whole belt; each lane carries half. [P]
    pub throughput: f64,
}

pub static BELT_TIERS: [BeltTier; 4] = [
    BeltTier { key: "basic", belt: "transport-belt", underground: "underground-belt", splitter: "splitter", max_underground_gap: 4, throughput: 15.0 },
    BeltTier { key: "fast", belt: "fast-transport-belt", underground: "fast-underground-belt", splitter: "fast-splitter", max_underground_gap: 6, throughput: 30.0 },
    BeltTier { key: "express", belt: "express-transport-belt", underground: "express-underground-belt", splitter: "express-splitter", max_underground_gap: 8, throughput: 45.0 },
    BeltTier { key: "turbo", belt: "turbo-transport-belt", underground: "turbo-underground-belt", splitter: "turbo-splitter", max_underground_gap: 10, throughput: 60.0 },
];

pub fn belt_tier(key: &str) -> Result<&'static BeltTier> {
    BELT_TIERS.iter().find(|t| t.key == key).ok_or_else(|| {
        anyhow!("Unknown belt tier '{key}' (known: {:?})", BELT_TIERS.iter().map(|t| t.key).collect::<Vec<_>>())
    })
}

/// Which tier a belt-family entity name belongs to, or None.
pub fn tier_of_entity(name: &str) -> Option<&'static BeltTier> {
    BELT_TIERS.iter().find(|t| name == t.belt || name == t.underground || name == t.splitter)
}

#[derive(Debug, Clone, Copy)]
pub struct InserterSpec {
    pub name: &'static str,
    /// Tiles from the inserter's own tile to its pickup and drop tiles. In a blueprint the inserter's
    /// direction points at the pickup side. [P] VERIFY
    pub reach: i32,
    pub filterable: bool,
    /// Items/s chest to chest at hand size 1, no research. [P] VERIFY
    pub base_rate: f64,
    pub bulk: bool,
    /// Relative cost used when picking the cheapest type that meets a rate.
    pub tier_cost: i32,
}

pub static INSERTERS: [InserterSpec; 4] = [
    InserterSpec { name: "inserter", reach: 1, filterable: false, base_rate: 0.83, bulk: false, tier_cost: 1 },
    InserterSpec { name: "long-handed-inserter", reach: 2, filterable: false, base_rate: 1.2, bulk: false, tier_cost: 2 },
    InserterSpec { name: "fast-inserter", reach: 1, filterable: true, base_rate: 2.31, bulk: false, tier_cost: 3 },
    // Same swing speed as the fast inserter; its larger hand comes from HAND_SIZES.
    InserterSpec { name: "bulk-inserter", reach: 1, filterable: true, base_rate: 2.31, bulk: true, tier_cost: 4 },
];

/// Hand sizes (non-bulk, bulk) per inserter capacity research level 0..7. [W6]
pub const HAND_SIZES: [(i32, i32); 8] = [(1, 2), (1, 3), (2, 4), (2, 5), (2, 6), (2, 8), (2, 10), (3, 12)];

pub fn hand_sizes(level: i32) -> (i32, i32) {
    HAND_SIZES[level.clamp(0, HAND_SIZES.len() as i32 - 1) as usize]
}

pub fn inserter_spec(name: &str) -> Option<&'static InserterSpec> {
    INSERTERS.iter().find(|i| i.name == name)
}

pub fn inserter(name: &str) -> Result<&'static InserterSpec> {
    inserter_spec(name)
        .ok_or_else(|| anyhow!("Unknown inserter '{name}' (known: {:?})", INSERTERS.iter().map(|i| i.name).collect::<Vec<_>>()))
}

#[derive(Debug, Clone, Copy)]
pub struct PoleSpec {
    pub name: &'static str,
    pub size: i32,
    /// Half-width of the square supply area, measured from the pole centre. [P] VERIFY
    pub supply_half: f64,
    pub wire_reach: f64,
}

pub static POLES: [PoleSpec; 4] = [
    PoleSpec { name: "small-electric-pole", size: 1, supply_half: 2.5, wire_reach: 7.5 },
    PoleSpec { name: "medium-electric-pole", size: 1, supply_half: 3.5, wire_reach: 9.0 },
    PoleSpec { name: "big-electric-pole", size: 2, supply_half: 2.0, wire_reach: 30.0 },
    PoleSpec { name: "substation", size: 2, supply_half: 9.0, wire_reach: 18.0 },
];

pub fn pole_spec(name: &str) -> Option<&'static PoleSpec> {
    POLES.iter().find(|p| p.name == name)
}

pub fn pole(name: &str) -> Result<&'static PoleSpec> {
    pole_spec(name).ok_or_else(|| anyhow!("Unknown pole '{name}' (known: {:?})", POLES.iter().map(|p| p.name).collect::<Vec<_>>()))
}

/// 1x1 chests usable as sinks or harness sources (2.0 names). [W3]
pub const CHESTS: [&str; 9] = [
    "wooden-chest",
    "iron-chest",
    "steel-chest",
    "passive-provider-chest",
    "active-provider-chest",
    "storage-chest",
    "requester-chest",
    "buffer-chest",
    "infinity-chest",
];

pub const INFINITY_CHEST: &str = "infinity-chest";
pub const INFINITY_PIPE: &str = "infinity-pipe";

/// Default size of non-machine entities (recipes.json lists only machines).
pub fn footprint(name: &str) -> (i32, i32) {
    if let Some(p) = pole_spec(name) {
        (p.size, p.size)
    } else if tier_of_entity(name).is_some_and(|t| t.splitter == name) {
        (2, 1)
    } else {
        (1, 1)
    }
}

/// Assembling machines 2 and 3 have their fluid input in the middle of the side they face and the fluid
/// output in the middle of the opposite side. With direction north the input pipe goes on the tile above
/// the top-middle tile. [P] VERIFY (v9, v14)
pub const FLUID_INPUT_ON_FACING_SIDE: bool = true;

/// Lane an inserter drops onto when it stands in line with the belt (0 left, 1 right). [P] VERIFY (v2)
pub const PARALLEL_DROP_LANE: i32 = 1;

/// Lane (0 left, 1 right of `out`) an inserter swinging `swing` drops onto a belt that items enter heading
/// `into` and leave heading `out` (the same for a straight belt). Straight: the far lane, or
/// `PARALLEL_DROP_LANE` for an inserter in line with the belt. Curve: always the far side, so the inside lane
/// from outside the bend (the side straight ahead of `into`, or behind `out`) and the outside lane from inside
/// it (long-handed, across the belt before or after the curve).
pub fn drop_lane(swing: Dir, into: Dir, out: Dir) -> i32 {
    if into == out || into == out.opposite() {
        return if swing == out.cw() {
            1 // coming from the belt's left, lands on the right lane
        } else if swing == out.ccw() {
            0
        } else {
            PARALLEL_DROP_LANE
        };
    }
    let inside = if out == into.ccw() { 0 } else { 1 };
    if swing == into.opposite() || swing == out { inside } else { 1 - inside }
}

// ---------------- pipes ----------------
pub const PIPE: &str = "pipe";
pub const PIPE_TO_GROUND: &str = "pipe-to-ground";
/// Tiles allowed between two pipe-to-ground ends (max distance 10). [P] VERIFY (v12)
pub const PIPE_TO_GROUND_GAP: i32 = 9;

// ---------------- machines ----------------
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Input,
    Output,
}

/// A fluid connection of a machine facing north: the pipe tile just outside the machine, relative to the
/// machine's top-left tile, and the side of the machine it sits on. `index` is 1-based among connections
/// of the same flow, which is how recipes map fluids to connections (their fluidbox_index, else order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FluidConnection {
    pub flow: Flow,
    pub index: i32,
    pub dx: i32,
    pub dy: i32,
    pub side: Dir,
}

const fn fc(flow: Flow, index: i32, dx: i32, dy: i32, side: Dir) -> FluidConnection {
    FluidConnection { flow, index, dx, dy, side }
}

#[derive(Debug, Clone)]
pub struct MachineGeom {
    pub name: String,
    pub w: i32,
    pub h: i32,
    pub connections: &'static [FluidConnection],
    /// Assembling machines expose their connections only while set to a recipe with fluids.
    pub connections_only_with_fluid_recipe: bool,
    pub burner: bool,
}

use Dir::{E, N, S, W};
use Flow::{Input, Output};

static ASSEMBLER_FLUIDS: [FluidConnection; 2] = [fc(Input, 1, 1, -1, N), fc(Output, 1, 1, 3, S)];
static CHEMICAL_PLANT: [FluidConnection; 4] = [fc(Input, 1, 0, -1, N), fc(Input, 2, 2, -1, N), fc(Output, 1, 0, 3, S), fc(Output, 2, 2, 3, S)];
static OIL_REFINERY: [FluidConnection; 5] =
    [fc(Input, 1, 1, 5, S), fc(Input, 2, 3, 5, S), fc(Output, 1, 0, -1, N), fc(Output, 2, 2, -1, N), fc(Output, 3, 4, -1, N)];
// Space Age, from the 2.0 prototypes (space-age/prototypes/entity/entities.lua), converted to north-facing
// top-left-relative pipe tiles. All four expose them only while set to a recipe with fluids. VERIFY (v17)
static FOUNDRY: [FluidConnection; 4] = [fc(Input, 1, 1, 5, S), fc(Input, 2, 3, 5, S), fc(Output, 1, 1, -1, N), fc(Output, 2, 3, -1, N)];
static ELECTROMAGNETIC_PLANT: [FluidConnection; 4] = [fc(Input, 1, -1, 2, W), fc(Input, 2, 4, 1, E), fc(Output, 1, 2, 4, S), fc(Output, 2, 1, -1, N)];
static CRYOGENIC_PLANT: [FluidConnection; 6] =
    [fc(Input, 1, 0, 5, S), fc(Input, 2, 2, 5, S), fc(Input, 3, 4, 5, S), fc(Output, 1, 0, -1, N), fc(Output, 2, 2, -1, N), fc(Output, 3, 4, -1, N)];
static BIOCHAMBER: [FluidConnection; 4] = [fc(Input, 1, 0, -1, N), fc(Input, 2, 2, -1, N), fc(Output, 1, 2, 3, S), fc(Output, 2, 0, 3, S)];

/// Fluid connections of base-game machines facing north (prototype fluid_boxes as remembered), and whether
/// they exist only with a fluid recipe. [P] VERIFY (v9-v11, v14, v17)
fn fluid_table(machine: &str) -> Option<(&'static [FluidConnection], bool)> {
    Some(match machine {
        "assembling-machine-2" | "assembling-machine-3" => (&ASSEMBLER_FLUIDS, true),
        "chemical-plant" => (&CHEMICAL_PLANT, false),
        "oil-refinery" => (&OIL_REFINERY, false),
        "foundry" => (&FOUNDRY, true),
        "electromagnetic-plant" => (&ELECTROMAGNETIC_PLANT, true),
        "cryogenic-plant" => (&CRYOGENIC_PLANT, true),
        "biochamber" => (&BIOCHAMBER, true),
        _ => return None,
    })
}

/// Assembling machines expose their connections only while set to a recipe with fluids.
pub fn connections_only_with_fluid_recipe(machine: &str) -> bool {
    fluid_table(machine).is_some_and(|t| t.1)
}

/// Fluid connections of a base machine facing north (empty for machines without fluids).
pub fn base_fluid_connections(machine: &str) -> &'static [FluidConnection] {
    fluid_table(machine).map_or(&[], |t| t.0)
}

/// Footprints of crafting machines, for layouts read without recipes.json (decoded blueprints).
pub const MACHINE_SIZES: [(&str, (i32, i32)); 15] = [
    ("assembling-machine-1", (3, 3)),
    ("assembling-machine-2", (3, 3)),
    ("assembling-machine-3", (3, 3)),
    ("stone-furnace", (2, 2)),
    ("steel-furnace", (2, 2)),
    ("electric-furnace", (3, 3)),
    ("chemical-plant", (3, 3)),
    ("oil-refinery", (5, 5)),
    ("centrifuge", (3, 3)),
    ("foundry", (5, 5)),
    ("electromagnetic-plant", (4, 4)),
    ("cryogenic-plant", (5, 5)),
    ("biochamber", (3, 3)),
    ("crusher", (2, 3)),
    ("recycler", (2, 4)),
];

pub fn machine_size(name: &str) -> Option<(i32, i32)> {
    MACHINE_SIZES.iter().find(|m| m.0 == name).map(|m| m.1)
}

/// Machines whose blueprint entity carries a recipe (furnaces and the recycler pick theirs from their input).
pub fn takes_recipe(name: &str) -> bool {
    !name.ends_with("furnace") && name != "recycler"
}

/// Base-game crafting machines.
pub const BASE_MACHINES: [&str; 9] = [
    "assembling-machine-1",
    "assembling-machine-2",
    "assembling-machine-3",
    "stone-furnace",
    "steel-furnace",
    "electric-furnace",
    "chemical-plant",
    "oil-refinery",
    "centrifuge",
];

/// Space Age crafting machines (the crusher and recycler have no fluid connections).
pub const SPACE_AGE_MACHINES: [&str; 6] = ["foundry", "electromagnetic-plant", "cryogenic-plant", "biochamber", "crusher", "recycler"];

pub fn supported_machines() -> impl Iterator<Item = &'static str> {
    BASE_MACHINES.into_iter().chain(SPACE_AGE_MACHINES)
}

pub fn machine_geom(data: &RootData, name: &str) -> Result<MachineGeom> {
    let e = data.entities.get(name).ok_or_else(|| anyhow!("Unknown machine '{name}'"))?;
    let (connections, only_fluid) = fluid_table(name).unwrap_or((&[], false));
    Ok(MachineGeom {
        name: name.to_owned(),
        w: e.width(),
        h: e.height(),
        connections,
        connections_only_with_fluid_recipe: only_fluid,
        burner: e.is_burner(),
    })
}

/// Where a connection ends up for a machine of size `w` x `h` at rotation `rot` (clockwise quarter turns
/// from north) and optional horizontal mirroring (applied first, in the machine's own frame). [P] VERIFY
pub fn place_connection(c: &FluidConnection, w: i32, h: i32, rot: i32, mirror: bool) -> (i32, i32) {
    // Work in doubled coordinates around the machine centre so odd and even sizes stay exact.
    let mut x = 2 * c.dx + 1 - w;
    let mut y = 2 * c.dy + 1 - h;
    if mirror {
        x = -x;
    }
    for _ in 0..rot.rem_euclid(4) {
        let nx = -y;
        y = x;
        x = nx;
    }
    let (rw, rh) = if rot % 2 == 0 { (w, h) } else { (h, w) };
    ((x + rw - 1) / 2, (y + rh - 1) / 2)
}

pub fn place_side(c: &FluidConnection, rot: i32, mirror: bool) -> Dir {
    let mut d = c.side;
    if mirror && d.axis() == 0 {
        d = d.opposite();
    }
    for _ in 0..rot.rem_euclid(4) {
        d = d.cw();
    }
    d
}
