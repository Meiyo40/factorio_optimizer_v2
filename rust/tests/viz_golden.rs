//! The Rust render against the Kotlin one. Java2D and tiny-skia antialias differently and the text is another font,
//! so the pictures cannot be equal pixel for pixel; what must hold is the picture's size (an exact formula), and,
//! where Kotlin drew a flat area (the box, markers, machine fills, chests, heat tiles), the same colour in the
//! same place, and everywhere else a picture that is close: only a small share of the map's pixels may differ
//! strongly. The goldens are dumped by rust/tools/kotlin/GoldenVizTest.kt (VIZ_OUT).
//! VIZ_CASE=<name> runs one case; VIZ_SAVE=<dir> writes Rust's pictures there to look at beside Kotlin's.

mod common;

use common::{ErrorJson, LayoutJson, files_in, layout_from_json, read};
use factorio_optimizer::geom::Tile;
use factorio_optimizer::validate::{ErrorType, ValidationError};
use factorio_optimizer::viz::{HeatOverlay, PortLabel, RenderOptions, render_image};
use serde::Deserialize;
use std::path::PathBuf;
use tiny_skia::Pixmap;

#[derive(Deserialize)]
struct HeatValue {
    x: i32,
    y: i32,
    v: f64,
}

#[derive(Deserialize)]
struct HeatJson {
    values: Vec<HeatValue>,
    overused: Vec<Tile>,
}

#[derive(Deserialize)]
struct PortJson {
    x: i32,
    y: i32,
    item: String,
    rate: f64,
    input: bool,
    lane: i32,
    fluid: bool,
}

#[derive(Deserialize)]
struct OptionsJson {
    px: i32,
    show_supply: bool,
    title: Option<String>,
    errors: Vec<ErrorJson>,
    heat: Option<HeatJson>,
    ports: Vec<PortJson>,
}

#[derive(Deserialize)]
struct Golden {
    case: String,
    png: String,
    width: u32,
    height: u32,
    layout: LayoutJson,
    options: OptionsJson,
}

fn golden_dir() -> PathBuf {
    common::repo_root().join("rust/tests/golden/viz")
}

fn options_of(o: &OptionsJson) -> RenderOptions {
    RenderOptions {
        px: o.px,
        show_supply: o.show_supply,
        title: o.title.clone(),
        errors: o
            .errors
            .iter()
            .map(|e| {
                let kind: ErrorType = serde_json::from_value(serde_json::Value::String(e.kind.clone())).unwrap();
                ValidationError { kind, tile: Tile::new(e.x, e.y), message: e.message.clone() }
            })
            .collect(),
        heat: o.heat.as_ref().map(|h| HeatOverlay { values: h.values.iter().map(|v| (Tile::new(v.x, v.y), v.v)).collect(), overused: h.overused.iter().copied().collect() }),
        ports: o.ports.iter().map(|p| PortLabel { tile: Tile::new(p.x, p.y), item: p.item.clone(), rate: p.rate, input: p.input, lane: p.lane, fluid: p.fluid }).collect(),
    }
}

fn pixel(p: &Pixmap, x: u32, y: u32) -> [u8; 3] {
    let c = p.pixel(x, y).expect("inside the picture");
    [c.red(), c.green(), c.blue()]
}

fn spread(a: [u8; 3], b: [u8; 3]) -> i32 {
    (0..3).map(|k| (i32::from(a[k]) - i32::from(b[k])).abs()).max().unwrap()
}

/// What a comparison found, for the report and the thresholds.
struct Stats {
    /// Share of the map's pixels that differ by more than 48 in some channel.
    strong: f64,
    /// Of the pixels in Kotlin's flat areas (5 x 5 of one colour), the share Rust paints differently (by more than 2).
    flat_wrong: f64,
    flat_pixels: usize,
}

fn compare(rust: &Pixmap, kotlin: &Pixmap, map_height: u32) -> Stats {
    let (w, h) = (kotlin.width(), kotlin.height().min(map_height));
    let (mut strong, mut flat, mut flat_wrong) = (0usize, 0usize, 0usize);
    for y in 0..h {
        for x in 0..w {
            let (k, r) = (pixel(kotlin, x, y), pixel(rust, x, y));
            if spread(k, r) > 48 {
                strong += 1;
            }
            let is_flat = x >= 2 && y >= 2 && x + 2 < w && y + 2 < h && (y - 2..=y + 2).all(|yy| (x - 2..=x + 2).all(|xx| pixel(kotlin, xx, yy) == k));
            if is_flat {
                flat += 1;
                flat_wrong += usize::from(spread(k, r) > 2);
            }
        }
    }
    Stats { strong: strong as f64 / (w * h) as f64, flat_wrong: flat_wrong as f64 / flat.max(1) as f64, flat_pixels: flat }
}

