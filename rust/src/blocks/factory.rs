use super::compact::compact_layout;
use super::split::{BlockPlan, Split, block_plan, merge_blocks, split_plan};
use crate::config::run_configuration::{BlocksConfig, BoxConfig, ConfigSource, RunConfiguration, SearchConfig};
use crate::config::tuning::Tuning;
use crate::data::data_model::RootData;
use crate::data::game_constants as gc;
use crate::geom::{Dir, Kind, Layout, PlacedEntity, Rect, Tile, border_tiles};
use crate::place::{PlacedMachine, Placement, port_candidates};
use crate::plan::{Net, Port, ProductionPlan, Role, Terminal};
use crate::power::{build_harness, place_poles, pole_candidates};
use crate::route::{RouteProblem, route};
use crate::search::search::Search;
use crate::search::trial::{TrialResult, score, validation_spec};
use crate::util::json::to_pretty_string;
use crate::util::now_ms;
use crate::util::text::{fmt_fixed, fmt_list};
use crate::validate::{OutputSpec, SourceSpec, ValidationError, validate};
use crate::viz::{RenderOptions, port_labels, render_layout};
use anyhow::{Context, Result, ensure};
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A stitched factory: the combined layout, what validation found, and where its inputs and outputs are.
pub struct Factory {
    pub layout: Layout,
    pub errors: Vec<ValidationError>,
    pub score: f64,
    pub sources: Vec<SourceSpec>,
    pub outputs: Vec<OutputSpec>,
    pub notes: Vec<String>,
}

impl Factory {
    pub fn success(&self) -> bool {
        self.errors.is_empty()
    }

    fn area(&self) -> i32 {
        self.layout.w * self.layout.h
    }
}

/// What `run_factory` shares between its steps.
struct Run<'a> {
    data: &'a RootData,
    config: &'a RunConfiguration,
    tuning: &'a Tuning,
    plan: &'a ProductionPlan,
    cfg: &'a BlocksConfig,
    out: &'a Path,
    log_file: PathBuf,
    started: i64,
    deadline: i64,
    best: Option<Factory>,
}

impl Run<'_> {
    fn say(&self, msg: &str) {
        let line = format!("[{:>7}s] {msg}", fmt_fixed((now_ms() - self.started) as f64 / 1000.0, 1));
        println!("{line}");
        let appended = std::fs::OpenOptions::new().create(true).append(true).open(&self.log_file).and_then(|mut f| f.write_all(format!("{line}\n").as_bytes()));
        if let Err(e) = appended {
            eprintln!("warning: could not write {}: {e}", self.log_file.display());
        }
    }

    /// Seconds left of the time budget.
    fn left(&self) -> f64 {
        (self.deadline - now_ms()) as f64 / 1000.0
    }

    /// The config a block is searched with: no blocks, no harness, no reserved areas, this budget and box shape.
    fn block_config(&self, budget: f64, fixed_height: Option<i32>, start_size: Option<i32>) -> RunConfiguration {
        let config = self.config;
        RunConfiguration {
            blocks: None,
            test_mode: false,
            reserved: Vec::new(),
            r#box: BoxConfig {
                start_size,
                max_size: None,
                fixed_height,
                fixed_width: if fixed_height.is_some() { None } else { config.r#box.fixed_width },
                ..config.r#box.clone()
            },
            search: SearchConfig { time_budget_s: budget, ..config.search.clone() },
            ..config.clone()
        }
    }

    /// Searches one block's plan with its own config (the Kotlin call sits inside `runCatching` where it can fail).
    fn search_block(&self, budget: f64, fixed_height: Option<i32>, start_size: Option<i32>, plan: &ProductionPlan, dir: &Path, start_from: Option<&Placement>) -> Result<Option<Arc<TrialResult>>> {
        let config = self.block_config(budget, fixed_height, start_size);
        Ok(Search::new(self.data, &config, self.tuning, plan, dir, start_from).run()?.best)
    }

    /// Stitches the blocks, widening the corridors when the belts between blocks do not fit.
    fn stitch_any(&self, sp: &Split, bpl: &[BlockPlan], res: &[Arc<TrialResult>], what: &str) -> Option<Factory> {
        let mut last: Option<Factory> = None;
        // Inputs and outputs routed to the factory's edge share the corridor along the rows: one track per belt, two
        // per pipe (pipes of different fluids may not touch). Compaction takes out what stays empty.
        let tracks = sp.links.iter().filter(|l| l.routed).map(|l| if l.fluid { 2 } else { 1 }).sum::<i32>() * if self.config.test_mode { 5 } else { 1 };
        for attempt in 0..=3 {
            let corridor = self.cfg.corridor + 3 * attempt;
            let stitched = stitch(self.data, self.config, self.tuning, self.plan, sp, bpl, res, corridor, corridor + tracks);
            if let Err(e) = &stitched {
                self.say(&format!("{what}, corridor {corridor}: failed with {e:#}"));
            }
            // Kotlin's `getOrElse { say(...); null } ?: run { say(...) }` says this after an exception too.
            let Some(f) = stitched.ok().flatten() else {
                self.say(&format!("{what}, corridor {corridor}: the belts between blocks did not route"));
                continue;
            };
            let verdict = if f.success() { "valid".to_owned() } else { format!("{} validation errors: {}", f.errors.len(), fmt_list(f.errors.iter().take(5))) };
            let notes: String = f.notes.iter().map(|n| format!("; {n}")).collect();
            self.say(&format!("{what}, corridor {corridor}: {} x {}, {verdict}{notes}", f.layout.w, f.layout.h));
            if f.success() {
                return Some(f);
            }
            last = Some(f);
        }
        last
    }

    /// Keeps `f` (and writes it) when it is valid and smaller than the best so far.
    fn consider(&mut self, f: Option<Factory>, what: &str) -> Result<bool> {
        let Some(f) = f.filter(Factory::success) else { return Ok(false) };
        if let Some(b) = &self.best
            && f.area() >= b.area()
        {
            self.say(&format!("{what}: {} x {} is not smaller than {} x {}", f.layout.w, f.layout.h, b.layout.w, b.layout.h));
            return Ok(false);
        }
        write_factory(&f, self.out, self.config)?;
        self.say(&format!("{what}: {} x {} ({} tiles), saved", f.layout.w, f.layout.h, f.area()));
        self.best = Some(f);
        Ok(true)
    }
}

