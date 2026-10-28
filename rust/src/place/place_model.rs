use super::placement::{PlacedMachine, Placement, connections};
use crate::config::run_configuration::RunConfiguration;
use crate::config::tuning::PlaceTuning;
use crate::geom::{Dir, Tile};
use crate::plan::{ProductionPlan, Role, best_inserter_rate};
use anyhow::{Result, ensure};
use std::collections::{HashMap, HashSet};

/// Items in insertion order, each at most once, cleared in O(1). The Kotlin built a LinkedHashSet or HashSet
/// for every move.
#[derive(Default)]
struct OrderedSet {
    stamp: Vec<u32>,
    gen_: u32,
    items: Vec<usize>,
}

impl OrderedSet {
    fn new(size: usize) -> Self {
        Self { stamp: vec![0; size], gen_: 1, items: Vec::new() }
    }

    fn clear(&mut self) {
        self.gen_ = self.gen_.wrapping_add(1);
        if self.gen_ == 0 {
            self.stamp.fill(0);
            self.gen_ = 1;
        }
        self.items.clear();
    }

    fn insert(&mut self, i: usize) {
        if self.stamp[i] != self.gen_ {
            self.stamp[i] = self.gen_;
            self.items.push(i);
        }
    }
}

/// The order a `java.util.HashSet<Integer>` filled with `items` (distinct, in insertion order) iterates in: by
/// bucket (the value masked by the table size, which starts at 16 and doubles past three quarters full), and by
/// insertion within a bucket. The direct-insertion credit is a float sum over such a set, and a cost that differs
/// in the last bit would change whether a move counts as a zero-cost accept, which draws no random number.
fn java_hash_order(items: &mut [usize]) {
    if items.len() < 2 {
        return;
    }
    let mut capacity = 16;
    while items.len() > capacity * 3 / 4 {
        capacity *= 2;
    }
    items.sort_by_key(|&i| (i ^ (i >> 16)) & (capacity - 1));
}

fn mark(dirty: &mut [u32], list: &mut Vec<usize>, gen_: u32, b: usize) {
    if dirty[b] != gen_ {
        dirty[b] = gen_;
        list.push(b);
    }
}

/// Grid index of the stub of connection `i` (relative triples `a`) of a machine `mh` tall at (x, y), or None
/// outside the box.
fn stub(mh: i32, w: i32, h: i32, a: &[i32], i: usize, x: i32, y: i32) -> Option<usize> {
    let (dx, dy) = (a[3 * i], a[3 * i + 1]);
    let (sx, sy) = if dy < 0 {
        (dx, dy - 1)
    } else if dy >= mh {
        (dx, dy + 1)
    } else if dx < 0 {
        (dx - 1, dy)
    } else {
        (dx + 1, dy)
    };
    let (tx, ty) = (x + sx, y + sy);
    (tx >= 0 && tx < w && ty >= 0 && ty < h).then(|| (ty * w + tx) as usize)
}

struct PortGeom {
    fixed: Vec<Tile>,
    /// x0, y0, x1, y1
    segments: Vec<[i32; 4]>,
}

/// Placement state and the annealing cost, updated incrementally per move:
///   C = alpha * sum_n w_n q(k_n) HPWL_n + beta * sum_bins max(0, D - S)^2 + gamma * sum_m A_m
///       - delta * DI + beta2 * sum_bins h * D
/// Every move costs O(touched nets + touched bins), never a routing call.
pub struct PlaceModel<'a> {
    pub plan: &'a ProductionPlan,
    pub config: &'a RunConfiguration,
    pub tun: &'a PlaceTuning,
    pub w: i32,
    pub h: i32,
    pub n_m: usize,
    pub mw: Vec<i32>,
    pub mh: Vec<i32>,
    /// Machines whose orientation matters (they have fluid connections).
    pub fluid: Vec<bool>,
    pub px: Vec<i32>,
    pub py: Vec<i32>,
    /// Orientation code: rotation (quarter turns) + 4 if mirrored.
    pub rot: Vec<i32>,
    placed: bool,
    /// Machines stamped into the grid (all of them once placed).
    pub is_placed: Vec<bool>,

    /// 0 free, -1 obstacle, id + 1 for a machine's footprint.
    pub occ: Vec<i32>,
    /// Used fluid connection tiles: how many machines put a pipe end here, and which fluid (id >= 1).
    conn_cnt: Vec<i32>,
    conn_fl: Vec<i32>,
    /// Connection tiles a machine exposes but its recipe does not use: no pipe may sit there.
    unused_cnt: Vec<i32>,
    /// The tile straight out from each used connection, kept free of machines so a pipe can always reach it.
    stub_cnt: Vec<i32>,
    /// Per machine and orientation code: connection tiles relative to the top-left, as (dx, dy, fluid id or 0).
    conn_rel: Vec<[Vec<i32>; 8]>,

    n_nets: usize,
    net_machines: Vec<Vec<usize>>,
    net_ports: Vec<Vec<PortGeom>>,
    net_weight: Vec<f64>,
    machine_nets: Vec<Vec<usize>>,
    /// x0, y0, x1, y1 per net
    nb: Vec<i32>,
    net_hpwl: Vec<f64>,
    net_density: Vec<f64>,

    bin: i32,
    bw: i32,
    bin_d: Vec<f64>,
    bin_s: Vec<i32>,
    bin_hist: Vec<f64>,
    bin_cong: Vec<f64>,
    bin_fb: Vec<f64>,
    dirty: Vec<u32>,
    dirty_list: Vec<usize>,
    dirty_gen: u32,

    access_need: Vec<f64>,

    pin_need: Vec<i32>,
    pin_pen: Vec<f64>,
    machine_terms: Vec<Vec<usize>>,

    /// Direct insertion pairs (producer, consumer) and their terminals.
    di_a: Vec<usize>,
    di_b: Vec<usize>,
    di_pt: Vec<usize>,
    di_ct: Vec<usize>,
    di_fit: Vec<bool>,
    di_gain: Vec<f64>,
    /// Current (possibly partial) credit of each pair: 1 when it fits, less when a tile or two off.
    di_score: Vec<f64>,
    machine_pairs: Vec<Vec<usize>>,
    ok5: bool,
    ok6: bool,

    pub wl: f64,
    pub cong: f64,
    pub pin: f64,
    /// Belt inserters the fitting direct-insertion pairs save.
    pub di: f64,
    pub fb: f64,
    pub beta2: f64,

    path_cost: bool,
    path_done: Vec<bool>,
    best_port_x: i32,
    best_port_y: i32,
    has_port: bool,

    touched_nets: OrderedSet,
    pin_machines: OrderedSet,
    pair_set: OrderedSet,
}

