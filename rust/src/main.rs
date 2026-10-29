//! Entry point. Flags (as in Main.kt):
//!   --config=<resource or path>   job config (default: the built-in config.json)
//!   --tuning=<resource or path>   tuning knobs (default: the built-in tuning.json); the job's own `tuning` section is
//!                                 merged over them
//!   --engine=sa                   the only engine there is
//!   --print-tuning                print every tuning knob with its value (after the job's overrides) and stop
//!   --out=<dir>                   output directory (default: solves/<time> - sa)
//!   --verify-kit=<dir>            write the in-game verification blueprints (see docs/VERIFY.md)
//!   --render=<dir>                redraw <dir>/best.png from its best_output_string.txt and ports.json (with ports)
//! The Kotlin mode --calibrate is not ported yet.

use anyhow::{Context, Result, bail};
use factorio_optimizer::blocks::factory::run_factory;
use factorio_optimizer::config::run_configuration::RunConfiguration;
use factorio_optimizer::config::tuning::Tuning;
use factorio_optimizer::data::loader::load_root_data;
use factorio_optimizer::emit::blueprint::decode_blueprint;
use factorio_optimizer::emit::verify_kit;
use factorio_optimizer::geom::Tile;
use factorio_optimizer::plan::build_plan;
use factorio_optimizer::search::search::run_search;
use factorio_optimizer::util::text::fmt_fixed;
use factorio_optimizer::viz::{PortLabel, RenderOptions, render_layout};
use serde_json::Value;
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
    if let Some(dir) = flags.get("render") {
        return render_output(Path::new(dir));
    }
    if let Some(dir) = flags.get("verify-kit") {
        let written = verify_kit::write(Path::new(dir))?;
        println!("wrote {written} verification blueprints to {dir}");
        return Ok(());
    }
    if flags.contains_key("calibrate") {
        bail!("--calibrate is not ported to Rust yet (stage 7 in rust/README.md); use the Kotlin build for it");
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

/// Redraws `<dir>/best.png` from the blueprint and ports.json a search or factory run left there, ports marked.
fn render_output(dir: &Path) -> Result<()> {
    let ports: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("ports.json")).context("Could not read ports.json")?).context("Bad ports.json")?;
    let number = |o: &Value, key: &str| -> Result<f64> { o[key].as_f64().with_context(|| format!("ports.json has no number {key}")) };
    let (w, h) = (number(&ports["box"], "w")? as i32, number(&ports["box"], "h")? as i32);
    let blueprint = std::fs::read_to_string(dir.join("best_output_string.txt")).context("Could not read best_output_string.txt")?;
    let layout = decode_blueprint(blueprint.trim())?.to_layout(w, h)?;
    let labels = |key: &str, input: bool| -> Result<Vec<PortLabel>> {
        ports[key]
            .as_array()
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .map(|o| {
                let lane = match o["lane"].as_str() {
                    Some("left") => 0,
                    Some("right") => 1,
                    _ => -1,
                };
                Ok(PortLabel {
                    tile: Tile::new(number(o, "x")? as i32, number(o, "y")? as i32),
                    item: o["item"].as_str().context("a port without an item")?.to_owned(),
                    rate: number(o, "rate")?,
                    input,
                    lane,
                    fluid: o["fluid"].as_bool().unwrap_or(false),
                })
            })
            .collect()
    };
    let mut all = labels("inputs", true)?;
    all.extend(labels("outputs", false)?);
    let file = dir.join("best.png");
    render_layout(&layout, &file, &RenderOptions { title: Some(format!("{w}x{h}")), ports: all, ..RenderOptions::default() })?;
    println!("rendered {}", file.display());
    Ok(())
}
