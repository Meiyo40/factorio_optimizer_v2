//! Pipe routing for fluid nets, inside `PathFinder`'s negotiation (shared tile occupancy, history and present
//! cost). Each fluid net is a tree grown from its source (a port, or a producer's output connection) to every
//! machine connection that uses the fluid: A* from all tiles of the tree to the nearest unconnected
//! terminal. Pipes join every adjacent pipe, so a pipe next to another fluid's pipe is a priced conflict;
//! pipe-to-ground pairs join only along their axis and pass under anything.
//!
//! Ported from route/Pipes.kt. Kotlin's `PipeRouter` keeps a back pointer to its `PathFinder` and works in its
//! search arrays; here the methods that do take it as `pf`, and `PathFinder::with_pipes` hands it over. Two lookups
//! that Kotlin does per expansion are done once instead: the B2 trick (`pf.ug_allowed`) and the set of banned
//! states (a stamp array); the results are the same.

use super::path_finder::PathFinder;
use crate::config::tuning::RouteTuning;
use crate::data::game_constants as gc;
use crate::geom::{Dir, GridDims, Kind, PlacedEntity, Tile};
use crate::place::port_candidates;
use crate::plan::Role;
use crate::validate::{OutputSpec, SourceSpec};
use std::collections::HashMap;

/// A fluid net: terminal tile groups (reaching any tile of a group connects it) and optional port tiles.
#[derive(Clone)]
struct FNet {
    idx: usize,
    plan_id: i32,
    fluid: String,
    groups: Vec<Vec<i32>>,
    port_tiles: Vec<i32>,
    port_out: Vec<i32>,
    hpwl: i32,
    source_rate: f64,
    /// Output port tiles (surplus leaves the block here), reached last; empty without an output.
    out_tiles: Vec<i32>,
    out_rate: f64,
    /// Plan ports of the source and the output (-1: none).
    src_port: i32,
    out_port: i32,
}

#[derive(Clone)]
pub(super) struct PRouted {
    pipes: Vec<i32>,
    /// tile * 4 + open-side dir
    ptgs: Vec<i32>,
    chans: Vec<i32>,
    source_tile: i32,
    out_tile: i32,
    ok: bool,
}

impl PRouted {
    fn new() -> Self {
        Self { pipes: Vec::new(), ptgs: Vec::new(), chans: Vec::new(), source_tile: -1, out_tile: -1, ok: false }
    }
}

pub struct PipeRouter {
    n: usize,
    dims: GridDims,
    tun: RouteTuning,
    d_idx: [i32; 4],
    nets: Vec<FNet>,
    routed: Vec<Option<PRouted>>,
    /// -1: any fluid; -2: no pipe; n >= 0: only fluid net n (its machine connection tile or fixed port).
    conn_net: Vec<i32>,
    pipe_count: Vec<i32>,
    pipe_owner: Vec<i32>,
    /// Joinable pipe ends of other nets next to each tile.
    pipe_nbr: Vec<i32>,
    /// Open side of a pipe-to-ground end on the tile, or -1 for a plain pipe.
    ptg_open: Vec<i32>,
    occ_p_chan: Vec<i32>,
    hist_p_chan: Vec<f32>,
    own_stamp: Vec<i32>,
    stamp: i32,
    max_j: i32,
    /// Port entry tiles just outside the box: a search starting there takes one step inward (direction), else -1.
    entry_in: Vec<i32>,
    /// States the current leg may not use (they made an earlier attempt cross itself).
    banned_stamp: Vec<i32>,
    banned_gen: i32,
    tile_stamp: Vec<i32>,
    tile_gen: i32,
}

