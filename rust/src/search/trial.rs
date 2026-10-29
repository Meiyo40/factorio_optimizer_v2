use crate::config::run_configuration::RunConfiguration;
use crate::config::tuning::Tuning;
use crate::data::data_model::RootData;
use crate::data::game_constants as gc;
use crate::geom::{Dir, Kind, Layout, PlacedEntity, Tile, border_tiles};
use crate::place::{AnnealOptions, Annealer, PlaceModel, PlacedMachine, Placement, static_obstacles};
use crate::plan::{ProductionPlan, Role, inserter_rate};
use crate::power::{build_harness, place_poles, pole_candidates, pole_lattice};
use crate::route::ring_ports::{RingedPorts, ring_ports};
use crate::route::{RouteProblem, RouteResult, route};
use crate::util::java_hash::JMap;
use crate::util::java_random::{JavaRandom, java_shuffle};
use crate::util::splittable_random::SplittableRandom;
use crate::util::text::fmt_list;
use crate::util::{average_f64, compare_f64, now_ms, sum_f64};
use crate::validate::{MachineSpec, OutputSpec, SourceSpec, ValidationError, ValidationSpec, recipe_info_from, validate};
use anyhow::{Result, ensure};
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

/// What the validator should expect of this plan on this placement.
#[allow(clippy::too_many_arguments)] // the parameters of validationSpec in Kotlin
pub fn validation_spec(
    data: &RootData,
    plan: &ProductionPlan,
    config: &RunConfiguration,
    placement: &Placement,
    sources: &[SourceSpec],
    require_power: bool,
    outside_allowed: &HashSet<Tile>,
    outputs: &[OutputSpec],
) -> ValidationSpec {
    let recipe_info = recipe_info_from(data);
    let mut machines: HashMap<Tile, MachineSpec> = HashMap::new();
    for pm in &placement.machines {
        let inst = &plan.machines[pm.id];
        let info = recipe_info(&inst.recipe).expect("a planned recipe is in the data");
        // Inserters move only item products; fluids leave through pipes.
        let items: IndexMap<String, f64> = inst.item_products().map(|(k, v)| (k.clone(), *v)).collect();
        // The first product with the largest rate (maxByOrNull keeps the first of equals).
        let mut best: Option<(&String, f64)> = None;
        for (k, &v) in &items {
            if best.is_none_or(|(_, bv)| bv < v) {
                best = Some((k, v));
            }
        }
        let sinks_here: Vec<_> = plan.terminals.iter().filter(|t| t.machine == Some(pm.id) && t.role == Role::Sink).collect();
        let mut sinks: IndexMap<String, f64> = IndexMap::new();
        for t in &sinks_here {
            sinks.entry(t.item.clone()).or_insert(0.0);
        }
        for (item, rate) in sinks.iter_mut() {
            *rate = sum_f64(sinks_here.iter().filter(|t| &t.item == item).map(|t| t.rate));
        }
        let mut ingredients = info.ingredients.clone();
        ingredients.extend(inst.demand.keys().cloned());
        let spec = MachineSpec {
            product: best.map_or_else(|| inst.product.clone(), |(k, _)| k.clone()),
            // Fuel counts as an ingredient (coal for stone furnaces, nutrients for biochambers).
            ingredients,
            output_rate: sum_f64(items.values().copied()),
            sink_rate: sum_f64(sinks_here.iter().map(|t| t.rate)),
            demand: inst.demand.clone(),
            needs_fluid: inst.needs_fluid.clone(),
            fluid_in: inst.fluid_in.clone(),
            fluid_out: inst.fluid_out.clone(),
            products: if items.len() > 1 { items } else { IndexMap::new() },
            sinks,
        };
        machines.insert(Tile::new(pm.x, pm.y), spec);
    }
    let outside = |t: &Tile| t.x < 0 || t.x >= placement.w || t.y < 0 || t.y >= placement.h;
    let mut outside_allowed = outside_allowed.clone();
    outside_allowed.extend(sources.iter().map(|s| s.tile).chain(outputs.iter().map(|o| o.tile)).filter(outside));
    let config = Arc::new(config.clone());
    let rate_config = Arc::clone(&config);
    ValidationSpec {
        machines,
        sources: sources.to_vec(),
        outputs: outputs.to_vec(),
        pole_ring: i32::from(config.power.allow_outside_ring),
        outside_allowed,
        require_power,
        fluid_mode: config.fluids.mode.clone(),
        check_rates: true,
        inserter_rate: Arc::new(move |name, from_belt| inserter_rate(name, &rate_config, from_belt)),
        recipe_info,
    }
}

