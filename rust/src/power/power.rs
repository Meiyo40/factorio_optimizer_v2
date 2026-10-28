use crate::data::game_constants::{self as gc, PoleSpec};
use crate::geom::{Kind, PlacedEntity, Rect, Tile};
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct PowerResult {
    pub ok: bool,
    pub poles: Vec<PlacedEntity>,
    /// Copper wires as indices into `poles`.
    pub wires: Vec<(usize, usize)>,
    /// Entities no candidate pole could cover.
    pub uncovered: Vec<PlacedEntity>,
    pub disconnected: bool,
}

/// Does a pole standing at `pole` (top-left) supply `r`? Overlap must be strict: touching edges do not count.
pub fn supplies(spec: &PoleSpec, pole: Tile, r: &Rect) -> bool {
    let cx = f64::from(pole.x) + f64::from(spec.size) / 2.0;
    let cy = f64::from(pole.y) + f64::from(spec.size) / 2.0;
    let s = spec.supply_half;
    f64::from(r.x) < cx + s && f64::from(r.x + r.w) > cx - s && f64::from(r.y) < cy + s && f64::from(r.y + r.h) > cy - s
}

fn center(spec: &PoleSpec, t: Tile) -> (f64, f64) {
    (f64::from(t.x) + f64::from(spec.size) / 2.0, f64::from(t.y) + f64::from(spec.size) / 2.0)
}

fn dist(spec: &PoleSpec, a: Tile, b: Tile) -> f64 {
    let ((ax, ay), (bx, by)) = (center(spec, a), center(spec, b));
    (ax - bx).hypot(ay - by)
}

fn mark_occupied(t: Tile, spec: &PoleSpec, occ: &mut HashSet<Tile>) {
    for dy in 0..spec.size {
        for dx in 0..spec.size {
            occ.insert(Tile::new(t.x + dx, t.y + dy));
        }
    }
}

fn fits(t: Tile, spec: &PoleSpec, occupied: &HashSet<Tile>) -> bool {
    (0..spec.size).all(|dy| (0..spec.size).all(|dx| !occupied.contains(&Tile::new(t.x + dx, t.y + dy))))
}

/// P1: after routing, cover every machine and inserter with poles placed greedily on free tiles (most
/// newly covered entities first, ties to the candidate nearest an existing pole), then wire them with a
/// minimum spanning tree, adding bridge poles along the line between components when wires cannot reach.
pub fn place_poles(entities: &[PlacedEntity], candidates: &[Tile], pole_name: &str, fixed_poles: &[Tile]) -> PowerResult {
    let spec = gc::pole(pole_name).expect("a known pole");
    let mut occupied: HashSet<Tile> = HashSet::new();
    for e in entities {
        occupied.extend(e.rect().tiles());
    }
    let mut seen = HashSet::new();
    let cands: Vec<Tile> = candidates.iter().copied().filter(|&t| fits(t, spec, &occupied) && seen.insert(t)).collect();
    let needs: Vec<&PlacedEntity> = entities.iter().filter(|e| matches!(e.kind, Kind::Machine | Kind::Inserter)).collect();
    let need_rects: Vec<Rect> = needs.iter().map(|e| e.rect()).collect();

    let mut poles: Vec<Tile> = Vec::new();
    for &t in fixed_poles {
        poles.push(t);
        mark_occupied(t, spec, &mut occupied);
    }
    let mut covered: Vec<bool> = need_rects.iter().map(|r| poles.iter().any(|&p| supplies(spec, p, r))).collect();
    let cover_lists: Vec<Vec<usize>> = cands.iter().map(|&c| (0..needs.len()).filter(|&i| supplies(spec, c, &need_rects[i])).collect()).collect();

    while covered.iter().any(|c| !c) {
        let mut best: Option<usize> = None;
        let mut best_gain = 0;
        let mut best_dist = f64::MAX;
        for (ci, &c) in cands.iter().enumerate() {
            if !fits(c, spec, &occupied) {
                continue;
            }
            let gain = cover_lists[ci].iter().filter(|&&i| !covered[i]).count();
            if gain == 0 || gain < best_gain {
                continue;
            }
            let d = poles.iter().map(|&p| dist(spec, c, p)).reduce(f64::min).unwrap_or(0.0);
            if gain > best_gain || d < best_dist {
                best = Some(ci);
                best_gain = gain;
                best_dist = d;
            }
        }
        let Some(ci) = best else { break };
        poles.push(cands[ci]);
        mark_occupied(cands[ci], spec, &mut occupied);
        for &i in &cover_lists[ci] {
            covered[i] = true;
        }
    }
    let uncovered: Vec<PlacedEntity> = (0..needs.len()).filter(|&i| !covered[i]).map(|i| needs[i].clone()).collect();

    // Connect: add bridge poles until one component, then take the MST.
    for _ in 0..200 {
        let comps = components(&poles, spec);
        if comps.iter().copied().max().unwrap_or(0) == 0 {
            break;
        }
        // Nearest pair of poles in different components.
        let (mut bi, mut bj, mut bd) = (usize::MAX, usize::MAX, f64::MAX);
        for i in 0..poles.len() {
            for j in 0..poles.len() {
                if comps[i] == comps[j] || j <= i {
                    continue;
                }
                let d = dist(spec, poles[i], poles[j]);
                if d < bd {
                    bd = d;
                    bi = i;
                    bj = j;
                }
            }
        }
        let free: Vec<Tile> = cands.iter().copied().filter(|&t| fits(t, spec, &occupied)).collect();
        let Some(bridge) = bridge_pole(poles[bi], poles[bj], spec, &free) else { break };
        poles.push(bridge);
        mark_occupied(bridge, spec, &mut occupied);
    }
    let disconnected = components(&poles, spec).iter().copied().max().unwrap_or(0) > 0;
    let wires = mst(&poles, spec);
    let placed: Vec<PlacedEntity> = poles
        .iter()
        .map(|t| PlacedEntity { w: spec.size, h: spec.size, ..PlacedEntity::new(pole_name, Kind::Pole, t.x, t.y, crate::geom::Dir::N) })
        .collect();
    PowerResult { ok: uncovered.is_empty() && !disconnected, poles: placed, wires, uncovered, disconnected }
}

