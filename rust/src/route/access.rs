use crate::config::run_configuration::RunConfiguration;
use crate::geom::{Dir, Rect, Tile};
use crate::place::Placement;
use crate::plan::{ProductionPlan, Role, Terminal, best_inserter_rate, inserter_count, inserter_rate};
use crate::util::sum_f64;
use std::collections::{HashMap, HashSet};

const LONG_HANDED: &str = "long-handed-inserter";

/// How one inserter connects a terminal to a belt (or a chest, for sinks). Tiles are box coordinates.
/// `ins_dir` is the inserter's blueprint direction (its pickup side).
#[derive(Debug, Clone, PartialEq)]
pub struct AccessOption {
    pub terminal: usize,
    pub belt_tile: Tile,
    pub ins_tile: Tile,
    pub ins_dir: Dir,
    pub long: bool,
    pub trick: &'static str,
    pub side: Dir,
}

/// A direct-insertion link: inserters in the gap between two machines (I5, or I6 with long-handed).
#[derive(Debug, Clone, PartialEq)]
pub struct DiLink {
    pub producer: usize,
    pub consumer: usize,
    pub item: String,
    pub flow: f64,
    pub tiles: Vec<Tile>,
    pub long: bool,
    /// Pickup side, pointing at the producer.
    pub ins_dir: Dir,
}

/// Rates and inserter counts per terminal after direct insertion took its share.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectiveTerminal {
    pub base: Terminal,
    pub rate: f64,
    pub k: i32,
}

/// Access options of a machine terminal for a placement (tricks I1, I2, I3; I7 for sinks).
/// `free` says whether a tile may hold an inserter or belt (in the box, not a machine or obstacle).
pub fn access_options(t: &Terminal, k: i32, rate: f64, rect: &Rect, config: &RunConfiguration, free: &dyn Fn(Tile) -> bool) -> Vec<AccessOption> {
    let mut out = Vec::new();
    let consumer = t.role == Role::Consumer;
    let sink = t.role == Role::Sink;
    let long_allowed = !sink
        && config.trick("I3")
        && config.inserters.allowed.iter().any(|n| n == LONG_HANDED)
        && inserter_rate(LONG_HANDED, config, consumer) * f64::from(k) >= rate - 1e-9;
    let normal_allowed = config.inserters.allowed.iter().any(|n| n != LONG_HANDED);
    for side in Dir::ALL {
        let d1 = rect.side_tiles(side, 1);
        let d2 = rect.side_tiles(side, 2);
        let d3 = rect.side_tiles(side, 3);
        // A consumer's inserter picks from the belt (outward); a producer's or sink's picks from the machine.
        let ins_dir = if consumer { side } else { side.opposite() };
        for i in 0..d1.len() {
            if !free(d1[i]) {
                continue;
            }
            if normal_allowed && free(d2[i]) {
                let trick = if sink { "I7" } else if consumer { "I1" } else { "I2" };
                out.push(AccessOption { terminal: t.id, belt_tile: d2[i], ins_tile: d1[i], ins_dir, long: false, trick, side });
            }
            if long_allowed && free(d3[i]) {
                out.push(AccessOption { terminal: t.id, belt_tile: d3[i], ins_tile: d1[i], ins_dir, long: true, trick: "I3", side });
            }
        }
    }
    out
}

/// Rate of the fastest allowed inserter that is not long-handed, and of the long-handed one (0 when absent).
fn di_rates(config: &RunConfiguration) -> (f64, f64) {
    let normal = config.inserters.allowed.iter().filter(|n| *n != LONG_HANDED).map(|n| inserter_rate(n, config, false)).reduce(f64::max).unwrap_or(0.0);
    let long = if config.inserters.allowed.iter().any(|n| n == LONG_HANDED) { inserter_rate(LONG_HANDED, config, false) } else { 0.0 };
    (normal, long)
}

struct Cand<'a> {
    p: &'a Terminal,
    c: &'a Terminal,
    tiles: Vec<Tile>,
    long: bool,
    dir: Dir,
}