impl<'a> PlaceModel<'a> {
    pub fn new(
        plan: &'a ProductionPlan,
        config: &'a RunConfiguration,
        tun: &'a PlaceTuning,
        w: i32,
        h: i32,
        obstacles: &HashSet<Tile>,
    ) -> Result<Self> {
        ensure!(tun.belt_cost == "hpwl" || tun.belt_cost == "path", "place.belt_cost must be \"hpwl\" or \"path\"");
        let n_m = plan.machines.len();
        let size = (w * h) as usize;
        let mut occ = vec![0; size];
        for t in obstacles {
            if t.x >= 0 && t.x < w && t.y >= 0 && t.y < h {
                occ[(t.y * w + t.x) as usize] = -1;
            }
        }
        let mut fluid_ids: HashMap<String, i32> = HashMap::new();
        let conn_rel: Vec<[Vec<i32>; 8]> = (0..n_m)
            .map(|m| {
                std::array::from_fn(|code| {
                    let cs = connections(&plan.machines[m], 0, 0, (code % 4) as i32, code >= 4);
                    let mut a = vec![0; cs.len() * 3];
                    for (i, c) in cs.iter().enumerate() {
                        a[3 * i] = c.tile.x;
                        a[3 * i + 1] = c.tile.y;
                        a[3 * i + 2] = match &c.fluid {
                            Some(f) => {
                                let next = fluid_ids.len() as i32 + 1;
                                *fluid_ids.entry(f.clone()).or_insert(next)
                            }
                            None => 0,
                        };
                    }
                    a
                })
            })
            .collect();

        let nets = &plan.nets;
        let n_nets = nets.len();
        let net_terminals: Vec<_> = nets.iter().map(|n| plan.net_terminals(n)).collect();
        let net_machines: Vec<Vec<usize>> = net_terminals
            .iter()
            .map(|ts| {
                let mut ms: Vec<usize> = Vec::new();
                for m in ts.iter().filter_map(|t| t.machine) {
                    if !ms.contains(&m) {
                        ms.push(m);
                    }
                }
                ms
            })
            .collect();
        let net_ports: Vec<Vec<PortGeom>> =
            net_terminals.iter().map(|ts| ts.iter().filter_map(|t| t.port).map(|pid| port_geom(plan, pid, w, h)).collect()).collect();
        // A path estimate already grows with the terminal count; q(k) corrects only the bounding box.
        let net_weight: Vec<f64> = (0..n_nets)
            .map(|n| {
                let k = net_terminals[n].len() as i32;
                f64::from(nets[n].belt_count) * if tun.belt_cost == "path" { 1.0 } else { tun.q(k) }
            })
            .collect();
        let machine_nets: Vec<Vec<usize>> = (0..n_m).map(|m| (0..n_nets).filter(|&n| net_machines[n].contains(&m)).collect()).collect();

        let mut pin_need = vec![0; n_m];
        let mut access_need = vec![0.0; n_m];
        for t in &plan.terminals {
            if let Some(m) = t.machine.filter(|_| t.role != Role::Source && !t.fluid) {
                pin_need[m] += t.k;
                access_need[m] += tun.access_demand * (f64::from(t.k) + tun.access_belt_tiles);
            }
        }

        let (ok5, ok6) = (config.trick("I5"), config.trick("I6"));
        let mut pairs: Vec<(usize, usize)> = Vec::new();
        if ok5 || ok6 {
            let producers: Vec<_> = plan.terminals.iter().filter(|t| t.role == Role::Producer && !t.fluid).collect();
            let consumers: Vec<_> = plan.terminals.iter().filter(|t| t.role == Role::Consumer && t.machine.is_some() && !t.fluid).collect();
            for p in &producers {
                for c in &consumers {
                    if p.item == c.item && p.machine != c.machine {
                        pairs.push((p.id, c.id));
                    }
                }
            }
        }
        let di_pt: Vec<usize> = pairs.iter().map(|p| p.0).collect();
        let di_ct: Vec<usize> = pairs.iter().map(|p| p.1).collect();
        let di_a: Vec<usize> = di_pt.iter().map(|&t| plan.terminals[t].machine.expect("a producer terminal has a machine")).collect();
        let di_b: Vec<usize> = di_ct.iter().map(|&t| plan.terminals[t].machine.expect("a consumer terminal has a machine")).collect();
        let pick = best_inserter_rate(config, true);
        let di_gain: Vec<f64> = (0..pairs.len()).map(|i| plan.terminals[di_pt[i]].rate.min(plan.terminals[di_ct[i]].rate) / pick).collect();
        let machine_pairs: Vec<Vec<usize>> = (0..n_m).map(|m| (0..pairs.len()).filter(|&i| di_a[i] == m || di_b[i] == m).collect()).collect();
        let machine_terms: Vec<Vec<usize>> = (0..n_m)
            .map(|m| plan.terminals.iter().filter(|t| t.machine == Some(m) && t.role != Role::Source && !t.fluid).map(|t| t.id).collect())
            .collect();

        let bin = tun.rudy_bin;
        let (bw, bh) = ((w + bin - 1) / bin, (h + bin - 1) / bin);
        let n_bins = (bw * bh) as usize;
        let mut bin_s = vec![0; n_bins];
        for by in 0..bh {
            for bx in 0..bw {
                let mut s = 0;
                for y in by * bin..h.min((by + 1) * bin) {
                    for x in bx * bin..w.min((bx + 1) * bin) {
                        if occ[(y * w + x) as usize] == 0 {
                            s += 1;
                        }
                    }
                }
                bin_s[(by * bw + bx) as usize] = s;
            }
        }

        Ok(Self {
            plan,
            config,
            tun,
            w,
            h,
            n_m,
            mw: plan.machines.iter().map(|m| m.w).collect(),
            mh: plan.machines.iter().map(|m| m.h).collect(),
            fluid: plan.machines.iter().map(super::placement::orientable).collect(),
            px: vec![0; n_m],
            py: vec![0; n_m],
            rot: vec![0; n_m],
            placed: false,
            is_placed: vec![false; n_m],
            occ,
            conn_cnt: vec![0; size],
            conn_fl: vec![0; size],
            unused_cnt: vec![0; size],
            stub_cnt: vec![0; size],
            conn_rel,
            n_nets,
            net_machines,
            net_ports,
            net_weight,
            machine_nets,
            nb: vec![0; 4 * n_nets],
            net_hpwl: vec![0.0; n_nets],
            net_density: vec![0.0; n_nets],
            bin,
            bw,
            bin_d: vec![0.0; n_bins],
            bin_s,
            bin_hist: vec![0.0; n_bins],
            bin_cong: vec![0.0; n_bins],
            bin_fb: vec![0.0; n_bins],
            dirty: vec![0; n_bins],
            dirty_list: Vec::with_capacity(n_bins),
            dirty_gen: 0,
            access_need,
            pin_need,
            pin_pen: vec![0.0; n_m],
            machine_terms,
            di_fit: vec![false; pairs.len()],
            di_score: vec![0.0; pairs.len()],
            di_a,
            di_b,
            di_pt,
            di_ct,
            di_gain,
            machine_pairs,
            ok5,
            ok6,
            wl: 0.0,
            cong: 0.0,
            pin: 0.0,
            di: 0.0,
            fb: 0.0,
            beta2: 0.0,
            path_cost: tun.belt_cost == "path",
            path_done: vec![false; n_m],
            best_port_x: 0,
            best_port_y: 0,
            has_port: false,
            touched_nets: OrderedSet::new(n_nets),
            pin_machines: OrderedSet::new(n_m),
            pair_set: OrderedSet::new(pairs.len()),
        })
    }

