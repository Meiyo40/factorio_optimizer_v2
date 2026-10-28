use super::place_model::PlaceModel;
use super::placement::Placement;
use crate::plan::ProductionPlan;
use crate::util::splittable_random::SplittableRandom;
use indexmap::IndexMap;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnnealStats {
    pub initial_cost: f64,
    pub final_cost: f64,
    pub t0: f64,
    pub moves: i64,
    pub temperatures: i32,
}

/// The arguments of `Annealer.anneal` in Kotlin.
#[derive(Debug, Clone, Copy)]
pub struct AnnealOptions {
    /// None: 20 x the std of the cost change over N random moves (accepted unconditionally).
    pub t0: Option<f64>,
    /// None: the tuning's `max_moves`.
    pub max_moves: Option<i64>,
    pub start_range: Option<i32>,
    /// Refine the current placement: the sampled starting temperature is scaled by `t0_scale` and the sampling
    /// moves are taken back, so a structured start (columns) is kept.
    pub refine: bool,
    pub t0_scale: f64,
}

impl Default for AnnealOptions {
    fn default() -> Self {
        Self { t0: None, max_moves: None, start_range: None, refine: false, t0_scale: 1.0 }
    }
}

/// Stage 2: simulated annealing over machine positions (VPR schedule). Every random choice comes from `rnd`.
/// The annealer owns the model; callers reach it as `annealer.model`.
pub struct Annealer<'a> {
    pub model: PlaceModel<'a>,
    rnd: SplittableRandom,
    n_m: usize,
    /// The machines of the proposed move, and where they go (nx, ny, nr) and came from (ox, oy, or_).
    cur: Vec<usize>,
    nx: Vec<i32>,
    ny: Vec<i32>,
    nr: Vec<i32>,
    ox: Vec<i32>,
    oy: Vec<i32>,
    or_: Vec<i32>,
    /// Only moves that keep columns (set by a column start with `column_moves_only`).
    structured: bool,
    /// Column of each machine from a column start (-1: none); a swap trades the two machines' columns.
    col_of: Vec<i32>,
    swap_cols: bool,
    fluid_machines: Vec<usize>,
    can_swap: bool,
    /// Machines with the same recipe have the same id, so the swap test is an integer comparison.
    recipe_id: Vec<usize>,
    pub last_t0: f64,
}

impl<'a> Annealer<'a> {
    pub fn new(model: PlaceModel<'a>, rnd: SplittableRandom) -> Self {
        let n_m = model.n_m;
        let mut recipes: IndexMap<&str, usize> = IndexMap::new();
        let recipe_id: Vec<usize> = model
            .plan
            .machines
            .iter()
            .map(|m| {
                let next = recipes.len();
                *recipes.entry(m.recipe.as_str()).or_insert(next)
            })
            .collect();
        let fluid_machines = (0..n_m).filter(|&m| model.fluid[m]).collect();
        Self {
            rnd,
            n_m,
            cur: Vec::new(),
            nx: vec![0; n_m],
            ny: vec![0; n_m],
            nr: vec![0; n_m],
            ox: vec![0; n_m],
            oy: vec![0; n_m],
            or_: vec![0; n_m],
            structured: false,
            col_of: vec![-1; n_m],
            swap_cols: false,
            fluid_machines,
            can_swap: recipes.len() > 1,
            recipe_id,
            last_t0: 0.0,
            model,
        }
    }

    pub fn structured(&self) -> bool {
        self.structured
    }

    fn legal1(&mut self, m: usize, x: i32, y: i32, r: i32) -> bool {
        self.model.legal(&[m], &[x], &[y], &[r])
    }

    // ---------------- initial placement ----------------
    /// Random legal positions; false if some machine finds no spot.
    pub fn initial_random(&mut self) -> bool {
        self.initial_random_tries(500)
    }

