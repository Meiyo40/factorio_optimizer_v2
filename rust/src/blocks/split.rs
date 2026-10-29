use crate::config::run_configuration::{BlocksConfig, ConfigSource};
use crate::config::tuning::Tuning;
use crate::data::game_constants as gc;
use crate::plan::{Net, Port, ProductionPlan, Role, Terminal};
use crate::util::java_hash::JMap;
use crate::util::{average_f64, compare_f64, sum_f64};
use indexmap::IndexMap;
use std::collections::HashMap;

/// `rate` of `item` going from block `from` to block `to`; -1 is the factory's outer edge (raw inputs, products).
/// `routed`: an edge link whose block does not touch an edge the job allows for it, so it runs through the
/// corridors to the factory's edge instead of leaving the block directly.
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub id: usize,
    pub item: String,
    pub rate: f64,
    pub from: i32,
    pub to: i32,
    pub fluid: bool,
    pub routed: bool,
}

impl Link {
    /// Between two blocks: one belt, routed through the corridors. Edge links leave or enter a block directly.
    pub fn inner(&self) -> bool {
        self.from >= 0 && self.to >= 0
    }

    /// One belt (or pipe) through the corridors: between two blocks, or between a block and the factory's edge.
    pub fn through_corridor(&self) -> bool {
        self.inner() || self.routed
    }
}

/// A block's place in the factory grid; blocks snake through the rows so neighbours in order sit side by side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub row: i32,
    pub col: i32,
}

/// The plan split into blocks (machine ids of the full plan, upstream first), their slots and the links.
#[derive(Debug, Clone, PartialEq)]
pub struct Split {
    pub blocks: Vec<Vec<usize>>,
    pub slots: Vec<Slot>,
    pub rows: i32,
    pub cols: i32,
    pub links: Vec<Link>,
    pub warnings: Vec<String>,
}

fn find(parent: &mut [usize], x: usize) -> usize {
    let mut a = x;
    while parent[a] != a {
        parent[a] = parent[parent[a]];
        a = parent[a];
    }
    a
}

fn key(a: i32, b: i32) -> i64 {
    if a < b { i64::from(a) * 1_000_000 + i64::from(b) } else { i64::from(b) * 1_000_000 + i64::from(a) }
}

fn low(k: i64) -> i32 {
    (k / 1_000_000) as i32
}

fn high(k: i64) -> i32 {
    (k % 1_000_000) as i32
}

/// Joins block `b` into block `a`: its machines, and its flows to the others.
fn merge_clusters(members: &mut JMap<i32, Vec<usize>>, cluster_of: &mut [i32], weight: &mut JMap<i64, f64>, a: i32, b: i32) {
    let moved_machines = members.remove(&b).expect("a cluster that exists");
    let target = members.get_mut(&a).expect("a cluster that exists");
    target.extend(moved_machines);
    for &m in target.iter() {
        cluster_of[m] = a;
    }
    let moved: Vec<i64> = weight.keys_java().copied().filter(|&k| low(k) == b || high(k) == b).collect();
    for k in moved {
        let w = weight.remove(&k).expect("a key just listed");
        let other = if low(k) == b { high(k) } else { low(k) };
        if other != a {
            weight.merge(key(a, other), w, |x, y| x + y);
        }
    }
}

