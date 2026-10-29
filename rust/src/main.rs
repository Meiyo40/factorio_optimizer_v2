//! Entry point. Flags (as in Main.kt):
//!   --config=<resource or path>   job config (default: the built-in config.json)
//!   --tuning=<resource or path>   tuning knobs (default: the built-in tuning.json); the job's own `tuning` section is
//!                                 merged over them
//!   --engine=sa                   the only engine there is
//!   --print-tuning                print every tuning knob with its value (after the job's overrides) and stop
//!   --out=<dir>                   output directory (default: solves/<time> - sa)
//! The Kotlin modes --calibrate, --verify-kit and --render are not ported yet.

use anyhow::{Context, Result, bail};
use factorio_optimizer::blocks::factory::run_factory;
use factorio_optimizer::config::run_configuration::RunConfiguration;
use factorio_optimizer::config::tuning::Tuning;
use factorio_optimizer::data::loader::load_root_data;
use factorio_optimizer::plan::build_plan;
use factorio_optimizer::search::search::run_search;
use factorio_optimizer::util::text::fmt_fixed;
use std::collections::BTreeMap;
use std::path::Path;

const KNOWN: [&str; 9] = ["config", "tuning", "engine", "out", "calibrate", "budget", "verify-kit", "print-tuning", "render"];

fn main() -> Result<()> {
    let flags: BTreeMap<String, String> = std::env::args()
        .skip(1)
        .filter_map(|a| a.strip_prefix("--").map(str::to_owned))
        .map(|a| match a.split_once('=') {
            Some((name, value)) => (name.to_owned(), value.to_owned()),
            None => (a, "true".to_owned()),
        })
        .collect();
    let unknown: Vec<&String> = flags.keys().filter(|k| !KNOWN.contains(&k.as_str())).collect();
    if !unknown.is_empty() {
        bail!("Unknown flags: {unknown:?} (known: {KNOWN:?})");
    }
    if let Some(mode) = ["render", "verify-kit", "calibrate"].into_iter().find(|m| flags.contains_key(*m)) {
        bail!("--{mode} is not ported to Rust yet (stage 7 in rust/README.md); use the Kotlin build for it");
    }

    let loaded = RunConfiguration::load(flags.get("config").map_or("config.json", String::as_str))?;
    let config = match flags.get("engine") {
        Some(engine) => {
            let c = RunConfiguration { engine: engine.clone(), ..loaded };
            c.validate()?;
            c
        }
        None => loaded,
    };
    // tuning.json holds the measured defaults; the job's tuning section overrides them.
    let base = match flags.get("tuning") {
        Some(name) => Tuning::load(name)?,
        None => Tuning::defaults(),
    };
    let tuning = base.with_overrides(config.tuning.as_ref())?;
    if flags.contains_key("print-tuning") {
        println!("{}", tuning.to_json());
        return Ok(());
    }
    let data = load_root_data("recipes.json")?;

    let stamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
    let out = flags.get("out").cloned().unwrap_or_else(|| format!("solves/{stamp} - sa"));
    let plan = build_plan(&data, &config, &tuning)?;
    plan.write(&Path::new(&out).join("plan.json"))?;
    std::fs::write(Path::new(&out).join("tuning.json"), tuning.to_json()).context("Could not write tuning.json")?; // what this run used, for reproducing it
    println!("Stage 1: {} machines, {} nets, {} ports -> {out}/plan.json", plan.machines.len(), plan.nets.len(), plan.ports.len());
    match config.blocks.as_ref().filter(|b| plan.machines.len() > b.max_machines as usize) {
        Some(_) => match run_factory(&data, &config, &tuning, &plan, Path::new(&out))? {
            None => println!("factory: no result"),
            Some(f) => {
                let verdict = if f.success() { "valid".to_owned() } else { format!("{} validation errors", f.errors.len()) };
                println!("factory: {}x{} score {}, {verdict}", f.layout.w, f.layout.h, fmt_fixed(f.score, 1));
            }
        },
        None => {
            run_search(&data, &config, &tuning, &plan, &out)?;
        }
    }
    Ok(())
}