pub fn layout_of(placement: &Placement, entities: Vec<PlacedEntity>, wires: Vec<(usize, usize)>, label: &str) -> Layout {
    Layout { w: placement.w, h: placement.h, entities, wires, label: label.to_owned(), tiles: Vec::new() }
}

/// score = W * H + lambda * belts + mu * undergrounds + nu * inserters (lower is better).
pub fn score(layout: &Layout, config: &RunConfiguration) -> f64 {
    f64::from(layout.w) * f64::from(layout.h)
        + config.score.belt * layout.count(Kind::Belt) as f64
        + config.score.underground * (layout.count(Kind::UgIn) + layout.count(Kind::UgOut)) as f64
        + config.score.inserter * layout.count(Kind::Inserter) as f64
}

pub struct TrialResult {
    pub success: bool,
    pub w: i32,
    pub h: i32,
    pub seed: i64,
    pub layout: Option<Layout>,
    pub placement: Option<Placement>,
    pub errors: Vec<ValidationError>,
    pub route: Option<RouteResult>,
    pub score: f64,
    /// Where it stopped: "placement", "route", "power", "validate" or "ok".
    pub stage: &'static str,
    pub anneal_t0: f64,
    pub millis: i64,
    pub notes: Vec<String>,
    /// Where inputs enter and outputs leave the finished layout (on the ring when ports_on_ring is set).
    pub sources: Vec<SourceSpec>,
    pub outputs: Vec<OutputSpec>,
    /// How routing went, for progress output.
    pub summary: Option<AttemptSummary>,
}

impl TrialResult {
    pub fn area(&self) -> i32 {
        self.w * self.h
    }
}

/// A trial's routing at a glance: how many rounds it routed and what its closest round still had wrong.
#[derive(Debug, Clone, PartialEq)]
pub struct AttemptSummary {
    /// Routing runs: placement feedback rounds plus spread and near-miss retries.
    pub rounds: i32,
    /// Tile conflicts left in the closest run (0 when it routed).
    pub overuse: i32,
    /// Plan nets the closest run left unrouted.
    pub failed_nets: Vec<i32>,
    /// The closest run's conflict tiles and the plan nets that meet on each.
    pub hotspots: IndexMap<Tile, Vec<i32>>,
}

/// The arguments of `runTrial` after the box and the seed, which Kotlin has defaults for.
#[derive(Debug, Clone, Copy)]
pub struct TrialOptions<'a> {
    /// A placement to start from (scaled from a larger box); `warm_t0` then replaces the sampled starting temperature.
    pub warm: Option<&'a Placement>,
    pub warm_t0: Option<f64>,
    /// Wall-clock milliseconds after which no further routing run starts.
    pub deadline: i64,
    /// Fixed poles in a lattice instead of a set cover; the config's power mode when not given.
    pub lattice: Option<bool>,
    /// Where the router draws its debug renders for the feedback rounds (a folder `round<N>` each), if anywhere.
    pub debug_dir: Option<&'a Path>,
}

impl Default for TrialOptions<'_> {
    fn default() -> Self {
        Self { warm: None, warm_t0: None, deadline: i64::MAX, lattice: None, debug_dir: None }
    }
}