/// Splits `plan` into blocks of at most `BlocksConfig.max_machines` machines. Machines joined by a pipe stay
/// together (fluids never cross a block border); then units merge greedily along the heaviest item flow between
/// them (items in `split_at` count `split_at_weight` as much) while the merged block stays within the limit, and small
/// leftovers merge with each other. Blocks are ordered upstream first and snaked through one or two rows, so
/// every block touches the factory's outer edge, where raw inputs enter and products leave. Each item's surplus
/// and deficit per block become links, nearest blocks first; links between blocks are cut into belts of at most
/// `link_belt_share` of the belt tier.
pub fn split_plan(plan: &ProductionPlan, cfg: &BlocksConfig, belt_throughput: f64) -> Split {
    let n = plan.machines.len();
    let max_machines = cfg.max_machines as usize;
    let mut warnings = Vec::new();
    // 1. Units: machines joined through fluid nets.
    let mut parent: Vec<usize> = (0..n).collect();
    for net in plan.nets.iter().filter(|net| net.fluid) {
        let ms: Vec<usize> = plan.net_terminals(net).iter().filter_map(|t| t.machine).collect();
        for &m in ms.iter().skip(1) {
            let a = find(&mut parent, ms[0]);
            let b = find(&mut parent, m);
            if a != b {
                parent[a] = b;
            }
        }
    }
    let mut clusters: JMap<i32, Vec<usize>> = JMap::new();
    for m in 0..n {
        let root = find(&mut parent, m) as i32;
        clusters.get_or_put_with(root, Vec::new).push(m);
    }
    let mut cluster_of = vec![0i32; n];
    let mut members: JMap<i32, Vec<usize>> = JMap::new();
    for (i, ms) in clusters.values_java().enumerate() {
        members.insert(i as i32, ms.clone());
        for &m in ms {
            cluster_of[m] = i as i32;
        }
    }
    for ms in members.values_java().filter(|ms| ms.len() > max_machines) {
        warnings.push(format!("{} machines are joined by pipes and stay one block, above the {}-machine limit", ms.len(), cfg.max_machines));
    }

    // 2. Flow between clusters.
    let mut weight: JMap<i64, f64> = JMap::new();
    for net in plan.nets.iter().filter(|net| !net.fluid) {
        let ts = plan.net_terminals(net);
        let prod: Vec<&Terminal> = ts.iter().copied().filter(|t| t.role == Role::Producer && t.machine.is_some()).collect();
        let cons: Vec<&Terminal> = ts.iter().copied().filter(|t| t.role == Role::Consumer && t.machine.is_some()).collect();
        let total = sum_f64(cons.iter().map(|t| t.rate));
        if total < 1e-12 {
            continue;
        }
        let f = if cfg.split_at.contains(&net.item) { cfg.split_at_weight } else { 1.0 };
        for p in &prod {
            for c in &cons {
                let a = cluster_of[p.machine.expect("filtered")];
                let b = cluster_of[c.machine.expect("filtered")];
                if a != b {
                    weight.merge(key(a, b), f * p.rate * c.rate / total, |x, y| x + y);
                }
            }
        }
    }

    // 3. Greedy merging along the heaviest flow, within the size limit; then small leftovers together.
    let size = |members: &JMap<i32, Vec<usize>>, c: i32| members.get(&c).expect("a cluster that exists").len();
    loop {
        // maxByOrNull: the first of the largest, in the order Java's table iterates.
        let mut best: Option<(i64, f64)> = None;
        for (&k, &v) in weight.iter_java() {
            if size(&members, low(k)) + size(&members, high(k)) <= max_machines && best.is_none_or(|(_, bv)| v > bv) {
                best = Some((k, v));
            }
        }
        let Some((k, _)) = best else { break };
        merge_clusters(&mut members, &mut cluster_of, &mut weight, low(k), high(k));
    }
    loop {
        let mut small: Vec<i32> = members.keys_java().copied().collect();
        small.sort_by_key(|&c| size(&members, c)); // stable, like sortedBy
        if small.len() < 2 || size(&members, small[0]) + size(&members, small[1]) > max_machines {
            break;
        }
        merge_clusters(&mut members, &mut cluster_of, &mut weight, small[0], small[1]);
    }

    // 4. Order upstream first: production depth of each machine (longest chain from inputs, capped for loops).
    let mut depth = vec![0.0f64; n];
    for _ in 0..40 {
        let mut item_depth: HashMap<&str, f64> = HashMap::new();
        for m in &plan.machines {
            for p in m.products.keys() {
                item_depth.entry(p.as_str()).and_modify(|d| *d = d.max(depth[m.id])).or_insert(depth[m.id]);
            }
        }
        for m in &plan.machines {
            let deepest = m.demand.keys().chain(m.fluid_demand.keys()).map(|i| item_depth.get(i.as_str()).copied().unwrap_or(0.0)).reduce(f64::max);
            depth[m.id] = 1.0 + deepest.unwrap_or(0.0).min(60.0);
        }
    }
    let mut blocks: Vec<Vec<usize>> = members
        .values_java()
        .map(|ms| {
            let mut sorted = ms.clone();
            sorted.sort_unstable();
            sorted
        })
        .collect();
    blocks.sort_by(|a, b| compare_f64(average_f64(a.iter().map(|&m| depth[m])), average_f64(b.iter().map(|&m| depth[m]))));
    let k = blocks.len() as i32;
    let rows = cfg.rows.map_or(if k <= 3 { 1 } else { 2 }, |r| r.min(k));
    let cols = (k + rows - 1) / rows;
    let slots: Vec<Slot> = (0..k)
        .map(|i| {
            let r = i / cols;
            Slot { row: r, col: if r == 0 { i % cols } else { cols - 1 - i % cols } }
        })
        .collect();
    let mut block_of = vec![0usize; n];
    for (b, ms) in blocks.iter().enumerate() {
        for &m in ms {
            block_of[m] = b;
        }
    }

    // 5. Links: per item, block surpluses to block deficits (nearest first), the edge supplying raw inputs
    //    and taking products. An edge link goes straight out of its block when the block touches an edge the job
    //    allows for the item (any edge, or its whole side); otherwise it is routed through the corridors.
    let routed = |item: &str, output: bool, b: usize| -> bool {
        let Some(port) = plan.ports.iter().find(|p| p.item == item && p.output == output) else { return false };
        let side = outer_side(rows, slots[b]);
        !port.specs.iter().any(|s| s.edge.as_deref() == Some("any") || (s.edge.as_deref() == Some(side) && s.from.is_none() && s.to.is_none()))
    };
    let dist = |a: i32, b: i32| -> i32 {
        if a < 0 || b < 0 {
            0
        } else {
            let (sa, sb) = (slots[a as usize], slots[b as usize]);
            (sa.row - sb.row).abs() * 2 + (sa.col - sb.col).abs()
        }
    };
    let mut links: Vec<Link> = Vec::new();
    for net in &plan.nets {
        let ts = plan.net_terminals(net);
        let mut balance = vec![0.0f64; k as usize];
        for t in &ts {
            let Some(m) = t.machine else { continue };
            match t.role {
                Role::Producer => balance[block_of[m]] += t.rate,
                Role::Consumer => balance[block_of[m]] -= t.rate,
                _ => {}
            }
        }
        let mut give: Vec<(i32, f64)> = Vec::new();
        let mut take: Vec<(i32, f64)> = Vec::new();
        for (b, &bal) in balance.iter().enumerate() {
            if bal > 1e-9 {
                give.push((b as i32, bal));
            } else if bal < -1e-9 {
                take.push((b as i32, -bal));
            }
        }
        let sourced = sum_f64(ts.iter().filter(|t| t.role == Role::Source).map(|t| t.rate));
        if sourced > 1e-9 {
            give.push((-1, sourced));
        }
        let output = sum_f64(ts.iter().filter(|t| t.role == Role::Output).map(|t| t.rate));
        if output > 1e-9 {
            take.push((-1, output));
        }
        let mut left: JMap<(i32, i32), f64> = JMap::new();
        let mut pairs: Vec<(i32, i32)> = give.iter().flat_map(|g| take.iter().map(move |t| (g.0, t.0))).filter(|(a, b)| a != b).collect();
        pairs.sort_by_key(|&(a, b)| dist(a, b));
        let mut give_left: HashMap<i32, f64> = give.iter().copied().collect();
        let mut take_left: HashMap<i32, f64> = take.iter().copied().collect();
        for (a, b) in pairs {
            let r = give_left[&a].min(take_left[&b]);
            if r < 1e-9 {
                continue;
            }
            give_left.insert(a, give_left[&a] - r);
            take_left.insert(b, take_left[&b] - r);
            left.merge((a, b), r, |x, y| x + y);
        }
        for (&(a, b), &r) in left.iter_java() {
            if net.fluid && a >= 0 && b >= 0 {
                warnings.push(format!("{} would have to be piped between blocks {} and {}; fluids are not stitched yet", net.item, a + 1, b + 1));
            }
            let via_corridor = if a < 0 {
                routed(&net.item, false, b as usize)
            } else if b < 0 {
                routed(&net.item, true, a as usize)
            } else {
                false
            };
            let pieces = if ((a >= 0 && b >= 0) || via_corridor) && !net.fluid {
                ((r / (cfg.link_belt_share * belt_throughput) - 1e-9).ceil() as i32).max(1)
            } else {
                1
            };
            for _ in 0..pieces {
                links.push(Link { id: links.len(), item: net.item.clone(), rate: r / f64::from(pieces), from: a, to: b, fluid: net.fluid, routed: via_corridor });
            }
        }
    }
    Split { blocks, slots, rows, cols, links, warnings }
}

