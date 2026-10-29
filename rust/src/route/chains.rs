//! The setup half of `PathFinder`: how nets become chains (belts that visit machines), shared belts, the access
//! options of every chain terminal, and, at the end, the entities a routed result is made of.

use super::access::{AccessOption, EffectiveTerminal, access_options};
use super::path_finder::{PathFinder, RNet, Vt};
use crate::data::game_constants as gc;
use crate::geom::{Dir, Kind, PlacedEntity, Rect, Tile};
use crate::place::{orientable, port_candidates};
use crate::plan::{Net, Role, Terminal, best_inserter_rate, inserter_count, inserter_rate};
use crate::util::text::fmt2;
use crate::util::{average_f64, compare_f64, sum_f64};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

fn ceil_div(a: i32, b: i32) -> i32 {
    (a + b - 1) / b
}

fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).abs() + (a.1 - b.1).abs()
}

/// A chain's share of a terminal, with an inserter count of its own unless it takes the whole rate.
fn part(t: &EffectiveTerminal, rate: f64, per_inserter: f64) -> EffectiveTerminal {
    EffectiveTerminal { base: t.base.clone(), rate, k: if (rate - t.rate).abs() < 1e-9 { t.k } else { inserter_count(rate, per_inserter) } }
}

/// The index of the first entry with the smallest value among those `eligible` (`minByOrNull` keeps the first).
fn first_min(n: usize, eligible: impl Fn(usize) -> bool, value: impl Fn(usize) -> f64) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for i in (0..n).filter(|&i| eligible(i)) {
        let v = value(i);
        if best.is_none_or(|(_, bv)| v < bv) {
            best = Some((i, v));
        }
    }
    best.map(|b| b.0)
}

impl<'a> PathFinder<'a> {
    /// Candidate tiles of plan port `port` in this box, each with the direction that points out (or None).
    pub(super) fn port_tiles_of(&self, port: usize) -> Vec<(Tile, Option<Dir>)> {
        port_candidates(&self.plan.ports[port].specs, self.w, self.h)
            .into_iter()
            .filter(|(t, _)| self.dims.in_full(t.x, t.y) && (self.dims.in_box(t.x, t.y) || self.is_ring1(*t)))
            .collect()
    }

    /// Keeps a port's free-standing tiles for the chain that starts or ends there.
    fn reserve(&mut self, cands: &[(Tile, Option<Dir>)], idx: i32) {
        for (t, out) in cands {
            let i = self.dims.idx_tile(*t);
            if out.is_none() && (self.allow_net[i] != -2 || !self.dims.in_box(t.x, t.y)) {
                self.allow_net[i] = idx;
            }
        }
    }

    fn center_tile(&self, rect_of: &HashMap<usize, Rect>, vts: &[Vt]) -> Vec<Tile> {
        vts.iter()
            .filter_map(|v| v.machine().map(|m| rect_of[&m]))
            .map(|r| Tile::new(r.x + r.w / 2, r.y + r.h / 2))
            .collect()
    }

    fn hpwl_of(pts: &[Tile]) -> i32 {
        if pts.is_empty() {
            return 0;
        }
        let (xs, ys) = (pts.iter().map(|t| t.x), pts.iter().map(|t| t.y));
        (xs.clone().max().expect("not empty") - xs.min().expect("not empty")) + (ys.clone().max().expect("not empty") - ys.min().expect("not empty"))
    }