    pub fn initial_random_tries(&mut self, tries: i32) -> bool {
        self.model.clear();
        self.structured = false;
        self.col_of.fill(-1);
        let mut order: Vec<usize> = (0..self.n_m).collect();
        self.shuffle(&mut order);
        for m in order {
            let mut ok = false;
            for _ in 0..tries {
                let x = self.rnd.next_int_range(0, 1.max(self.model.w - self.model.mw[m] + 1));
                let y = self.rnd.next_int_range(0, 1.max(self.model.h - self.model.mh[m] + 1));
                let r = if self.model.fluid[m] { self.rnd.next_int_bound(8) } else { 0 };
                if self.legal1(m, x, y, r) {
                    self.model.place_one(m, x, y, r);
                    ok = true;
                    break;
                }
            }
            if !ok {
                return false;
            }
        }
        self.model.finish_placing();
        true
    }

    /// Row by row in topological order of the recipe graph, one free tile between machines.
    pub fn initial_rows(&mut self, plan: &ProductionPlan) -> bool {
        self.model.clear();
        self.structured = false;
        self.col_of.fill(-1);
        let depth = recipe_depth(plan);
        let mut order: Vec<usize> = (0..self.n_m).collect();
        order.sort_by_key(|&m| (depth.get(plan.machines[m].product.as_str()).copied().unwrap_or(0), m));
        let (mut x, mut y, mut row_h) = (0, 0, 0);
        for m in order {
            let (mw, mh, fluid) = (self.model.mw[m], self.model.mh[m], self.model.fluid[m]);
            let mut placed_here = false;
            while !placed_here {
                if x + mw > self.model.w {
                    x = 0;
                    y += row_h + 1;
                    row_h = 0;
                }
                if y + mh > self.model.h {
                    return false;
                }
                for r in 0..8 {
                    if !fluid && r > 0 {
                        break;
                    }
                    if self.legal1(m, x, y, r) {
                        self.model.place_one(m, x, y, r);
                        placed_here = true;
                        break;
                    }
                }
                if placed_here {
                    row_h = row_h.max(mh + i32::from(fluid));
                    x += mw + 1;
                } else {
                    x += 1;
                }
            }
        }
        self.model.finish_placing();
        true
    }

    /// Columns: machines grouped by recipe (upstream recipes first), stacked from the top of the box with
    /// `column_spacing` free tiles between them, a new column when the box height is used up (a recipe
    /// never shares a column with one of another machine width).
    /// The corridor before a column is wide enough for an inserter on each side and about half the belts of the
    /// columns on both sides (each machine draws its items from two sides), and at least `column_gap`. A machine
    /// that does not fit (fluid stubs, obstacles) slides down. False if the columns run out of width or height.
    pub fn initial_columns(&mut self, plan: &ProductionPlan) -> bool {
        let spacing = self.model.tun.column_spacing;
        // Tighter and tighter until it fits: as configured, then without lanes, then narrow corridors with machines
        // of different widths sharing a column.
        self.try_columns(plan, spacing, true, false)
            || (spacing > 0 && self.try_columns(plan, 0, true, false))
            || self.try_columns(plan, 0, false, true)
    }