/// Poles, their wires, whether every entity is covered, and the tiles of those that are not.
type Power = (Vec<PlacedEntity>, Vec<(usize, usize)>, bool, Vec<Tile>);

/// What the trial's routing runs left behind, so the progress output can show the closest one.
struct Closest {
    weight: i32,
    overuse: i32,
    failed_nets: Vec<i32>,
    conflict_nets: IndexMap<Tile, Vec<i32>>,
}

/// The bookkeeping of one trial that its early exits share.
struct Progress {
    w: i32,
    h: i32,
    seed: i64,
    started: i64,
    notes: Vec<String>,
    runs: i32,
    closest: Option<Closest>,
}

impl Progress {
    /// Every routing run counts towards the summary; the closest one (fewest conflicts, unrouted chains weighted)
    /// is what the progress output shows.
    fn consider(&mut self, r: &RouteResult, unrouted_penalty: i32) {
        self.runs += 1;
        let weight = r.overuse + unrouted_penalty * r.failed_nets.len() as i32;
        if self.closest.as_ref().is_none_or(|c| weight < c.weight) {
            self.closest = Some(Closest { weight, overuse: r.overuse, failed_nets: r.failed_nets.clone(), conflict_nets: r.conflict_nets.clone() });
        }
    }

    fn summary(&self) -> Option<AttemptSummary> {
        self.closest.as_ref().map(|c| AttemptSummary {
            rounds: self.runs,
            overuse: c.overuse.max(0),
            failed_nets: c.failed_nets.clone(),
            hotspots: c.conflict_nets.clone(),
        })
    }

    fn fail(&mut self, annealer: &Annealer, stage: &'static str, route: Option<RouteResult>, errors: Vec<ValidationError>, layout: Option<Layout>) -> TrialResult {
        TrialResult {
            success: false,
            w: self.w,
            h: self.h,
            seed: self.seed,
            layout,
            placement: annealer.model.placed().then(|| annealer.model.placement()),
            errors,
            route,
            score: f64::MAX,
            stage,
            anneal_t0: annealer.last_t0,
            millis: now_ms() - self.started,
            notes: std::mem::take(&mut self.notes),
            sources: Vec::new(),
            outputs: Vec::new(),
            summary: self.summary(),
        }
    }
}

/// Tiles covered by poles at `lattice` (each pole is `size` x `size`).
fn lattice_cover(lattice: &[Tile], size: i32) -> Vec<Tile> {
    lattice.iter().flat_map(|t| (0..size).flat_map(move |dy| (0..size).map(move |dx| Tile::new(t.x + dx, t.y + dy)))).collect()
}

/// One route of `placement` with the given tuning, obstacles and per-attempt inputs.
fn route_once(
    plan: &ProductionPlan,
    config: &RunConfiguration,
    tuning: &crate::config::tuning::RouteTuning,
    placement: &Placement,
    obstacles: &HashSet<Tile>,
    tweak: impl FnOnce(&mut RouteProblem),
) -> RouteResult {
    let mut problem = RouteProblem::new(plan, config, tuning.clone(), placement);
    problem.obstacles = obstacles.clone();
    tweak(&mut problem);
    route(&problem)
}