    /// Nets that still have a producer side and a consumer side, split into chains where needed.
    pub(super) fn build_chains(&mut self) {
        let plan = self.plan;
        let config = self.config;
        let placement = self.p.placement;
        let tun = self.tun.clone();
        let rect_of: HashMap<usize, Rect> = placement.machines.iter().map(|m| (m.id, placement.rect(plan, m))).collect();
        let center = |t: &Terminal| -> (f64, f64) {
            let r = rect_of[&t.machine.expect("a machine terminal")];
            (f64::from(r.x) + f64::from(r.w) / 2.0, f64::from(r.y) + f64::from(r.h) / 2.0)
        };
        let drop_rate = best_inserter_rate(config, false);
        let belt_cap = self.tier.throughput * tun.chain_belt_share;
        for net in plan.nets.iter().filter(|n| !n.fluid) {
            let ts: Vec<EffectiveTerminal> = net.terminals.iter().filter_map(|t| self.eff.get(t).cloned()).collect();
            let consumers: Vec<EffectiveTerminal> = ts.iter().filter(|t| t.base.role == Role::Consumer && t.base.machine.is_some()).cloned().collect();
            let producers: Vec<EffectiveTerminal> = ts.iter().filter(|t| t.base.role == Role::Producer).cloned().collect();
            let source = ts.iter().find(|t| t.base.role == Role::Source).cloned();
            let output = ts.iter().find(|t| t.base.role == Role::Output).cloned();
            if (consumers.is_empty() && output.is_none()) || (producers.is_empty() && source.is_none()) {
                continue;
            }
            let sources: Vec<EffectiveTerminal> = ts.iter().filter(|t| t.base.role == Role::Source).cloned().collect();
            let outputs: Vec<EffectiveTerminal> = ts.iter().filter(|t| t.base.role == Role::Output).cloned().collect();
            let single = sources.iter().chain(&outputs).any(|t| plan.ports[t.base.port.expect("a port terminal")].single);
            if single || sources.len() > 1 || outputs.len() > 1 || (!sources.is_empty() && !producers.is_empty()) {
                let pick_rate = best_inserter_rate(config, true);
                self.build_general_chains(net, &consumers, &producers, &sources, &outputs, &center, &rect_of, drop_rate, pick_rate, belt_cap);
                continue;
            }
            let visits: i32 = consumers.iter().map(|t| t.k).sum::<i32>() + producers.iter().map(|t| t.k).sum::<i32>();
            let flow = sum_f64(consumers.iter().map(|t| t.rate)) + output.as_ref().map_or(0.0, |o| o.rate);
            // Every chain starts on its own port tile (and ends on one at an output port): a port narrower than
            // the visits would split into gets fewer, longer chains, which branch with splitters instead.
            let room = source
                .as_ref()
                .map_or(i32::MAX, |s| self.port_tiles_of(s.base.port.expect("a port terminal")).len() as i32)
                .min(output.as_ref().map_or(i32::MAX, |o| self.port_tiles_of(o.base.port.expect("a port terminal")).len() as i32));
            let g0 = ceil_div(visits, tun.max_chain_visits).min(room).max((flow / belt_cap - 1e-9).ceil() as i32).max(1);
            // Each chain ends at the output port when there is one, so chains need not have consumers.
            let g = if output.is_some() { g0 } else { g0.min(consumers.len() as i32) };
            let groups: Vec<Vec<EffectiveTerminal>> = if g == 1 {
                vec![consumers.clone()]
            } else if consumers.len() as i32 >= g {
                self.group_consumers(&consumers, g as usize, &center)
            } else {
                consumers.iter().map(|c| vec![c.clone()]).chain((0..g as usize - consumers.len()).map(|_| Vec::new())).collect()
            };
            let out_share = output.as_ref().map_or(0.0, |o| o.rate) / groups.len() as f64;
            let out_cands: Vec<(Tile, Option<Dir>)> = output.as_ref().map_or_else(Vec::new, |o| self.port_tiles_of(o.base.port.expect("a port terminal")));

            let mut remaining: Vec<f64> = producers.iter().map(|p| p.rate).collect();
            for group in &groups {
                let idx = self.nets.len() as i32;
                let demand = sum_f64(group.iter().map(|t| t.rate)) + out_share;
                let mut members: Vec<Vt> = group.iter().map(|c| Vt { base: c.base.clone(), rate: c.rate, k: c.k, net: idx, slot: 0 }).collect();
                if source.is_none() {
                    // Nearest producers with spare output supply this chain (around its consumers, else all producers).
                    let spare: Vec<&EffectiveTerminal> = producers.iter().enumerate().filter(|(i, _)| remaining[*i] > 1e-9).map(|(_, p)| p).collect();
                    let around: Vec<&EffectiveTerminal> = if !group.is_empty() {
                        group.iter().collect()
                    } else if !spare.is_empty() {
                        spare
                    } else {
                        producers.iter().collect()
                    };
                    let gx = average_f64(around.iter().map(|t| center(&t.base).0));
                    let gy = average_f64(around.iter().map(|t| center(&t.base).1));
                    let mut left = demand;
                    while left > 1e-9 {
                        let Some(pi) = first_min(producers.len(), |i| remaining[i] > 1e-9, |i| {
                            let (x, y) = center(&producers[i].base);
                            (x - gx).abs() + (y - gy).abs()
                        }) else {
                            break;
                        };
                        let take = left.min(remaining[pi]);
                        remaining[pi] -= take;
                        left -= take;
                        // A producer whose whole spare goes here keeps its k; a share gets its own count.
                        let pr = &producers[pi];
                        let k = if (take - pr.rate).abs() < 1e-9 { pr.k } else { inserter_count(take, drop_rate) };
                        members.push(Vt { base: pr.base.clone(), rate: take, k, net: idx, slot: 0 });
                    }
                    if left > 1e-6 {
                        self.log.push(format!("net {} ({}): chain short of {}/s supply", net.id, net.item, fmt2(left)));
                    }
                }
                let first_vt = self.vts.len();
                let member_ids: Vec<i32> = (first_vt..first_vt + members.len()).map(|i| i as i32).collect();
                let pts_of_members = self.center_tile(&rect_of, &members);
                self.vts.extend(members);
                let mut port_tiles: Vec<i32> = Vec::new();
                let mut port_out: Vec<i32> = Vec::new();
                if let Some(src) = &source {
                    let cands = self.port_tiles_of(src.base.port.expect("a port terminal"));
                    port_tiles = cands.iter().map(|(t, _)| self.dims.idx_tile(*t) as i32).collect();
                    port_out = cands.iter().map(|(_, d)| d.map_or(-1, |d| d.ordinal() as i32)).collect();
                    // Fixed ports (no outward edge) belong to this chain alone; edge segments stay shared.
                    self.reserve(&cands, idx);
                }
                self.reserve(&out_cands, idx);
                let mut pts = pts_of_members;
                pts.extend(port_tiles.iter().map(|&t| self.dims.tile(t as usize)));
                pts.extend(out_cands.iter().take(1).map(|c| c.0));
                // Unlike addChain's, this hpwl has no empty case: a chain always has a point.
                let hpwl = Self::hpwl_of(&pts);
                let net_entry = RNet {
                    idx,
                    plan_id: net.id as i32,
                    item: net.item.clone(),
                    terms: member_ids,
                    port_tiles,
                    port_out,
                    hpwl,
                    source_rate: if source.is_some() { demand } else { 0.0 },
                    out_tiles: out_cands.iter().map(|c| self.dims.idx_tile(c.0) as i32).collect(),
                    out_out: out_cands.iter().map(|c| c.1.map_or(-1, |d| d.ordinal() as i32)).collect(),
                    out_rate: out_share,
                    item2: None,
                    source_rate2: 0.0,
                    src_port: source.as_ref().and_then(|s| s.base.port).map_or(-1, |p| p as i32),
                    out_port: output.as_ref().and_then(|o| o.base.port).map_or(-1, |p| p as i32),
                };
                Rc::make_mut(&mut self.nets).push(net_entry);
            }
        }
    }

