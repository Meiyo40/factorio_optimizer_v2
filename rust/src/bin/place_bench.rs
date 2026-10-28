//! Times the annealer on the cases the Kotlin benchmark (rust/tools/kotlin/GoldenPlaceTest.kt) wrote, with the
//! same protocol: a fresh random placement, only the anneal timed, warm-up runs first, the median of the timed
//! runs, then `threads` anneals at once for total throughput. Reads bench_kotlin.json beside the cases, when it
//! is there, to print the comparison and to check that both sides did the same work.
//!
//!   place_bench --dir <dir with bench_cases.json and plans/> [--warm 3] [--reps 7] [--threads 8]

use anyhow::{Context, Result};
use factorio_optimizer::config::run_configuration::RunConfiguration;
use factorio_optimizer::config::tuning::{PlaceTuning, Tuning};
use factorio_optimizer::geom::Tile;
use factorio_optimizer::place::{AnnealOptions, AnnealStats, Annealer, PlaceModel, static_obstacles};
use factorio_optimizer::plan::ProductionPlan;
use factorio_optimizer::util::splittable_random::SplittableRandom;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Deserialize)]
struct Case {
    case: String,
    job: String,
    w: i32,
    h: i32,
    seed: i64,
    max_moves: i64,
    plan: String,
}

#[derive(Deserialize)]
struct Cases {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct KotlinReport {
    cases: Vec<KotlinCase>,
}

#[derive(Deserialize)]
struct KotlinCase {
    case: String,
    moves: i64,
    temperatures: i32,
    final_cost: f64,
    single_moves_per_s: f64,
    parallel_moves_per_s: f64,
}

#[derive(Serialize)]
struct Row {
    case: String,
    machines: usize,
    moves: i64,
    temperatures: i32,
    final_cost: f64,
    single_moves_per_s: f64,
    parallel_threads: usize,
    parallel_moves_per_s: f64,
}

struct Loaded {
    case: Case,
    plan: ProductionPlan,
    config: RunConfiguration,
    obstacles: HashSet<Tile>,
}

/// One anneal from a fresh random placement; only the anneal is timed. Returns (nanoseconds, stats, final cost).
fn once(l: &Loaded, tun: &PlaceTuning, seed: i64) -> (u64, AnnealStats, f64) {
    let model = PlaceModel::new(&l.plan, &l.config, tun, l.case.w, l.case.h, &l.obstacles).expect("a valid placement model");
    let mut a = Annealer::new(model, SplittableRandom::new(seed));
    assert!(a.initial_random() || a.initial_rows(&l.plan), "{}: no initial placement", l.case.case);
    let t = Instant::now();
    let stats = a.anneal(AnnealOptions { max_moves: Some(l.case.max_moves), ..Default::default() });
    (t.elapsed().as_nanos() as u64, stats, a.model.cost())
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).expect("no NaN timings"));
    v[v.len() / 2]
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let dir = PathBuf::from(arg(&args, "--dir").context("--dir <benchmark directory> is required")?);
    let warm: usize = arg(&args, "--warm").map_or(3, |v| v.parse().expect("--warm is a number"));
    let reps: usize = arg(&args, "--reps").map_or(7, |v| v.parse().expect("--reps is a number"));
    let threads: usize = arg(&args, "--threads").map_or(8, |v| v.parse().expect("--threads is a number"));
    let only = arg(&args, "--only");
    let max_moves: Option<i64> = arg(&args, "--max-moves").map(|v| v.parse().expect("--max-moves is a number"));
    let tun = Tuning::defaults().place;
    let mut cases: Cases = serde_json::from_str(&std::fs::read_to_string(dir.join("bench_cases.json"))?)?;
    cases.cases.retain(|c| only.as_ref().is_none_or(|o| *o == c.case));
    for c in &mut cases.cases {
        c.max_moves = max_moves.unwrap_or(c.max_moves);
    }
    let kotlin: Option<KotlinReport> =
        std::fs::read_to_string(dir.join("bench_kotlin.json")).ok().map(|t| serde_json::from_str(&t)).transpose()?;
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("the crate lives in the repo");

    let mut rows = Vec::new();
    println!(
        "{:<18} {:>8} {:>9} {:>14} {:>14} {:>7}   {:>14} {:>14} {:>7}   same work",
        "case", "machines", "moves", "jvm 1 thread", "rust 1 thread", "ratio", format!("jvm x{threads}"), format!("rust x{threads}"), "ratio"
    );
    for case in cases.cases {
        let plan: ProductionPlan = serde_json::from_str(&std::fs::read_to_string(dir.join(&case.plan))?)?;
        let config = RunConfiguration::load(repo.join(&case.job).to_str().expect("a UTF-8 path"))?;
        let obstacles = static_obstacles(&config, &plan, case.w, case.h);
        let l = Loaded { case, plan, config, obstacles };

        for _ in 0..warm {
            once(&l, &tun, l.case.seed);
        }
        let runs: Vec<_> = (0..reps).map(|_| once(&l, &tun, l.case.seed)).collect();
        let stats = runs[0].1;
        let single = median(runs.iter().map(|r| stats.moves as f64 * 1e9 / r.0 as f64).collect());

        // --threads 0 skips the all-cores measurement (for profiling a single anneal).
        let parallel = if threads == 0 {
            0.0
        } else {
            median(
            (0..reps)
                .map(|_| {
                    let t = Instant::now();
                    let (l, tun) = (&l, &tun);
                    let moves: i64 = std::thread::scope(|s| {
                        let handles: Vec<_> = (0..threads).map(|i| s.spawn(move || once(l, tun, l.case.seed + i as i64).1.moves)).collect();
                        handles.into_iter().map(|h| h.join().expect("an anneal thread panicked")).sum()
                    });
                    moves as f64 * 1e9 / t.elapsed().as_nanos() as f64
                })
                .collect(),
            )
        };

        let jvm = kotlin.as_ref().and_then(|k| k.cases.iter().find(|c| c.case == l.case.case));
        let same = jvm.map(|k| {
            let ok = k.moves == stats.moves && k.temperatures == stats.temperatures && (k.final_cost - runs[0].2).abs() <= 1e-9 * k.final_cost.abs().max(1.0);
            if ok { "yes" } else { "NO" }
        });
        println!(
            "{:<18} {:>8} {:>9} {:>14} {:>14.0} {:>7}   {:>14} {:>14.0} {:>7}   {}",
            l.case.case,
            l.plan.machines.len(),
            stats.moves,
            jvm.map_or("-".to_owned(), |k| format!("{:.0}", k.single_moves_per_s)),
            single,
            jvm.map_or("-".to_owned(), |k| format!("{:.2}x", single / k.single_moves_per_s)),
            jvm.map_or("-".to_owned(), |k| format!("{:.0}", k.parallel_moves_per_s)),
            parallel,
            jvm.map_or("-".to_owned(), |k| format!("{:.2}x", parallel / k.parallel_moves_per_s)),
            same.unwrap_or("-"),
        );
        rows.push(Row {
            case: l.case.case.clone(),
            machines: l.plan.machines.len(),
            moves: stats.moves,
            temperatures: stats.temperatures,
            final_cost: runs[0].2,
            single_moves_per_s: single,
            parallel_threads: threads,
            parallel_moves_per_s: parallel,
        });
    }
    std::fs::write(dir.join("bench_rust.json"), serde_json::to_string_pretty(&rows)?)?;
    Ok(())
}
