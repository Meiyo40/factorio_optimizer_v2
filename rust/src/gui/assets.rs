//! Factorio's own icons, read from the player's install: level B of the preview. Nothing is copied into the repo —
//! the pictures are decoded when the window first needs them and live in memory (and the GPU) after that.
//!
//! The files are 120x64: a 64x64 icon with its mipmaps packed to the right (64 + 32 + 16 + 8), so the first 64x64
//! square is the icon. Some prototypes have no file at all, which is not an error: the schematic is drawn without it.

use eframe::egui;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Where the game is, unless `FACTORIO_DIR` says otherwise: the install this was developed against.
pub const DEFAULT_DIR: &str = r"E:\SteamLibrary\steamapps\common\Factorio";

/// Icons of the entities in a layout, by prototype name.
pub struct Icons {
    dir: Option<PathBuf>,
    cache: HashMap<String, Option<egui::TextureHandle>>,
}

impl Icons {
    pub fn new() -> Self {
        let root = std::env::var_os("FACTORIO_DIR").map_or_else(|| PathBuf::from(DEFAULT_DIR), PathBuf::from);
        let dir = root.join("data/base/graphics/icons");
        Self { dir: dir.is_dir().then_some(dir), cache: HashMap::new() }
    }

    /// The directory the icons come from, or none when there is no game there.
    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// A prototype's icon, decoded once per name; none when the install has no file for it.
    pub fn icon(&mut self, ctx: &egui::Context, name: &str) -> Option<egui::TextureHandle> {
        if let Some(cached) = self.cache.get(name) {
            return cached.clone();
        }
        let decoded = self.dir.as_ref().and_then(|dir| decode(&dir.join(format!("{name}.png"))));
        let texture = decoded.map(|pixels| ctx.load_texture(name.to_owned(), pixels, egui::TextureOptions::LINEAR));
        self.cache.insert(name.to_owned(), texture.clone());
        texture
    }
}

/// The first 64x64 of an icon file, as premultiplied RGBA for `egui`.
fn decode(path: &Path) -> Option<egui::ColorImage> {
    let bytes = std::fs::read(path).ok()?;
    let image = image::load_from_memory(&bytes).ok()?;
    let (w, h) = (image.width().min(64), image.height().min(64));
    let square = image.crop_imm(0, 0, w, h).to_rgba8();
    let size = [square.width() as usize, square.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, square.as_raw()))
}
