//! The search, ported from Kotlin's SearchTest: the best layout only ever gets smaller, what is written decodes and
//! validates, the box limits hold, and boxes with a fixed side carry the area on the other. The runs use a short
//! extra-rounds phase instead of Kotlin's wall-clock budget, and one more test checks that the number of threads
//! does not change what the search finds.

mod common;

use common::repo_root;
use factorio_optimizer::config::run_configuration::{BoxConfig, RunConfiguration, SearchConfig};
use factorio_optimizer::config::tuning::Tuning;
use factorio_optimizer::data::data_model::RootData;
use factorio_optimizer::data::loader::load_root_data;
use factorio_optimizer::emit::blueprint::decode_blueprint;
use factorio_optimizer::geom::Tile;
use factorio_optimizer::plan::{ProductionPlan, build_plan};
use factorio_optimizer::search::search::{Search, SearchResult};
use factorio_optimizer::search::trial::validation_spec;
use factorio_optimizer::validate::validate;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::LazyLock;

static DATA: LazyLock<RootData> = LazyLock::new(|| load_root_data("recipes.json").unwrap());

fn job(name: &str) -> RunConfiguration {
    RunConfiguration::load(repo_root().join(format!("src/test/resources/jobs/{name}.json")).to_str().unwrap()).unwrap()
}

fn with_search(config: RunConfiguration, threads: i32) -> RunConfiguration {
    RunConfiguration { search: SearchConfig { time_budget_s: 3600.0, seeds_per_size: 2, threads, seed: 7, verbose: false }, ..config }
}

fn quick() -> Tuning {
    let mut tuning = Tuning::default();
    tuning.search.extra_rounds = 2;
    tuning
}

fn out_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("factorio-search-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn run(config: &RunConfiguration, out: &PathBuf) -> (ProductionPlan, Tuning, SearchResult) {
    let tuning = quick();
    let plan = build_plan(&DATA, config, &tuning).unwrap();
    let result = Search::new(&DATA, config, &tuning, &plan, out, None).run().unwrap();
    (plan, tuning, result)
}

#[test]
fn box_size_never_grows_across_improvements_and_the_best_result_is_written() {
    let out = out_dir("t3");
    let config = with_search(job("t3"), 2);
    let (plan, _, res) = run(&config, &out);
    let best = res.best.as_ref().expect("no layout found");
    let areas: Vec<i32> = res.improvements.iter().map(|i| i.w * i.h).collect();
    assert!(areas.windows(2).all(|p| p[1] <= p[0]), "areas {areas:?}");
    assert!(res.improvements.windows(2).all(|p| p[1].w * p[1].h < p[0].w * p[0].h || p[1].score < p[0].score));

    // The written blueprint decodes to a layout that still validates.
    let text = std::fs::read_to_string(out.join("best_output_string.txt")).unwrap();
    let decoded = decode_blueprint(text.trim()).unwrap().to_layout(best.w, best.h).unwrap();
    assert_eq!(best.layout.as_ref().unwrap().entities.len(), decoded.entities.len());
    let outside: HashSet<Tile> = decoded.entities.iter().flat_map(|e| e.rect().tiles()).filter(|t| t.x < 0 || t.x >= best.w || t.y < 0 || t.y >= best.h).collect();
    let spec = validation_spec(&DATA, &plan, &config, best.placement.as_ref().unwrap(), &best.sources, true, &outside, &best.outputs);
    assert_eq!(validate(&decoded, &spec), Vec::new());
    assert_eq!(std::fs::read_to_string(out.join("scores.tsv")).unwrap().lines().count(), res.improvements.len() + 1);
    assert!(out.join("best.png").exists());
    for n in 1..=res.improvements.len() {
        assert!(out.join(format!("renders/improvement_{n:03}.png")).exists(), "a render per improvement");
    }
    assert!(std::fs::read_to_string(out.join("score_vs_time.svg")).unwrap().matches("<circle").count() == res.improvements.len());
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn flush_rewrites_the_best_result_and_notes_the_stop() {
    let out = out_dir("t1");
    let config = with_search(job("t1"), 2);
    let tuning = quick();
    let plan = build_plan(&DATA, &config, &tuning).unwrap();
    let mut search = Search::new(&DATA, &config, &tuning, &plan, &out, None);
    assert!(search.run().unwrap().best.is_some());
    std::fs::remove_file(out.join("best_output_string.txt")).unwrap();
    search.flush("interrupted").unwrap();
    assert!(out.join("best_output_string.txt").exists());
    assert!(std::fs::read_to_string(out.join("log.txt")).unwrap().contains("stopped: interrupted"));
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn the_search_starts_at_max_size_and_never_tries_a_larger_box() {
    let out = out_dir("t2max");
    let config = with_search(RunConfiguration { r#box: BoxConfig { max_size: Some(12), ..BoxConfig::default() }, ..job("t2") }, 2);
    let (_, _, res) = run(&config, &out);
    assert!(res.log[0].contains("start 12, max 12"), "{}", res.log[0]);
    let sizes: Vec<i32> = res
        .log
        .iter()
        .filter_map(|l| l.split_once("] size ").and_then(|(_, rest)| rest.split_once(" (")).and_then(|(n, _)| n.parse().ok()))
        .collect();
    assert!(!sizes.is_empty() && sizes.iter().all(|&s| s <= 12), "sizes tried: {sizes:?}");
    assert!(res.best.is_some(), "T2 fits in 12 x 12: {:?}", res.log);
    let bad = RunConfiguration { r#box: BoxConfig { start_size: Some(20), max_size: Some(12), ..BoxConfig::default() }, ..config };
    assert!(bad.validate().is_err());
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn fixed_width_boxes_put_the_area_into_the_height() {
    let tuning = Tuning::default();
    let box_of = |config: &RunConfiguration, s: i32| {
        let plan = build_plan(&DATA, config, &tuning).unwrap();
        Search::new(&DATA, config, &tuning, &plan, out_dir("box"), None).box_size(s)
    };
    let fixed = RunConfiguration { r#box: BoxConfig { fixed_width: Some(12), ..BoxConfig::default() }, ..job("t1") };
    assert_eq!(box_of(&fixed, 12), (12, 12));
    assert_eq!(box_of(&fixed, 18), (12, 27));
    assert_eq!(box_of(&job("t1"), 15), (15, 15));
    let tall = RunConfiguration { r#box: BoxConfig { fixed_height: Some(10), ..BoxConfig::default() }, ..job("t1") };
    assert_eq!(box_of(&tall, 20), (40, 10));
}

#[test]
fn the_number_of_threads_does_not_change_what_the_search_finds() {
    let found = |threads: i32, name: &str| {
        let out = out_dir(name);
        let (_, _, res) = run(&with_search(job("t3"), threads), &out);
        let _ = std::fs::remove_dir_all(&out);
        (res.trials, res.improvements.iter().map(|i| (i.w, i.h, i.score, i.seed)).collect::<Vec<_>>())
    };
    let one = found(1, "threads1");
    assert!(one.1.len() >= 2, "{one:?}");
    assert_eq!(one, found(4, "threads4"));
}