/// One trial at a box size: place (Stage 2), route with up to N feedback rounds (Stage 3), harness and
/// power (Stage 4), validate. `opts.warm` seeds the placement (scaled from a larger box); `opts.warm_t0` then
/// replaces the sampled starting temperature.
#[allow(clippy::too_many_arguments)] // the parameters of runTrial in Kotlin
pub fn run_trial(
    data: &RootData,
    plan: &ProductionPlan,
    config: &RunConfiguration,
    tuning: &Tuning,
    w: i32,
    h: i32,
    seed: i64,
    opts: TrialOptions,
) -> Result<TrialResult> {
    let mut run = Progress { w, h, seed, started: now_ms(), notes: Vec::new(), runs: 0, closest: None };
    let lattice = opts.lattice.unwrap_or(config.power.mode == "lattice");
    let lattice_tiles = if lattice { pole_lattice(w, h, &config.power.pole) } else { Vec::new() };
    let pole_size = gc::pole(&config.power.pole)?.size;
    let mut obstacles = static_obstacles(config, plan, w, h);
    obstacles.extend(lattice_cover(&lattice_tiles, pole_size));
    let model = PlaceModel::new(plan, config, &tuning.place, w, h, &obstacles)?;
    let mut annealer = Annealer::new(model, SplittableRandom::new(seed));
    let pt = &tuning.place;
    let rt = &tuning.route;

    // Column starts (all seeds, or even ones with "mixed") are refined cool so their structure survives; the
    // others start warm from a larger box's best placement when there is one, else at random.
    ensure!(matches!(pt.initial.as_str(), "random" | "columns" | "mixed"), "place.initial must be random, columns or mixed");
    let columns = (pt.initial == "columns" || (pt.initial == "mixed" && seed % 2 == 0)) && annealer.initial_columns(plan);
    let warmed = match opts.warm {
        Some(warm) if !columns => {
            let legal = legalize(warm, &mut annealer.model);
            annealer.initial_from(&legal)
        }
        _ => false,
    };
    let started = columns || warmed || annealer.initial_random() || annealer.initial_rows(plan);
    if !started {
        return Ok(run.fail(&annealer, "placement", None, Vec::new(), None));
    }
    if columns {
        run.notes.push("column start".to_owned());
    }
    let budget = max_moves(plan, tuning);
    if columns {
        annealer.anneal(AnnealOptions { max_moves: Some(budget), start_range: Some(pt.column_start_range), refine: true, t0_scale: pt.column_t0_scale, ..AnnealOptions::default() });
    } else if warmed && opts.warm_t0.is_none() {
        // A given placement without a temperature of its own (merged factory blocks): refine it, keep its shape.
        annealer.anneal(AnnealOptions { max_moves: Some(budget), start_range: Some(3), refine: true, t0_scale: pt.column_t0_scale, ..AnnealOptions::default() });
    } else {
        let t0 = opts.warm_t0.filter(|_| warmed).map(|t| t / tuning.search.warm_t0_divisor);
        annealer.anneal(AnnealOptions { t0, max_moves: Some(budget), start_range: warmed.then_some(4), ..AnnealOptions::default() });
    }

    // Routing with feedback to the placer.
    let mut routed: Option<RouteResult> = None;
    let mut history: JMap<Tile, f64> = JMap::new();
    // The round that came closest, kept so a near miss is not lost to the next re-anneal.
    let mut best_placement: Option<Placement> = None;
    let mut best_score = i32::MAX;
    let mut best_conflicts: Vec<Tile> = Vec::new();
    for round in 0..=pt.feedback_rounds {
        if now_ms() > opts.deadline {
            return Ok(run.fail(&annealer, "route", routed, Vec::new(), None));
        }
        let placement = annealer.model.placement();
        let round_dir = opts.debug_dir.map(|d| d.join(format!("round{round}")));
        let r = route_once(plan, config, rt, &placement, &obstacles, |p| p.debug_dir = round_dir);
        run.consider(&r, rt.unrouted_penalty);
        if r.success {
            routed = Some(r);
            break;
        }
        let sc = r.overuse + rt.unrouted_penalty * r.failed_nets.len() as i32;
        if sc < best_score {
            best_score = sc;
            best_placement = Some(placement);
            best_conflicts = r.conflicts.clone();
        }
        let tail: Vec<&String> = r.log.iter().skip(r.log.len().saturating_sub(3)).collect();
        run.notes.push(format!("round {round}: overuse {}, failed nets {} {}", r.overuse, fmt_list(&r.failed_nets), fmt_list(tail)));
        if round == pt.feedback_rounds {
            routed = Some(r);
            break;
        }
        for (t, v) in r.history.iter_java() {
            history.merge(*t, *v, |old, add| old + add);
        }
        let ordered: Vec<(Tile, f64)> = history.iter_java().map(|(t, v)| (*t, *v)).collect();
        annealer.model.set_history(&ordered, pt.beta2);
        annealer.anneal(AnnealOptions { t0: Some(annealer.last_t0 / pt.feedback_t0_divisor), max_moves: Some(budget / 2), start_range: Some(3), ..AnnealOptions::default() });
        routed = Some(r);
    }
    let mut result = routed.expect("the feedback loop routes at least once");
    // Not close yet: spread the machines around the remaining conflicts apart and route again.
    if !result.success && best_placement.is_some() && best_score > pt.near_miss_max && best_score <= pt.spread_max {
        for attempt in 1..=pt.spread_attempts {
            let best = best_placement.as_ref().expect("checked above");
            if now_ms() > opts.deadline || !annealer.initial_from(best) {
                break;
            }
            let moved = spread_around(&mut annealer.model, &best_conflicts, pt.spread_radius);
            if moved == 0 {
                break;
            }
            let placement = annealer.model.placement();
            let again = route_once(plan, config, rt, &placement, &obstacles, |p| p.endgame_seed = 100 + i64::from(attempt));
            run.consider(&again, rt.unrouted_penalty);
            run.notes.push(format!(
                "spread retry {attempt} ({best_score}): moved {moved} machines, overuse {}, failed nets {}",
                again.overuse,
                fmt_list(&again.failed_nets)
            ));
            result = again;
            if result.success {
                break;
            }
            let sc = result.overuse + rt.unrouted_penalty * result.failed_nets.len() as i32;
            if sc < best_score {
                best_score = sc;
                best_placement = Some(placement);
                best_conflicts = result.conflicts.clone();
            }
        }
    }
    let near_start = best_placement.as_ref().filter(|_| !result.success && best_score <= pt.near_miss_max);
    if let Some(best) = near_start {
        let f = pt.near_miss_endgame_factor;
        let mut near_tuning = rt.clone();
        near_tuning.endgame_rounds = (f64::from(rt.endgame_rounds) * f) as i32;
        near_tuning.endgame_time_factor = rt.endgame_time_factor * f;
        for attempt in 1..=pt.near_miss_attempts {
            if now_ms() > opts.deadline || !annealer.initial_from(best) {
                break;
            }
            let moved = nudge_near(&mut annealer.model, &best_conflicts, &mut annealer.rnd);
            let placement = annealer.model.placement();
            let again = route_once(plan, config, &near_tuning, &placement, &obstacles, |p| p.endgame_seed = i64::from(attempt));
            run.consider(&again, rt.unrouted_penalty);
            let moving = moved.map_or_else(String::new, |m| format!(" moving machine {m}"));
            run.notes.push(format!("near miss ({best_score}) retry {attempt}{moving}: overuse {}, failed nets {}", again.overuse, fmt_list(&again.failed_nets)));
            result = again;
            if result.success {
                break;
            }
        }
    }
    if !result.success {
        return Ok(run.fail(&annealer, "route", Some(result), Vec::new(), None));
    }
    let placement = annealer.model.placement();

    // Harness (test mode) and power, with up to two warm reroutes that keep tiles free near unpowered entities.
    let belt = gc::belt_tier(&config.belt_tier)?.belt;
    let mut bias: JMap<Tile, f64> = JMap::new();
    for attempt in 0..=2 {
        let ringed = if config.ports_on_ring {
            ring_ports(&result.entities, &result.sources, &result.outputs, w, h, belt)
        } else {
            RingedPorts { entities: result.entities.clone(), sources: result.sources.clone(), outputs: result.outputs.clone() }
        };
        let harness = if config.test_mode { Some(build_harness(&ringed.sources, &ringed.entities, w, h, config, &ringed.outputs)?) } else { None };
        if let Some(hn) = &harness {
            run.notes.extend(hn.warnings.iter().cloned());
        }
        let mut entities = ringed.entities.clone();
        entities.extend(harness.iter().flat_map(|hn| hn.entities.iter().cloned()));
        let outside: Vec<Tile> = harness.iter().flat_map(|hn| hn.tiles.iter_java().copied()).collect();
        let sources = if config.test_mode { Vec::new() } else { ringed.sources.clone() };
        let (poles, wires, ok, uncovered): Power = if config.power.mode == "none" {
            (Vec::new(), Vec::new(), true, Vec::new())
        } else if lattice {
            let ps = place_poles(&entities, &[], &config.power.pole, &lattice_tiles);
            (ps.poles, ps.wires, ps.ok, ps.uncovered.iter().map(PlacedEntity::tile).collect())
        } else {
            let extra: Vec<Tile> = outside.iter().flat_map(|t| (-2..=2).flat_map(move |dy| (-2..=2).map(move |dx| Tile::new(t.x + dx, t.y + dy)))).collect();
            let keep_free: HashSet<Tile> = config.reserved_rects(w, h).iter().flat_map(|r| r.tiles()).collect();
            let candidates: Vec<Tile> = pole_candidates(w, h, config.power.allow_outside_ring, &extra).into_iter().filter(|t| !keep_free.contains(t)).collect();
            let ps = place_poles(&entities, &candidates, &config.power.pole, &[]);
            (ps.poles, ps.wires, ps.ok, ps.uncovered.iter().map(PlacedEntity::tile).collect())
        };
        if !ok && config.power.mode == "set-cover" && !lattice && attempt < 2 {
            run.notes.push(format!("power attempt {attempt}: {} uncovered", uncovered.len()));
            for u in &uncovered {
                for dy in -3..=3 {
                    for dx in -3..=3 {
                        bias.merge(Tile::new(u.x + dx, u.y + dy), 0.5, |old, add| old + add);
                    }
                }
            }
            let ordered: Vec<(Tile, f64)> = bias.iter_java().map(|(t, v)| (*t, *v)).collect();
            let raw_history = result.raw_history.clone();
            let again = route_once(plan, config, rt, &placement, &obstacles, |p| {
                p.tile_bias = ordered;
                p.warm_history = Some(raw_history);
            });
            if again.success {
                result = again;
            }
            continue;
        }
        if !ok && config.power.mode == "set-cover" && !lattice {
            run.notes.push("set cover failed twice; falling back to the pole lattice".to_owned());
            let again = TrialOptions { warm: Some(&placement), warm_t0: Some(annealer.last_t0), deadline: opts.deadline, lattice: Some(true), debug_dir: opts.debug_dir };
            return run_trial(data, plan, config, tuning, w, h, seed, again);
        }
        let offset = entities.len();
        let mut all = entities;
        all.extend(poles.iter().cloned());
        let marks = if config.marker_tile == "none" { Vec::new() } else { config.reserved_rects(w, h).iter().flat_map(|r| border_tiles(r, &config.marker_tile)).collect() };
        let layout = Layout {
            w,
            h,
            entities: all,
            wires: wires.iter().map(|&(a, b)| (a + offset, b + offset)).collect(),
            label: format!("factorio-optimizer {w}x{h} seed {seed}"),
            tiles: marks,
        };
        // Outside the box: harness, poles in the ring, and ring ports (in test mode the inputs are fed, not listed).
        let mut outside_allowed: HashSet<Tile> = outside.iter().copied().collect();
        outside_allowed.extend(poles.iter().flat_map(|p| p.rect().tiles()).filter(|t| t.x < 0 || t.x >= w || t.y < 0 || t.y >= h));
        outside_allowed.extend(ringed.sources.iter().map(|s| s.tile));
        let spec = validation_spec(data, plan, config, &placement, &sources, config.power.mode != "none", &outside_allowed, &ringed.outputs);
        let errors = validate(&layout, &spec);
        if !ok {
            return Ok(run.fail(&annealer, "power", Some(result), errors, Some(layout)));
        }
        if !errors.is_empty() {
            return Ok(run.fail(&annealer, "validate", Some(result), errors, Some(layout)));
        }
        let scored = score(&layout, config);
        return Ok(TrialResult {
            success: true,
            w,
            h,
            seed,
            layout: Some(layout),
            placement: Some(placement),
            errors,
            route: Some(result),
            score: scored,
            stage: "ok",
            anneal_t0: annealer.last_t0,
            millis: now_ms() - run.started,
            notes: std::mem::take(&mut run.notes),
            sources: ringed.sources,
            outputs: ringed.outputs,
            summary: run.summary(),
        });
    }
    Ok(run.fail(&annealer, "power", Some(result), Vec::new(), None))
}