impl PipeRouter {
    pub(super) fn new(pf: &PathFinder) -> Self {
        let plan = pf.plan;
        let placement = pf.p.placement;
        let dims = pf.dims;
        let n = pf.n;
        let mut conn_net = vec![-1i32; n];
        let fluid_nets: Vec<_> = plan.nets.iter().filter(|net| net.fluid).collect();
        let mut net_of_fluid: HashMap<&str, i32> = HashMap::new();
        for (i, net) in fluid_nets.iter().enumerate() {
            net_of_fluid.insert(net.item.as_str(), i as i32);
        }
        // Connection tiles: the right fluid only; unused ones and fluids nobody routes take no pipe at all.
        for m in &placement.machines {
            for c in placement.connections(plan, m) {
                if !dims.in_full(c.tile.x, c.tile.y) {
                    continue;
                }
                conn_net[dims.idx_tile(c.tile)] = c.fluid.as_deref().and_then(|f| net_of_fluid.get(f).copied()).unwrap_or(-2);
            }
        }
        let mut nets: Vec<FNet> = Vec::new();
        for (idx, net) in fluid_nets.iter().enumerate() {
            let ts = plan.net_terminals(net);
            let groups: Vec<Vec<i32>> = ts
                .iter()
                .filter(|t| t.machine.is_some())
                .map(|t| {
                    let pm = placement.machines.iter().find(|m| Some(m.id) == t.machine).expect("a placed machine");
                    placement.connections(plan, pm).iter().filter(|c| c.fluid.as_deref() == Some(net.item.as_str())).map(|c| dims.idx_tile(c.tile) as i32).collect::<Vec<_>>()
                })
                .filter(|g| !g.is_empty())
                .collect();
            let source = ts.iter().find(|t| t.role == Role::Source);
            let mut port_tiles: Vec<i32> = Vec::new();
            let mut port_out: Vec<i32> = Vec::new();
            if let Some(source) = source {
                let cands: Vec<(Tile, Option<Dir>)> = port_candidates(&plan.ports[source.port.expect("a port terminal")].specs, pf.w, pf.h)
                    .into_iter()
                    .filter(|(t, _)| dims.in_full(t.x, t.y) && (dims.in_box(t.x, t.y) || (t.x >= -1 && t.y >= -1 && t.x <= pf.w && t.y <= pf.h)))
                    .collect();
                port_tiles = cands.iter().map(|(t, _)| dims.idx_tile(*t) as i32).collect();
                port_out = cands.iter().map(|(_, d)| d.map_or(-1, |d| d.ordinal() as i32)).collect();
                for (t, out) in &cands {
                    if out.is_none() {
                        conn_net[dims.idx_tile(*t)] = idx as i32;
                    }
                }
            }
            let output = ts.iter().find(|t| t.role == Role::Output);
            let mut out_tiles: Vec<i32> = Vec::new();
            if let Some(output) = output {
                let cands: Vec<(Tile, Option<Dir>)> = port_candidates(&plan.ports[output.port.expect("a port terminal")].specs, pf.w, pf.h)
                    .into_iter()
                    .filter(|(t, _)| dims.in_box(t.x, t.y))
                    .collect();
                out_tiles = cands.iter().map(|(t, _)| dims.idx_tile(*t) as i32).collect();
                for (t, out) in &cands {
                    if out.is_none() {
                        conn_net[dims.idx_tile(*t)] = idx as i32;
                    }
                }
            }
            let mut pts: Vec<Tile> = groups.iter().map(|g| dims.tile(g[0] as usize)).collect();
            pts.extend(port_tiles.iter().take(1).map(|&t| dims.tile(t as usize)));
            pts.extend(out_tiles.iter().take(1).map(|&t| dims.tile(t as usize)));
            let hpwl = if pts.is_empty() {
                0
            } else {
                let (xs, ys) = (pts.iter().map(|t| t.x), pts.iter().map(|t| t.y));
                (xs.clone().max().expect("not empty") - xs.min().expect("not empty")) + (ys.clone().max().expect("not empty") - ys.min().expect("not empty"))
            };
            nets.push(FNet {
                idx,
                plan_id: net.id as i32,
                fluid: net.item.clone(),
                groups,
                port_tiles,
                port_out,
                hpwl,
                source_rate: source.map_or(0.0, |s| s.rate),
                out_tiles,
                out_rate: output.map_or(0.0, |o| o.rate),
                src_port: source.and_then(|s| s.port).map_or(-1, |p| p as i32),
                out_port: output.and_then(|o| o.port).map_or(-1, |p| p as i32),
            });
        }
        let count = nets.len();
        PipeRouter {
            n,
            dims,
            tun: pf.tun.clone(),
            d_idx: pf.d_idx,
            nets,
            routed: vec![None; count],
            conn_net,
            pipe_count: vec![0; n],
            pipe_owner: vec![-1; n],
            pipe_nbr: vec![0; n],
            ptg_open: vec![-1; n],
            occ_p_chan: vec![0; 2 * n],
            hist_p_chan: vec![0f32; 2 * n],
            own_stamp: vec![0; n],
            stamp: 0,
            max_j: gc::PIPE_TO_GROUND_GAP + 1,
            entry_in: vec![-1; n],
            banned_stamp: vec![0; 5 * n],
            banned_gen: 0,
            tile_stamp: vec![0; n],
            tile_gen: 0,
        }
    }

