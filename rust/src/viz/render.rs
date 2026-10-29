//! Headless PNG render of a layout: machines as labelled boxes, belts as arrows coloured per net, inserters as small
//! arrows. A statement-for-statement port of viz/Render.kt, which draws with Java2D; `Canvas` keeps the Java2D
//! habits the picture depends on (the stroke stays set until the next one, integer rectangles, `drawRect` covering
//! one pixel more than its width), and tiny-skia draws the antialiased shapes. Text comes from a bundled bitmap font
//! (Noto Sans Mono) scaled to the size Java was asked for, so it is monospace where Java's is sans-serif.

use crate::data::game_constants as gc;
use crate::geom::{Dir, Kind, Layout, PlacedEntity, Tile};
use crate::util::java_hash::JavaHash;
use crate::util::text::fmt2;
use crate::validate::{OutputSpec, SourceSpec, ValidationError};
use anyhow::{Context, Result, anyhow};
use noto_sans_mono_bitmap::{FontWeight, RasterHeight, get_raster, get_raster_width};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Rect, Stroke, StrokeDash, Transform};

type Rgba = (u8, u8, u8, u8);

const fn rgb(r: u8, g: u8, b: u8) -> Rgba {
    (r, g, b, 255)
}

/// Per-tile overlay for PathFinder debug renders: history cost as heat, overused tiles in red.
pub struct HeatOverlay {
    pub values: Vec<(Tile, f64)>,
    pub overused: HashSet<Tile>,
}

/// An input (`input`) or output port of a layout: where `item` enters or leaves, at `rate` per second. For an input
/// belt, `lane` says which lane it arrives on, looking along the belt into the block: 0 left, 1 right, -1 both (a
/// belt of its own); fluids have none.
#[derive(Debug, Clone, PartialEq)]
pub struct PortLabel {
    pub tile: Tile,
    pub item: String,
    pub rate: f64,
    pub input: bool,
    pub lane: i32,
    pub fluid: bool,
}

pub struct RenderOptions {
    pub px: i32,
    pub show_supply: bool,
    pub errors: Vec<ValidationError>,
    pub heat: Option<HeatOverlay>,
    pub title: Option<String>,
    /// Inputs and outputs to mark (numbered on their tiles, listed under the render).
    pub ports: Vec<PortLabel>,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self { px: 32, show_supply: false, errors: Vec::new(), heat: None, title: None, ports: Vec::new() }
    }
}

/// Port labels for a layout's sources and outputs (as the validator and ports.json list them).
pub fn port_labels(sources: &[SourceSpec], outputs: &[OutputSpec]) -> Vec<PortLabel> {
    let inputs = sources.iter().map(|s| PortLabel { tile: s.tile, item: s.item.clone(), rate: s.rate, input: true, lane: s.lane, fluid: s.fluid });
    let outs = outputs.iter().map(|o| PortLabel { tile: o.tile, item: o.item.clone(), rate: o.rate, input: false, lane: -1, fluid: o.fluid });
    inputs.chain(outs).collect()
}

/// `java.awt.Color.HSBtoRGB`, in `f32` like the original.
fn hsb_to_rgb(hue: f32, saturation: f32, brightness: f32) -> Rgba {
    let byte = |v: f32| (v * 255.0 + 0.5) as u8;
    if saturation == 0.0 {
        let v = byte(brightness);
        return rgb(v, v, v);
    }
    let h = (hue - hue.floor()) * 6.0;
    let f = h - h.floor();
    let p = brightness * (1.0 - saturation);
    let q = brightness * (1.0 - saturation * f);
    let t = brightness * (1.0 - saturation * (1.0 - f));
    let (r, g, b) = match h as i32 {
        0 => (brightness, t, p),
        1 => (q, brightness, p),
        2 => (p, brightness, t),
        3 => (p, q, brightness),
        4 => (t, p, brightness),
        _ => (brightness, p, q),
    };
    rgb(byte(r), byte(g), byte(b))
}

