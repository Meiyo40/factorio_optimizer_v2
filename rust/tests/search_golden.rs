//! The Rust box-size search against the Kotlin one, on one thread with a budget that is never reached and a short
//! extra-rounds phase, so a run is reproducible: the improvements, the number of trials, the best result, the log
//! lines (without their clocks), the rows of attempts.tsv and scores.tsv (without their times) and the files written
//! for the best layout. The goldens are dumped by rust/tools/kotlin/GoldenTrialTest.kt (SEARCH_OUT).

mod common;

use common::{assert_no_differences, compare, compare_list, read};
use factorio_optimizer::config::run_configuration::RunConfiguration;
use factorio_optimizer::config::tuning::Tuning;
use factorio_optimizer::data::loader::load_root_data;
use factorio_optimizer::emit::encode::decode_to_json;
use factorio_optimizer::plan::ProductionPlan;
use factorio_optimizer::search::search::Search;
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;

#[derive(Deserialize, PartialEq, Debug)]
struct ImprovementJson {
    w: i32,
    h: i32,
    score: f64,
    belts: usize,
    undergrounds: usize,
    inserters: usize,
    seed: i64,
}

#[derive(Deserialize, PartialEq, Debug)]
struct BestJson {
    w: i32,
    h: i32,
    seed: i64,
    score: f64,
    anneal_t0: f64,
}

#[derive(Deserialize)]
struct Expected {
    trials: usize,
    improvements: Vec<ImprovementJson>,
    best: Option<BestJson>,
    log: Vec<String>,
    attempts: Vec<Vec<String>>,
    scores: Vec<Vec<String>>,
    boxes: Vec<(i32, i32, i32)>,
}

fn golden_dir() -> PathBuf {
    common::repo_root().join("rust/tests/golden/search")
}

/// A log line without its clock: the stamp and any "1.2s" duration vary between runs (Kotlin's regexes
/// `^\[ *[0-9.]+s\] ` and `[0-9]+\.[0-9]s`).
fn normalise(line: &str) -> String {
    let rest = line
        .strip_prefix('[')
        .and_then(|l| l.split_once("s] "))
        .filter(|(clock, _)| clock.trim_start().chars().all(|c| c.is_ascii_digit() || c == '.') && !clock.trim().is_empty())
        .map_or(line, |(_, rest)| rest);
    let chars: Vec<char> = rest.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            let mut j = i;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            if j + 2 < chars.len() && chars[j] == '.' && chars[j + 1].is_ascii_digit() && chars[j + 2] == 's' {
                out.push_str("Ns");
                i = j + 3;
                continue;
            }
            out.extend(&chars[i..j]);
            i = j;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// The rows of a tsv without its header, minus the columns that hold times.
fn rows(path: &std::path::Path, drop: &[usize]) -> Vec<Vec<String>> {
    read(path.to_path_buf()).lines().skip(1).map(|l| l.split('\t').enumerate().filter(|(i, _)| !drop.contains(i)).map(|(_, c)| c.to_owned()).collect()).collect()
}

fn blueprint_json(text: &str) -> Value {
    serde_json::from_str(&decode_to_json(text.trim()).unwrap()).unwrap()
}

fn run_case(name: &str) -> Vec<String> {
    let dir = golden_dir().join(name);
    let want: Expected = serde_json::from_str(&read(dir.join("expected.json"))).unwrap();
    let plan: ProductionPlan = serde_json::from_str(&read(dir.join("plan.json"))).unwrap();
    let config = RunConfiguration::load(dir.join("config.json").to_str().unwrap()).unwrap();
    let tuning = Tuning::load(dir.join("tuning.json").to_str().unwrap()).unwrap();
    let data = load_root_data("recipes.json").unwrap();
    let out = std::env::temp_dir().join(format!("factorio-search-golden-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let mut search = Search::new(&data, &config, &tuning, &plan, &out, None);
    let got = search.run().unwrap();

    let mut diffs = Vec::new();
    compare("trials", &got.trials, &want.trials, &mut diffs);
    let improvements: Vec<ImprovementJson> =
        got.improvements.iter().map(|i| ImprovementJson { w: i.w, h: i.h, score: i.score, belts: i.belts, undergrounds: i.undergrounds, inserters: i.inserters, seed: i.seed }).collect();
    compare_list("improvements", &improvements, &want.improvements, &mut diffs);
    let best = got.best.as_ref().map(|b| BestJson { w: b.w, h: b.h, seed: b.seed, score: b.score, anneal_t0: b.anneal_t0 });
    compare("best", &best, &want.best, &mut diffs);
    let log: Vec<String> = got.log.iter().map(|l| normalise(l)).collect();
    compare_list("log", &log, &want.log, &mut diffs);
    compare_list("attempts.tsv", &rows(&out.join("attempts.tsv"), &[0, 5]), &want.attempts, &mut diffs);
    compare_list("scores.tsv", &rows(&out.join("scores.tsv"), &[0]), &want.scores, &mut diffs);
    let boxes: Vec<(i32, i32, i32)> = (3..=60).map(|s| (s, search.box_size(s).0, search.box_size(s).1)).collect();
    compare_list("box sizes", &boxes, &want.boxes, &mut diffs);

    // The blueprint compresses to different bytes in different zlibs: compare what it decodes to.
    let files = dir.join("files");
    let (mine, theirs) = (read(out.join("best_output_string.txt")), read(files.join("best_output_string.txt")));
    if blueprint_json(&mine) != blueprint_json(&theirs) {
        diffs.push("best_output_string.txt decodes to a different blueprint".to_owned());
    }
    let (mine, theirs) = (read(out.join("best_blueprint.json")), read(files.join("best_blueprint.json")));
    if serde_json::from_str::<Value>(&mine).unwrap() != serde_json::from_str::<Value>(&theirs).unwrap() {
        diffs.push("best_blueprint.json differs".to_owned());
    }
    // Git may check the golden out with CRLF line endings.
    compare("ports.json", &read(out.join("ports.json")).replace("\r\n", "\n"), &read(files.join("ports.json")).replace("\r\n", "\n"), &mut diffs);
    let _ = std::fs::remove_dir_all(&out);
    diffs
}

#[test]
fn log_lines_lose_only_their_clocks() {
    assert_eq!(normalise("[   12.3s] size 20 (20x20): 1/2 seeds succeeded"), "size 20 (20x20): 1/2 seeds succeeded");
    assert_eq!(normalise("[    0.0s]   9x9 try 1: ok in 0.1s, 1 routing run, score 91.2"), "  9x9 try 1: ok in Ns, 1 routing run, score 91.2");
    assert_eq!(normalise("budget 100000.0s, 2 seeds x 1 threads"), "budget Ns, 2 seeds x 1 threads");
    assert_eq!(normalise("a 1.25s b"), "a 1.25s b");
}

fn check(name: &str) {
    assert_no_differences(name, run_case(name));
}

#[test]
fn search_t1() {
    check("t1");
}

#[test]
fn search_t2_with_a_largest_size() {
    check("t2_max12");
}

#[test]
fn search_t1_with_a_fixed_width() {
    check("t1_fixed_width");
}

#[test]
fn search_t3() {
    check("t3");
}

#[test]
fn search_t3_in_a_wide_box() {
    check("t3_aspect");
}