fn run_case(name: &str) -> (Vec<String>, Stats) {
    let g: Golden = serde_json::from_str(&read(golden_dir().join("cases").join(format!("{name}.json")))).unwrap_or_else(|e| panic!("{name}: {e}"));
    let layout = layout_from_json(&g.layout);
    let options = options_of(&g.options);
    let rust = render_image(&layout, &options).unwrap();
    if let Ok(dir) = std::env::var("VIZ_SAVE") {
        std::fs::create_dir_all(&dir).unwrap();
        rust.save_png(PathBuf::from(dir).join(format!("{}.png", g.case))).unwrap();
    }
    let kotlin = Pixmap::load_png(golden_dir().join(&g.png)).unwrap();
    let mut diffs = Vec::new();
    if (rust.width(), rust.height()) != (g.width, g.height) {
        diffs.push(format!("size: rust {}x{}, kotlin {}x{}", rust.width(), rust.height(), g.width, g.height));
        return (diffs, Stats { strong: 1.0, flat_wrong: 1.0, flat_pixels: 0 });
    }
    let b = layout.bounds();
    let stats = compare(&rust, &kotlin, ((b.h + 2) * options.px) as u32);
    (diffs, stats)
}

fn case_names() -> Vec<String> {
    let mut names: Vec<String> = files_in("rust/tests/golden/viz/cases", "json").iter().map(|p| p.file_stem().unwrap().to_string_lossy().into_owned()).collect();
    if let Ok(one) = std::env::var("VIZ_CASE") {
        names.retain(|n| *n == one);
        assert!(!names.is_empty(), "no case named {one}");
    }
    names
}

/// The most pixels of a case's map (in percent) that may differ strongly: what antialiasing and the font cost now,
/// rounded up. A shifted or wrongly coloured element would add to it.
const MAX_STRONG: [(&str, f64); 16] = [
    ("example_chemical", 8.0),
    ("example_red_green", 4.5),
    ("kit_v11_refineries", 3.5),
    ("kit_v12_pipe_to_ground", 3.0),
    ("kit_v13_splitter", 3.5),
    ("kit_v16_weaving", 4.5),
    ("kit_v17_fluid_machines", 2.0),
    ("kit_v2_lanes", 1.0),
    ("kit_v3_undergrounds", 3.5),
    ("kit_v5_poles_supply", 1.0),
    ("kit_v6_infinity", 3.5),
    ("kit_v8_inserters", 4.0),
    ("trial_errors", 4.0),
    ("trial_heat", 4.0),
    ("trial_t3_reserved", 4.0),
    ("trial_t4_test_mode", 5.0),
];

/// Of Kotlin's flat pixels, the share (in percent) Rust may paint differently.
const MAX_FLAT_WRONG: f64 = 0.1;

#[test]
fn renders_match_kotlin_in_size_and_in_flat_areas_and_stay_close_elsewhere() {
    let names = case_names();
    assert!(names.len() >= 16 || std::env::var("VIZ_CASE").is_ok(), "the render goldens are missing: {names:?}");
    let mut failures = Vec::new();
    for name in &names {
        let (diffs, stats) = run_case(name);
        println!("{name}: {:.2}% strongly different, {:.3}% of {} flat pixels wrong", stats.strong * 100.0, stats.flat_wrong * 100.0, stats.flat_pixels);
        let mut problems = diffs;
        let allowed = MAX_STRONG.iter().find(|(n, _)| n == name).map_or(6.0, |(_, p)| *p);
        if stats.strong * 100.0 > allowed {
            problems.push(format!("{:.1}% of the map's pixels differ strongly (at most {allowed:.1}%)", stats.strong * 100.0));
        }
        if stats.flat_wrong * 100.0 > MAX_FLAT_WRONG {
            problems.push(format!("{:.2}% of Kotlin's flat pixels are painted differently (at most {MAX_FLAT_WRONG:.2}%)", stats.flat_wrong * 100.0));
        }
        if !problems.is_empty() {
            failures.push(format!("{name}:\n  {}", problems.join("\n  ")));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
