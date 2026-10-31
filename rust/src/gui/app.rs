//! The window: the job form on the left, tabs in the middle (preview, job JSON), a status line at the bottom.
//! The worker runs the job and reports what happens; the picture is drawn by the search's own renderer, with the
//! game's icons on the machines when the install is there.

use super::assets::Icons;
use super::form::JobForm;
use super::preview::Preview;
use super::sprites::Sprites;
use super::worker::{self, Event, Job};
use crate::config::run_configuration::{ConfigTarget, DEFAULT_TRICKS, RunConfiguration, UNIMPLEMENTED_TRICKS};
use crate::config::tuning::Tuning;
use crate::data::data_model::RootData;
use crate::data::game_constants as gc;
use crate::data::loader::load_root_data;
use crate::geom::{Dir, Kind, PlacedEntity};
use crate::search::search::Improvement;
use crate::search::trial::TrialResult;
use eframe::egui;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Instant;

const BAD: egui::Color32 = egui::Color32::from_rgb(220, 90, 90);
const GOOD: egui::Color32 = egui::Color32::from_rgb(120, 190, 120);

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Preview,
    Json,
}

/// How the picture is drawn: the schematic the command line renders, or the game's own graphics over it.
#[derive(Clone, Copy, PartialEq)]
enum View {
    Schematic,
    Game,
}

/// A line at the bottom: what the last action did, or why it did not.
struct Status {
    text: String,
    error: bool,
}

impl Status {
    fn info(text: impl Into<String>) -> Self {
        Self { text: text.into(), error: false }
    }

    fn error(text: impl Into<String>) -> Self {
        Self { text: text.into(), error: true }
    }
}

/// The names the dropdowns offer, collected once: the data holds a few hundred items, and a fresh `Vec` per frame
/// would churn for nothing.
struct Choices {
    items: Vec<String>,
    fuels: Vec<String>,
    machines: Vec<String>,
    qualities: Vec<String>,
    belts: Vec<String>,
}

impl Choices {
    fn new(data: &RootData) -> Self {
        Self {
            items: data.items.keys().cloned().collect(),
            fuels: data.items.values().filter(|i| i.fuel_category.is_some()).map(|i| i.name.clone()).collect(),
            machines: gc::supported_machines().map(str::to_owned).collect(),
            qualities: data.quality_names.clone(),
            belts: gc::BELT_TIERS.iter().map(|t| t.key.to_owned()).collect(),
        }
    }
}

pub struct GuiApp {
    data: Arc<RootData>,
    choices: Choices,
    form: JobForm,
    /// The list fields as text, so a half-typed line survives the frame it was typed in.
    forbidden: String,
    machines: String,
    tab: Tab,
    status: Option<Status>,
    /// The run in progress, or the one that just ended.
    run: Option<Run>,
    preview: Preview,
    /// The game's icons, read from the install on demand.
    icons: Icons,
    /// The game's sprite sheets and the geometry to place them.
    sprites: Sprites,
    /// The trial the picture came from, so the window can redraw it at the zoom it is being shown at.
    shown: Option<Arc<TrialResult>>,
    view: View,
    /// Jobs the preset list offers, worked out once at startup.
    presets: Vec<String>,
    /// The scenarios the Test button queued, and how far through them the window is.
    tests: VecDeque<JobForm>,
    test_of: usize,
    test_total: usize,
}

/// A run the window is watching: what the worker sent, and how to stop it.
struct Run {
    rx: Receiver<Event>,
    cancel: Arc<AtomicBool>,
    /// Where the run writes; the worker confirms it once the plan is built.
    out: String,
    started: Instant,
    improvements: Vec<Improvement>,
    log: Vec<String>,
    summary: Option<String>,
    error: Option<String>,
    done: bool,
}

