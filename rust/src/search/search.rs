use super::trial::{AttemptSummary, TrialOptions, TrialResult, run_trial};
use crate::config::run_configuration::RunConfiguration;
use crate::config::tuning::Tuning;
use crate::data::data_model::RootData;
use crate::geom::{Kind, Layout, Tile};
use crate::place::{Placement, port_candidates};
use crate::plan::ProductionPlan;
use crate::util::json::to_pretty_string;
use crate::util::now_ms;
use crate::util::text::{fmt_fixed, fmt_list};
use crate::validate::{OutputSpec, SourceSpec};
use crate::viz::{RenderOptions, ScorePoint, port_labels, render_layout, write_score_graph};
use anyhow::{Context, Result};
use indexmap::{IndexMap, IndexSet};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq)]
pub struct Improvement {
    pub elapsed_ms: i64,
    pub w: i32,
    pub h: i32,
    pub score: f64,
    pub belts: usize,
    pub undergrounds: usize,
    pub inserters: usize,
    pub seed: i64,
}

pub struct SearchResult {
    pub best: Option<Arc<TrialResult>>,
    pub improvements: Vec<Improvement>,
    pub trials: usize,
    pub log: Vec<String>,
}

/// What a search calls with every finished trial.
type TrialHook<'a> = Box<dyn FnMut(&TrialResult) + Send + Sync + 'a>;

/// Every box tried so far, in order: tries, successes, best score and the closest failure.
#[derive(Clone)]
struct SizeStats {
    w: i32,
    h: i32,
    tries: i32,
    ok: i32,
    best_score: f64,
    closest_weight: i32,
    closest: String,
}

#[derive(Default)]
struct Shared {
    log: Vec<String>,
    history: IndexMap<(i32, i32), SizeStats>,
}

/// Stage 5: binary search on the box size, K seeds per size in parallel, warm starts from the best
/// placement so far, and anytime outputs on every improvement. A longer run never gives a worse result.
pub struct Search<'a> {
    data: &'a RootData,
    config: &'a RunConfiguration,
    tuning: &'a Tuning,
    plan: &'a ProductionPlan,
    out: PathBuf,
    /// A placement to start from until the search has a success of its own (it is refined, not re-annealed).
    start_from: Option<&'a Placement>,
    start: i64,
    deadline: i64,
    /// What the worker threads write to: the log and the size history (Kotlin's `lock`).
    shared: Mutex<Shared>,
    best: Option<Arc<TrialResult>>,
    improvements: Vec<Improvement>,
    trials: usize,
    /// Seeds used so far per size, so every retry at a size gets fresh ones.
    seeds_used: HashMap<i32, i32>,
    /// The score of every improvement and when it came, for the graph.
    scores: Vec<ScorePoint>,
    /// Called for every finished trial (calibration collects routing statistics here).
    on_trial: Option<TrialHook<'a>>,
}

/// `distinct()`: the first of each, in order.
fn distinct(items: impl IntoIterator<Item = String>) -> Vec<String> {
    items.into_iter().collect::<IndexSet<_>>().into_iter().collect()
}

/// Writes `text` to `path` through a temporary file, so a reader (or a kill) never sees half of it.
fn write_file(path: &Path, text: &str) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text).with_context(|| format!("Could not write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("Could not write {}", path.display()))
}

fn append_file(path: &Path, text: &str) -> Result<()> {
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).with_context(|| format!("Could not open {}", path.display()))?;
    f.write_all(text.as_bytes()).with_context(|| format!("Could not write {}", path.display()))
}

impl<'a> Search<'a> {
    pub fn new(
        data: &'a RootData,
        config: &'a RunConfiguration,
        tuning: &'a Tuning,
        plan: &'a ProductionPlan,
        out: impl Into<PathBuf>,
        start_from: Option<&'a Placement>,
    ) -> Self {
        let start = now_ms();
        Self {
            data,
            config,
            tuning,
            plan,
            out: out.into(),
            start_from,
            start,
            deadline: start + (config.search.time_budget_s * 1000.0) as i64,
            shared: Mutex::new(Shared::default()),
            best: None,
            improvements: Vec::new(),
            trials: 0,
            seeds_used: HashMap::new(),
            scores: Vec::new(),
            on_trial: None,
        }
    }

