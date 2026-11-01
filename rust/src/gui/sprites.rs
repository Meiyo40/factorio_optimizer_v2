//! The game's own pictures for the in-game view. The geometry (which file, where frame 0 sits in it, its size, scale
//! and shift) comes from the prototype dump `factorio --dump-data` writes; the pixels come from the install. Nothing is
//! copied into the repository, and both always come from the same game version.
//!
//! `load` runs on a background thread: the dump is 31 MB, and the sheets are big (a turbo belt sheet is 8192 px wide),
//! so it reads each sheet once, cuts out frame 0 of every picture the view can need, and keeps only the cuts.
//!
//! Each prototype becomes a `Proto` whose parts are named the way the view asks for them:
//! - belt-family entities: their tier's 20 belt rows by index name (`east_to_north_index`, see `belts`);
//! - undergrounds: `in/<side>`, `out/<side>`, `in_side/<side>`, `out_side/<side>`, `back/<side>`, `front/<side>`, the
//!   side being the structure sheet's column;
//! - splitters: `structure/<side>`, `patch/<side>`;
//! - inserters: the platform by side, `hand_base`, `hand_open`, and `pickup`;
//! - pipes: the game's shape names (`corner_up_right`, ...), named after the sides they connect;
//! - everything else: its picture by side (`north`...), the side being the entity's blueprint direction.

use super::belts::{BELT_ROWS, side};
use crate::data::game_constants as gc;
use crate::geom::Dir;
use anyhow::{Context, Result};
use eframe::egui;
use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Where the game is, unless `FACTORIO_DIR` says otherwise: the install this was developed against.
pub const DEFAULT_DIR: &str = r"E:\SteamLibrary\steamapps\common\Factorio";

/// The pipe pictures, named after the sides they connect.
pub const PIPE_SHAPES: [&str; 16] = [
    "straight_vertical_single",
    "ending_up",
    "ending_right",
    "ending_down",
    "ending_left",
    "straight_vertical",
    "straight_horizontal",
    "corner_up_right",
    "corner_up_left",
    "corner_down_right",
    "corner_down_left",
    "t_up",
    "t_down",
    "t_right",
    "t_left",
    "cross",
];

/// The install's root: `FACTORIO_DIR` when it is set, the default above otherwise.
pub fn install_dir() -> PathBuf {
    std::env::var_os("FACTORIO_DIR").map_or_else(|| PathBuf::from(DEFAULT_DIR), PathBuf::from)
}

/// The prototype dump: `FACTORIO_DUMP` when it is set, else where `factorio --dump-data` writes it.
pub fn dump_path() -> PathBuf {
    if let Some(path) = std::env::var_os("FACTORIO_DUMP") {
        return PathBuf::from(path);
    }
    let appdata = std::env::var_os("APPDATA").map_or_else(PathBuf::new, PathBuf::from);
    appdata.join("Factorio").join("script-output").join("data-raw-dump.json")
}

/// A file named the game's way (`__base__/graphics/x.png`) in the install.
pub fn asset(root: &Path, file: &str) -> PathBuf {
    match file.strip_prefix("__").and_then(|rest| rest.split_once("__/")) {
        Some((module, rest)) => root.join("data").join(module).join(rest),
        None => root.join(file),
    }
}

/// Frame 0 of one sprite layer: which cut it is, its size in tiles and how far its centre sits from the entity's.
#[derive(Clone, Debug)]
pub struct Frame {
    pub image: usize,
    pub size: egui::Vec2,
    pub shift: egui::Vec2,
    pub shadow: bool,
    pub tint: egui::Color32,
}

#[derive(Default)]
pub struct Proto {
    pub parts: HashMap<String, Vec<Frame>>,
    /// Inserters: where the hand picks up, in tiles from the centre, for one facing north.
    pub pickup: Option<egui::Vec2>,
}

impl Proto {
    pub fn part(&self, name: &str) -> &[Frame] {
        self.parts.get(name).map_or(&[], Vec::as_slice)
    }
}

/// An item's or fluid's icon: the file and the size of its first mip, which sits at the top-left.
#[derive(Clone, Debug)]
pub struct Icon {
    pub file: PathBuf,
    pub size: u32,
}

pub struct Catalog {
    pub protos: HashMap<String, Proto>,
    /// The cuts the frames point at; None where a file was missing or too small for the frame.
    pub images: Vec<Option<egui::ColorImage>>,
    pub icons: HashMap<String, Icon>,
    /// Prototypes the view will draw as boxes, and files it could not read: said once, in the window.
    pub missing: Vec<String>,
}

