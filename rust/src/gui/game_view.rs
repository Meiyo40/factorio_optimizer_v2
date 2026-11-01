//! The in-game view: the layout drawn with the game's own pictures (`sprites`), on the camera the schematic uses.
//!
//! A layout is turned into quads once (when the window shows a new trial), in the order the game draws its layers:
//! belt surfaces, the lips of open belt ends, shadows, then objects from north to south so a tall sprite overlaps the
//! one behind it, inserter hands, and last the alt-mode icon of each machine's product. Which belt picture a tile takes
//! is `belts`' business. Anything without a picture is a plain box with its own icon, so a missing sprite shows as a
//! box, not as a hole.

use super::belts::{Hood, belt_sprites, side};
use super::sprites::{self, Catalog, Frame, Proto};
use crate::data::data_model::RootData;
use crate::geom::{Dir, Kind, Layout, PlacedEntity};
use crate::search::trial::TrialResult;
use anyhow::Result;
use eframe::egui;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::Receiver;

/// Mipmaps keep a sprite drawn at a fraction of its size from shimmering when the camera is zoomed out.
const TEXTURES: egui::TextureOptions = egui::TextureOptions { mipmap_mode: Some(egui::TextureFilter::Linear), ..egui::TextureOptions::LINEAR };
/// Outside the layout's box, and inside it.
const GROUND: egui::Color32 = egui::Color32::from_rgb(38, 36, 32);
const FLOOR: egui::Color32 = egui::Color32::from_rgb(62, 60, 55);
const BOX: egui::Color32 = egui::Color32::from_rgb(110, 112, 120);
/// How dark shadows lie on the ground.
const SHADOW: egui::Color32 = egui::Color32::from_rgba_premultiplied(150, 150, 150, 150);

enum State {
    Idle,
    Loading(Receiver<Result<Catalog>>),
    Ready(Box<Loaded>),
    Failed(String),
}

struct Loaded {
    catalog: Catalog,
    textures: Vec<Option<egui::TextureHandle>>,
    /// Icons are read when a layout first needs them: a few per layout, against hundreds the dump knows.
    icons: HashMap<String, Option<egui::TextureHandle>>,
}

/// A picture on the map, its corners in tiles (x right, y down; tile (0, 0) spans 0..1), clockwise from the one
/// that takes the picture's top-left.
struct Quad {
    texture: egui::TextureId,
    corners: [egui::Pos2; 4],
    uv: egui::Rect,
    color: egui::Color32,
}

/// An alt-mode icon: what to draw, where, how wide, in tiles.
struct Mark {
    texture: egui::TextureId,
    centre: egui::Pos2,
    size: f32,
}

pub struct GameView {
    state: State,
    /// The trial the quads were made for (a trial is never changed once it is sent), and the quads.
    scene: Option<(usize, Vec<Quad>, Vec<Mark>)>,
}

impl Default for GameView {
    fn default() -> Self {
        Self { state: State::Idle, scene: None }
    }
}