/// How many tiles `m` is from `t` (0 when `t` is inside it).
fn distance(model: &PlaceModel, m: usize, t: Tile) -> i32 {
    let dx = 0.max(model.px[m] - t.x).max(t.x - (model.px[m] + model.mw[m] - 1));
    let dy = 0.max(model.py[m] - t.y).max(t.y - (model.py[m] + model.mh[m] - 1));
    dx + dy
}

/// Moves machine `m` one step `d * k` if that spot is legal; true when it moved.
fn try_shift(model: &mut PlaceModel, m: usize, d: Dir, k: i32) -> bool {
    let (nx, ny, nr) = (model.px[m] + d.dx() * k, model.py[m] + d.dy() * k, model.rot[m]);
    if !model.legal(&[m], &[nx], &[ny], &[nr]) {
        return false;
    }
    model.move_machines(&[m], &[nx], &[ny], &[nr]);
    true
}

/// Moves the machines within `radius` tiles of each cluster of `conflicts` one tile away from the cluster's
/// centre (two tiles, or along the other axis, when that is blocked), outermost first so the inner ones find
/// room. Returns how many moved.
fn spread_around(model: &mut PlaceModel, conflicts: &[Tile], radius: i32) -> usize {
    if conflicts.is_empty() {
        return 0;
    }
    let mut clusters: Vec<Vec<Tile>> = Vec::new();
    for &t in conflicts {
        match clusters.iter_mut().find(|cl| cl.iter().any(|c| c.manhattan(t) <= 2 * radius)) {
            Some(c) => c.push(t),
            None => clusters.push(vec![t]),
        }
    }
    let mut done: HashSet<usize> = HashSet::new();
    for cl in &clusters {
        let cx = average_f64(cl.iter().map(|t| f64::from(t.x)));
        let cy = average_f64(cl.iter().map(|t| f64::from(t.y)));
        let mx = |model: &PlaceModel, m: usize| f64::from(model.px[m]) + f64::from(model.mw[m]) / 2.0;
        let my = |model: &PlaceModel, m: usize| f64::from(model.py[m]) + f64::from(model.mh[m]) / 2.0;
        let mut machines: Vec<usize> = (0..model.n_m).filter(|m| !done.contains(m) && cl.iter().any(|&t| distance(model, *m, t) <= radius)).collect();
        // sortedByDescending: stable, largest distance from the centre first.
        let from_centre = |model: &PlaceModel, m: usize| (mx(model, m) - cx).abs() + (my(model, m) - cy).abs();
        machines.sort_by(|&a, &b| compare_f64(from_centre(model, b), from_centre(model, a)));
        for m in machines {
            let dx = mx(model, m) - cx;
            let dy = my(model, m) - cy;
            let horizontal = dx.abs() >= dy.abs();
            let primary = if horizontal { if dx >= 0.0 { Dir::E } else { Dir::W } } else if dy >= 0.0 { Dir::S } else { Dir::N };
            let secondary = if horizontal { if dy >= 0.0 { Dir::S } else { Dir::N } } else if dx >= 0.0 { Dir::E } else { Dir::W };
            for (d, k) in [(primary, 1), (primary, 2), (secondary, 1)] {
                if try_shift(model, m, d, k) {
                    done.insert(m);
                    break;
                }
            }
        }
    }
    done.len()
}

