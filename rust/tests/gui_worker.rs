//! The worker's own facts, without a window: what a run reports, in what order, and what it leaves on disk.

mod common;

use factorio_optimizer::config::run_configuration::RunConfiguration;
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