/// Runs a job as several blocks (see `split_plan`): each block is searched on its own (its share of the time
/// budget, by machine count), then the blocks are placed in their rows with corridors between them, the belts
/// between blocks are routed through the corridors, rows and columns that only straight belts cross are taken
/// out, poles cover the whole factory, and the result is validated against the full plan. If the corridor belts
/// do not fit, the corridors widen and it tries again. With `BlocksConfig.repack` every block is then
/// searched again at a common height per row, and with `BlocksConfig.merge` neighbouring blocks are merged
/// and searched again from their placements until the budget runs out; either is kept only if the factory gets
/// smaller. Every better factory is written to `out` (blueprint, render, ports.json; factory.txt logs it all), each block
/// to `block_<n>`.
pub fn run_factory(data: &RootData, config: &RunConfiguration, tuning: &Tuning, plan: &ProductionPlan, out: &Path) -> Result<Option<Factory>> {
    let cfg = config.blocks.as_ref().context("run_factory needs a blocks section")?;
    ensure!(
        plan.ports.iter().all(|p| p.specs.iter().all(|s| s.edge.is_some())),
        "a factory of blocks takes its inputs and outputs by edge (with from and to), not by x and y"
    );
    std::fs::create_dir_all(out)?;
    let log_file = out.join("factory.txt");
    std::fs::write(&log_file, "")?;
    let started = now_ms();
    let mut run = Run { data, config, tuning, plan, cfg, out, log_file, started, deadline: started + (config.search.time_budget_s * 1000.0) as i64, best: None };
    let tier = gc::belt_tier(&config.belt_tier)?;
    let mut split = split_plan(plan, cfg, tier.throughput);
    for w in &split.warnings {
        run.say(&format!("WARNING: {w}"));
    }
    let sizes: Vec<String> = split.blocks.iter().enumerate().map(|(b, ms)| format!("block {}: {}", b + 1, ms.len())).collect();
    run.say(&format!("{} machines in {} blocks ({} row(s)): {}", plan.machines.len(), split.blocks.len(), split.rows, sizes.join(", ")));
    let mut between: IndexMap<(i32, i32), Vec<&super::split::Link>> = IndexMap::new();
    for l in split.links.iter().filter(|l| l.inner()) {
        between.entry((l.from, l.to)).or_default().push(l);
    }
    for ((a, b), links) in &between {
        let mut per_item: IndexMap<&str, Vec<&super::split::Link>> = IndexMap::new();
        for l in links {
            per_item.entry(l.item.as_str()).or_default().push(l);
        }
        let items: Vec<String> = per_item.iter().map(|(item, ls)| format!("{item} {}/s ({} belt(s))", fmt_fixed(crate::util::sum_f64(ls.iter().map(|l| l.rate)), 2), ls.len())).collect();
        run.say(&format!("  block {} -> block {}: {}", a + 1, b + 1, items.join(", ")));
    }
    let mut bps: Vec<BlockPlan> = (0..split.blocks.len()).map(|b| block_plan(plan, &split, b, tuning)).collect();
    let later = (if cfg.repack { cfg.repack_share } else { 0.0 }) + (if cfg.merge { cfg.merge_share } else { 0.0 });

    // 1. Blocks, one after another, each with all threads and its share of the budget.
    let total = plan.machines.len() as f64;
    let mut results: Vec<Arc<TrialResult>> = Vec::new();
    for bp in &bps {
        let budget = run.left().min(config.search.time_budget_s * cfg.block_time_share * (1.0 - later) * bp.global_ids.len() as f64 / total);
        let dir = out.join(format!("block_{}", bp.index + 1));
        bp.plan.write(&dir.join("plan.json"))?;
        run.say(&format!("block {}: {} machines, {} s", bp.index + 1, bp.global_ids.len(), fmt_fixed(budget, 0)));
        let Some(best) = run.search_block(budget, None, None, &bp.plan, &dir, None)? else {
            run.say(&format!("block {}: no layout found; giving up", bp.index + 1));
            return Ok(None);
        };
        run.say(&format!("block {}: {} x {}", bp.index + 1, best.w, best.h));
        results.push(best);
    }

    // 2. Stitch, widening the corridors when the belts between blocks do not fit. Keep the smallest valid factory.
    let first = run.stitch_any(&split, &bps, &results, "factory");
    if !first.as_ref().is_some_and(Factory::success) {
        if let Some(f) = &first {
            write_factory(f, out, config)?;
        }
        return Ok(first);
    }
    run.consider(first, "factory")?;

    // 3. Repack: every block again at a common height per row: the height of the row's tallest block (no empty
    //    space above the others), and heights that make the factory about square. Each row repacked or not, the
    //    smallest valid factory wins.
    if cfg.repack && run.left() > 60.0 {
        let areas: Vec<i32> = results.iter().map(|r| r.w * r.h).collect();
        let corridor = cfg.corridor;
        let width = (f64::from(areas.iter().sum::<i32>()) * 1.15).sqrt().ceil() as i32;
        let rows: Vec<Vec<usize>> = (0..split.rows).map(|r| (0..split.blocks.len()).filter(|&b| split.slots[b].row == r).collect()).collect();
        let tallest: Vec<i32> = rows.iter().map(|bs| bs.iter().map(|&b| results[b].h).max().unwrap_or(0)).collect();
        let square: Vec<i32> = rows
            .iter()
            .map(|bs| {
                let row_area: i32 = bs.iter().map(|&b| areas[b]).sum();
                12.max((f64::from(row_area) * 1.1 / f64::from(1.max(width - corridor * (bs.len() as i32 - 1)))).ceil() as i32)
            })
            .collect();
        let mut plans: Vec<(&str, Vec<i32>)> = vec![("tallest", tallest), ("square", square)];
        if plans[0].1 == plans[1].1 {
            plans.pop(); // distinctBy the heights
        }
        let budget = config.search.time_budget_s * cfg.repack_share / plans.len() as f64;
        for (name, heights) in &plans {
            if run.left() < 60.0 {
                break;
            }
            run.say(&format!("repack ({name}): rows at height {}", fmt_list(heights)));
            let mut fresh = results.clone();
            for b in 0..split.blocks.len() {
                if run.left() < 30.0 {
                    break;
                }
                let hgt = heights[split.slots[b].row as usize];
                if hgt == results[b].h && *name == "tallest" {
                    continue; // already that tall
                }
                let start = ((f64::from(areas[b]) * 1.2 / f64::from(hgt)).ceil() * f64::from(hgt)).sqrt().ceil() as i32;
                let share = run.left().min(budget * bps[b].global_ids.len() as f64 / total);
                let dir = out.join(format!("repack_{name}/block_{}", b + 1));
                let found = run.search_block(share, Some(hgt), Some(start), &bps[b].plan, &dir, None).unwrap_or_else(|e| {
                    run.say(&format!("repack block {}: failed with {e:#}", b + 1));
                    None
                });
                match found {
                    Some(r) => {
                        run.say(&format!("repack block {}: {} x {} (was {} x {})", b + 1, r.w, r.h, results[b].w, results[b].h));
                        fresh[b] = r;
                    }
                    None => run.say(&format!("repack block {}: nothing at height {hgt}, keeping {} x {}", b + 1, results[b].w, results[b].h)),
                }
            }
            let base = results.clone();
            for mask in 1..(1usize << split.rows) {
                let repacked = |b: usize| (mask >> split.slots[b].row) & 1 == 1;
                let mix: Vec<Arc<TrialResult>> = (0..results.len()).map(|b| if repacked(b) { fresh[b].clone() } else { base[b].clone() }).collect();
                let which: Vec<usize> = (0..split.rows as usize).filter(|r| (mask >> r) & 1 == 1).collect();
                let stitched = run.stitch_any(&split, &bps, &mix, &format!("repacked ({name}) rows {}", fmt_list(&which)));
                if run.consider(stitched, "repacked factory")? {
                    results = mix;
                }
            }
        }
    }

    // 4. Merge neighbours: two blocks of a row become one, searched from their placements side by side.
    if cfg.merge {
        let max_merged = cfg.merge_max_machines.unwrap_or(2 * cfg.max_machines) as usize;
        let mut tried: HashSet<BTreeSet<usize>> = HashSet::new();
        while run.left() > 120.0 {
            let union = |split: &Split, a: usize, b: usize| -> BTreeSet<usize> { split.blocks[a].iter().chain(&split.blocks[b]).copied().collect() };
            let count = split.blocks.len();
            let pairs: Vec<(usize, usize)> = (0..count)
                .flat_map(|a| (0..count).map(move |b| (a, b)))
                .filter(|&(a, b)| split.slots[b].row == split.slots[a].row && split.slots[b].col == split.slots[a].col + 1)
                .filter(|&(a, b)| split.blocks[a].len() + split.blocks[b].len() <= max_merged && !tried.contains(&union(&split, a, b)))
                .collect();
            if pairs.is_empty() {
                run.say("merge: no more pairs to try");
                break;
            }
            // minBy: the first of the smallest.
            let (l, r) = pairs.iter().copied().fold(pairs[0], |best, p| {
                if split.blocks[p.0].len() + split.blocks[p.1].len() < split.blocks[best.0].len() + split.blocks[best.1].len() { p } else { best }
            });
            tried.insert(union(&split, l, r));
            let merged = merge_blocks(&split, l, r);
            let m = l - usize::from(r < l);
            let bp = block_plan(plan, &merged, m, tuning);
            // The two placements side by side, the right one a corridor further on, aligned as in their row.
            let hgt = results[l].h.max(results[r].h);
            let bottom = split.rows == 2 && split.slots[l].row == 1;
            let wide = results[l].w + cfg.corridor + results[r].w;
            let local: HashMap<usize, usize> = bp.global_ids.iter().enumerate().map(|(i, &g)| (g, i)).collect();
            let mut warm: Vec<PlacedMachine> = Vec::new();
            for (b, dx) in [(l, 0), (r, results[l].w + cfg.corridor)] {
                let dy = if bottom { hgt - results[b].h } else { 0 };
                let placement = results[b].placement.as_ref().context("a block's best trial has a placement")?;
                for pm in &placement.machines {
                    warm.push(PlacedMachine { id: local[&bps[b].global_ids[pm.id]], x: pm.x + dx, y: pm.y + dy, ..*pm });
                }
            }
            warm.sort_by_key(|pm| pm.id);
            let budget = (run.left() / pairs.len() as f64).clamp(600.0, 3600.0).min(run.left() - 60.0);
            run.say(&format!("merge blocks {} and {} ({} machines) from {wide} x {hgt}, {} s", l + 1, r + 1, bp.global_ids.len(), fmt_fixed(budget, 0)));
            let dir = out.join(format!("merge/{}_blocks_{}_{}", tried.len(), l + 1, r + 1));
            bp.plan.write(&dir.join("plan.json"))?;
            let start = (f64::from(wide) * f64::from(hgt)).sqrt().ceil() as i32;
            let start_from = Placement { w: wide, h: hgt, machines: warm };
            let found = run.search_block(budget, Some(hgt), Some(start), &bp.plan, &dir, Some(&start_from)).unwrap_or_else(|e| {
                run.say(&format!("merge blocks {} and {}: failed with {e:#}", l + 1, r + 1));
                None
            });
            let Some(res) = found else {
                run.say(&format!("merge blocks {} and {}: no layout", l + 1, r + 1));
                continue;
            };
            run.say(&format!("merge blocks {} and {}: {} x {} instead of {wide} x {hgt}", l + 1, r + 1, res.w, res.h));
            let gone = if split.slots[l].col < split.slots[r].col { r } else { l };
            let mut new_results: Vec<Arc<TrialResult>> = results.iter().enumerate().filter(|&(i, _)| i != gone).map(|(_, t)| t.clone()).collect();
            new_results[m] = res;
            let new_bps: Vec<BlockPlan> = (0..merged.blocks.len()).map(|i| if i == m { bp.clone() } else { block_plan(plan, &merged, i, tuning) }).collect();
            let stitched = run.stitch_any(&merged, &new_bps, &new_results, &format!("merged {}+{}", l + 1, r + 1));
            if run.consider(stitched, "merged factory")? {
                split = merged;
                bps = new_bps;
                results = new_results;
            }
        }
    }
    let summary = run.best.as_ref().map_or_else(|| "no valid factory".to_owned(), |f| format!("{} x {} ({} tiles)", f.layout.w, f.layout.h, f.area()));
    run.say(&format!("done: {summary}"));
    Ok(run.best)
}

