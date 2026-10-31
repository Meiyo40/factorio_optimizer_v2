//! The worker's own facts, without a window: what a run reports, in what order, and what it leaves on disk.

mod common;

use factorio_optimizer::config::run_configuration::{BlocksConfig, RunConfiguration};
use factorio_optimizer::config::tuning::Tuning;
use factorio_optimizer::data::loader::load_root_data;
use factorio_optimizer::gui::worker::{Event, Job, spawn};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::{Duration, Instant};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("factorio-gui-worker-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A job from the repository's test jobs, with a quick search: the same trick `tests/cli.rs` uses.
fn job(name: &str, dir: &Path, budget: f64) -> Job {
    let text = std::fs::read_to_string(common::repo_root().join(format!("src/test/resources/jobs/{name}.json"))).unwrap();
    let mut config = RunConfiguration::parse(&text).unwrap();
    config.search.time_budget_s = budget;
    config.search.threads = 2;
    config.search.seeds_per_size = 2;
    config.search.verbose = false;
    Job { config, tuning: Tuning::defaults(), out: dir.join("out") }
}

#[test]
fn a_zoom_draws_at_the_size_it_is_shown_at_within_a_memory_cap() {
    use factorio_optimizer::gui::worker::wanted_px;

    // A small layout may be drawn as finely as the camera asks, between sensible bounds.
    assert_eq!(wanted_px(24.0, 22, 22), 24);
    assert_eq!(wanted_px(0.2, 22, 22), 4);
    assert_eq!(wanted_px(400.0, 22, 22), 48);
    // A 140x140 factory is capped so one picture stays near 16 MB of RGBA whatever the zoom.
    let big = wanted_px(48.0, 140, 140);
    assert!(big < 48, "the cap bites: {big}");
    assert!(big * big * 140 * 140 <= 4_000_000, "a picture stays within the cap: {big}");
}

#[test]
fn a_run_reports_started_log_improvements_then_finished() {
    let dir = temp_dir("sequence");
    let data = Arc::new(load_root_data("recipes.json").unwrap());
    let (tx, rx) = channel();
    let handle = spawn(Arc::clone(&data), job("t1", &dir, 8.0), tx, Arc::new(AtomicBool::new(false)));
    handle.join().unwrap();
    let events: Vec<Event> = rx.iter().collect();

    match events.first() {
        Some(Event::Started { machines, .. }) => assert!(*machines > 0, "the plan has machines"),
        other => panic!("the first event is not Started: {:?}", other.is_some()),
    }
    assert!(events.iter().any(|event| matches!(event, Event::Log(_))), "the log streams");
    let improvements: Vec<(&factorio_optimizer::search::search::Improvement, &Option<factorio_optimizer::gui::worker::Image>)> = events
        .iter()
        .filter_map(|event| match event {
            Event::Improvement { fact, image, .. } => Some((fact, image)),
            _ => None,
        })
        .collect();
    assert!(!improvements.is_empty(), "a small job finds at least one layout");
    assert!(improvements.iter().all(|(_, image)| image.as_ref().is_some_and(|i| i.w > 0 && i.h > 0)));
    match events.last() {
        Some(Event::Finished { summary, .. }) => assert!(summary.contains("trials"), "unexpected summary: {summary}"),
        _ => panic!("the last event is not Finished"),
    }

    // The files the command line would have written are all there, so the run can be replayed outside the window.
    for file in ["plan.json", "tuning.json", "best_output_string.txt", "best_blueprint.json", "ports.json", "best.png", "log.txt"] {
        assert!(dir.join("out").join(file).is_file(), "{file} was not written");
    }
}

#[test]
fn a_factory_job_reports_its_stitched_result() {
    let dir = temp_dir("factory");
    // A small job cut into blocks of four machines: a real factory run, but one that finishes inside a test budget.
    let text = std::fs::read_to_string(common::repo_root().join("src/test/resources/jobs/t3.json")).unwrap();
    let mut config = RunConfiguration::parse(&text).unwrap();
    config.blocks = Some(BlocksConfig { max_machines: 4, ..BlocksConfig::default() });
    config.search.time_budget_s = 60.0;
    config.search.threads = 4;
    config.search.verbose = false;
    let data = Arc::new(load_root_data("recipes.json").unwrap());
    let (tx, rx) = channel();
    let handle = spawn(
        Arc::clone(&data),
        Job { config, tuning: Tuning::defaults(), out: dir.join("out") },
        tx,
        Arc::new(AtomicBool::new(false)),
    );
    handle.join().unwrap();
    let events: Vec<Event> = rx.iter().collect();

    // A factory reports at the end, with a picture, rather than through improvements.
    match events.last() {
        Some(Event::Finished { summary, image }) => {
            assert!(summary.starts_with("factory"), "unexpected summary: {summary}");
            assert!(image.as_ref().is_some_and(|image| image.w > 0 && image.px > 1.0), "the factory's picture is drawn: {summary}");
        }
        _ => panic!("the last event is not Finished"),
    }
    assert!(dir.join("out").join("best_output_string.txt").is_file(), "the factory's blueprint is written");
    assert!(dir.join("out").join("factory.txt").is_file(), "the factory logs to factory.txt");
}

#[test]
fn stop_ends_a_run_and_says_so() {
    let dir = temp_dir("cancel");
    let data = Arc::new(load_root_data("recipes.json").unwrap());
    let (tx, rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let handle = spawn(Arc::clone(&data), job("pair", &dir, 600.0), tx, Arc::clone(&cancel));

    let mut events = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(180);
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(event) => {
                // Stop as soon as there is something on screen, as a person would.
                if matches!(&event, Event::Improvement { .. }) {
                    cancel.store(true, Ordering::Relaxed);
                }
                events.push(event);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    handle.join().unwrap();
    assert!(events.iter().any(|event| matches!(event, Event::Improvement { .. })), "the run never improved");
    match events.last() {
        Some(Event::Finished { summary, .. }) => {
            assert!(summary.starts_with("stopped"), "a cancelled run says so: {summary}");
        }
        _ => panic!("the last event is not Finished"),
    }
    assert!(dir.join("out").join("best_output_string.txt").is_file(), "stopping still leaves the best result");
    let log = std::fs::read_to_string(dir.join("out").join("log.txt")).unwrap();
    assert!(log.contains("cancelled"), "the log says the run was cancelled");
}

#[test]
fn the_test_scenarios_all_find_a_layout_inside_a_minute() {
    use factorio_optimizer::gui::form::JobForm;

    let scenarios = JobForm::test_scenarios();
    assert_eq!(scenarios.len(), 3, "the Test button runs three scenarios");
    let total: f64 = scenarios.iter().map(|form| form.config.search.time_budget_s).sum();
    assert!(total <= 60.0, "a test run has to fit in a minute, not {total} s");
    // Growing sizes, whatever order the shuffle put them in.
    let mut machines: Vec<usize> = Vec::new();
    let data = Arc::new(load_root_data("recipes.json").unwrap());
    let dir = temp_dir("scenarios");
    let started = Instant::now();
    for (index, form) in scenarios.into_iter().enumerate() {
        form.validate().unwrap_or_else(|e| panic!("scenario {} does not validate: {e:#}", index + 1));
        let (tx, rx) = channel();
        let job = Job { config: form.config, tuning: Tuning::defaults(), out: dir.join(format!("out{index}")) };
        spawn(Arc::clone(&data), job, tx, Arc::new(AtomicBool::new(false))).join().unwrap();
        let mut found = 0;
        for event in rx.iter() {
            match event {
                Event::Started { machines: count, .. } => machines.push(count),
                Event::Improvement { image, .. } => {
                    assert!(image.as_ref().is_some_and(|image| image.w > 0), "a picture is drawn");
                    found += 1;
                }
                Event::Failed(error) => panic!("scenario {} failed: {error}", index + 1),
                _ => {}
            }
        }
        assert!(found > 0, "scenario {} found no layout", index + 1);
    }
    machines.sort_unstable();
    println!("three scenarios in {:.1} s, machines {machines:?}", started.elapsed().as_secs_f32());
    assert!(machines[0] < machines[2], "the scenarios differ in size: {machines:?}");
}
