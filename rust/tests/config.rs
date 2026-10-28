mod common;

use factorio_optimizer::config::run_configuration::RunConfiguration;
use factorio_optimizer::config::tuning::Tuning;
use factorio_optimizer::geom::Rect;
use serde_json::json;

fn job(extra: serde_json::Value) -> String {
    let mut base = json!({"targets": [{"name": "electronic-circuit", "amount": 2.0}]});
    base.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    base.to_string()
}

fn rejected(extra: serde_json::Value) -> String {
    format!("{:#}", RunConfiguration::parse(&job(extra)).expect_err("this job should be rejected"))
}

#[test]
fn every_job_file_in_the_repo_loads() {
    let mut files = common::files_in("docs/examples", "json");
    files.extend(common::files_in("docs/status", "json"));
    files.extend(common::files_in("src/test/resources/jobs", "json").into_iter().filter(|p| !p.to_string_lossy().ends_with("_expected.json")));
    assert!(files.len() >= 15, "found only {} job files", files.len());
    for file in files {
        RunConfiguration::load(file.to_str().unwrap()).unwrap_or_else(|e| panic!("{}: {e:#}", file.display()));
    }
}

#[test]
fn the_built_in_example_configs_load() {
    for name in ["config.json", "example_config.json"] {
        RunConfiguration::load(name).unwrap_or_else(|e| panic!("{name}: {e:#}"));
    }
}

#[test]
fn a_job_names_only_what_it_changes() {
    let c = RunConfiguration::parse(&job(json!({}))).unwrap();
    assert_eq!(c.engine, "sa");
    assert_eq!(c.machine, "assembling-machine-3");
    assert_eq!(c.belt_tier, "fast");
    assert_eq!(c.search.threads, 4);
    assert_eq!(c.search.seed, 12345);
    assert_eq!(c.search.time_budget_s, 300.0);
    assert!(c.ports_on_ring && !c.lane_sharing && !c.test_mode);
    assert_eq!(c.marker_tile, "hazard-concrete-left");
    assert_eq!(c.inserters.allowed, ["inserter", "long-handed-inserter", "fast-inserter"]);
    assert_eq!(c.score.belt, 0.3);
    assert!(c.blocks.is_none() && c.raw_resources.is_none() && c.machines.is_none());
}

