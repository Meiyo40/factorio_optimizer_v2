//! The routing core of `PathFinder`: lane bookkeeping, A* over (tile, direction, belt or underground exit),
//! committing and undoing legs, routing one net as a chain of legs with backtracking, branch starts and sinks.

use super::access::AccessOption;
use super::path_finder::{AK_BELT, AK_SPLITTER, AK_UG_IN, AK_UG_OUT, BELT, PathFinder, REPAIR, RNet, Routed, UG_EXIT, role_initial};
use crate::data::game_constants as gc;
use crate::geom::{Dir, Tile};
use crate::plan::Role;
use crate::util::text::fmt2;
use crate::viz::{HeatOverlay, RenderOptions};
use crate::util::{average_f64, compare_f32, compare_f64, sum_f64};
use std::collections::{HashSet, VecDeque};

#[derive(Clone, Copy)]
pub(super) struct Start {
    pub state: i32,
    pub g: f32,
    pub opt: i32,
}

/// One committed leg of a chain, kept so that a later leg that finds no path can undo it and ban its goal state.
struct LegRec {
    starts: Vec<Start>,
    leg1: bool,
    belts: usize,
    ug_in: usize,
    ug_out: usize,
    chans: usize,
    claims: usize,
    tiles: usize,
    source_tile: i32,
    banned: HashSet<i32>,
    done: i32,
    splitters: usize,
    dead_ends: usize,
    reserved: usize,
    branch_from_belt: i32,
    lanes: [f64; 2],
}