fn fluid_color(fluid: Option<&str>) -> Rgba {
    match fluid {
        None => rgb(120, 140, 160),
        Some("water") => rgb(60, 120, 230),
        Some("crude-oil") => rgb(40, 40, 40),
        Some("heavy-oil") => rgb(170, 70, 20),
        Some("light-oil") => rgb(230, 170, 30),
        Some("petroleum-gas") => rgb(150, 60, 160),
        Some("sulfuric-acid") => rgb(200, 200, 40),
        Some("lubricant") => rgb(60, 160, 60),
        Some(other) => hsb_to_rgb(((other.java_hash() & 0xffff) as f32) / 65535.0, 0.7, 0.7),
    }
}

/// Belt-family tiles grouped into chains: a belt joins the entity it outputs into; entrances join their exits.
fn belt_chains(layout: &Layout) -> HashMap<Tile, usize> {
    let belts: Vec<&PlacedEntity> = layout.entities.iter().filter(|e| e.kind.belt_family()).collect();
    let at: HashMap<Tile, &PlacedEntity> = belts.iter().map(|e| (e.tile(), *e)).collect();
    let mut parent: HashMap<Tile, Tile> = HashMap::new();
    fn find(parent: &mut HashMap<Tile, Tile>, t: Tile) -> Tile {
        let mut a = t;
        while *parent.entry(a).or_insert(a) != a {
            a = parent[&a];
        }
        a
    }
    let join = |parent: &mut HashMap<Tile, Tile>, a: Tile, b: Tile| {
        let (ra, rb) = (find(parent, a), find(parent, b));
        if ra != rb {
            parent.insert(ra, rb);
        }
    };
    for e in &belts {
        if e.kind == Kind::UgIn {
            let reach = gc::tier_of_entity(&e.name).map_or(4, |t| t.max_underground_gap) + 1;
            let exit = (1..=reach).map(|i| e.tile().step(e.dir, i)).find(|t| at.get(t).is_some_and(|o| o.name == e.name && o.dir.axis() == e.dir.axis()));
            if let Some(t) = exit
                && at[&t].kind == Kind::UgOut
                && at[&t].dir == e.dir
            {
                join(&mut parent, e.tile(), t);
            }
        } else {
            let t = e.tile() + e.dir;
            if let Some(o) = at.get(&t)
                && o.dir != e.dir.opposite()
                && o.kind != Kind::UgOut
            {
                join(&mut parent, e.tile(), t);
            }
        }
    }
    let mut ids: HashMap<Tile, usize> = HashMap::new();
    belts
        .iter()
        .map(|e| {
            let root = find(&mut parent, e.tile());
            let next = ids.len();
            (e.tile(), *ids.entry(root).or_insert(next))
        })
        .collect()
}

/// The size of the image: the layout with a one-tile margin, wide enough for the legend, tall enough for its lines.
pub fn image_size(layout: &Layout, opt: &RenderOptions) -> (i32, i32) {
    let b = layout.bounds();
    let legend_lines = i32::from(opt.title.is_some()) + opt.errors.len().min(30) as i32 + opt.ports.len() as i32;
    let line_h = (opt.px * 14 / 16).max(14);
    let width = ((b.w + 2) * opt.px).max(if legend_lines > 0 { 720 * line_h / 14 } else { 0 });
    (width, (b.h + 2) * opt.px + legend_lines * line_h + 8)
}

/// The Java2D calls Render.kt makes, on a tiny-skia pixmap. The stroke stays what it was last set to.
struct Canvas {
    pixmap: Pixmap,
    color: Rgba,
    width: f32,
    cap: LineCap,
    dash: Option<(f32, f32)>,
    clip: Option<(i32, i32, i32, i32)>,
}