    /// Calls `f` on every finished trial, on the thread that runs the search, once the trials of a size are in.
    pub fn with_on_trial(mut self, f: impl FnMut(&TrialResult) + Send + Sync + 'a) -> Self {
        self.on_trial = Some(Box::new(f));
        self
    }

    fn item(&self, net: i32) -> String {
        usize::try_from(net).ok().and_then(|i| self.plan.nets.get(i)).map_or_else(|| format!("net {net}"), |n| n.item.clone())
    }

    /// "copper-plate x6 near (0,5)": conflict tiles grouped by the items that meet there, largest groups first.
    fn hotspots(&self, s: &AttemptSummary) -> String {
        let mut groups: IndexMap<String, Vec<Tile>> = IndexMap::new();
        for (tile, nets) in &s.hotspots {
            let mut items = distinct(nets.iter().map(|&n| self.item(n)));
            items.sort();
            let key = items.join("/");
            groups.entry(if key.is_empty() { "?".to_owned() } else { key }).or_default().push(*tile);
        }
        let mut entries: Vec<(&String, &Vec<Tile>)> = groups.iter().collect();
        entries.sort_by_key(|e| std::cmp::Reverse(e.1.len())); // stable, like sortedByDescending
        entries.iter().take(3).map(|(items, tiles)| format!("{items} x{} near ({},{})", tiles.len(), tiles[0].x, tiles[0].y)).collect::<Vec<_>>().join(", ")
    }

    fn weight(&self, s: &AttemptSummary) -> i32 {
        s.overuse + self.tuning.route.unrouted_penalty * s.failed_nets.len() as i32
    }

    /// One trial in a few words: how it ended, and for a failure how close it came and where.
    fn describe(&self, r: &TrialResult) -> String {
        let secs = format!("{}s", fmt_fixed(r.millis as f64 / 1000.0, 1));
        let s = r.summary.as_ref();
        let runs = s.map_or_else(String::new, |s| format!(", {} routing run{}", s.rounds, if s.rounds == 1 { "" } else { "s" }));
        match r.stage {
            "ok" => format!("ok in {secs}{runs}, score {}", fmt_fixed(r.score, 1)),
            "route" => {
                let closest = s.map_or_else(String::new, |s| {
                    let mut text = format!("; closest: {} conflicts", s.overuse);
                    if !s.failed_nets.is_empty() {
                        text += &format!(", unrouted {}", fmt_list(distinct(s.failed_nets.iter().map(|&n| self.item(n)))));
                    }
                    if !s.hotspots.is_empty() {
                        text += &format!(" ({})", self.hotspots(s));
                    }
                    text
                });
                format!("no route in {secs}{runs}{closest}")
            }
            "placement" => "the machines do not fit".to_owned(),
            stage => {
                let errors: Vec<String> = r.errors.iter().take(2).map(|e| format!("{} at ({},{}) {}", e.kind, e.tile.x, e.tile.y, e.message)).collect();
                format!("routed in {secs} but {stage} failed: {}", errors.join("; "))
            }
        }
    }