/// Direct insertion (I5, I6), decided per placement: for each producer-consumer pair of the same item whose
/// footprints leave a 1-tile gap (or a 3-tile gap for a long-handed) with at least one tile of overlap,
/// assign flow greedily, largest first, up to the consumer's demand and the producer's spare output.
pub fn assign_direct_insertion(plan: &ProductionPlan, placement: &Placement, config: &RunConfiguration, free: &dyn Fn(Tile) -> bool) -> Vec<DiLink> {
    if !config.trick("I5") && !config.trick("I6") {
        return Vec::new();
    }
    let rect_of: HashMap<usize, Rect> = placement.machines.iter().map(|m| (m.id, placement.rect(plan, m))).collect();
    let mut producer_rate: HashMap<usize, f64> = HashMap::new(); // terminal id -> spare
    let mut consumer_need: HashMap<usize, f64> = HashMap::new();
    for t in plan.terminals.iter().filter(|t| !t.fluid) {
        match t.role {
            Role::Producer => {
                producer_rate.insert(t.id, t.rate);
            }
            Role::Consumer => {
                consumer_need.insert(t.id, t.rate);
            }
            _ => {}
        }
    }
    let (normal_rate, long_rate) = di_rates(config);
    let rate_of = |long: bool| if long { long_rate } else { normal_rate };

    let mut cands: Vec<Cand> = Vec::new();
    let producers = plan.terminals.iter().filter(|t| t.role == Role::Producer && !t.fluid);
    let consumers: Vec<&Terminal> = plan.terminals.iter().filter(|t| t.role == Role::Consumer && t.machine.is_some() && !t.fluid).collect();
    for p in producers {
        for &c in &consumers {
            if p.item != c.item || p.machine == c.machine {
                continue;
            }
            let a = rect_of[&p.machine.expect("a producer sits on a machine")];
            let b = rect_of[&c.machine.expect("filtered above")];
            for (gap, long) in [(1, false), (3, true)] {
                if long && (!config.trick("I6") || long_rate == 0.0) {
                    continue;
                }
                if !long && (!config.trick("I5") || normal_rate == 0.0) {
                    continue;
                }
                if let Some((tiles, dir)) = gap_tiles(&a, &b, gap) {
                    let usable: Vec<Tile> = tiles.into_iter().filter(|&t| free(t)).collect();
                    if !usable.is_empty() {
                        cands.push(Cand { p, c, tiles: usable, long, dir });
                    }
                }
            }
        }
    }
    let mut used: HashSet<Tile> = HashSet::new();
    let mut links: Vec<DiLink> = Vec::new();
    loop {
        // The first candidate with the largest achievable flow (maxByOrNull keeps the first of equals).
        let mut best: Option<(usize, f64)> = None;
        for (i, c) in cands.iter().enumerate() {
            let free_tiles = c.tiles.iter().filter(|t| !used.contains(t)).count();
            let v = producer_rate[&c.p.id].min(consumer_need[&c.c.id]).min(free_tiles as f64 * rate_of(c.long));
            if best.is_none_or(|(_, bv)| bv < v) {
                best = Some((i, v));
            }
        }
        let Some((bi, _)) = best else { break };
        let best = cands.remove(bi);
        let tiles: Vec<Tile> = best.tiles.iter().copied().filter(|t| !used.contains(t)).collect();
        let rate = rate_of(best.long);
        let f = producer_rate[&best.p.id].min(consumer_need[&best.c.id]).min(tiles.len() as f64 * rate);
        if f < 1e-9 {
            continue;
        }
        let k = inserter_count(f, rate);
        // Prefer tiles near the middle of the overlap.
        let mid = sum_f64(tiles.iter().map(|t| f64::from(t.x + t.y))) / tiles.len() as f64;
        let mut by_middle = tiles.clone();
        by_middle.sort_by(|a, b| (f64::from(a.x + a.y) - mid).abs().total_cmp(&(f64::from(b.x + b.y) - mid).abs()));
        let chosen: Vec<Tile> = by_middle.into_iter().take(k as usize).collect();
        used.extend(chosen.iter().copied());
        *producer_rate.get_mut(&best.p.id).expect("a producer terminal") -= f;
        *consumer_need.get_mut(&best.c.id).expect("a consumer terminal") -= f;
        links.push(DiLink {
            producer: best.p.machine.expect("a producer sits on a machine"),
            consumer: best.c.machine.expect("filtered above"),
            item: best.p.item.clone(),
            flow: f,
            tiles: chosen,
            long: best.long,
            ins_dir: best.dir,
        });
    }
    settle_di_flows(plan, links, config)
}