impl GuiApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let data = Arc::new(load_root_data("recipes.json").expect("the built-in recipes.json parses"));
        let choices = Choices::new(&data);
        let (form, status) = match JobForm::default_job() {
            Ok(form) => {
                let loaded = format!("loaded {}", form.path);
                (form, Some(Status::info(loaded)))
            }
            Err(e) => (JobForm::new(RunConfiguration::default()), Some(Status::error(format!("{e:#}")))),
        };
        let icons = Icons::new();
        let mut app = Self {
            data,
            choices,
            form,
            forbidden: String::new(),
            machines: String::new(),
            tab: Tab::Preview,
            status,
            run: None,
            preview: Preview::default(),
            view: if icons.dir().is_some() { View::Game } else { View::Schematic },
            icons,
            sprites: Sprites::new(),
            shown: None,
            presets: JobForm::presets(),
            tests: VecDeque::new(),
            test_of: 0,
            test_total: 0,
        };
        app.resync_lists();
        app
    }

    /// Refills the text buffers from the job, after a load or an applied JSON tab.
    fn resync_lists(&mut self) {
        self.forbidden = self.form.config.forbidden_recipes.join("\n");
        self.machines = self.form.config.machines.as_ref().map(|m| m.join(", ")).unwrap_or_default();
    }

    fn load(&mut self, path: &str) {
        match JobForm::load(path) {
            Ok(form) => {
                self.form = form;
                self.resync_lists();
                self.status = Some(Status::info(format!("loaded {path}")));
            }
            Err(e) => self.status = Some(Status::error(format!("{e:#}"))),
        }
    }

    fn save(&mut self) {
        let path = self.form.path.clone();
        match self.form.save() {
            Ok(()) => self.status = Some(Status::info(format!("saved {path}"))),
            Err(e) => self.status = Some(Status::error(format!("{e:#}"))),
        }
    }

    /// Hands the job to a worker thread; the window keeps polling its events.
    fn start(&mut self) {
        let tuning = match Tuning::defaults().with_overrides(self.form.config.tuning.as_ref()) {
            Ok(tuning) => tuning,
            Err(e) => {
                self.status = Some(Status::error(format!("{e:#}")));
                return;
            }
        };
        let stamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
        let out = self.form.out_dir(&stamp);
        let job = Job { config: self.form.config.clone(), tuning, out: PathBuf::from(&out) };
        let (tx, rx) = std::sync::mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        worker::spawn(Arc::clone(&self.data), job, tx, Arc::clone(&cancel));
        self.run = Some(Run {
            rx,
            cancel,
            out,
            started: Instant::now(),
            improvements: Vec::new(),
            log: Vec::new(),
            summary: None,
            error: None,
            done: false,
        });
        self.status = Some(Status::info("running"));
    }

    fn stop(&mut self) {
        if let Some(run) = &self.run {
            run.cancel.store(true, Ordering::Relaxed);
        }
        // Stopping is also how a test run is abandoned early.
        self.tests.clear();
        self.status = Some(Status::info("stopping at the next size boundary"));
    }

    /// Queues the three test scenarios and starts the first: small, then bigger, shuffled, 30 s of budget in all.
    fn start_test(&mut self) {
        self.tests = JobForm::test_scenarios().into();
        self.test_of = 0;
        self.test_total = self.tests.len();
        self.next_test();
    }

    /// Runs the first queued scenario, or says the run is over when there is none left.
    fn next_test(&mut self) {
        let Some(form) = self.tests.pop_front() else {
            if self.test_total > 0 {
                self.status = Some(Status::info("the test run is done"));
                self.test_total = 0;
            }
            return;
        };
        self.test_of += 1;
        let label = format!("test {} of {}: {} s", self.test_of, self.test_total, form.config.search.time_budget_s);
        self.form = form;
        self.resync_lists();
        self.start();
        self.status = Some(Status::info(label));
    }

    /// Drains what the worker sent since the last frame.
    fn poll(&mut self, ctx: &egui::Context) {
        let mut advance = false;
        {
            let Self { run, preview, status, shown, .. } = self;
            let Some(run) = run.as_mut() else {
                return;
            };
            while let Ok(event) = run.rx.try_recv() {
                match event {
                    Event::Started { out, machines } => {
                        run.out = out.clone();
                        run.log.push(format!("plan: {machines} machines -> {out}"));
                        // The picture on screen belongs to the last run; it stays, but nothing can redraw it.
                        *shown = None;
                    }
                    Event::Log(line) => {
                        run.log.push(line);
                        if run.log.len() > 2000 {
                            run.log.drain(..500);
                        }
                    }
                    Event::Improvement { fact, trial, image } => {
                        run.improvements.push(fact);
                        *shown = Some(trial);
                        if let Some(image) = image {
                            preview.set(ctx, &image);
                        }
                    }
                    Event::Finished { summary, image } => {
                        run.summary = Some(summary);
                        run.done = true;
                        *status = Some(Status::info("the run ended"));
                        advance = true;
                        if let Some(image) = image {
                            preview.set(ctx, &image);
                        }
                    }
                    Event::Failed(error) => {
                        run.error = Some(error);
                        run.done = true;
                        *status = Some(Status::error("the run failed: why is under the preview"));
                        advance = true;
                    }
                }
            }
        }
        // The Test button's next scenario starts as the last one ends.
        if advance && !self.tests.is_empty() {
            self.next_test();
        }
    }

    fn top(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("job-bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Job");
                ui.add(
                    egui::TextEdit::singleline(&mut self.form.path)
                        .desired_width(300.0)
                        .hint_text("a built-in name, a preset, or a path"),
                );
                if ui.button("Load").clicked() {
                    let path = self.form.path.clone();
                    self.load(&path);
                }
                if ui.button("Save").clicked() {
                    self.save();
                }
                let presets = self.presets.clone();
                egui::ComboBox::from_id_salt("presets").selected_text("Preset").width(210.0).show_ui(ui, |ui| {
                    for path in &presets {
                        let label = std::path::Path::new(path).file_name().map_or_else(|| path.clone(), |name| name.to_string_lossy().into_owned());
                        if ui.selectable_label(false, label).clicked() {
                            self.load(path);
                        }
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.run.as_ref().is_some_and(|run| !run.done) {
                        if ui.button("Stop").clicked() {
                            self.stop();
                        }
                        let elapsed = self.run.as_ref().map_or(0.0, |run| run.started.elapsed().as_secs_f32());
                        ui.label(format!("{elapsed:.1} s"));
                    } else {
                        let ready = self.form.validate().is_ok();
                        let run = ui.add_enabled(ready, egui::Button::new("Run"));
                        if run.on_disabled_hover_text("the job has to validate first").clicked() {
                            self.start();
                        }
                        let test = ui.button("Test").on_hover_text("three small factories of growing size, 30 s in all");
                        if test.clicked() {
                            self.start_test();
                        }
                    }
                });
            });
        });
    }

    fn bottom(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status-bar").show(ui, |ui| {
            if let Some(status) = &self.status {
                let colour = if status.error { BAD } else { GOOD };
                ui.label(egui::RichText::new(&status.text).color(colour));
            }
            match self.form.validate() {
                Ok(()) => ui.label(egui::RichText::new("the job validates").color(GOOD).small()),
                Err(e) => ui.label(egui::RichText::new(format!("{e:#}")).color(BAD).small()),
            };
        });
    }

    fn left(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("job-form").resizable(true).default_size(390.0).min_size(320.0).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                self.form_ui(ui);
            });
        });
    }

    fn form_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Targets");
        self.targets_ui(ui);
        ui.add_space(8.0);
        ui.heading("Machines and belts");
        self.production_ui(ui);
        ui.add_space(8.0);
        ui.heading("Blocks");
        self.blocks_ui(ui);
        ui.add_space(8.0);
        ui.heading("Search and box");
        self.search_ui(ui);
        ui.add_space(8.0);
        ui.heading("Filters");
        self.filters_ui(ui);
        ui.add_space(8.0);
        self.tricks_ui(ui);
        ui.add_space(8.0);
        ui.heading("Output");
        ui.add(
            egui::TextEdit::singleline(&mut self.form.out)
                .desired_width(f32::INFINITY)
                .hint_text("empty: solves/<stamp> - gui"),
        );
    }

    fn targets_ui(&mut self, ui: &mut egui::Ui) {
        let items = &self.choices.items;
        let targets = &mut self.form.config.targets;
        let mut remove = None;
        for (i, target) in targets.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                choose(ui, &format!("target-{i}"), &mut target.name, items);
                ui.add(egui::DragValue::new(&mut target.amount).speed(0.1).range(0.0..=1e6).fixed_decimals(2));
                if ui.small_button("x").on_hover_text("remove this target").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            targets.remove(i);
        }
        if ui.button("+ target").clicked() {
            let science = items.iter().find(|name| name.ends_with("science-pack")).cloned();
            targets.push(ConfigTarget { name: science.unwrap_or_default(), amount: 1.0 });
        }
    }

    fn production_ui(&mut self, ui: &mut egui::Ui) {
        let Self { form, choices, machines, .. } = self;
        let config = &mut form.config;
        egui::Grid::new("production").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            ui.label("quality");
            choose(ui, "quality", &mut config.quality, &choices.qualities);
            ui.end_row();

            ui.label("machine");
            choose(ui, "machine", &mut config.machine, &choices.machines);
            ui.end_row();

            ui.label("then");
            let response = ui.add(
                egui::TextEdit::singleline(machines).desired_width(180.0).hint_text("comma separated, in order"),
            );
            if response.changed() {
                config.machines = parse_list(machines);
            }
            ui.end_row();

            ui.label("fuel");
            choose(ui, "fuel", &mut config.fuel, &choices.fuels);
            ui.end_row();

            ui.label("belt tier");
            choose(ui, "belt-tier", &mut config.belt_tier, &choices.belts);
            ui.end_row();
        });
    }

    fn blocks_ui(&mut self, ui: &mut egui::Ui) {
        let mut on = self.form.config.blocks.is_some();
        if ui.checkbox(&mut on, "split into blocks").changed() {
            self.form.toggle_blocks(on);
        }
        let Some(blocks) = self.form.config.blocks.as_mut() else {
            return;
        };
        egui::Grid::new("blocks").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            ui.label("max machines");
            ui.add(egui::DragValue::new(&mut blocks.max_machines).speed(1.0).range(2..=100_000));
            ui.end_row();

            ui.label("corridor");
            ui.add(egui::DragValue::new(&mut blocks.corridor).speed(1.0).range(2..=32));
            ui.end_row();

            ui.label("rows");
            let mut rows = blocks.rows.unwrap_or(0);
            egui::ComboBox::from_id_salt("rows").selected_text(if rows == 0 { "auto".to_owned() } else { rows.to_string() }).show_ui(ui, |ui| {
                for (value, label) in [(0, "auto"), (1, "1"), (2, "2")] {
                    if ui.selectable_label(rows == value, label).clicked() {
                        rows = value;
                    }
                }
            });
            blocks.rows = (rows != 0).then_some(rows);
            ui.end_row();

            ui.label("repack");
            ui.checkbox(&mut blocks.repack, "again at a common height per row");
            ui.end_row();

            ui.label("merge");
            ui.checkbox(&mut blocks.merge, "merge neighbouring blocks");
            ui.end_row();
        });
    }

    fn search_ui(&mut self, ui: &mut egui::Ui) {
        let search = &mut self.form.config.search;
        let bb = &mut self.form.config.r#box;
        egui::Grid::new("search").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            ui.label("budget (s)");
            ui.add(egui::DragValue::new(&mut search.time_budget_s).speed(5.0).range(1.0..=86_400.0));
            ui.end_row();

            ui.label("threads");
            ui.add(egui::DragValue::new(&mut search.threads).speed(1.0).range(1..=256));
            ui.end_row();

            ui.label("seeds per size");
            ui.add(egui::DragValue::new(&mut search.seeds_per_size).speed(1.0).range(1..=64));
            ui.end_row();

            ui.label("seed");
            ui.add(egui::DragValue::new(&mut search.seed).speed(1.0));
            ui.end_row();

            ui.label("fixed width");
            optional_i32(ui, &mut bb.fixed_width, 40, 3..=1000);
            ui.end_row();

            ui.label("fixed height");
            optional_i32(ui, &mut bb.fixed_height, 40, 3..=1000);
            ui.end_row();

            ui.label("start size");
            optional_i32(ui, &mut bb.start_size, 40, 3..=1000);
            ui.end_row();

            ui.label("max size");
            optional_i32(ui, &mut bb.max_size, 80, 3..=1000);
            ui.end_row();
        });
    }

    fn filters_ui(&mut self, ui: &mut egui::Ui) {
        let Self { form, forbidden, .. } = self;
        ui.label(egui::RichText::new("forbidden recipes, one per line").small());
        let response = ui.add(
            egui::TextEdit::multiline(forbidden).desired_rows(2).desired_width(f32::INFINITY).hint_text("productivity-module"),
        );
        if response.changed() {
            form.config.forbidden_recipes = parse_lines(forbidden);
        }
    }

    fn tricks_ui(&mut self, ui: &mut egui::Ui) {
        let config = &mut self.form.config;
        egui::CollapsingHeader::new(format!("Tricks ({})", DEFAULT_TRICKS.len())).default_open(false).show(ui, |ui| {
            egui::Grid::new("tricks").num_columns(3).spacing([6.0, 2.0]).show(ui, |ui| {
                for (n, (id, _)) in DEFAULT_TRICKS.iter().enumerate() {
                    let (id, mut on) = (*id, config.trick(id));
                    if UNIMPLEMENTED_TRICKS.contains(&id) {
                        ui.add_enabled(false, egui::Checkbox::new(&mut on, id))
                            .on_disabled_hover_text("planned for v2/v3, not implemented yet");
                    } else if ui.checkbox(&mut on, id).changed() {
                        config.tricks.insert(id.to_owned(), on);
                    }
                    if n % 3 == 2 {
                        ui.end_row();
                    }
                }
            });
        });
    }

    fn central(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Preview, "Preview");
                ui.selectable_value(&mut self.tab, Tab::Json, "Job JSON");
            });
            ui.separator();
            match self.tab {
                Tab::Preview => self.preview_ui(ui),
                Tab::Json => self.json_ui(ui),
            }
        });
    }

    fn preview_ui(&mut self, ui: &mut egui::Ui) {
        if self.run.as_ref().is_some_and(|run| !run.done) {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        if let Some(run) = &self.run {
            if let Some(error) = &run.error {
                ui.label(egui::RichText::new(error).color(BAD));
            }
            if let Some(summary) = &run.summary {
                ui.label(egui::RichText::new(summary).color(GOOD));
            }
        }        egui::Panel::right("improvements").resizable(true).default_size(300.0).min_size(180.0).show(ui, |ui| {
            ui.heading("Improvements");
            match &self.run {
                None => {
                    ui.label("no run yet");
                }
                Some(run) if run.improvements.is_empty() => {
                    ui.label("no improvement yet");
                }
                Some(run) => {
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                        egui::Grid::new("improvement-rows").num_columns(4).striped(true).show(ui, |ui| {
                            for title in ["#", "size", "score", "s"] {
                                ui.label(egui::RichText::new(title).strong());
                            }
                            ui.end_row();
                            for (n, fact) in run.improvements.iter().enumerate().rev() {
                                ui.label(format!("{}", n + 1));
                                ui.label(format!("{}x{}", fact.w, fact.h));
                                ui.label(format!("{:.1}", fact.score));
                                ui.label(format!("{:.0}", fact.elapsed_ms as f64 / 1000.0));
                                ui.end_row();
                            }
                        });
                    });
                }
            }
        });
        egui::Panel::bottom("run-log").resizable(true).default_size(150.0).show(ui, |ui| {
            egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                match &self.run {
                    Some(run) if !run.log.is_empty() => {
                        for line in &run.log {
                            ui.label(egui::RichText::new(line).monospace().small());
                        }
                    }
                    _ => {
                        ui.label(egui::RichText::new("the run's log shows up here").small());
                    }
                }
            });
        });
        ui.horizontal(|ui| {
            if ui.button("Fit").clicked() {
                self.preview.fit();
            }
            ui.separator();
            ui.selectable_value(&mut self.view, View::Schematic, "Schematic");
            ui.selectable_value(&mut self.view, View::Game, "Game");
            ui.separator();
            ui.label(format!("{:.1} px/tile", self.preview.zoom()));
            if self.view == View::Game {
                match self.icons.dir() {
                    Some(dir) => {
                        ui.label(egui::RichText::new("icons from the install").small()).on_hover_text(dir.display().to_string());
                    }
                    None => {
                        ui.label(egui::RichText::new("no game install: set FACTORIO_DIR").color(BAD).small());
                    }
                }
            }
            if let Some(run) = &self.run {
                ui.label(egui::RichText::new(&run.out).small());
            }
        });
        let Self { preview, icons, sprites, shown, data, view, .. } = self;
        let game = *view == View::Game;
        preview.ui(ui, !game, |painter, rect, px| {
            if !game || px < 3.0 {
                return;
            }
            let Some(trial) = shown.as_ref() else {
                return;
            };
            let Some(layout) = &trial.layout else {
                return;
            };
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            // A belt's sprite spans two tiles each way and overlaps its neighbours, so the belt downstream has to be
            // drawn last for the joint to land right: that is the order the game draws belts in. Everything else is an
            // object and goes above the belts, lower rows over higher ones.
            let (mut belts, mut objects): (Vec<&PlacedEntity>, Vec<&PlacedEntity>) =
                layout.entities.iter().partition(|entity| entity.kind.belt_family());
            belts.sort_by_key(|entity| {
                let along = match entity.dir {
                    Dir::N => -(entity.y),
                    Dir::E => entity.x,
                    Dir::S => entity.y,
                    Dir::W => -(entity.x),
                };
                (entity.dir.ordinal(), along)
            });
            objects.sort_by_key(|entity| (entity.y, entity.x));
            for entity in belts.into_iter().chain(objects) {
                let (w, h) = data.entities.get(&entity.name).map_or((1, 1), |model| (model.width(), model.height()));
                let footprint = (w as f32, h as f32);
                // The game's own sheet when the table has one; the icon when it does not; nothing beyond that.
                if sprites.draw(painter, rect.min, px, entity, footprint) {
                    continue;
                }
                if !matches!(entity.kind, Kind::Machine | Kind::Chest) || px < 6.0 {
                    continue;
                }
                let centre = rect.min + egui::vec2((entity.x as f32 + footprint.0 / 2.0) * px, (entity.y as f32 + footprint.1 / 2.0) * px);
                let size = egui::vec2(footprint.0, footprint.1) * px * 0.9;
                if let Some(icon) = icons.icon(painter.ctx(), &entity.name) {
                    painter.image(icon.id(), egui::Rect::from_center_size(centre, size), uv, egui::Color32::from_white_alpha(235));
                }
            }
        });
        // The live pictures are drawn at 12 px/tile to stay cheap; on screen the camera usually wants more, so the
        // window draws the layout again at the size it is being shown at (bounded by `wanted_px`).
        let zoom = self.preview.zoom();
        if let Some(trial) = self.shown.clone() {
            let want = worker::wanted_px(zoom, trial.w, trial.h);
            if (want as f32 / self.preview.rendered_px().max(1.0) - 1.0).abs() > 0.2 {
                match worker::picture_of(&trial, want) {
                    Ok(image) => {
                        self.preview.set(ui.ctx(), &image);
                        ui.ctx().request_repaint();
                    }
                    Err(e) => self.status = Some(Status::error(format!("{e:#}"))),
                }
            }
        }
    }

    fn json_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Apply").clicked() {
                match self.form.apply_json() {
                    Ok(()) => {
                        self.resync_lists();
                        self.status = Some(Status::info("took the JSON tab into the job"));
                    }
                    Err(e) => self.status = Some(Status::error(format!("{e:#}"))),
                }
            }
            if ui.button("Revert").clicked() {
                self.form.json_edited = false;
                self.form.json_error = None;
                self.form.sync_json();
            }
            ui.label(egui::RichText::new("the tab is the job, as the file would hold it").small());
        });
        if let Some(error) = &self.form.json_error {
            ui.label(egui::RichText::new(error).color(BAD));
        }
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            let response = ui.add(
                egui::TextEdit::multiline(&mut self.form.json)
                    .code_editor()
                    .desired_rows(30)
                    .desired_width(f32::INFINITY),
            );
            if response.changed() {
                self.form.json_edited = true;
            }
        });
    }
}