    pub fn cost(&self) -> f64 {
        self.tun.alpha * self.wl + self.tun.beta * self.cong + self.tun.gamma * self.pin - self.tun.delta * self.di + self.beta2 * self.fb
    }

    pub fn placed(&self) -> bool {
        self.placed
    }

    pub fn net_count(&self) -> usize {
        self.n_nets
    }

    pub fn q(&self, k: i32) -> f64 {
        self.tun.q(k)
    }

    // ---------------- geometry helpers ----------------
    fn in_box(&self, x: i32, y: i32) -> bool {
        x >= 0 && x < self.w && y >= 0 && y < self.h
    }

    fn idx(&self, x: i32, y: i32) -> usize {
        (y * self.w + x) as usize
    }

    /// Used fluid connection tiles of machine `m` in orientation `code` at (x, y), with their fluid ids.
    pub fn fluid_tiles_of(&self, m: usize, x: i32, y: i32, code: usize) -> Vec<(Tile, i32)> {
        let a = &self.conn_rel[m][code];
        (0..a.len() / 3).filter(|&i| a[3 * i + 2] != 0).map(|i| (Tile::new(x + a[3 * i], y + a[3 * i + 1]), a[3 * i + 2])).collect()
    }

    /// Writes or erases a machine in the grids only (no cost bookkeeping).
    fn raw_stamp(&mut self, m: usize, x: i32, y: i32, code: usize, add: bool) {
        let (w, h) = (self.w, self.h);
        let v = if add { m as i32 + 1 } else { 0 };
        for yy in y..y + self.mh[m] {
            for xx in x..x + self.mw[m] {
                self.occ[(yy * w + xx) as usize] = v;
            }
        }
        let delta = if add { 1 } else { -1 };
        let a = &self.conn_rel[m][code];
        for i in 0..a.len() / 3 {
            let (cx, cy) = (x + a[3 * i], y + a[3 * i + 1]);
            if cx < 0 || cx >= w || cy < 0 || cy >= h {
                continue;
            }
            let t = (cy * w + cx) as usize;
            let f = a[3 * i + 2];
            if f == 0 {
                self.unused_cnt[t] += delta;
            } else {
                self.conn_cnt[t] += delta;
                if add {
                    self.conn_fl[t] = f;
                } else if self.conn_cnt[t] == 0 {
                    self.conn_fl[t] = 0;
                }
                if let Some(st) = stub(self.mh[m], w, h, a, i, x, y) {
                    self.stub_cnt[st] += delta;
                }
            }
        }
    }