/// `split` with blocks `a` and `b` (side by side in one row) made one block in `a`'s place: the links between
/// them disappear (their items now flow inside the block), the blocks after them in the row move up one slot,
/// and link ids are renumbered to stay list indices.
pub fn merge_blocks(split: &Split, a: usize, b: usize) -> Split {
    assert!(
        split.slots[a].row == split.slots[b].row && (split.slots[a].col - split.slots[b].col).abs() == 1,
        "blocks {a} and {b} are not neighbours"
    );
    let (keep, gone) = if split.slots[a].col < split.slots[b].col { (a, b) } else { (b, a) };
    let row = split.slots[keep].row;
    let gone_col = split.slots[gone].col;
    // New block indices: everything but `gone`, in order.
    let mut index = vec![-1i32; split.blocks.len()];
    let mut next = 0;
    for (i, slot) in index.iter_mut().enumerate() {
        if i != gone {
            *slot = next;
            next += 1;
        }
    }
    index[gone] = index[keep];
    let blocks: Vec<Vec<usize>> = (0..split.blocks.len())
        .filter(|&i| i != gone)
        .map(|i| {
            if i == keep {
                let mut both = split.blocks[keep].clone();
                both.extend(&split.blocks[gone]);
                both.sort_unstable();
                both
            } else {
                split.blocks[i].clone()
            }
        })
        .collect();
    let slots: Vec<Slot> = (0..split.blocks.len())
        .filter(|&i| i != gone)
        .map(|i| {
            let s = split.slots[i];
            if s.row == row && s.col > gone_col { Slot { row: s.row, col: s.col - 1 } } else { s }
        })
        .collect();
    let links: Vec<Link> = split
        .links
        .iter()
        .filter(|l| !(l.from >= 0 && l.to >= 0 && index[l.from as usize] == index[l.to as usize]))
        .enumerate()
        .map(|(i, l)| Link {
            id: i,
            from: if l.from >= 0 { index[l.from as usize] } else { -1 },
            to: if l.to >= 0 { index[l.to as usize] } else { -1 },
            ..l.clone()
        })
        .collect();
    let cols = slots.iter().map(|s| s.col).max().expect("a block is left") + 1;
    Split { blocks, slots, rows: split.rows, cols, links, warnings: split.warnings.clone() }
}