/// Reads the dump and the sheets. Slow (about a second): call it off the window's thread.
pub fn load() -> Result<Catalog> {
    let root = install_dir();
    anyhow::ensure!(root.join("data").is_dir(), "no game at {} (set FACTORIO_DIR to the install)", root.display());
    let dump = dump_path();
    let text = std::fs::read_to_string(&dump)
        .with_context(|| format!("no prototype dump at {} (run `factorio --dump-data`, or set FACTORIO_DUMP)", dump.display()))?;
    // Only the prototypes the view draws are parsed; the rest of the dump is skipped as raw text.
    let groups: HashMap<String, HashMap<String, &RawValue>> = serde_json::from_str(&text).context("the dump parses")?;

    let mut cuts = Cuts::default();
    let mut protos = HashMap::new();
    let mut missing = Vec::new();
    for name in names() {
        let Some(prototype) = entity(&groups, &name) else {
            missing.push(name);
            continue;
        };
        let proto = proto(&mut cuts, &name, &prototype);
        if proto.parts.is_empty() {
            missing.push(name);
        } else {
            protos.insert(name, proto);
        }
    }
    let images = cuts.cut(&root, &mut missing);
    // A frame whose cut failed is dropped, so a part either draws whole or falls back to the box.
    for proto in protos.values_mut() {
        for frames in proto.parts.values_mut() {
            frames.retain(|frame| images[frame.image].is_some());
        }
        proto.parts.retain(|_, frames| !frames.is_empty());
    }
    Ok(Catalog { protos, images, icons: icons(&groups, &root), missing })
}

/// Every entity a layout can hold.
fn names() -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for tier in gc::BELT_TIERS.iter() {
        names.extend([tier.belt, tier.underground, tier.splitter].map(str::to_owned));
    }
    names.extend(gc::INSERTERS.iter().map(|i| i.name.to_owned()));
    names.extend(gc::supported_machines().map(str::to_owned));
    names.extend(gc::POLES.iter().map(|p| p.name.to_owned()));
    names.extend(gc::CHESTS.iter().map(|c| (*c).to_owned()));
    names.extend([gc::PIPE, gc::PIPE_TO_GROUND, gc::INFINITY_PIPE].map(str::to_owned));
    names
}

/// The entity prototype called `name`: names are shared with items and recipes, and only the entity has a
/// collision box.
fn entity(groups: &HashMap<String, HashMap<String, &RawValue>>, name: &str) -> Option<Value> {
    groups
        .values()
        .filter_map(|group| group.get(name))
        .filter_map(|raw| serde_json::from_str::<Value>(raw.get()).ok())
        .find(|prototype| prototype.get("collision_box").is_some())
}

fn proto(cuts: &mut Cuts, name: &str, p: &Value) -> Proto {
    let mut proto = Proto::default();
    let mut put = |key: String, frames: Vec<Frame>| {
        if !frames.is_empty() {
            proto.parts.insert(key, frames);
        }
    };
    if let Some(set) = p.get("belt_animation_set") {
        for (n, row_name) in BELT_ROWS.iter().enumerate() {
            // The game numbers rows from 1, and a prototype may move any of them.
            let row = set.get(*row_name).and_then(Value::as_u64).map_or(n as u64, |index| index.saturating_sub(1));
            if let Some(frame) = set.get("animation_set").and_then(|sheet| frame(cuts, sheet, row as u32)) {
                put((*row_name).to_owned(), vec![frame]);
            }
        }
    }
    if let Some(structure) = p.get("structure") {
        if p.get("belt_animation_set").is_some() && structure.get("direction_in").is_some() {
            for d in Dir::ALL {
                for (key, part) in [
                    ("in", "direction_in"),
                    ("out", "direction_out"),
                    ("in_side", "direction_in_side_loading"),
                    ("out_side", "direction_out_side_loading"),
                    ("back", "back_patch"),
                    ("front", "front_patch"),
                ] {
                    put(format!("{key}/{}", side(d)), structure.get(part).map(|v| frames(cuts, v, d)).unwrap_or_default());
                }
            }
        } else {
            for d in Dir::ALL {
                put(format!("structure/{}", side(d)), frames(cuts, structure, d));
                put(format!("patch/{}", side(d)), p.get("structure_patch").map(|v| frames(cuts, v, d)).unwrap_or_default());
            }
        }
    }
    if let Some(platform) = p.get("platform_picture") {
        for d in Dir::ALL {
            put(side(d).to_owned(), frames(cuts, platform, d));
        }
        put("hand_base".to_owned(), p.get("hand_base_picture").map(|v| frames(cuts, v, Dir::N)).unwrap_or_default());
        put("hand_open".to_owned(), p.get("hand_open_picture").map(|v| frames(cuts, v, Dir::N)).unwrap_or_default());
        proto.pickup = vec2(p.get("pickup_position"));
        return proto;
    }
    if name == gc::PIPE || name == gc::INFINITY_PIPE {
        if let Some(pictures) = p.get("pictures") {
            for shape in PIPE_SHAPES {
                put(shape.to_owned(), pictures.get(shape).map(|v| frames(cuts, v, Dir::N)).unwrap_or_default());
            }
        }
        return proto;
    }
    if p.get("belt_animation_set").is_none() {
        // Machines, furnaces, chests, poles, pipe-to-ground: the first picture the prototype has, by side.
        let picture = ["graphics_set.animation", "graphics_set.idle_animation", "animation", "picture", "pictures"]
            .iter()
            .find_map(|path| path.split('.').try_fold(p, |v, key| v.get(key)));
        if let Some(picture) = picture {
            for d in Dir::ALL {
                put(side(d).to_owned(), frames(cuts, picture, d));
            }
        }
    }
    proto
}