impl GameView {
    /// Starts reading the game's pictures the first time it is asked, then says whether they are ready: `Ok(false)`
    /// while they load, an error that says where it looked when they cannot be read.
    pub fn ready(&mut self, ctx: &egui::Context) -> Result<bool, String> {
        if let State::Idle = self.state {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(sprites::load());
            });
            self.state = State::Loading(rx);
        }
        if let State::Loading(rx) = &self.state {
            match rx.try_recv() {
                Ok(Ok(mut catalog)) => {
                    let images = std::mem::take(&mut catalog.images);
                    let textures = images
                        .into_iter()
                        .enumerate()
                        .map(|(n, image)| image.map(|image| ctx.load_texture(format!("game-{n}"), image, TEXTURES)))
                        .collect();
                    self.state = State::Ready(Box::new(Loaded { catalog, textures, icons: HashMap::new() }));
                }
                Ok(Err(e)) => self.state = State::Failed(format!("{e:#}")),
                Err(std::sync::mpsc::TryRecvError::Empty) => return Ok(false),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.state = State::Failed("reading the game's pictures stopped".to_owned()),
            }
        }
        match &self.state {
            State::Ready(_) => Ok(true),
            State::Failed(e) => Err(e.clone()),
            _ => Ok(false),
        }
    }

    /// What the view draws as boxes for want of a picture, when anything.
    pub fn missing(&self) -> Option<String> {
        let State::Ready(loaded) = &self.state else { return None };
        (!loaded.catalog.missing.is_empty()).then(|| loaded.catalog.missing.join(", "))
    }

    /// Draws `trial`'s layout with `origin` the screen position of tile (0, 0)'s top-left and `zoom` screen pixels a
    /// tile; `scene` is the ground's rectangle on screen.
    pub fn paint(&mut self, painter: &egui::Painter, trial: &Arc<TrialResult>, data: &RootData, scene: egui::Rect, origin: egui::Pos2, zoom: f32) {
        let State::Ready(loaded) = &mut self.state else { return };
        let Some(layout) = trial.layout.as_ref() else { return };
        let key = Arc::as_ptr(trial) as usize;
        if self.scene.as_ref().is_none_or(|(shown, _, _)| *shown != key) {
            let (quads, marks) = compose(painter.ctx(), loaded, layout, data);
            self.scene = Some((key, quads, marks));
        }
        let Some((_, quads, marks)) = &self.scene else { return };

        let at = |p: egui::Pos2| origin + p.to_vec2() * zoom;
        painter.rect_filled(scene, 0.0, GROUND);
        painter.rect_filled(egui::Rect::from_min_max(at(egui::Pos2::ZERO), at(egui::pos2(layout.w as f32, layout.h as f32))), 0.0, FLOOR);
        let clip = painter.clip_rect();
        let mut mesh = egui::Mesh::default();
        for quad in quads {
            let corners = quad.corners.map(at);
            if !clip.intersects(egui::Rect::from_points(&corners)) {
                continue;
            }
            if mesh.texture_id != quad.texture {
                if !mesh.is_empty() {
                    painter.add(egui::Shape::mesh(std::mem::take(&mut mesh)));
                }
                mesh.texture_id = quad.texture;
            }
            let uvs = [quad.uv.left_top(), quad.uv.right_top(), quad.uv.right_bottom(), quad.uv.left_bottom()];
            let first = mesh.vertices.len() as u32;
            for (pos, uv) in corners.into_iter().zip(uvs) {
                mesh.vertices.push(egui::epaint::Vertex { pos, uv, color: quad.color });
            }
            mesh.indices.extend([first, first + 1, first + 2, first, first + 2, first + 3]);
        }
        if !mesh.is_empty() {
            painter.add(egui::Shape::mesh(mesh));
        }
        let whole = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        for mark in marks {
            let centre = at(mark.centre);
            let radius = mark.size * zoom / 2.0;
            painter.circle_filled(centre, radius, egui::Color32::from_black_alpha(170));
            painter.image(mark.texture, egui::Rect::from_center_size(centre, egui::Vec2::splat(radius * 1.6)), whole, egui::Color32::WHITE);
        }
    }
}