    pub(super) fn count(&self) -> usize {
        self.nets.len()
    }

    // ---- placeability and costs ----
    fn pipe_ok(&self, pf: &PathFinder, q: i32, n: i32) -> bool {
        if q < 0 || q as usize >= self.n || self.own_stamp[q as usize] == self.stamp {
            return false;
        }
        let c = self.conn_net[q as usize];
        if c == n {
            return true;
        }
        c == -1 && pf.allow_net[q as usize] == -1
    }

    fn pipe_cost(&self, pf: &PathFinder, q: i32, joins: bool) -> f32 {
        let q = q as usize;
        (self.tun.belt_base as f32 + pf.hist[q]) * (1f32 + pf.p_fac * (pf.occ_tile[q] + if joins { self.pipe_nbr[q] } else { 0 }) as f32)
            + (if pf.pin_count[q] > 0 { self.tun.pin_reservation as f32 } else { 0f32 })
            + pf.bias[q]
    }

    fn chan_cost(&self, pf: &PathFinder, c: i32) -> f32 {
        let c = c as usize;
        self.hist_p_chan[c] + (1f32 + self.hist_p_chan[c]) * pf.p_fac * self.occ_p_chan[c] as f32
    }

    // ---- commit / rip-up ----
    fn in_grid(&self, u: i32) -> bool {
        u >= 0 && (u as usize) < self.n
    }

    fn add_pipe(&mut self, pf: &mut PathFinder, r: &mut PRouted, t: i32, n: i32) {
        let tu = t as usize;
        r.pipes.push(t);
        pf.occ_tile[tu] += 1;
        self.pipe_count[tu] += 1;
        self.pipe_owner[tu] = n;
        self.own_stamp[tu] = self.stamp;
        for d in 0..4 {
            let u = t + self.d_idx[d];
            if self.in_grid(u) {
                self.pipe_nbr[u as usize] += 1;
            }
        }
    }

    fn add_ptg(&mut self, pf: &mut PathFinder, r: &mut PRouted, t: i32, open: i32, n: i32) {
        let tu = t as usize;
        r.ptgs.push(t * 4 + open);
        pf.occ_tile[tu] += 1;
        self.pipe_count[tu] += 1;
        self.pipe_owner[tu] = n;
        self.own_stamp[tu] = self.stamp;
        self.ptg_open[tu] = open;
        let u = t + self.d_idx[open as usize];
        if self.in_grid(u) {
            self.pipe_nbr[u as usize] += 1;
        }
    }

    fn rip_up_routed(&mut self, pf: &mut PathFinder, r: &PRouted) {
        for &t in &r.pipes {
            let tu = t as usize;
            pf.occ_tile[tu] -= 1;
            self.pipe_count[tu] -= 1;
            for d in 0..4 {
                let u = t + self.d_idx[d];
                if self.in_grid(u) {
                    self.pipe_nbr[u as usize] -= 1;
                }
            }
        }
        for &e in &r.ptgs {
            let t = e >> 2;
            let tu = t as usize;
            pf.occ_tile[tu] -= 1;
            self.pipe_count[tu] -= 1;
            self.ptg_open[tu] = -1;
            let u = t + self.d_idx[(e & 3) as usize];
            if self.in_grid(u) {
                self.pipe_nbr[u as usize] -= 1;
            }
        }
        for &c in &r.chans {
            self.occ_p_chan[c as usize] -= 1;
        }
    }

