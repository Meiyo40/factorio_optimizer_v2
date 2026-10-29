//! The command line: what a run writes, what --print-tuning shows, and the flags it refuses. The jobs get a search
//! that ends after one extra round instead of running out a wall-clock budget.

mod common;

use factorio_optimizer::emit::blueprint::decode_blueprint;
use factorio_optimizer::geom::Tile;
use factorio_optimizer::viz::{PortLabel, RenderOptions, image_size};
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
    for file in [
        "plan.json",
        "tuning.json",
        "best_output_string.txt",
        "best_blueprint.json",
        "ports.json",
        "attempts.tsv",
        "scores.tsv",
        "log.txt",
        "best.png",
        "renders/improvement_001.png",
        "score_vs_time.svg",
    ] {
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
    for file in ["plan.json", "factory.txt", "best_output_string.txt", "ports.json", "best.png", "block_1/best_output_string.txt", "block_1/best.png", "block_2/best_output_string.txt"] {
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
fn unknown_flags_and_bad_numbers_are_refused() {
    let result = run(&["--bogus".to_owned()]);
    assert!(!result.status.success());
    assert!(text(&result.stderr).contains("Unknown flags"), "{}", text(&result.stderr));
    let dir = temp_dir("badbudget");
    let job = write_job(&dir, "t1", json!({}));
    let result = run(&[format!("--calibrate={}", job.display()), "--budget=soon".to_owned()]);
    assert!(!result.status.success());
    assert!(text(&result.stderr).contains("--budget wants"), "{}", text(&result.stderr));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn calibrate_searches_the_jobs_reports_and_writes_a_tuning_next_to_them() {
    let dir = temp_dir("calibrate");
    let job = write_job(&dir, "t2", json!({}));
    let tuning = dir.join("base.json");
    std::fs::write(&tuning, r#"{"search": {"extra_rounds": 1}}"#).unwrap();
    let out = dir.join("out");
    let result = run(&[format!("--calibrate={}", job.display()), format!("--tuning={}", tuning.display()), "--budget=3600".to_owned(), format!("--out={}", out.display())]);
    assert!(result.status.success(), "{}", text(&result.stderr));
    let stdout = text(&result.stdout);
    assert!(stdout.contains("job: best "), "named after the file: {stdout}");
    assert!(stdout.contains("area factor") && stdout.contains("routed chains"), "{stdout}");
    let tuned: Value = serde_json::from_str(&std::fs::read_to_string(out.join("tuning.json")).unwrap()).unwrap();
    // The factor is the box area over the machine area of the best layout, less a margin.
    let factor = tuned["search"]["area_factor"].as_f64().unwrap();
    assert!(factor > 1.0 && factor < 6.0 && factor != 1.8, "{factor}");
    assert!(out.join("job/best_output_string.txt").exists(), "each job searches in a folder of its own");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn verify_kit_writes_the_blueprints_of_the_kit() {
    let dir = temp_dir("kit");
    let out = dir.join("kit");
    let result = run(&[format!("--verify-kit={}", out.display())]);
    assert!(result.status.success(), "{}", text(&result.stderr));
    assert!(text(&result.stdout).contains("wrote 18 verification blueprints"), "{}", text(&result.stdout));
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 19);
    assert!(out.join("v1-inserter-direction.txt").exists() && out.join("t1-hand.txt").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn render_redraws_best_png_from_a_folder_with_its_blueprint_and_ports() {
    let dir = temp_dir("render");
    let golden = common::repo_root().join("rust/tests/golden/search/t3/files");
    for f in ["best_output_string.txt", "ports.json"] {
        std::fs::copy(golden.join(f), dir.join(f)).unwrap();
    }
    let result = run(&[format!("--render={}", dir.display())]);
    assert!(result.status.success(), "{}", text(&result.stderr));
    assert!(text(&result.stdout).contains("rendered"), "{}", text(&result.stdout));
    let png = tiny_skia::Pixmap::load_png(dir.join("best.png")).unwrap();
    // As big as the library says a picture of this layout with a title and one line per port is.
    let ports: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("ports.json")).unwrap()).unwrap();
    let count = ports["inputs"].as_array().unwrap().len() + ports["outputs"].as_array().unwrap().len();
    assert_eq!(count, 4, "t3 has two inputs and two outputs");
    let (w, h) = (ports["box"]["w"].as_i64().unwrap() as i32, ports["box"]["h"].as_i64().unwrap() as i32);
    let layout = decode_blueprint(std::fs::read_to_string(dir.join("best_output_string.txt")).unwrap().trim()).unwrap().to_layout(w, h).unwrap();
    let port = PortLabel { tile: Tile::new(0, 0), item: String::new(), rate: 0.0, input: true, lane: -1, fluid: false };
    let expected = image_size(&layout, &RenderOptions { title: Some(String::new()), ports: vec![port; count], ..RenderOptions::default() });
    assert_eq!((png.width() as i32, png.height() as i32), expected);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_job_file_is_an_error_not_a_panic() {
    let result = run(&["--config=no/such/job.json".to_owned()]);
    assert!(!result.status.success());
    assert!(text(&result.stderr).contains("Could not find"), "{}", text(&result.stderr));
}
