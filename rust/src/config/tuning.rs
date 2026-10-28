//! Experiment knobs. Defaults come from tuning.json (the measured values); a job may override any of them in
//! its own `tuning` section.

use crate::data::loader::read_resource_or_file;
use crate::util::json::to_pretty_string;
use anyhow::{Context, Result};
use serde_json::Value;

config_struct! {
    Tuning {
        planner: PlannerTuning = PlannerTuning::default(),
        net: NetTuning = NetTuning::default(),
        place: PlaceTuning = PlaceTuning::default(),
        route: RouteTuning = RouteTuning::default(),
        search: SearchTuning = SearchTuning::default(),
    }
}

impl Tuning {
    /// This tuning with `overrides` (same layout as tuning.json, any subset of keys) merged over it.
    pub fn with_overrides(&self, overrides: Option<&Value>) -> Result<Tuning> {
        let Some(overrides) = overrides.filter(|o| o.as_object().is_none_or(|m| !m.is_empty())) else {
            return Ok(self.clone());
        };
        let merged = merge(serde_json::to_value(self)?, overrides);
        serde_json::from_value(merged).context("Bad tuning override (keys and layout as in tuning.json)")
    }

    pub fn to_json(&self) -> String {
        to_pretty_string(self)
    }

    pub fn parse(text: &str) -> Result<Tuning> {
        serde_json::from_str(text).context("Bad tuning")
    }

    pub fn load(name: &str) -> Result<Tuning> {
        Self::parse(&read_resource_or_file(name)?)
    }

    /// The built-in tuning.json (the measured defaults), else the compiled-in ones.
    pub fn defaults() -> Tuning {
        Self::load("tuning.json").unwrap_or_default()
    }
}

fn merge(base: Value, over: &Value) -> Value {
    match (base, over) {
        (Value::Object(mut a), Value::Object(b)) => {
            for (k, v) in b {
                let merged = match a.remove(k) {
                    Some(existing) => merge(existing, v),
                    None => v.clone(),
                };
                a.insert(k.clone(), merged);
            }
            Value::Object(a)
        }
        (_, b) => b.clone(),
    }
}

config_struct! {
    /// Linear-program weights. Machines cost their area in assembler units (3 x 3 = 1) per machine.
    PlannerTuning {
        /// Per item/s supplied through a port.
        supply_cost_item: f64 = 0.001,
        /// Per fluid unit/s supplied.
        supply_cost_fluid: f64 = 0.0001,
        /// Per item/s of a by-product leaving the block unused (cracking and recycling are usually cheaper).
        surplus_cost_item: f64 = 10.0,
        surplus_cost_fluid: f64 = 0.1,
        /// Share of the tiles around a machine its belt inserters may take. A recipe whose machines would need more
        /// gets more machines, each with a smaller share (fast machines with productivity need this most).
        access_share: f64 = 0.75,
    }
}

config_struct! {
    NetTuning {
        /// Target belt utilisation when splitting an item over several belts.
        belt_utilisation: f64 = 0.9,
    }
}