impl PathFinder<'_> {
    // ---- chain order: a consumer slot may be claimed once the supply upstream of it covers its share ----
    pub(super) fn share(&self, v: usize) -> f64 {
        self.vts[v].rate / f64::from(self.vts[v].k)
    }

    pub(super) fn claimable(&self, v: i32) -> bool {
        let v = v as usize;
        self.need[v] > 0 && (self.vts[v].role() == Role::Producer || self.surplus[self.vts[v].slot as usize] >= self.share(v) - 1e-9)
    }

    pub(super) fn lane_cap(&self) -> f64 {
        self.tier.throughput / 2.0 * 0.98
    }

    /// Lane option `o`'s inserter drops onto when the belt at its tile heads `d` and items enter it heading `into`
    /// (a curve when they differ), as the validator models it (`gc::drop_lane`).
    pub(super) fn drop_lane(&self, o: i32, d: i32, into: i32) -> i32 {
        let (belt, ins) = (self.opt_belt[o as usize] as usize, self.opt_ins[o as usize] as usize);
        // Long-handed options sit two tiles from their belt, so compare by direction only.
        let dx = (self.dims.x(belt) - self.dims.x(ins)).signum();
        let dy = (self.dims.y(belt) - self.dims.y(ins)).signum();
        let swing = Dir::ALL.into_iter().find(|dir| dir.dx() == dx && dir.dy() == dy).expect("an axis-aligned offset");
        gc::drop_lane(swing, Dir::ALL[into as usize], Dir::ALL[d as usize])
    }

    /// A producer option is usable only if its lane still has room, and on a shared belt only if it drops onto
    /// its item's lane; consumers take from either lane.
    pub(super) fn lane_ok(&self, o: i32, d: i32, into: i32) -> bool {
        let v = self.opt_term[o as usize] as usize;
        if self.vts[v].role() != Role::Producer {
            return true;
        }
        let lane = self.drop_lane(o, d, into);
        if self.paired && lane != self.vts[v].slot {
            return false;
        }
        self.lane_load[lane as usize] + self.share(v) <= self.lane_cap() + 1e-9
    }

    fn remaining_visits(&self, n: &RNet) -> i32 {
        n.terms.iter().map(|&t| self.need[t as usize].max(0)).sum()
    }

    // ---- A* ----
    /// One leg: from `starts` to any option of a terminal still needing a visit.
    /// Returns the index into `done_state`/`done_opt`, or -1.
    pub(super) fn search(&mut self, n: &RNet, starts: &[Start], final_: bool, leg1: bool, banned: &HashSet<i32>) -> i32 {
        self.forbid_gen += 1;
        for _ in 0..=self.tun.repair_restarts {
            let r = self.search_once(n, starts, final_, leg1, banned);
            if r != REPAIR {
                return r;
            }
        }
        -1
    }

    /// One A* pass; returns a done index, -1 for no path, or `REPAIR` when it forbade a state and should rerun.
    fn search_once(&mut self, n: &RNet, starts: &[Start], final_: bool, leg1: bool, banned: &HashSet<i32>) -> i32 {
        self.search_gen += 1;
        self.heap.clear();
        self.done_state.clear();
        self.done_opt.clear();
        let mut repaired = false;
        // Goal tiles and the L1 distance transform for the heuristic.
        self.goal_gen += 1;
        if leg1 {
            for s in starts {
                self.start_stamp[(s.state >> 3) as usize] = self.goal_gen;
            }
        }
        let mut any_goal = false;
        if self.output_leg {
            for (i, &t) in n.out_tiles.iter().enumerate() {
                self.goal_stamp[t as usize] = self.goal_gen;
                self.out_dir_at[t as usize] = n.out_out[i];
                any_goal = true;
            }
        } else {
            for &t in &n.terms {
                if !self.claimable(t) || (self.leg_target >= 0 && t != self.leg_target) {
                    continue;
                }
                for k in 0..self.term_opts[t as usize].len() {
                    let o = self.term_opts[t as usize][k];
                    if self.goal_usable(o, leg1) {
                        self.goal_stamp[self.opt_belt[o as usize] as usize] = self.goal_gen;
                        any_goal = true;
                    }
                }
            }
        }
        if !any_goal {
            return -1;
        }
        // Leg 1: a start option whose inserter tile is a goal's belt tile would void itself on the way.
        let filtered: Vec<Start>;
        let usable: &[Start] = if leg1 {
            filtered = starts.iter().copied().filter(|s| s.opt < 0 || self.goal_stamp[self.opt_ins[s.opt as usize] as usize] != self.goal_gen).collect();
            if filtered.is_empty() { starts } else { &filtered }
        } else {
            starts
        };
        self.distance_transform();
        // Keep off the access tiles of consumers that cannot be served yet, or they are lost for later.
        for &t in &n.terms {
            let tu = t as usize;
            if self.need[tu] <= 0 || self.vts[tu].role() != Role::Consumer || self.claimable(t) {
                continue;
            }
            for k in 0..self.term_opts[tu].len() {
                let o = self.term_opts[tu][k] as usize;
                self.res_stamp[self.opt_belt[o] as usize] = self.goal_gen;
                self.res_stamp[self.opt_ins[o] as usize] = self.goal_gen;
            }
        }
        let res = self.tun.own_consumer_reservation as f32;
        for s in usable {
            let tile = (s.state >> 3) as usize;
            let su = s.state as usize;
            if self.forbid_stamp[su] == self.forbid_gen {
                continue;
            }
            if self.seen_gen[su] != self.search_gen || s.g < self.g_arr[su] {
                self.seen_gen[su] = self.search_gen;
                self.g_arr[su] = s.g;
                self.par[su] = -1;
                self.heap.push(s.g + self.h_weight * self.min_step * self.dist[tile] as f32, s.state);
            }
            if leg1 {
                self.start_opt[tile] = s.opt;
            }
        }
        let mut budget = self.tun.max_expansions;
        while !self.heap.is_empty() {
            let s = self.heap.pop();
            if s >= self.s_states {
                let di = (s - self.s_states) as usize;
                let bad = self.valid_done(self.done_state[di], self.done_opt[di], final_, leg1);
                if bad == -1 {
                    return di as i32;
                }
                if bad >= 0 && self.forbid_stamp[bad as usize] != self.forbid_gen {
                    self.forbid_stamp[bad as usize] = self.forbid_gen;
                    repaired = true;
                }
                continue;
            }
            let su = s as usize;
            if self.closed_gen[su] == self.search_gen {
                continue;
            }
            self.closed_gen[su] = self.search_gen;
            budget -= 1;
            if budget < 0 {
                self.log.push(format!("net {}: search budget exhausted", n.plan_id));
                return -1;
            }
            self.expansions += 1;
            let t = s >> 3;
            let d = (s >> 1) & 3;
            let kind = s & 1;
            let g_s = self.g_arr[su];
            let tu = t as usize;

            // Output leg: a belt on a port tile facing out of the box.
            if self.output_leg
                && kind == BELT
                && self.goal_stamp[tu] == self.goal_gen
                && self.par[su] != -1
                && !banned.contains(&s)
                && (self.out_dir_at[tu] < 0 || self.out_dir_at[tu] == d)
            {
                self.done_state.push(s);
                self.done_opt.push(-1);
                let key = g_s + self.dead_end_cost(t + self.d_idx[d as usize], d);
                self.heap.push(key, self.s_states + self.done_state.len() as i32 - 1);
            }
            // Goal: a belt on an option's belt tile (not the very start state).
            if !self.output_leg && kind == BELT && self.goal_stamp[tu] == self.goal_gen && self.par[su] != -1 && !banned.contains(&s) {
                let t2 = t + self.d_idx[d as usize];
                for k in self.at_belt_start[tu]..self.at_belt_start[tu + 1] {
                    let o = self.at_belt_list[k as usize];
                    let term = self.opt_term[o as usize];
                    // Items enter this tile heading the way the previous state heads: a curve when that differs.
                    let into = if self.par[su] != -1 { (self.par[su] >> 1) & 3 } else { d };
                    if self.term_net[term as usize] != n.idx
                        || !self.claimable(term)
                        || (self.leg_target >= 0 && term != self.leg_target)
                        || !self.lane_ok(o, d, into)
                    {
                        continue;
                    }
                    let ins = self.opt_ins[o as usize];
                    if !self.goal_usable(o, leg1) {
                        continue;
                    }
                    let extra: f32 = if final_ {
                        if t2 >= 0 && (t2 as usize) < self.n && self.own_belt_stamp[t2 as usize] == self.stamp {
                            continue;
                        }
                        self.dead_end_cost(t2, d)
                    } else {
                        if !self.placeable(t2) || t2 == ins || !self.can_continue(t, d, t2) {
                            continue;
                        }
                        0f32
                    };
                    self.done_state.push(s);
                    self.done_opt.push(o);
                    let key = g_s + self.ins_cost(ins, self.opt_long[o as usize]) + extra;
                    self.heap.push(key, self.s_states + self.done_state.len() as i32 - 1);
                }
            }

            // Belt step (B1).
            let q = t + self.d_idx[d as usize];
            if !self.placeable(q) {
                continue;
            }
            let res_q = if self.res_stamp[q as usize] == self.goal_gen { res } else { 0f32 };
            for nd in 0..4 {
                if nd == (d + 2) % 4 {
                    continue;
                }
                if kind == UG_EXIT && nd != d && !self.tun.turn_after_ug_exit {
                    continue;
                }
                let cost = g_s
                    + self.belt_cost(q, self.dead_ends_taken(q, AK_BELT, nd))
                    + res_q
                    + (if nd != d { self.tun.turn_penalty as f32 } else { 0f32 });
                self.relax(Self::st(q, nd, BELT), s, cost);
            }

            // Underground (B2, B3): entrance on q facing d, exit j tiles further.
            if self.ug_allowed {
                let axis = if d == 1 || d == 3 { 0 } else { 1 };
                let bc = self.belt_cost(q, self.dead_ends_taken(q, AK_UG_IN, d)) + res_q;
                let qx = self.dims.x(q as usize);
                let qy = self.dims.y(q as usize);
                let n_i = self.n as i32;
                // Each tier has its own channels; the exit state keeps the cheaper, and the commit picks it again.
                for ti in 0..(if self.weave_ok { 2 } else { 1 }) {
                    let ch0 = ti * 2 * n_i + axis * n_i;
                    if self.own_chan_stamp[(ch0 + q) as usize] == self.stamp {
                        continue;
                    }
                    let mut chan_acc = self.chan_cost(ch0 + q) + (if ti == 1 { self.tun.weave_penalty as f32 } else { 0f32 });
                    for j in 1..=(if ti == 0 { self.max_j } else { self.max_jw }) {
                        let ex = qx + self.d_x[d as usize] * j;
                        let ey = qy + self.d_y[d as usize] * j;
                        if !self.dims.in_full(ex, ey) {
                            break;
                        }
                        let e = q + self.d_idx[d as usize] * j;
                        if self.own_chan_stamp[(ch0 + e) as usize] == self.stamp || self.no_tunnel[e as usize] {
                            break;
                        }
                        chan_acc += self.chan_cost(ch0 + e);
                        if !self.placeable(e) {
                            continue;
                        }
                        let cost = g_s
                            + bc
                            + self.belt_cost(e, self.dead_ends_taken(e, AK_UG_OUT, d))
                            + (if self.res_stamp[e as usize] == self.goal_gen { res } else { 0f32 })
                            + (self.tun.ug_penalty + self.tun.ug_penalty_per_tile * f64::from(j)) as f32
                            + chan_acc;
                        self.relax(Self::st(e, d, UG_EXIT), s, cost);
                    }
                }
            }
        }
        if repaired { REPAIR } else { -1 }
    }

    /// A goal option is usable if its inserter tile is free of this net and, in leg 1, not a start tile.
    fn goal_usable(&self, o: i32, leg1: bool) -> bool {
        let ins = self.opt_ins[o as usize] as usize;
        self.own_stamp[ins] != self.stamp && !(leg1 && self.start_stamp[ins] == self.goal_gen)
    }

    /// Look-ahead for a leg's end: from `t2` (where the head at `t` points) the next leg must have somewhere to go.
    fn can_continue(&self, t: i32, d: i32, t2: i32) -> bool {
        for dd in 0..4 {
            let x = t2 + self.d_idx[dd];
            if x != t && self.placeable(x) {
                return true;
            }
        }
        if !self.ug_allowed {
            return false;
        }
        let axis = if d == 1 || d == 3 { 0 } else { 1 };
        let (x0, y0) = (self.dims.x(t2 as usize), self.dims.y(t2 as usize));
        let n_i = self.n as i32;
        for ti in 0..(if self.weave_ok { 2 } else { 1 }) {
            let ch0 = ti * 2 * n_i + axis * n_i;
            if self.own_chan_stamp[(ch0 + t2) as usize] == self.stamp {
                continue;
            }
            for j in 1..=(if ti == 0 { self.max_j } else { self.max_jw }) {
                if !self.dims.in_full(x0 + self.d_x[d as usize] * j, y0 + self.d_y[d as usize] * j) {
                    break;
                }
                let e = t2 + self.d_idx[d as usize] * j;
                if self.own_chan_stamp[(ch0 + e) as usize] == self.stamp || self.no_tunnel[e as usize] {
                    break;
                }
                if self.placeable(e) {
                    return true;
                }
            }
        }
        false
    }

    fn relax(&mut self, ns: i32, from: i32, g: f32) {
        let nu = ns as usize;
        if self.closed_gen[nu] == self.search_gen || self.forbid_stamp[nu] == self.forbid_gen {
            return;
        }
        if self.seen_gen[nu] == self.search_gen && g >= self.g_arr[nu] {
            return;
        }
        self.seen_gen[nu] = self.search_gen;
        self.g_arr[nu] = g;
        self.par[nu] = from;
        self.heap.push(g + self.h_weight * self.min_step * self.dist[(ns >> 3) as usize] as f32, ns);
    }

    /// L1 distance from every tile to the nearest goal tile (two-pass chamfer).
    pub(super) fn distance_transform(&mut self) {
        let big = 1 << 20;
        for i in 0..self.n {
            self.dist[i] = if self.goal_stamp[i] == self.goal_gen { 0 } else { big };
        }
        let fw = self.fw as usize;
        let fh = self.dims.full_h as usize;
        for y in 0..fh {
            for x in 0..fw {
                let i = y * fw + x;
                let mut v = self.dist[i];
                if x > 0 {
                    v = v.min(self.dist[i - 1] + 1);
                }
                if y > 0 {
                    v = v.min(self.dist[i - fw] + 1);
                }
                self.dist[i] = v;
            }
        }
        for y in (0..fh).rev() {
            for x in (0..fw).rev() {
                let i = y * fw + x;
                let mut v = self.dist[i];
                if x < fw - 1 {
                    v = v.min(self.dist[i + 1] + 1);
                }
                if y < fh - 1 {
                    v = v.min(self.dist[i + fw] + 1);
                }
                self.dist[i] = v;
            }
        }
    }

    /// Path states in order, from the start to `s`.
    fn path_states(&self, s: i32) -> Vec<i32> {
        let mut out = Vec::new();
        let mut c = s;
        while c != -1 {
            out.push(c);
            c = self.par[c as usize];
        }
        out.reverse();
        out
    }

    /// Checks a found leg against what the state graph cannot see: the path may not cross itself, run over
    /// the inserter tiles it claims, or end pointing back into itself. Returns -1 if valid, else a path
    /// state to forbid before searching again (-2 if there is none to blame).
    fn valid_done(&mut self, s: i32, o: i32, final_: bool, leg1: bool) -> i32 {
        self.path_gen += 1;
        let states = self.path_states(s);
        // Later legs start on the previous head (already committed) or on a new branch tile (not yet).
        let skip_first = !leg1 && self.own_stamp[(states[0] >> 3) as usize] == self.stamp;
        let n_i = self.n as i32;
        for (i, &st) in states.iter().enumerate() {
            let t = st >> 3;
            let d = (st >> 1) & 3;
            if i == 0 && skip_first {
                continue;
            }
            if self.path_stamp[t as usize] == self.path_gen {
                return self.path_first[t as usize];
            }
            self.path_stamp[t as usize] = self.path_gen;
            self.path_first[t as usize] = st;
            if (st & 1) == UG_EXIT {
                let ent = (states[i - 1] >> 3) + self.d_idx[d as usize];
                if self.path_stamp[ent as usize] == self.path_gen {
                    return st;
                }
                self.path_stamp[ent as usize] = self.path_gen;
                self.path_first[ent as usize] = st;
                let axis = if d == 1 || d == 3 { 0 } else { 1 };
                let mut c = ent;
                loop {
                    let ci = (axis * n_i + c) as usize;
                    if self.path_chan_stamp[ci] == self.path_gen {
                        return st;
                    }
                    self.path_chan_stamp[ci] = self.path_gen;
                    if c == t {
                        break;
                    }
                    c += self.d_idx[d as usize];
                }
            }
        }
        let last0 = *states.last().expect("a path has a state");
        if o < 0 {
            // output leg: only the path itself must be clean
            let t2 = (last0 >> 3) + self.d_idx[((last0 >> 1) & 3) as usize];
            return if self.path_stamp[t2 as usize] == self.path_gen { self.path_first[t2 as usize] } else { -1 };
        }
        let ins = self.opt_ins[o as usize];
        if self.path_stamp[ins as usize] == self.path_gen {
            return self.path_first[ins as usize];
        }
        if leg1 {
            let so = self.start_opt[(states[0] >> 3) as usize];
            if so >= 0 && self.opt_ins[so as usize] == ins {
                return -2;
            }
            if so >= 0 && self.path_stamp[self.opt_ins[so as usize] as usize] == self.path_gen {
                return self.path_first[self.opt_ins[so as usize] as usize];
            }
        }
        let t2 = (last0 >> 3) + self.d_idx[((last0 >> 1) & 3) as usize];
        if self.path_stamp[t2 as usize] == self.path_gen {
            return self.path_first[t2 as usize];
        }
        if !final_ && t2 == ins {
            return -2;
        }
        -1
    }

    // ---- commit / rip-up ----
    fn occupy(&mut self, r: &mut Routed, t: i32, belt: bool) {
        let tu = t as usize;
        self.occ_tile[tu] += 1;
        if belt {
            self.occ_belt[tu] += 1;
            self.own_belt_stamp[tu] = self.stamp;
        }
        self.own_stamp[tu] = self.stamp;
        r.tiles.push(t);
    }

    fn claim(&mut self, r: &mut Routed, o: i32, d: i32, into: i32) {
        r.claims.push(o);
        self.occupy(r, self.opt_ins[o as usize], false);
        let v = self.opt_term[o as usize] as usize;
        self.need[v] -= 1;
        let slot = self.vts[v].slot as usize;
        let share = self.share(v);
        self.surplus[slot] += if self.vts[v].role() == Role::Producer { share } else { -share };
        if self.vts[v].role() == Role::Producer {
            let lane = self.drop_lane(o, d, into) as usize;
            self.lane_load[lane] += share;
        } else if self.paired {
            self.lane_load[slot] = f64::max(0.0, self.lane_load[slot] - share);
        } else {
            let total = self.lane_load[0] + self.lane_load[1];
            if total > 1e-12 {
                let keep = f64::max(0.0, 1.0 - share / total);
                self.lane_load[0] *= keep;
                self.lane_load[1] *= keep;
            }
        }
    }

    /// Takes net `i`'s route out of the congestion counts and forgets it.
    pub(super) fn rip_up(&mut self, i: usize) {
        if let Some(r) = self.routed[i].take() {
            self.rip_up_routed(&r);
        }
    }

    fn rip_up_routed(&mut self, r: &Routed) {
        for &e in &r.belts {
            let t = e >> 2;
            self.occ_tile[t as usize] -= 1;
            self.occ_belt[t as usize] -= 1;
            self.accept(t, AK_BELT, e & 3, -1);
        }
        for &e in &r.ug_in {
            let t = e >> 2;
            self.occ_tile[t as usize] -= 1;
            self.occ_belt[t as usize] -= 1;
            self.accept(t, AK_UG_IN, e & 3, -1);
        }
        for &e in &r.ug_out {
            let t = e >> 2;
            self.occ_tile[t as usize] -= 1;
            self.occ_belt[t as usize] -= 1;
            self.accept(t, AK_UG_OUT, e & 3, -1);
        }
        for &o in &r.claims {
            self.occ_tile[self.opt_ins[o as usize] as usize] -= 1;
        }
        for &c in &r.chans {
            self.occ_chan[c as usize] -= 1;
        }
        for sp in &r.splitters {
            self.occ_tile[sp[1] as usize] -= 1;
            self.occ_belt[sp[1] as usize] -= 1;
            self.accept(sp[1], AK_SPLITTER, sp[2], -1);
        }
        for &e in &r.dead_ends {
            self.remove_dead_end(e);
        }
    }

    /// Puts a ripped-up chain's route back as it was (endgame rollback).
    pub(super) fn restore(&mut self, i: usize, r: Option<Routed>) {
        let Some(r) = r else { return };
        for &e in &r.belts {
            let t = e >> 2;
            self.occ_tile[t as usize] += 1;
            self.occ_belt[t as usize] += 1;
            self.accept(t, AK_BELT, e & 3, 1);
        }
        for &e in &r.ug_in {
            let t = e >> 2;
            self.occ_tile[t as usize] += 1;
            self.occ_belt[t as usize] += 1;
            self.accept(t, AK_UG_IN, e & 3, 1);
        }
        for &e in &r.ug_out {
            let t = e >> 2;
            self.occ_tile[t as usize] += 1;
            self.occ_belt[t as usize] += 1;
            self.accept(t, AK_UG_OUT, e & 3, 1);
        }
        for &o in &r.claims {
            self.occ_tile[self.opt_ins[o as usize] as usize] += 1;
        }
        for &c in &r.chans {
            self.occ_chan[c as usize] += 1;
        }
        for sp in &r.splitters {
            self.occ_tile[sp[1] as usize] += 1;
            self.occ_belt[sp[1] as usize] += 1;
            self.accept(sp[1], AK_SPLITTER, sp[2], 1);
        }
        for &e in &r.dead_ends {
            self.dead_end_dir[e as usize] += 1;
            self.dead_end_at[(e >> 2) as usize] += 1;
        }
        self.routed[i] = Some(r);
    }

    /// Tier for an underground from `ent` to `ex` heading `d`: the one the search priced lower (main on a tie).
    fn ug_tier_for(&self, ent: i32, ex: i32, d: i32) -> i32 {
        if !self.weave_ok {
            return 0;
        }
        let axis = if d == 1 || d == 3 { 0 } else { 1 };
        let n_i = self.n as i32;
        let mut j = 0;
        let mut c = ent;
        while c != ex {
            c += self.d_idx[d as usize];
            j += 1;
        }
        let mut best = 0;
        let mut best_cost = f32::MAX;
        for ti in 0..=1 {
            if j > (if ti == 0 { self.max_j } else { self.max_jw }) {
                continue;
            }
            let ch0 = ti * 2 * n_i + axis * n_i;
            let mut cost = if ti == 1 { self.tun.weave_penalty as f32 } else { 0f32 };
            let mut c = ent;
            let mut ok = true;
            loop {
                if self.own_chan_stamp[(ch0 + c) as usize] == self.stamp {
                    ok = false;
                    break;
                }
                cost += self.chan_cost(ch0 + c);
                if c == ex {
                    break;
                }
                c += self.d_idx[d as usize];
            }
            if ok && cost < best_cost {
                best_cost = cost;
                best = ti;
            }
        }
        best
    }

    /// Commits a found leg: belts, undergrounds, the goal option and options the path passed on the way.
    fn commit_leg(&mut self, n: &RNet, r: &mut Routed, s: i32, goal_opt: i32, leg1: bool) {
        let states = self.path_states(s);
        let n_i = self.n as i32;
        // 1. Occupy every path tile first, so no claim below can take a tile the belt runs over.
        for (i, &st) in states.iter().enumerate() {
            let t = st >> 3;
            let d = (st >> 1) & 3;
            if i == 0 && !leg1 {
                if self.own_stamp[t as usize] == self.stamp {
                    continue;
                }
                // A branch: a splitter across the trunk tile and this tile.
                r.splitters.push([*self.branch_from.get(&t).expect("a branch start has its trunk tile"), t, d]);
                self.occupy(r, t, true);
                self.own_dir[t as usize] = d;
                self.accept(t, AK_SPLITTER, d, 1);
                continue;
            }
            if (st & 1) == UG_EXIT {
                let ent = (states[i - 1] >> 3) + self.d_idx[d as usize];
                let ti = self.ug_tier_for(ent, t, d);
                r.ug_in.push(ent * 4 + d);
                self.occupy(r, ent, true);
                self.own_dir[ent as usize] = -1;
                self.accept(ent, AK_UG_IN, d, 1);
                r.ug_out.push(t * 4 + d);
                self.occupy(r, t, true);
                self.own_dir[t as usize] = d;
                self.accept(t, AK_UG_OUT, d, 1);
                r.ug_tier.push(ti);
                let ch0 = ti * 2 * n_i + (if d == 1 || d == 3 { 0 } else { 1 }) * n_i;
                let mut c = ent;
                loop {
                    let ci = ch0 + c;
                    self.occ_chan[ci as usize] += 1;
                    self.own_chan_stamp[ci as usize] = self.stamp;
                    r.chans.push(ci);
                    if c == t {
                        break;
                    }
                    c += self.d_idx[d as usize];
                }
            } else {
                r.belts.push(t * 4 + d);
                self.occupy(r, t, true);
                self.own_dir[t as usize] = d;
                self.accept(t, AK_BELT, d, 1);
            }
        }
        let last = *states.last().expect("a path has a state");
        let next = (last >> 3) + self.d_idx[((last >> 1) & 3) as usize]; // where the chain continues (or dead-ends)
        // Passing claims skip an inserter tile another net holds when the terminal has a free option further on.
        let mut last_free: std::collections::HashMap<i32, usize> = std::collections::HashMap::new();
        for (i, &st) in states.iter().enumerate() {
            if i == 0 || (st & 1) != BELT {
                continue;
            }
            let t = (st >> 3) as usize;
            for k in self.at_belt_start[t]..self.at_belt_start[t + 1] {
                let o = self.at_belt_list[k as usize] as usize;
                let ins = self.opt_ins[o];
                if self.term_net[self.opt_term[o] as usize] == n.idx && self.occ_tile[ins as usize] == 0 && self.allow_net[ins as usize] != -2 && ins != next {
                    last_free.insert(self.opt_term[o], i);
                }
            }
        }
        // 2. Claims in path order: the start option, options passed on the way, then the goal.
        for (i, &st) in states.iter().enumerate() {
            if (st & 1) != BELT {
                continue;
            }
            let t = (st >> 3) as usize;
            let d = (st >> 1) & 3;
            if i == 0 {
                if leg1 {
                    let so = self.start_opt[t];
                    if so >= 0 {
                        self.claim(r, so, d, d);
                    } else {
                        r.source_tile = t as i32;
                    }
                }
                continue;
            }
            let into = (states[i - 1] >> 1) & 3;
            if goal_opt >= 0
                && i == states.len() - 1
                && self.claimable(self.opt_term[goal_opt as usize])
                && self.own_stamp[self.opt_ins[goal_opt as usize] as usize] != self.stamp
                && self.lane_ok(goal_opt, d, into)
            {
                self.claim(r, goal_opt, d, into);
            }
            for k in self.at_belt_start[t]..self.at_belt_start[t + 1] {
                let o = self.at_belt_list[k as usize];
                let term = self.opt_term[o as usize];
                if self.term_net[term as usize] != n.idx || !self.claimable(term) || !self.lane_ok(o, d, into) {
                    continue;
                }
                let ins = self.opt_ins[o as usize];
                if self.own_stamp[ins as usize] == self.stamp || self.allow_net[ins as usize] == -2 || ins == next {
                    continue;
                }
                if self.occ_tile[ins as usize] > 0 && last_free.get(&term).map_or(-1, |&v| v as i64) >= i as i64 {
                    continue;
                }
                self.claim(r, o, d, into);
            }
        }
    }

    // ---- routing one net ----
    /// Routes one net, trying other start producers (and start options) when the chain gets stuck.
    pub(super) fn route_net(&mut self, n: &RNet) -> bool {
        if self.tun.keep_chain_order
            && let Some(remembered) = self.net_order[n.idx as usize].clone()
        {
            let start = if n.port_tiles.is_empty() { remembered[0] } else { -1 };
            self.order = Some(remembered);
            let ok = self.route_net_from(n, start, None);
            self.order = None;
            if ok {
                return true;
            }
        }
        if !n.port_tiles.is_empty() {
            return self.route_net_from(n, -1, None);
        }
        let producers = self.ranked_producers(n);
        let mut tries = 0;
        for &pt in &producers {
            if tries >= self.tun.start_attempts {
                break;
            }
            let others = producers.iter().any(|&p| p != pt);
            // With k > 1 and no other producer to head for, leg 1 must start on one option so the terminal's other
            // options can be its goals; otherwise start on all of them at once.
            let choices: Vec<Option<Vec<i32>>> = if self.term_k[pt as usize] > 1 && !others {
                self.rank_options_far_first(n, &self.term_opts[pt as usize].clone()).into_iter().take(4).map(|o| Some(vec![o])).collect()
            } else {
                vec![None]
            };
            for c in choices {
                let attempt = tries;
                tries += 1;
                if attempt >= self.tun.start_attempts + 3 {
                    break;
                }
                if self.route_net_from(n, pt, c.as_deref()) {
                    return true;
                }
            }
        }
        false
    }

    /// Routes one net as a chain from producer `start_term` (-1: from its port), on `start_opts` or all its options.
    fn route_net_from(&mut self, n: &RNet, start_term: i32, start_opts: Option<&[i32]>) -> bool {
        self.stamp += 1;
        self.cur = n.idx;
        // Weaving undergrounds of a slower tier only on chains whose flow fits that tier.
        self.weave_ok = self
            .weave
            .is_some_and(|w| self.ug_allowed && n.source_rate + sum_f64(n.terms.iter().filter(|&&t| self.vts[t as usize].role() == Role::Producer).map(|&t| self.vts[t as usize].rate)) <= w.throughput * self.tun.weave_belt_share);
        for &t in &n.terms {
            self.need[t as usize] = self.term_k[t as usize];
        }
        self.paired = n.item2.is_some();
        self.surplus = [n.source_rate, n.source_rate2];
        self.lane_load = if self.paired { [n.source_rate, n.source_rate2] } else { [n.source_rate / 2.0, n.source_rate / 2.0] };
        let mut r = Routed::new();

        let mut starts: Vec<Start> = Vec::new();
        if start_term < 0 {
            for (i, &t) in n.port_tiles.iter().enumerate() {
                if !self.placeable(t) {
                    continue;
                }
                let c = self.belt_cost(t, self.dead_end_at[t as usize]);
                // A shared belt, or one whose port moves onto the ring, enters straight from the edge (lanes stay put).
                let straight = self.paired
                    || (self.config.ports_on_ring && n.port_out[i] >= 0 && self.dims.in_box(self.dims.x(t as usize), self.dims.y(t as usize)));
                for d in 0..4 {
                    if d != n.port_out[i] && (!straight || d == (n.port_out[i] + 2) % 4) {
                        starts.push(Start { state: Self::st(t, d, BELT), g: c, opt: -1 });
                    }
                }
            }
        } else {
            let all_opts;
            let opts: &[i32] = match start_opts {
                Some(o) => o,
                None => {
                    all_opts = self.term_opts[start_term as usize].clone();
                    &all_opts
                }
            };
            for &o in opts.iter().filter(|&&o| self.placeable(self.opt_belt[o as usize]) && self.placeable(self.opt_ins[o as usize])) {
                let belt = self.opt_belt[o as usize];
                let c = self.belt_cost(belt, self.dead_end_at[belt as usize]) + self.ins_cost(self.opt_ins[o as usize], self.opt_long[o as usize]);
                let term = self.opt_term[o as usize] as usize;
                for d in 0..4 {
                    if !self.paired || self.vts[term].role() != Role::Producer || self.drop_lane(o, d, d) == self.vts[term].slot {
                        starts.push(Start { state: Self::st(belt, d, BELT), g: c, opt: o });
                    }
                }
            }
        }
        if starts.is_empty() {
            return self.fail(n, "no free start tile".to_owned(), &r);
        }

        // Legs are committed on a stack; when a leg finds no path, the previous one is undone and its goal
        // state banned, so the chain can end that leg somewhere that does not trap it.
        // A chain fed only by its port may branch anywhere; producers on it must all be passed first.
        if start_term < 0 && n.terms.iter().all(|&t| self.vts[t as usize].role() != Role::Producer) {
            r.branch_from_belt = 0;
        }
        let mut legs: Vec<LegRec> = Vec::new();
        let mut leg1 = true;
        let mut current: Vec<Start> = starts;
        let mut banned: HashSet<i32> = HashSet::new();
        let mut backtracks = 0;
        loop {
            // The start option counts as visited while searching leg 1; claim() books it on commit.
            if leg1 && start_term >= 0 {
                let slot = self.vts[start_term as usize].slot as usize;
                self.need[start_term as usize] -= 1;
                self.surplus[slot] += self.share(start_term as usize);
            }
            self.output_leg = !n.out_tiles.is_empty() && n.terms.iter().all(|&t| self.need[t as usize] <= 0);
            // The last terminal ends the chain only when there is no output port to go on to.
            let final_ = self.output_leg || (self.remaining_visits(n) == 1 && n.out_tiles.is_empty());
            // Following a remembered order: head for the first terminal in it that still needs a visit.
            self.leg_target = match &self.order {
                Some(o) => o.iter().copied().find(|&t| self.need[t as usize] > 0 && self.claimable(t)).unwrap_or(-1),
                None => -1,
            };
            let head = if leg1 { -1 } else { current[0].state };
            let starts: Vec<Start> = if leg1 {
                current.clone()
            } else {
                let mut s = current.clone();
                s.extend(self.branch_starts(&r, head));
                s
            };
            let found = self.search(n, &starts, final_, leg1, &banned);
            self.leg_target = -1;
            let was_output_leg = self.output_leg;
            self.output_leg = false;
            if leg1 && start_term >= 0 {
                let slot = self.vts[start_term as usize].slot as usize;
                self.need[start_term as usize] += 1;
                self.surplus[slot] -= self.share(start_term as usize);
            }
            if found < 0 {
                if !legs.is_empty() && backtracks < self.tun.max_backtracks {
                    backtracks += 1;
                    let mut last = legs.pop().expect("checked");
                    self.undo_leg(&mut r, &last);
                    while r.splitters.len() > last.splitters {
                        let sp = r.splitters.pop().expect("longer than last.splitters");
                        self.occ_belt[sp[1] as usize] -= 1;
                        self.own_belt_stamp[sp[1] as usize] = 0;
                        self.accept(sp[1], AK_SPLITTER, sp[2], -1);
                    }
                    while r.dead_ends.len() > last.dead_ends {
                        let e = r.dead_ends.pop().expect("longer than last.dead_ends");
                        self.remove_dead_end(e);
                    }
                    while r.reserved.len() > last.reserved {
                        let t = r.reserved.pop().expect("longer than last.reserved");
                        self.own_stamp[t as usize] = 0;
                    }
                    r.branch_from_belt = last.branch_from_belt;
                    self.lane_load = last.lanes;
                    r.source_tile = last.source_tile;
                    last.banned.insert(last.done);
                    current = last.starts;
                    leg1 = last.leg1;
                    banned = last.banned;
                    continue;
                }
                let detail = n
                    .terms
                    .iter()
                    .filter(|&&t| self.need[t as usize] > 0)
                    .map(|&t| {
                        let tu = t as usize;
                        let free = self.term_opts[tu]
                            .iter()
                            .filter(|&&o| self.own_stamp[self.opt_ins[o as usize] as usize] != self.stamp && self.own_stamp[self.opt_belt[o as usize] as usize] != self.stamp)
                            .count();
                        format!("t{t} {} need {} free opts {free}/{}", role_initial(self.vts[tu].role()), self.need[tu], self.term_opts[tu].len())
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let reach = if legs.is_empty() {
                    // Diagnostic: belt-step reachability from the start tiles over tiles this net may use.
                    let mut seen: HashSet<i32> = HashSet::new();
                    let mut q: VecDeque<i32> = VecDeque::new();
                    for s0 in &current {
                        let t = s0.state >> 3;
                        if seen.insert(t) {
                            q.push_back(t);
                        }
                    }
                    while let Some(c) = q.pop_front() {
                        for dd in 0..4 {
                            let x = c + self.d_idx[dd];
                            if x >= 0 && (x as usize) < self.n && self.placeable(x) && seen.insert(x) {
                                q.push_back(x);
                            }
                        }
                    }
                    let g: usize = n
                        .terms
                        .iter()
                        .filter(|&&t| self.claimable(t))
                        .map(|&t| self.term_opts[t as usize].iter().filter(|&&o| seen.contains(&self.opt_belt[o as usize])).count())
                        .sum();
                    format!(" starts {} reach {} goals reachable {g}", current.len(), seen.len())
                } else {
                    String::new()
                };
                let msg = format!(
                    "leg {} found no path after {backtracks} backtracks (surplus {}, final {final_}): {detail}{reach}",
                    legs.len() + 1,
                    fmt2(sum_f64(self.surplus))
                );
                return self.fail(n, msg, &r);
            }
            let s = self.done_state[found as usize];
            legs.push(LegRec {
                starts: current.clone(),
                leg1,
                belts: r.belts.len(),
                ug_in: r.ug_in.len(),
                ug_out: r.ug_out.len(),
                chans: r.chans.len(),
                claims: r.claims.len(),
                tiles: r.tiles.len(),
                source_tile: r.source_tile,
                banned: std::mem::take(&mut banned),
                done: s,
                splitters: r.splitters.len(),
                dead_ends: r.dead_ends.len(),
                reserved: r.reserved.len(),
                branch_from_belt: r.branch_from_belt,
                lanes: self.lane_load,
            });
            // A leg that starts on a branch leaves the old head as a leaf.
            let mut first = s;
            while self.par[first as usize] != -1 {
                first = self.par[first as usize];
            }
            let branched = !leg1 && (first >> 3) != (head >> 3);
            let goal_opt = self.done_opt[found as usize];
            self.commit_leg(n, &mut r, s, goal_opt, leg1);
            if branched {
                let hd = (head >> 1) & 3;
                let t2 = (head >> 3) + self.d_idx[hd as usize];
                if t2 >= 0 && (t2 as usize) < self.n {
                    self.add_dead_end(&mut r, t2, hd);
                    if self.own_stamp[t2 as usize] != self.stamp {
                        self.own_stamp[t2 as usize] = self.stamp;
                        r.reserved.push(t2);
                    }
                }
            }
            if r.branch_from_belt < 0 && !n.terms.iter().any(|&t| self.vts[t as usize].role() == Role::Producer && self.need[t as usize] > 0) {
                r.branch_from_belt = r.belts.len() as i32;
            }
            leg1 = false;
            if was_output_leg {
                r.out_tile = s >> 3;
            }
            if n.terms.iter().all(|&t| self.need[t as usize] <= 0) && (n.out_tiles.is_empty() || was_output_leg) {
                let hd = (s >> 1) & 3;
                let t2 = (s >> 3) + self.d_idx[hd as usize];
                if t2 >= 0 && (t2 as usize) < self.n {
                    self.add_dead_end(&mut r, t2, hd);
                }
                r.ok = true;
                let mut order: Vec<i32> = Vec::new();
                for &o in &r.claims {
                    let term = self.opt_term[o as usize];
                    if !order.contains(&term) {
                        order.push(term);
                    }
                }
                self.net_order[n.idx as usize] = Some(order);
                self.routed[n.idx as usize] = Some(r);
                return true;
            }
            current = vec![Start { state: s, g: 0f32, opt: -1 }];
        }
    }

    /// Branch starts (B9): a splitter on a straight trunk belt placed after every producer was visited, with
    /// its second half beside it; the branch leaves from that half. The leg's search picks a branch only
    /// when it beats continuing from the head.
    fn branch_starts(&mut self, r: &Routed, head: i32) -> Vec<Start> {
        self.branch_from.clear();
        if !self.config.trick("B9") || r.branch_from_belt < 0 {
            return Vec::new();
        }
        let mut out = Vec::new();
        let head_tile = head >> 3;
        let claimed_belts: HashSet<i32> = r.claims.iter().map(|&o| self.opt_belt[o as usize]).collect();
        let split_tiles: HashSet<i32> = r.splitters.iter().flat_map(|sp| [sp[0], sp[1]]).collect();
        let n_i = self.n as i32;
        let in_grid = |t: i32| t >= 0 && t < n_i;
        for k in r.branch_from_belt as usize..r.belts.len() {
            let e = r.belts[k];
            let b = e >> 2;
            let h = e & 3;
            if b == head_tile || claimed_belts.contains(&b) || split_tiles.contains(&b) {
                continue;
            }
            let behind = b - self.d_idx[h as usize];
            let front = b + self.d_idx[h as usize];
            if !in_grid(behind) || self.own_belt_stamp[behind as usize] != self.stamp || self.own_dir[behind as usize] != h || split_tiles.contains(&behind) {
                continue;
            }
            if !in_grid(front) || self.own_belt_stamp[front as usize] != self.stamp {
                continue;
            }
            for side in [(h + 1) % 4, (h + 3) % 4] {
                let s_tile = b + self.d_idx[side as usize];
                if !self.placeable(s_tile) || !self.placeable(s_tile + self.d_idx[h as usize]) {
                    continue;
                }
                let back = s_tile - self.d_idx[h as usize];
                if in_grid(back) && self.own_belt_stamp[back as usize] == self.stamp {
                    continue;
                }
                // Anything belt-like behind the second half would feed foreign items into the splitter.
                let foreign = if in_grid(back) && self.occ_belt[back as usize] > 0 { self.tun.dead_end_conflict as f32 * 10f32 } else { 0f32 };
                out.push(Start {
                    state: Self::st(s_tile, h, BELT),
                    g: self.belt_cost(s_tile, self.dead_end_at[s_tile as usize]) + self.tun.splitter_cost as f32 + foreign,
                    opt: -1,
                });
                self.branch_from.insert(s_tile, b);
            }
        }
        out
    }

    /// Takes back everything committed after the list sizes `to` recorded (one or more legs).
    fn undo_leg(&mut self, r: &mut Routed, to: &LegRec) {
        let LegRec { belts, ug_in, ug_out, chans, claims, tiles, .. } = *to;
        while r.claims.len() > claims {
            let o = r.claims.pop().expect("longer than claims");
            let v = self.opt_term[o as usize] as usize;
            self.need[v] += 1;
            let slot = self.vts[v].slot as usize;
            let share = self.share(v);
            self.surplus[slot] -= if self.vts[v].role() == Role::Producer { share } else { -share };
        }
        while r.belts.len() > belts {
            let e = r.belts.pop().expect("longer than belts");
            let t = e >> 2;
            self.occ_belt[t as usize] -= 1;
            self.own_belt_stamp[t as usize] = 0;
            self.accept(t, AK_BELT, e & 3, -1);
        }
        while r.ug_in.len() > ug_in {
            let e = r.ug_in.pop().expect("longer than ug_in");
            let t = e >> 2;
            self.occ_belt[t as usize] -= 1;
            self.own_belt_stamp[t as usize] = 0;
            self.accept(t, AK_UG_IN, e & 3, -1);
        }
        while r.ug_out.len() > ug_out {
            let e = r.ug_out.pop().expect("longer than ug_out");
            let t = e >> 2;
            self.occ_belt[t as usize] -= 1;
            self.own_belt_stamp[t as usize] = 0;
            self.accept(t, AK_UG_OUT, e & 3, -1);
        }
        while r.ug_tier.len() > ug_out {
            r.ug_tier.pop();
        }
        while r.chans.len() > chans {
            let c = r.chans.pop().expect("longer than chans");
            self.occ_chan[c as usize] -= 1;
            self.own_chan_stamp[c as usize] = 0;
        }
        while r.tiles.len() > tiles {
            let t = r.tiles.pop().expect("longer than tiles");
            self.occ_tile[t as usize] -= 1;
            self.own_stamp[t as usize] = 0;
        }
    }

    /// Gives up on the net: logs why, takes back whatever it had committed and reports failure.
    fn fail(&mut self, n: &RNet, why: String, r: &Routed) -> bool {
        self.log.push(format!("net {} ({}): {why}", n.plan_id, n.item));
        self.render_failure(n, &why, r);
        self.rip_up_routed(r);
        self.routed[n.idx as usize] = None;
        false
    }

    /// With a debug dir, the first six failed nets are drawn: the tiles of their access options as heat, the head of
    /// the route they got as far as in red.
    fn render_failure(&mut self, n: &RNet, why: &str, r: &Routed) {
        let Some(dir) = self.p.debug_dir.clone() else { return };
        let drawn = self.fail_renders;
        self.fail_renders += 1;
        if drawn >= 6 {
            return;
        }
        let mut heat: indexmap::IndexMap<Tile, f64> = indexmap::IndexMap::new();
        for &t in n.terms.iter().filter(|&&t| self.need[t as usize] > 0) {
            for &o in &self.term_opts[t as usize] {
                heat.insert(self.dims.tile(self.opt_belt[o as usize] as usize), 1.0);
                heat.insert(self.dims.tile(self.opt_ins[o as usize] as usize), 0.4);
            }
        }
        let head: HashSet<Tile> = r.belts.last().map(|&b| self.dims.tile((b >> 2) as usize)).into_iter().collect();
        let options = RenderOptions {
            px: 28,
            heat: Some(HeatOverlay { values: heat.into_iter().collect(), overused: head }),
            title: Some(format!("net {} {}: {why}", n.plan_id, n.item)),
            ..RenderOptions::default()
        };
        // The route so far is not stored yet: show it too.
        let stored = self.routed[n.idx as usize].replace(r.clone());
        self.render_debug(&dir.join(format!("fail_{:02}_net{}.png", self.fail_renders, n.plan_id)), &options);
        self.routed[n.idx as usize] = stored;
    }

    pub(super) fn machine_center_of(&self, m: usize) -> Tile {
        let pm = self.p.placement.machines.iter().find(|pm| pm.id == m).expect("a placed machine");
        let r = self.p.placement.rect(self.plan, pm);
        Tile::new(r.x + r.w / 2, r.y + r.h / 2)
    }

    fn machine_center(&self, term: usize) -> Tile {
        self.machine_center_of(self.vts[term].machine().expect("a machine terminal"))
    }

    fn consumer_centroid(&self, n: &RNet) -> (f64, f64) {
        let cs: Vec<Tile> = n.terms.iter().filter(|&&t| self.vts[t as usize].role() == Role::Consumer).map(|&t| self.machine_center(t as usize)).collect();
        (average_f64(cs.iter().map(|t| f64::from(t.x))), average_f64(cs.iter().map(|t| f64::from(t.y))))
    }

    /// Producers by distance from the consumers, farthest first, so the chain flows towards them.
    fn ranked_producers(&self, n: &RNet) -> Vec<i32> {
        let (cx, cy) = self.consumer_centroid(n);
        let mut keyed: Vec<(f64, i32)> = n
            .terms
            .iter()
            .copied()
            .filter(|&t| self.vts[t as usize].role() == Role::Producer && !self.term_opts[t as usize].is_empty())
            .map(|t| {
                let c = self.machine_center(t as usize);
                ((f64::from(c.x) - cx).abs() + (f64::from(c.y) - cy).abs(), t)
            })
            .collect();
        keyed.sort_by(|a, b| compare_f64(b.0, a.0));
        keyed.into_iter().map(|k| k.1).collect()
    }

    fn rank_options_far_first(&self, n: &RNet, opts: &[i32]) -> Vec<i32> {
        let (cx, cy) = self.consumer_centroid(n);
        // Called between attempts: the stamp and current chain still belong to the last one, so only the
        // static allowance counts here (route_net_from checks the rest).
        let open = |q: i32| self.allow_net[q as usize] == -1 || self.allow_net[q as usize] == n.idx;
        let mut keyed: Vec<(f64, i32)> = opts
            .iter()
            .copied()
            .filter(|&o| open(self.opt_belt[o as usize]) && open(self.opt_ins[o as usize]))
            .map(|o| {
                let t = self.dims.tile(self.opt_belt[o as usize] as usize);
                ((f64::from(t.x) - cx).abs() + (f64::from(t.y) - cy).abs(), o)
            })
            .collect();
        keyed.sort_by(|a, b| compare_f64(b.0, a.0));
        keyed.into_iter().map(|k| k.1).collect()
    }

    // ---- sinks (I7) ----
    /// Gives each sink terminal its cheapest inserter-and-chest options, in the order Kotlin's hash map of
    /// sinks iterates: each terminal's claims change the prices the next one sees.
    pub(super) fn solve_sinks(&mut self) {
        for (o, chest) in std::mem::take(&mut self.sink_claims) {
            self.occ_tile[self.dims.idx_tile(o.ins_tile)] -= 1;
            self.occ_tile[self.dims.idx_tile(chest)] -= 1;
        }
        let terms: Vec<usize> = self.sink_options.iter_java().map(|(k, _)| *k).collect();
        for term in terms {
            let k = self.sink_k[&term];
            let opts: Vec<AccessOption> = self.sink_options.get(&term).expect("a sink terminal").clone();
            let mut used: HashSet<usize> = HashSet::new();
            let mut ranked: Vec<(f32, AccessOption)> = opts
                .into_iter()
                .map(|o| {
                    let chest = self.sink_chest_tile(&o);
                    (self.ins_cost(self.dims.idx_tile(o.ins_tile) as i32, false) + self.chest_cost(self.dims.idx_tile(chest) as i32), o)
                })
                .collect();
            ranked.sort_by(|a, b| compare_f32(a.0, b.0));
            let mut taken = 0;
            for (_, o) in &ranked {
                if taken == k {
                    break;
                }
                let chest = self.sink_chest_tile(o);
                let a = self.dims.idx_tile(o.ins_tile);
                let b = self.dims.idx_tile(chest);
                if used.contains(&a) || used.contains(&b) {
                    continue;
                }
                used.insert(a);
                used.insert(b);
                self.occ_tile[a] += 1;
                self.occ_tile[b] += 1;
                self.sink_claims.push((o.clone(), chest));
                taken += 1;
            }
            if taken < k {
                self.log.push(format!("sink terminal {term}: only {taken} of {k} options"));
            }
        }
    }

    fn sink_chest_tile(&self, o: &AccessOption) -> Tile {
        o.belt_tile
    }
}
