//! The picture panel: a texture from the worker, zoom by scroll (around the pointer), pan by drag, fit on request.
//! The camera counts screen pixels per tile rather than per picture pixel, so the live previews at 12 px/tile and the
//! 32 px picture that ends a run show the same tiles at the same size.

use super::worker::Image;
use eframe::egui;

pub struct Preview {
    texture: Option<egui::TextureHandle>,
    /// The picture's size in tiles.
    tiles: egui::Vec2,
    /// Screen pixels per tile.
    zoom: f32,
    offset: egui::Vec2,
    /// Fit the next picture to the panel: the first one, or after a resize the user asked for.
    fit_next: bool,
}

impl Default for Preview {
    fn default() -> Self {
        Self { texture: None, tiles: egui::Vec2::ZERO, zoom: 32.0, offset: egui::Vec2::ZERO, fit_next: true }
    }
}

impl Preview {
    /// Uploads a picture; the camera only refits for the first one, so a run does not jump about as it improves.
    pub fn set(&mut self, ctx: &egui::Context, image: &Image) {
        let pixels = egui::ColorImage::from_rgba_premultiplied([image.w as usize, image.h as usize], &image.rgba);
        let px = image.px.max(1.0);
        self.tiles = egui::vec2(image.w as f32 / px, image.h as f32 / px);
        if self.texture.is_none() {
            self.fit_next = true;
        }
        self.texture = Some(ctx.load_texture("preview", pixels, egui::TextureOptions::LINEAR));
    }

    pub fn has_picture(&self) -> bool {
        self.texture.is_some()
    }

    /// Fits the picture to the panel on the next frame.
    pub fn fit(&mut self) {
        self.fit_next = true;
    }

    /// Screen pixels per tile.
    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let Some(texture) = &self.texture else {
            ui.centered_and_justified(|ui| ui.label("no picture yet: run a job and it appears here as it improves"));
            return;
        };
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        if self.fit_next {
            let fit = (rect.width() / self.tiles.x).min(rect.height() / self.tiles.y);
            self.zoom = fit.clamp(0.5, 128.0);
            self.offset = egui::Vec2::ZERO;
            self.fit_next = false;
        }
        // Zoom around the pointer: the tile under it stays put.
        if let Some(pointer) = response.hover_pos() {
            let scroll = ui.input(|input| input.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let anchor = (pointer - rect.center() - self.offset) / self.zoom;
                self.zoom = (self.zoom * (scroll * 0.002).exp()).clamp(0.5, 128.0);
                self.offset = pointer - rect.center() - anchor * self.zoom;
            }
        }
        if response.dragged() {
            self.offset += response.drag_delta();
        }
        let drawn = egui::Rect::from_center_size(rect.center() + self.offset, self.tiles * self.zoom);
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        ui.painter_at(rect).image(texture.id(), drawn, uv, egui::Color32::WHITE);
    }
}