#[test]
fn values_from_the_job_replace_the_defaults() {
    let c = RunConfiguration::parse(&job(json!({
        "belt_tier": "express", "box": {"max_size": 20}, "search": {"threads": 10, "seed": 2026},
        "sources": [{"name": "iron-plate", "edge": "west", "from": 0, "to": 20}, {"name": "water", "x": -1, "y": 12, "dir": "W"}],
        "research": {"productivity": {"steel-plate": 0.2}},
    })))
    .unwrap();
    assert_eq!(c.belt_tier, "express");
    assert_eq!(c.r#box.max_size, Some(20));
    assert_eq!((c.search.threads, c.search.seed), (10, 2026));
    assert_eq!(c.search.seeds_per_size, 3, "the rest of `search` keeps its defaults");
    assert_eq!(c.sources[0].to, Some(20));
    assert_eq!((c.sources[1].x, c.sources[1].dir.as_deref()), (Some(-1), Some("W")));
    assert_eq!(c.research.productivity["steel-plate"], 0.2);
}

#[test]
fn legacy_cp_sat_fields_are_accepted_and_ignored() {
    let c = RunConfiguration::parse(&job(json!({
        "forbidden_places": [{"x": 1, "y": 2}], "sources": [{"name": "iron-plate", "edge": "west", "force": 2.5}],
    })))
    .unwrap();
    assert_eq!(c.forbidden_places.len(), 1);
    assert_eq!(c.sources[0].force, 2.5);
}

#[test]
fn unknown_keys_are_errors_at_every_level() {
    assert!(rejected(json!({"machnie": "assembling-machine-2"})).contains("machnie"));
    assert!(rejected(json!({"search": {"thread": 4}})).contains("thread"));
    assert!(rejected(json!({"sources": [{"name": "iron-plate", "edge": "west", "colour": "red"}]})).contains("colour"));
    assert!(rejected(json!({"blocks": {"max_machines": 10, "max_machine": 10}})).contains("max_machine"));
}

#[test]
fn wrong_types_are_errors() {
    rejected(json!({"search": {"threads": "many"}}));
    rejected(json!({"search": {"threads": 2.5}}));
    rejected(json!({"targets": [{"name": "x"}]}));
}

#[test]
fn a_job_needs_targets() {
    let err = RunConfiguration::parse("{}").unwrap_err().to_string();
    assert!(err.contains("target"), "{err}");
}

#[test]
fn validation_rejects_bad_choices() {
    assert!(rejected(json!({"engine": "cp-sat"})).contains("engine must be 'sa'"));
    assert!(rejected(json!({"belt_tier": "ludicrous"})).contains("Unknown belt tier"));
    assert!(rejected(json!({"machine": "rocket-silo"})).contains("unsupported machine"));
    assert!(rejected(json!({"machines": ["assembling-machine-3", "toaster"]})).contains("toaster"));
    assert!(rejected(json!({"inserters": {"allowed": ["inserter", "catapult"]}})).contains("catapult"));
    assert!(rejected(json!({"inserters": {"allowed": []}})).contains("empty"));
    assert!(rejected(json!({"research": {"inserter_capacity": 8}})).contains("0..7"));
    assert!(rejected(json!({"power": {"pole": "tree"}})).contains("Unknown pole"));
    assert!(rejected(json!({"power": {"mode": "magic"}})).contains("power.mode"));
    assert!(rejected(json!({"sinks": {"mode": "bin"}})).contains("sinks.mode"));
    assert!(rejected(json!({"sinks": {"chest": "coffin"}})).contains("Unknown chest"));
    assert!(rejected(json!({"fluids": {"mode": "bucket"}})).contains("fluids.mode"));
    assert!(rejected(json!({"marker_tile": "lava"})).contains("marker_tile"));
}

#[test]
fn belt_weaving_validation() {
    assert!(rejected(json!({"weave_tier": "fast"})).contains("must differ"));
    assert!(rejected(json!({"weave_tier": "ludicrous"})).contains("Unknown belt tier"));
    RunConfiguration::parse(&job(json!({"weave_tier": "turbo"}))).unwrap();
    RunConfiguration::parse(&job(json!({"weave_tier": "none"}))).unwrap();
}

#[test]
fn box_validation() {
    assert!(rejected(json!({"box": {"start_size": 2}})).contains("start_size"));
    assert!(rejected(json!({"box": {"max_size": 2}})).contains("max_size"));
    assert!(rejected(json!({"box": {"start_size": 30, "max_size": 20}})).contains("must not exceed"));
    RunConfiguration::parse(&job(json!({"box": {"start_size": 20, "max_size": 20}}))).unwrap();
}

#[test]
fn source_validation() {
    assert!(rejected(json!({"sources": [{"name": "iron-plate"}]})).contains("needs either edge or x and y"));
    assert!(rejected(json!({"sources": [{"name": "iron-plate", "edge": "west", "x": 1, "y": 1}]})).contains("needs either edge or x and y"));
    assert!(rejected(json!({"sources": [{"name": "iron-plate", "x": 1}]})).contains("needs either edge or x and y"));
    assert!(rejected(json!({"sources": [{"name": "iron-plate", "edge": "up"}]})).contains("unknown edge 'up'"));
    assert!(rejected(json!({"sources": [{"name": "iron-plate", "edge": "west", "dir": "N"}]})).contains("dir needs x and y"));
    assert!(rejected(json!({"outputs": [{"name": "gear", "x": 1, "y": 1, "dir": "up"}]})).contains("dir needs x and y"));
    assert!(rejected(json!({"sources": [{"edge": "west"}]})).contains("needs a name"));
    RunConfiguration::parse(&job(json!({"sources": [{"name": "iron-plate", "edge": "any"}, {"name": "gear", "x": 1, "y": 1, "dir": "S"}]}))).unwrap();
}

#[test]
fn tricks() {
    let c = RunConfiguration::parse(&job(json!({"tricks": {"B2": false, "B7": false}}))).unwrap();
    assert!(!c.trick("B2"), "the job turns B2 off");
    assert!(c.trick("B5"), "v1 tricks are on by default");
    assert!(!c.trick("I10"), "later tricks are off by default");
    assert!(rejected(json!({"tricks": {"Z9": true}})).contains("Unknown tricks"));
    assert!(rejected(json!({"tricks": {"B7": true}})).contains("not implemented"));
}

#[test]
#[should_panic(expected = "Unknown trick 'Q1'")]
fn asking_for_an_unknown_trick_is_a_bug() {
    RunConfiguration::parse(&job(json!({}))).unwrap().trick("Q1");
}

#[test]
fn the_weaving_tier_follows_the_belt_tier() {
    let weave = |extra| RunConfiguration::parse(&job(extra)).unwrap().weave().map(|t| t.key);
    assert_eq!(weave(json!({})), Some("express"), "fast belts weave under express");
    assert_eq!(weave(json!({"belt_tier": "basic"})), Some("fast"));
    assert_eq!(weave(json!({"belt_tier": "express"})), Some("fast"));
    assert_eq!(weave(json!({"belt_tier": "turbo"})), Some("express"));
    assert_eq!(weave(json!({"weave_tier": "turbo"})), Some("turbo"));
    assert_eq!(weave(json!({"weave_tier": "none"})), None);
    assert_eq!(weave(json!({"tricks": {"B5": false}})), None);
}

#[test]
fn reserved_area_validation() {
    assert!(rejected(json!({"reserved": [{"w": 0, "h": 3}]})).contains("at least 1"));
    assert!(rejected(json!({"reserved": [{"w": 2, "h": 3, "x": 1}]})).contains("both x and y"));
    assert!(rejected(json!({"reserved": [{"w": 2, "h": 3, "corner": "middle"}]})).contains("reserved corner"));
    assert!(rejected(json!({"reserved": [{"w": 2, "h": 3, "x": 1, "y": 1, "corner": "north-west"}]})).contains("reserved corner"));
}

#[test]
fn reserved_areas_take_the_corners_in_turn() {
    let c = RunConfiguration::parse(&job(json!({"reserved": [
        {"w": 2, "h": 3}, {"w": 2, "h": 3}, {"w": 2, "h": 3}, {"w": 2, "h": 3}, {"w": 1, "h": 1, "x": 4, "y": 5},
        {"w": 2, "h": 2, "corner": "north-east"},
    ]})))
    .unwrap();
    assert_eq!(
        c.reserved_rects(10, 8),
        [
            Rect::new(8, 5, 2, 3),
            Rect::new(0, 5, 2, 3),
            Rect::new(8, 0, 2, 3),
            Rect::new(0, 0, 2, 3),
            Rect::new(4, 5, 1, 1),
            Rect::new(8, 0, 2, 2),
        ]
    );
}

#[test]
fn block_validation() {
    assert!(rejected(json!({"blocks": {}})).contains("max_machines"));
    assert!(rejected(json!({"blocks": {"max_machines": 1}})).contains("max_machines"));
    assert!(rejected(json!({"blocks": {"max_machines": 10, "corridor": 1}})).contains("corridor"));
    assert!(rejected(json!({"blocks": {"max_machines": 10, "rows": 3}})).contains("rows"));
    assert!(rejected(json!({"blocks": {"max_machines": 10, "link_belt_share": 0.0}})).contains("link_belt_share"));
    assert!(rejected(json!({"blocks": {"max_machines": 10, "split_at_weight": -1.0}})).contains("split_at_weight"));
    assert!(rejected(json!({"blocks": {"max_machines": 10, "block_time_share": 1.5}})).contains("block_time_share"));
    assert!(rejected(json!({"blocks": {"max_machines": 10, "repack_share": 0.5, "merge_share": 0.5}})).contains("leave time"));
    let ok = RunConfiguration::parse(&job(json!({"blocks": {"max_machines": 120, "split_at": ["iron-plate"]}}))).unwrap();
    let b = ok.blocks.unwrap();
    assert_eq!((b.corridor, b.link_belt_share, b.split_at_weight, b.block_time_share), (4, 0.9, 0.1, 0.9));
    assert!(b.compact && !b.repack && !b.merge);
}

#[test]
fn tuning_in_a_job_is_checked_up_front() {
    assert!(rejected(json!({"tuning": {"route": {"p_mlt": 1.1}}})).contains("tuning"));
    assert!(rejected(json!({"tuning": {"router": {}}})).contains("tuning"));
    RunConfiguration::parse(&job(json!({"tuning": {"route": {"p_mult": 1.1}, "place": {"beta2": 1.0}}}))).unwrap();
}

#[test]
fn the_built_in_tuning_file_is_valid_and_is_what_defaults_returns() {
    let file = Tuning::load("tuning.json").expect("tuning.json must parse, or defaults() would silently fall back");
    assert_eq!(Tuning::defaults(), file);
    assert_ne!(file, Tuning::default(), "the measured values differ from the compiled-in ones");
}

#[test]
fn tuning_overrides_change_only_the_named_knobs() {
    let base = Tuning::default();
    let t = base.with_overrides(Some(&json!({"route": {"p_mult": 1.5}, "search": {"grow_rounds": 7}}))).unwrap();
    assert_eq!(t.route.p_mult, 1.5);
    assert_eq!(t.search.grow_rounds, 7);
    assert_eq!(t.route.max_iterations, base.route.max_iterations);
    assert_eq!(t.place, base.place);
    assert_eq!(base.with_overrides(None).unwrap(), base);
    assert_eq!(base.with_overrides(Some(&json!({}))).unwrap(), base);
}

#[test]
fn tuning_overrides_reject_unknown_keys_and_wrong_types() {
    let base = Tuning::default();
    assert!(format!("{:#}", base.with_overrides(Some(&json!({"route": {"p_mlt": 1.1}}))).unwrap_err()).contains("p_mlt"));
    assert!(base.with_overrides(Some(&json!({"route": {"p_mult": "fast"}}))).is_err());
    assert!(base.with_overrides(Some(&json!(5))).is_err());
}

#[test]
fn the_q_table_interpolates_and_extends() {
    let p = Tuning::default().place;
    assert_eq!(p.q(0), 1.0, "below the table");
    assert_eq!(p.q(1), 1.0);
    assert!((p.q(4) - 1.0828).abs() < 1e-12, "a table point");
    // Between (10, 1.4493) and (15, 1.6899).
    assert!((p.q(12) - (1.4493 + (1.6899 - 1.4493) * 2.0 / 5.0)).abs() < 1e-12);
    assert!((p.q(50) - 2.7933).abs() < 1e-12, "the last point");
    assert!((p.q(60) - (2.7933 + 10.0 * 0.02616)).abs() < 1e-12, "past the end, linearly");
}

#[test]
fn tuning_prints_and_reads_back() {
    let t = Tuning::defaults();
    let text = t.to_json();
    assert!(text.contains("\n    \"planner\": {\n        \""), "four-space indent like tuning.json:\n{}", &text[..120]);
    assert_eq!(Tuning::parse(&text).unwrap(), t);
}

#[test]
fn a_partial_tuning_file_fills_in_the_rest() {
    let t = Tuning::parse(r#"{"route": {"p_mult": 1.3}}"#).unwrap();
    assert_eq!(t.route.p_mult, 1.3);
    assert_eq!(t.place, Tuning::default().place);
    assert!(Tuning::parse(r#"{"route": {"nope": 1}}"#).is_err());
}
