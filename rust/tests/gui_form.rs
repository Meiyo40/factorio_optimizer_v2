//! The job form's own facts, without opening a window: what a new form holds, the built-in example, the presets in
//! `docs/`, what the JSON tab does with text that is not a job, and that a saved job reloads as itself.

mod common;

use factorio_optimizer::config::run_configuration::RunConfiguration;
use factorio_optimizer::gui::form::JobForm;
use std::path::PathBuf;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("factorio-gui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_new_form_holds_the_default_job() {
    let form = JobForm::new(RunConfiguration::default());
    let back: RunConfiguration = serde_json::from_str(&form.json).expect("the tab holds the job");
    assert_eq!(back, RunConfiguration::default());
    assert!(!form.json_edited);
    assert!(form.json_error.is_none());
}

#[test]
fn the_built_in_example_loads_and_validates() {
    let form = JobForm::example().expect("the example job loads");
    form.validate().expect("the example job validates");
    // It carries its targets and its forbidden recipe, so the tab is the file and not a default.
    assert_eq!(form.config.targets.len(), 2);
    assert_eq!(form.config.forbidden_recipes, vec!["productivity-module".to_owned()]);
    assert_eq!(form.path, "example_config.json");
}

#[test]
fn an_empty_job_says_what_is_missing() {
    let form = JobForm::new(RunConfiguration::default());
    let error = format!("{:#}", form.validate().expect_err("a job without targets does not validate"));
    assert!(error.contains("at least one target"), "unexpected message: {error}");
}

#[test]
fn the_json_tab_round_trips_through_the_form() {
    let mut form = JobForm::example().expect("the example job loads");
    let edited = form.json.replace("\"time_budget_s\": 300.0", "\"time_budget_s\": 42.0");
    assert_ne!(edited, form.json, "the tab should hold the default budget");
    form.json = edited;
    form.json_edited = true;
    // The form has not taken the edit yet, so syncing must leave the tab alone.
    form.sync_json();
    assert!(form.json.contains("42.0"));
    form.apply_json().expect("42 s is a valid budget");
    assert_eq!(form.config.search.time_budget_s, 42.0);
    assert!(!form.json_edited);
}

#[test]
fn text_that_is_not_a_job_leaves_the_form_alone() {
    let mut form = JobForm::example().expect("the example job loads");
    let before = form.config.clone();
    form.json = "{ \"targets\": [ ".to_owned();
    form.json_edited = true;
    form.apply_json().expect_err("half a JSON object is not a job");
    assert_eq!(form.config, before);
    assert!(form.json_error.as_deref().is_some_and(|e| !e.is_empty()));
    // A job that parses but would not run is refused as well, and says why.
    form.json = serde_json::to_string(&RunConfiguration { targets: Vec::new(), ..before.clone() }).unwrap();
    let error = format!("{:#}", form.apply_json().expect_err("a job without targets does not run"));
    assert!(error.contains("at least one target"), "unexpected message: {error}");
    assert_eq!(form.config, before);
}

#[test]
fn a_saved_job_reloads_as_itself() {
    let dir = temp_dir("save");
    let path = dir.join("job.json");
    let mut form = JobForm::example().expect("the example job loads");
    form.path = path.to_string_lossy().into_owned();
    form.config.search.threads = 3;
    form.save().expect("the job saves");

    let back = JobForm::load(&form.path).expect("the saved job loads");
    assert_eq!(back.config, form.config);
    assert_eq!(back.path, form.path);
}

#[test]
fn saving_without_a_path_says_so() {
    let form = JobForm::new(RunConfiguration::default());
    let error = format!("{:#}", form.save().expect_err("there is nowhere to save to"));
    assert!(error.contains("no path"), "unexpected message: {error}");
}

#[test]
fn blocks_start_from_a_section_that_validates_and_keep_their_settings() {
    let mut form = JobForm::example().expect("the example job loads");
    form.toggle_blocks(true);
    let blocks = form.config.blocks.clone().expect("the section is on");
    assert!(blocks.max_machines >= 2, "a fresh section has to validate");
    form.validate().expect("the example job with blocks validates");

    form.config.blocks.as_mut().unwrap().max_machines = 24;
    form.toggle_blocks(true);
    assert_eq!(form.config.blocks.as_ref().unwrap().max_machines, 24, "toggling on twice keeps the settings");

    form.toggle_blocks(false);
    assert!(form.config.blocks.is_none());
    form.validate().expect("turning blocks off validates again");
}

#[test]
fn a_trick_toggle_reaches_the_job() {
    let mut form = JobForm::example().expect("the example job loads");
    assert!(form.config.trick("I1"));
    form.toggle_trick("I1", false);
    assert!(!form.config.trick("I1"));
    assert!(!form.json_edited, "a toggle goes through the job, not the tab");
}

#[test]
fn the_output_directory_is_the_jobs_or_a_stamped_one() {
    let mut form = JobForm::example().expect("the example job loads");
    assert_eq!(form.out_dir("2025-01-01_00-00-00"), "solves/2025-01-01_00-00-00 - gui");
    form.out = "  solves/mine  ".to_owned();
    assert_eq!(form.out_dir("ignored"), "solves/mine");
}

#[test]
fn every_preset_in_docs_loads() {
    let mut seen = 0;
    for path in common::files_in("docs/examples", "json").into_iter().chain(common::files_in("docs/status", "json")) {
        let path = path.to_string_lossy().into_owned();
        let form = JobForm::load(&path).unwrap_or_else(|e| panic!("{path}: {e:#}"));
        form.validate().unwrap_or_else(|e| panic!("{path} does not validate: {e:#}"));
        assert!(!form.config.targets.is_empty(), "{path} has no targets");
        seen += 1;
    }
    assert!(seen >= 7, "only {seen} presets were found");
}