fn reach(spec: &PoleSpec, a: Tile, b: Tile) -> bool {
    dist(spec, a, b) <= spec.wire_reach + 1e-9
}

/// Component label per pole under wire reach.
fn components(poles: &[Tile], spec: &PoleSpec) -> Vec<i32> {
    let mut comp = vec![-1; poles.len()];
    let mut c = 0;
    for s in 0..poles.len() {
        if comp[s] >= 0 {
            continue;
        }
        let mut stack = vec![s];
        comp[s] = c;
        while let Some(i) = stack.pop() {
            for j in 0..poles.len() {
                if comp[j] < 0 && reach(spec, poles[i], poles[j]) {
                    comp[j] = c;
                    stack.push(j);
                }
            }
        }
        c += 1;
    }
    comp
}

/// The free candidate nearest the point one wire reach from `a` towards `b`.
fn bridge_pole(a: Tile, b: Tile, spec: &PoleSpec, cands: &[Tile]) -> Option<Tile> {
    let ((ax, ay), (bx, by)) = (center(spec, a), center(spec, b));
    let d = (bx - ax).hypot(by - ay);
    let step = (d / 2.0).min(spec.wire_reach * 0.9);
    let tx = ax + (bx - ax) / d * step;
    let ty = ay + (by - ay) / d * step;
    let mut best: Option<(Tile, f64)> = None;
    for &c in cands.iter().filter(|&&c| reach(spec, a, c)) {
        let (cx, cy) = center(spec, c);
        let score = (cx - tx).hypot(cy - ty) + 0.5 * (cx - bx).hypot(cy - by);
        if best.is_none_or(|(_, s)| score < s) {
            best = Some((c, score)); // the first of the lowest, like minByOrNull
        }
    }
    best.map(|(t, _)| t)
}

/// Prim's minimum spanning tree over poles within wire reach.
fn mst(poles: &[Tile], spec: &PoleSpec) -> Vec<(usize, usize)> {
    if poles.len() < 2 {
        return Vec::new();
    }
    let n = poles.len();
    let mut in_tree = vec![false; n];
    let mut best = vec![f64::MAX; n];
    let mut from: Vec<Option<usize>> = vec![None; n];
    let mut edges = Vec::new();
    // Run Prim from every unreached pole so disconnected parts still get their own trees.
    for root in 0..n {
        if in_tree[root] {
            continue;
        }
        best[root] = 0.0;
        loop {
            let mut u: Option<usize> = None;
            for i in 0..n {
                if !in_tree[i] && best[i] < f64::MAX && u.is_none_or(|u| best[i] < best[u]) {
                    u = Some(i);
                }
            }
            let Some(u) = u else { break };
            in_tree[u] = true;
            if let Some(f) = from[u] {
                edges.push((f, u));
            }
            for v in 0..n {
                if in_tree[v] || !reach(spec, poles[u], poles[v]) {
                    continue;
                }
                let d = dist(spec, poles[u], poles[v]);
                if d < best[v] {
                    best[v] = d;
                    from[v] = Some(u);
                }
            }
        }
    }
    edges
}

/// P2: pole positions on a fixed lattice, reserved as obstacles before placement. Medium poles every 7
/// tiles (7x7 supply), substations every 18; the lattice is offset so supply areas tile the box.
pub fn pole_lattice(w: i32, h: i32, pole_name: &str) -> Vec<Tile> {
    let spec = gc::pole(pole_name).expect("a known pole");
    let pitch = ((2.0 * spec.supply_half) as i32).min(spec.wire_reach as i32);
    let off = (pitch - spec.size) / 2;
    let mut out: Vec<Tile> = Vec::new();
    let mut y = off;
    while y < h + pitch / 2 {
        let mut x = off;
        while x < w + pitch / 2 {
            let t = Tile::new(x.min(w - spec.size), y.min(h - spec.size));
            if !out.contains(&t) {
                out.push(t);
            }
            x += pitch;
        }
        y += pitch;
    }
    out
}

/// Free candidate tiles for poles: the box, plus a 1-tile ring around it when allowed, plus `extra` tiles.
pub fn pole_candidates(w: i32, h: i32, ring: bool, extra: &[Tile]) -> Vec<Tile> {
    let r = i32::from(ring);
    let mut out = Vec::new();
    for y in -r..h + r {
        for x in -r..w + r {
            out.push(Tile::new(x, y));
        }
    }
    out.extend_from_slice(extra);
    out
}