/// The factory edge a block in `slot` touches: the top for the first row, the bottom for the second.
pub fn outer_side(rows: i32, slot: Slot) -> &'static str {
    if rows == 1 || slot.row == 0 { "north" } else { "south" }
}

/// The side of a block in `slot` that faces the corridor along the rows (below a single row, between two rows).
pub fn corridor_side(rows: i32, slot: Slot) -> &'static str {
    if rows == 1 || slot.row == 0 { "south" } else { "north" }
}

/// The edge of block `b` that a link's port goes on: towards its partner block, the factory's outer edge, or the
/// corridor for links routed through it.
pub fn edge_for(split: &Split, link: &Link, b: usize) -> &'static str {
    let me = split.slots[b];
    let other = if link.from == b as i32 { link.to } else { link.from };
    if other < 0 {
        return if link.routed { corridor_side(split.rows, me) } else { outer_side(split.rows, me) };
    }
    let o = split.slots[other as usize];
    if o.row == me.row && (o.col - me.col).abs() == 1 {
        return if o.col > me.col { "east" } else { "west" };
    }
    // Not side by side: through the corridor along the rows.
    corridor_side(split.rows, me)
}

/// One block as a plan of its own: local machine ids, a port per link, and which link each port serves.
#[derive(Debug, Clone)]
pub struct BlockPlan {
    pub index: usize,
    pub plan: ProductionPlan,
    pub global_ids: Vec<usize>,
    pub link_of_port: HashMap<usize, usize>,
}