config_struct! {
    PlaceTuning {
        alpha: f64 = 1.0,
        /// RUDY overflow weight. The plan started at 0.5; 20 measured best on T3/T4 (see TUNING.md).
        beta: f64 = 20.0,
        /// Weight of the router's congestion map in the placer's cost after a failed route (1 measured best, NIGHT_LOG).
        beta2: f64 = 1.0,
        /// Pin-access weight (plan start 4; 12 measured best).
        gamma: f64 = 12.0,
        /// Direct-insertion bonus per belt inserter a fitting pair saves, in units of one belt tile.
        delta: f64 = 6.0,
        rudy_bin: i32 = 4,
        /// Belt-access demand around machines, added to the RUDY bins: per belt terminal, its inserters plus
        /// access_belt_tiles belt tiles, spread over a band access_range tiles wide around the machine and scaled
        /// by access_demand (0 turns it off). Many-input machines then keep room for the belts that pass them.
        access_demand: f64 = 1.0,
        access_belt_tiles: f64 = 3.0,
        access_range: i32 = 2,
        /// VPR crossing-count correction q(k): (terminal count, factor) points, interpolated linearly.
        q_table: Vec<Vec<f64>> = vec![
            vec![1.0, 1.0], vec![3.0, 1.0], vec![4.0, 1.0828], vec![5.0, 1.1536], vec![6.0, 1.2206],
            vec![7.0, 1.2823], vec![8.0, 1.3385], vec![9.0, 1.3991], vec![10.0, 1.4493], vec![15.0, 1.6899],
            vec![20.0, 1.8924], vec![25.0, 2.0743], vec![30.0, 2.2334], vec![35.0, 2.3895],
            vec![40.0, 2.5356], vec![45.0, 2.6625], vec![50.0, 2.7933],
        ],
        shift_share: f64 = 0.6,
        swap_share: f64 = 0.3,
        t0_factor: f64 = 20.0,
        moves_per_temp_factor: f64 = 10.0,
        stop_factor: f64 = 0.005,
        target_acceptance: f64 = 0.44,
        max_moves: i64 = 2_000_000,
        /// Anneal budget per machine (the budget is the smaller of this times the machine count and max_moves).
        moves_per_machine: i64 = 20_000,
        feedback_rounds: i32 = 3,
        /// A trial whose best routing round ended within this many conflicts goes back to that round's placement
        /// and retries up to near_miss_attempts times: each retry shifts one machine next to a remaining conflict
        /// by a tile or two and routes again with a larger endgame (different shuffles each time).
        near_miss_max: i32 = 4,
        near_miss_attempts: i32 = 4,
        /// Endgame budget multiplier (rounds and time) for near-miss retries.
        near_miss_endgame_factor: f64 = 3.0,
        /// A trial whose best round ended with more conflicts than a near miss but at most spread_max moves the
        /// machines within spread_radius tiles of each conflict cluster one tile outward and routes again, up to
        /// spread_attempts times (each building on the last if it got closer). Off by default (0): it did not raise
        /// success rates and costs time (NIGHT_LOG sweep 4).
        spread_max: i32 = 0,
        spread_attempts: i32 = 3,
        spread_radius: i32 = 3,
        feedback_t0_divisor: f64 = 20.0,
        /// How a net's belt length is estimated: "hpwl" (bounding box of its machines times q(k)) or "path" (the
        /// bounding box plus path_weight times the gaps a greedy path leaves between the machines' inserter rings,
        /// so machines along one corridor cost no more than its length and scattered ones pay for every detour).
        belt_cost: String = "hpwl".to_owned(),
        path_weight: f64 = 1.0,
        /// Starting placement: "random", "columns" (machines of a recipe stacked flush in columns, column_gap tiles
        /// of corridor between columns, refined at a low temperature so the structure stays), or "mixed" (columns on
        /// even seeds, random on odd ones).
        initial: String = "random".to_owned(),
        column_gap: i32 = 3,
        /// Free tiles between machines within a column (0: flush; 1 leaves a lane for belts crossing the column).
        column_spacing: i32 = 1,
        /// Share of annealing moves that shift or swap a whole column of machines (random starts).
        column_share: f64 = 0.1,
        /// From a column start, only moves that keep columns: shift or swap whole columns, swap two machines, slide
        /// a machine up or down its column, rotate a fluid machine in place. False (measured better on chemical
        /// science, equal on job 9) lets single machines leave their column once annealing starts.
        column_moves_only: bool = false,
        /// With column moves only: whether two machines of different columns may swap (mixing the columns' recipes).
        column_swap_across: bool = true,
        /// Starting temperature when refining a column start, as a share of the sampled one.
        column_t0_scale: f64 = 0.05,
        /// Largest shift of the first refinement moves from a column start, in tiles.
        column_start_range: i32 = 3,
    }
}

impl PlaceTuning {
    /// Crossing-count correction q(k), interpolated linearly in the table, extended linearly past its end.
    pub fn q(&self, k: i32) -> f64 {
        let t = &self.q_table;
        let k = f64::from(k);
        if k <= t[0][0] {
            return t[0][1];
        }
        for pair in t.windows(2) {
            let (k0, q0, k1, q1) = (pair[0][0], pair[0][1], pair[1][0], pair[1][1]);
            if k <= k1 {
                return q0 + (q1 - q0) * (k - k0) / (k1 - k0);
            }
        }
        let last = &t[t.len() - 1];
        last[1] + (k - last[0]) * 0.02616
    }
}

