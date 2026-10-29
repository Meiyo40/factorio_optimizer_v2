//! The router: negotiated-congestion routing of belt chains (and, through `PipeRouter`, pipes) on the placed
//! machines, ported from route/PathFinder.kt. Kotlin's single file is split over three here, all `impl PathFinder`:
//! this one (state, setup, the negotiation loop, the endgame, the result), `search.rs` (A*, legs, commit and undo,
//! sinks) and `chains.rs` (chain construction, entity emission). Function names follow the Kotlin.
//!
//! Tile indices and search states are `i32` as in Kotlin: `tile + d_idx[dir]` may leave `0..n` and the code guards
//! it the same way. Floats are `f32` where Kotlin has `Float` (costs, history, heap keys) and `f64` where it has `Double`.

use super::access::{AccessOption, DiLink, EffectiveTerminal, assign_direct_insertion, effective_terminals};
use super::min_heap::MinHeap;
use super::pipes::{PRouted, PipeRouter};
use crate::config::run_configuration::RunConfiguration;
use crate::config::tuning::RouteTuning;
use crate::data::game_constants::{self as gc, BeltTier};
use crate::geom::{Dir, GridDims, Kind, Layout, PlacedEntity, Rect, Tile};
use crate::place::Placement;
use crate::plan::{ProductionPlan, Role, Terminal};
use crate::util::java_hash::{JMap, JSet};
use crate::util::java_random::{JavaRandom, java_shuffle};
use crate::validate::{OutputSpec, SourceSpec};
use crate::viz::{HeatOverlay, RenderOptions, render_layout};
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) const MARGIN: i32 = 2;
pub(super) const BELT: i32 = 0;
pub(super) const UG_EXIT: i32 = 1;
/// Belt-family kinds for `accept_mask`.
pub(super) const AK_BELT: i32 = 0;
pub(super) const AK_UG_IN: i32 = 1;
pub(super) const AK_UG_OUT: i32 = 2;
pub(super) const AK_SPLITTER: i32 = 3;
pub(super) const REPAIR: i32 = -3;

pub struct RouteProblem<'a> {
    pub plan: &'a ProductionPlan,
    pub config: &'a RunConfiguration,
    pub tuning: RouteTuning,
    pub placement: &'a Placement,
    /// Extra fixed obstacles (forbidden tiles, fixed ports, reserved pole lattice, power reservations).
    pub obstacles: HashSet<Tile>,
    /// Extra per-tile cost on belts, e.g. to keep tiles free for poles.
    pub tile_bias: Vec<(Tile, f64)>,
    /// History from an earlier run on the same placement (warm start), one value per tile of the grid with margin.
    pub warm_history: Option<Vec<f32>>,
    /// Varies the endgame's shuffles, so repeated attempts on one placement differ.
    pub endgame_seed: i64,
    /// Tiles no underground may pass under (blocks, when stitching a split factory).
    pub no_tunnel: HashSet<Tile>,
    /// Where to draw the router's debug renders (one per iteration, and one per net that fails), if anywhere.
    pub debug_dir: Option<PathBuf>,
}