/// What each link really delivers in game, as the validator models it: a consumer draws an item evenly
/// through all its direct-insertion inserters (each capped by its rate), and a producer that cannot keep up
/// is scaled down to its spare output. The greedy choice above only decides where inserters go.
pub fn settle_di_flows(plan: &ProductionPlan, links: Vec<DiLink>, config: &RunConfiguration) -> Vec<DiLink> {
    let (normal_rate, long_rate) = di_rates(config);
    let mut n_in: HashMap<(usize, &str), usize> = HashMap::new();
    for l in &links {
        *n_in.entry((l.consumer, l.item.as_str())).or_insert(0) += l.tiles.len();
    }
    let want: Vec<f64> = links
        .iter()
        .map(|l| {
            let need = plan.machines[l.consumer].demand.get(&l.item).copied().unwrap_or(0.0);
            let per = (if l.long { long_rate } else { normal_rate }).min(need / n_in[&(l.consumer, l.item.as_str())] as f64);
            per * l.tiles.len() as f64
        })
        .collect();
    // Per machine and item: a machine with several products has a spare output of each.
    let mut spare: HashMap<(usize, &str), f64> = HashMap::new();
    for t in plan.terminals.iter().filter(|t| t.role == Role::Producer && !t.fluid) {
        *spare.entry((t.machine.expect("a producer sits on a machine"), t.item.as_str())).or_insert(0.0) += t.rate;
    }
    let mut total: HashMap<(usize, &str), f64> = HashMap::new();
    for (i, l) in links.iter().enumerate() {
        *total.entry((l.producer, l.item.as_str())).or_insert(0.0) += want[i];
    }
    let flows: Vec<f64> = links
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let s = spare.get(&(l.producer, l.item.as_str())).copied().unwrap_or(0.0);
            let tot = total[&(l.producer, l.item.as_str())];
            let scale = if tot > s && tot > 0.0 { s / tot } else { 1.0 };
            want[i] * scale
        })
        .collect();
    links.into_iter().zip(flows).map(|(l, flow)| DiLink { flow, ..l }).collect()
}

/// Gap tiles where an inserter reaches from `a` into `b` across `gap` free tiles, with the pickup direction.
fn gap_tiles(a: &Rect, b: &Rect, gap: i32) -> Option<(Vec<Tile>, Dir)> {
    let mid = gap / 2 + 1; // tile index into the gap holding the inserter (1 for gap 1, 2 for gap 3)
    let (y_lo, y_hi) = (a.y.max(b.y), a.y2().min(b.y2()));
    let (x_lo, x_hi) = (a.x.max(b.x), a.x2().min(b.x2()));
    if y_lo <= y_hi && b.x - a.x2() - 1 == gap {
        Some(((y_lo..=y_hi).map(|y| Tile::new(a.x2() + mid, y)).collect(), Dir::W))
    } else if y_lo <= y_hi && a.x - b.x2() - 1 == gap {
        Some(((y_lo..=y_hi).map(|y| Tile::new(a.x - mid, y)).collect(), Dir::E))
    } else if x_lo <= x_hi && b.y - a.y2() - 1 == gap {
        Some(((x_lo..=x_hi).map(|x| Tile::new(x, a.y2() + mid)).collect(), Dir::N))
    } else if x_lo <= x_hi && a.y - b.y2() - 1 == gap {
        Some(((x_lo..=x_hi).map(|x| Tile::new(x, a.y - mid)).collect(), Dir::S))
    } else {
        None
    }
}

/// Terminal rates after direct insertion; terminals fully served by it are dropped.
pub fn effective_terminals(plan: &ProductionPlan, links: &[DiLink], config: &RunConfiguration) -> HashMap<usize, EffectiveTerminal> {
    let drop = best_inserter_rate(config, false);
    let pick = best_inserter_rate(config, true);
    let mut out = HashMap::new();
    for t in plan.terminals.iter().filter(|t| !t.fluid) {
        let di = match t.role {
            Role::Producer => sum_f64(links.iter().filter(|l| Some(l.producer) == t.machine && l.item == t.item).map(|l| l.flow)),
            Role::Consumer => sum_f64(links.iter().filter(|l| Some(l.consumer) == t.machine && l.item == t.item).map(|l| l.flow)),
            _ => 0.0,
        };
        let rate = t.rate - di;
        if rate < 1e-6 {
            continue;
        }
        let k = match t.role {
            Role::Consumer => inserter_count(rate, pick),
            Role::Producer | Role::Sink => inserter_count(rate, drop),
            Role::Source | Role::Output => 1,
        };
        out.insert(t.id, EffectiveTerminal { base: t.clone(), rate, k });
    }
    out
}
