//! The block splitter and the compaction against the Kotlin ones, and the two Java behaviours they lean on
//! (HashMap iteration after removals, `%.nf` formatting). The goldens are dumped by
//! rust/tools/kotlin/GoldenTrialTest.kt (SPLIT_OUT): for each job and block limit the split, the plan of every
//! block, and the split after merging each pair of neighbours; for each layout the compacted entities and the
//! coordinate maps.

mod common;

use common::{EntityJson, assert_no_differences, compare, compare_list, entity_from_json, entity_json, files_in, read};
use factorio_optimizer::blocks::compact::compact_layout;
use factorio_optimizer::blocks::split::{BlockPlan, Link, Split, block_plan, merge_blocks, split_plan};
use factorio_optimizer::config::run_configuration::{BlocksConfig, RunConfiguration};
use factorio_optimizer::config::tuning::Tuning;
use factorio_optimizer::plan::ProductionPlan;
use factorio_optimizer::util::java_hash::{JMap, JavaHash};
use factorio_optimizer::util::text::fmt_fixed;
use serde::Deserialize;
use serde_json::Value;
use std::fmt::Debug;
use std::hash::Hash;
use std::path::PathBuf;

fn golden_dir() -> PathBuf {
    common::repo_root().join("rust/tests/golden/split")
}

#[derive(Deserialize, PartialEq, Debug)]
struct LinkJson {
    id: usize,
    item: String,
    rate: f64,
    from: i32,
    to: i32,
    fluid: bool,
    routed: bool,
}

#[derive(Deserialize)]
struct SplitJson {
    blocks: Vec<Vec<usize>>,
    slots: Vec<(i32, i32)>,
    rows: i32,
    cols: i32,
    links: Vec<LinkJson>,
    warnings: Vec<String>,
}

#[derive(Deserialize)]
struct BlockPlanJson {
    index: usize,
    global_ids: Vec<usize>,
    plan: String,
    link_of_port: Vec<(usize, usize)>,
}

#[derive(Deserialize)]
struct MergeJson {
    a: usize,
    b: usize,
    split: SplitJson,
    m: usize,
    block: BlockPlanJson,
}

#[derive(Deserialize)]
struct SplitCase {
    plan: String,
    config: String,
    blocks_config: BlocksConfig,
    belt_throughput: f64,
    split: SplitJson,
    block_plans: Vec<BlockPlanJson>,
    merges: Vec<MergeJson>,
}

fn link_json(l: &Link) -> LinkJson {
    LinkJson { id: l.id, item: l.item.clone(), rate: l.rate, from: l.from, to: l.to, fluid: l.fluid, routed: l.routed }
}

fn compare_split(what: &str, got: &Split, want: &SplitJson, out: &mut Vec<String>) {
    compare_list(&format!("{what} blocks"), &got.blocks, &want.blocks, out);
    let slots: Vec<(i32, i32)> = got.slots.iter().map(|s| (s.row, s.col)).collect();
    compare_list(&format!("{what} slots"), &slots, &want.slots, out);
    compare(&format!("{what} rows"), &got.rows, &want.rows, out);
    compare(&format!("{what} cols"), &got.cols, &want.cols, out);
    let links: Vec<LinkJson> = got.links.iter().map(link_json).collect();
    compare_list(&format!("{what} links"), &links, &want.links, out);
    compare_list(&format!("{what} warnings"), &got.warnings, &want.warnings, out);
}