config_struct! {
    RouteTuning {
        belt_base: f64 = 1.0,
        turn_penalty: f64 = 0.2,
        ug_penalty: f64 = 2.0,
        ug_penalty_per_tile: f64 = 0.1,
        inserter_base: f64 = 0.5,
        long_inserter_base: f64 = 0.6,
        chest_base: f64 = 0.5,
        pin_reservation: f64 = 1.5,
        /// Two chains share a belt only if their consumer machines overlap at least this much (shared / all).
        lane_share_overlap: f64 = 0.5,
        /// Extra cost of an underground pair of the weaving tier over one of the main tier (B5).
        weave_penalty: f64 = 1.0,
        /// Extra cost of a splitter that starts a belt branch (B9).
        splitter_cost: f64 = 2.0,
        /// While visiting producers, extra cost on the access tiles of the same net's consumers.
        own_consumer_reservation: f64 = 3.0,
        /// Start producers tried per net and iteration before the net counts as unrouted.
        start_attempts: i32 = 3,
        /// Leg undo-and-retry steps per net attempt when the chain gets trapped.
        max_backtracks: i32 = 12,
        /// Nets needing more inserter visits than this are split into several belt chains.
        max_chain_visits: i32 = 12,
        /// A* reruns per leg after forbidding a state whose path crossed itself.
        repair_restarts: i32 = 40,
        dead_end_conflict: f64 = 1.0,
        p_fac_first: f64 = 0.5,
        p_mult: f64 = 1.1,
        /// Cap on the present-congestion factor (VPR's max_pres_fac).
        p_fac_max: f64 = 1000.0,
        h_fac: f64 = 0.5,
        max_iterations: i32 = 120,
        stagnation_stop: i32 = 40,
        full_reroute_iterations: i32 = 5,
        /// Weighted A* factor; 1.0 keeps the heuristic admissible.
        heuristic_weight: f64 = 1.2,
        /// Reroutes first try the chain's previous terminal order.
        keep_chain_order: bool = false,
        /// Log a per-iteration overuse breakdown.
        trace_detail: bool = false,
        /// Allow a belt directly after an underground exit to turn. VERIFY in game.
        turn_after_ug_exit: bool = true,
        /// Endgame (LNS) after negotiation stalls: rounds of ripping up and rerouting everything near the conflicts.
        endgame_rounds: i32 = 30,
        /// Half-width of the square ripped up around each conflict; grows by 2 every 4 rounds.
        endgame_radius: i32 = 3,
        /// Negotiation iterations among the rerouted nets per endgame round.
        endgame_iterations: i32 = 4,
        /// The endgame runs only when negotiation ends this close to a solution.
        endgame_max_overuse: i32 = 150,
        /// ... or within this many conflicts per belt chain, whichever is larger (big blocks have more).
        endgame_overuse_per_chain: f64 = 2.0,
        endgame_max_unrouted: i32 = 4,
        /// Weight of each conflict left at the end in the congestion map handed back to the placer.
        feedback_overuse_weight: f64 = 2.0,
        /// The endgame stops starting rounds after this multiple of the negotiation's own time.
        endgame_time_factor: f64 = 2.0,
        /// A net's flow above this share of a belt is split over several belt chains.
        chain_belt_share: f64 = 0.9,
        /// A chain's flow may use up to this share of the weaving (second) underground tier's throughput.
        weave_belt_share: f64 = 0.98,
        /// How many tile conflicts one unrouted chain counts as when comparing routing iterations and rounds.
        unrouted_penalty: i32 = 10,
        /// Search states one path search (belt or pipe) may expand before it gives up.
        max_expansions: i32 = 3_000_000,
    }
}

config_struct! {
    SearchTuning {
        /// s_lo = sqrt(machine area * factor).
        area_factor: f64 = 1.8,
        warm_t0_divisor: f64 = 10.0,
        /// Extra seed rounds at the smallest feasible size (and the one below) once the interval closes.
        extra_rounds: i32 = 1000,
        /// Until a first success the box grows by this factor (a failure is usually a routing dead end, not a lack of room).
        grow_factor: f64 = 1.25,
        /// Seed rounds at each size before growing the box.
        grow_rounds: i32 = 2,
    }
}