/// Places the finished blocks, routes the belts between them, adds poles (and the test harness) and validates.
/// None when the blocks' ports do not line up or the belts between them do not route.
#[allow(clippy::too_many_arguments)] // the parameters of stitch in Kotlin
pub fn stitch(
    data: &RootData,
    config: &RunConfiguration,
    tuning: &Tuning,
    plan: &ProductionPlan,
    split: &Split,
    bps: &[BlockPlan],
    results: &[Arc<TrialResult>],
    corridor: i32,
    row_corridor: i32,
) -> Result<Option<Factory>> {
    let k = bps.len();
    // Geometry: every row left to right in slot order with a corridor between blocks, row 0 against the top
    // edge, row 1 against the bottom, and `row_corridor` along the rows (below a single row, between two).
    let mut row_h = vec![0i32; split.rows as usize];
    for (b, s) in split.slots.iter().enumerate() {
        row_h[s.row as usize] = row_h[s.row as usize].max(results[b].h);
    }
    let mut ox = vec![0i32; k];
    let mut widest = 0;
    for r in 0..split.rows {
        let mut x = 0;
        let mut in_row: Vec<usize> = (0..k).filter(|&b| split.slots[b].row == r).collect();
        in_row.sort_by_key(|&b| split.slots[b].col); // stable
        for b in in_row {
            ox[b] = x;
            x += results[b].w + corridor;
        }
        widest = widest.max(x - corridor);
    }
    // Reserved areas: columns of their own east of the blocks, against the top edge.
    let mut rx = widest + corridor;
    let mut reserved_rects: Vec<Rect> = Vec::new();
    for r in &config.reserved {
        reserved_rects.push(Rect::new(rx, 0, r.w, r.h));
        rx += r.w + corridor;
    }
    let width = reserved_rects.last().map_or(widest, |r| r.x2() + 1);
    let height = (row_h[0] + row_corridor + if split.rows == 2 { row_h[1] } else { 0 }).max(reserved_rects.iter().map(|r| r.h).max().unwrap_or(0));
    let oy: Vec<i32> = (0..k).map(|b| if split.slots[b].row == 0 { 0 } else { height - results[b].h }).collect();

    let mut entities: Vec<PlacedEntity> = Vec::new();
    let mut machines: Vec<PlacedMachine> = Vec::new();
    // Entities carry the full plan's net of their item (blocks number their nets on their own), so a corridor
    // belt handing over to a block's belt counts as the same net.
    let mut net_of_item: HashMap<&str, usize> = HashMap::new();
    for fluid in [false, true] {
        for n in plan.nets.iter().filter(|n| n.fluid == fluid) {
            net_of_item.insert(n.item.as_str(), n.id);
        }
    }
    let global = |e: &PlacedEntity, local: &ProductionPlan| -> Option<i32> {
        let net = usize::try_from(e.net?).ok().and_then(|n| local.nets.get(n))?;
        net_of_item.get(net.item.as_str()).map(|&id| id as i32)
    };
    for b in 0..k {
        let layout = results[b].layout.as_ref().context("a block's best trial has a layout")?;
        for e in layout.entities.iter().filter(|e| e.kind != Kind::Pole) {
            entities.push(PlacedEntity { x: e.x + ox[b], y: e.y + oy[b], net: global(e, &bps[b].plan), ..e.clone() });
        }
        let placement = results[b].placement.as_ref().context("a block's best trial has a placement")?;
        for pm in &placement.machines {
            machines.push(PlacedMachine { id: bps[b].global_ids[pm.id], x: pm.x + ox[b], y: pm.y + oy[b], ..*pm });
        }
    }
    let mut by_tile: HashMap<Tile, usize> = HashMap::new();
    for (i, e) in entities.iter().enumerate() {
        for t in e.rect().tiles() {
            by_tile.insert(t, i);
        }
    }

    // Link ends: an output belt of one block, an input belt of another; edge links are the factory's ports, either
    // where they leave their block or, routed through the corridors, on the factory edge the job asks for.
    let mut starts: HashMap<usize, (Tile, Dir)> = HashMap::new(); // link -> first tile outside the exporting block, the way its belt points
    let mut ends: HashMap<usize, (Tile, Dir)> = HashMap::new(); // link -> last tile outside the importing block, the way into it
    let mut sources: Vec<SourceSpec> = Vec::new();
    let mut outputs: Vec<OutputSpec> = Vec::new();
    for b in 0..k {
        let (bw, bh) = (results[b].w, results[b].h);
        let link_of = |port: i32| usize::try_from(port).ok().and_then(|p| bps[b].link_of_port.get(&p)).map(|&l| &split.links[l]);
        for o in &results[b].outputs {
            let Some(l) = link_of(o.port) else { continue };
            let t = Tile::new(o.tile.x + ox[b], o.tile.y + oy[b]);
            if !l.through_corridor() {
                outputs.push(OutputSpec { tile: t, port: -1, ..o.clone() });
                continue;
            }
            let d = if o.fluid {
                side_of(o.tile, bw, bh)
            } else {
                match by_tile.get(&t) {
                    Some(&i) => entities[i].dir,
                    None => return Ok(None),
                }
            };
            starts.insert(l.id, (t + d, d));
        }
        for s in &results[b].sources {
            let Some(l) = link_of(s.port) else { continue };
            let t = Tile::new(s.tile.x + ox[b], s.tile.y + oy[b]);
            if !l.through_corridor() {
                sources.push(SourceSpec { tile: t, port: -1, ..s.clone() });
                continue;
            }
            // The side the port is on, as the block was searched (after a merge a neighbour may sit elsewhere).
            let side = side_of(s.tile, bw, bh);
            ends.insert(l.id, (t + side, side.opposite()));
        }
    }
    let corridor_links: Vec<&super::split::Link> = split.links.iter().filter(|l| l.through_corridor()).collect();
    if corridor_links.iter().any(|l| (l.from >= 0 && !starts.contains_key(&l.id)) || (l.to >= 0 && !ends.contains_key(&l.id))) {
        return Ok(None);
    }

    // The belts (pipes) through the corridors: one net per link, from a fixed start tile (or the factory edge the
    // job asks for) to a fixed end tile (or that edge).
    let mut ports: Vec<Port> = Vec::new();
    let mut terminals: Vec<Terminal> = Vec::new();
    let mut nets: Vec<Net> = Vec::new();
    let mut edge_ports: HashSet<i32> = HashSet::new();
    let edge_specs = |item: &str, output: bool| -> Result<Vec<ConfigSource>> {
        let port = plan.ports.iter().find(|p| p.item == item && p.output == output).with_context(|| format!("the plan has no {} port for {item}", if output { "output" } else { "input" }))?;
        if !config.test_mode {
            return Ok(port.specs.clone());
        }
        // Test mode: every fifth tile of the edge only, so each port's harness (a feeder with inserters and chests
        // on both sides) has room next to its neighbours.
        Ok(port_candidates(&port.specs, width, height)
            .into_iter()
            .filter(|(t, d)| (if matches!(d, Some(Dir::N | Dir::S)) { t.x } else { t.y }) % 5 == 2)
            .map(|(t, d)| ConfigSource { name: item.to_owned(), x: Some(t.x), y: Some(t.y), dir: d.map(|d| d.name().to_owned()), ..ConfigSource::default() })
            .collect())
    };
    let fixed_port = |name: &str, (tile, dir): (Tile, Dir)| ConfigSource { name: name.to_owned(), x: Some(tile.x), y: Some(tile.y), dir: Some(dir.name().to_owned()), ..ConfigSource::default() };
    for l in &corridor_links {
        let p1 = if l.from < 0 {
            let p = Port { id: ports.len(), item: l.item.clone(), specs: edge_specs(&l.item, false)?, fluid: l.fluid, output: false, single: true };
            edge_ports.insert(p.id as i32);
            p
        } else {
            let (s, sd) = starts[&l.id];
            Port { id: ports.len(), item: l.item.clone(), specs: vec![fixed_port(&l.item, (s, sd.opposite()))], fluid: l.fluid, output: false, single: true }
        };
        let p1_id = p1.id;
        ports.push(p1);
        let p2 = if l.to < 0 {
            let p = Port { id: ports.len(), item: l.item.clone(), specs: edge_specs(&l.item, true)?, fluid: l.fluid, output: true, single: true };
            edge_ports.insert(p.id as i32);
            p
        } else {
            Port { id: ports.len(), item: l.item.clone(), specs: vec![fixed_port(&l.item, ends[&l.id])], fluid: l.fluid, output: true, single: true }
        };
        let p2_id = p2.id;
        ports.push(p2);
        let t1 = terminals.len();
        terminals.push(Terminal { id: t1, role: Role::Source, item: l.item.clone(), rate: l.rate, k: 1, machine: None, port: Some(p1_id), fluid: l.fluid });
        let t2 = terminals.len();
        terminals.push(Terminal { id: t2, role: Role::Output, item: l.item.clone(), rate: l.rate, k: 1, machine: None, port: Some(p2_id), fluid: l.fluid });
        nets.push(Net { id: nets.len(), item: l.item.clone(), flow: l.rate, terminals: vec![t1, t2], belt_count: 1, lane_pair: None, fluid: l.fluid });
    }
    let stitch_plan = ProductionPlan {
        machines: Vec::new(),
        ports,
        terminals,
        nets,
        raw_rates: IndexMap::new(),
        fluid_rates: IndexMap::new(),
        recipe_rates: IndexMap::new(),
        belt_tier: plan.belt_tier.clone(),
        warnings: Vec::new(),
        outputs: IndexMap::new(),
    };
    let mut block_tiles: HashSet<Tile> = HashSet::new();
    for b in 0..k {
        for y in oy[b]..oy[b] + results[b].h {
            for x in ox[b]..ox[b] + results[b].w {
                block_tiles.insert(Tile::new(x, y));
            }
        }
    }
    block_tiles.extend(reserved_rects.iter().flat_map(|r| r.tiles()));
    // Tiles a block belt points into from inside (chain ends at the block edge) must not get a corridor belt.
    let start_tiles: HashSet<Tile> = starts.values().map(|(t, _)| *t).collect();
    let pointed_at: Vec<Tile> = entities.iter().filter(|e| e.kind.belt_family()).filter_map(PlacedEntity::output_tile).filter(|t| !block_tiles.contains(t) && !start_tiles.contains(t)).collect();
    let stitch_config = RunConfiguration { blocks: None, lane_sharing: false, ..config.clone() };
    // Everything the blocks put outside their boxes (ring ports) is in the way too.
    let entity_tiles: Vec<Tile> = entities.iter().flat_map(|e| e.rect().tiles()).collect();
    // A corridor pipe beside a block's pipe would join it: keep those tiles free, except where a pipe link ends.
    let beside_pipes: Vec<Tile> = if corridor_links.iter().any(|l| l.fluid) {
        let link_tiles: HashSet<Tile> = starts.values().chain(ends.values()).map(|(t, _)| *t).collect();
        entities
            .iter()
            .filter(|e| matches!(e.kind, Kind::Pipe | Kind::PipeUg))
            .flat_map(|e| e.rect().tiles())
            .flat_map(|t| Dir::ALL.map(|d| t + d))
            .filter(|t| !block_tiles.contains(t) && !link_tiles.contains(t))
            .collect()
    } else {
        Vec::new()
    };
    let empty = Placement { w: width, h: height, machines: Vec::new() };
    let mut problem = RouteProblem::new(&stitch_plan, &stitch_config, tuning.route.clone(), &empty);
    problem.obstacles = block_tiles.iter().copied().chain(entity_tiles).chain(pointed_at).chain(beside_pipes).collect();
    problem.no_tunnel = block_tiles;
    let routed = route(&problem);
    if !routed.success {
        return Ok(None);
    }
    entities.extend(routed.entities.iter().map(|e| PlacedEntity { net: global(e, &stitch_plan), ..e.clone() }));
    sources.extend(routed.sources.iter().filter(|s| edge_ports.contains(&s.port)).map(|s| SourceSpec { port: -1, ..s.clone() }));
    outputs.extend(routed.outputs.iter().filter(|o| edge_ports.contains(&o.port)).map(|o| OutputSpec { port: -1, ..o.clone() }));
    let block_rects: Vec<Rect> = (0..k).map(|b| Rect::new(ox[b], oy[b], results[b].w, results[b].h)).collect();

    // Test harness on the factory's ports, then poles over everything, block borders, and validation.
    let finish = |ents: &[PlacedEntity], mach: &[PlacedMachine], srcs: &[SourceSpec], outs: &[OutputSpec], w: i32, h: i32, rects: &[Rect], reserved: &[Rect], mut notes: Vec<String>| -> Result<Factory> {
        let harness = if config.test_mode { Some(build_harness(srcs, ents, w, h, config, outs)?) } else { None };
        let mut all: Vec<PlacedEntity> = ents.to_vec();
        all.extend(harness.iter().flat_map(|hn| hn.entities.iter().cloned()));
        notes.extend(harness.iter().flat_map(|hn| hn.warnings.iter().cloned()));
        let mut poles: Vec<PlacedEntity> = Vec::new();
        let mut wires: Vec<(usize, usize)> = Vec::new();
        let keep_free: HashSet<Tile> = reserved.iter().flat_map(|r| r.tiles()).collect();
        let outside: Vec<Tile> = harness.iter().flat_map(|hn| hn.tiles.iter_java().copied()).collect();
        if config.power.mode != "none" {
            let extra: Vec<Tile> = outside.iter().flat_map(|t| (-2..=2).flat_map(move |dy| (-2..=2).map(move |dx| Tile::new(t.x + dx, t.y + dy)))).collect();
            let candidates: Vec<Tile> = pole_candidates(w, h, config.power.allow_outside_ring, &extra).into_iter().filter(|t| !keep_free.contains(t)).collect();
            let ps = place_poles(&all, &candidates, &config.power.pole, &[]);
            if !ps.ok {
                notes.push(format!("power: {} entities not covered", ps.uncovered.len()));
            }
            poles = ps.poles;
            wires = ps.wires;
        }
        let offset = all.len();
        // Block borders as floor tiles, so each block shows in the game.
        let marks = if config.marker_tile == "none" { Vec::new() } else { rects.iter().chain(reserved).flat_map(|r| border_tiles(r, &config.marker_tile)).collect() };
        let mut everything = all;
        everything.extend(poles.iter().cloned());
        let layout = Layout { w, h, entities: everything, wires: wires.iter().map(|&(a, b)| (a + offset, b + offset)).collect(), label: format!("factorio-optimizer factory {w}x{h}"), tiles: marks };
        // Outside the box: harness, poles in the ring, and ring ports (in test mode the harness feeds them instead of listing them).
        let mut outside_allowed: HashSet<Tile> = outside.iter().copied().collect();
        outside_allowed.extend(poles.iter().flat_map(|p| p.rect().tiles()).filter(|t| t.x < 0 || t.x >= w || t.y < 0 || t.y >= h));
        outside_allowed.extend(srcs.iter().map(|s| s.tile));
        let spec_sources: &[SourceSpec] = if config.test_mode { &[] } else { srcs };
        let placement = Placement { w, h, machines: mach.to_vec() };
        let spec = validation_spec(data, plan, config, &placement, spec_sources, config.power.mode != "none", &outside_allowed, outs);
        let errors = validate(&layout, &spec);
        let scored = score(&layout, config);
        Ok(Factory { layout, errors, score: scored, sources: srcs.to_vec(), outputs: outs.to_vec(), notes })
    };

    // Rows and columns that only straight belts and pipes cross go first; if that breaks anything (it should
    // not), the factory as stitched.
    if config.blocks.as_ref().is_none_or(|b| b.compact) {
        let c = compact_layout(&entities, &[], width, height);
        if c.rows_removed + c.cols_removed > 0 {
            let rect = |r: &Rect| Rect::new(c.x(r.x), c.y(r.y), c.x(r.x2() + 1) - c.x(r.x), c.y(r.y2() + 1) - c.y(r.y));
            let notes = vec![format!("compacted: {} rows and {} columns removed from {width} x {height}", c.rows_removed, c.cols_removed)];
            let moved_machines: Vec<PlacedMachine> = machines.iter().map(|m| PlacedMachine { x: c.x(m.x), y: c.y(m.y), ..*m }).collect();
            let moved_sources: Vec<SourceSpec> = sources.iter().map(|s| SourceSpec { tile: c.tile(s.tile), ..s.clone() }).collect();
            let moved_outputs: Vec<OutputSpec> = outputs.iter().map(|o| OutputSpec { tile: c.tile(o.tile), ..o.clone() }).collect();
            let rects: Vec<Rect> = block_rects.iter().map(rect).collect();
            let reserved: Vec<Rect> = reserved_rects.iter().map(rect).collect();
            let f = finish(&c.entities, &moved_machines, &moved_sources, &moved_outputs, c.w, c.h, &rects, &reserved, notes)?;
            if f.success() {
                return Ok(Some(f));
            }
            println!("compaction broke the factory ({}); keeping it as stitched", fmt_list(f.errors.iter().take(3)));
        }
    }
    Ok(Some(finish(&entities, &machines, &sources, &outputs, width, height, &block_rects, &reserved_rects, Vec::new())?))
}