    fn try_columns(&mut self, plan: &ProductionPlan, spacing: i32, item_corridors: bool, mix_widths: bool) -> bool {
        self.model.clear();
        self.structured = false;
        self.col_of.fill(-1);
        let depth_of = {
            let depth = recipe_depth(plan);
            move |m: usize| depth.get(plan.machines[m].product.as_str()).copied().unwrap_or(0)
        };
        let mut groups: IndexMap<&str, Vec<usize>> = IndexMap::new();
        for m in 0..self.n_m {
            groups.entry(plan.machines[m].recipe.as_str()).or_default().push(m);
        }
        let mut groups: Vec<Vec<usize>> = groups.into_values().collect();
        groups.sort_by(|a, b| {
            depth_of(a[0])
                .cmp(&depth_of(b[0]))
                .then(b.len().cmp(&a.len()))
                .then_with(|| plan.machines[a[0]].recipe.cmp(&plan.machines[b[0]].recipe))
        });
        // Items each machine moves by inserter (inputs, fuel and item products).
        let items: Vec<usize> = plan.machines.iter().map(|m| m.demand.len() + m.item_products().count()).collect();
        // 1. Columns by height (a fluid machine counts a spare tile for its pipe stubs).
        let mut cols: Vec<Vec<usize>> = Vec::new();
        let (mut used, mut col_w) = (self.model.h, 0);
        for &m in groups.iter().flatten() {
            let need = self.model.mh[m] + spacing + i32::from(self.model.fluid[m]);
            if used + need > self.model.h || (!mix_widths && self.model.mw[m] != col_w) {
                cols.push(Vec::new());
                used = 0;
                col_w = self.model.mw[m];
            }
            cols.last_mut().expect("a column was just opened").push(m);
            used += need;
        }
        // 2. Corridors and placement.
        let busy: Vec<usize> = cols.iter().map(|c| c.iter().map(|&m| items[m]).max().expect("a column has machines")).collect();
        let column_gap = self.model.tun.column_gap;
        let corridor = |i: usize| -> i32 {
            if !item_corridors {
                return column_gap;
            }
            let left = if i > 0 { busy[i - 1] } else { 0 };
            column_gap.max(1 + (left + busy[i]).div_ceil(2) as i32)
        };
        let mut x = 0;
        for (i, c) in cols.iter().enumerate() {
            for &m in c {
                self.col_of[m] = i as i32;
            }
            x += corridor(i);
            let mut y = 0;
            for &m in c {
                let (mw, mh, fluid) = (self.model.mw[m], self.model.mh[m], self.model.fluid[m]);
                let mut placed_here = false;
                while !placed_here {
                    if x + mw > self.model.w || y + mh > self.model.h {
                        self.model.clear();
                        self.col_of.fill(-1);
                        return false;
                    }
                    for r in 0..8 {
                        if !fluid && r > 0 {
                            break;
                        }
                        if self.legal1(m, x, y, r) {
                            self.model.place_one(m, x, y, r);
                            placed_here = true;
                            break;
                        }
                    }
                    if placed_here {
                        y += mh + spacing;
                    } else {
                        y += 1;
                    }
                }
            }
            x += c.iter().map(|&m| self.model.mw[m]).max().expect("a column has machines");
        }
        self.model.finish_placing();
        self.structured = self.model.tun.column_moves_only;
        true
    }

    /// Starts from `p` if it is legal for this model; false otherwise.
    pub fn initial_from(&mut self, p: &Placement) -> bool {
        let mut codes: Vec<[i32; 3]> = vec![[0; 3]; self.n_m];
        let mut ids: Vec<usize> = Vec::with_capacity(p.machines.len());
        for pm in &p.machines {
            codes[pm.id] = [pm.x, pm.y, pm.rot.ordinal() as i32 + if pm.mirror { 4 } else { 0 }];
            ids.push(pm.id);
        }
        ids.sort_unstable();
        self.restore(&ids, &codes)
    }

    /// Re-places the machines `ids` (ascending) at (x, y, orientation code) from `snapshot`.
    fn restore(&mut self, ids: &[usize], snapshot: &[[i32; 3]]) -> bool {
        self.model.clear();
        for &id in ids {
            let [x, y, code] = snapshot[id];
            if !self.legal1(id, x, y, code) {
                self.model.clear();
                return false;
            }
            self.model.place_one(id, x, y, code);
        }
        self.model.finish_placing();
        true
    }

    // ---------------- moves ----------------
    /// Proposes a legal move into the scratch arrays; false if the proposal is illegal.
    fn propose(&mut self, range: i32) -> bool {
        self.cur.clear();
        self.swap_cols = false;
        let tun = self.model.tun;
        if self.structured {
            let u = self.rnd.next_double();
            return if u < 0.3 {
                self.shift_column(range)
            } else if u < 0.4 {
                self.swap_columns()
            } else if u < 0.7 && self.can_swap {
                if tun.column_swap_across { self.swap() } else { self.swap_in_column() }
            } else if u < 0.9 || self.fluid_machines.is_empty() {
                self.slide(range)
            } else {
                self.rotate()
            };
        }
        if tun.column_share > 0.0 && self.rnd.next_double() < tun.column_share {
            return if self.rnd.next_boolean() { self.shift_column(range) } else { self.swap_columns() };
        }
        let u = self.rnd.next_double();
        if u < tun.shift_share || (!self.can_swap && self.fluid_machines.is_empty()) {
            self.shift(range)
        } else if u < tun.shift_share + tun.swap_share && self.can_swap {
            self.swap()
        } else if !self.fluid_machines.is_empty() {
            self.rotate()
        } else {
            self.shift(range)
        }
    }