/// Frame 0 of every layer of `v`, facing `d`: a `layers` list, a picture keyed by side, a four-way `sheet` (one frame
/// per side across), or a single sprite or animation.
fn frames(cuts: &mut Cuts, v: &Value, d: Dir) -> Vec<Frame> {
    if let Some(layers) = v.get("layers").and_then(Value::as_array) {
        return layers.iter().flat_map(|layer| frames(cuts, layer, d)).collect();
    }
    if let Some(by_side) = v.get(side(d)).or_else(|| v.get("north")) {
        return frames(cuts, by_side, d);
    }
    if let Some(sheet) = v.get("sheet") {
        return sheet_frame(cuts, sheet, d).into_iter().collect();
    }
    if let Some(sheets) = v.get("sheets").and_then(Value::as_array) {
        return sheets.iter().filter_map(|sheet| sheet_frame(cuts, sheet, d)).collect();
    }
    frame(cuts, v, 0).into_iter().collect()
}

/// One side of a four-way sheet: the sides sit across, north first.
fn sheet_frame(cuts: &mut Cuts, sheet: &Value, d: Dir) -> Option<Frame> {
    let (w, _) = frame_size(sheet)?;
    let mut f = frame_at(cuts, sheet, d.ordinal() as u32 * w, 0)?;
    // A sheet's shift may differ per side.
    if let Some(shifts) = sheet.get("shifts").and_then(Value::as_array) {
        f.shift = vec2(shifts.get(d.ordinal())).unwrap_or(f.shift);
    }
    Some(f)
}

/// Frame 0 of direction `direction` of a sprite or an animation: frames run along rows of `line_length` (all of a
/// direction's frames by default), the directions one after another.
fn frame(cuts: &mut Cuts, v: &Value, direction: u32) -> Option<Frame> {
    let (w, h) = frame_size(v)?;
    let frames = v.get("frame_count").and_then(Value::as_u64).unwrap_or(1).max(1) as u32;
    let columns = v.get("line_length").and_then(Value::as_u64).filter(|&n| n > 0).map_or(frames, |n| n as u32);
    let index = direction * frames;
    frame_at(cuts, v, (index % columns) * w, (index / columns) * h)
}

fn frame_at(cuts: &mut Cuts, v: &Value, x: u32, y: u32) -> Option<Frame> {
    if ["draw_as_glow", "draw_as_light"].iter().any(|flag| v.get(*flag).and_then(Value::as_bool).unwrap_or(false)) {
        return None;
    }
    let file = v
        .get("filename")
        .or_else(|| v.get("filenames").and_then(|files| files.get(0)))
        .or_else(|| v.get("stripes").and_then(|stripes| stripes.get(0)).and_then(|stripe| stripe.get("filename")))?
        .as_str()?;
    let (w, h) = frame_size(v)?;
    // `__core__/graphics/empty.png` and its like stand for "no picture".
    if w <= 1 || h <= 1 {
        return None;
    }
    let offset = |key: &str| v.get(key).and_then(Value::as_u64).unwrap_or(0) as u32;
    let scale = v.get("scale").and_then(Value::as_f64).unwrap_or(1.0) as f32;
    Some(Frame {
        image: cuts.request(file, [offset("x") + x, offset("y") + y, w, h]),
        size: egui::vec2(w as f32, h as f32) * scale / 32.0,
        shift: vec2(v.get("shift")).unwrap_or(egui::Vec2::ZERO),
        shadow: v.get("draw_as_shadow").and_then(Value::as_bool).unwrap_or(false),
        tint: tint(v.get("tint")),
    })
}

fn frame_size(v: &Value) -> Option<(u32, u32)> {
    let number = |key: &str| v.get(key).and_then(Value::as_u64).map(|n| n as u32);
    let size = v.get("size");
    let (sw, sh) = match size {
        Some(Value::Array(pair)) => (pair.first().and_then(Value::as_u64), pair.get(1).and_then(Value::as_u64)),
        Some(n) => (n.as_u64(), n.as_u64()),
        None => (None, None),
    };
    Some((number("width").or(sw.map(|n| n as u32))?, number("height").or(sh.map(|n| n as u32))?))
}