    /// Records a finished trial: a progress line, attempts.tsv, and the box's history.
    fn report(&self, r: &TrialResult, attempt: usize) {
        let described = self.describe(r);
        if self.config.search.verbose {
            self.say(&format!("  {}x{} try {attempt}: {described}", r.w, r.h));
        }
        let s = r.summary.as_ref();
        let mut shared = self.shared.lock().expect("a worker panicked");
        let path = self.out.join("attempts.tsv");
        if !path.exists() {
            let header = "elapsed_s\tw\th\tseed\tstage\tseconds\trouting_runs\tconflicts\tunrouted\tscore\twhere\n";
            if let Err(e) = write_file(&path, header) {
                eprintln!("warning: {e:#}");
            }
        }
        let where_ = if r.success {
            String::new()
        } else if r.stage == "route" {
            s.map_or_else(String::new, |s| self.hotspots(s))
        } else {
            r.errors.iter().take(2).map(|e| format!("{} ({},{})", e.kind, e.tile.x, e.tile.y)).collect::<Vec<_>>().join("; ")
        };
        let row = [
            fmt_fixed((now_ms() - self.start) as f64 / 1000.0, 1),
            r.w.to_string(),
            r.h.to_string(),
            r.seed.to_string(),
            r.stage.to_owned(),
            fmt_fixed(r.millis as f64 / 1000.0, 1),
            s.map_or(0, |s| s.rounds).to_string(),
            s.map_or_else(String::new, |s| s.overuse.to_string()),
            s.map_or_else(String::new, |s| distinct(s.failed_nets.iter().map(|&n| self.item(n))).join(",")),
            if r.success { fmt_fixed(r.score, 1) } else { String::new() },
            where_,
        ];
        if let Err(e) = append_file(&path, &format!("{}\n", row.join("\t"))) {
            eprintln!("warning: {e:#}");
        }
        let h = shared.history.entry((r.w, r.h)).or_insert_with(|| SizeStats { w: r.w, h: r.h, tries: 0, ok: 0, best_score: f64::MAX, closest_weight: i32::MAX, closest: String::new() });
        h.tries += 1;
        if r.success {
            h.ok += 1;
            h.best_score = h.best_score.min(r.score);
        } else {
            let weight = match s {
                Some(s) if r.stage == "route" => self.weight(s),
                _ => i32::MAX - 1,
            };
            if weight < h.closest_weight || h.closest.is_empty() {
                h.closest_weight = weight;
                h.closest = described.split_once("; closest: ").map_or(described.as_str(), |(_, rest)| rest).to_owned();
            }
        }
    }

    /// The boxes tried so far, smallest first: tries, successes, and the best result or closest miss.
    fn say_history(&self) {
        let mut rows: Vec<SizeStats> = self.shared.lock().expect("a worker panicked").history.values().cloned().collect();
        rows.sort_by_key(|h| h.w * h.h); // stable, like sortedBy
        self.say("history (box: tries, successes, best or closest):");
        for h in rows {
            let tail = if h.ok > 0 { format!("best score {}", fmt_fixed(h.best_score, 1)) } else { format!("closest {}", h.closest) };
            self.say(&format!("  {}x{}: {} tries, {} ok, {tail}", h.w, h.h, h.tries, h.ok));
        }
    }

    /// The plan in a few lines: machines per recipe, and per item its rate, inserters, belts and port.
    fn say_plan(&self, w: i32, h: i32) {
        let plan = self.plan;
        let mut per_recipe: IndexMap<String, usize> = IndexMap::new();
        for m in &plan.machines {
            *per_recipe.entry(format!("{} ({})", m.recipe, m.machine)).or_default() += 1;
        }
        let mut counts: Vec<(&String, &usize)> = per_recipe.iter().collect();
        counts.sort_by(|a, b| b.1.cmp(a.1)); // stable, like sortedByDescending
        let recipes: Vec<String> = counts.iter().map(|(name, n)| format!("{n}x {name}")).collect();
        self.say(&format!("plan: {} machines: {}", plan.machines.len(), recipes.join(", ")));
        self.say(&format!("  {:<26} {:>8} {:>9} {:>5}  {}", "item", "rate/s", "inserters", "belts", "port"));
        let visits_of = |n: &crate::plan::Net| -> i32 { plan.net_terminals(n).iter().filter(|t| t.machine.is_some()).map(|t| t.k).sum() };
        let mut nets: Vec<&crate::plan::Net> = plan.nets.iter().collect();
        nets.sort_by_key(|n| std::cmp::Reverse(visits_of(n))); // stable
        for n in nets {
            let visits = visits_of(n);
            let ports: Vec<&crate::plan::Port> = plan.net_terminals(n).iter().filter_map(|t| t.port.map(|p| &plan.ports[p])).collect();
            let described: Vec<String> = ports
                .iter()
                .map(|p| {
                    let tiles = port_candidates(&p.specs, w, h).len();
                    let place = |v: Option<i32>| v.map_or_else(|| "null".to_owned(), |v| v.to_string());
                    let wheres: Vec<String> = p
                        .specs
                        .iter()
                        .map(|s| match &s.edge {
                            Some(e) if s.from.is_some() || s.to.is_some() => format!("{e} {}..{}", s.from.unwrap_or(0), s.to.map_or_else(|| "end".to_owned(), |t| t.to_string())),
                            Some(e) => e.clone(),
                            None => format!("({},{})", place(s.x), place(s.y)),
                        })
                        .collect();
                    let plural = if tiles == 1 { "" } else { "s" };
                    let splitters = if !n.fluid && visits > self.tuning.route.max_chain_visits * tiles as i32 {
                        format!(" -> {tiles} belt{plural} with splitter branches (more than {} inserters per belt)", self.tuning.route.max_chain_visits)
                    } else {
                        String::new()
                    };
                    format!("{}{}, {tiles} tile{plural}{splitters}", if p.output { "out " } else { "in " }, wheres.join(","))
                })
                .collect();
            let item = format!("{}{}", n.item, if n.fluid { " (fluid)" } else { "" });
            let (inserters, belts) = if n.fluid { ("-".to_owned(), "-".to_owned()) } else { (visits.to_string(), n.belt_count.to_string()) };
            self.say(&format!("  {item:<26} {:>8} {inserters:>9} {belts:>5}  {}", fmt_fixed(n.flow, 2), described.join("; ")));
        }
    }