    fn legal_move(&mut self) -> bool {
        self.model.legal(&self.cur, &self.nx, &self.ny, &self.nr)
    }

    fn random_machine(&mut self) -> usize {
        self.rnd.next_int_bound(self.n_m as i32) as usize
    }

    fn shift(&mut self, range: i32) -> bool {
        let m = self.random_machine();
        let (mut dx, mut dy);
        loop {
            dx = self.rnd.next_int_range(-range, range + 1);
            dy = self.rnd.next_int_range(-range, range + 1);
            if !(dx == 0 && dy == 0) {
                break;
            }
        }
        self.cur.push(m);
        self.nx[0] = self.model.px[m] + dx;
        self.ny[0] = self.model.py[m] + dy;
        self.nr[0] = self.model.rot[m];
        self.legal_move()
    }

    fn swap(&mut self) -> bool {
        let a = self.random_machine();
        let mut guard = 0;
        let mut b;
        loop {
            b = self.random_machine();
            guard += 1;
            if !((b == a || self.recipe_id[a] == self.recipe_id[b]) && guard < 50) {
                break;
            }
        }
        if b == a || self.recipe_id[a] == self.recipe_id[b] {
            return false;
        }
        self.cur.extend([a, b]);
        self.swap_cols = self.col_of[a] != self.col_of[b];
        self.set_swap(a, b);
        self.legal_move()
    }

    /// The machines `a` and `b` trade positions, each keeping its own orientation.
    fn set_swap(&mut self, a: usize, b: usize) {
        let m = &self.model;
        (self.nx[0], self.ny[0], self.nr[0]) = (m.px[b], m.py[b], m.rot[a]);
        (self.nx[1], self.ny[1], self.nr[1]) = (m.px[a], m.py[a], m.rot[b]);
    }

    /// The column machine `m` stands in: its column from the column start, else the machines with the same left
    /// edge and width whose vertical extents chain together with at most one free tile between neighbours.
    fn column_of(&self, m: usize) -> Vec<usize> {
        let model = &self.model;
        if self.col_of[m] >= 0 {
            return (0..self.n_m).filter(|&i| self.col_of[i] == self.col_of[m]).collect();
        }
        let (x, wd) = (model.px[m], model.mw[m]);
        let mut same: Vec<usize> = (0..self.n_m).filter(|&i| model.px[i] == x && model.mw[i] == wd).collect();
        same.sort_by_key(|&i| model.py[i]);
        let i = same.iter().position(|&s| s == m).expect("the machine is in its own column");
        let (mut lo, mut hi) = (i, i);
        while lo > 0 && model.py[same[lo]] - (model.py[same[lo - 1]] + model.mh[same[lo - 1]]) <= 1 {
            lo -= 1;
        }
        while hi < same.len() - 1 && model.py[same[hi + 1]] - (model.py[same[hi]] + model.mh[same[hi]]) <= 1 {
            hi += 1;
        }
        same[lo..=hi].to_vec()
    }

    fn shift_column(&mut self, range: i32) -> bool {
        let pick = self.random_machine();
        let col = self.column_of(pick);
        let (mut dx, mut dy);
        loop {
            dx = self.rnd.next_int_range(-range, range + 1);
            dy = self.rnd.next_int_range(-1, 2);
            if !(dx == 0 && dy == 0) {
                break;
            }
        }
        for (i, &m) in col.iter().enumerate() {
            self.nx[i] = self.model.px[m] + dx;
            self.ny[i] = self.model.py[m] + dy;
            self.nr[i] = self.model.rot[m];
        }
        self.cur = col;
        self.legal_move()
    }