/// Every quad of a layout, in drawing order, and the alt-mode icons to put over them.
fn compose(ctx: &egui::Context, loaded: &mut Loaded, layout: &Layout, data: &RootData) -> (Vec<Quad>, Vec<Mark>) {
    let protos = &loaded.catalog.protos;
    let textures = &loaded.textures;
    let texture = |frame: &Frame| textures[frame.image].as_ref().map(egui::TextureHandle::id);
    let belts = belt_sprites(layout);

    let mut surfaces = Vec::new();
    for piece in &belts.pieces {
        let Some(proto) = protos.get(&layout.entities[piece.entity].name) else { continue };
        let centre = tile_centre(piece.tile.x, piece.tile.y);
        for frame in proto.part(&piece.row) {
            let Some(id) = texture(frame) else { continue };
            let (rect, uv) = placed(frame, centre);
            let (rect, uv) = match piece.half {
                Some(open) => half(rect, uv, centre, open),
                None => (rect, uv),
            };
            surfaces.push(quad(id, rect, uv, frame.tint));
        }
    }
    let mut lips = Vec::new();
    for cap in &belts.caps {
        let Some(proto) = protos.get(&layout.entities[cap.entity].name) else { continue };
        for frame in proto.part(&cap.row) {
            let Some(id) = texture(frame) else { continue };
            let (rect, uv) = placed(frame, tile_centre(cap.tile.x, cap.tile.y));
            lips.push(quad(id, rect, uv, frame.tint));
        }
    }
    let hoods: HashMap<usize, &Hood> = belts.hoods.iter().map(|hood| (hood.entity, hood)).collect();
    let tiles = layout.tile_index();

    let mut shadows = Vec::new();
    let mut objects: Vec<((i32, i32), Vec<Quad>)> = Vec::new();
    let mut arms = Vec::new();
    let mut products: Vec<(egui::Pos2, f32, String)> = Vec::new();
    let mut boxes: Vec<(egui::Pos2, f32, String)> = Vec::new();
    for (i, e) in layout.entities.iter().enumerate() {
        let centre = egui::pos2(e.x as f32 + e.w as f32 / 2.0, e.y as f32 + e.h as f32 / 2.0);
        let proto = protos.get(&e.name);
        let parts: Vec<String> = match e.kind {
            // A belt is all surface; it is a box only when its tier has no sheet.
            Kind::Belt => Vec::new(),
            Kind::UgIn | Kind::UgOut => {
                let hood = hoods.get(&i).expect("every underground has a hood");
                let variant = match (e.kind, hood.side_loading) {
                    (Kind::UgIn, false) => "in",
                    (Kind::UgIn, true) => "in_side",
                    (_, false) => "out",
                    (_, true) => "out_side",
                };
                let column = side(hood.column);
                vec![format!("back/{column}"), format!("{variant}/{column}"), format!("front/{column}")]
            }
            Kind::Splitter => vec![format!("patch/{}", side(e.dir)), format!("structure/{}", side(e.dir))],
            Kind::Pipe | Kind::InfinityPipe => vec![pipe_shape(layout, &tiles, e).to_owned()],
            Kind::Pole => vec!["north".to_owned()],
            _ => vec![side(e.dir).to_owned()],
        };
        let frames: Vec<&Frame> = proto.map(|proto| parts.iter().flat_map(|part| proto.part(part)).collect()).unwrap_or_default();
        let drawn = if e.kind == Kind::Belt { proto.is_some() } else { !frames.is_empty() };
        let mut own = Vec::new();
        if drawn {
            for frame in frames {
                let Some(id) = texture(frame) else { continue };
                let (rect, uv) = placed(frame, centre);
                if frame.shadow {
                    shadows.push(quad(id, rect, uv, SHADOW));
                } else {
                    own.push(quad(id, rect, uv, frame.tint));
                }
            }
        } else {
            let rect = egui::Rect::from_min_size(egui::pos2(e.x as f32, e.y as f32), egui::vec2(e.w as f32, e.h as f32)).shrink(0.08);
            own.push(quad(egui::TextureId::default(), rect, egui::Rect::from_min_max(egui::epaint::WHITE_UV, egui::epaint::WHITE_UV), BOX));
            boxes.push((centre, e.w.min(e.h) as f32 * 0.7, e.name.clone()));
        }
        if e.kind == Kind::Inserter
            && let Some(proto) = proto
        {
            arms.extend(arm(proto, e, centre, &texture));
        }
        if e.kind == Kind::Machine {
            let recipe = e.recipe.as_ref().and_then(|recipe| data.recipes.get(recipe));
            if let Some(product) = recipe.and_then(|r| r.main_product.as_ref().or(r.products.first())) {
                products.push((centre, e.w.min(e.h) as f32 * 0.55, product.name.clone()));
            }
        }
        objects.push(((e.y + e.h, e.x), own));
    }
    // North to south, then west to east, as the game sorts what stands on the ground.
    objects.sort_by_key(|(at, _)| *at);

    let mut marks = Vec::new();
    for (centre, size, name) in products.into_iter().chain(boxes) {
        if let Some(texture) = icon(ctx, loaded, &name) {
            marks.push(Mark { texture, centre, size });
        }
    }
    let quads = surfaces.into_iter().chain(lips).chain(shadows).chain(objects.into_iter().flat_map(|(_, quads)| quads)).chain(arms).collect();
    (quads, marks)
}

/// An item's or fluid's icon as a texture, read on first use.
fn icon(ctx: &egui::Context, loaded: &mut Loaded, name: &str) -> Option<egui::TextureId> {
    if !loaded.icons.contains_key(name) {
        let image = loaded.catalog.icons.get(name).and_then(sprites::icon_image);
        let texture = image.map(|image| ctx.load_texture(format!("icon-{name}"), image, TEXTURES));
        loaded.icons.insert(name.to_owned(), texture);
    }
    loaded.icons.get(name)?.as_ref().map(egui::TextureHandle::id)
}

fn tile_centre(x: i32, y: i32) -> egui::Pos2 {
    egui::pos2(x as f32 + 0.5, y as f32 + 0.5)
}

/// Where a frame lies when its entity is centred on `centre`, and the whole of its picture.
fn placed(frame: &Frame, centre: egui::Pos2) -> (egui::Rect, egui::Rect) {
    let rect = egui::Rect::from_center_size(centre + frame.shift, frame.size);
    (rect, egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)))
}