    pub(super) fn rip_up(&mut self, pf: &mut PathFinder, i: usize) {
        if let Some(r) = self.routed[i].take() {
            self.rip_up_routed(pf, &r);
        }
    }

    /// Net `i`'s current route, for putting it back with `restore`.
    pub(super) fn saved(&self, i: usize) -> Option<PRouted> {
        self.routed[i].clone()
    }

    /// Puts a route returned by `saved` back after the net was ripped up.
    pub(super) fn restore(&mut self, pf: &mut PathFinder, i: usize, saved: Option<PRouted>) {
        let Some(r) = saved else { return };
        for &t in &r.pipes {
            let tu = t as usize;
            pf.occ_tile[tu] += 1;
            self.pipe_count[tu] += 1;
            self.pipe_owner[tu] = i as i32;
            for d in 0..4 {
                let u = t + self.d_idx[d];
                if self.in_grid(u) {
                    self.pipe_nbr[u as usize] += 1;
                }
            }
        }
        for &e in &r.ptgs {
            let t = e >> 2;
            let tu = t as usize;
            pf.occ_tile[tu] += 1;
            self.pipe_count[tu] += 1;
            self.pipe_owner[tu] = i as i32;
            self.ptg_open[tu] = e & 3;
            let u = t + self.d_idx[(e & 3) as usize];
            if self.in_grid(u) {
                self.pipe_nbr[u as usize] += 1;
            }
        }
        for &c in &r.chans {
            self.occ_p_chan[c as usize] += 1;
        }
        self.routed[i] = Some(r);
    }

    // ---- A* over (tile, k): k = 0 a pipe, k = 1..4 a pipe-to-ground exit whose open side is direction k - 1 ----
    fn st(t: i32, k: i32) -> i32 {
        t * 5 + k
    }

    /// Grows net `n` from `sources` (pipe tiles, cost 0 or given) to the nearest tile in `goal`; returns the end state or -1.
    fn search(&mut self, pf: &mut PathFinder, n: &FNet, sources: &[(i32, f32)], goal: &[i32]) -> i32 {
        pf.search_gen += 1;
        pf.heap.clear();
        pf.goal_gen += 1;
        for &g in goal {
            pf.goal_stamp[g as usize] = pf.goal_gen;
        }
        pf.distance_transform();
        let hw = pf.h_weight * f32::min(1f32, (2f32 * self.tun.belt_base as f32 + self.tun.ug_penalty as f32) / (self.max_j + 1) as f32);
        for &(t, g) in sources {
            let s = Self::st(t, 0) as usize;
            pf.seen_gen[s] = pf.search_gen;
            pf.g_arr[s] = g;
            pf.par[s] = -1;
            pf.heap.push(g + hw * pf.dist[t as usize] as f32, s as i32);
        }
        let n_i = self.n as i32;
        let mut budget = self.tun.max_expansions;
        while !pf.heap.is_empty() {
            let s = pf.heap.pop();
            let su = s as usize;
            if pf.closed_gen[su] == pf.search_gen {
                continue;
            }
            pf.closed_gen[su] = pf.search_gen;
            budget -= 1;
            if budget < 0 {
                return -1;
            }
            let t = s / 5;
            let k = s % 5;
            let g_s = pf.g_arr[su];
            if k == 0 && pf.goal_stamp[t as usize] == pf.goal_gen && pf.par[su] != -1 {
                return s;
            }
            let dirs: std::ops::RangeInclusive<i32> = if k != 0 {
                (k - 1)..=(k - 1)
            } else if pf.par[su] == -1 && self.entry_in[t as usize] >= 0 {
                self.entry_in[t as usize]..=self.entry_in[t as usize]
            } else {
                0..=3
            };
            for d in dirs {
                let q = t + pf.d_idx[d as usize];
                if !self.pipe_ok(pf, q, n.idx as i32) {
                    continue;
                }
                let cost = g_s + self.pipe_cost(pf, q, true);
                self.relax(pf, Self::st(q, 0), s, cost, hw);
                // Pipe-to-ground: entrance on q (open towards t), exit j tiles on (open away), continue past it.
                // Neither end may sit on a machine's connection tile: it joins only through its open side.
                if !pf.ug_allowed || self.conn_net[q as usize] >= 0 {
                    continue;
                }
                let axis = if d == 1 || d == 3 { 0 } else { 1 };
                let mut chan_acc = self.chan_cost(pf, axis * n_i + q);
                let (qx, qy) = (self.dims.x(q as usize), self.dims.y(q as usize));
                for j in 1..=self.max_j {
                    if !self.dims.in_full(qx + pf.d_x[d as usize] * j, qy + pf.d_y[d as usize] * j) {
                        break;
                    }
                    let e = q + pf.d_idx[d as usize] * j;
                    chan_acc += self.chan_cost(pf, axis * n_i + e);
                    if !self.pipe_ok(pf, e, n.idx as i32) || self.conn_net[e as usize] >= 0 {
                        continue;
                    }
                    let cost = g_s
                        + self.pipe_cost(pf, q, false)
                        + self.pipe_cost(pf, e, false)
                        + (self.tun.ug_penalty + self.tun.ug_penalty_per_tile * f64::from(j)) as f32
                        + chan_acc;
                    self.relax(pf, Self::st(e, d + 1), s, cost, hw);
                }
            }
        }
        -1
    }

