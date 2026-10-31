//! The game view: entity sprites composed from the game's own sheets, using the geometry `distil_sprites` wrote into
//! `sprite-table.json`. The pictures stay in the player's install; only paths and frame layout are in the repository.

use crate::geom::PlacedEntity;
use eframe::egui;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Deserialize, Clone)]
struct Layer {
    file: String,
    w: f32,
    h: f32,
    columns: f32,
    frame_count: f32,
    directions: f32,
    shift: [f32; 2],
    scale: f32,
    offset: [f32; 2],
    rotate: bool,
}

#[derive(Deserialize)]
struct Entry {
    sides: [Vec<Layer>; 4],
}

#[derive(Deserialize)]
struct Table {
    game: String,
    entities: HashMap<String, Entry>,
}

/// The sheets, decoded on first use: a belt sheet is 2048x2560, so they are cached rather than reloaded per entity.
#[derive(Default)]
struct Sheets(HashMap<String, Option<(egui::TextureHandle, egui::Vec2)>>);

pub struct Sprites {
    table: Table,
    root: Option<PathBuf>,
    sheets: Sheets,
}

impl Sprites {
    /// The table is compiled in; the sheets are read from the install when a picture first needs them.
    pub fn new() -> Self {
        let table: Table = serde_json::from_str(include_str!("../../sprite-table.json")).expect("the sprite table parses");
        let root = Some(super::assets::root()).filter(|root| root.join("data").is_dir());
        Self { table, root, sheets: Sheets::default() }
    }

    pub fn game(&self) -> &str {
        &self.table.game
    }

    pub fn entities(&self) -> usize {
        self.table.entities.len()
    }

    /// Draws one entity at `origin` (the picture's top-left) with `px` screen pixels per tile; false when the table has
    /// no sprites for it, so the caller can fall back.
    pub fn draw(&mut self, painter: &egui::Painter, origin: egui::Pos2, px: f32, entity: &PlacedEntity, footprint: (f32, f32)) -> bool {
        // The layer lists are a few hundred bytes; cloning them keeps the sheet cache borrowable while drawing.
        let Some(layers) = self.table.entities.get(&entity.name).map(|entry| entry.sides[entity.dir.ordinal()].clone()) else {
            return false;
        };
        if layers.is_empty() {
            return false;
        }
        let centre = origin + egui::vec2((entity.x as f32 + footprint.0 / 2.0) * px, (entity.y as f32 + footprint.1 / 2.0) * px);
        let mut drew = false;
        for layer in layers {
            let Some((texture, sheet)) = self.sheet(painter.ctx(), &layer.file) else {
                continue;
            };
            // Directions are baked into rows: a belt sheet bakes 16 of them (north, east, south, west every fourth),
            // a pole bakes 4 across one row, a single-frame sheet has none.
            let step = if layer.directions >= 16.0 {
                4.0
            } else if layer.directions >= 4.0 {
                1.0
            } else {
                0.0
            };
            let frame = entity.dir.ordinal() as f32 * step * layer.frame_count;
            let column = frame % layer.columns;
            let row = (frame / layer.columns).floor();
            let left = layer.offset[0] + column * layer.w;
            let top = layer.offset[1] + row * layer.h;
            let uv = egui::Rect::from_min_size(egui::pos2(left / sheet.x, top / sheet.y), egui::vec2(layer.w / sheet.x, layer.h / sheet.y));
            let size = egui::vec2(layer.w, layer.h) * (layer.scale / 32.0) * px;
            let at = centre + egui::vec2(layer.shift[0] * px, layer.shift[1] * px);
            let rect = egui::Rect::from_center_size(at, size);
            if layer.rotate && entity.dir.ordinal() != 0 {
                rotated(painter, texture, rect, uv, entity.dir.ordinal() as f32 * std::f32::consts::FRAC_PI_2);
            } else {
                painter.image(texture, rect, uv, egui::Color32::WHITE);
            }
            drew = true;
        }
        drew
    }

    /// A sheet as a texture, decoded once. A missing file is not an error: the entity keeps its fallback.
    fn sheet(&mut self, ctx: &egui::Context, file: &str) -> Option<(egui::TextureId, egui::Vec2)> {
        let root = self.root.as_ref()?;
        if !self.sheets.0.contains_key(file) {
            let loaded = decode(&root.join("data").join(file)).map(|(image, size)| (ctx.load_texture(file.to_owned(), image, egui::TextureOptions::LINEAR), size));
            self.sheets.0.insert(file.to_owned(), loaded);
        }
        self.sheets.0.get(file)?.as_ref().map(|(handle, size)| (handle.id(), *size))
    }
}

fn decode(path: &std::path::Path) -> Option<(egui::ColorImage, egui::Vec2)> {
    let bytes = std::fs::read(path).ok()?;
    let image = image::load_from_memory(&bytes).ok()?;
    let size = egui::vec2(image.width() as f32, image.height() as f32);
    let rgba = image.to_rgba8();
    let pixels = egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw());
    Some((pixels, size))
}

/// Draws a texture rotated about its centre, which is what the game does to a sprite that carries no baked directions
/// (an inserter's platform, a pipe's stub).
fn rotated(painter: &egui::Painter, texture: egui::TextureId, rect: egui::Rect, uv: egui::Rect, angle: f32) {
    let (sin, cos) = angle.sin_cos();
    let centre = rect.center();
    let (hw, hh) = (rect.width() / 2.0, rect.height() / 2.0);
    let corner = |x: f32, y: f32| egui::pos2(centre.x + x * cos - y * sin, centre.y + x * sin + y * cos);
    let corners = [corner(-hw, -hh), corner(hw, -hh), corner(hw, hh), corner(-hw, hh)];
    let uvs = [
        egui::pos2(uv.min.x, uv.min.y),
        egui::pos2(uv.max.x, uv.min.y),
        egui::pos2(uv.max.x, uv.max.y),
        egui::pos2(uv.min.x, uv.max.y),
    ];
    let mut mesh = egui::Mesh::with_texture(texture);
    mesh.vertices = corners.iter().zip(uvs).map(|(pos, uv)| egui::epaint::Vertex { pos: *pos, uv, color: egui::Color32::WHITE }).collect();
    mesh.indices = vec![0, 1, 2, 0, 2, 3];
    painter.add(egui::Shape::mesh(mesh));
}