    /// Can machine `m` stand at (x, y) in orientation `code` against what is in the grids now?
    fn fits_at(&self, m: usize, x: i32, y: i32, code: usize) -> bool {
        for yy in y..y + self.mh[m] {
            for xx in x..x + self.mw[m] {
                if !self.in_box(xx, yy) {
                    return false;
                }
                let t = self.idx(xx, yy);
                if self.occ[t] != 0 || self.conn_cnt[t] != 0 || self.stub_cnt[t] != 0 {
                    return false;
                }
            }
        }
        let a = &self.conn_rel[m][code];
        for i in 0..a.len() / 3 {
            let (cx, cy) = (x + a[3 * i], y + a[3 * i + 1]);
            let f = a[3 * i + 2];
            if f == 0 {
                // An unused connection may not sit on another machine's pipe end.
                if self.in_box(cx, cy) {
                    let t = self.idx(cx, cy);
                    if self.conn_cnt[t] != 0 || self.stub_cnt[t] != 0 {
                        return false;
                    }
                }
                continue;
            }
            if !self.in_box(cx, cy) {
                return false;
            }
            let t = self.idx(cx, cy);
            if self.occ[t] != 0 || self.unused_cnt[t] != 0 {
                return false;
            }
            let Some(st) = stub(self.mh[m], self.w, self.h, a, i, x, y) else { return false };
            if self.occ[st] != 0 || self.unused_cnt[st] != 0 || self.stub_cnt[st] != 0 {
                return false;
            }
            if self.conn_cnt[st] != 0 && self.conn_fl[st] != f {
                return false;
            }
            if self.conn_cnt[t] != 0 && self.conn_fl[t] != f {
                return false;
            }
            // Pipes of different fluids may not touch.
            for d in Dir::ALL {
                let (nx, ny) = (cx + d.dx(), cy + d.dy());
                if self.in_box(nx, ny) {
                    let n = self.idx(nx, ny);
                    if self.conn_cnt[n] != 0 && self.conn_fl[n] != f {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Can the machines in `ms` stand at the given positions and orientations (others staying put)?
    pub fn legal(&mut self, ms: &[usize], nx: &[i32], ny: &[i32], nr: &[i32]) -> bool {
        for &m in ms {
            if self.is_placed[m] {
                self.raw_stamp(m, self.px[m], self.py[m], self.rot[m] as usize, false);
            }
        }
        let mut ok = true;
        let mut added = 0;
        for i in 0..ms.len() {
            if !self.fits_at(ms[i], nx[i], ny[i], nr[i] as usize) {
                ok = false;
                break;
            }
            self.raw_stamp(ms[i], nx[i], ny[i], nr[i] as usize, true);
            added += 1;
        }
        for i in 0..added {
            self.raw_stamp(ms[i], nx[i], ny[i], nr[i] as usize, false);
        }
        for &m in ms {
            if self.is_placed[m] {
                self.raw_stamp(m, self.px[m], self.py[m], self.rot[m] as usize, true);
            }
        }
        ok
    }

    // ---------------- occupancy ----------------
    fn is_free(&self, t: usize) -> bool {
        self.occ[t] == 0 && self.conn_cnt[t] == 0
    }

    fn stamp(&mut self, m: usize, add: bool) {
        let (w, h, bin, bw) = (self.w, self.h, self.bin, self.bw);
        let v = if add { m as i32 + 1 } else { 0 };
        let (x0, y0) = (self.px[m], self.py[m]);
        for yy in y0..y0 + self.mh[m] {
            for xx in x0..x0 + self.mw[m] {
                self.occ[(yy * w + xx) as usize] = v;
                let b = ((yy / bin) * bw + xx / bin) as usize;
                self.bin_s[b] += if add { -1 } else { 1 };
                mark(&mut self.dirty, &mut self.dirty_list, self.dirty_gen, b);
            }
        }
        let delta = if add { 1 } else { -1 };
        let a = &self.conn_rel[m][self.rot[m] as usize];
        for i in 0..a.len() / 3 {
            let f = a[3 * i + 2];
            let (cx, cy) = (x0 + a[3 * i], y0 + a[3 * i + 1]);
            if cx < 0 || cx >= w || cy < 0 || cy >= h {
                continue;
            }
            let t = (cy * w + cx) as usize;
            if f == 0 {
                self.unused_cnt[t] += delta;
                continue;
            }
            if let Some(st) = stub(self.mh[m], w, h, a, i, x0, y0) {
                self.stub_cnt[st] += delta;
            }
            let was_free = self.occ[t] == 0 && self.conn_cnt[t] == 0;
            self.conn_cnt[t] += delta;
            if add {
                self.conn_fl[t] = f;
            } else if self.conn_cnt[t] == 0 {
                self.conn_fl[t] = 0;
            }
            let now_free = self.occ[t] == 0 && self.conn_cnt[t] == 0;
            if was_free != now_free {
                let b = ((cy / bin) * bw + cx / bin) as usize;
                self.bin_s[b] += if was_free { -1 } else { 1 };
                mark(&mut self.dirty, &mut self.dirty_list, self.dirty_gen, b);
            }
        }
    }

    // ---------------- nets ----------------
    fn cx(&self, m: usize) -> i32 {
        self.px[m] + self.mw[m] / 2
    }

    fn cy(&self, m: usize) -> i32 {
        self.py[m] + self.mh[m] / 2
    }

    /// Gaps a greedy path leaves between the inserter rings (footprint plus one tile) of net `n`'s machines,
    /// starting at its port point when it has one, else at its first machine: 0 when every next machine shares
    /// a corridor with the last, more for every detour a belt would have to make.
    fn ring_gaps(&mut self, n: usize, port_x: i32, port_y: i32, has_port: bool) -> f64 {
        let ms = &self.net_machines[n];
        if ms.is_empty() {
            return 0.0;
        }
        let (mut ax0, mut ay0, mut ax1, mut ay1);
        let mut left = ms.len();
        if has_port {
            (ax0, ax1, ay0, ay1) = (port_x, port_x, port_y, port_y);
        } else {
            let m = ms[0];
            self.path_done[m] = true;
            left -= 1;
            (ax0, ay0, ax1, ay1) = (self.px[m] - 1, self.py[m] - 1, self.px[m] + self.mw[m], self.py[m] + self.mh[m]);
        }
        let mut total = 0;
        while left > 0 {
            let (mut best, mut best_d) = (usize::MAX, i32::MAX);
            for &m in ms {
                if self.path_done[m] {
                    continue;
                }
                let (bx0, by0, bx1, by1) = (self.px[m] - 1, self.py[m] - 1, self.px[m] + self.mw[m], self.py[m] + self.mh[m]);
                let d = 0.max((bx0 - ax1).max(ax0 - bx1)) + 0.max((by0 - ay1).max(ay0 - by1));
                if d < best_d {
                    best_d = d;
                    best = m;
                }
            }
            self.path_done[best] = true;
            left -= 1;
            total += best_d;
            (ax0, ay0, ax1, ay1) = (self.px[best] - 1, self.py[best] - 1, self.px[best] + self.mw[best], self.py[best] + self.mh[best]);
        }
        for &m in ms {
            self.path_done[m] = false;
        }
        f64::from(total)
    }

    fn compute_net(&mut self, n: usize) {
        let (w, h) = (self.w, self.h);
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for &m in &self.net_machines[n] {
            let (x, y) = (self.cx(m), self.cy(m));
            x0 = x0.min(x);
            x1 = x1.max(x);
            y0 = y0.min(y);
            y1 = y1.max(y);
        }
        if x0 == i32::MAX {
            (x0, x1, y0, y1) = (w / 2, w / 2, h / 2, h / 2);
        }
        // Each port adds its point closest to the machines' box (the first one also starts the path estimate).
        self.has_port = false;
        for pg in &self.net_ports[n] {
            let (mut best_x, mut best_y, mut best_d) = (0, 0, i32::MAX);
            let mut consider = |x: i32, y: i32| {
                let d = 0.max((x0 - x).max(x - x1)) + 0.max((y0 - y).max(y - y1));
                if d < best_d {
                    best_d = d;
                    best_x = x;
                    best_y = y;
                }
            };
            for t in &pg.fixed {
                consider(t.x, t.y);
            }
            for s in &pg.segments {
                consider(x0.clamp(s[0], s[2]), y0.clamp(s[1], s[3]));
                consider(x1.clamp(s[0], s[2]), y1.clamp(s[1], s[3]));
            }
            if best_d != i32::MAX {
                if !self.has_port {
                    self.has_port = true;
                    self.best_port_x = best_x;
                    self.best_port_y = best_y;
                }
                x0 = x0.min(best_x);
                x1 = x1.max(best_x);
                y0 = y0.min(best_y);
                y1 = y1.max(best_y);
            }
        }
        self.nb[4 * n] = x0.clamp(0, w - 1);
        self.nb[4 * n + 1] = y0.clamp(0, h - 1);
        self.nb[4 * n + 2] = x1.clamp(0, w - 1);
        self.nb[4 * n + 3] = y1.clamp(0, h - 1);
        let mut hpwl = f64::from((x1 - x0) + (y1 - y0));
        if self.path_cost {
            let gaps = self.ring_gaps(n, self.best_port_x, self.best_port_y, self.has_port);
            hpwl += self.tun.path_weight * gaps;
        }
        self.net_hpwl[n] = hpwl;
        let area = (self.nb[4 * n + 2] - self.nb[4 * n] + 1) * (self.nb[4 * n + 3] - self.nb[4 * n + 1] + 1);
        self.net_density[n] = self.net_weight[n] * self.net_hpwl[n] / f64::from(area);
    }

    /// Adds (sign 1) or removes (sign -1) a net's RUDY demand.
    fn spread(&mut self, n: usize, sign: f64) {
        let (x0, y0, x1, y1) = (self.nb[4 * n], self.nb[4 * n + 1], self.nb[4 * n + 2], self.nb[4 * n + 3]);
        let d = self.net_density[n] * sign;
        if d == 0.0 {
            return;
        }
        let (bin, bw) = (self.bin, self.bw);
        let (bx_first, bx_last) = ((x0 / bin) as usize, (x1 / bin) as usize);
        let width_in = |bx: usize| {
            let bx = bx as i32;
            x1.min((bx + 1) * bin - 1) - x0.max(bx * bin) + 1
        };
        let (first_ox, last_ox) = (f64::from(width_in(bx_first)), f64::from(width_in(bx_last)));
        let bin_f = f64::from(bin);
        for by in y0 / bin..=y1 / bin {
            let oy = f64::from(y1.min((by + 1) * bin - 1) - y0.max(by * bin) + 1);
            let row = (by * bw) as usize;
            let (first, last) = (row + bx_first, row + bx_last);
            // Only the first and last bin of the row are partly covered; every bin between gets the same amount
            // (the same operations in the same order as for any other bin, so the sums are unchanged).
            let bins = &mut self.bin_d[first..=last];
            bins[0] += d * first_ox * oy;
            if last > first {
                let whole = d * bin_f * oy;
                let n = last - first;
                for v in &mut bins[1..n] {
                    *v += whole;
                }
                bins[n] += d * last_ox * oy;
            }
            for b in first..=last {
                mark(&mut self.dirty, &mut self.dirty_list, self.dirty_gen, b);
            }
        }
    }

    /// Adds (sign 1) or removes (sign -1) machine `m`'s belt-access demand: spread evenly over the free band
    /// `access_range` tiles wide around its footprint, bin by bin.
    fn spread_access(&mut self, m: usize, sign: f64) {
        if self.access_need[m] == 0.0 {
            return;
        }
        let r = self.tun.access_range;
        let (x0, y0) = (0.max(self.px[m] - r), 0.max(self.py[m] - r));
        let (x1, y1) = ((self.w - 1).min(self.px[m] + self.mw[m] - 1 + r), (self.h - 1).min(self.py[m] + self.mh[m] - 1 + r));
        let (fx0, fy0, fx1, fy1) = (self.px[m], self.py[m], self.px[m] + self.mw[m] - 1, self.py[m] + self.mh[m] - 1);
        let band = (x1 - x0 + 1) * (y1 - y0 + 1) - self.mw[m] * self.mh[m];
        if band <= 0 {
            return;
        }
        let d = self.access_need[m] / f64::from(band) * sign;
        let overlap = |a0: i32, a1: i32, b0: i32, b1: i32| 0.max(a1.min(b1) - a0.max(b0) + 1);
        let (bin, bw) = (self.bin, self.bw);
        for by in y0 / bin..=y1 / bin {
            for bx in x0 / bin..=x1 / bin {
                let (bx0, by0) = (bx * bin, by * bin);
                let (bx1, by1) = (bx0 + bin - 1, by0 + bin - 1);
                let tiles = overlap(x0, x1, bx0, bx1) * overlap(y0, y1, by0, by1) - overlap(fx0, fx1, bx0, bx1) * overlap(fy0, fy1, by0, by1);
                if tiles <= 0 {
                    continue;
                }
                let b = (by * bw + bx) as usize;
                self.bin_d[b] += d * f64::from(tiles);
                mark(&mut self.dirty, &mut self.dirty_list, self.dirty_gen, b);
            }
        }
    }

    // ---------------- pins and DI ----------------
    fn free_slots(&self, m: usize) -> i32 {
        let (x, y) = (self.px[m], self.py[m]);
        let (x2, y2) = (x + self.mw[m] - 1, y + self.mh[m] - 1);
        let free = |x: i32, y: i32| self.in_box(x, y) && self.is_free(self.idx(x, y));
        let slot = |ax: i32, ay: i32, bx: i32, by: i32| i32::from(free(ax, ay) && free(bx, by));
        let mut f = 0;
        for xx in x..=x2 {
            f += slot(xx, y - 1, xx, y - 2) + slot(xx, y2 + 1, xx, y2 + 2);
        }
        for yy in y..=y2 {
            f += slot(x - 1, yy, x - 2, yy) + slot(x2 + 1, yy, x2 + 2, yy);
        }
        f
    }

    /// Belt inserter slots machine `m` needs: each terminal's k, scaled down by the share of its rate that
    /// fitting direct-insertion neighbours already cover (their inserters sit in the gap, not on free slots).
    fn slots_needed(&self, m: usize) -> i32 {
        if self.machine_pairs[m].is_empty() {
            return self.pin_need[m];
        }
        let mut need = 0;
        for &t in &self.machine_terms[m] {
            let term = &self.plan.terminals[t];
            let mut covered = 0.0;
            for &i in &self.machine_pairs[m] {
                if !self.di_fit[i] {
                    continue;
                }
                if self.di_ct[i] == t {
                    covered += self.plan.terminals[self.di_pt[i]].rate;
                }
                if self.di_pt[i] == t {
                    covered += self.plan.terminals[self.di_ct[i]].rate;
                }
            }
            let left = (1.0 - covered / term.rate).max(0.0);
            need += (f64::from(term.k) * left - 1e-9).ceil() as i32;
        }
        need
    }

    fn pin_penalty(&self, m: usize) -> f64 {
        let s = 0.max(self.slots_needed(m) + 1 - self.free_slots(m));
        f64::from(s * s)
    }

    fn pair_fits(&self, a: usize, b: usize) -> bool {
        let (ax, ay, aw, ah) = (self.px[a], self.py[a], self.mw[a], self.mh[a]);
        let (bx, by, bw, bh) = (self.px[b], self.py[b], self.mw[b], self.mh[b]);
        let (ax2, ay2, bx2, by2) = (ax + aw - 1, ay + ah - 1, bx + bw - 1, by + bh - 1);
        let yo = ay.max(by) <= ay2.min(by2);
        let xo = ax.max(bx) <= ax2.min(bx2);
        let gx = (bx - ax2 - 1).max(ax - bx2 - 1);
        let gy = (by - ay2 - 1).max(ay - by2 - 1);
        (yo && ((gx == 1 && self.ok5) || (gx == 3 && self.ok6))) || (xo && ((gy == 1 && self.ok5) || (gy == 3 && self.ok6)))
    }

    /// Credit for a direct-insertion pair: 1 when the gap fits (I5/I6), otherwise a small share that fades
    /// over a few tiles, so the annealer feels which way to move. Only exact fits remove inserter slots.
    fn closeness(&self, a: usize, b: usize) -> f64 {
        if self.pair_fits(a, b) {
            return 1.0;
        }
        let (ax, ay, aw, ah) = (self.px[a], self.py[a], self.mw[a], self.mh[a]);
        let (bx, by, bw, bh) = (self.px[b], self.py[b], self.mw[b], self.mh[b]);
        let (ax2, ay2, bx2, by2) = (ax + aw - 1, ay + ah - 1, bx + bw - 1, by + bh - 1);
        let oy = ay2.min(by2) - ay.max(by) + 1;
        let ox = ax2.min(bx2) - ax.max(bx) + 1;
        let gx = (bx - ax2 - 1).max(ax - bx2 - 1);
        let gy = (by - ay2 - 1).max(ay - by2 - 1);
        let off = |g: i32| if self.ok6 { (g - 1).abs().min((g - 3).abs()) } else { (g - 1).abs() };
        // Distance in tiles from a fitting position: gap error plus missing overlap.
        let dh = off(gx) + 0.max(1 - oy);
        let dv = off(gy) + 0.max(1 - ox);
        let d = dh.min(dv);
        if d >= 4 { 0.0 } else { 0.35 * f64::from(4 - d) / 4.0 }
    }

    /// Machines whose pin count may change when the rect area around `m` changes.
    fn neighbours(&self, m: usize, out: &mut OrderedSet) {
        for yy in self.py[m] - 4..self.py[m] + self.mh[m] + 4 {
            for xx in self.px[m] - 4..self.px[m] + self.mw[m] + 4 {
                if !self.in_box(xx, yy) {
                    continue;
                }
                let o = self.occ[self.idx(xx, yy)];
                if o > 0 {
                    out.insert(o as usize - 1);
                }
            }
        }
    }

    // ---------------- full and incremental evaluation ----------------
    /// Places every machine from scratch and recomputes all terms.
    pub fn set_all(&mut self, xs: &[i32], ys: &[i32], rs: &[i32]) {
        self.clear();
        for m in 0..self.n_m {
            self.place_one(m, xs[m], ys[m], rs[m]);
        }
        self.finish_placing();
    }

    /// Removes every machine (start of an initial placement).
    pub fn clear(&mut self) {
        self.next_dirty_generation();
        for m in 0..self.n_m {
            if self.is_placed[m] {
                self.stamp(m, false);
                self.is_placed[m] = false;
            }
        }
        self.placed = false;
    }

    /// Stamps one machine during initial placement; costs are computed by `finish_placing`.
    pub fn place_one(&mut self, m: usize, x: i32, y: i32, r: i32) {
        self.px[m] = x;
        self.py[m] = y;
        self.rot[m] = r;
        self.stamp(m, true);
        self.is_placed[m] = true;
    }

    pub fn finish_placing(&mut self) {
        assert!(self.is_placed.iter().all(|&p| p), "not every machine is placed");
        self.placed = true;
        self.recompute_all();
    }

    pub fn recompute_all(&mut self) {
        self.bin_d.fill(0.0);
        self.wl = 0.0;
        for n in 0..self.n_nets {
            self.compute_net(n);
            self.wl += self.net_weight[n] * self.net_hpwl[n];
            self.spread(n, 1.0);
        }
        for m in 0..self.n_m {
            self.spread_access(m, 1.0);
        }
        self.cong = 0.0;
        self.fb = 0.0;
        for b in 0..self.bin_d.len() {
            self.bin_cong[b] = self.cong_of(b);
            self.cong += self.bin_cong[b];
            self.bin_fb[b] = self.bin_hist[b] * self.bin_d[b];
            self.fb += self.bin_fb[b];
        }
        self.di = 0.0;
        for i in 0..self.di_a.len() {
            self.di_fit[i] = self.pair_fits(self.di_a[i], self.di_b[i]);
            self.di_score[i] = self.closeness(self.di_a[i], self.di_b[i]);
            self.di += self.di_gain[i] * self.di_score[i];
        }
        self.pin = 0.0;
        for m in 0..self.n_m {
            self.pin_pen[m] = self.pin_penalty(m);
            self.pin += self.pin_pen[m];
        }
    }

    fn cong_of(&self, b: usize) -> f64 {
        let o = self.bin_d[b] - f64::from(self.bin_s[b]);
        if o > 0.0 { o * o } else { 0.0 }
    }

    fn next_dirty_generation(&mut self) {
        self.dirty_gen = self.dirty_gen.wrapping_add(1);
        if self.dirty_gen == 0 {
            self.dirty.fill(0);
            self.dirty_gen = 1;
        }
        self.dirty_list.clear();
    }

    /// Moves `ms` to the new positions (assumed legal) and updates every term incrementally.
    pub fn move_machines(&mut self, ms: &[usize], nx: &[i32], ny: &[i32], nr: &[i32]) {
        self.next_dirty_generation();
        let mut touched = std::mem::take(&mut self.touched_nets);
        let mut pin_machines = std::mem::take(&mut self.pin_machines);
        let mut pairs = std::mem::take(&mut self.pair_set);
        touched.clear();
        pin_machines.clear();
        pairs.clear();
        for &m in ms {
            for &n in &self.machine_nets[m] {
                touched.insert(n);
            }
            self.neighbours(m, &mut pin_machines);
        }

        for &n in &touched.items {
            self.wl -= self.net_weight[n] * self.net_hpwl[n];
            self.spread(n, -1.0);
        }
        for &m in ms {
            self.spread_access(m, -1.0);
        }
        for &m in ms {
            self.stamp(m, false);
        }
        for (i, &m) in ms.iter().enumerate() {
            self.px[m] = nx[i];
            self.py[m] = ny[i];
            self.rot[m] = nr[i];
        }
        for &m in ms {
            self.stamp(m, true);
        }
        for &m in ms {
            self.spread_access(m, 1.0);
        }
        for &n in &touched.items {
            self.compute_net(n);
            self.wl += self.net_weight[n] * self.net_hpwl[n];
            self.spread(n, 1.0);
        }

        for i in 0..self.dirty_list.len() {
            let b = self.dirty_list[i];
            self.cong -= self.bin_cong[b];
            self.bin_cong[b] = self.cong_of(b);
            self.cong += self.bin_cong[b];
            self.fb -= self.bin_fb[b];
            self.bin_fb[b] = self.bin_hist[b] * self.bin_d[b];
            self.fb += self.bin_fb[b];
        }
        for &m in ms {
            self.neighbours(m, &mut pin_machines);
        }

        for &m in ms {
            for &i in &self.machine_pairs[m] {
                pairs.insert(i);
            }
        }
        java_hash_order(&mut pairs.items);
        for &i in &pairs.items {
            let f = self.pair_fits(self.di_a[i], self.di_b[i]);
            if f != self.di_fit[i] {
                self.di_fit[i] = f;
                pin_machines.insert(self.di_a[i]);
                pin_machines.insert(self.di_b[i]);
            }
            let c = self.closeness(self.di_a[i], self.di_b[i]);
            if c != self.di_score[i] {
                self.di += self.di_gain[i] * (c - self.di_score[i]);
                self.di_score[i] = c;
            }
        }
        // Pin penalties are whole numbers, so their sum does not depend on the order.
        for &m in &pin_machines.items {
            self.pin -= self.pin_pen[m];
            self.pin_pen[m] = self.pin_penalty(m);
            self.pin += self.pin_pen[m];
        }
        self.touched_nets = touched;
        self.pin_machines = pin_machines;
        self.pair_set = pairs;
    }

    /// Routing feedback: PathFinder history per tile, summed per bin (in the order given).
    pub fn set_history(&mut self, history: &[(Tile, f64)], weight: f64) {
        self.bin_hist.fill(0.0);
        for (t, v) in history {
            if self.in_box(t.x, t.y) {
                self.bin_hist[((t.y / self.bin) * self.bw + t.x / self.bin) as usize] += v;
            }
        }
        self.beta2 = weight;
        self.recompute_all();
    }

    pub fn placement(&self) -> Placement {
        Placement {
            w: self.w,
            h: self.h,
            machines: (0..self.n_m)
                .map(|i| PlacedMachine { id: i, x: self.px[i], y: self.py[i], rot: Dir::ALL[(self.rot[i] % 4) as usize], mirror: self.rot[i] >= 4 })
                .collect(),
        }
    }

    pub fn di_pairs_fitting(&self) -> Vec<(usize, usize)> {
        (0..self.di_a.len()).filter(|&i| self.di_fit[i]).map(|i| (self.di_a[i], self.di_b[i])).collect()
    }
}

fn port_geom(plan: &ProductionPlan, pid: usize, w: i32, h: i32) -> PortGeom {
    fn edge(segments: &mut Vec<[i32; 4]>, e: &str, from: Option<i32>, to: Option<i32>, w: i32, h: i32) {
        match e {
            "north" => segments.push([from.unwrap_or(0).max(0), 0, (w - 1).min(to.unwrap_or(w - 1)), 0]),
            "south" => segments.push([from.unwrap_or(0).max(0), h - 1, (w - 1).min(to.unwrap_or(w - 1)), h - 1]),
            "west" => segments.push([0, from.unwrap_or(0).max(0), 0, (h - 1).min(to.unwrap_or(h - 1))]),
            "east" => segments.push([w - 1, from.unwrap_or(0).max(0), w - 1, (h - 1).min(to.unwrap_or(h - 1))]),
            "any" => {
                for side in ["north", "east", "south", "west"] {
                    edge(segments, side, None, None, w, h);
                }
            }
            _ => {}
        }
    }
    let (mut fixed, mut segments) = (Vec::new(), Vec::new());
    for s in &plan.ports[pid].specs {
        match &s.edge {
            Some(e) => edge(&mut segments, e, s.from, s.to, w, h),
            None => fixed.push(Tile::new(s.x.expect("a port without an edge has x"), s.y.expect("a port without an edge has y"))),
        }
    }
    PortGeom { fixed, segments }
}