impl<'a> RouteProblem<'a> {
    pub fn new(plan: &'a ProductionPlan, config: &'a RunConfiguration, tuning: RouteTuning, placement: &'a Placement) -> Self {
        Self {
            plan,
            config,
            tuning,
            placement,
            obstacles: HashSet::new(),
            tile_bias: Vec::new(),
            warm_history: None,
            endgame_seed: 0,
            no_tunnel: HashSet::new(),
            debug_dir: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteStats {
    pub belts: usize,
    pub undergrounds: usize,
    pub inserters: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChainStat {
    pub item: String,
    pub terminals: usize,
    pub hpwl: i32,
    pub tiles: usize,
}

pub struct RouteResult {
    pub success: bool,
    pub entities: Vec<PlacedEntity>,
    pub iterations: i32,
    /// Conflicts left at the end, or -1 when no iteration ran.
    pub overuse: i32,
    /// Congestion map for the placer, in the order Kotlin's `HashMap<Tile, Double>` iterates it.
    pub history: JMap<Tile, f64>,
    pub raw_history: Vec<f32>,
    pub failed_nets: Vec<i32>,
    pub sources: Vec<SourceSpec>,
    pub outputs: Vec<OutputSpec>,
    pub conflicts: Vec<Tile>,
    /// Plan nets at each conflict tile, each in the order Kotlin's set iterates.
    pub conflict_nets: IndexMap<Tile, Vec<i32>>,
    pub di: Vec<DiLink>,
    pub stats: RouteStats,
    pub chains: usize,
    pub chain_stats: Vec<ChainStat>,
    pub millis: i64,
    pub log: Vec<String>,
    /// Overuse plus unrouted penalty after each iteration.
    pub trace: Vec<i32>,
    /// A* states expanded: the work done, for comparing runs.
    pub expansions: u64,
}

/// A chain's share of a plan terminal. Nets with many inserter visits (or more flow than one belt) are split into
/// several chains after direct insertion, so one producer may feed two chains.
#[derive(Debug, Clone)]
pub struct Vt {
    pub base: Terminal,
    pub rate: f64,
    pub k: i32,
    pub net: i32,
    /// Which of its chain's items this is: 0, or 1 for the second item of a shared belt (its lane).
    pub slot: i32,
}

impl Vt {
    pub fn role(&self) -> Role {
        self.base.role
    }

    pub fn machine(&self) -> Option<usize> {
        self.base.machine
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RNet {
    pub idx: i32,
    pub plan_id: i32,
    pub item: String,
    pub terms: Vec<i32>,
    pub port_tiles: Vec<i32>,
    pub port_out: Vec<i32>,
    pub hpwl: i32,
    pub source_rate: f64,
    /// Output port: candidate edge tiles, their outward direction (-1: any), and this chain's share of the output.
    pub out_tiles: Vec<i32>,
    pub out_out: Vec<i32>,
    pub out_rate: f64,
    /// Second item of a shared belt (right lane; `item` takes the left lane), and its port rate.
    pub item2: Option<String>,
    pub source_rate2: f64,
    /// Plan ports this chain starts from and ends at (-1: none), for telling ports of one item apart.
    pub src_port: i32,
    pub out_port: i32,
}

#[derive(Clone)]
pub(super) struct Routed {
    /// tile * 4 + dir
    pub belts: Vec<i32>,
    pub ug_in: Vec<i32>,
    pub ug_out: Vec<i32>,
    /// Tier of each underground pair (0 main, 1 weaving), parallel to `ug_in` and `ug_out`.
    pub ug_tier: Vec<i32>,
    /// tier * 2N + axis * N + tile
    pub chans: Vec<i32>,
    /// Option ids.
    pub claims: Vec<i32>,
    /// Where the chain's leaves point (the final head, and heads left behind by branches): tile * 4 + dir.
    pub dead_ends: Vec<i32>,
    /// Splitters inserted for branches: [trunk tile, branch tile, direction].
    pub splitters: Vec<[i32; 3]>,
    /// Tiles kept free of this net (where a leaf points).
    pub reserved: Vec<i32>,
    /// Belts placed once every producer was visited; branches may start from these on.
    pub branch_from_belt: i32,
    pub source_tile: i32,
    /// Edge tile where the chain leaves the block (output port), or -1.
    pub out_tile: i32,
    pub ok: bool,
    /// Every tile this net occupies.
    pub tiles: Vec<i32>,
}

impl Routed {
    pub fn new() -> Self {
        Self {
            belts: Vec::new(),
            ug_in: Vec::new(),
            ug_out: Vec::new(),
            ug_tier: Vec::new(),
            chans: Vec::new(),
            claims: Vec::new(),
            dead_ends: Vec::new(),
            splitters: Vec::new(),
            reserved: Vec::new(),
            branch_from_belt: -1,
            source_tile: -1,
            out_tile: -1,
            ok: false,
            tiles: Vec::new(),
        }
    }
}

/// A route as the negotiation loop and the endgame keep it for rolling back.
pub(super) enum Saved {
    Belt(Option<Routed>),
    Pipe(Option<PRouted>),
}

pub struct PathFinder<'a> {
    pub(super) p: &'a RouteProblem<'a>,
    pub(super) plan: &'a ProductionPlan,
    pub(super) config: &'a RunConfiguration,
    pub(super) tun: RouteTuning,
    pub(super) w: i32,
    pub(super) h: i32,
    pub dims: GridDims,
    pub(super) n: usize,
    pub(super) fw: i32,
    pub(super) tier: &'static BeltTier,
    pub(super) max_j: i32,
    pub(super) ug_allowed: bool,
    pub(super) no_tunnel: Vec<bool>,
    pub(super) weave: Option<&'static BeltTier>,
    pub(super) max_jw: i32,
    /// Channels: tier * 2N + axis * N + tile (tier 0 main, 1 weaving); 4N of them.
    pub(super) ch: usize,
    /// The net being routed may use the weaving tier (its flow fits that tier's belts).
    pub(super) weave_ok: bool,
    pub(super) log: Vec<String>,
    pub trace: Vec<i32>,
    /// Failed nets drawn so far (at most 6 are).
    pub(super) fail_renders: i32,
    pub(super) d_idx: [i32; 4],
    pub(super) d_x: [i32; 4],
    pub(super) d_y: [i32; 4],

    // static tile data
    /// -2: never; -1: any net; n >= 0: only net index n (fixed ports).
    pub(super) allow_net: Vec<i32>,
    pub(super) pin_count: Vec<i32>,
    pub(super) bias: Vec<f32>,

    // terminals and options
    pub(super) eff: HashMap<usize, EffectiveTerminal>,
    pub di: Vec<DiLink>,
    pub(super) vts: Vec<Vt>,
    pub(super) term_net: Vec<i32>,
    pub(super) term_k: Vec<i32>,
    pub(super) need: Vec<i32>,
    pub(super) term_opts: Vec<Vec<i32>>,
    pub(super) sink_k: HashMap<usize, i32>,
    /// Terminal visiting order of each chain's last successful route; reroutes follow it.
    pub(super) net_order: Vec<Option<Vec<i32>>>,
    /// The chain's order while routing, and the leg's target terminal (-1: any claimable one).
    pub(super) order: Option<Vec<i32>>,
    pub(super) leg_target: i32,
    pub(super) options: Vec<AccessOption>,
    pub(super) opt_belt: Vec<i32>,
    pub(super) opt_ins: Vec<i32>,
    pub(super) opt_term: Vec<i32>,
    pub(super) opt_long: Vec<bool>,
    pub(super) at_belt_start: Vec<i32>,
    pub(super) at_belt_list: Vec<i32>,
    pub(super) sink_options: JMap<usize, Vec<AccessOption>>,
    pub(super) nets: Rc<Vec<RNet>>,

    // dynamic congestion state
    pub(super) occ_tile: Vec<i32>,
    pub(super) occ_belt: Vec<i32>,
    /// Chain ends pointing into each tile: total, and per direction they point (tile * 4 + dir).
    pub(super) dead_end_at: Vec<i32>,
    pub(super) dead_end_dir: Vec<i32>,
    /// Belt-family entities on each tile that take items moving in each direction (tile * 4 + dir).
    pub(super) belt_accept: Vec<i32>,
    pub(super) occ_chan: Vec<i32>,
    pub(super) hist: Vec<f32>,
    pub(super) hist_chan: Vec<f32>,
    pub(super) p_fac: f32,
    pub(super) routed: Vec<Option<Routed>>,
    /// (inserter option, chest tile)
    pub(super) sink_claims: Vec<(AccessOption, Tile)>,

    // per-net own markers
    pub(super) own_stamp: Vec<i32>,
    pub(super) own_belt_stamp: Vec<i32>,
    pub(super) own_chan_stamp: Vec<i32>,
    /// Direction of this net's belt or underground exit on a tile (valid where `own_belt_stamp` matches); -1 on an
    /// underground entrance, which feeds nothing in front of it.
    pub(super) own_dir: Vec<i32>,
    /// The leg being searched heads for the output port (every terminal is served).
    pub(super) output_leg: bool,
    pub(super) out_dir_at: Vec<i32>,
    /// Branch start tile -> trunk tile the splitter goes on, for the current search.
    pub(super) branch_from: HashMap<i32, i32>,
    pub(super) stamp: i32,
    pub(super) cur: i32,

    // A* working arrays
    pub(super) s_states: i32,
    pub(super) g_arr: Vec<f32>,
    pub(super) par: Vec<i32>,
    pub(super) seen_gen: Vec<i32>,
    pub(super) closed_gen: Vec<i32>,
    pub(super) search_gen: i32,
    pub(super) heap: MinHeap,
    pub(super) dist: Vec<i32>,
    pub(super) goal_stamp: Vec<i32>,
    pub(super) goal_gen: i32,
    pub(super) res_stamp: Vec<i32>,
    pub(super) start_stamp: Vec<i32>,
    pub(super) path_stamp: Vec<i32>,
    pub(super) path_first: Vec<i32>,
    pub(super) forbid_stamp: Vec<i32>,
    pub(super) forbid_gen: i32,
    pub(super) path_chan_stamp: Vec<i32>,
    pub(super) path_gen: i32,
    pub(super) start_opt: Vec<i32>,
    pub(super) done_state: Vec<i32>,
    pub(super) done_opt: Vec<i32>,
    pub(super) min_step: f32,
    pub(super) h_weight: f32,
    pub expansions: u64,

    /// Items/s on the chain at its current head: supply dropped so far minus what consumers took (per item slot).
    pub(super) surplus: [f64; 2],
    /// The chain being routed carries two items, one per lane (slot 0 left, slot 1 right).
    pub(super) paired: bool,
    /// Items/s on each lane of the chain at its head (0 left, 1 right of the belt's heading).
    pub(super) lane_load: [f64; 2],
    /// Fluid nets routed as pipes ("pipes" mode); created once the static grids exist.
    pub(super) pipes: Option<PipeRouter>,
}

pub(super) fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

pub(super) fn role_initial(role: Role) -> &'static str {
    match role {
        Role::Producer => "P",
        Role::Consumer => "C",
        Role::Sink => "S",
        Role::Source => "S",
        Role::Output => "O",
    }
}

impl<'a> PathFinder<'a> {
    pub fn new(p: &'a RouteProblem<'a>) -> Self {
        let plan = p.plan;
        let config = p.config;
        let tun = p.tuning.clone();
        let (w, h) = (p.placement.w, p.placement.h);
        let dims = GridDims::new(w, h, MARGIN);
        let n = dims.size;
        let fw = dims.full_w;
        let tier = gc::belt_tier(&config.belt_tier).expect("belt_tier is validated");
        let max_j = tier.max_underground_gap + 1;
        let ug_allowed = config.trick("B2");
        let mut no_tunnel = vec![false; n];
        for t in &p.no_tunnel {
            if dims.in_full(t.x, t.y) {
                no_tunnel[dims.idx_tile(*t)] = true;
            }
        }
        let weave = config.weave();
        let max_jw = weave.map_or(-1, |t| t.max_underground_gap) + 1;
        let placement = p.placement;

        // Box tiles are free unless a machine, obstacle or fluid tile sits there.
        let mut allow_net = vec![-2i32; n];
        for y in 0..h {
            for x in 0..w {
                allow_net[dims.idx(x, y)] = -1;
            }
        }
        let machine_rects: Vec<Rect> = placement.machines.iter().map(|m| placement.rect(plan, m)).collect();
        for r in &machine_rects {
            for t in r.tiles() {
                if dims.in_full(t.x, t.y) {
                    allow_net[dims.idx_tile(t)] = -2;
                }
            }
        }
        // Used fluid connection tiles hold a pipe end; belts never go there.
        for m in &placement.machines {
            for c in placement.connections(plan, m).iter().filter(|c| c.fluid.is_some()) {
                if dims.in_full(c.tile.x, c.tile.y) {
                    allow_net[dims.idx_tile(c.tile)] = -2;
                }
            }
        }
        for t in &p.obstacles {
            if dims.in_full(t.x, t.y) {
                allow_net[dims.idx_tile(*t)] = -2;
            }
        }
        let mut bias = vec![0f32; n];
        for (t, v) in &p.tile_bias {
            if dims.in_full(t.x, t.y) {
                bias[dims.idx_tile(*t)] = *v as f32;
            }
        }
        let mut hist = vec![0f32; n];
        if let Some(warm) = &p.warm_history {
            let k = warm.len().min(n);
            hist[..k].copy_from_slice(&warm[..k]);
        }
        let mut pin_count = vec![0i32; n];
        for r in &machine_rects {
            for side in Dir::ALL {
                for t in r.side_tiles(side, 1) {
                    if dims.in_full(t.x, t.y) {
                        pin_count[dims.idx_tile(t)] += 1;
                    }
                }
            }
        }

        // Direct insertion claims gap tiles before anything else.
        let di = {
            let free = |t: Tile| dims.in_box(t.x, t.y) && allow_net[dims.idx_tile(t)] == -1;
            assign_direct_insertion(plan, placement, config, &free)
        };
        for link in &di {
            for t in &link.tiles {
                allow_net[dims.idx_tile(*t)] = -2;
            }
        }
        let eff = effective_terminals(plan, &di, config);

        let s = n * 8;
        let mut pf = PathFinder {
            p,
            plan,
            config,
            tun: tun.clone(),
            w,
            h,
            dims,
            n,
            fw,
            tier,
            max_j,
            ug_allowed,
            no_tunnel,
            weave,
            max_jw,
            ch: 4 * n,
            weave_ok: false,
            log: Vec::new(),
            trace: Vec::new(),
            fail_renders: 0,
            d_idx: [-fw, 1, fw, -1],
            d_x: [0, 1, 0, -1],
            d_y: [-1, 0, 1, 0],
            allow_net,
            pin_count,
            bias,
            eff,
            di,
            vts: Vec::new(),
            term_net: Vec::new(),
            term_k: Vec::new(),
            need: Vec::new(),
            term_opts: Vec::new(),
            sink_k: HashMap::new(),
            net_order: Vec::new(),
            order: None,
            leg_target: -1,
            options: Vec::new(),
            opt_belt: Vec::new(),
            opt_ins: Vec::new(),
            opt_term: Vec::new(),
            opt_long: Vec::new(),
            at_belt_start: Vec::new(),
            at_belt_list: Vec::new(),
            sink_options: JMap::new(),
            nets: Rc::new(Vec::new()),
            occ_tile: vec![0; n],
            occ_belt: vec![0; n],
            dead_end_at: vec![0; n],
            dead_end_dir: vec![0; n * 4],
            belt_accept: vec![0; n * 4],
            occ_chan: vec![0; 4 * n],
            hist,
            hist_chan: vec![0f32; 4 * n],
            p_fac: tun.p_fac_first as f32,
            routed: Vec::new(),
            sink_claims: Vec::new(),
            own_stamp: vec![0; n],
            own_belt_stamp: vec![0; n],
            own_chan_stamp: vec![0; 4 * n],
            own_dir: vec![0; n],
            output_leg: false,
            out_dir_at: vec![0; n],
            branch_from: HashMap::new(),
            stamp: 0,
            cur: -1,
            s_states: s as i32,
            g_arr: vec![0f32; s],
            par: vec![0; s],
            seen_gen: vec![0; s],
            closed_gen: vec![0; s],
            search_gen: 0,
            heap: MinHeap::new(4096),
            dist: vec![0; n],
            goal_stamp: vec![0; n],
            goal_gen: 0,
            res_stamp: vec![0; n],
            start_stamp: vec![0; n],
            path_stamp: vec![0; n],
            path_first: vec![0; n],
            forbid_stamp: vec![0; s],
            forbid_gen: 0,
            path_chan_stamp: vec![0; 2 * n],
            path_gen: 0,
            start_opt: vec![-1; n],
            done_state: Vec::new(),
            done_opt: Vec::new(),
            min_step: 0.0,
            h_weight: tun.heuristic_weight as f32,
            expansions: 0,
            surplus: [0.0; 2],
            paired: false,
            lane_load: [0.0; 2],
            pipes: None,
        };
        pf.build_chains();
        if config.lane_sharing {
            pf.pair_chains();
        }
        pf.build_options();
        pf
    }

    /// The chains the router built, their terminals and the access options of those: its setup, before any routing.
    pub fn nets(&self) -> &[RNet] {
        &self.nets
    }

    pub fn chain_terminals(&self) -> &[Vt] {
        &self.vts
    }

    pub fn options(&self) -> &[AccessOption] {
        &self.options
    }

    /// Access options of the sink terminals, by terminal id.
    pub fn sink_options(&self) -> Vec<(usize, &Vec<AccessOption>)> {
        self.sink_options.iter_java().map(|(k, v)| (*k, v)).collect()
    }

    pub(super) fn is_ring1(&self, t: Tile) -> bool {
        !self.dims.in_box(t.x, t.y) && t.x >= -1 && t.y >= -1 && t.x <= self.w && t.y <= self.h
    }

    // ---- cost functions ----
    pub(super) fn placeable(&self, q: i32) -> bool {
        let a = self.allow_net[q as usize];
        (a == -1 || a == self.cur) && self.own_stamp[q as usize] != self.stamp
    }

    /// Cost of a belt-family entity on `q` that would take items from `dead_taken` chain ends pointing into it.
    pub(super) fn belt_cost(&self, q: i32, dead_taken: i32) -> f32 {
        let q = q as usize;
        (self.tun.belt_base as f32 + self.hist[q]) * (1f32 + self.p_fac * (self.occ_tile[q] + dead_taken) as f32)
            + (if self.pin_count[q] > 0 { self.tun.pin_reservation as f32 } else { 0f32 })
            + self.bias[q]
    }

    pub(super) fn ins_cost(&self, q: i32, long: bool) -> f32 {
        let q = q as usize;
        ((if long { self.tun.long_inserter_base } else { self.tun.inserter_base }) as f32 + self.hist[q]) * (1f32 + self.p_fac * self.occ_tile[q] as f32)
    }

    pub(super) fn chest_cost(&self, q: i32) -> f32 {
        let q = q as usize;
        (self.tun.chest_base as f32 + self.hist[q]) * (1f32 + self.p_fac * self.occ_tile[q] as f32)
    }

    pub(super) fn chan_cost(&self, c: i32) -> f32 {
        let c = c as usize;
        self.hist_chan[c] + (1f32 + self.hist_chan[c]) * self.p_fac * self.occ_chan[c] as f32
    }

    /// Cost of a chain end pointing into `t2` in direction `d`: only belts there that would take its items count.
    pub(super) fn dead_end_cost(&self, t2: i32, d: i32) -> f32 {
        if t2 < 0 || t2 as usize >= self.n {
            return 0f32;
        }
        let a = self.belt_accept[(t2 * 4 + d) as usize];
        if a == 0 { 0f32 } else { (1f32 + self.hist[t2 as usize]) * self.p_fac * a as f32 * self.tun.dead_end_conflict as f32 }
    }

    // ---- which way belt-family entities take items (the validator's rule) ----
    /// Directions of travel a belt (or underground entrance) facing `h` takes items from, as a bit mask.
    pub(super) fn accept_mask(kind: i32, h: i32) -> i32 {
        match kind {
            AK_UG_OUT => (1 << ((h + 1) & 3)) | (1 << ((h + 3) & 3)), // sides only
            AK_SPLITTER => 1 << h,                                    // from behind only
            _ => 0xF & !(1 << ((h + 2) & 3)),                         // anything but head-on
        }
    }

    pub(super) fn accept(&mut self, t: i32, kind: i32, h: i32, delta: i32) {
        let m = Self::accept_mask(kind, h);
        for d in 0..4 {
            if (m >> d) & 1 == 1 {
                self.belt_accept[(t * 4 + d) as usize] += delta;
            }
        }
    }

    /// Chain ends that an entity of `kind` facing `h` on `q` would take items from.
    pub(super) fn dead_ends_taken(&self, q: i32, kind: i32, h: i32) -> i32 {
        let m = Self::accept_mask(kind, h);
        let mut c = 0;
        for d in 0..4 {
            if (m >> d) & 1 == 1 {
                c += self.dead_end_dir[(q * 4 + d) as usize];
            }
        }
        c
    }

    pub(super) fn add_dead_end(&mut self, r: &mut Routed, t2: i32, d: i32) {
        r.dead_ends.push(t2 * 4 + d);
        self.dead_end_dir[(t2 * 4 + d) as usize] += 1;
        self.dead_end_at[t2 as usize] += 1;
    }

    pub(super) fn remove_dead_end(&mut self, e: i32) {
        self.dead_end_dir[e as usize] -= 1;
        self.dead_end_at[(e >> 2) as usize] -= 1;
    }

    /// Chain ends pointing into `t` that a belt there would take items from.
    pub(super) fn dead_end_conflicts(&self, t: usize) -> i32 {
        let mut c = 0;
        for d in 0..4 {
            if self.belt_accept[t * 4 + d] > 0 {
                c += self.dead_end_dir[t * 4 + d];
            }
        }
        c
    }

    // ---- state encoding ----
    pub(super) fn st(tile: i32, dir: i32, kind: i32) -> i32 {
        (((tile << 2) | dir) << 1) | kind
    }

    // ---- the iteration loop ----
    pub(super) fn overuse_at(&self, t: usize) -> i32 {
        (self.occ_tile[t] - 1).max(0) + self.dead_end_conflicts(t) + self.pipes.as_ref().map_or(0, |p| p.adjacency_overuse(t))
    }

    pub(super) fn total_overuse(&self) -> i32 {
        let mut o = 0;
        for t in 0..self.n {
            o += self.overuse_at(t);
        }
        for c in 0..self.ch {
            o += (self.occ_chan[c] - 1).max(0);
        }
        o + self.pipes.as_ref().map_or(0, |p| p.channel_overuse())
    }

    fn touches_overuse(&self, i: usize) -> bool {
        let Some(r) = &self.routed[i] else { return true };
        r.tiles.iter().any(|&t| self.overuse_at(t as usize) > 0)
            || r.chans.iter().any(|&c| self.occ_chan[c as usize] > 1)
            || r.dead_ends.iter().any(|&e| self.belt_accept[e as usize] > 0)
    }

    /// Belt chains are `w >= 0`, fluid nets `-1 - i`.
    fn ok(&self, w: i32) -> bool {
        if w >= 0 { self.routed[w as usize].as_ref().is_some_and(|r| r.ok) } else { self.pipes.as_ref().expect("a fluid net").ok((-1 - w) as usize) }
    }

    fn net_overused(&self, w: i32) -> bool {
        !self.ok(w)
            || if w >= 0 {
                self.touches_overuse(w as usize)
            } else {
                self.pipes.as_ref().expect("a fluid net").touches_overuse((-1 - w) as usize, &|t| self.overuse_at(t))
            }
    }

    /// Rips up chain or fluid net `w`.
    fn rip_up_net(&mut self, w: i32) {
        if w >= 0 {
            self.rip_up(w as usize);
        } else {
            self.with_pipes(|pr, pf| pr.rip_up(pf, (-1 - w) as usize));
        }
    }

    fn reroute(&mut self, w: i32, nets: &[RNet]) {
        self.rip_up_net(w);
        if w >= 0 {
            self.route_net(&nets[w as usize]);
        } else {
            self.with_pipes(|pr, pf| pr.route(pf, (-1 - w) as usize));
        }
    }

    /// Runs `f` on the pipe router and the rest of the router, which the Kotlin `PipeRouter` reaches through a back pointer.
    pub(super) fn with_pipes<R>(&mut self, f: impl FnOnce(&mut PipeRouter, &mut PathFinder<'a>) -> R) -> R {
        let mut pr = self.pipes.take().expect("pipes mode");
        let r = f(&mut pr, self);
        self.pipes = Some(pr);
        r
    }

    fn unrouted(&self, nets: &[RNet]) -> Vec<i32> {
        nets.iter().filter(|n| !self.routed[n.idx as usize].as_ref().is_some_and(|r| r.ok)).map(|n| n.idx).collect()
    }

    fn failed_fluids(&self) -> Vec<i32> {
        self.pipes.as_ref().map_or_else(Vec::new, |p| p.failed())
    }

    pub fn run(&mut self) -> RouteResult {
        let t0 = now_ms();
        if self.config.fluids.mode == "pipes" {
            self.pipes = Some(PipeRouter::new(self));
        }
        let nets = Rc::clone(&self.nets);
        let tun = self.tun.clone();
        let pipe_count = self.pipes.as_ref().map_or(0, |p| p.count());
        // Belt chains (>= 0) and fluid nets (encoded as -1 - i), longest first (a stable sort).
        let mut ranked: Vec<(i32, i32)> = nets.iter().map(|n| (n.idx, n.hpwl)).collect();
        for i in 0..pipe_count {
            ranked.push((-1 - i as i32, self.pipes.as_ref().expect("pipes").hpwl(i)));
        }
        ranked.sort_by_key(|r| std::cmp::Reverse(r.1));
        let order: Vec<i32> = ranked.into_iter().map(|r| r.0).collect();

        let mut best = i32::MAX;
        let mut since_best = 0;
        // Routes of the best iteration: negotiation can wander off from it, and the endgame starts there.
        let mut best_routes: Option<Vec<Option<Routed>>> = None;
        let mut best_pipes: Option<Vec<Option<PRouted>>> = None;
        // History as it stood at the best iteration: later detours smear it away from the real hotspots.
        let mut hist_best = self.hist.clone();
        let mut hist_chan_best = self.hist_chan.clone();
        let mut iter = 0;
        let mut overuse = i32::MAX;
        let mut unrouted: Vec<i32> = nets.iter().map(|n| n.idx).collect();
        let mut failed_fluids: Vec<i32> = Vec::new();
        while iter < tun.max_iterations {
            iter += 1;
            let todo: Vec<i32> = if iter <= tun.full_reroute_iterations { order.clone() } else { order.iter().copied().filter(|&w| self.net_overused(w)).collect() };
            for &w in &todo {
                self.reroute(w, &nets);
            }
            self.solve_sinks();
            unrouted = self.unrouted(&nets);
            failed_fluids = self.failed_fluids();
            overuse = self.total_overuse();
            let score = overuse + tun.unrouted_penalty * (unrouted.len() + failed_fluids.len()) as i32;
            self.trace.push(score);
            if tun.trace_detail || self.p.debug_dir.is_some() {
                self.log_iteration_detail(iter, unrouted.len(), &nets);
            }
            self.render_iteration(iter, overuse, unrouted.len());
            if overuse == 0 && unrouted.is_empty() && failed_fluids.is_empty() {
                break;
            }
            if score < best {
                best = score;
                since_best = 0;
                best_routes = Some(self.routed.clone());
                best_pipes = self.pipes.as_ref().map(|pr| (0..pr.count()).map(|i| pr.saved(i)).collect());
                hist_best.copy_from_slice(&self.hist);
                hist_chan_best.copy_from_slice(&self.hist_chan);
            } else {
                since_best += 1;
                if since_best >= tun.stagnation_stop {
                    break;
                }
            }
            // History and present-congestion factor.
            for t in 0..self.n {
                let o = self.overuse_at(t);
                if o > 0 {
                    self.hist[t] += (tun.h_fac * f64::from(o)) as f32;
                }
            }
            for c in 0..self.ch {
                let o = self.occ_chan[c] - 1;
                if o > 0 {
                    self.hist_chan[c] += (tun.h_fac * f64::from(o)) as f32;
                }
            }
            if let Some(pr) = &mut self.pipes {
                pr.update_history();
            }
            self.p_fac = (self.p_fac * tun.p_mult as f32).min(tun.p_fac_max as f32);
        }
        let penalty = |unrouted: usize, fluids: usize| tun.unrouted_penalty * (unrouted + fluids) as i32;
        if let Some(best_routes) = best_routes.filter(|_| overuse.wrapping_add(penalty(unrouted.len(), failed_fluids.len())) > best) {
            for i in 0..nets.len() {
                self.rip_up(i);
            }
            for i in 0..pipe_count {
                self.with_pipes(|pr, pf| pr.rip_up(pf, i));
            }
            for (i, route) in best_routes.into_iter().enumerate() {
                self.restore(i, route);
            }
            if let Some(best_pipes) = best_pipes {
                for (i, saved) in best_pipes.into_iter().enumerate() {
                    self.with_pipes(|pr, pf| pr.restore(pf, i, saved));
                }
            }
            self.solve_sinks();
            unrouted = self.unrouted(&nets);
            failed_fluids = self.failed_fluids();
            overuse = self.total_overuse();
            self.log.push(format!("back to the best iteration: overuse {overuse}, unrouted {}", unrouted.len() + failed_fluids.len()));
        }
        if (overuse > 0 || !unrouted.is_empty() || !failed_fluids.is_empty())
            && overuse <= tun.endgame_max_overuse.max((tun.endgame_overuse_per_chain * nets.len() as f64) as i32)
            && (unrouted.len() + failed_fluids.len()) as i32 <= tun.endgame_max_unrouted
        {
            let before = format!("overuse {overuse}, unrouted {}", unrouted.len() + failed_fluids.len());
            let negotiated = now_ms() - t0;
            let deadline = now_ms() + (negotiated as f64 * tun.endgame_time_factor) as i64;
            let rounds = self.endgame(&order, deadline, &nets);
            unrouted = self.unrouted(&nets);
            failed_fluids = self.failed_fluids();
            overuse = self.total_overuse();
            self.log.push(format!("endgame from {before}: {rounds} rounds, now overuse {overuse}, unrouted {}", unrouted.len() + failed_fluids.len()));
        }
        let success = overuse == 0 && unrouted.is_empty() && failed_fluids.is_empty();
        if !success {
            self.log_leftovers(&nets);
        }
        // Congestion map for the placer: history at the best iteration plus the conflicts still left.
        let mut history: JMap<Tile, f64> = JMap::new();
        for y in 0..self.h {
            for x in 0..self.w {
                let i = self.dims.idx(x, y);
                let mut left = self.overuse_at(i);
                let mut hc = 0f32;
                for k in 0..4 {
                    left += (self.occ_chan[k * self.n + i] - 1).max(0);
                    hc += hist_chan_best[k * self.n + i];
                }
                let v = f64::from(hist_best[i] + hc) + tun.feedback_overuse_weight * f64::from(left);
                if v > 0.0 {
                    history.insert(Tile::new(x, y), v);
                }
            }
        }
        let ents = self.entities();
        let (conflicts, conflict_nets) = if success { (Vec::new(), IndexMap::new()) } else { self.conflicts(&nets) };
        let mut sources: Vec<SourceSpec> = Vec::new();
        for n in nets.iter() {
            let Some(r) = self.routed[n.idx as usize].as_ref().filter(|r| r.source_tile >= 0) else { continue };
            let t = self.dims.tile(r.source_tile as usize);
            match &n.item2 {
                None => sources.push(SourceSpec { tile: t, item: n.item.clone(), rate: n.source_rate, fluid: false, lane: -1, port: n.src_port }),
                Some(item2) => {
                    if n.source_rate > 0.0 {
                        sources.push(SourceSpec { tile: t, item: n.item.clone(), rate: n.source_rate, fluid: false, lane: 0, port: -1 });
                    }
                    if n.source_rate2 > 0.0 {
                        sources.push(SourceSpec { tile: t, item: item2.clone(), rate: n.source_rate2, fluid: false, lane: 1, port: -1 });
                    }
                }
            }
        }
        if let Some(pr) = &self.pipes {
            sources.extend(pr.sources());
        }
        let mut outputs: Vec<OutputSpec> = Vec::new();
        for n in nets.iter() {
            if let Some(r) = self.routed[n.idx as usize].as_ref().filter(|r| r.out_tile >= 0) {
                outputs.push(OutputSpec { tile: self.dims.tile(r.out_tile as usize), item: n.item.clone(), rate: n.out_rate, fluid: false, port: n.out_port });
            }
        }
        if let Some(pr) = &self.pipes {
            outputs.extend(pr.outputs());
        }
        let chain_stats = self.chain_stats(&nets);
        let stats = RouteStats {
            belts: ents.iter().filter(|e| e.kind == Kind::Belt).count(),
            undergrounds: ents.iter().filter(|e| e.kind == Kind::UgIn).count(),
            inserters: ents.iter().filter(|e| e.kind == Kind::Inserter).count(),
        };
        let millis = now_ms() - t0;
        let mut log = std::mem::take(&mut self.log);
        log.push(format!("trace [{}]", self.trace.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")));
        RouteResult {
            success,
            entities: ents,
            iterations: iter,
            overuse: if overuse == i32::MAX { -1 } else { overuse },
            history,
            raw_history: self.hist.clone(),
            failed_nets: unrouted.iter().map(|&i| nets[i as usize].plan_id).chain(failed_fluids.iter().copied()).collect(),
            sources,
            outputs,
            conflicts,
            conflict_nets,
            di: self.di.clone(),
            stats,
            chains: nets.len(),
            chain_stats,
            millis,
            log,
            trace: self.trace.clone(),
            expansions: self.expansions,
        }
    }

    /// With a debug dir: this iteration's picture, the history cost as heat and the overused tiles in red.
    fn render_iteration(&self, iter: i32, overuse: i32, unrouted: usize) {
        let Some(dir) = &self.p.debug_dir else { return };
        let heat: Vec<(Tile, f64)> = (0..self.n).filter(|&t| self.hist[t] > 0.0).map(|t| (self.dims.tile(t), f64::from(self.hist[t]))).collect();
        let overused: HashSet<Tile> = (0..self.n).filter(|&t| self.overuse_at(t) > 0).map(|t| self.dims.tile(t)).collect();
        let options = RenderOptions { heat: Some(HeatOverlay { values: heat, overused }), title: Some(format!("iteration {iter}: overuse {overuse}, unrouted {unrouted}")), ..RenderOptions::default() };
        self.render_debug(&dir.join(format!("route_iter_{iter:03}.png")), &options);
    }

    /// Draws the layout as it stands; a picture that cannot be written is a warning, never a failed route.
    pub(super) fn render_debug(&self, file: &Path, options: &RenderOptions) {
        let layout = Layout::new(self.w, self.h, self.entities());
        if let Err(e) = render_layout(&layout, file, options) {
            eprintln!("warning: could not draw {}: {e:#}", file.display());
        }
    }

    /// The per-iteration breakdown of `trace_detail`: what collides with what.
    fn log_iteration_detail(&mut self, iter: i32, unrouted: usize, nets: &[RNet]) {
        let (mut tile_o, mut dead_o, mut chan_o) = (0, 0, 0);
        for t in 0..self.n {
            tile_o += (self.occ_tile[t] - 1).max(0);
            dead_o += self.dead_end_conflicts(t);
        }
        for c in 0..self.ch {
            chan_o += (self.occ_chan[c] - 1).max(0);
        }
        let belt_tiles: usize = nets.iter().map(|n| self.routed[n.idx as usize].as_ref().map_or(0, |r| r.belts.len() + r.ug_in.len() + r.ug_out.len())).sum();
        // What collides: belt/belt, belt/inserter, inserter/inserter, anything with a sink, pin tiles.
        let (mut bb, mut bi, mut ii, mut sk, mut pins) = (0, 0, 0, 0, 0);
        let mut sink_tiles: HashSet<usize> = HashSet::new();
        for (o, c) in &self.sink_claims {
            sink_tiles.insert(self.dims.idx_tile(o.ins_tile));
            sink_tiles.insert(self.dims.idx_tile(*c));
        }
        for t in 0..self.n {
            if self.occ_tile[t] > 1 {
                let ins = self.occ_tile[t] - self.occ_belt[t];
                if sink_tiles.contains(&t) {
                    sk += 1;
                } else if self.occ_belt[t] >= 2 && ins == 0 {
                    bb += 1;
                } else if self.occ_belt[t] >= 1 && ins >= 1 {
                    bi += 1;
                } else {
                    ii += 1;
                }
                if self.pin_count[t] > 0 {
                    pins += 1;
                }
            }
        }
        self.log.push(format!(
            "iter {iter}: tile {tile_o} (belt/belt {bb}, belt/ins {bi}, ins/ins {ii}, sink {sk}, on pin tiles {pins}) dead {dead_o} chan {chan_o} unrouted {unrouted} beltTiles {belt_tiles} pFac {:?}",
            self.p_fac
        ));
    }

    /// Why a failed route failed: the first overused tiles and channels and who uses them.
    fn log_leftovers(&mut self, nets: &[RNet]) {
        let mut reported = 0;
        for t in 0..self.n {
            if self.overuse_at(t) > 0 && reported < 12 {
                reported += 1;
                let ti = t as i32;
                let mut users: Vec<String> = nets
                    .iter()
                    .filter(|n| self.routed[n.idx as usize].as_ref().is_some_and(|r| r.tiles.contains(&ti)))
                    .map(|n| format!("net{}/{}", n.plan_id, n.idx))
                    .collect();
                users.extend(self.sink_claims.iter().filter(|(o, c)| self.dims.idx_tile(o.ins_tile) == t || self.dims.idx_tile(*c) == t).map(|(o, _)| format!("sink{}", o.terminal)));
                users.extend(
                    nets.iter()
                        .filter(|n| self.routed[n.idx as usize].as_ref().is_some_and(|r| r.dead_ends.iter().any(|&e| (e >> 2) as usize == t)))
                        .map(|n| format!("deadend{}/{}", n.plan_id, n.idx)),
                );
                self.log.push(format!(
                    "overused {}: occ {} belt {} deadEnd {} users [{}]",
                    self.dims.tile(t),
                    self.occ_tile[t],
                    self.occ_belt[t],
                    self.dead_end_at[t],
                    users.join(", ")
                ));
            }
        }
        for c in 0..self.ch {
            if self.occ_chan[c] > 1 {
                let ci = c as i32;
                let users: Vec<String> = nets
                    .iter()
                    .filter(|n| self.routed[n.idx as usize].as_ref().is_some_and(|r| r.chans.contains(&ci)))
                    .map(|n| format!("net{}/{}x{}", n.plan_id, n.idx, self.routed[n.idx as usize].as_ref().expect("filtered").chans.iter().filter(|&&x| x == ci).count()))
                    .collect();
                self.log.push(format!(
                    "overused channel {}axis {} at {}: {} users [{}] hist {:?}",
                    if c >= 2 * self.n { "weave " } else { "" },
                    (c / self.n) % 2,
                    self.dims.tile(c % self.n),
                    self.occ_chan[c],
                    users.join(", "),
                    self.hist_chan[c]
                ));
            }
        }
    }

    /// Tiles in trouble, for the trial's spreading, and the plan nets at each of the first 40.
    fn conflicts(&self, nets: &[RNet]) -> (Vec<Tile>, IndexMap<Tile, Vec<i32>>) {
        let mut c: JSet<usize> = JSet::new();
        for t in 0..self.n {
            if self.overuse_at(t) > 0 {
                c.insert(t);
            }
        }
        for ch in 0..self.ch {
            if self.occ_chan[ch] > 1 {
                c.insert(ch % self.n);
            }
        }
        if let Some(pr) = &self.pipes {
            for t in pr.channel_conflicts() {
                c.insert(t as usize);
            }
        }
        for n in nets.iter().filter(|n| !self.routed[n.idx as usize].as_ref().is_some_and(|r| r.ok)) {
            for m in self.terminal_machines(n) {
                c.insert(self.dims.idx_tile(self.machine_center_of(m)));
            }
        }
        let conflicts: Vec<Tile> = c.iter_java().map(|&t| self.dims.tile(t)).collect();

        let mut m: IndexMap<Tile, Vec<i32>> = IndexMap::new();
        for t in 0..self.n {
            if self.overuse_at(t) > 0 && m.len() < 40 {
                let ti = t as i32;
                let mut ids: JSet<i32> = JSet::new();
                for n in nets.iter() {
                    if let Some(r) = &self.routed[n.idx as usize]
                        && (r.tiles.contains(&ti) || r.dead_ends.iter().any(|&e| (e >> 2) as usize == t))
                    {
                        ids.insert(n.plan_id);
                    }
                }
                if let Some(pr) = &self.pipes {
                    for id in pr.plan_nets_near(t) {
                        ids.insert(id);
                    }
                }
                m.insert(self.dims.tile(t), ids.iter_java().copied().collect());
            }
        }
        for ch in 0..self.ch {
            if self.occ_chan[ch] > 1 && m.len() < 40 {
                let key = self.dims.tile(ch % self.n);
                if !m.contains_key(&key) {
                    let ci = ch as i32;
                    let mut ids: Vec<i32> = Vec::new();
                    for n in nets.iter().filter(|n| self.routed[n.idx as usize].as_ref().is_some_and(|r| r.chans.contains(&ci))) {
                        if !ids.contains(&n.plan_id) {
                            ids.push(n.plan_id);
                        }
                    }
                    m.insert(key, ids);
                }
            }
        }
        (conflicts, m)
    }

    /// The distinct machines of a chain's terminals, in terminal order.
    fn terminal_machines(&self, n: &RNet) -> Vec<usize> {
        let mut out: Vec<usize> = Vec::new();
        for &t in &n.terms {
            if let Some(m) = self.vts[t as usize].machine()
                && !out.contains(&m)
            {
                out.push(m);
            }
        }
        out
    }

    fn chain_stats(&self, nets: &[RNet]) -> Vec<ChainStat> {
        let mut out = Vec::new();
        for n in nets {
            let Some(r) = self.routed[n.idx as usize].as_ref().filter(|r| r.ok) else { continue };
            let mut pts: Vec<Tile> = self.terminal_machines(n).into_iter().map(|m| self.machine_center_of(m)).collect();
            if r.source_tile >= 0 {
                pts.push(self.dims.tile(r.source_tile as usize));
            }
            let hp = (pts.iter().map(|t| t.x).max().expect("a chain has points") - pts.iter().map(|t| t.x).min().expect("a chain has points"))
                + (pts.iter().map(|t| t.y).max().expect("a chain has points") - pts.iter().map(|t| t.y).min().expect("a chain has points"));
            // `distinct()` on the machines counts a terminal without a machine as one more.
            let mut machines: Vec<Option<usize>> = Vec::new();
            for &t in &n.terms {
                let m = self.vts[t as usize].machine();
                if !machines.contains(&m) {
                    machines.push(m);
                }
            }
            out.push(ChainStat { item: n.item.clone(), terminals: machines.len() + usize::from(!n.port_tiles.is_empty()), hpwl: hp, tiles: r.belts.len() + r.ug_in.len() + r.ug_out.len() });
        }
        out
    }

    // ---- endgame ----
    /// LNS endgame. Negotiation that stalls on a few conflicts keeps rerouting each net alone against the
    /// rest, and each comes back to the same tiles. Rip up every chain and pipe net near the conflicts,
    /// reroute them together in a shuffled order and negotiate among them for a few iterations; the region
    /// grows every few rounds that do not clear it. A round that ends worse than it started is undone.
    /// Returns the rounds used.
    fn endgame(&mut self, order: &[i32], deadline: i64, nets: &[RNet]) -> i32 {
        let tun = self.tun.clone();
        let mut rnd = JavaRandom::new(31 * self.n as i64 + nets.len() as i64 + 1_000_003 * self.p.endgame_seed);
        let pipe_count = self.pipes.as_ref().map_or(0, |p| p.count());
        let score = |pf: &Self| {
            pf.total_overuse()
                + tun.unrouted_penalty
                    * (nets.iter().filter(|n| !pf.routed[n.idx as usize].as_ref().is_some_and(|r| r.ok)).count() + pf.pipes.as_ref().map_or(0, |p| p.failed().len())) as i32
        };
        let mut round = 0;
        while round < tun.endgame_rounds && now_ms() < deadline {
            let start = score(self);
            let seed_tiles = self.endgame_seeds(nets, pipe_count);
            if seed_tiles.is_empty() {
                break;
            }
            round += 1;
            let r = tun.endgame_radius + (round - 1) / 4 * 2;
            let mut region = vec![false; self.n];
            for &t in &seed_tiles {
                let (x0, y0) = (self.dims.x(t as usize), self.dims.y(t as usize));
                for y in y0 - r..=y0 + r {
                    for x in x0 - r..=x0 + r {
                        if self.dims.in_full(x, y) {
                            region[self.dims.idx(x, y)] = true;
                        }
                    }
                }
            }
            let mut victims: Vec<i32> = order
                .iter()
                .copied()
                .filter(|&w| {
                    !self.ok(w)
                        || if w >= 0 {
                            let rr = self.routed[w as usize].as_ref().expect("an ok net is routed");
                            rr.tiles.iter().any(|&t| region[t as usize]) || rr.dead_ends.iter().any(|&e| region[(e >> 2) as usize])
                        } else {
                            self.pipes.as_ref().expect("a fluid net").touches_region((-1 - w) as usize, &region)
                        }
                })
                .collect();
            let saved: HashMap<i32, Saved> = victims
                .iter()
                .map(|&w| (w, if w >= 0 { Saved::Belt(self.routed[w as usize].clone()) } else { Saved::Pipe(self.pipes.as_ref().expect("a fluid net").saved((-1 - w) as usize)) }))
                .collect();
            for &w in &victims {
                self.rip_up_net(w);
            }
            java_shuffle(&mut victims, &mut rnd);
            for &w in &victims {
                if w >= 0 {
                    self.route_net(&nets[w as usize]);
                } else {
                    self.with_pipes(|pr, pf| pr.route(pf, (-1 - w) as usize));
                }
            }
            for _ in 0..tun.endgame_iterations {
                self.solve_sinks();
                let todo: Vec<i32> = victims.iter().copied().filter(|&w| self.net_overused(w)).collect();
                if todo.is_empty() {
                    break;
                }
                for t in 0..self.n {
                    let o = self.overuse_at(t);
                    if o > 0 {
                        self.hist[t] += (tun.h_fac * f64::from(o)) as f32;
                    }
                }
                for c in 0..self.ch {
                    let o = self.occ_chan[c] - 1;
                    if o > 0 {
                        self.hist_chan[c] += (tun.h_fac * f64::from(o)) as f32;
                    }
                }
                if let Some(pr) = &mut self.pipes {
                    pr.update_history();
                }
                let mut shuffled = todo;
                java_shuffle(&mut shuffled, &mut rnd);
                for w in shuffled {
                    self.reroute(w, nets);
                }
            }
            self.solve_sinks();
            if score(self) > start {
                for &w in &victims {
                    self.rip_up_net(w);
                }
                for &w in &victims {
                    match saved.get(&w).expect("every victim was saved") {
                        Saved::Belt(r) => self.restore(w as usize, r.clone()),
                        Saved::Pipe(r) => {
                            let r = r.clone();
                            self.with_pipes(|pr, pf| pr.restore(pf, (-1 - w) as usize, r));
                        }
                    }
                }
                self.solve_sinks();
            }
        }
        round
    }

    /// Tiles around which the endgame rips up: overused tiles and channels, unrouted machines and ports.
    fn endgame_seeds(&self, nets: &[RNet], pipe_count: usize) -> Vec<i32> {
        let mut s: Vec<i32> = Vec::new();
        for t in 0..self.n {
            if self.overuse_at(t) > 0 {
                s.push(t as i32);
            }
        }
        for c in 0..self.ch {
            if self.occ_chan[c] > 1 {
                s.push((c % self.n) as i32);
            }
        }
        if let Some(pr) = &self.pipes {
            s.extend(pr.channel_conflicts());
        }
        for n in nets.iter().filter(|n| !self.routed[n.idx as usize].as_ref().is_some_and(|r| r.ok)) {
            for m in self.terminal_machines(n) {
                s.push(self.dims.idx_tile(self.machine_center_of(m)) as i32);
            }
            if let Some(&t) = n.port_tiles.first() {
                s.push(t);
            }
        }
        if let Some(pr) = &self.pipes
            && !pr.failed().is_empty()
        {
            for i in (0..pipe_count).filter(|&i| !pr.ok(i)) {
                s.extend(pr.seed_tiles(i));
            }
        }
        // `distinct()`: first occurrences, in order.
        let mut seen: HashSet<i32> = HashSet::new();
        s.retain(|t| seen.insert(*t));
        s
    }
}

/// Plans, placement and tuning in, routed entities out.
pub fn route(problem: &RouteProblem) -> RouteResult {
    PathFinder::new(problem).run()
}