    /// Box for size parameter s: aspect ratio, or a fixed width (height) with the other side carrying the area.
    pub fn box_size(&self, s: i32) -> (i32, i32) {
        let area = f64::from(s) * f64::from(s);
        if let Some(w) = self.config.r#box.fixed_width {
            return (w, 3.max((area / f64::from(w)).ceil() as i32));
        }
        if let Some(h) = self.config.r#box.fixed_height {
            return (3.max((area / f64::from(h)).ceil() as i32), h);
        }
        let r = self.config.r#box.aspect.sqrt();
        (3.max((f64::from(s) * r).round() as i32), 3.max((f64::from(s) / r).round() as i32))
    }

    fn say(&self, msg: &str) {
        let line = format!("[{:>7}s] {msg}", fmt_fixed((now_ms() - self.start) as f64 / 1000.0, 1));
        println!("{line}");
        let mut shared = self.shared.lock().expect("a worker panicked");
        shared.log.push(line.clone());
        if let Err(e) = append_file(&self.out.join("log.txt"), &format!("{line}\n")) {
            eprintln!("warning: {e:#}");
        }
    }

    fn better(t: &TrialResult, than: Option<&TrialResult>) -> bool {
        than.is_none_or(|b| t.area() < b.area() || (t.area() == b.area() && t.score < b.score - 1e-9))
    }

    /// Records a successful trial if it improves on the best (area first, then score) and writes outputs.
    fn offer(&mut self, t: TrialResult) -> Result<()> {
        if !t.success || !Self::better(&t, self.best.as_deref()) {
            return Ok(());
        }
        let layout = t.layout.as_ref().expect("a successful trial has a layout");
        let imp = Improvement {
            elapsed_ms: now_ms() - self.start,
            w: t.w,
            h: t.h,
            score: t.score,
            belts: layout.count(Kind::Belt),
            undergrounds: layout.count(Kind::UgIn) + layout.count(Kind::UgOut),
            inserters: layout.count(Kind::Inserter),
            seed: t.seed,
        };
        self.improvements.push(imp.clone());
        self.write_best(&t)?;
        self.say(&format!(
            "improvement #{}: {}x{} score {} (belts {}, undergrounds {}, inserters {}, seed {})",
            self.improvements.len(),
            t.w,
            t.h,
            fmt_fixed(t.score, 1),
            imp.belts,
            imp.undergrounds,
            imp.inserters,
            t.seed
        ));
        self.best = Some(Arc::new(t));
        Ok(())
    }

    /// The best blueprint, its ports, its renders, a line of scores.tsv and the score graph.
    fn write_best(&mut self, t: &TrialResult) -> Result<()> {
        let n = self.improvements.len().max(1);
        let layout = Layout { label: format!("factorio-optimizer {}x{}", t.w, t.h), ..t.layout.clone().expect("a successful trial has a layout") };
        let blueprint = layout.to_blueprint_string(&self.config.quality);
        std::fs::create_dir_all(&self.out)?;
        write_file(&self.out.join("best_output_string.txt"), &blueprint)?;
        write_file(&self.out.join("best_blueprint.json"), &layout.to_blueprint(&self.config.quality).to_json())?;
        write_file(&self.out.join("ports.json"), &to_pretty_string(&port_manifest(t)))?;
        std::fs::create_dir_all(self.out.join("allSolves"))?;
        write_file(&self.out.join(format!("allSolves/{n:03} - {}x{} - {}.txt", t.w, t.h, fmt_fixed(t.score, 1))), &blueprint)?;
        let best_title = format!("{}x{} score {} seed {}", t.w, t.h, fmt_fixed(t.score, 1), t.seed);
        render_layout(&layout, &self.out.join("best.png"), &RenderOptions { ports: port_labels(&t.sources, &t.outputs), title: Some(best_title), ..RenderOptions::default() })?;
        let improvement_title = format!("#{n} {}x{} score {}", t.w, t.h, fmt_fixed(t.score, 1));
        render_layout(&layout, &self.out.join(format!("renders/improvement_{n:03}.png")), &RenderOptions { title: Some(improvement_title), ..RenderOptions::default() })?;
        let imp = self.improvements.last().expect("an improvement was just recorded");
        let header = if n == 1 { "elapsed_s\tw\th\tscore\tbelts\tundergrounds\tinserters\tseed\n" } else { "" };
        let row = format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            fmt_fixed(imp.elapsed_ms as f64 / 1000.0, 1),
            imp.w,
            imp.h,
            fmt_fixed(imp.score, 2),
            imp.belts,
            imp.undergrounds,
            imp.inserters,
            imp.seed
        );
        append_file(&self.out.join("scores.tsv"), &format!("{header}{row}"))?;
        self.scores.push(ScorePoint { timestamp: now_ms(), score: t.score });
        write_score_graph(&self.scores, &self.out, "Score (area + weighted entities)")
    }

    /// Rewrites the best result and notes why the search ended (Kotlin also calls it from a Ctrl-C hook; here every
    /// file is written whole or not at all, so a kill leaves the last complete result).
    pub fn flush(&mut self, reason: &str) -> Result<()> {
        if let Some(best) = self.best.clone() {
            self.write_best(&best)?;
        }
        let best = self.best.as_ref().map_or_else(|| "none".to_owned(), |b| format!("{}x{} score {}", b.w, b.h, fmt_fixed(b.score, 1)));
        append_file(&self.out.join("log.txt"), &format!("stopped: {reason}; best {best}\n"))
    }

    fn time_left(&self) -> bool {
        now_ms() < self.deadline
    }

    /// Runs K fresh seeds at size s in parallel; true if any succeeded.
    fn try_size(&mut self, s: i32, seeds: usize) -> Result<bool> {
        let (w, h) = self.box_size(s);
        let offset = self.seeds_used.get(&s).copied().unwrap_or(0) as usize;
        self.seeds_used.insert(s, (offset + seeds) as i32);
        let warm: Option<Placement> = self.best.as_ref().and_then(|b| b.placement.clone()).or_else(|| self.start_from.cloned());
        let warm_t0 = self.best.as_ref().map(|b| b.anneal_t0);
        let workers = (self.config.search.threads.max(1) as usize).min(seeds);
        let next = AtomicUsize::new(0);
        let me: &Search = self;
        let mut done: Vec<(usize, Option<Result<TrialResult>>)> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..workers)
                .map(|_| {
                    scope.spawn(|| {
                        let mut mine = Vec::new();
                        loop {
                            let k = next.fetch_add(1, Ordering::SeqCst);
                            if k >= seeds {
                                break;
                            }
                            let seed = me.config.search.seed.wrapping_mul(1_000_003).wrapping_add(i64::from(s).wrapping_mul(1009)).wrapping_add((k + offset) as i64);
                            // With debug_renders the router draws the first seed's routes of this size.
                            let debug = (me.config.debug_renders && k == 0).then(|| me.out.join(format!("debug/route_{w}x{h}_{seed}")));
                            let result = me.time_left().then(|| {
                                let opts = TrialOptions { warm: warm.as_ref(), warm_t0, deadline: me.deadline, lattice: None, debug_dir: debug.as_deref() };
                                run_trial(me.data, me.plan, me.config, me.tuning, w, h, seed, opts).inspect(|r| me.report(r, offset + k + 1))
                            });
                            mine.push((k, result));
                        }
                        mine
                    })
                })
                .collect();
            handles.into_iter().flat_map(|handle| handle.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic))).collect()
        });
        done.sort_by_key(|(k, _)| *k);
        let mut results: Vec<TrialResult> = Vec::new();
        for (_, result) in done {
            if let Some(result) = result {
                results.push(result?);
            }
        }
        self.trials += results.len();
        let ok = results.iter().filter(|r| r.success).count();
        let total = results.len();
        let mut failures: IndexMap<&'static str, usize> = IndexMap::new();
        for r in results.iter().filter(|r| !r.success) {
            *failures.entry(r.stage).or_default() += 1;
        }
        if self.config.debug_renders {
            for r in results.iter().filter(|r| !r.success) {
                let Some(layout) = &r.layout else { continue };
                let title = format!("{} {}", r.stage, fmt_list(r.errors.iter().take(3)));
                render_layout(layout, &self.out.join(format!("debug/fail_{w}x{h}_{}.png", r.seed)), &RenderOptions { errors: r.errors.clone(), title: Some(title), ..RenderOptions::default() })?;
            }
        }
        for r in results {
            if let Some(hook) = self.on_trial.as_mut() {
                hook(&r);
            }
            self.offer(r)?;
        }
        let failed = if failures.is_empty() {
            String::new()
        } else {
            format!(", failures {{{}}}", failures.iter().map(|(stage, n)| format!("{stage}={n}")).collect::<Vec<_>>().join(", "))
        };
        self.say(&format!("size {s} ({w}x{h}): {ok}/{total} seeds succeeded{failed}"));
        self.say_history();
        Ok(ok > 0)
    }

    pub fn run(&mut self) -> Result<SearchResult> {
        std::fs::create_dir_all(&self.out)?;
        let area = self.plan.total_machine_area() + self.config.reserved.iter().map(|r| r.w * r.h).sum::<i32>();
        let cap = self.config.r#box.max_size;
        let estimate = 3.max((f64::from(area) * self.tuning.search.area_factor).sqrt().ceil() as i32);
        let mut hi = cap.unwrap_or(i32::MAX).min(self.config.r#box.start_size.or(cap).unwrap_or(2 * estimate));
        let s_lo = estimate.min(hi);
        let k = self.config.search.seeds_per_size.max(0) as usize;
        let max = cap.map_or_else(String::new, |c| format!(", max {c}"));
        self.say(&format!(
            "machines {}, machine area {area}, s_lo {s_lo}, start {hi}{max}, budget {:?}s, {} seeds x {} threads",
            self.plan.machines.len(),
            self.config.search.time_budget_s,
            self.config.search.seeds_per_size,
            self.config.search.threads
        ));
        let (bw, bh) = self.box_size(hi);
        self.say_plan(bw, bh);

        // 1. Upper bound: grow gently until a size succeeds, with several seed rounds per size first. A failed
        //    size usually means routing dead ends other seeds avoid; doubling the box gives sparse layouts.
        let mut feasible = -1;
        let grow = self.tuning.search.grow_rounds.max(1);
        while self.time_left() && feasible < 0 {
            for _ in 0..grow {
                if !self.time_left() {
                    break;
                }
                if self.try_size(hi, k)? {
                    feasible = hi;
                    break;
                }
            }
            if feasible >= 0 {
                break;
            }
            if cap.is_some_and(|c| hi >= c) {
                continue; // at the largest allowed size: keep trying new seeds
            }
            if cap.is_none() && hi >= 8 * s_lo {
                self.say(&format!("no success up to size {hi}"));
                break;
            }
            hi = cap.unwrap_or(i32::MAX).min((hi + 1).max((f64::from(hi) * self.tuning.search.grow_factor).ceil() as i32));
        }
        if feasible < 0 {
            return Ok(self.finish("no feasible size found"));
        }

        // 2. Binary search between s_lo and the smallest known success.
        let mut lo = s_lo;
        hi = feasible;
        while lo < hi && self.time_left() {
            let mid = (lo + hi) / 2;
            if self.try_size(mid, k)? {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        self.say(&format!("search interval closed at size {hi}"));

        // 3. Extra seeds at the smallest feasible size and the one below, until the budget runs out.
        let mut round = 1;
        while self.time_left() && round <= self.tuning.search.extra_rounds {
            if hi > s_lo && self.try_size(hi - 1, k)? {
                hi -= 1;
            } else if self.time_left() {
                self.try_size(hi, k)?;
            }
            round += 1;
        }
        let reason = if self.time_left() { "extra seed rounds done" } else { "time budget spent" };
        Ok(self.finish(reason))
    }

    fn finish(&self, reason: &str) -> SearchResult {
        let best = self.best.as_ref().map_or_else(|| "none".to_owned(), |b| format!("{}x{} score {}", b.w, b.h, fmt_fixed(b.score, 1)));
        self.say(&format!("done: {reason} after {} trials; best {best}", self.trials));
        SearchResult { best: self.best.clone(), improvements: self.improvements.clone(), trials: self.trials, log: self.shared.lock().expect("a worker panicked").log.clone() }
    }
}


fn edge_of(t: &TrialResult, x: i32, y: i32) -> &'static str {
    if x <= 0 {
        "west"
    } else if x >= t.w - 1 {
        "east"
    } else if y <= 0 {
        "north"
    } else if y >= t.h - 1 {
        "south"
    } else {
        "inside"
    }
}

/// Where the block's inputs and outputs are, for connecting it: box-relative tiles (the blueprint's own
/// coordinates; a tile's centre is x + 0.5), the box edge, the item and its rate, and for two items sharing an
/// input belt, the lane each arrives on.
pub fn port_manifest(t: &TrialResult) -> Value {
    let route = t.route.as_ref();
    let sources: &[SourceSpec] = if t.sources.is_empty() { route.map_or(&[][..], |r| &r.sources[..]) } else { &t.sources };
    let outputs: &[OutputSpec] = if t.outputs.is_empty() { route.map_or(&[][..], |r| &r.outputs[..]) } else { &t.outputs };
    let inputs: Vec<Value> = sources
        .iter()
        .map(|s| {
            let mut o = json!({"item": s.item, "rate": s.rate, "fluid": s.fluid, "x": s.tile.x, "y": s.tile.y, "edge": edge_of(t, s.tile.x, s.tile.y)});
            // A shared belt: which lane this item arrives on, looking along the belt into the block.
            if s.lane >= 0 {
                o["lane"] = json!(if s.lane == 0 { "left" } else { "right" });
            }
            o
        })
        .collect();
    let outs: Vec<Value> = outputs.iter().map(|o| json!({"item": o.item, "rate": o.rate, "fluid": o.fluid, "x": o.tile.x, "y": o.tile.y, "edge": edge_of(t, o.tile.x, o.tile.y)})).collect();
    json!({"box": {"w": t.w, "h": t.h}, "inputs": inputs, "outputs": outs})
}

/// Entry for Main: runs the search, then writes the best result once more and notes that it finished.
pub fn run_search(data: &RootData, config: &RunConfiguration, tuning: &Tuning, plan: &ProductionPlan, out: &str) -> Result<SearchResult> {
    let mut search = Search::new(data, config, tuning, plan, out, None);
    let result = search.run()?;
    search.flush("finished")?;
    Ok(result)
}