impl eframe::App for GuiApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll(ui.ctx());
        self.form.sync_json();
        self.top(ui);
        self.bottom(ui);
        self.left(ui);
        self.central(ui);
    }
}

/// A combo box over `options` with a filter field, so a few hundred items stay usable. True if the value changed.
fn choose(ui: &mut egui::Ui, id: &str, value: &mut String, options: &[String]) -> bool {
    let mut changed = false;
    let filter_id = egui::Id::unique(("filter", id));
    let mut filter: String = ui.data(|data| data.get_temp(filter_id)).unwrap_or_default();
    egui::ComboBox::from_id_salt(id)
        .selected_text(value.as_str())
        .width(180.0)
        // A combo closes on any click by default, which shuts it the moment the filter field is clicked; with this
        // it closes on a click outside, and the pick below closes it by hand.
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, |ui| {
            let typed = ui.add(egui::TextEdit::singleline(&mut filter).hint_text("filter").desired_width(160.0));
            ui.separator();
            let needle = filter.to_lowercase();
            let matches: Vec<&String> = options.iter().filter(|option| needle.is_empty() || option.to_lowercase().contains(&needle)).collect();
            let mut picked: Option<String> = None;
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for option in &matches {
                    if ui.selectable_label(value.as_str() == option.as_str(), option.as_str()).clicked() {
                        picked = Some((*option).clone());
                    }
                }
                if matches.is_empty() {
                    ui.label(egui::RichText::new("nothing matches").small());
                }
            });
            // A name typed in full and Enter is the other way to pick, which is what makes a long list usable.
            if picked.is_none() && typed.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                picked = matches.first().map(|option| (*option).clone());
            }
            if let Some(option) = picked {
                if value != &option {
                    *value = option;
                    changed = true;
                }
                filter.clear();
                ui.close();
            }
        });
    ui.data_mut(|data| data.insert_temp(filter_id, filter));
    changed
}

/// A checkbox that turns an `Option<i32>` on at `start`, and a drag value while it is on.
fn optional_i32(ui: &mut egui::Ui, value: &mut Option<i32>, start: i32, range: std::ops::RangeInclusive<i32>) {
    let mut on = value.is_some();
    if ui.checkbox(&mut on, "").changed() {
        *value = on.then_some(start);
    }
    if let Some(v) = value.as_mut() {
        ui.add(egui::DragValue::new(v).speed(1.0).range(range));
    } else {
        ui.label(egui::RichText::new("auto").small());
    }
}

fn parse_list(text: &str) -> Option<Vec<String>> {
    let list: Vec<String> = text.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect();
    (!list.is_empty()).then_some(list)
}

fn parse_lines(text: &str) -> Vec<String> {
    text.split(['\n', ',']).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect()
}
