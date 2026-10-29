//! The command line: what a run writes, what --print-tuning shows, and the flags it refuses. The jobs get a search
//! that ends after one extra round instead of running out a wall-clock budget.

mod common;

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("factorio-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A job file: a test job with a quick search, and `more` merged over it.
fn write_job(dir: &Path, job: &str, more: Value) -> PathBuf {
    let text = std::fs::read_to_string(common::repo_root().join(format!("src/test/resources/jobs/{job}.json"))).unwrap();
    let mut config: Value = serde_json::from_str(&text).unwrap();
    config["search"] = json!({"time_budget_s": 3600, "seeds_per_size": 2, "threads": 2, "seed": 7, "verbose": false});
    config["tuning"] = json!({"search": {"extra_rounds": 1}});
    for (k, v) in more.as_object().unwrap() {
        config[k] = v.clone();
    }
    let path = dir.join("job.json");
    std::fs::write(&path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    path
}

fn run(args: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_factorio_optimizer")).args(args).output().unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn a_search_writes_its_plan_tuning_blueprint_ports_and_logs() {
    let dir = temp_dir("search");
    let job = write_job(&dir, "t1", json!({}));
    let out = dir.join("out");
    let result = run(&[format!("--config={}", job.display()), format!("--out={}", out.display())]);
    assert!(result.status.success(), "{}", text(&result.stderr));
    assert!(text(&result.stdout).contains("Stage 1: 1 machines"), "{}", text(&result.stdout));
    for file in ["plan.json", "tuning.json", "best_output_string.txt", "best_blueprint.json", "ports.json", "attempts.tsv", "scores.tsv", "log.txt"] {
        assert!(out.join(file).exists(), "{file} was not written");
    }
    assert!(text(&std::fs::read(out.join("log.txt")).unwrap()).contains("stopped: finished"));
    let tuning: Value = serde_json::from_str(&std::fs::read_to_string(out.join("tuning.json")).unwrap()).unwrap();
    assert_eq!(tuning["search"]["extra_rounds"], 1, "the tuning file is what the run used");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_job_with_more_machines_than_a_block_runs_as_a_factory() {
    let dir = temp_dir("factory");
    let job = write_job(&dir, "t3", json!({"blocks": {"max_machines": 5}}));
    let out = dir.join("out");
    let result = run(&[format!("--config={}", job.display()), format!("--out={}", out.display())]);
    assert!(result.status.success(), "{}", text(&result.stderr));
    assert!(text(&result.stdout).contains("factory: ") && text(&result.stdout).contains(", valid"), "{}", text(&result.stdout));
    for file in ["plan.json", "factory.txt", "best_output_string.txt", "ports.json", "block_1/best_output_string.txt", "block_2/best_output_string.txt"] {
        assert!(out.join(file).exists(), "{file} was not written");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn print_tuning_shows_the_knobs_after_the_jobs_overrides_and_stops() {
    let dir = temp_dir("tuning");
    let job = write_job(&dir, "t1", json!({"tuning": {"search": {"extra_rounds": 7}}}));
    let result = run(&[format!("--config={}", job.display()), "--print-tuning".to_owned()]);
    assert!(result.status.success(), "{}", text(&result.stderr));
    let tuning: Value = serde_json::from_str(&text(&result.stdout)).expect("a JSON document");
    assert_eq!(tuning["search"]["extra_rounds"], 7);
    assert!(tuning["route"].is_object() && tuning["place"].is_object());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unknown_flags_and_the_stage_7_modes_are_refused() {
    let result = run(&["--bogus".to_owned()]);
    assert!(!result.status.success());
    assert!(text(&result.stderr).contains("Unknown flags"), "{}", text(&result.stderr));
    for mode in ["--render=somewhere", "--verify-kit=somewhere", "--calibrate=a.json"] {
        let result = run(&[mode.to_owned()]);
        assert!(!result.status.success(), "{mode}");
        assert!(text(&result.stderr).contains("not ported"), "{mode}: {}", text(&result.stderr));
    }
}

#[test]
fn a_missing_job_file_is_an_error_not_a_panic() {
    let result = run(&["--config=no/such/job.json".to_owned()]);
    assert!(!result.status.success());
    assert!(text(&result.stderr).contains("Could not find"), "{}", text(&result.stderr));
}