/// The part of a picture on side `open` of a line through `centre`: the open side of an underground's hood.
fn half(rect: egui::Rect, uv: egui::Rect, centre: egui::Pos2, open: Dir) -> (egui::Rect, egui::Rect) {
    let (mut rect, mut uv) = (rect, uv);
    let tx = ((centre.x - rect.min.x) / rect.width()).clamp(0.0, 1.0);
    let ty = ((centre.y - rect.min.y) / rect.height()).clamp(0.0, 1.0);
    let ux = uv.min.x + tx * uv.width();
    let uy = uv.min.y + ty * uv.height();
    match open {
        Dir::N => (rect.max.y, uv.max.y) = (centre.y, uy),
        Dir::S => (rect.min.y, uv.min.y) = (centre.y, uy),
        Dir::W => (rect.max.x, uv.max.x) = (centre.x, ux),
        Dir::E => (rect.min.x, uv.min.x) = (centre.x, ux),
    }
    (rect, uv)
}

fn quad(texture: egui::TextureId, rect: egui::Rect, uv: egui::Rect, color: egui::Color32) -> Quad {
    Quad { texture, corners: [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()], uv, color }
}

/// A picture stood on the line from `from` to `to`, its top toward `to`, `width` across.
fn along(texture: egui::TextureId, from: egui::Pos2, to: egui::Pos2, width: f32) -> Quad {
    let up = (to - from).normalized();
    let right = egui::vec2(-up.y, up.x) * width / 2.0;
    let whole = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
    Quad { texture, corners: [to - right, to + right, from + right, from - right], uv: whole, color: egui::Color32::WHITE }
}

/// An inserter's arm where an idle one waits: the hand over the tile it picks from.
fn arm(proto: &Proto, e: &PlacedEntity, centre: egui::Pos2, texture: &impl Fn(&Frame) -> Option<egui::TextureId>) -> Vec<Quad> {
    let Some(pickup) = proto.pickup else { return Vec::new() };
    // The prototype gives the pickup for an inserter facing north; turn it to face `e.dir`.
    let pickup = match e.dir {
        Dir::N => pickup,
        Dir::E => egui::vec2(-pickup.y, pickup.x),
        Dir::S => -pickup,
        Dir::W => egui::vec2(pickup.y, -pickup.x),
    };
    // The open hand's claw is the top of its picture and its bar runs back toward the base: the claw grips just past
    // the pickup point, and the arm's base piece fills whatever is left between the platform and the bar.
    let u = pickup.normalized();
    let tip = centre + pickup + u * 0.2;
    let hand = proto.part("hand_open").first().and_then(|f| texture(f).map(|id| (f, id)));
    let elbow = hand.map_or(centre + pickup, |(frame, _)| tip - u * frame.size.y);
    let mut quads = Vec::new();
    if let Some((frame, id)) = proto.part("hand_base").first().and_then(|f| texture(f).map(|id| (f, id)))
        && (elbow - centre).dot(u) > 0.1
    {
        quads.push(along(id, centre, elbow, frame.size.x));
    }
    if let Some((frame, id)) = hand {
        quads.push(along(id, elbow, tip, frame.size.x));
    }
    quads
}

/// The game's name for a pipe's picture, from the sides it connects: to pipes, to a pipe-to-ground whose open end
/// faces it, and to machines (which take pipes only at their fluid boxes, where the layout puts them).
fn pipe_shape(layout: &Layout, tiles: &HashMap<crate::geom::Tile, usize>, e: &PlacedEntity) -> &'static str {
    let joins = |d: Dir| {
        tiles.get(&(e.tile() + d)).is_some_and(|&j| {
            let n = &layout.entities[j];
            match n.kind {
                Kind::Pipe | Kind::InfinityPipe | Kind::Machine => true,
                Kind::PipeUg => n.dir == d.opposite(),
                _ => false,
            }
        })
    };
    let [n, east, s, w] = Dir::ALL.map(joins);
    match (n, east, s, w) {
        (false, false, false, false) => "straight_vertical_single",
        (true, false, false, false) => "ending_up",
        (false, true, false, false) => "ending_right",
        (false, false, true, false) => "ending_down",
        (false, false, false, true) => "ending_left",
        (true, false, true, false) => "straight_vertical",
        (false, true, false, true) => "straight_horizontal",
        (true, true, false, false) => "corner_up_right",
        (true, false, false, true) => "corner_up_left",
        (false, true, true, false) => "corner_down_right",
        (false, false, true, true) => "corner_down_left",
        (true, true, false, true) => "t_up",
        (false, true, true, true) => "t_down",
        (true, true, true, false) => "t_right",
        (true, false, true, true) => "t_left",
        (true, true, true, true) => "cross",
    }
}