/// The plan of block `b`: its machines, their terminals, and an input or output port per link it is part of.
pub fn block_plan(full: &ProductionPlan, split: &Split, b: usize, tuning: &Tuning) -> BlockPlan {
    let ids = &split.blocks[b];
    let local: HashMap<usize, usize> = ids.iter().enumerate().map(|(i, &g)| (g, i)).collect();
    let machines = ids.iter().map(|&g| crate::plan::MachineInstance { id: local[&g], ..full.machines[g].clone() }).collect();
    let mut terminals: Vec<Terminal> = Vec::new();
    for t in full.terminals.iter().filter(|t| t.machine.is_some_and(|m| local.contains_key(&m))) {
        terminals.push(Terminal { id: terminals.len(), machine: t.machine.map(|m| local[&m]), port: None, ..t.clone() });
    }
    let mut ports: Vec<Port> = Vec::new();
    let mut link_of_port = HashMap::new();
    let me = b as i32;
    for l in split.links.iter().filter(|l| l.from == me || l.to == me) {
        let out = l.from == me;
        let spec = ConfigSource { name: l.item.clone(), edge: Some(edge_for(split, l, b).to_owned()), ..ConfigSource::default() };
        let port = Port { id: ports.len(), item: l.item.clone(), specs: vec![spec], fluid: l.fluid, output: out, single: l.through_corridor() };
        link_of_port.insert(port.id, l.id);
        terminals.push(Terminal {
            id: terminals.len(),
            role: if out { Role::Output } else { Role::Source },
            item: l.item.clone(),
            rate: l.rate,
            k: 1,
            machine: None,
            port: Some(port.id),
            fluid: l.fluid,
        });
        ports.push(port);
    }
    let tier = gc::belt_tier(&full.belt_tier).expect("the plan's belt tier is validated");
    let mut by_item: IndexMap<&str, Vec<usize>> = IndexMap::new();
    for t in terminals.iter().filter(|t| t.role != Role::Sink) {
        by_item.entry(t.item.as_str()).or_default().push(t.id);
    }
    let mut nets: Vec<Net> = Vec::new();
    for (item, ts) in by_item {
        let takers: Vec<&Terminal> = ts.iter().map(|&t| &terminals[t]).filter(|t| t.role == Role::Consumer || t.role == Role::Output).collect();
        if ts.len() < 2 || takers.is_empty() {
            continue;
        }
        let fluid = terminals[ts[0]].fluid;
        let flow = sum_f64(takers.iter().map(|t| t.rate));
        let belts = if fluid { 1 } else { ((flow / (tuning.net.belt_utilisation * tier.throughput) - 1e-9).ceil() as i32).max(1) };
        nets.push(Net { id: nets.len(), item: item.to_owned(), flow, terminals: ts, belt_count: belts, lane_pair: None, fluid });
    }
    let sum_by_item = |links: Vec<&Link>| -> IndexMap<String, f64> {
        let mut grouped: IndexMap<String, Vec<f64>> = IndexMap::new();
        for l in links {
            grouped.entry(l.item.clone()).or_default().push(l.rate);
        }
        grouped.into_iter().map(|(item, rates)| (item, sum_f64(rates))).collect()
    };
    let inputs = sum_by_item(split.links.iter().filter(|l| l.to == me && l.from < 0).collect());
    let outputs = sum_by_item(split.links.iter().filter(|l| l.from == me && l.to < 0).collect());
    let fluid_inputs = inputs.iter().filter(|(k, _)| split.links.iter().any(|l| &l.item == *k && l.fluid)).map(|(k, v)| (k.clone(), *v)).collect();
    let plan = ProductionPlan {
        machines,
        ports,
        terminals,
        nets,
        raw_rates: inputs,
        fluid_rates: fluid_inputs,
        recipe_rates: IndexMap::new(),
        belt_tier: full.belt_tier.clone(),
        warnings: Vec::new(),
        outputs,
    };
    BlockPlan { index: b, plan, global_ids: ids.clone(), link_of_port }
}