    fn relax(&self, pf: &mut PathFinder, ns: i32, from: i32, g: f32, hw: f32) {
        let nu = ns as usize;
        if pf.closed_gen[nu] == pf.search_gen || self.banned_stamp[nu] == self.banned_gen {
            return;
        }
        if pf.seen_gen[nu] == pf.search_gen && g >= pf.g_arr[nu] {
            return;
        }
        pf.seen_gen[nu] = pf.search_gen;
        pf.g_arr[nu] = g;
        pf.par[nu] = from;
        pf.heap.push(g + hw * pf.dist[(ns / 5) as usize] as f32, ns);
    }

    fn path_of(pf: &PathFinder, s: i32) -> Vec<i32> {
        let mut states = Vec::new();
        let mut c = s;
        while c != -1 {
            states.push(c);
            c = pf.par[c as usize];
        }
        states.reverse();
        states
    }

    /// Marks `t` as used by the path being checked; false if it already was.
    fn add_tile(&mut self, t: i32) -> bool {
        let tu = t as usize;
        if self.tile_stamp[tu] == self.tile_gen {
            return false;
        }
        self.tile_stamp[tu] = self.tile_gen;
        true
    }

    /// The first state of the path ending in `s` that lands on a tile the path already uses, or -1.
    fn crossing(&mut self, pf: &PathFinder, s: i32) -> i32 {
        let states = Self::path_of(pf, s);
        self.tile_gen += 1;
        for (i, &st) in states.iter().enumerate() {
            let t = st / 5;
            if i == 0 && self.own_stamp[t as usize] == self.stamp {
                continue; // starts on the tree (or is the first tile)
            }
            if i == 0 && self.entry_in[t as usize] >= 0 {
                self.add_tile(t); // a port entry outside the box: nothing goes there
                continue;
            }
            if st % 5 != 0 {
                let ent = (states[i - 1] / 5) + pf.d_idx[((st % 5) - 1) as usize];
                if !self.add_tile(ent) {
                    return st;
                }
            }
            if !self.add_tile(t) {
                return st;
            }
        }
        -1
    }

    /// A* with lazy repair: a path that crosses itself bans the state where it does and searches again.
    fn leg(&mut self, pf: &mut PathFinder, n: &FNet, sources: &[(i32, f32)], goal: &[i32]) -> i32 {
        self.banned_gen += 1;
        for _ in 0..=self.tun.repair_restarts {
            let s = self.search(pf, n, sources, goal);
            if s < 0 {
                return -1;
            }
            let bad = self.crossing(pf, s);
            if bad < 0 {
                return s;
            }
            self.banned_stamp[bad as usize] = self.banned_gen;
        }
        -2
    }