impl Canvas {
    fn paint(&self, anti_alias: bool) -> Paint<'static> {
        let mut paint = Paint::default();
        paint.set_color_rgba8(self.color.0, self.color.1, self.color.2, self.color.3);
        paint.anti_alias = anti_alias;
        paint
    }

    /// `BasicStroke(width)`: square caps, mitred joins.
    fn stroke(&mut self, width: f32) {
        self.width = width;
        self.cap = LineCap::Square;
        self.dash = None;
    }

    /// `BasicStroke(width, CAP_BUTT, JOIN_MITER, 10f, floatArrayOf(on, off), 0f)`.
    fn dashed_stroke(&mut self, width: f32, on: f32, off: f32) {
        self.width = width;
        self.cap = LineCap::Butt;
        self.dash = Some((on, off));
    }

    fn skia_stroke(&self) -> Stroke {
        Stroke {
            width: self.width,
            line_cap: self.cap,
            line_join: LineJoin::Miter,
            miter_limit: 10.0,
            dash: self.dash.and_then(|(on, off)| StrokeDash::new(vec![on, off], 0.0)),
        }
    }

    /// Where a stroke along an integer coordinate really lies: Java centres a 1 px line on its pixel.
    fn lift(&self) -> f32 {
        if self.width == 1.0 { 0.5 } else { 0.0 }
    }

    fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32) {
        if let Some(rect) = (w > 0 && h > 0).then(|| Rect::from_xywh(x as f32, y as f32, w as f32, h as f32)).flatten() {
            self.pixmap.fill_rect(rect, &self.paint(false), Transform::identity(), None);
        }
    }

    fn draw_rect(&mut self, x: i32, y: i32, w: i32, h: i32) {
        let lift = self.lift();
        if let Some(rect) = Rect::from_xywh(x as f32 + lift, y as f32 + lift, w as f32, h as f32) {
            self.pixmap.stroke_path(&PathBuilder::from_rect(rect), &self.paint(true), &self.skia_stroke(), Transform::identity(), None);
        }
    }

    fn draw_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        let lift = self.lift();
        let mut pb = PathBuilder::new();
        pb.move_to(x0 as f32 + lift, y0 as f32 + lift);
        pb.line_to(x1 as f32 + lift, y1 as f32 + lift);
        if let Some(path) = pb.finish() {
            self.pixmap.stroke_path(&path, &self.paint(true), &self.skia_stroke(), Transform::identity(), None);
        }
    }

    fn fill_polygon(&mut self, xs: [i32; 3], ys: [i32; 3]) {
        let mut pb = PathBuilder::new();
        pb.move_to(xs[0] as f32, ys[0] as f32);
        pb.line_to(xs[1] as f32, ys[1] as f32);
        pb.line_to(xs[2] as f32, ys[2] as f32);
        pb.close();
        if let Some(path) = pb.finish() {
            self.pixmap.fill_path(&path, &self.paint(true), FillRule::Winding, Transform::identity(), None);
        }
    }

    fn oval(&self, x: i32, y: i32, w: i32, h: i32) -> Option<tiny_skia::Path> {
        PathBuilder::from_oval(Rect::from_xywh(x as f32, y as f32, w as f32, h as f32)?)
    }

    fn fill_oval(&mut self, x: i32, y: i32, w: i32, h: i32) {
        if let Some(path) = self.oval(x, y, w, h) {
            self.pixmap.fill_path(&path, &self.paint(true), FillRule::Winding, Transform::identity(), None);
        }
    }

    fn draw_oval(&mut self, x: i32, y: i32, w: i32, h: i32) {
        if let Some(path) = self.oval(x, y, w, h) {
            self.pixmap.stroke_path(&path, &self.paint(true), &self.skia_stroke(), Transform::identity(), None);
        }
    }

    /// `g.clipRect(x, y, w, h)` for the text drawn next; None lifts it.
    fn clip_rect(&mut self, rect: Option<(i32, i32, i32, i32)>) {
        self.clip = rect;
    }

    /// `g.drawString(text, x, baseline)` at font size `size`: the bundled glyphs, scaled bilinearly from the nearest
    /// size they exist in, blended over the (opaque) picture and cut off at the clip rectangle.
    fn draw_string(&mut self, text: &str, x: i32, baseline: i32, size: f32, bold: bool) {
        let weight = if bold { FontWeight::Bold } else { FontWeight::Regular };
        let (height, scale) = glyph_size(size);
        let ascent = baseline_row(weight, height) as f32 * scale;
        let advance = get_raster_width(weight, height) as f32 * scale;
        let (w, h) = (self.pixmap.width() as i32, self.pixmap.height() as i32);
        let (cx0, cy0, cx1, cy1) = match self.clip {
            Some((cx, cy, cw, ch)) => (cx.max(0), cy.max(0), (cx + cw).min(w), (cy + ch).min(h)),
            None => (0, 0, w, h),
        };
        let top = baseline as f32 - ascent;
        for (i, c) in text.chars().enumerate() {
            let Some(glyph) = get_raster(c, weight, height).or_else(|| get_raster('?', weight, height)) else { continue };
            let rows = glyph.raster();
            let left = x as f32 + i as f32 * advance;
            let (gw, gh) = (rows.first().map_or(0, |r| r.len()), rows.len());
            let (x0, y0) = (left.floor() as i32, top.floor() as i32);
            let (x1, y1) = ((left + gw as f32 * scale).ceil() as i32, (top + gh as f32 * scale).ceil() as i32);
            for py in y0.max(cy0)..y1.min(cy1) {
                for px in x0.max(cx0)..x1.min(cx1) {
                    // The source position of the pixel's centre, in glyph pixels.
                    let sx = ((px as f32 + 0.5 - left) / scale - 0.5).clamp(-0.5, gw as f32 - 0.5);
                    let sy = ((py as f32 + 0.5 - top) / scale - 0.5).clamp(-0.5, gh as f32 - 0.5);
                    let sample = |ix: i32, iy: i32| -> f32 {
                        if ix < 0 || iy < 0 || ix >= gw as i32 || iy >= gh as i32 { 0.0 } else { f32::from(rows[iy as usize][ix as usize]) }
                    };
                    let (fx, fy) = (sx.floor(), sy.floor());
                    let (tx, ty) = (sx - fx, sy - fy);
                    let (ix, iy) = (fx as i32, fy as i32);
                    let top_mix = sample(ix, iy) * (1.0 - tx) + sample(ix + 1, iy) * tx;
                    let bottom_mix = sample(ix, iy + 1) * (1.0 - tx) + sample(ix + 1, iy + 1) * tx;
                    let coverage = (top_mix * (1.0 - ty) + bottom_mix * ty) / 255.0 * f32::from(self.color.3) / 255.0;
                    if coverage <= 0.0 {
                        continue;
                    }
                    let idx = (py as usize * w as usize + px as usize) * 4;
                    let data = self.pixmap.data_mut();
                    for (k, channel) in [self.color.0, self.color.1, self.color.2].into_iter().enumerate() {
                        let old = f32::from(data[idx + k]);
                        data[idx + k] = (old + (f32::from(channel) - old) * coverage).round() as u8;
                    }
                }
            }
        }
    }
}