/// Shifts one machine next to `conflicts` by one or two tiles (random machine, direction and distance among
/// the legal moves) and returns its id, or None if none can move.
fn nudge_near(model: &mut PlaceModel, conflicts: &[Tile], rnd: &mut SplittableRandom) -> Option<usize> {
    if conflicts.is_empty() {
        return None;
    }
    let mut near: Vec<usize> = (0..model.n_m).filter(|&m| conflicts.iter().any(|&t| distance(model, m, t) <= 3)).collect();
    while !near.is_empty() {
        let m = near.remove(rnd.next_int_bound(near.len() as i32) as usize);
        let mut moves: Vec<(Dir, i32)> = Dir::ALL.iter().flat_map(|&d| [1, 2].map(|k| (d, k))).collect();
        java_shuffle(&mut moves, &mut JavaRandom::new(rnd.next_long()));
        for (d, k) in moves {
            if try_shift(model, m, d, k) {
                return Some(m);
            }
        }
    }
    None
}

fn max_moves(plan: &ProductionPlan, tuning: &Tuning) -> i64 {
    tuning.place.max_moves.min(tuning.place.moves_per_machine * plan.machines.len().max(5) as i64)
}

/// Scales `p` into `model`'s box and pushes each machine to the nearest legal spot (spiral search),
/// keeping the relative arrangement.
pub fn legalize(p: &Placement, model: &mut PlaceModel) -> Placement {
    let sx = f64::from(model.w) / f64::from(p.w);
    let sy = f64::from(model.h) / f64::from(p.h);
    model.clear();
    let mut out: Vec<PlacedMachine> = Vec::new();
    let mut order: Vec<&PlacedMachine> = p.machines.iter().collect();
    order.sort_by_key(|pm| pm.y * 10000 + pm.x); // stable, like sortedBy
    for pm in order {
        let m = pm.id;
        let tx = ((f64::from(pm.x) * sx).round() as i32).clamp(0, 0.max(model.w - model.mw[m]));
        let ty = ((f64::from(pm.y) * sy).round() as i32).clamp(0, 0.max(model.h - model.mh[m]));
        let code0 = pm.rot.ordinal() as i32 + if pm.mirror { 4 } else { 0 };
        let codes: Vec<i32> = if model.fluid[m] {
            let mut all = vec![code0];
            all.extend((0..8).filter(|&c| c != code0));
            all
        } else {
            vec![0]
        };
        let mut found: Option<PlacedMachine> = None;
        'search: for r in 0..=model.w.max(model.h) {
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs().max(dy.abs()) != r {
                        continue;
                    }
                    for &code in &codes {
                        let (x, y) = (tx + dx, ty + dy);
                        if model.legal(&[m], &[x], &[y], &[code]) {
                            found = Some(PlacedMachine { id: m, x, y, rot: Dir::ALL[(code % 4) as usize], mirror: code >= 4 });
                            break 'search;
                        }
                    }
                }
            }
        }
        let Some(f) = found else {
            model.clear();
            return Placement { w: model.w, h: model.h, machines: p.machines.clone() };
        };
        model.place_one(m, f.x, f.y, f.rot.ordinal() as i32 + if f.mirror { 4 } else { 0 });
        out.push(f);
    }
    model.clear();
    out.sort_by_key(|f| f.id);
    Placement { w: model.w, h: model.h, machines: out }
}