    /// Commits the path ending in `s` (checked by `leg` not to cross itself).
    fn commit(&mut self, pf: &mut PathFinder, n: &FNet, r: &mut PRouted, s: i32) {
        let states = Self::path_of(pf, s);
        let n_i = self.n as i32;
        for (i, &st) in states.iter().enumerate() {
            let t = st / 5;
            let k = st % 5;
            if i == 0 && (self.own_stamp[t as usize] == self.stamp || self.entry_in[t as usize] >= 0) {
                continue;
            }
            if k == 0 {
                self.add_pipe(pf, r, t, n.idx as i32);
            } else {
                let d = k - 1;
                let ent = (states[i - 1] / 5) + pf.d_idx[d as usize];
                self.add_ptg(pf, r, ent, (d + 2) % 4, n.idx as i32);
                self.add_ptg(pf, r, t, d, n.idx as i32);
                let axis = if d == 1 || d == 3 { 0 } else { 1 };
                let mut x = ent;
                loop {
                    let ci = axis * n_i + x;
                    self.occ_p_chan[ci as usize] += 1;
                    r.chans.push(ci);
                    if x == t {
                        break;
                    }
                    x += pf.d_idx[d as usize];
                }
            }
        }
    }

    fn fail(&mut self, pf: &mut PathFinder, n: &FNet, r: &PRouted, why: &str) -> bool {
        pf.log.push(format!("fluid {}: {why}", n.fluid));
        self.rip_up_routed(pf, r);
        self.routed[n.idx] = None;
        false
    }

    /// Routes one fluid net as a tree; on failure rips it up and returns false.
    pub(super) fn route(&mut self, pf: &mut PathFinder, i: usize) -> bool {
        let n = self.nets[i].clone();
        self.stamp += 1;
        let mut r = PRouted::new();
        let mut left: Vec<Vec<i32>> = n.groups.clone();
        let flat = |groups: &[Vec<i32>]| -> Vec<i32> { groups.iter().flatten().copied().collect() };
        // First leg: from the port (any candidate tile) or from the first terminal. A net with no machines (a pipe
        // between blocks of a factory) runs from its port straight to its output.
        if !n.port_tiles.is_empty() {
            if left.is_empty() && n.out_tiles.is_empty() {
                return self.fail(pf, &n, &r, "no consumers");
            }
            // An edge port tile is entered from just outside the box, heading in, so it can hold a pipe or a
            // pipe-to-ground open to the outside: that joins nothing beside it, so ports of other fluids may touch.
            let mut entries: Vec<i32> = Vec::new();
            let mut starts: Vec<(i32, f32)> = Vec::new();
            for (k, &t) in n.port_tiles.iter().enumerate() {
                let o = n.port_out[k];
                let (ox, oy) = if (0..4).contains(&o) { (pf.d_x[o as usize], pf.d_y[o as usize]) } else { (0, 0) };
                let x = self.dims.x(t as usize) + ox;
                let y = self.dims.y(t as usize) + oy;
                if o >= 0 && self.dims.in_full(x, y) && !self.dims.in_box(x, y) {
                    let q = self.dims.idx(x, y) as i32;
                    self.entry_in[q as usize] = (o + 2) % 4;
                    entries.push(q);
                    starts.push((q, 0f32));
                } else if self.pipe_ok(pf, t, n.idx as i32) {
                    starts.push((t, self.pipe_cost(pf, t, true)));
                }
            }
            let goal: Vec<i32> = if left.is_empty() { n.out_tiles.iter().copied().filter(|&t| self.pipe_ok(pf, t, n.idx as i32)).collect() } else { flat(&left) };
            let s = self.leg(pf, &n, &starts, &goal);
            if s >= 0 {
                self.commit(pf, &n, &mut r, s);
                let mut c = s;
                while pf.par[c as usize] != -1 {
                    c = pf.par[c as usize];
                }
                let t0 = c / 5;
                r.source_tile = if self.entry_in[t0 as usize] >= 0 { t0 + self.d_idx[self.entry_in[t0 as usize] as usize] } else { t0 };
            }
            for e in entries {
                self.entry_in[e as usize] = -1;
            }
            if s == -1 {
                return self.fail(pf, &n, &r, "no path from the port");
            }
            if s < 0 {
                return self.fail(pf, &n, &r, "every path from the port crosses itself");
            }
        } else {
            if left.len() + usize::from(!n.out_tiles.is_empty()) < 2 {
                return self.fail(pf, &n, &r, "needs two connections");
            }
            let first = left.remove(0);
            if !self.pipe_ok(pf, first[0], n.idx as i32) {
                return self.fail(pf, &n, &r, "first connection blocked");
            }
            self.add_pipe(pf, &mut r, first[0], n.idx as i32);
        }
        loop {
            let stamp = self.stamp;
            left.retain(|g| !g.iter().any(|&t| self.own_stamp[t as usize] == stamp));
            if left.is_empty() {
                break;
            }
            let tree: Vec<(i32, f32)> = r.pipes.iter().map(|&t| (t, 0f32)).collect();
            let s = self.leg(pf, &n, &tree, &flat(&left));
            if s == -1 {
                return self.fail(pf, &n, &r, &format!("no path to {} more connections", left.len()));
            }
            if s < 0 {
                return self.fail(pf, &n, &r, &format!("every path to {} more connections crosses itself", left.len()));
            }
            self.commit(pf, &n, &mut r, s);
        }
        r.out_tile = n.out_tiles.iter().copied().find(|&t| self.own_stamp[t as usize] == self.stamp && r.pipes.contains(&t)).unwrap_or(-1);
        if !n.out_tiles.is_empty() && r.out_tile < 0 {
            let tree: Vec<(i32, f32)> = r.pipes.iter().map(|&t| (t, 0f32)).collect();
            let goal: Vec<i32> = n.out_tiles.iter().copied().filter(|&t| self.pipe_ok(pf, t, n.idx as i32) || self.own_stamp[t as usize] == self.stamp).collect();
            let s = self.leg(pf, &n, &tree, &goal);
            if s < 0 {
                return self.fail(pf, &n, &r, "no path to the output port");
            }
            self.commit(pf, &n, &mut r, s);
            r.out_tile = s / 5;
        }
        r.ok = true;
        self.routed[i] = Some(r);
        true
    }