/// `[x, y]` or `{x, y}`, in tiles.
fn vec2(v: Option<&Value>) -> Option<egui::Vec2> {
    let v = v?;
    let at = |i: usize, key: &str| v.get(i).or_else(|| v.get(key)).and_then(Value::as_f64);
    Some(egui::vec2(at(0, "x")? as f32, at(1, "y")? as f32))
}

/// A tint as the game writes it: `[r, g, b, a]` or `{r, g, b, a}`, in 0..1 or 0..255.
fn tint(v: Option<&Value>) -> egui::Color32 {
    let Some(v) = v else { return egui::Color32::WHITE };
    let at = |i: usize, key: &str| v.get(i).or_else(|| v.get(key)).and_then(Value::as_f64);
    let mut rgba = [at(0, "r"), at(1, "g"), at(2, "b"), at(3, "a")].map(|c| c.unwrap_or(1.0) as f32);
    if rgba.iter().any(|&c| c > 1.0) {
        rgba = rgba.map(|c| c / 255.0);
    }
    let [r, g, b, a] = rgba.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
    egui::Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// Item and fluid icons by name. An item is the prototype with a stack size; a layered icon gives its first layer.
fn icons(groups: &HashMap<String, HashMap<String, &RawValue>>, root: &Path) -> HashMap<String, Icon> {
    #[derive(Deserialize)]
    struct Layer {
        icon: String,
        icon_size: Option<f64>,
    }
    #[derive(Deserialize)]
    struct IconRef {
        icon: Option<String>,
        icons: Option<Vec<Layer>>,
        icon_size: Option<f64>,
        stack_size: Option<f64>,
    }
    let mut icons = HashMap::new();
    for (group, prototypes) in groups {
        for (name, raw) in prototypes {
            let Ok(found) = serde_json::from_str::<IconRef>(raw.get()) else { continue };
            if found.stack_size.is_none() && group != "fluid" {
                continue;
            }
            let first = found.icons.as_ref().and_then(|layers| layers.first());
            let Some(file) = found.icon.as_deref().or(first.map(|layer| layer.icon.as_str())) else { continue };
            let size = found.icon_size.or(first.and_then(|layer| layer.icon_size)).unwrap_or(64.0);
            icons.insert(name.clone(), Icon { file: asset(root, file), size: size as u32 });
        }
    }
    icons
}

/// The rectangles to cut, each file read once: a request hands out an index, `cut` fills them in that order.
#[derive(Default)]
struct Cuts {
    wanted: Vec<(String, [u32; 4])>,
    seen: HashMap<(String, [u32; 4]), usize>,
}

impl Cuts {
    fn request(&mut self, file: &str, rect: [u32; 4]) -> usize {
        let key = (file.to_owned(), rect);
        if let Some(&index) = self.seen.get(&key) {
            return index;
        }
        self.wanted.push(key.clone());
        self.seen.insert(key, self.wanted.len() - 1);
        self.wanted.len() - 1
    }

    fn cut(&self, root: &Path, missing: &mut Vec<String>) -> Vec<Option<egui::ColorImage>> {
        let mut images: Vec<Option<egui::ColorImage>> = vec![None; self.wanted.len()];
        let mut by_file: HashMap<&str, Vec<usize>> = HashMap::new();
        for (index, (file, _)) in self.wanted.iter().enumerate() {
            by_file.entry(file.as_str()).or_default().push(index);
        }
        for (file, indices) in by_file {
            let Ok(sheet) = image::open(asset(root, file)).map(|image| image.to_rgba8()) else {
                missing.push(file.to_owned());
                continue;
            };
            for index in indices {
                let [x, y, w, h] = self.wanted[index].1;
                if x + w > sheet.width() || y + h > sheet.height() {
                    missing.push(format!("{file} ({x},{y} {w}x{h})"));
                    continue;
                }
                let piece = image::imageops::crop_imm(&sheet, x, y, w, h).to_image();
                images[index] = Some(egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], piece.as_raw()));
            }
        }
        images
    }
}

/// An icon's first mip, as the window uploads it.
pub fn icon_image(icon: &Icon) -> Option<egui::ColorImage> {
    let image = image::open(&icon.file).ok()?.to_rgba8();
    let size = icon.size.min(image.width()).min(image.height());
    let square = image::imageops::crop_imm(&image, 0, 0, size, size).to_image();
    Some(egui::ColorImage::from_rgba_unmultiplied([size as usize, size as usize], square.as_raw()))
}
