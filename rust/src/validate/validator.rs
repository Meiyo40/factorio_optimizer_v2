use super::max_flow::MaxFlow;
use crate::data::data_model::RootData;
use crate::data::game_constants as gc;
use crate::geom::{Dir, Kind, Layout, PlacedEntity, Rect, Tile};
use crate::place::connections_of;
use crate::util::java_hash::{JMap, JSet};
use crate::util::sum_f64;
use crate::util::text::{fmt_list, fmt2};
use indexmap::{IndexMap, IndexSet};
use serde::Deserialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorType {
    Overlap,
    OutOfBounds,
    UnknownRecipe,
    Contamination,
    ShapeChanged,
    Unsupported,
    UgUnpaired,
    Unreachable,
    Starved,
    MissingInput,
    MissingSink,
    InserterBadPickup,
    InserterBadDrop,
    InserterWrongItem,
    InserterRate,
    BeltOverCapacity,
    Unpowered,
    PolesDisconnected,
    WireTooLong,
    FluidMissing,
    FluidBlocked,
    FluidMix,
    FluidStuck,
    FluidLeak,
    OutputMissing,
}

impl ErrorType {
    pub fn name(self) -> &'static str {
        use ErrorType::*;
        match self {
            Overlap => "OVERLAP",
            OutOfBounds => "OUT_OF_BOUNDS",
            UnknownRecipe => "UNKNOWN_RECIPE",
            Contamination => "CONTAMINATION",
            ShapeChanged => "SHAPE_CHANGED",
            Unsupported => "UNSUPPORTED",
            UgUnpaired => "UG_UNPAIRED",
            Unreachable => "UNREACHABLE",
            Starved => "STARVED",
            MissingInput => "MISSING_INPUT",
            MissingSink => "MISSING_SINK",
            InserterBadPickup => "INSERTER_BAD_PICKUP",
            InserterBadDrop => "INSERTER_BAD_DROP",
            InserterWrongItem => "INSERTER_WRONG_ITEM",
            InserterRate => "INSERTER_RATE",
            BeltOverCapacity => "BELT_OVER_CAPACITY",
            Unpowered => "UNPOWERED",
            PolesDisconnected => "POLES_DISCONNECTED",
            WireTooLong => "WIRE_TOO_LONG",
            FluidMissing => "FLUID_MISSING",
            FluidBlocked => "FLUID_BLOCKED",
            FluidMix => "FLUID_MIX",
            FluidStuck => "FLUID_STUCK",
            FluidLeak => "FLUID_LEAK",
            OutputMissing => "OUTPUT_MISSING",
        }
    }
}

impl fmt::Display for ErrorType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ValidationError {
    pub kind: ErrorType,
    pub tile: Tile,
    pub message: String,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at {}: {}", self.kind, self.tile, self.message)
    }
}

/// What a machine is supposed to do, for item and rate checks. Rates are items/s.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct MachineSpec {
    pub product: String,
    pub ingredients: HashSet<String>,
    pub output_rate: f64,
    pub sink_rate: f64,
    pub demand: IndexMap<String, f64>,
    pub needs_fluid: Option<String>,
    /// Fluid -> 1-based input / output connection the recipe uses.
    pub fluid_in: IndexMap<String, i32>,
    pub fluid_out: IndexMap<String, i32>,
    /// Item products and their rates when there are several (asteroid crushing, uranium processing).
    pub products: IndexMap<String, f64>,
    /// Of `products`, what goes into sink chests.
    pub sinks: IndexMap<String, f64>,
}

impl MachineSpec {
    /// Item products leaving the machine, with their rates.
    pub fn outputs(&self) -> IndexMap<String, f64> {
        if !self.products.is_empty() {
            self.products.clone()
        } else if self.output_rate > 0.0 {
            IndexMap::from([(self.product.clone(), self.output_rate)])
        } else {
            IndexMap::new()
        }
    }

    pub fn sink_of(&self, item: &str) -> f64 {
        if self.products.is_empty() {
            if item == self.product { self.sink_rate } else { 0.0 }
        } else {
            self.sinks.get(item).copied().unwrap_or(0.0)
        }
    }

    /// Whether an inserter taking from this machine needs a filter (it would take every product).
    pub fn multi_product(&self) -> bool {
        self.products.len() > 1
    }
}

/// Items entering the layout on a belt tile (or a fluid on a pipe tile) from outside it (ports).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SourceSpec {
    pub tile: Tile,
    pub item: String,
    pub rate: f64,
    #[serde(default)]
    pub fluid: bool,
    /// Lane the item arrives on (0 left, 1 right of the first belt's heading), or -1 for both.
    #[serde(default = "minus_one")]
    pub lane: i32,
    /// Plan port it belongs to (-1: unknown).
    #[serde(default = "minus_one")]
    pub port: i32,
}

fn minus_one() -> i32 {
    -1
}

/// Items (or a fluid) leaving the layout from a belt (pipe) on this tile at this rate (output ports).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct OutputSpec {
    pub tile: Tile,
    pub item: String,
    pub rate: f64,
    #[serde(default)]
    pub fluid: bool,
    #[serde(default = "minus_one")]
    pub port: i32,
}

pub type InserterRateFn = Arc<dyn Fn(&str, bool) -> f64 + Send + Sync>;
pub type RecipeInfoFn = Arc<dyn Fn(&str) -> Option<MachineSpec> + Send + Sync>;

#[derive(Clone)]
pub struct ValidationSpec {
    /// Machine top-left tile -> what it should do. Machines missing here get item checks from their recipe only.
    pub machines: HashMap<Tile, MachineSpec>,
    pub sources: Vec<SourceSpec>,
    pub outputs: Vec<OutputSpec>,
    /// Ring around the box where poles may stand.
    pub pole_ring: i32,
    /// Tiles outside the box that may hold anything (ports, test harness).
    pub outside_allowed: HashSet<Tile>,
    pub require_power: bool,
    /// "infinity-pipe" or "manual".
    pub fluid_mode: String,
    pub check_rates: bool,
    /// Effective items/s of an inserter by name, picking from a belt or not.
    pub inserter_rate: InserterRateFn,
    /// recipe -> (product, solid ingredients, fluid ingredient) for machines without a `MachineSpec`.
    pub recipe_info: RecipeInfoFn,
}

impl Default for ValidationSpec {
    fn default() -> Self {
        Self {
            machines: HashMap::new(),
            sources: Vec::new(),
            outputs: Vec::new(),
            pole_ring: 0,
            outside_allowed: HashSet::new(),
            require_power: true,
            fluid_mode: "infinity-pipe".to_owned(),
            check_rates: true,
            inserter_rate: Arc::new(|name, from_belt| {
                gc::inserter(name).expect("a known inserter").base_rate * if from_belt { 0.7 } else { 1.0 }
            }),
            recipe_info: Arc::new(|_| None),
        }
    }
}