    // ---- negotiation bookkeeping ----
    /// Joins with other fluids at tile `t` (a pipe end of one net next to a joinable end of another).
    pub(super) fn adjacency_overuse(&self, t: usize) -> i32 {
        if self.pipe_count[t] == 0 {
            return 0;
        }
        let mut o = 0;
        for d in 0..4 {
            let u = t as i32 + self.d_idx[d];
            if self.in_grid(u)
                && self.pipe_count[u as usize] > 0
                && self.pipe_owner[u as usize] != self.pipe_owner[t]
                && self.joins(t, d as i32)
                && self.joins(u as usize, (d as i32 + 2) % 4)
            {
                o += 1;
            }
        }
        o
    }

    /// Does the pipe end at `t` connect towards direction `d`? (normal pipes: always; pipe-to-ground: open side only)
    fn joins(&self, t: usize, d: i32) -> bool {
        self.ptg_open[t] < 0 || self.ptg_open[t] == d
    }

    pub(super) fn channel_overuse(&self) -> i32 {
        self.occ_p_chan.iter().map(|&c| (c - 1).max(0)).sum()
    }

    pub(super) fn update_history(&mut self) {
        for c in 0..2 * self.n {
            let o = self.occ_p_chan[c] - 1;
            if o > 0 {
                self.hist_p_chan[c] += (self.tun.h_fac * f64::from(o)) as f32;
            }
        }
    }

    pub(super) fn touches_overuse(&self, i: usize, overuse_at: &dyn Fn(usize) -> i32) -> bool {
        let Some(r) = &self.routed[i] else { return true };
        if !r.ok {
            return true;
        }
        r.pipes.iter().any(|&t| overuse_at(t as usize) > 0) || r.ptgs.iter().any(|&e| overuse_at((e >> 2) as usize) > 0) || r.chans.iter().any(|&c| self.occ_p_chan[c as usize] > 1)
    }

