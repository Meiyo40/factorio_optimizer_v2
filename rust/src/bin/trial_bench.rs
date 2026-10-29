//! Times whole trials (place, route, power, validate) on cases of the committed trial goldens, with the protocol of
//! the Kotlin benchmark (rust/tools/kotlin/GoldenTrialTest.kt, `benchTrials`): warm-up runs first, the median of the
//! timed runs. Each case's plan, config and tuning are the files Kotlin read, and the result must equal the golden
//! (stage, score, notes), so both sides did the same work. Reads trial_bench_kotlin.json, when given, for the JVM's
//! numbers and prints the ratio.
//!
//!   trial_bench [--dir rust/tests/golden/trial] [--kotlin <trial_bench_kotlin.json>] [--cases a,b,c] [--warm 3] [--reps 5]
//!               [--only <case>] [--no-check]

use anyhow::{Context, Result, bail};
use factorio_optimizer::config::run_configuration::RunConfiguration;
use factorio_optimizer::config::tuning::Tuning;
use factorio_optimizer::data::loader::load_root_data;
use factorio_optimizer::plan::ProductionPlan;
use factorio_optimizer::search::trial::{TrialOptions, run_trial};
use serde::Deserialize;
use std::path::PathBuf;
use std::time::Instant;

const CASES: [&str; 10] =
    ["t3_24_s1", "red_green_20_s1", "chem_40_s4", "em_science_46_s2", "metallurgic_52_s1", "t4_pipes_26_s1", "iron40_28_s1", "t3_16_s2", "t3_spread_14_s1", "chem_tight_36_s7"];

#[derive(Deserialize)]
struct Golden {
    plan: String,
    config: String,
    tuning: String,
    w: i32,
    h: i32,
    seed: i64,
    result: GoldenResult,
}

#[derive(Deserialize)]
struct GoldenResult {
    stage: String,
    score: f64,
    notes: Vec<String>,
}

#[derive(Deserialize)]
struct KotlinReport {
    cases: Vec<KotlinCase>,
}

#[derive(Deserialize)]
struct KotlinCase {
    case: String,
    stage: String,
    score: f64,
    notes: usize,
    median_ms: f64,
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
    let dir = PathBuf::from(arg(&args, "--dir").unwrap_or_else(|| "rust/tests/golden/trial".to_owned()));
    let kotlin: Option<KotlinReport> = arg(&args, "--kotlin").map(|p| -> Result<KotlinReport> { Ok(serde_json::from_str(&std::fs::read_to_string(&p).with_context(|| format!("reading {p}"))?)?) }).transpose()?;
    let names: Vec<String> = match (arg(&args, "--only"), arg(&args, "--cases")) {
        (Some(one), _) => vec![one],
        (None, Some(list)) => list.split(',').map(str::to_owned).collect(),
        _ => CASES.iter().map(|c| (*c).to_owned()).collect(),
    };
    let warm: Option<usize> = arg(&args, "--warm").map(|v| v.parse().expect("--warm is a number"));
    let reps: Option<usize> = arg(&args, "--reps").map(|v| v.parse().expect("--reps is a number"));
    let check = !args.iter().any(|a| a == "--no-check");
    let data = load_root_data("recipes.json")?;

    println!("{:<20} {:>8} {:>8} {:>10} {:>10} {:>7}   same work", "case", "machines", "stage", "jvm ms", "rust ms", "ratio");
    for name in names {
        let text = std::fs::read_to_string(dir.join("cases").join(format!("{name}.json"))).with_context(|| format!("case {name}"))?;
        let g: Golden = serde_json::from_str(&text)?;
        let plan: ProductionPlan = serde_json::from_str(&std::fs::read_to_string(dir.join(&g.plan))?)?;
        let config = RunConfiguration::load(dir.join(&g.config).to_str().context("a UTF-8 path")?)?;
        let tuning = Tuning::load(dir.join(&g.tuning).to_str().context("a UTF-8 path")?)?;
        let once = || -> Result<(f64, _)> {
            let t = Instant::now();
            let r = run_trial(&data, &plan, &config, &tuning, g.w, g.h, g.seed, TrialOptions::default())?;
            Ok((t.elapsed().as_secs_f64() * 1e3, r))
        };
        let (first_ms, first) = once()?;
        // Like the JVM side: a trial over 10 s is warmed once and timed three times.
        let heavy = first_ms > 10_000.0;
        for _ in 0..warm.unwrap_or(if heavy { 1 } else { 3 }).saturating_sub(1) {
            once()?;
        }
        let runs: Vec<_> = (0..reps.unwrap_or(if heavy { 3 } else { 5 })).map(|_| once()).collect::<Result<_>>()?;
        let ms = median(runs.iter().map(|r| r.0).collect());
        let same_golden = first.stage == g.result.stage && first.score == g.result.score && first.notes == g.result.notes;
        let jvm = kotlin.as_ref().and_then(|k| k.cases.iter().find(|c| c.case == name));
        let same_jvm = jvm.is_none_or(|j| j.stage == first.stage && j.score == first.score && j.notes == first.notes.len());
        if check && !(same_golden && same_jvm) {
            bail!("{name}: Rust does not reproduce the golden (stage {}, score {}), so the timing would compare different work", first.stage, first.score);
        }
        let (jvm_ms, ratio) = jvm.map_or(("-".to_owned(), "-".to_owned()), |j| (format!("{:.1}", j.median_ms), format!("{:.2}x", j.median_ms / ms)));
        println!("{name:<20} {:>8} {:>8} {jvm_ms:>10} {ms:>10.1} {ratio:>7}   {}", plan.machines.len(), first.stage, if same_golden && same_jvm { "yes" } else { "NO" });
    }
    Ok(())
}