/// Recipe facts for machines that have no `MachineSpec` (item checks only, no rates).
pub fn recipe_info_from(data: &RootData) -> RecipeInfoFn {
    let table: HashMap<String, MachineSpec> = data
        .recipes
        .iter()
        .map(|(name, r)| {
            let fluid_ingredients: Vec<_> = r.ingredients.iter().filter(|i| i.is_fluid()).collect();
            let fluid_products: Vec<_> = r.products.iter().filter(|p| p.r#type == "fluid").collect();
            let spec = MachineSpec {
                product: r
                    .products
                    .iter()
                    .find(|p| p.r#type != "fluid")
                    .map(|p| p.name.clone())
                    .or_else(|| r.main_product.as_ref().map(|p| p.name.clone()))
                    .unwrap_or_else(|| r.name.clone()),
                ingredients: r.ingredients.iter().filter(|i| !i.is_fluid()).map(|i| i.name.clone()).collect(),
                needs_fluid: fluid_ingredients.first().map(|i| i.name.clone()),
                fluid_in: fluid_ingredients.iter().enumerate().map(|(i, ing)| (ing.name.clone(), ing.fluidbox_index.unwrap_or(i as i32 + 1))).collect(),
                fluid_out: fluid_products.iter().enumerate().map(|(i, p)| (p.name.clone(), p.fluidbox_index.unwrap_or(i as i32 + 1))).collect(),
                ..Default::default()
            };
            (name.clone(), spec)
        })
        .collect();
    Arc::new(move |name| table.get(name).cloned())
}

/// Kotlin prints a null in a string template as `null`.
fn nul<T: fmt::Display>(v: &Option<T>) -> String {
    v.as_ref().map_or_else(|| "null".to_owned(), |x| x.to_string())
}

fn sorted_list<'a>(items: impl IntoIterator<Item = &'a String>) -> String {
    let mut v: Vec<&String> = items.into_iter().collect();
    v.sort();
    fmt_list(v)
}

fn add_all(dst: &mut IndexSet<String>, src: &IndexSet<String>) -> bool {
    let mut changed = false;
    for s in src {
        changed |= dst.insert(s.clone());
    }
    changed
}

/// Does belt-family `f` take items pushed into it travelling in direction `d`?
fn accepts(f: &PlacedEntity, d: Dir) -> bool {
    match f.kind {
        Kind::Belt | Kind::UgIn => d != f.dir.opposite(),
        Kind::UgOut => d.axis() != f.dir.axis(), // sides only; its back is underground, its front faces away
        Kind::Splitter => d == f.dir,
        _ => false,
    }
}

type Lanes = [IndexSet<String>; 2];

fn lanes(m: &mut JMap<usize, Lanes>, i: usize) -> &mut Lanes {
    m.get_or_put_with(i, || [IndexSet::new(), IndexSet::new()])
}

struct BeltGraph {
    /// entity index -> successor entity indices (two for a splitter)
    next: JMap<usize, Vec<usize>>,
    prev: JMap<usize, Vec<usize>>,
    /// items present on each belt-family entity
    items: HashMap<usize, IndexSet<String>>,
    /// items on each lane of a belt-family entity (0 left, 1 right of its heading), and injected per lane
    lane_items: JMap<usize, Lanes>,
    injected_lane: JMap<usize, Lanes>,
}

impl BeltGraph {
    /// Puts `item` onto entity `i`: on `lane` (0 left, 1 right), or both lanes for -1.
    fn inject(&mut self, i: usize, item: &str, lane: i32) {
        let l = lanes(&mut self.injected_lane, i);
        if lane != 1 {
            l[0].insert(item.to_owned());
        }
        if lane != 0 {
            l[1].insert(item.to_owned());
        }
    }

    /// Lanes of entity `i` that carry `item` (bit 0 left, bit 1 right).
    fn lanes_with(&self, i: usize, item: &str) -> i32 {
        let Some(l) = self.lane_items.get(&i) else { return 0 };
        i32::from(l[0].contains(item)) | (i32::from(l[1].contains(item)) << 1)
    }

    /// The single item on a belt entity, if exactly one.
    fn item_on(&self, i: usize) -> Option<&String> {
        let set = self.items.get(&i)?;
        if set.len() == 1 { set.first() } else { None }
    }
}

struct Draw {
    item: String,
    quota: f64,
    pickups: Vec<usize>,
    left: f64,
}

/// Demand below a belt, memoised; the guards keep a cyclic belt graph from recursing forever.
struct DemandBelow<'a> {
    est: &'a HashMap<usize, JMap<String, f64>>,
    succs: &'a JMap<usize, Vec<usize>>,
    below: HashMap<usize, f64>,
    below_item: HashMap<(usize, String), f64>,
}

impl DemandBelow<'_> {
    fn est_at(&self, i: usize) -> f64 {
        self.est.get(&i).map_or(0.0, |m| sum_f64(m.values_java().copied()))
    }

    fn any(&mut self, i: usize, depth: i32) -> f64 {
        if let Some(&v) = self.below.get(&i) {
            return v;
        }
        let here = self.est_at(i);
        self.below.insert(i, here); // guards cycles
        let mut downstream = 0.0;
        if depth <= 4000 {
            let succs: Vec<usize> = self.succs.get(&i).cloned().unwrap_or_default();
            for n in succs {
                downstream += self.any(n, depth + 1);
            }
        }
        let v = here + if depth > 4000 { 0.0 } else { downstream };
        self.below.insert(i, v);
        v
    }

    fn item(&mut self, i: usize, item: &str, depth: i32) -> f64 {
        if let Some(&v) = self.below_item.get(&(i, item.to_owned())) {
            return v;
        }
        let here = self.est.get(&i).and_then(|m| m.get(&item.to_owned())).copied().unwrap_or(0.0);
        self.below_item.insert((i, item.to_owned()), here);
        let mut downstream = 0.0;
        if depth <= 4000 {
            let succs: Vec<usize> = self.succs.get(&i).cloned().unwrap_or_default();
            for n in succs {
                downstream += self.item(n, item, depth + 1);
            }
        }
        let v = here + if depth > 4000 { 0.0 } else { downstream };
        self.below_item.insert((i, item.to_owned()), v);
        v
    }
}

/// Re-derives belts, items, rates, power and fluids from the entity list alone, so a router bug cannot
/// hide itself. Net ids on entities are used only in messages.
pub struct Validator<'a> {
    layout: &'a Layout,
    spec: &'a ValidationSpec,
    errors: Vec<ValidationError>,
    at: HashMap<Tile, usize>,
    /// What each machine entity is supposed to do (None for every other entity).
    infos: Vec<Option<MachineSpec>>,
}

pub fn validate(layout: &Layout, spec: &ValidationSpec) -> Vec<ValidationError> {
    Validator::new(layout, spec).run()
}

impl<'a> Validator<'a> {
    pub fn new(layout: &'a Layout, spec: &'a ValidationSpec) -> Self {
        let infos = layout
            .entities
            .iter()
            .map(|e| {
                if e.kind != Kind::Machine {
                    return None;
                }
                spec.machines.get(&e.tile()).cloned().or_else(|| e.recipe.as_ref().and_then(|r| (spec.recipe_info)(r)))
            })
            .collect();
        Self { layout, spec, errors: Vec::new(), at: HashMap::new(), infos }
    }

    fn err(&mut self, kind: ErrorType, tile: Tile, message: String) {
        self.errors.push(ValidationError { kind, tile, message });
    }

    pub fn run(mut self) -> Vec<ValidationError> {
        self.check_overlap_and_bounds();
        self.check_machines();
        let pairs = self.pair_undergrounds();
        let mut belts = self.belt_graph(pairs);
        self.check_inserter_ends();
        self.propagate_items(&mut belts);
        self.check_inserter_items(&belts);
        self.check_outputs(&belts);
        if self.spec.check_rates {
            self.check_rates(&mut belts);
        } else {
            self.check_reachability_only(&belts);
        }
        self.check_power();
        self.check_fluids();
        let mut seen = HashSet::new();
        self.errors.retain(|e| seen.insert(e.clone()));
        self.errors
    }