    pub(super) fn ok(&self, i: usize) -> bool {
        self.routed[i].as_ref().is_some_and(|r| r.ok)
    }

    /// Does net `i` lie in `region` (or is it unrouted)?
    pub(super) fn touches_region(&self, i: usize, region: &[bool]) -> bool {
        let Some(r) = &self.routed[i] else { return true };
        if !r.ok {
            return true;
        }
        r.pipes.iter().any(|&t| region[t as usize]) || r.ptgs.iter().any(|&e| region[(e >> 2) as usize])
    }

    /// Plan nets of the pipes on `t` and beside it, for naming the fluids in a conflict there.
    pub(super) fn plan_nets_near(&self, t: usize) -> Vec<i32> {
        let mut out: Vec<i32> = Vec::new();
        let around = std::iter::once(t as i32).chain((0..4).map(|d| t as i32 + self.d_idx[d]));
        for u in around {
            if self.in_grid(u) && self.pipe_count[u as usize] > 0 && self.pipe_owner[u as usize] >= 0 {
                let id = self.nets[self.pipe_owner[u as usize] as usize].plan_id;
                if !out.contains(&id) {
                    out.push(id);
                }
            }
        }
        out
    }

    /// Where net `i`'s trouble is: its connection tiles and ports.
    pub(super) fn seed_tiles(&self, i: usize) -> Vec<i32> {
        let n = &self.nets[i];
        n.groups.iter().flatten().copied().chain(n.port_tiles.iter().take(1).copied()).chain(n.out_tiles.iter().take(1).copied()).collect()
    }

    /// Tiles of overused pipe-to-ground channels.
    pub(super) fn channel_conflicts(&self) -> Vec<i32> {
        (0..2 * self.n).filter(|&c| self.occ_p_chan[c] > 1).map(|c| (c % self.n) as i32).collect()
    }

    pub(super) fn hpwl(&self, i: usize) -> i32 {
        self.nets[i].hpwl
    }

    pub(super) fn failed(&self) -> Vec<i32> {
        self.nets.iter().filter(|n| !self.ok(n.idx)).map(|n| n.plan_id).collect()
    }

    pub(super) fn sources(&self) -> Vec<SourceSpec> {
        self.nets
            .iter()
            .filter_map(|n| {
                let r = self.routed[n.idx].as_ref().filter(|r| r.ok && r.source_tile >= 0)?;
                Some(SourceSpec { tile: self.dims.tile(r.source_tile as usize), item: n.fluid.clone(), rate: n.source_rate, fluid: true, lane: -1, port: n.src_port })
            })
            .collect()
    }

    pub(super) fn outputs(&self) -> Vec<OutputSpec> {
        self.nets
            .iter()
            .filter_map(|n| {
                let r = self.routed[n.idx].as_ref().filter(|r| r.ok && r.out_tile >= 0)?;
                Some(OutputSpec { tile: self.dims.tile(r.out_tile as usize), item: n.fluid.clone(), rate: n.out_rate, fluid: true, port: n.out_port })
            })
            .collect()
    }

    pub(super) fn entities(&self) -> Vec<PlacedEntity> {
        let mut out = Vec::new();
        for n in &self.nets {
            let Some(r) = &self.routed[n.idx] else { continue };
            for &t in &r.pipes {
                let tile = self.dims.tile(t as usize);
                let mut e = PlacedEntity::new(gc::PIPE, Kind::Pipe, tile.x, tile.y, Dir::N);
                e.fluid = Some(n.fluid.clone());
                e.net = Some(n.plan_id);
                out.push(e);
            }
            for &e in &r.ptgs {
                let tile = self.dims.tile((e >> 2) as usize);
                let mut ent = PlacedEntity::new(gc::PIPE_TO_GROUND, Kind::PipeUg, tile.x, tile.y, Dir::ALL[(e & 3) as usize]);
                ent.fluid = Some(n.fluid.clone());
                ent.net = Some(n.plan_id);
                out.push(ent);
            }
        }
        out
    }
}