/// The box side a port tile of a `w` x `h` block is on (on the ring around it, or on its edge).
fn side_of(t: Tile, w: i32, h: i32) -> Dir {
    if t.x < 0 {
        Dir::W
    } else if t.x >= w {
        Dir::E
    } else if t.y < 0 {
        Dir::N
    } else if t.y >= h {
        Dir::S
    } else if t.x == 0 {
        Dir::W
    } else if t.x == w - 1 {
        Dir::E
    } else if t.y == 0 {
        Dir::N
    } else {
        Dir::S
    }
}

/// Writes the factory's blueprint, render and ports.
fn write_factory(f: &Factory, out: &Path, config: &RunConfiguration) -> Result<()> {
    let write = |name: &str, text: String| -> Result<()> {
        let path = out.join(name);
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, text).with_context(|| format!("Could not write {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("Could not write {}", path.display()))
    };
    write("best_output_string.txt", f.layout.to_blueprint_string(&config.quality))?;
    write("best_blueprint.json", f.layout.to_blueprint(&config.quality).to_json())?;
    let title = format!("factory {}x{} score {}", f.layout.w, f.layout.h, fmt_fixed(f.score, 1));
    render_layout(&f.layout, &out.join("best.png"), &RenderOptions { errors: f.errors.clone(), ports: port_labels(&f.sources, &f.outputs), title: Some(title), ..RenderOptions::default() })?;
    write("ports.json", to_pretty_string(&factory_manifest(f)))
}

/// Where the factory's inputs and outputs are (box-relative tiles, edge, item, rate, lane for shared belts).
pub fn factory_manifest(f: &Factory) -> Value {
    let (w, h) = (f.layout.w, f.layout.h);
    let edge = |x: i32, y: i32| {
        if y <= 0 {
            "north"
        } else if y >= h - 1 {
            "south"
        } else if x <= 0 {
            "west"
        } else if x >= w - 1 {
            "east"
        } else {
            "inside"
        }
    };
    let inputs: Vec<Value> = f
        .sources
        .iter()
        .map(|s| {
            let mut o = json!({"item": s.item, "rate": s.rate, "fluid": s.fluid, "x": s.tile.x, "y": s.tile.y, "edge": edge(s.tile.x, s.tile.y)});
            if s.lane >= 0 {
                o["lane"] = json!(if s.lane == 0 { "left" } else { "right" });
            }
            o
        })
        .collect();
    let outputs: Vec<Value> = f.outputs.iter().map(|o| json!({"item": o.item, "rate": o.rate, "fluid": o.fluid, "x": o.tile.x, "y": o.tile.y, "edge": edge(o.tile.x, o.tile.y)})).collect();
    json!({"box": {"w": w, "h": h}, "inputs": inputs, "outputs": outputs})
}