    /// Two columns of equal width trade places (each keeps its own top).
    fn swap_columns(&mut self) -> bool {
        let first = self.random_machine();
        let a = self.column_of(first);
        let second = self.random_machine();
        let b = self.column_of(second);
        let model = &self.model;
        if a == b || model.mw[a[0]] != model.mw[b[0]] || a.iter().any(|m| b.contains(m)) {
            return false;
        }
        let min_of = |col: &[usize], v: &[i32]| col.iter().map(|&m| v[m]).min().expect("a column has machines");
        let (ax, bx) = (min_of(&a, &model.px), min_of(&b, &model.px));
        let (ay, by) = (min_of(&a, &model.py), min_of(&b, &model.py));
        for (i, &m) in a.iter().enumerate() {
            self.nx[i] = model.px[m] - ax + bx;
            self.ny[i] = model.py[m] - ay + by;
            self.nr[i] = model.rot[m];
        }
        for (j, &m) in b.iter().enumerate() {
            let i = a.len() + j;
            self.nx[i] = model.px[m] - bx + ax;
            self.ny[i] = model.py[m] - by + ay;
            self.nr[i] = model.rot[m];
        }
        self.cur = a;
        self.cur.extend(b);
        self.legal_move()
    }

    /// Two machines of one column trade places (the column keeps its recipes, their order changes).
    fn swap_in_column(&mut self) -> bool {
        let a = self.random_machine();
        if self.col_of[a] < 0 {
            return self.swap();
        }
        let mates: Vec<usize> =
            (0..self.n_m).filter(|&i| i != a && self.col_of[i] == self.col_of[a] && self.recipe_id[i] != self.recipe_id[a]).collect();
        if mates.is_empty() {
            return false;
        }
        let b = mates[self.rnd.next_int_bound(mates.len() as i32) as usize];
        self.cur.extend([a, b]);
        self.set_swap(a, b);
        self.legal_move()
    }

    /// One machine up or down its column.
    fn slide(&mut self, range: i32) -> bool {
        let m = self.random_machine();
        let mut dy;
        loop {
            dy = self.rnd.next_int_range(-range, range + 1);
            if dy != 0 {
                break;
            }
        }
        self.cur.push(m);
        self.nx[0] = self.model.px[m];
        self.ny[0] = self.model.py[m] + dy;
        self.nr[0] = self.model.rot[m];
        self.legal_move()
    }

    fn rotate(&mut self) -> bool {
        let m = self.fluid_machines[self.rnd.next_int_bound(self.fluid_machines.len() as i32) as usize];
        self.cur.push(m);
        self.nx[0] = self.model.px[m];
        self.ny[0] = self.model.py[m];
        self.nr[0] = (self.model.rot[m] + 1 + self.rnd.next_int_bound(7)) % 8;
        self.legal_move()
    }

    fn apply(&mut self) {
        for (i, &m) in self.cur.iter().enumerate() {
            self.ox[i] = self.model.px[m];
            self.oy[i] = self.model.py[m];
            self.or_[i] = self.model.rot[m];
        }
        self.model.move_machines(&self.cur, &self.nx, &self.ny, &self.nr);
        if self.swap_cols {
            self.trade_columns();
        }
    }

    fn revert(&mut self) {
        self.model.move_machines(&self.cur, &self.ox, &self.oy, &self.or_);
        if self.swap_cols {
            self.trade_columns();
        }
    }

    fn trade_columns(&mut self) {
        self.col_of.swap(self.cur[0], self.cur[1]);
    }

    fn snapshot(&self, into: &mut [[i32; 3]]) {
        for (m, s) in into.iter_mut().enumerate() {
            *s = [self.model.px[m], self.model.py[m], self.model.rot[m]];
        }
    }