/// The bundled raster height nearest to `size`, and the factor that scales it to `size`.
fn glyph_size(size: f32) -> (RasterHeight, f32) {
    let sizes = [(RasterHeight::Size16, 16.0), (RasterHeight::Size20, 20.0), (RasterHeight::Size24, 24.0), (RasterHeight::Size32, 32.0)];
    let (height, nominal) = sizes.into_iter().min_by(|a, b| (a.1 - size).abs().total_cmp(&(b.1 - size).abs())).expect("sizes is not empty");
    (height, size / nominal)
}

/// The row just under the letters in a glyph raster (found from an 'H').
fn baseline_row(weight: FontWeight, height: RasterHeight) -> usize {
    get_raster('H', weight, height).map_or(height.val() * 4 / 5, |g| g.raster().iter().rposition(|row| row.iter().any(|&p| p > 0)).map_or(0, |last| last + 1))
}

/// Java's `fontMetrics.height` for the labels in machines, near enough: a line a bit taller than the font.
fn line_height(size: f32) -> i32 {
    (size * 1.17).ceil() as i32
}

/// Draws the layout and returns the picture.
pub fn render_image(layout: &Layout, opt: &RenderOptions) -> Result<Pixmap> {
    let b = layout.bounds();
    let px = opt.px;
    let line_h = (px * 14 / 16).max(14);
    let (width, height) = image_size(layout, opt);
    let mut pixmap = Pixmap::new(width as u32, height as u32).ok_or_else(|| anyhow!("a {width} x {height} picture does not fit"))?;
    pixmap.fill(tiny_skia::Color::from_rgba8(235, 235, 235, 255));
    let mut g = Canvas { pixmap, color: rgb(235, 235, 235), width: 1.0, cap: LineCap::Square, dash: None, clip: None };

    let ox = (1 - b.x) * px;
    let oy = (1 - b.y) * px;
    let sx = |x: f64| (f64::from(ox) + x * f64::from(px)) as i32;
    let sy = |y: f64| (f64::from(oy) + y * f64::from(px)) as i32;

    // Box and grid.
    g.color = rgb(255, 255, 255);
    g.fill_rect(sx(0.0), sy(0.0), layout.w * px, layout.h * px);
    g.color = rgb(225, 225, 225);
    for x in b.x..=b.x + b.w {
        g.draw_line(sx(f64::from(x)), sy(f64::from(b.y)), sx(f64::from(x)), sy(f64::from(b.y + b.h)));
    }
    for y in b.y..=b.y + b.h {
        g.draw_line(sx(f64::from(b.x)), sy(f64::from(y)), sx(f64::from(b.x + b.w)), sy(f64::from(y)));
    }
    // Floor marker tiles (block borders, reserved areas) under everything else.
    g.color = rgb(250, 214, 120);
    for t in &layout.tiles {
        g.fill_rect(sx(f64::from(t.x)) + 1, sy(f64::from(t.y)) + 1, px - 1, px - 1);
    }
    g.color = rgb(80, 80, 80);
    g.stroke(2.0);
    g.draw_rect(sx(0.0), sy(0.0), layout.w * px, layout.h * px);

    if let Some(heat) = &opt.heat {
        let max = heat.values.iter().map(|(_, v)| *v).fold(f64::NEG_INFINITY, f64::max).max(1e-9);
        for (t, v) in &heat.values {
            if *v <= 0.0 {
                continue;
            }
            let a = ((40.0 + 160.0 * (v / max)) as i32).clamp(0, 200);
            g.color = (255, 140, 0, a as u8);
            g.fill_rect(sx(f64::from(t.x)), sy(f64::from(t.y)), px, px);
        }
        for t in &heat.overused {
            g.color = (230, 0, 0, 170);
            g.fill_rect(sx(f64::from(t.x)), sy(f64::from(t.y)), px, px);
        }
    }

    g.stroke(1.0);

    // Without net ids (e.g. a decoded blueprint), colour belts by connected chain instead.
    let chain_of = if layout.entities.iter().any(|e| e.kind.belt_family() && e.net.is_some()) { HashMap::new() } else { belt_chains(layout) };
    let net_color = |e: &PlacedEntity| -> Rgba {
        let Some(n) = e.net.map(|n| n as usize).or_else(|| chain_of.get(&e.tile()).copied()) else { return rgb(110, 110, 110) };
        hsb_to_rgb(((n as f64 * 0.61803398875) % 1.0) as f32, 0.75, 0.8)
    };

    let arrow = |g: &mut Canvas, cx: f64, cy: f64, d: Dir, len: f64, color: Rgba, width: f32| {
        g.color = color;
        g.stroke(width);
        let (dx, dy) = (f64::from(d.dx()), f64::from(d.dy()));
        let (x0, y0) = (cx - dx * len / 2.0, cy - dy * len / 2.0);
        let (x1, y1) = (cx + dx * len / 2.0, cy + dy * len / 2.0);
        g.draw_line(sx(x0), sy(y0), sx(x1), sy(y1));
        let (hx, hy) = (dx * 0.22, dy * 0.22);
        let (px2, py2) = (-dy * 0.18, dx * 0.18);
        g.fill_polygon([sx(x1), sx(x1 - hx + px2), sx(x1 - hx - px2)], [sy(y1), sy(y1 - hy + py2), sy(y1 - hy - py2)]);
    };

    let by_tile = layout.tile_index();
    let px_f = f64::from(px);
    let scaled = |v: f64| (v * px_f) as i32;
    for e in &layout.entities {
        let (ex, ey, ew, eh) = (f64::from(e.x), f64::from(e.y), f64::from(e.w), f64::from(e.h));
        let (cx, cy) = (ex + ew / 2.0, ey + eh / 2.0);
        match e.kind {
            Kind::Machine => {
                g.color = rgb(200, 210, 230);
                let (rx, ry, rw, rh) = (sx(ex + 0.08), sy(ey + 0.08), scaled(ew - 0.16), scaled(eh - 0.16));
                g.fill_rect(rx, ry, rw, rh);
                g.color = rgb(60, 70, 100);
                g.draw_rect(rx, ry, rw, rh);
                // Machine name, then the recipe one word per line, clipped to the machine's box.
                let mut lines: Vec<&str> = vec![e.name.as_str()];
                if let Some(recipe) = &e.recipe {
                    lines.extend(recipe.split('-'));
                }
                g.clip_rect(Some((rx + 1, ry + 1, rw - 1, rh - 1)));
                let size = ((px_f * 0.42) as i32).max(7) as f32;
                let lh = line_height(size);
                for (i, line) in lines.iter().enumerate() {
                    g.draw_string(line, sx(ex + 0.15), sy(ey + 0.1) + lh * (i as i32 + 1) - 2, size, false);
                }
                g.clip_rect(None);
            }
            Kind::Belt => arrow(&mut g, cx, cy, e.dir, 0.8, net_color(e), (px as f32 / 6.0).max(2.0)),
            Kind::UgIn | Kind::UgOut => {
                g.color = net_color(e);
                g.fill_rect(sx(ex + 0.2), sy(ey + 0.2), scaled(0.6), scaled(0.6));
                arrow(&mut g, cx, cy, e.dir, 0.6, rgb(255, 255, 255), 1.5);
                if e.kind == Kind::UgIn {
                    let gap = gc::tier_of_entity(&e.name).map_or(4, |t| t.max_underground_gap);
                    let exit = (1..=gap + 1)
                        .map(|i| e.tile().step(e.dir, i))
                        .find(|t| by_tile.get(t).is_some_and(|&i| layout.entities[i].kind == Kind::UgOut && layout.entities[i].name == e.name));
                    if let Some(exit) = exit {
                        g.color = net_color(e);
                        g.dashed_stroke(1.5, 4.0, 4.0);
                        g.draw_line(sx(cx), sy(cy), sx(f64::from(exit.x) + 0.5), sy(f64::from(exit.y) + 0.5));
                    }
                }
            }
            Kind::Splitter => {
                g.color = net_color(e);
                g.fill_rect(sx(ex + 0.1), sy(ey + 0.1), scaled(ew - 0.2), scaled(eh - 0.2));
            }
            Kind::Inserter => {
                let long = e.name.starts_with("long");
                let color = if long {
                    rgb(200, 40, 40)
                } else if e.name.starts_with("fast") {
                    rgb(30, 110, 220)
                } else if e.name.starts_with("bulk") {
                    rgb(40, 160, 60)
                } else {
                    rgb(200, 160, 0)
                };
                g.color = (color.0, color.1, color.2, 60);
                g.fill_rect(sx(ex + 0.1), sy(ey + 0.1), scaled(0.8), scaled(0.8));
                // From pickup towards drop.
                arrow(&mut g, cx, cy, e.dir.opposite(), 0.7, color, 2.0 * px as f32 / 16.0);
                if long {
                    g.color = color;
                    let (p, d) = (e.pickup_tile(), e.drop_tile());
                    g.draw_line(sx(f64::from(p.x) + 0.5), sy(f64::from(p.y) + 0.5), sx(f64::from(d.x) + 0.5), sy(f64::from(d.y) + 0.5));
                }
            }
            Kind::Chest => {
                g.color = rgb(150, 100, 50);
                g.fill_rect(sx(ex + 0.15), sy(ey + 0.15), scaled(0.7), scaled(0.7));
            }
            Kind::InfinityChest => {
                g.color = rgb(230, 90, 200);
                g.fill_rect(sx(ex + 0.15), sy(ey + 0.15), scaled(0.7), scaled(0.7));
            }
            Kind::InfinityPipe => {
                g.color = if e.fluid.is_some() { fluid_color(e.fluid.as_deref()) } else { rgb(130, 60, 200) };
                g.fill_oval(sx(ex + 0.15), sy(ey + 0.15), scaled(0.7), scaled(0.7));
            }
            Kind::Pipe | Kind::PipeUg => {
                g.color = fluid_color(e.fluid.as_deref());
                g.stroke((px as f32 / 4.0).max(2.0));
                // Stubs towards neighbouring pipes of any kind (the validator decides what really joins).
                for d in Dir::ALL {
                    let other = by_tile.get(&(e.tile() + d)).map(|&i| &layout.entities[i]);
                    if other.is_some_and(|o| matches!(o.kind, Kind::Pipe | Kind::PipeUg | Kind::InfinityPipe)) && (e.kind == Kind::Pipe || d == e.dir) {
                        g.draw_line(sx(cx), sy(cy), sx(cx + f64::from(d.dx()) * 0.5), sy(cy + f64::from(d.dy()) * 0.5));
                    }
                }
                if e.kind == Kind::PipeUg {
                    g.fill_rect(sx(ex + 0.25), sy(ey + 0.25), scaled(0.5), scaled(0.5));
                    g.draw_line(sx(cx), sy(cy), sx(cx + f64::from(e.dir.dx()) * 0.5), sy(cy + f64::from(e.dir.dy()) * 0.5));
                } else {
                    g.fill_oval(sx(cx - 0.18), sy(cy - 0.18), scaled(0.36), scaled(0.36));
                }
            }
            Kind::Pole => {
                g.color = rgb(90, 60, 30);
                g.fill_oval(sx(cx - 0.25), sy(cy - 0.25), scaled(0.5), scaled(0.5));
                if opt.show_supply {
                    let s = gc::pole(&e.name)?.supply_half;
                    g.color = (90, 160, 255, 40);
                    g.fill_rect(sx(cx - s), sy(cy - s), scaled(2.0 * s), scaled(2.0 * s));
                }
            }
        }
    }

    // Wires.
    g.stroke(1.0);
    g.color = rgb(184, 115, 51);
    for &(a, c) in &layout.wires {
        let (pa, pc) = (&layout.entities[a], &layout.entities[c]);
        g.draw_line(
            sx(f64::from(pa.x) + f64::from(pa.w) / 2.0),
            sy(f64::from(pa.y) + f64::from(pa.h) / 2.0),
            sx(f64::from(pc.x) + f64::from(pc.w) / 2.0),
            sy(f64::from(pc.y) + f64::from(pc.h) / 2.0),
        );
    }

    // Errors, numbered.
    let legend_size = (line_h * 11 / 14) as f32;
    for (i, err) in opt.errors.iter().enumerate() {
        g.color = rgb(230, 0, 0);
        g.stroke(2.5);
        g.draw_rect(sx(f64::from(err.tile.x)), sy(f64::from(err.tile.y)), px, px);
        g.draw_string(&(i + 1).to_string(), sx(f64::from(err.tile.x) + 1.0), sy(f64::from(err.tile.y)), legend_size, true);
    }
    // Ports: a numbered ring on the tile, green for inputs, red for outputs.
    let (in_color, out_color) = (rgb(20, 150, 60), rgb(210, 40, 40));
    let port_size = ((px_f * 0.6) as i32).max(9) as f32;
    for (i, p) in opt.ports.iter().enumerate() {
        g.color = if p.input { in_color } else { out_color };
        g.stroke(2.5 * px as f32 / 16.0);
        let (tx, ty) = (f64::from(p.tile.x), f64::from(p.tile.y));
        g.draw_oval(sx(tx - 0.1), sy(ty - 0.1), scaled(1.2), scaled(1.2));
        g.draw_string(&(i + 1).to_string(), sx(tx + 1.05), sy(ty + 0.2), port_size, true);
    }
    let mut ty = (b.h + 2) * px + line_h;
    g.color = rgb(0, 0, 0);
    if let Some(title) = &opt.title {
        g.draw_string(title, 6, ty, legend_size, false);
        ty += line_h;
    }
    for (i, p) in opt.ports.iter().enumerate() {
        g.color = if p.input { in_color } else { out_color };
        let lane = match (p.input && !p.fluid, p.lane) {
            (false, _) => "",
            (true, 0) => "  left lane",
            (true, 1) => "  right lane",
            (true, _) => "  both lanes",
        };
        let text = format!("{}. {}  {}  {}/s  at ({}, {}){lane}", i + 1, if p.input { "in " } else { "out" }, p.item, fmt2(p.rate), p.tile.x, p.tile.y);
        g.draw_string(&text, 6, ty, legend_size, false);
        ty += line_h;
    }
    g.color = rgb(0, 0, 0);
    for (i, err) in opt.errors.iter().take(30).enumerate() {
        g.draw_string(&format!("{}. {err}", i + 1), 6, ty, legend_size, false);
        ty += line_h;
    }
    Ok(g.pixmap)
}

/// Draws the layout into a PNG file (and the folders above it).
pub fn render_layout(layout: &Layout, file: &Path, opt: &RenderOptions) -> Result<()> {
    let pixmap = render_image(layout, opt)?;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("Could not create {}", dir.display()))?;
    }
    pixmap.save_png(file).with_context(|| format!("Could not write {}", file.display()))
}