fn compare_block_plan(what: &str, got: &BlockPlan, want: &BlockPlanJson, out: &mut Vec<String>) {
    compare(&format!("{what} index"), &got.index, &want.index, out);
    compare(&format!("{what} global ids"), &got.global_ids, &want.global_ids, out);
    let mut ports: Vec<(usize, usize)> = got.link_of_port.iter().map(|(&p, &l)| (p, l)).collect();
    ports.sort_unstable();
    compare(&format!("{what} link of port"), &ports, &want.link_of_port, out);
    let expected: ProductionPlan = serde_json::from_str(&read(golden_dir().join(&want.plan))).unwrap();
    let p = &got.plan;
    compare_list(&format!("{what} machines"), &p.machines, &expected.machines, out);
    compare_list(&format!("{what} ports"), &p.ports, &expected.ports, out);
    compare_list(&format!("{what} terminals"), &p.terminals, &expected.terminals, out);
    compare_list(&format!("{what} nets"), &p.nets, &expected.nets, out);
    let pairs = |m: &indexmap::IndexMap<String, f64>| m.iter().map(|(k, v)| (k.clone(), *v)).collect::<Vec<_>>();
    compare_list(&format!("{what} raw rates"), &pairs(&p.raw_rates), &pairs(&expected.raw_rates), out);
    compare_list(&format!("{what} fluid rates"), &pairs(&p.fluid_rates), &pairs(&expected.fluid_rates), out);
    compare_list(&format!("{what} recipe rates"), &pairs(&p.recipe_rates), &pairs(&expected.recipe_rates), out);
    compare_list(&format!("{what} outputs"), &pairs(&p.outputs), &pairs(&expected.outputs), out);
    compare(&format!("{what} belt tier"), &p.belt_tier, &expected.belt_tier, out);
    compare_list(&format!("{what} warnings"), &p.warnings, &expected.warnings, out);
}

fn run_case(name: &str) -> Vec<String> {
    let dir = golden_dir();
    let g: SplitCase = serde_json::from_str(&read(dir.join("cases").join(format!("{name}.json")))).unwrap_or_else(|e| panic!("{name}: {e}"));
    let plan: ProductionPlan = serde_json::from_str(&read(dir.join(&g.plan))).unwrap();
    let config = RunConfiguration::load(dir.join(&g.config).to_str().unwrap()).unwrap();
    let tuning = Tuning::defaults().with_overrides(config.tuning.as_ref()).unwrap();
    let mut out = Vec::new();
    let split = split_plan(&plan, &g.blocks_config, g.belt_throughput);
    compare_split("split", &split, &g.split, &mut out);
    for want in &g.block_plans {
        compare_block_plan(&format!("block {}", want.index), &block_plan(&plan, &split, want.index, &tuning), want, &mut out);
    }
    for want in &g.merges {
        let merged = merge_blocks(&split, want.a, want.b);
        let what = format!("merge {}+{}", want.a, want.b);
        compare_split(&what, &merged, &want.split, &mut out);
        compare_block_plan(&format!("{what} block {}", want.m), &block_plan(&plan, &merged, want.m, &tuning), &want.block, &mut out);
    }
    out
}

fn case_names(sub: &str) -> Vec<String> {
    files_in(&format!("rust/tests/golden/split/{sub}"), "json").iter().map(|p| p.file_stem().unwrap().to_string_lossy().into_owned()).collect()
}