    fn ents(&self) -> &'a [PlacedEntity] {
        &self.layout.entities
    }

    fn index_at(&self, t: Tile) -> Option<usize> {
        self.at.get(&t).copied()
    }

    fn entity_at(&self, t: Tile) -> Option<&'a PlacedEntity> {
        self.index_at(t).map(|i| &self.layout.entities[i])
    }

    fn machine_info(&self, idx: usize) -> Option<&MachineSpec> {
        self.infos[idx].as_ref()
    }

    // ---- 1. overlap and bounds ----
    fn check_overlap_and_bounds(&mut self) {
        let (layout, spec) = (self.layout, self.spec);
        let ents = &layout.entities;
        let bounds = Rect::new(0, 0, layout.w, layout.h);
        for (i, e) in ents.iter().enumerate() {
            for t in e.rect().tiles() {
                if let Some(prev) = self.at.insert(t, i) {
                    self.err(ErrorType::Overlap, t, format!("{} and {} share a tile", ents[prev].name, e.name));
                }
                let inside = bounds.contains_tile(t);
                let ring = e.kind == Kind::Pole
                    && spec.pole_ring > 0
                    && t.x >= -spec.pole_ring
                    && t.y >= -spec.pole_ring
                    && t.x < layout.w + spec.pole_ring
                    && t.y < layout.h + spec.pole_ring;
                if !inside && !ring && !spec.outside_allowed.contains(&t) {
                    self.err(ErrorType::OutOfBounds, t, format!("{} lies outside the box", e.name));
                }
            }
        }
    }

    fn check_machines(&mut self) {
        for (i, m) in self.ents().iter().enumerate() {
            if m.kind != Kind::Machine {
                continue;
            }
            if m.recipe.is_none() || self.machine_info(i).is_none() {
                self.err(ErrorType::UnknownRecipe, m.tile(), format!("machine without a known recipe ({})", nul(&m.recipe)));
            }
        }
    }

    // ---- 3. underground pairing, simulated with the game's nearest-same-axis rule ----
    /// entrance index -> exit index
    fn pair_undergrounds(&mut self) -> HashMap<usize, usize> {
        let ents = self.ents();
        let mut pairs = HashMap::new();
        let mut paired_exits = HashSet::new();
        for (i, e) in ents.iter().enumerate() {
            if e.kind != Kind::UgIn {
                continue;
            }
            let tier = gc::tier_of_entity(&e.name).expect("an underground entrance has a belt tier");
            let mut found = None;
            for j in 1..=tier.max_underground_gap + 1 {
                let Some(k) = self.index_at(e.tile().step(e.dir, j)) else { continue };
                let o = &ents[k];
                if o.name != e.name || o.dir.axis() != e.dir.axis() {
                    continue;
                }
                if o.kind == Kind::UgOut && o.dir == e.dir {
                    found = Some(k);
                }
                break;
            }
            match found {
                None => self.err(ErrorType::UgUnpaired, e.tile(), format!("underground entrance has no exit within reach (net {})", nul(&e.net))),
                Some(k) => {
                    pairs.insert(i, k);
                    paired_exits.insert(k);
                }
            }
        }
        for (i, e) in ents.iter().enumerate() {
            if e.kind == Kind::UgOut && !paired_exits.contains(&i) {
                self.err(ErrorType::UgUnpaired, e.tile(), format!("underground exit has no entrance (net {})", nul(&e.net)));
            }
        }
        pairs
    }

    // ---- 2. belt graph ----
    fn belt_graph(&self, ug_pairs: HashMap<usize, usize>) -> BeltGraph {
        let ents = self.ents();
        let mut g = BeltGraph {
            next: JMap::new(),
            prev: JMap::new(),
            items: HashMap::new(),
            lane_items: JMap::new(),
            injected_lane: JMap::new(),
        };
        for (i, e) in ents.iter().enumerate() {
            if !e.kind.belt_family() {
                continue;
            }
            let succs: Vec<usize> = match e.kind {
                Kind::UgIn => ug_pairs.get(&i).copied().into_iter().collect(),
                Kind::Splitter => {
                    let mut v: Vec<usize> = Vec::new();
                    for t in e.rect().tiles() {
                        if let Some(k) = self.index_at(t + e.dir).filter(|&k| k != i && accepts(&ents[k], e.dir))
                            && !v.contains(&k)
                        {
                            v.push(k);
                        }
                    }
                    v
                }
                _ => e.output_tile().and_then(|t| self.index_at(t)).filter(|&k| accepts(&ents[k], e.dir)).into_iter().collect(),
            };
            for succ in succs {
                g.next.get_or_put_with(i, Vec::new).push(succ);
                g.prev.get_or_put_with(succ, Vec::new).push(i);
            }
        }
        g
    }

    /// Items arriving on `lane` of entity `i`: from its feeders, and injected there.
    fn arriving(&self, belts: &BeltGraph, i: usize, lane: usize) -> Vec<IndexSet<String>> {
        let ents = self.ents();
        let mut out: Vec<IndexSet<String>> = Vec::new();
        for &p in belts.prev.get(&i).map_or(&[][..], |v| v.as_slice()) {
            let (to_l, to_r) = self.lane_into(&ents[p], &ents[i], belts);
            let mut set: IndexSet<String> = IndexSet::new();
            if let Some(pl) = belts.lane_items.get(&p) {
                if to_l == lane {
                    add_all(&mut set, &pl[0]);
                }
                if to_r == lane {
                    add_all(&mut set, &pl[1]);
                }
            }
            out.push(set);
        }
        out.push(belts.injected_lane.get(&i).map(|l| l[lane].clone()).unwrap_or_default());
        out
    }

    fn propagate_items(&mut self, belts: &mut BeltGraph) {
        let (spec, ents) = (self.spec, self.ents());
        for s in spec.sources.iter().filter(|s| !s.fluid) {
            match self.index_at(s.tile).filter(|&i| ents[i].kind.belt_family()) {
                None => self.err(ErrorType::Unreachable, s.tile, format!("source of {} has no belt", s.item)),
                Some(i) => belts.inject(i, &s.item, s.lane),
            }
        }
        for e in ents.iter().filter(|e| e.kind == Kind::Inserter) {
            let Some(d) = self.index_at(e.drop_tile()) else { continue };
            if ents[d].kind != Kind::Belt {
                continue;
            }
            if let Some(item) = self.carried_by_inserter(e, None) {
                let lane = self.drop_lane(e, d, belts);
                belts.inject(d, &item, lane);
            }
        }
        // Fixpoint: items flow downstream along next, lane by lane.
        let mut queue: VecDeque<usize> = VecDeque::new();
        let injected: Vec<(usize, Lanes)> = belts.injected_lane.iter_java().map(|(i, l)| (*i, l.clone())).collect();
        for (i, l) in &injected {
            let t = lanes(&mut belts.lane_items, *i);
            add_all(&mut t[0], &l[0]);
            add_all(&mut t[1], &l[1]);
            queue.push_back(*i);
        }
        while let Some(i) = queue.pop_front() {
            let src = belts.lane_items.get(&i).expect("a queued belt has lanes").clone();
            let nexts: Vec<usize> = belts.next.get(&i).cloned().unwrap_or_default();
            for n in nexts {
                let (to_l, to_r) = self.lane_into(&ents[i], &ents[n], belts);
                let t = lanes(&mut belts.lane_items, n);
                let a = add_all(&mut t[to_l], &src[0]);
                let b = add_all(&mut t[to_r], &src[1]);
                if a || b {
                    queue.push_back(n);
                }
            }
        }
        let flat: Vec<(usize, IndexSet<String>)> = belts
            .lane_items
            .iter_java()
            .map(|(i, l)| {
                let mut all = l[0].clone();
                add_all(&mut all, &l[1]);
                (*i, all)
            })
            .collect();
        for (i, all) in flat {
            belts.items.insert(i, all);
        }
        // An inserter putting its item onto a lane that already carries something else.
        for e in ents.iter().filter(|e| e.kind == Kind::Inserter) {
            let Some(d) = self.index_at(e.drop_tile()) else { continue };
            if ents[d].kind != Kind::Belt {
                continue;
            }
            let Some(mine) = self.carried_by_inserter(e, Some(belts)) else { continue };
            let lane = self.drop_lane(e, d, belts) as usize;
            let mut there: IndexSet<String> = IndexSet::new();
            for s in self.arriving(belts, d, lane) {
                add_all(&mut there, &s);
            }
            if there.iter().any(|it| *it != mine) {
                self.err(ErrorType::InserterWrongItem, e.tile(), format!("{} drops {} onto a lane of {}", e.name, mine, sorted_list(&there)));
            }
        }
        self.check_shapes(belts);
        // Report merge points: a lane carrying several items where no single input already did. Two items
        // on the two lanes of one belt are fine (lanes move independently).
        let merged: Vec<(usize, Lanes)> = belts.lane_items.iter_java().map(|(i, l)| (*i, l.clone())).collect();
        for (i, l) in merged {
            for lane in 0..2 {
                let set = &l[lane];
                if set.len() <= 1 {
                    continue;
                }
                let injected_here = belts.injected_lane.get(&i).map_or(0, |il| il[lane].len());
                if injected_here > 1 || self.arriving(belts, i, lane).iter().all(|a| !set.iter().all(|s| a.contains(s))) {
                    self.err(
                        ErrorType::Contamination,
                        ents[i].tile(),
                        format!("items {} meet on one lane (net {})", sorted_list(set), nul(&ents[i].net)),
                    );
                }
            }
        }
    }

    /// Belt shape follows its feeders: a rear feeder makes it straight; else exactly one side feeder makes
    /// it a curve. When net ids are known, report belts whose shape a feeder from another net changes.
    fn check_shapes(&mut self, belts: &BeltGraph) {
        let ents = self.ents();
        fn shape(e: &PlacedEntity, feeders: &[&PlacedEntity]) -> String {
            let dirs: Vec<Dir> = feeders.iter().map(|f| f.dir).collect();
            if dirs.contains(&e.dir) {
                "straight".to_owned()
            } else if dirs.len() == 1 {
                format!("curve-{}", dirs[0].name())
            } else {
                "straight".to_owned()
            }
        }
        let all: Vec<(usize, Vec<usize>)> = belts.prev.iter_java().map(|(i, f)| (*i, f.clone())).collect();
        for (i, feeders) in all {
            let e = &ents[i];
            if e.kind != Kind::Belt || e.net.is_none() || feeders.iter().any(|&f| ents[f].net.is_none()) {
                continue;
            }
            let own: Vec<&PlacedEntity> = feeders.iter().map(|&f| &ents[f]).filter(|f| f.net == e.net).collect();
            if own.len() == feeders.len() {
                continue;
            }
            let all_feeders: Vec<&PlacedEntity> = feeders.iter().map(|&f| &ents[f]).collect();
            if shape(e, &own) != shape(e, &all_feeders) {
                let other = all_feeders.iter().find(|f| f.net != e.net).expect("a feeder of another net");
                self.err(
                    ErrorType::ShapeChanged,
                    e.tile(),
                    format!("a belt of net {} changes the shape of net {}'s belt", nul(&other.net), nul(&e.net)),
                );
            }
        }
    }

    /// What an inserter moves, judged from its pickup tile alone.
    fn carried_by_inserter(&self, ins: &PlacedEntity, belts: Option<&BeltGraph>) -> Option<String> {
        let p = self.index_at(ins.pickup_tile())?;
        let src = &self.ents()[p];
        match src.kind {
            Kind::Machine => self.machine_info(p).map(|mi| ins.item.clone().unwrap_or_else(|| mi.product.clone())),
            Kind::InfinityChest | Kind::Chest => src.item.clone(),
            Kind::Belt => belts.and_then(|b| b.item_on(p)).cloned(),
            _ => None,
        }
    }

    /// What an inserter takes out of a machine: its filter, else the machine's product.
    fn machine_output(&self, ins: usize) -> Option<String> {
        let ents = self.ents();
        let p = self.index_at(ents[ins].pickup_tile())?;
        if ents[p].kind != Kind::Machine {
            return None;
        }
        self.machine_info(p).map(|mi| ents[ins].item.clone().unwrap_or_else(|| mi.product.clone()))
    }

    // ---- 5. inserter ends ----
    fn check_inserter_ends(&mut self) {
        let ok_end = |e: Option<&PlacedEntity>| e.is_some_and(|e| matches!(e.kind, Kind::Belt | Kind::Machine | Kind::Chest | Kind::InfinityChest));
        for ins in self.ents().iter().filter(|e| e.kind == Kind::Inserter) {
            let pick = self.entity_at(ins.pickup_tile());
            let drop = self.entity_at(ins.drop_tile());
            if !ok_end(pick) {
                self.err(
                    ErrorType::InserterBadPickup,
                    ins.tile(),
                    format!("{} picks from {} at {}", ins.name, pick.map_or("nothing", |p| p.name.as_str()), ins.pickup_tile()),
                );
            }
            if !ok_end(drop) {
                self.err(
                    ErrorType::InserterBadDrop,
                    ins.tile(),
                    format!("{} drops into {} at {}", ins.name, drop.map_or("nothing", |p| p.name.as_str()), ins.drop_tile()),
                );
            }
            // Out of a machine with several products, an unfiltered inserter takes all of them.
            let Some(pick) = pick.filter(|p| p.kind == Kind::Machine) else { continue };
            let pick_idx = self.index_at(ins.pickup_tile()).expect("the pickup entity exists");
            let Some(mi) = self.machine_info(pick_idx).cloned() else { continue };
            let outputs = mi.outputs();
            if ins.item.is_none() && mi.multi_product() {
                self.err(
                    ErrorType::InserterWrongItem,
                    ins.tile(),
                    format!("{} takes every product of {} ({}); it needs a filter", ins.name, nul(&pick.recipe), sorted_list(outputs.keys())),
                );
            }
            if let Some(item) = &ins.item
                && !outputs.contains_key(item)
            {
                self.err(ErrorType::InserterWrongItem, ins.tile(), format!("{} is filtered to {}, which {} does not make", ins.name, item, nul(&pick.recipe)));
            }
        }
    }

    fn check_inserter_items(&mut self, belts: &BeltGraph) {
        let ents = self.ents();
        for ins in ents.iter().filter(|e| e.kind == Kind::Inserter) {
            let Some(pick_idx) = self.index_at(ins.pickup_tile()) else { continue };
            let Some(drop_idx) = self.index_at(ins.drop_tile()) else { continue };
            let (pick, drop) = (&ents[pick_idx], &ents[drop_idx]);
            let item = self.carried_by_inserter(ins, Some(belts));
            match drop.kind {
                Kind::Machine => {
                    let Some(info) = self.machine_info(drop_idx).cloned() else { continue };
                    if pick.kind == Kind::Belt {
                        let on_belt = belts.items.get(&pick_idx);
                        if let Some(on_belt) = on_belt.filter(|s| !s.is_empty() && s.iter().all(|i| !info.ingredients.contains(i))) {
                            self.err(
                                ErrorType::InserterWrongItem,
                                ins.tile(),
                                format!("{} feeds {} from a belt of {}", ins.name, nul(&drop.recipe), sorted_list(on_belt)),
                            );
                        }
                    } else if let Some(item) = item.filter(|i| !info.ingredients.contains(i)) {
                        self.err(ErrorType::InserterWrongItem, ins.tile(), format!("{} moves {} into {}, which does not use it", ins.name, item, nul(&drop.recipe)));
                    }
                }
                Kind::Chest if pick.kind != Kind::Machine => {
                    self.err(ErrorType::InserterWrongItem, ins.tile(), format!("chest is fed from {}, not a machine", pick.name));
                }
                _ => {}
            }
        }
    }

    // ---- 4 and 6. reachability, prefix condition and rates ----
    fn belt_capacity(e: &PlacedEntity) -> f64 {
        gc::tier_of_entity(&e.name).map_or(f64::MAX, |t| t.throughput)
    }

    fn check_reachability_only(&mut self, belts: &BeltGraph) {
        let ents = self.ents();
        for ins in ents.iter().filter(|e| e.kind == Kind::Inserter) {
            let Some(p) = self.index_at(ins.pickup_tile()) else { continue };
            if ents[p].kind == Kind::Belt && belts.items.get(&p).is_none_or(|s| s.is_empty()) {
                self.err(ErrorType::Unreachable, ins.pickup_tile(), format!("{} picks from a belt no item reaches", ins.name));
            }
        }
    }

    fn pick_idx(&self, i: usize) -> Option<usize> {
        self.index_at(self.ents()[i].pickup_tile())
    }

    fn drop_idx(&self, i: usize) -> Option<usize> {
        self.index_at(self.ents()[i].drop_tile())
    }

    fn picks_from_belt(&self, i: usize) -> bool {
        self.pick_idx(i).is_some_and(|p| self.ents()[p].kind == Kind::Belt)
    }

    fn cap(&self, i: usize) -> f64 {
        (self.spec.inserter_rate)(&self.ents()[i].name, self.picks_from_belt(i))
    }

    fn feeder_is_machine_of(&self, ins: usize, item: &str) -> bool {
        let Some(p) = self.pick_idx(ins) else { return false };
        self.ents()[p].kind == Kind::Machine && self.machine_output(ins).as_deref() == Some(item)
    }

    fn feeder_carries(&self, ins: usize, item: &str, belts: &BeltGraph) -> bool {
        let Some(p) = self.pick_idx(ins) else { return false };
        let src = &self.ents()[p];
        match src.kind {
            Kind::Belt => belts.items.get(&p).is_some_and(|s| s.contains(item)),
            Kind::Machine => self.machine_output(ins).as_deref() == Some(item),
            Kind::Chest | Kind::InfinityChest => src.item.as_deref() == Some(item),
            _ => false,
        }
    }

    fn check_rates(&mut self, belts: &mut BeltGraph) {
        self.check_reachability_only(belts);
        let ents = self.ents();
        let spec = self.spec;
        let inserters: Vec<usize> = (0..ents.len()).filter(|&i| ents[i].kind == Kind::Inserter).collect();
        let machines: Vec<usize> = (0..ents.len()).filter(|&i| ents[i].kind == Kind::Machine).collect();
        let mut out_ins: HashMap<usize, Vec<usize>> = machines.iter().map(|&m| (m, Vec::new())).collect();
        let mut in_ins: HashMap<usize, Vec<usize>> = machines.iter().map(|&m| (m, Vec::new())).collect();
        for &i in &inserters {
            if let Some(list) = self.pick_idx(i).and_then(|p| out_ins.get_mut(&p)) {
                list.push(i);
            }
            if let Some(list) = self.drop_idx(i).and_then(|d| in_ins.get_mut(&d)) {
                list.push(i);
            }
        }

        // Output and input inserter capacity per machine; sinks present.
        for &m in &machines {
            let Some(mi) = self.machine_info(m).cloned() else { continue };
            let e = &ents[m];
            let outputs = mi.outputs();
            for (item, &rate) in &outputs {
                let out_cap = sum_f64(out_ins[&m].iter().filter(|&&i| self.machine_output(i).as_deref() == Some(item)).map(|&i| self.cap(i)));
                if rate > 1e-9 && out_cap + 1e-9 < rate {
                    let what = if outputs.len() > 1 { format!("{item} ") } else { String::new() };
                    self.err(
                        ErrorType::InserterRate,
                        e.tile(),
                        format!("{} makes {}/s {}but its output inserters move {}/s", nul(&e.recipe), fmt2(rate), what, fmt2(out_cap)),
                    );
                }
                let has_chest = out_ins[&m].iter().any(|&i| {
                    self.machine_output(i).as_deref() == Some(item) && self.drop_idx(i).is_some_and(|d| ents[d].kind == Kind::Chest)
                });
                if mi.sink_of(item) > 1e-9 && !has_chest {
                    self.err(ErrorType::MissingSink, e.tile(), format!("{} makes {} for the sink but has no output chest for it", nul(&e.recipe), item));
                }
            }
            // Input inserters against demand. An inserter taking from a shared belt moves whichever of its items
            // the machine is short of, so capacity goes where it is needed: max flow inserter -> item -> demand.
            let feeders = &in_ins[&m];
            let needs: Vec<(String, f64)> = mi.demand.iter().map(|(k, v)| (k.clone(), *v)).collect();
            let mut g = MaxFlow::new(2 + feeders.len() + needs.len());
            for (k, &ins) in feeders.iter().enumerate() {
                g.edge(0, 2 + k, self.cap(ins));
            }
            for (j, (item, _)) in needs.iter().enumerate() {
                for (k, &ins) in feeders.iter().enumerate() {
                    if self.feeder_carries(ins, item, belts) {
                        g.edge(2 + k, 2 + feeders.len() + j, self.cap(ins));
                    }
                }
            }
            let demand_edge: Vec<usize> = needs.iter().enumerate().map(|(j, (_, need))| g.edge(2 + feeders.len() + j, 1, *need)).collect();
            g.run(0, 1);
            for (j, (item, need)) in needs.iter().enumerate() {
                if !feeders.iter().any(|&f| self.feeder_carries(f, item, belts)) {
                    self.err(ErrorType::MissingInput, e.tile(), format!("{} gets no {}", nul(&e.recipe), item));
                } else {
                    let c = g.flow_on(demand_edge[j]);
                    if c + 1e-9 < *need {
                        self.err(ErrorType::InserterRate, e.tile(), format!("{} needs {}/s of {}, its inserters move {}/s", nul(&e.recipe), fmt2(*need), item, fmt2(c)));
                    }
                }
            }
        }

        // Direct insertion: delivered = min(capacity, consumer share), scaled down to the producer's spare output.
        let mut delivered: HashMap<usize, f64> = HashMap::new();
        let mut groups: IndexMap<(usize, String), Vec<usize>> = IndexMap::new();
        for &i in &inserters {
            let both = self.pick_idx(i).is_some_and(|p| ents[p].kind == Kind::Machine) && self.drop_idx(i).is_some_and(|d| ents[d].kind == Kind::Machine);
            if both {
                let key = (self.pick_idx(i).expect("a machine pickup"), self.machine_output(i).expect("a machine output"));
                groups.entry(key).or_default().push(i);
            }
        }
        for ((p, item), list) in &groups {
            let Some(pi) = self.machine_info(*p) else { continue };
            let want: Vec<(usize, f64)> = list
                .iter()
                .map(|&i| {
                    let m = self.drop_idx(i).expect("a machine drop");
                    let need = self.machine_info(m).and_then(|mi| mi.demand.get(item)).copied().unwrap_or(0.0);
                    let n_di = in_ins[&m].iter().filter(|&&f| self.feeder_is_machine_of(f, item)).count();
                    (i, self.cap(i).min(need / n_di.max(1) as f64))
                })
                .collect();
            let spare = (pi.outputs().get(item).copied().unwrap_or(0.0) - pi.sink_of(item)).max(0.0);
            let total = sum_f64(want.iter().map(|w| w.1));
            let scale = if total > spare && total > 0.0 { spare / total } else { 1.0 };
            for (i, w) in want {
                delivered.insert(i, w * scale);
            }
        }

        // Producer outputs onto belts, after direct insertion.
        let mut belt_rate: JMap<usize, f64> = JMap::new(); // inserter -> items/s dropped onto its belt
        let mut belt_out: JMap<(usize, String), f64> = JMap::new();
        let mut to_belt: JMap<(usize, String), Vec<usize>> = JMap::new();
        for &m in &machines {
            let Some(mi) = self.machine_info(m) else { continue };
            for (item, &rate) in &mi.outputs() {
                let mine: Vec<usize> = out_ins[&m].iter().copied().filter(|&i| self.machine_output(i).as_deref() == Some(item)).collect();
                to_belt.insert((m, item.clone()), mine.iter().copied().filter(|&i| self.drop_idx(i).is_some_and(|d| ents[d].kind == Kind::Belt)).collect());
                let di_out = sum_f64(mine.iter().map(|i| delivered.get(i).copied().unwrap_or(0.0)));
                belt_out.insert((m, item.clone()), (rate - mi.sink_of(item) - di_out).max(0.0));
            }
        }
        for &i in &inserters {
            if self.pick_idx(i).is_some_and(|p| ents[p].kind == Kind::InfinityChest) && self.drop_idx(i).is_some_and(|d| ents[d].kind == Kind::Belt) {
                belt_rate.insert(i, self.cap(i));
            }
        }

        // Consumers: what each machine still needs of each item after direct insertion, drawn through any of its
        // belt inserters (on one chain or several): it takes items as they pass, upstream machines first.
        let comp = self.components(belts);
        let mut draws: Vec<Draw> = Vec::new();
        for &m in &machines {
            let Some(mi) = self.machine_info(m) else { continue };
            for (item, &need) in &mi.demand {
                let di_in = sum_f64(in_ins[&m].iter().filter(|&&i| self.feeder_is_machine_of(i, item)).map(|i| delivered.get(i).copied().unwrap_or(0.0)));
                let rest = (need - di_in).max(0.0);
                let from_belts: Vec<usize> = in_ins[&m]
                    .iter()
                    .copied()
                    .filter(|&i| self.picks_from_belt(i) && belts.items.get(&self.pick_idx(i).expect("a belt pickup")).is_some_and(|s| s.contains(item)))
                    .collect();
                if rest < 1e-12 || from_belts.is_empty() {
                    continue;
                }
                draws.push(Draw { item: item.clone(), quota: rest, pickups: from_belts, left: rest });
            }
        }
        let mut draws_at: HashMap<usize, Vec<(usize, usize)>> = HashMap::new(); // pickup belt -> (draw, inserter)
        for (j, d) in draws.iter().enumerate() {
            for &ins in &d.pickups {
                draws_at.entry(self.pick_idx(ins).expect("a belt pickup")).or_default().push((j, ins));
            }
        }
        let mut outputs_at: JMap<usize, Vec<&OutputSpec>> = JMap::new();
        for o in spec.outputs.iter().filter(|o| !o.fluid) {
            if let Some(b) = self.index_at(o.tile).filter(|&b| ents[b].kind.belt_family()) {
                outputs_at.get_or_put_with(b, Vec::new).push(o);
            }
        }
        let output_list: Vec<(usize, &OutputSpec)> = outputs_at.iter_java().flat_map(|(b, os)| os.iter().map(move |o| (*b, *o))).collect();
        let short = self.allocate_producers(
            belts,
            &mut belt_rate,
            &belt_out,
            &to_belt,
            &draws.iter().map(|d| (d.item.clone(), d.quota, d.pickups.clone())).collect::<Vec<_>>(),
            &output_list.iter().map(|(b, o)| (o.item.clone(), o.rate, *b)).collect::<Vec<_>>(),
        );
        // The max flow decides whether a chain is short of an item (a splitter sends items wherever they are
        // drawn); the walk below decides who starves when it is (upstream machines draw first).
        let mut short_groups: HashSet<(String, usize)> = HashSet::new();
        for (j, d) in draws.iter().enumerate() {
            if short[j] > 1e-6 {
                for &ins in &d.pickups {
                    short_groups.insert((d.item.clone(), comp[self.pick_idx(ins).expect("a belt pickup")]));
                }
            }
        }
        for (j, (b, o)) in output_list.iter().enumerate() {
            if short[draws.len() + j] > 1e-6 {
                short_groups.insert((o.item.clone(), comp[*b]));
            }
        }

        // Walk belt chains in flow order, lane by lane: consumers draw what passes, lane and belt capacity hold.
        // Straight feeds and curves keep lanes, a sideload goes to the near lane, an inserter drops on the far
        // lane, a splitter divides each lane by what the consumers below each side draw of its item.
        let mut add_l: HashMap<usize, f64> = HashMap::new();
        let mut add_r: HashMap<usize, f64> = HashMap::new();
        for s in spec.sources.iter().filter(|s| !s.fluid) {
            if let Some(i) = self.index_at(s.tile) {
                match s.lane {
                    0 => *add_l.entry(i).or_insert(0.0) += s.rate,
                    1 => *add_r.entry(i).or_insert(0.0) += s.rate,
                    _ => {
                        *add_l.entry(i).or_insert(0.0) += s.rate / 2.0;
                        *add_r.entry(i).or_insert(0.0) += s.rate / 2.0;
                    }
                }
            }
        }
        for (&ins, &r) in belt_rate.iter_java() {
            let Some(b) = self.drop_idx(ins).filter(|&d| ents[d].kind == Kind::Belt) else { continue };
            if self.drop_lane(&ents[ins], b, belts) == 0 {
                *add_l.entry(b).or_insert(0.0) += r;
            } else {
                *add_r.entry(b).or_insert(0.0) += r;
            }
        }
        // Expected draw per belt and item (a machine's share spread over its pickups), for splitting at splitters.
        let mut est: HashMap<usize, JMap<String, f64>> = HashMap::new();
        let merge = |est: &mut HashMap<usize, JMap<String, f64>>, b: usize, item: &str, v: f64| {
            *est.entry(b).or_default().get_or_put_with(item.to_owned(), || 0.0) += v;
        };
        for d in &draws {
            for &ins in &d.pickups {
                merge(&mut est, self.pick_idx(ins).expect("a belt pickup"), &d.item, d.quota / d.pickups.len() as f64);
            }
        }
        for (b, os) in outputs_at.iter_java() {
            for o in os {
                merge(&mut est, *b, &o.item, o.rate);
            }
        }
        let succs = belts.next.clone();
        let mut demand = DemandBelow { est: &est, succs: &succs, below: HashMap::new(), below_item: HashMap::new() };
        let mut indeg: HashMap<usize, i32> = HashMap::new();
        for (_, ns) in succs.iter_java() {
            for &n in ns {
                *indeg.entry(n).or_insert(0) += 1;
            }
        }
        let mut in_l: HashMap<usize, f64> = HashMap::new();
        let mut in_r: HashMap<usize, f64> = HashMap::new();
        let mut queue: VecDeque<usize> = (0..ents.len()).filter(|&i| ents[i].kind.belt_family() && indeg.get(&i).copied().unwrap_or(0) == 0).collect();
        let mut done: HashSet<usize> = HashSet::new();
        let mut order: HashMap<usize, usize> = HashMap::new();
        let mut ins_left: HashMap<usize, f64> = HashMap::new();
        while let Some(i) = queue.pop_front() {
            if !done.insert(i) {
                continue;
            }
            order.insert(i, order.len());
            let e = &ents[i];
            let mut l = in_l.get(&i).copied().unwrap_or(0.0) + add_l.get(&i).copied().unwrap_or(0.0);
            let mut r = in_r.get(&i).copied().unwrap_or(0.0) + add_r.get(&i).copied().unwrap_or(0.0);
            let lane_cap = Self::belt_capacity(e) / 2.0 * if e.kind == Kind::Splitter { 2.0 } else { 1.0 };
            if l > lane_cap + 1e-6 || r > lane_cap + 1e-6 {
                self.err(
                    ErrorType::BeltOverCapacity,
                    e.tile(),
                    format!("lanes carry {}/s and {}/s; a lane holds {}/s", fmt2(l), fmt2(r), fmt2(lane_cap)),
                );
            }
            // Takes up to `want` of `item` from the lane(s) that carry it; a belt with one item on both lanes
            // drains both evenly.
            let draw = |l: &mut f64, r: &mut f64, item: &str, want: f64| -> f64 {
                let mask = belts.lanes_with(i, item);
                let have = (if mask & 1 != 0 { *l } else { 0.0 }) + (if mask & 2 != 0 { *r } else { 0.0 });
                let t = want.min(have);
                if have > 1e-12 {
                    let keep = (1.0 - t / have).max(0.0);
                    if mask & 1 != 0 {
                        *l *= keep;
                    }
                    if mask & 2 != 0 {
                        *r *= keep;
                    }
                }
                t
            };
            for &(j, ins) in draws_at.get(&i).map_or(&[][..], |v| v.as_slice()) {
                if draws[j].left <= 1e-12 {
                    continue;
                }
                let c = *ins_left.entry(ins).or_insert_with(|| self.cap(ins));
                let t = draw(&mut l, &mut r, &draws[j].item, draws[j].left.min(c));
                draws[j].left -= t;
                ins_left.insert(ins, c - t);
            }
            for o in outputs_at.get(&i).map_or(&[][..], |v| v.as_slice()) {
                let t = draw(&mut l, &mut r, &o.item, o.rate);
                if t + 1e-6 < o.rate && short_groups.contains(&(o.item.clone(), comp[i])) {
                    self.err(ErrorType::Starved, e.tile(), format!("the {} output port gets {}/s of {}/s", o.item, fmt2(t), fmt2(o.rate)));
                }
            }
            let outs: Vec<usize> = succs.get(&i).cloned().unwrap_or_default();
            let mut shares = |lane: usize| -> Vec<f64> {
                if outs.len() == 1 {
                    return vec![1.0];
                }
                let item = belts.lane_items.get(&i).and_then(|li| if li[lane].len() == 1 { li[lane].first().cloned() } else { None });
                let wants: Vec<f64> = outs
                    .iter()
                    .map(|&n| match &item {
                        Some(item) => demand.item(n, item, 0),
                        None => demand.any(n, 0),
                    })
                    .collect();
                let sum = sum_f64(wants.iter().copied());
                wants.iter().map(|&w| if sum > 1e-12 { w / sum } else { 1.0 / outs.len() as f64 }).collect()
            };
            let share_l = shares(0);
            let share_r = shares(1);
            for (k, &n) in outs.iter().enumerate() {
                let (to_l, to_r) = self.lane_into(e, &ents[n], belts);
                let (fl, fr) = (l * share_l[k], r * share_r[k]);
                // to_l / to_r: where this node's left and right lanes land on the successor (0 = left, 1 = right).
                *(if to_l == 0 { &mut in_l } else { &mut in_r }).entry(n).or_insert(0.0) += fl;
                *(if to_r == 0 { &mut in_l } else { &mut in_r }).entry(n).or_insert(0.0) += fr;
                let remaining = indeg.get(&n).copied().unwrap_or(1) - 1;
                indeg.insert(n, remaining);
                if remaining == 0 {
                    queue.push_back(n);
                }
            }
        }
        // A machine that did not get its share along a chain: reported at its last pickup on that chain.
        for d in &draws {
            if d.left <= 1e-6 || !d.pickups.iter().any(|&p| short_groups.contains(&(d.item.clone(), comp[self.pick_idx(p).expect("a belt pickup")]))) {
                continue;
            }
            let mut at = self.pick_idx(d.pickups[0]).expect("a belt pickup");
            for &p in &d.pickups[1..] {
                let b = self.pick_idx(p).expect("a belt pickup");
                if order.get(&b).map_or(-1, |&o| o as i64) > order.get(&at).map_or(-1, |&o| o as i64) {
                    at = b; // the first of the latest, like maxByOrNull
                }
            }
            self.err(
                ErrorType::Starved,
                ents[at].tile(),
                format!("consumers here need {}/s of {} but only {}/s arrives (net {})", fmt2(d.quota), d.item, fmt2(d.quota - d.left), nul(&ents[at].net)),
            );
        }
    }

    /// Connected belt chains: entity index -> a representative of its chain.
    fn components(&self, belts: &BeltGraph) -> Vec<usize> {
        let n = self.ents().len();
        let mut parent: Vec<usize> = (0..n).collect();
        fn find(parent: &[usize], x: usize) -> usize {
            let mut a = x;
            while parent[a] != a {
                a = parent[a];
            }
            a
        }
        for (a, bs) in belts.next.iter_java() {
            for b in bs {
                let (ra, rb) = (find(&parent, *a), find(&parent, *b));
                if ra != rb {
                    parent[ra] = rb;
                }
            }
        }
        (0..n).map(|i| find(&parent, i)).collect()
    }

    /// Lane (0 left, 1 right of the belt's heading) inserter `ins` drops onto belt entity `b`: the far one, on a
    /// curve the far side of the bend (see `game_constants::drop_lane`). [P] VERIFY (v2)
    fn drop_lane(&self, ins: &PlacedEntity, b: usize, belts: &BeltGraph) -> i32 {
        let belt = &self.ents()[b];
        gc::drop_lane(ins.dir.opposite(), self.curve_into(b, belts).unwrap_or(belt.dir), belt.dir)
    }

    /// The heading items enter belt `b` with when it is a curve (fed from one side only), else None.
    fn curve_into(&self, b: usize, belts: &BeltGraph) -> Option<Dir> {
        let ents = self.ents();
        let f = &ents[b];
        if f.kind != Kind::Belt {
            return None;
        }
        let feeders = belts.prev.get(&b)?;
        if feeders.len() != 1 || feeders.iter().any(|&p| ents[p].dir == f.dir) {
            return None;
        }
        Some(ents[feeders[0]].dir)
    }

    /// Where `e`'s left and right lanes land on successor `f`: kept for a straight feed, a curve, an
    /// underground or a splitter; both onto the near lane for a sideload.
    fn lane_into(&self, e: &PlacedEntity, f: &PlacedEntity, belts: &BeltGraph) -> (usize, usize) {
        let ents = self.ents();
        if e.kind == Kind::UgIn || f.kind == Kind::Splitter || e.dir == f.dir {
            return (0, 1);
        }
        if f.kind == Kind::Belt {
            let feeders = belts.prev.get(&self.index_at(f.tile()).expect("the successor is placed")).map_or(&[][..], |v| v.as_slice());
            let rear = feeders.iter().any(|&p| ents[p].dir == f.dir);
            if !rear && feeders.len() == 1 {
                return (0, 1); // a curve keeps lanes
            }
        }
        // Sideload: arriving from the belt's left side (moving towards its right) lands on its left lane.
        let near = if e.dir == f.dir.cw() { 0 } else { 1 };
        (near, near)
    }

    /// Where each producer's belt output goes. A producer feeding several belts sends its output wherever it
    /// is taken (a full belt backs up and its inserters wait), and an item dropped on a belt reaches only what
    /// lies downstream of the drop. Max flow: producer -> each of its belt inserters (capped by the inserter's
    /// rate) -> each consumer inserter of its item picking downstream of the drop (capped by that inserter's
    /// rate) -> the machine's draw of the item along that chain -> sink (capped by the draw); output ports
    /// take directly. Fixed supplies (ports, harness chests) join as sources at their rate. Each producer
    /// inserter's belt rate is the flow through it; what the flow cannot meet shows up as starvation.
    /// Returns what each draw (then each output) misses.
    #[allow(clippy::type_complexity)]
    fn allocate_producers(
        &self,
        belts: &BeltGraph,
        belt_rate: &mut JMap<usize, f64>,
        belt_out: &JMap<(usize, String), f64>,
        to_belt: &JMap<(usize, String), Vec<usize>>,
        // Machine draws: (item, items/s, consumer inserters that may serve it).
        draws: &[(String, f64, Vec<usize>)],
        // Output ports: (item, items/s, belt).
        outputs: &[(String, f64, usize)],
    ) -> Vec<f64> {
        let ents = self.ents();
        let spec = self.spec;
        let producers: Vec<(usize, String)> = to_belt
            .iter_java()
            .filter(|(k, v)| !v.is_empty() && belt_out.get(k).copied().unwrap_or(0.0) > 1e-12)
            .map(|(k, _)| k.clone())
            .collect();
        let drop_belt = |ins: usize| self.index_at(ents[ins].drop_tile()).filter(|&b| ents[b].kind.belt_family());
        let mut fixed: Vec<(usize, String, f64)> = Vec::new(); // belt, item, items/s
        for s in spec.sources.iter().filter(|s| !s.fluid) {
            if let Some(b) = self.index_at(s.tile).filter(|&b| ents[b].kind.belt_family()) {
                fixed.push((b, s.item.clone(), s.rate));
            }
        }
        for (&ins, &r) in belt_rate.iter_java() {
            if r > 0.0
                && let Some(b) = drop_belt(ins)
                && let Some(item) = self.carried_by_inserter(&ents[ins], None)
            {
                fixed.push((b, item, r));
            }
        }

        let mut reach: HashMap<usize, HashSet<usize>> = HashMap::new();
        let mut downstream = |b: usize| -> HashSet<usize> {
            reach
                .entry(b)
                .or_insert_with(|| {
                    let mut seen = HashSet::from([b]);
                    let mut q = VecDeque::from([b]);
                    while let Some(u) = q.pop_front() {
                        for &n in belts.next.get(&u).map_or(&[][..], |v| v.as_slice()) {
                            if seen.insert(n) {
                                q.push_back(n);
                            }
                        }
                    }
                    seen
                })
                .clone()
        };
        let ins_list: Vec<(usize, usize)> = producers
            .iter()
            .enumerate()
            .flat_map(|(k, m)| to_belt.get(m).expect("a producer has inserters").iter().map(move |&ins| (k, ins)))
            .collect();
        // Consumer inserter nodes, one per (inserter, item), each capped by the inserter's rate.
        let mut pick_nodes: Vec<(usize, String)> = Vec::new();
        for (item, _, list) in draws {
            for &ins in list {
                let node = (ins, item.clone());
                if !pick_nodes.contains(&node) {
                    pick_nodes.push(node);
                }
            }
        }
        // Nodes: 0 source, 1 sink, producers, producer inserters, fixed supplies, pickups (in, out), draws, outputs.
        let p_base = 2;
        let i_base = p_base + producers.len();
        let f_base = i_base + ins_list.len();
        let k_base = f_base + fixed.len();
        let d_base = k_base + 2 * pick_nodes.len();
        let o_base = d_base + draws.len();
        let mut g = MaxFlow::new(o_base + outputs.len());
        let pick_index: HashMap<(usize, String), usize> = pick_nodes.iter().enumerate().map(|(k, p)| (p.clone(), k_base + 2 * k)).collect();
        for (k, (ins, _)) in pick_nodes.iter().enumerate() {
            g.edge(k_base + 2 * k, k_base + 2 * k + 1, self.cap(*ins));
        }
        let mut sink_edge = vec![0usize; draws.len() + outputs.len()];
        for (j, (item, q, list)) in draws.iter().enumerate() {
            for &ins in list {
                g.edge(pick_index[&(ins, item.clone())] + 1, d_base + j, f64::MAX);
            }
            sink_edge[j] = g.edge(d_base + j, 1, *q);
        }
        for (j, (_, r, _)) in outputs.iter().enumerate() {
            sink_edge[draws.len() + j] = g.edge(o_base + j, 1, *r);
        }
        let mut connect = |g: &mut MaxFlow, from: usize, b: usize, item: &str| {
            let down = downstream(b);
            for (k, (ins, it2)) in pick_nodes.iter().enumerate() {
                if it2 == item && self.index_at(ents[*ins].pickup_tile()).is_some_and(|p| down.contains(&p)) {
                    g.edge(from, k_base + 2 * k, f64::MAX);
                }
            }
            for (j, (it2, _, ob)) in outputs.iter().enumerate() {
                if it2 == item && down.contains(ob) {
                    g.edge(from, o_base + j, f64::MAX);
                }
            }
        };
        for (k, m) in producers.iter().enumerate() {
            g.edge(0, p_base + k, belt_out.get(m).copied().expect("a producer has an output"));
        }
        let ins_edge: Vec<usize> = ins_list
            .iter()
            .enumerate()
            .map(|(k, &(pk, ins))| {
                let e = g.edge(p_base + pk, i_base + k, self.cap(ins));
                if let Some(b) = drop_belt(ins) {
                    connect(&mut g, i_base + k, b, &producers[pk].1);
                }
                e
            })
            .collect();
        for (k, (b, item, r)) in fixed.iter().enumerate() {
            g.edge(0, f_base + k, *r);
            connect(&mut g, f_base + k, *b, item);
        }
        g.run(0, 1);
        for (k, &(_, ins)) in ins_list.iter().enumerate() {
            belt_rate.insert(ins, g.flow_on(ins_edge[k]));
        }
        // What each draw (then each output) misses.
        (0..sink_edge.len()).map(|j| (if j < draws.len() { draws[j].1 } else { outputs[j - draws.len()].1 }) - g.flow_on(sink_edge[j])).collect()
    }

    /// Item output ports: a belt of the item on the port tile that leads out of the box.
    fn check_outputs(&mut self, belts: &BeltGraph) {
        let (spec, ents) = (self.spec, self.ents());
        let bounds = Rect::new(0, 0, self.layout.w, self.layout.h);
        for o in spec.outputs.iter().filter(|o| !o.fluid) {
            let i = self.index_at(o.tile);
            let e = i.map(|i| &ents[i]);
            match e {
                Some(e) if e.kind.belt_family() => {
                    if !i.is_some_and(|i| belts.items.get(&i).is_some_and(|s| s.contains(&o.item))) {
                        self.err(ErrorType::OutputMissing, o.tile, format!("no {} reaches its output port", o.item));
                    } else if !matches!(e.output_tile().map(|t| bounds.contains_tile(t)), Some(false)) {
                        self.err(ErrorType::OutputMissing, o.tile, format!("the belt at the {} output port does not lead out of the box", o.item));
                    }
                }
                _ => self.err(ErrorType::OutputMissing, o.tile, format!("output port of {} has no belt", o.item)),
            }
        }
    }

    // ---- 7. power ----
    fn check_power(&mut self) {
        let ents = self.ents();
        let poles: Vec<usize> = (0..ents.len()).filter(|&i| ents[i].kind == Kind::Pole).collect();
        if !self.spec.require_power {
            return;
        }
        for e in ents.iter().filter(|e| matches!(e.kind, Kind::Machine | Kind::Inserter)) {
            if !poles.iter().any(|&p| Self::covers(&ents[p], &e.rect())) {
                self.err(ErrorType::Unpowered, e.tile(), format!("{} is outside every pole's supply area", e.name));
            }
        }
        if poles.len() > 1 {
            let mut adj: HashMap<usize, Vec<usize>> = poles.iter().map(|&p| (p, Vec::new())).collect();
            for &(a, b) in &self.layout.wires {
                let (pa, pb) = (&ents[a], &ents[b]);
                if pa.kind != Kind::Pole || pb.kind != Kind::Pole {
                    continue;
                }
                let reach = gc::pole(&pa.name).expect("a known pole").wire_reach.min(gc::pole(&pb.name).expect("a known pole").wire_reach);
                let d = (f64::from(pa.x) + f64::from(pa.w) / 2.0 - f64::from(pb.x) - f64::from(pb.w) / 2.0)
                    .hypot(f64::from(pa.y) + f64::from(pa.h) / 2.0 - f64::from(pb.y) - f64::from(pb.h) / 2.0);
                if d > reach + 1e-9 {
                    self.err(ErrorType::WireTooLong, pa.tile(), format!("wire of {} tiles exceeds reach {}", fmt2(d), fmt2(reach)));
                }
                adj.get_mut(&a).expect("a pole").push(b);
                adj.get_mut(&b).expect("a pole").push(a);
            }
            let mut seen: HashSet<usize> = HashSet::new();
            let mut stack = vec![poles[0]];
            while let Some(p) = stack.pop() {
                if seen.insert(p) {
                    stack.extend(adj[&p].iter().copied());
                }
            }
            for &p in poles.iter().filter(|p| !seen.contains(p)) {
                self.err(ErrorType::PolesDisconnected, ents[p].tile(), "pole is not wired to the rest".to_owned());
            }
        }
    }

    fn covers(pole: &PlacedEntity, r: &Rect) -> bool {
        let s = gc::pole(&pole.name).expect("a known pole").supply_half;
        let cx = f64::from(pole.x) + f64::from(pole.w) / 2.0;
        let cy = f64::from(pole.y) + f64::from(pole.h) / 2.0;
        // Overlap of open intervals: touching edges do not count.
        f64::from(r.x) < cx + s && f64::from(r.x + r.w) > cx - s && f64::from(r.y) < cy + s && f64::from(r.y + r.h) > cy - s
    }

    // ---- 8. fluids ----
    /// Rebuilds fluid networks: pipes join adjacent pipes, pipe-to-ground ends join along their open side and
    /// to their pair, and a pipe on a machine's connection tile joins that connection. Each network may carry
    /// one fluid, every used input needs a supply of its fluid, every used output a consumer or a drain, and
    /// no pipe may touch a connection the recipe does not use.
    fn check_fluids(&mut self) {
        let (spec, ents) = (self.spec, self.ents());
        let machines: Vec<usize> = (0..ents.len()).filter(|&i| ents[i].kind == Kind::Machine).collect();
        let empty_in: IndexMap<String, i32> = IndexMap::new();
        let conn_list: Vec<(usize, crate::place::ConnTile)> = machines
            .iter()
            .flat_map(|&i| {
                let m = &ents[i];
                let info = self.machine_info(i);
                connections_of(
                    &m.name,
                    m.w,
                    m.h,
                    info.map_or(&empty_in, |x| &x.fluid_in),
                    info.map_or(&empty_in, |x| &x.fluid_out),
                    m.x,
                    m.y,
                    m.dir.ordinal() as i32,
                    m.mirror,
                )
                .into_iter()
                .map(move |c| (i, c))
            })
            .collect();
        if spec.fluid_mode == "manual" {
            for (_, c) in conn_list.iter().filter(|(_, c)| c.fluid.is_some()) {
                if let Some(e) = self.entity_at(c.tile) {
                    self.err(ErrorType::FluidBlocked, c.tile, format!("fluid connection must stay free for manual piping, found {}", e.name));
                }
            }
            return;
        }
        let is_pipe = |k: Kind| matches!(k, Kind::Pipe | Kind::PipeUg | Kind::InfinityPipe);
        let pipes: Vec<usize> = (0..ents.len()).filter(|&i| is_pipe(ents[i].kind)).collect();
        let opens_to = |i: usize, d: Dir| ents[i].kind != Kind::PipeUg || ents[i].dir == d;
        // Union-find over pipe entities and machine connections (ids after the entities).
        let mut parent: Vec<usize> = (0..ents.len() + conn_list.len()).collect();
        fn find(parent: &mut [usize], x: usize) -> usize {
            let mut a = x;
            while parent[a] != a {
                parent[a] = parent[parent[a]];
                a = parent[a];
            }
            a
        }
        fn join(parent: &mut [usize], a: usize, b: usize) {
            let (ra, rb) = (find(parent, a), find(parent, b));
            if ra != rb {
                parent[ra] = rb;
            }
        }
        for &i in &pipes {
            let e = &ents[i];
            for d in Dir::ALL {
                let Some(j) = self.index_at(e.tile() + d) else { continue };
                if is_pipe(ents[j].kind) && opens_to(i, d) && opens_to(j, d.opposite()) {
                    join(&mut parent, i, j);
                }
            }
            // Pipe-to-ground pairing: the first pipe-to-ground on the axis behind the open side.
            if e.kind == Kind::PipeUg {
                let back = e.dir.opposite();
                for k in 1..=gc::PIPE_TO_GROUND_GAP + 1 {
                    let Some(j) = self.index_at(e.tile().step(back, k)) else { continue };
                    let o = &ents[j];
                    if o.kind != Kind::PipeUg || o.dir.axis() != e.dir.axis() {
                        continue;
                    }
                    if o.dir == back {
                        join(&mut parent, i, j);
                    }
                    break;
                }
            }
        }
        let mut attached = vec![false; conn_list.len()];
        for (k, (_, c)) in conn_list.iter().enumerate() {
            let Some(j) = self.index_at(c.tile) else { continue };
            if is_pipe(ents[j].kind) && opens_to(j, c.side.opposite()) {
                join(&mut parent, ents.len() + k, j);
                attached[k] = true;
            }
        }
        // What each network holds.
        let mut supplies: JMap<usize, IndexSet<String>> = JMap::new();
        let mut drains: JMap<usize, IndexSet<String>> = JMap::new();
        for &i in &pipes {
            let e = &ents[i];
            if e.kind == Kind::InfinityPipe
                && let Some(f) = &e.fluid
            {
                let root = find(&mut parent, i);
                (if e.drain { &mut drains } else { &mut supplies }).get_or_put_with(root, IndexSet::new).insert(f.clone());
            }
        }
        for s in spec.sources.iter().filter(|s| s.fluid) {
            match self.index_at(s.tile).filter(|&i| is_pipe(ents[i].kind)) {
                None => self.err(ErrorType::FluidMissing, s.tile, format!("fluid port of {} has no pipe", s.item)),
                Some(i) => {
                    let root = find(&mut parent, i);
                    supplies.get_or_put_with(root, IndexSet::new).insert(s.item.clone());
                }
            }
        }
        for o in spec.outputs.iter().filter(|o| o.fluid) {
            match self.index_at(o.tile).filter(|&i| is_pipe(ents[i].kind)) {
                None => self.err(ErrorType::OutputMissing, o.tile, format!("fluid output port of {} has no pipe", o.item)),
                Some(i) => {
                    let root = find(&mut parent, i);
                    drains.get_or_put_with(root, IndexSet::new).insert(o.item.clone());
                }
            }
        }
        for (k, (m, c)) in conn_list.iter().enumerate() {
            if !attached[k] {
                continue;
            }
            let r = find(&mut parent, ents.len() + k);
            let Some(fluid) = &c.fluid else {
                self.err(ErrorType::FluidLeak, c.tile, format!("a pipe touches an unused fluid connection of {}", nul(&ents[*m].recipe)));
                continue;
            };
            (if c.flow == gc::Flow::Output { &mut supplies } else { &mut drains }).get_or_put_with(r, IndexSet::new).insert(fluid.clone());
        }
        let roots: Vec<usize> = supplies.keys_java().chain(drains.keys_java()).copied().collect();
        for r in roots {
            let mut all: IndexSet<String> = supplies.get(&r).cloned().unwrap_or_default();
            add_all(&mut all, drains.get(&r).unwrap_or(&IndexSet::new()));
            if all.len() > 1 {
                let at = pipes.iter().copied().find(|&p| find(&mut parent, p) == r).map_or(Tile::new(0, 0), |p| ents[p].tile());
                self.err(ErrorType::FluidMix, at, format!("fluids {} share one pipe network", sorted_list(&all)));
            }
        }
        for (k, (m, c)) in conn_list.iter().enumerate() {
            let Some(f) = &c.fluid else { continue };
            let r = find(&mut parent, ents.len() + k);
            let linked = attached[k];
            if c.flow == gc::Flow::Input && (!linked || !supplies.get(&r).is_some_and(|s| s.contains(f))) {
                self.err(ErrorType::FluidMissing, c.tile, format!("{} gets no {} here", nul(&ents[*m].recipe), f));
            }
            if c.flow == gc::Flow::Output && (!linked || !drains.get(&r).is_some_and(|s| s.contains(f))) {
                self.err(ErrorType::FluidStuck, c.tile, format!("{} outputs {} with nowhere to go", nul(&ents[*m].recipe), f));
            }
        }
    }
}

/// Keeps `JSet` in use where a Kotlin `HashSet` is iterated; none of the validator's sets are.
#[allow(dead_code)]
type _Unused = JSet<usize>;
