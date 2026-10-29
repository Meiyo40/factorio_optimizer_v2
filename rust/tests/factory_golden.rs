//! The Rust factory of blocks against the Kotlin one, on one thread with a budget that is never reached and a short
//! extra-rounds phase per block: the stitched factory (layout entity by entity, errors, score, ports, notes), the log of
//! every step (clocks aside), and the blueprint and ports.json of the factory, of every block, of every repacked
//! block and of every merged one. The goldens are dumped by rust/tools/kotlin/GoldenTrialTest.kt (FACTORY_OUT). One case
//! repacks the blocks to two heights and merges two pairs, so the corridor widening, the repack masks and the merge
//! warm start are all in it.

mod common;

use common::{ErrorJson, LayoutJson, assert_no_differences, compare, compare_errors, compare_layout, compare_list, read};
use factorio_optimizer::blocks::factory::run_factory;
use factorio_optimizer::config::run_configuration::RunConfiguration;
use factorio_optimizer::config::tuning::Tuning;
use factorio_optimizer::data::loader::load_root_data;
use factorio_optimizer::emit::encode::decode_to_json;
use factorio_optimizer::plan::ProductionPlan;
use factorio_optimizer::validate::{OutputSpec, SourceSpec};
use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
struct FactoryJson {
    success: bool,
    score: f64,
    notes: Vec<String>,
    layout: LayoutJson,
    errors: Vec<ErrorJson>,
    sources: Vec<SourceSpec>,
    outputs: Vec<OutputSpec>,
}

#[derive(Deserialize)]
struct Expected {
    factory: Option<FactoryJson>,
    log: Vec<String>,
}

fn golden_dir() -> PathBuf {
    common::repo_root().join("rust/tests/golden/factory")
}

/// A log line without its clock and its durations (see `search_golden`).
fn normalise(line: &str) -> String {
    let rest = line.strip_prefix('[').and_then(|l| l.split_once("s] ")).filter(|(clock, _)| !clock.trim().is_empty() && clock.trim().chars().all(|c| c.is_ascii_digit() || c == '.')).map_or(line, |(_, rest)| rest);
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

/// Every blueprint and ports.json under `dir`, by path relative to it.
fn written(dir: &Path) -> Vec<String> {
    fn walk(dir: &Path, prefix: &str, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap().map(Result::unwrap) {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if path.is_dir() {
                walk(&path, &format!("{prefix}{name}/"), found);
            } else if name == "best_output_string.txt" || name == "ports.json" {
                found.push(format!("{prefix}{name}"));
            }
        }
    }
    let mut found = Vec::new();
    walk(dir, "", &mut found);
    found.sort();
    found
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
    let out = std::env::temp_dir().join(format!("factorio-factory-golden-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let got = run_factory(&data, &config, &tuning, &plan, &out).unwrap();

    let mut diffs = Vec::new();
    match (&got, &want.factory) {
        (Some(g), Some(w)) => {
            compare("success", &g.success(), &w.success, &mut diffs);
            compare("score", &g.score, &w.score, &mut diffs);
            compare_list("notes", &g.notes, &w.notes, &mut diffs);
            compare_layout(&g.layout, &w.layout, &mut diffs);
            compare_errors(&g.errors, &w.errors, &mut diffs);
            compare_list("sources", &g.sources, &w.sources, &mut diffs);
            compare_list("outputs", &g.outputs, &w.outputs, &mut diffs);
        }
        (None, None) => {}
        (g, w) => diffs.push(format!("factory: rust {}, kotlin {}", g.is_some(), w.is_some())),
    }
    let log: Vec<String> = read(out.join("factory.txt")).lines().map(normalise).collect();
    compare_list("log", &log, &want.log, &mut diffs);

    // The blueprints compress to different bytes in different zlibs: compare what they decode to.
    let files = dir.join("files");
    compare_list("files written", &written(&out), &written(&files), &mut diffs);
    for file in written(&files) {
        if !out.join(&file).exists() {
            continue;
        }
        let (mine, theirs) = (read(out.join(&file)), read(files.join(&file)));
        if file.ends_with("ports.json") {
            if mine.replace("\r\n", "\n") != theirs.replace("\r\n", "\n") {
                diffs.push(format!("{file} differs"));
            }
        } else if blueprint_json(&mine) != blueprint_json(&theirs) {
            diffs.push(format!("{file} decodes to a different blueprint"));
        }
    }
    let _ = std::fs::remove_dir_all(&out);
    diffs
}

fn check(name: &str) {
    assert_no_differences(name, run_case(name));
}

#[test]
fn t3_in_two_blocks() {
    check("t3_two_blocks");
}

#[test]
fn t3_in_two_blocks_in_test_mode() {
    check("t3_two_blocks_test");
}

#[test]
fn t3_with_shared_lanes_inside_its_blocks() {
    check("t3_shared");
}

#[test]
fn t3_with_a_reserved_area_beside_the_blocks() {
    check("t3_reserved");
}

#[test]
fn t4_with_its_inputs_and_outputs_on_named_edges() {
    check("t4_edges");
}

#[test]
fn acid_leaving_by_pipe_on_the_east_edge() {
    check("acid_east");
}

#[test]
fn t3_repacked_and_merged() {
    check("t3_repack_merge");
}

#[test]
fn log_lines_lose_only_their_clocks() {
    assert_eq!(normalise("[   12.3s] block 1: 5 machines, 45000 s"), "block 1: 5 machines, 45000 s");
    assert_eq!(normalise("[    0.0s] factory, corridor 4: 21 x 27, valid; compacted: 2 rows"), "factory, corridor 4: 21 x 27, valid; compacted: 2 rows");
}