    /// Two items per belt: merges pairs of single chains whose consumer machines mostly coincide and whose flow
    /// fits one lane each into one chain carrying one item per lane (slot 0 left, slot 1 right). A port-fed item
    /// takes slot 0; two port-fed items need an edge tile both ports allow. Chains with fixed-tile ports, output
    /// ports or single-belt ports (links between the blocks of a factory) keep a belt of their own.
    pub(super) fn pair_chains(&mut self) {
        let nets: Vec<RNet> = self.nets.as_ref().clone();
        let flow = |n: &RNet| n.source_rate + sum_f64(n.terms.iter().filter(|&&t| self.vts[t as usize].role() == Role::Producer).map(|&t| self.vts[t as usize].rate));
        let consumers = |n: &RNet| -> Vec<usize> {
            let mut out: Vec<usize> = Vec::new();
            for &t in &n.terms {
                let v = &self.vts[t as usize];
                if v.role() == Role::Consumer
                    && let Some(m) = v.machine()
                    && !out.contains(&m)
                {
                    out.push(m);
                }
            }
            out
        };
        let edge_ports = |n: &RNet| -> HashMap<i32, i32> { n.port_tiles.iter().copied().zip(n.port_out.iter().copied()).collect() };
        let mut chains_of: HashMap<i32, usize> = HashMap::new();
        for n in &nets {
            *chains_of.entry(n.plan_id).or_insert(0) += 1;
        }
        let lane_cap = self.lane_cap();
        let cand: Vec<&RNet> = nets
            .iter()
            .filter(|n| {
                chains_of[&n.plan_id] == 1
                    && n.out_tiles.is_empty()
                    && flow(n) <= lane_cap
                    && consumers(n).len() >= 2
                    && n.port_out.iter().all(|&o| o >= 0)
                    && (n.src_port < 0 || !self.plan.ports[n.src_port as usize].single)
            })
            .collect();
        struct Cand<'n> {
            a: &'n RNet,
            b: &'n RNet,
            score: f64,
        }
        let mut found: Vec<Cand> = Vec::new();
        for i in 0..cand.len() {
            for j in i + 1..cand.len() {
                let (a, b) = (cand[i], cand[j]);
                let (ca, cb) = (consumers(a), consumers(b));
                let shared = ca.iter().filter(|m| cb.contains(m)).count();
                if shared < 2 {
                    continue;
                }
                let union = ca.len() + cb.iter().filter(|m| !ca.contains(m)).count();
                let overlap = shared as f64 / union as f64;
                if overlap < self.tun.lane_share_overlap {
                    continue;
                }
                if !a.port_tiles.is_empty() && !b.port_tiles.is_empty() {
                    let pb = edge_ports(b);
                    if edge_ports(a).iter().all(|(t, o)| pb.get(t) != Some(o)) {
                        continue;
                    }
                }
                found.push(Cand { a, b, score: overlap + 0.01 * shared as f64 });
            }
        }
        found.sort_by(|x, y| compare_f64(y.score, x.score));
        let mut partner: HashMap<i32, &RNet> = HashMap::new();
        for c in &found {
            if !partner.contains_key(&c.a.idx) && !partner.contains_key(&c.b.idx) {
                partner.insert(c.a.idx, c.b);
                partner.insert(c.b.idx, c.a);
            }
        }
        if partner.is_empty() {
            return;
        }
        let mut merged: Vec<RNet> = Vec::new();
        let mut remap: HashMap<i32, i32> = HashMap::new();
        let mut slot_one: Vec<i32> = Vec::new();
        let mut log: Vec<String> = Vec::new();
        for n in &nets {
            if remap.contains_key(&n.idx) {
                continue;
            }
            let idx = merged.len() as i32;
            let Some(&p) = partner.get(&n.idx) else {
                remap.insert(n.idx, idx);
                merged.push(RNet { idx, ..n.clone() });
                continue;
            };
            let (first, second) = if n.port_tiles.is_empty() && !p.port_tiles.is_empty() { (p, n) } else { (n, p) };
            remap.insert(first.idx, idx);
            remap.insert(second.idx, idx);
            slot_one.extend(second.terms.iter().copied());
            let (mut tiles, mut outs) = (first.port_tiles.clone(), first.port_out.clone());
            if !second.port_tiles.is_empty() {
                let pb = edge_ports(second);
                let keep: Vec<usize> = (0..first.port_tiles.len()).filter(|&i| pb.get(&first.port_tiles[i]) == Some(&first.port_out[i])).collect();
                tiles = keep.iter().map(|&i| first.port_tiles[i]).collect();
                outs = keep.iter().map(|&i| first.port_out[i]).collect();
            }
            merged.push(RNet {
                idx,
                plan_id: first.plan_id,
                item: first.item.clone(),
                terms: first.terms.iter().chain(&second.terms).copied().collect(),
                port_tiles: tiles,
                port_out: outs,
                hpwl: first.hpwl.max(second.hpwl),
                source_rate: first.source_rate,
                out_tiles: Vec::new(),
                out_out: Vec::new(),
                out_rate: 0.0,
                item2: Some(second.item.clone()),
                source_rate2: second.source_rate,
                src_port: -1,
                out_port: -1,
            });
            log.push(format!("shared belt: {} (left lane) with {} (right lane)", first.item, second.item));
        }
        for t in slot_one {
            self.vts[t as usize].slot = 1;
        }
        self.log.extend(log);
        self.nets = Rc::new(merged);
        for v in &mut self.vts {
            v.net = remap[&v.net];
        }
        for a in self.allow_net.iter_mut() {
            if *a >= 0 {
                *a = remap.get(a).copied().unwrap_or(*a);
            }
        }
    }

    /// Adds one chain of `net`: its machine terminals (each with this chain's share of the rate and its own
    /// inserter count), and optionally the port it starts from and the port it ends at.
    fn add_chain(
        &mut self,
        net: &Net,
        members: &[EffectiveTerminal],
        source: Option<(&EffectiveTerminal, f64)>,
        output: Option<(&EffectiveTerminal, f64)>,
        rect_of: &HashMap<usize, Rect>,
    ) {
        let (source_rate, out_rate) = (source.map_or(0.0, |s| s.1), output.map_or(0.0, |o| o.1));
        let (source, output) = (source.map(|s| s.0), output.map(|o| o.0));
        let idx = self.nets.len() as i32;
        let first_vt = self.vts.len();
        let ids: Vec<i32> = (first_vt..first_vt + members.len()).map(|i| i as i32).collect();
        let new_vts: Vec<Vt> = members.iter().map(|t| Vt { base: t.base.clone(), rate: t.rate, k: t.k, net: idx, slot: 0 }).collect();
        let mut pts = self.center_tile(rect_of, &new_vts);
        self.vts.extend(new_vts);
        let src = source.map_or_else(Vec::new, |s| self.port_tiles_of(s.base.port.expect("a port terminal")));
        let out = output.map_or_else(Vec::new, |o| self.port_tiles_of(o.base.port.expect("a port terminal")));
        self.reserve(&src, idx);
        self.reserve(&out, idx);
        pts.extend(src.iter().take(1).map(|c| c.0));
        pts.extend(out.iter().take(1).map(|c| c.0));
        let hpwl = Self::hpwl_of(&pts);
        let entry = RNet {
            idx,
            plan_id: net.id as i32,
            item: net.item.clone(),
            terms: ids,
            port_tiles: src.iter().map(|c| self.dims.idx_tile(c.0) as i32).collect(),
            port_out: src.iter().map(|c| c.1.map_or(-1, |d| d.ordinal() as i32)).collect(),
            hpwl,
            source_rate,
            out_tiles: out.iter().map(|c| self.dims.idx_tile(c.0) as i32).collect(),
            out_out: out.iter().map(|c| c.1.map_or(-1, |d| d.ordinal() as i32)).collect(),
            out_rate,
            item2: None,
            source_rate2: 0.0,
            src_port: source.and_then(|s| s.base.port).map_or(-1, |p| p as i32),
            out_port: output.and_then(|o| o.base.port).map_or(-1, |p| p as i32),
        };
        Rc::make_mut(&mut self.nets).push(entry);
    }

    /// Chains of a net with several input or output ports, or with both an input port and producers (a block of
    /// a split factory imports part of an item and makes the rest). Every output port gets chains of its own,
    /// fed by the producers nearest to it; each input port feeds the consumers nearest to it (splitting one
    /// consumer between ports if it has to); producers feed the rest. Chains are split by inserter visits and flow
    /// as everywhere else.
    #[allow(clippy::too_many_arguments)] // the parameters of buildGeneralChains in Kotlin
    fn build_general_chains(
        &mut self,
        net: &Net,
        consumers: &[EffectiveTerminal],
        producers: &[EffectiveTerminal],
        sources: &[EffectiveTerminal],
        outputs: &[EffectiveTerminal],
        center: &dyn Fn(&Terminal) -> (f64, f64),
        rect_of: &HashMap<usize, Rect>,
        drop_rate: f64,
        pick_rate: f64,
        belt_cap: f64,
    ) {
        let plan = self.plan;
        let max_chain_visits = self.tun.max_chain_visits;
        let port_center = |pf: &Self, t: &EffectiveTerminal| -> (f64, f64) {
            let c: Vec<Tile> = pf.port_tiles_of(t.base.port.expect("a port terminal")).into_iter().map(|p| p.0).collect();
            if c.is_empty() {
                (f64::from(pf.w) / 2.0, f64::from(pf.h) / 2.0)
            } else {
                (average_f64(c.iter().map(|t| f64::from(t.x))), average_f64(c.iter().map(|t| f64::from(t.y))))
            }
        };
        let mut prod_left: Vec<f64> = producers.iter().map(|p| p.rate).collect();
        let mut src_left: Vec<f64> = sources.iter().map(|s| s.rate).collect();
        let from_producers = |prod_left: &mut Vec<f64>, amount: f64, at: (f64, f64)| -> (Vec<EffectiveTerminal>, f64) {
            let mut left = amount;
            let mut parts = Vec::new();
            while left > 1e-9 {
                let Some(pi) = first_min(producers.len(), |i| prod_left[i] > 1e-9, |i| dist(center(&producers[i].base), at)) else { break };
                let take = left.min(prod_left[pi]);
                prod_left[pi] -= take;
                left -= take;
                parts.push(part(&producers[pi], take, drop_rate));
            }
            (parts, left)
        };
        // Chains for `parts`, at most `room` by visits (one per tile of the port they start from).
        let chains_of = |pf: &Self, parts: &[EffectiveTerminal], room: i32| -> Vec<Vec<EffectiveTerminal>> {
            if parts.is_empty() {
                return Vec::new();
            }
            let visits: i32 = parts.iter().map(|t| t.k).sum();
            let g = ceil_div(visits, max_chain_visits).min(room).max((sum_f64(parts.iter().map(|t| t.rate)) / belt_cap - 1e-9).ceil() as i32).max(1).min(parts.len() as i32);
            if g == 1 { vec![parts.to_vec()] } else { pf.group_consumers(parts, g as usize, center) }
        };
        // 1. Output ports.
        for o in outputs {
            let port = o.base.port.expect("a port terminal");
            let pieces = if plan.ports[port].single { 1 } else { ((o.rate / belt_cap - 1e-9).ceil() as i32).max(1) };
            for _ in 0..pieces {
                let r = o.rate / f64::from(pieces);
                let at = port_center(self, o);
                let (parts, missing) = from_producers(&mut prod_left, r, at);
                let mut src: Option<&EffectiveTerminal> = None;
                let mut src_rate = 0.0;
                if missing > 1e-6 {
                    // The input port with the most left (maxByOrNull keeps the first of equals).
                    let mut best: Option<(usize, f64)> = None;
                    for (i, &v) in src_left.iter().enumerate().filter(|(_, v)| **v > 1e-9) {
                        if best.is_none_or(|(_, bv)| bv < v) {
                            best = Some((i, v));
                        }
                    }
                    if let Some((si, _)) = best {
                        src = Some(&sources[si]);
                        src_rate = missing.min(src_left[si]);
                        src_left[si] -= src_rate;
                    }
                }
                if missing - src_rate > 1e-6 {
                    self.log.push(format!("net {} ({}): output chain short of {}/s", net.id, net.item, fmt2(missing - src_rate)));
                }
                self.add_chain(net, &parts, src.map(|s| (s, src_rate)), Some((o, r)), rect_of);
            }
        }
        // 2. Consumers nearest each input port, fed by it.
        let mut need: Vec<f64> = consumers.iter().map(|c| c.rate).collect();
        let mut by_left: Vec<usize> = (0..sources.len()).collect();
        by_left.sort_by(|&a, &b| compare_f64(src_left[b], src_left[a]));
        for si in by_left {
            let s = &sources[si];
            let mut left = src_left[si];
            let at = port_center(self, s);
            let mut mine: Vec<EffectiveTerminal> = Vec::new();
            while left > 1e-9 {
                let Some(ci) = first_min(consumers.len(), |i| need[i] > 1e-9, |i| dist(center(&consumers[i].base), at)) else { break };
                let take = left.min(need[ci]);
                need[ci] -= take;
                left -= take;
                mine.push(part(&consumers[ci], take, pick_rate));
            }
            src_left[si] = left;
            // A link port is one belt: one chain (branches serve many consumers), so it pairs with its other end.
            let port = s.base.port.expect("a port terminal");
            let groups = if plan.ports[port].single {
                if mine.is_empty() { Vec::new() } else { vec![mine] }
            } else {
                chains_of(self, &mine, self.port_tiles_of(port).len() as i32)
            };
            for group in groups {
                let rate = sum_f64(group.iter().map(|t| t.rate));
                self.add_chain(net, &group, Some((s, rate)), None, rect_of);
            }
        }
        // 3. The rest from producers.
        let rest: Vec<EffectiveTerminal> = consumers.iter().enumerate().filter(|(i, _)| need[*i] > 1e-9).map(|(i, c)| part(c, need[i], pick_rate)).collect();
        for group in chains_of(self, &rest, i32::MAX) {
            let ps: Vec<(f64, f64)> = group.iter().map(|t| center(&t.base)).collect();
            let at = (average_f64(ps.iter().map(|p| p.0)), average_f64(ps.iter().map(|p| p.1)));
            let (parts, missing) = from_producers(&mut prod_left, sum_f64(group.iter().map(|t| t.rate)), at);
            if missing > 1e-6 {
                self.log.push(format!("net {} ({}): chain short of {}/s supply", net.id, net.item, fmt2(missing)));
            }
            let members: Vec<EffectiveTerminal> = group.into_iter().chain(parts).collect();
            self.add_chain(net, &members, None, None, rect_of);
        }
        for &r in prod_left.iter().filter(|&&r| r > 1e-6) {
            self.log.push(format!("net {} ({}): {}/s of a producer has no chain", net.id, net.item, fmt2(r)));
        }
        for &r in src_left.iter().filter(|&&r| r > 1e-6) {
            self.log.push(format!("net {} ({}): {}/s of an input port has no chain", net.id, net.item, fmt2(r)));
        }
    }

    /// Capacity-balanced k-means of consumers into `g` chains, seeded with far-apart consumers.
    pub(super) fn group_consumers(&self, cs: &[EffectiveTerminal], g: usize, center: &dyn Fn(&Terminal) -> (f64, f64)) -> Vec<Vec<EffectiveTerminal>> {
        let pts: Vec<(f64, f64)> = cs.iter().map(|c| center(&c.base)).collect();
        let mean = (average_f64(pts.iter().map(|p| p.0)), average_f64(pts.iter().map(|p| p.1)));
        let first_max = |n: usize, eligible: &dyn Fn(usize) -> bool, value: &dyn Fn(usize) -> f64| -> usize {
            let mut best: Option<(usize, f64)> = None;
            for i in (0..n).filter(|&i| eligible(i)) {
                let v = value(i);
                if best.is_none_or(|(_, bv)| bv < v) {
                    best = Some((i, v));
                }
            }
            best.expect("a candidate").0
        };
        let mut seeds: Vec<usize> = vec![first_max(pts.len(), &|_| true, &|i| dist(pts[i], mean))];
        while seeds.len() < g {
            let next = first_max(pts.len(), &|i| !seeds.contains(&i), &|i| seeds.iter().map(|&s| dist(pts[i], pts[s])).fold(f64::INFINITY, f64::min));
            seeds.push(next);
        }
        let mut centers: Vec<(f64, f64)> = seeds.iter().map(|&s| pts[s]).collect();
        let total = sum_f64(cs.iter().map(|c| c.rate));
        let capacity = cs.iter().map(|c| c.rate).fold(f64::NEG_INFINITY, f64::max).max(total / g as f64 * 1.2);
        let mut assign = vec![0usize; cs.len()];
        for _ in 0..8 {
            let mut load = vec![0f64; g];
            let mut next = vec![0usize; cs.len()];
            let mut by_rate: Vec<usize> = (0..cs.len()).collect();
            by_rate.sort_by(|&a, &b| compare_f64(cs[b].rate, cs[a].rate));
            for i in by_rate {
                let mut order: Vec<usize> = (0..g).collect();
                order.sort_by(|&a, &b| compare_f64(dist(pts[i], centers[a]), dist(pts[i], centers[b])));
                let pick = order.iter().copied().find(|&c| load[c] + cs[i].rate <= capacity + 1e-9).unwrap_or(order[0]);
                next[i] = pick;
                load[pick] += cs[i].rate;
            }
            assign = next;
            for (c, center) in centers.iter_mut().enumerate() {
                let mine: Vec<usize> = (0..cs.len()).filter(|&i| assign[i] == c).collect();
                if !mine.is_empty() {
                    *center = (average_f64(mine.iter().map(|&i| pts[i].0)), average_f64(mine.iter().map(|&i| pts[i].1)));
                }
            }
        }
        (0..g).map(|c| (0..cs.len()).filter(|&i| assign[i] == c).map(|i| cs[i].clone()).collect::<Vec<_>>()).filter(|v| !v.is_empty()).collect()
    }

    /// The access options of every chain terminal and sink, indexed by belt tile, and the heuristic's smallest step.
    pub(super) fn build_options(&mut self) {
        self.routed = vec![None; self.nets.len()];
        self.net_order = vec![None; self.nets.len()];
        self.term_net = self.vts.iter().map(|v| v.net).collect();
        self.term_k = self.vts.iter().map(|v| v.k).collect();
        self.need = vec![0; self.vts.len()];
        self.term_opts = vec![Vec::new(); self.vts.len()];

        // Access options for chain terminals, sink options for sinks.
        let plan = self.plan;
        let placement = self.p.placement;
        let mut opts: Vec<AccessOption> = Vec::new();
        let mut opt_vt: Vec<i32> = Vec::new();
        for (vi, v) in self.vts.iter().enumerate() {
            let pm = placement.machines.iter().find(|m| Some(m.id) == v.machine()).expect("a placed machine");
            let free = |t: Tile| self.dims.in_box(t.x, t.y) && self.allow_net[self.dims.idx_tile(t)] == -1;
            let list = access_options(&v.base, v.k, v.rate, &placement.rect(plan, pm), self.config, &free);
            let mut ids: Vec<i32> = Vec::new();
            for o in list {
                ids.push(opts.len() as i32);
                opts.push(o);
                opt_vt.push(vi as i32);
            }
            self.term_opts[vi] = ids;
        }
        for t in plan.terminals.iter().filter(|t| t.role == Role::Sink) {
            let Some(e) = self.eff.get(&t.id) else { continue };
            self.sink_k.insert(t.id, e.k);
            let pm = placement.machines.iter().find(|m| Some(m.id) == t.machine).expect("a placed machine");
            let free = |t: Tile| self.dims.in_box(t.x, t.y) && self.allow_net[self.dims.idx_tile(t)] == -1;
            self.sink_options.insert(t.id, access_options(t, e.k, e.rate, &placement.rect(plan, pm), self.config, &free));
        }
        self.opt_belt = opts.iter().map(|o| self.dims.idx_tile(o.belt_tile) as i32).collect();
        self.opt_ins = opts.iter().map(|o| self.dims.idx_tile(o.ins_tile) as i32).collect();
        self.opt_term = opt_vt;
        self.opt_long = opts.iter().map(|o| o.long).collect();
        self.options = opts;
        // CSR index belt tile -> options.
        let mut counts = vec![0i32; self.n + 1];
        for &b in &self.opt_belt {
            counts[b as usize + 1] += 1;
        }
        for i in 0..self.n {
            counts[i + 1] += counts[i];
        }
        let mut fill = counts.clone();
        let mut list = vec![0i32; self.options.len()];
        for (o, &b) in self.opt_belt.iter().enumerate() {
            list[fill[b as usize] as usize] = o as i32;
            fill[b as usize] += 1;
        }
        self.at_belt_start = counts;
        self.at_belt_list = list;

        let mj = self.max_j.max(self.max_jw);
        let ug_step = ((2.0 * self.tun.belt_base + self.tun.ug_penalty + self.tun.ug_penalty_per_tile * f64::from(mj)) / f64::from(mj + 1)) as f32;
        self.min_step = if self.ug_allowed { (self.tun.belt_base as f32).min(ug_step) } else { self.tun.belt_base as f32 };
    }

    // ---- emission ----
    fn choose_inserter(&self, share: f64, from_belt: bool, long: bool) -> String {
        if long {
            return "long-handed-inserter".to_owned();
        }
        let mut allowed: Vec<&String> = self.config.inserters.allowed.iter().filter(|n| *n != "long-handed-inserter").collect();
        allowed.sort_by_key(|n| gc::inserter(n).expect("inserters.allowed is validated").tier_cost);
        let rate = |n: &String| inserter_rate(n, self.config, from_belt);
        if let Some(n) = allowed.iter().find(|n| rate(n) >= share - 1e-9) {
            return (*n).clone();
        }
        // The fastest (maxByOrNull keeps the first of equals).
        let mut best: Option<(&String, f64)> = None;
        for n in &allowed {
            let r = rate(n);
            if best.is_none_or(|(_, br)| br < r) {
                best = Some((n, r));
            }
        }
        best.expect("an allowed inserter").0.clone()
    }

    /// An inserter taking from a machine with several item products is filtered to the one it carries.
    fn filter_for(&self, machine: Option<usize>, item: &str) -> Option<String> {
        machine.filter(|&m| self.plan.machines[m].item_products().count() > 1).map(|_| item.to_owned())
    }

    pub(super) fn entities(&self) -> Vec<PlacedEntity> {
        let mut out: Vec<PlacedEntity> = Vec::new();
        let placement = self.p.placement;
        for m in &placement.machines {
            let inst = &self.plan.machines[m.id];
            let oriented = orientable(inst);
            let mut e = PlacedEntity::new(&inst.machine, Kind::Machine, m.x, m.y, if oriented { m.rot } else { Dir::N });
            e.recipe = Some(inst.recipe.clone());
            e.mirror = oriented && m.mirror;
            e.w = inst.w;
            e.h = inst.h;
            out.push(e);
            if self.config.fluids.mode == "infinity-pipe" {
                // Test shortcut: every used connection gets its own infinity pipe (a drain on outputs).
                for c in placement.connections(self.plan, m).into_iter().filter(|c| c.fluid.is_some()) {
                    let mut pipe = PlacedEntity::new(gc::INFINITY_PIPE, Kind::InfinityPipe, c.tile.x, c.tile.y, Dir::N);
                    pipe.fluid = c.fluid.clone();
                    pipe.drain = c.flow == gc::Flow::Output;
                    out.push(pipe);
                }
            }
        }
        for link in &self.di {
            let share = link.flow / link.tiles.len() as f64;
            let name = self.choose_inserter(share, false, link.long);
            for t in &link.tiles {
                let mut e = PlacedEntity::new(&name, Kind::Inserter, t.x, t.y, link.ins_dir);
                e.item = self.filter_for(Some(link.producer), &link.item);
                out.push(e);
            }
        }
        for n in self.nets.iter() {
            let Some(r) = &self.routed[n.idx as usize] else { continue };
            let add = |out: &mut Vec<PlacedEntity>, e: i32, kind: Kind, name: &str| {
                let t = self.dims.tile((e >> 2) as usize);
                let mut ent = PlacedEntity::new(name, kind, t.x, t.y, Dir::ALL[(e & 3) as usize]);
                ent.net = Some(n.plan_id);
                out.push(ent);
            };
            let under_splitter: HashSet<i32> = r.splitters.iter().map(|sp| sp[0]).collect();
            for &e in &r.belts {
                if !under_splitter.contains(&(e >> 2)) {
                    add(&mut out, e, Kind::Belt, self.tier.belt);
                }
            }
            for sp in &r.splitters {
                let (a, b) = (self.dims.tile(sp[0] as usize), self.dims.tile(sp[1] as usize));
                let mut e = PlacedEntity::new(self.tier.splitter, Kind::Splitter, a.x.min(b.x), a.y.min(b.y), Dir::ALL[sp[2] as usize]);
                e.net = Some(n.plan_id);
                out.push(e);
            }
            let ug_name = |tier: i32| if tier == 1 { self.weave.expect("a weaving tier").underground } else { self.tier.underground };
            for (k, &e) in r.ug_in.iter().enumerate() {
                add(&mut out, e, Kind::UgIn, ug_name(r.ug_tier[k]));
            }
            for (k, &e) in r.ug_out.iter().enumerate() {
                add(&mut out, e, Kind::UgOut, ug_name(r.ug_tier[k]));
            }
            for &o in &r.claims {
                let opt = &self.options[o as usize];
                let v = &self.vts[self.opt_term[o as usize] as usize];
                let consumer = v.role() == Role::Consumer;
                let name = self.choose_inserter(v.rate / f64::from(v.k), consumer, opt.long);
                let mut e = PlacedEntity::new(&name, Kind::Inserter, opt.ins_tile.x, opt.ins_tile.y, opt.ins_dir);
                e.item = if consumer { None } else { self.filter_for(v.machine(), &v.base.item) };
                e.net = Some(n.plan_id);
                out.push(e);
            }
        }
        if let Some(pr) = &self.pipes {
            out.extend(pr.entities());
        }
        for (o, chest) in &self.sink_claims {
            let e = &self.eff[&o.terminal];
            let mut ins = PlacedEntity::new(&self.choose_inserter(e.rate / f64::from(e.k), false, false), Kind::Inserter, o.ins_tile.x, o.ins_tile.y, o.ins_dir);
            ins.item = self.filter_for(e.base.machine, &e.base.item);
            out.push(ins);
            out.push(PlacedEntity::new(&self.config.sinks.chest, Kind::Chest, chest.x, chest.y, Dir::N));
        }
        out
    }
}
