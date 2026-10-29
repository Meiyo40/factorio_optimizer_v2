//! Times the router on the cases the Kotlin benchmark (rust/tools/kotlin/GoldenRouteTest.kt, `bench`) wrote, with the
//! same protocol: a fixed plan and placement, building the router and running it timed together, warm-up runs first,
//! the median of the timed runs. Reads bench_kotlin.json beside the cases, when it is there, to print the comparison
//! and to check that both sides did the same work (same iterations, A* expansions and entities).
//!
//!   route_bench --dir <dir with bench_cases.json and plans/ and configs/> [--warm 3] [--reps 5] [--only <case>]

use anyhow::{Context, Result};
use factorio_optimizer::config::run_configuration::RunConfiguration;
use factorio_optimizer::config::tuning::RouteTuning;
use factorio_optimizer::geom::Dir;
use factorio_optimizer::place::{PlacedMachine, Placement, static_obstacles};
use factorio_optimizer::plan::ProductionPlan;
use factorio_optimizer::route::{PathFinder, RouteProblem};
use serde::Deserialize;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Deserialize)]
struct PlacedJson {
    id: usize,
    x: i32,
    y: i32,
    rot: usize,
    mirror: bool,
}

#[derive(Deserialize)]
struct Case {
    case: String,
    config: String,
    plan: String,
    w: i32,
    h: i32,
    placement: Vec<PlacedJson>,
    tuning: RouteTuning,
}

#[derive(Deserialize)]
struct Cases {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct KotlinCase {
    case: String,
    machines: usize,
    success: bool,
    iterations: i32,
    expansions: u64,
    entities: usize,
    median_ms: f64,
}

#[derive(Deserialize)]
struct KotlinReport {
    cases: Vec<KotlinCase>,
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let dir = PathBuf::from(arg(&args, "--dir").context("--dir <benchmark directory> is required")?);
    let warm: usize = arg(&args, "--warm").map_or(3, |v| v.parse().expect("--warm is a number"));
    let reps: usize = arg(&args, "--reps").map_or(5, |v| v.parse().expect("--reps is a number"));
    let only = arg(&args, "--only");
    let mut cases: Cases = serde_json::from_str(&std::fs::read_to_string(dir.join("bench_cases.json"))?)?;
    cases.cases.retain(|c| only.as_ref().is_none_or(|o| *o == c.case));
    let kotlin: Option<KotlinReport> = std::fs::read_to_string(dir.join("bench_kotlin.json")).ok().map(|t| serde_json::from_str(&t)).transpose()?;

    println!(
        "{:<22} {:>8} {:>5} {:>11} {:>11} {:>11} {:>7} {:>12}   same work",
        "case", "machines", "iter", "expansions", "jvm ms", "rust ms", "ratio", "rust Mexp/s"
    );
    for case in cases.cases {
        let plan: ProductionPlan = serde_json::from_str(&std::fs::read_to_string(dir.join(&case.plan))?)?;
        let config = RunConfiguration::load(dir.join(&case.config).to_str().expect("a UTF-8 path"))?;
        let placement = Placement {
            w: case.w,
            h: case.h,
            machines: case.placement.iter().map(|m| PlacedMachine { id: m.id, x: m.x, y: m.y, rot: Dir::ALL[m.rot], mirror: m.mirror }).collect(),
        };
        let mut problem = RouteProblem::new(&plan, &config, case.tuning.clone(), &placement);
        problem.obstacles = static_obstacles(&config, &plan, case.w, case.h);
        let once = || {
            let t = Instant::now();
            let mut pf = PathFinder::new(&problem);
            let r = pf.run();
            (t.elapsed().as_secs_f64() * 1e3, r)
        };
        for _ in 0..warm {
            once();
        }
        let runs: Vec<_> = (0..reps).map(|_| once()).collect();
        let ms = median(runs.iter().map(|r| r.0).collect());
        let r = &runs[0].1;
        let jvm = kotlin.as_ref().and_then(|k| k.cases.iter().find(|c| c.case == case.case));
        let (jvm_ms, ratio, same) = match jvm {
            Some(j) => {
                let same = j.success == r.success && j.iterations == r.iterations && j.expansions == r.expansions && j.entities == r.entities.len();
                (format!("{:.1}", j.median_ms), format!("{:.2}x", j.median_ms / ms), if same { "yes" } else { "NO" })
            }
            None => ("-".to_owned(), "-".to_owned(), "-"),
        };
        println!(
            "{:<22} {:>8} {:>5} {:>11} {:>11} {:>11.1} {:>7} {:>12.2}   {same}",
            case.case,
            jvm.map_or(plan.machines.len(), |j| j.machines),
            r.iterations,
            r.expansions,
            jvm_ms,
            ms,
            ratio,
            r.expansions as f64 / ms / 1e3
        );
    }
    Ok(())
}