    // ---------------- schedule ----------------
    /// Anneals from the current placement. The model ends at the best placement seen.
    pub fn anneal(&mut self, o: AnnealOptions) -> AnnealStats {
        let tun = self.model.tun;
        let initial = self.model.cost();
        let mut moves: i64 = 0;
        let max_r = self.model.w.max(self.model.h);
        let mut range = o.start_range.unwrap_or(max_r).clamp(1, max_r);
        let want = 10.max(self.n_m);
        let mut temp = match o.t0 {
            Some(t) => t,
            None => {
                let mut deltas: Vec<f64> = Vec::new();
                let mut guard = 0;
                while deltas.len() < want && {
                    guard += 1;
                    guard - 1 < 100 * want
                } {
                    if !self.propose(if o.refine { range } else { max_r }) {
                        continue;
                    }
                    let before = self.model.cost();
                    self.apply();
                    moves += 1;
                    deltas.push(self.model.cost() - before);
                    if o.refine {
                        self.revert();
                    }
                }
                let mean = deltas.iter().sum::<f64>() / deltas.len() as f64;
                let sd = (deltas.iter().map(|d| (d - mean) * (d - mean)).sum::<f64>() / 1.max(deltas.len() as i64 - 1) as f64).sqrt();
                tun.t0_factor * sd.max(1e-6) * o.t0_scale
            }
        };
        self.last_t0 = temp;
        let per_temp = ((tun.moves_per_temp_factor * (self.n_m as f64).powf(4.0 / 3.0)) as i32).max(50);
        let mut best = self.model.cost();
        let ids: Vec<usize> = (0..self.n_m).collect();
        let mut best_p = vec![[0; 3]; self.n_m];
        self.snapshot(&mut best_p);
        let mut best_cols = self.col_of.clone();
        let mut temps = 0;
        let nets = self.model.net_count().max(1) as f64;
        let max_moves = o.max_moves.unwrap_or(tun.max_moves);
        while moves < max_moves {
            let (mut accepted, mut tried) = (0, 0);
            for _ in 0..per_temp {
                tried += 1;
                if !self.propose(range) {
                    continue;
                }
                let before = self.model.cost();
                self.apply();
                moves += 1;
                let d = self.model.cost() - before;
                if d <= 0.0 || self.rnd.next_double() < (-d / temp).exp() {
                    accepted += 1;
                    if self.model.cost() < best - 1e-9 {
                        best = self.model.cost();
                        self.snapshot(&mut best_p);
                        best_cols.copy_from_slice(&self.col_of);
                    }
                } else {
                    self.revert();
                }
            }
            temps += 1;
            let a = f64::from(accepted) / f64::from(1.max(tried));
            temp *= if a > 0.96 {
                0.5
            } else if a > 0.8 {
                0.9
            } else if a > 0.15 {
                0.95
            } else {
                0.8
            };
            range = ((f64::from(range) * (1.0 - tun.target_acceptance + a)) as i32).clamp(1, max_r);
            if temp < tun.stop_factor * self.model.cost().max(1.0) / nets {
                break;
            }
        }
        // Greedy quench at T = 0.
        for _ in 0..per_temp {
            if !self.propose(1) {
                continue;
            }
            let before = self.model.cost();
            self.apply();
            moves += 1;
            if self.model.cost() > before {
                self.revert();
            } else if self.model.cost() < best - 1e-9 {
                best = self.model.cost();
                self.snapshot(&mut best_p);
                best_cols.copy_from_slice(&self.col_of);
            }
        }
        if self.model.cost() > best + 1e-9 {
            self.restore(&ids, &best_p);
            self.col_of.copy_from_slice(&best_cols);
        }
        AnnealStats { initial_cost: initial, final_cost: self.model.cost(), t0: self.last_t0, moves, temperatures: temps }
    }

    fn shuffle(&mut self, list: &mut [usize]) {
        for i in (0..list.len()).rev() {
            let j = self.rnd.next_int_bound(i as i32 + 1) as usize;
            list.swap(i, j);
        }
    }
}

/// Depth of each item in the recipe graph: raw-fed items first.
pub fn recipe_depth(plan: &ProductionPlan) -> HashMap<String, i32> {
    fn depth(item: &str, guard: i32, plan: &ProductionPlan, by_product: &HashMap<&str, usize>, memo: &mut HashMap<String, i32>) -> i32 {
        if let Some(&d) = memo.get(item) {
            return d;
        }
        let Some(&first) = by_product.get(item) else { return 0 };
        if guard > 50 {
            return 0;
        }
        let deepest = plan.machines[first].demand.keys().map(|k| depth(k, guard + 1, plan, by_product, memo)).max().unwrap_or(0);
        let d = 1 + deepest;
        memo.insert(item.to_owned(), d);
        d
    }
    // The first machine making each product stands for all of them.
    let mut by_product: HashMap<&str, usize> = HashMap::new();
    for m in &plan.machines {
        by_product.entry(m.product.as_str()).or_insert(m.id);
    }
    let mut memo = HashMap::new();
    for m in &plan.machines {
        depth(&m.product, 0, plan, &by_product, &mut memo);
    }
    memo
}