#[test]
fn splits_match_kotlin() {
    let names = case_names("cases");
    assert!(names.len() >= 20, "the split goldens are missing: {names:?}");
    let mut failures = Vec::new();
    for name in &names {
        let diffs = run_case(name);
        if !diffs.is_empty() {
            failures.push(format!("{name}:\n  {}", diffs.join("\n  ")));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[derive(Deserialize)]
struct CompactResult {
    w: i32,
    h: i32,
    rows_removed: i32,
    cols_removed: i32,
    entities: Vec<EntityJson>,
    xs: Vec<i32>,
    ys: Vec<i32>,
}

#[derive(Deserialize)]
struct CompactCase {
    w: i32,
    h: i32,
    entities: Vec<EntityJson>,
    result: CompactResult,
}

#[test]
fn compaction_matches_kotlin() {
    let names = case_names("compact");
    assert!(names.len() >= 10, "the compaction goldens are missing: {names:?}");
    let mut removed = 0;
    for name in &names {
        let g: CompactCase = serde_json::from_str(&read(golden_dir().join("compact").join(format!("{name}.json")))).unwrap();
        let entities: Vec<_> = g.entities.iter().map(entity_from_json).collect();
        let c = compact_layout(&entities, &[], g.w, g.h);
        let mut out = Vec::new();
        compare("size", &(c.w, c.h), &(g.result.w, g.result.h), &mut out);
        compare("rows and columns removed", &(c.rows_removed, c.cols_removed), &(g.result.rows_removed, g.result.cols_removed), &mut out);
        let got: Vec<EntityJson> = c.entities.iter().map(entity_json).collect();
        compare_list("entities", &got, &g.result.entities, &mut out);
        let xs: Vec<i32> = (-2..g.w + 3).map(|x| c.x(x)).collect();
        let ys: Vec<i32> = (-2..g.h + 3).map(|y| c.y(y)).collect();
        compare("x map", &xs, &g.result.xs, &mut out);
        compare("y map", &ys, &g.result.ys, &mut out);
        assert_no_differences(name, out);
        removed += usize::from(c.rows_removed + c.cols_removed > 0);
    }
    assert!(removed >= 5, "only {removed} layouts lost a row or column: the comparison would be vacuous");
}

#[derive(Deserialize)]
struct HashRun {
    kind: String,
    ops: Vec<(String, Value)>,
    snapshots: Vec<Snapshot>,
}

#[derive(Deserialize)]
struct Snapshot {
    after: usize,
    order: Vec<Value>,
}

#[derive(Deserialize)]
struct FmtSample {
    value: f64,
    digits: usize,
    text: String,
}

#[derive(Deserialize)]
struct JavaSamples {
    hash_runs: Vec<HashRun>,
    fmt: Vec<FmtSample>,
}

fn samples() -> JavaSamples {
    serde_json::from_str(&read(golden_dir().join("java_samples.json"))).unwrap()
}

/// Puts and removals on a map, with the order Java's HashMap iterates in after every eight operations.
fn replay<K: JavaHash + Hash + Eq + Clone + Debug>(run: &HashRun, decode: impl Fn(&Value) -> K) {
    let mut map: JMap<K, usize> = JMap::new();
    let mut checked = 0;
    for (i, (op, key)) in run.ops.iter().enumerate() {
        let k = decode(key);
        match op.as_str() {
            "put" => map.insert(k, i),
            "merge" => map.merge(k, 1, |old, add| old + add),
            "remove" => {
                map.remove(&k);
            }
            other => panic!("unknown operation {other}"),
        }
        if let Some(s) = run.snapshots.iter().find(|s| s.after == i + 1) {
            let got: Vec<K> = map.keys_java().cloned().collect();
            let want: Vec<K> = s.order.iter().map(&decode).collect();
            assert_eq!(got, want, "{} keys after {} operations", run.kind, s.after);
            checked += 1;
        }
    }
    assert_eq!(checked, run.snapshots.len());
}

#[test]
fn hash_maps_with_removals_iterate_like_java() {
    let s = samples();
    let kinds: Vec<&str> = s.hash_runs.iter().map(|r| r.kind.as_str()).collect();
    assert!(["int", "long", "pair"].iter().all(|k| kinds.contains(k)));
    for run in &s.hash_runs {
        match run.kind.as_str() {
            "int" => replay(run, |v| v.as_i64().unwrap() as i32),
            "long" => replay(run, |v| v.as_i64().unwrap()),
            "pair" => replay(run, |v| (v[0].as_i64().unwrap() as i32, v[1].as_i64().unwrap() as i32)),
            other => panic!("unknown key kind {other}"),
        }
    }
}

#[test]
fn fixed_formatting_matches_java() {
    let s = samples();
    assert!(s.fmt.len() > 100);
    for f in &s.fmt {
        assert_eq!(fmt_fixed(f.value, f.digits), f.text, "%.{}f of {}", f.digits, f.value);
    }
}
