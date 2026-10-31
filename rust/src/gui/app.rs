//! The window: the job form on the left, tabs in the middle (preview, job JSON), a status line at the bottom.
//! The worker runs the job and reports what happens; the picture is drawn by the search's own renderer, with the
//! game's icons on the machines when the install is there.

use super::form::JobForm;
use super::preview::Preview;
use super::worker::{self, Event, Job};
use crate::config::run_configuration::{ConfigTarget, DEFAULT_TRICKS, RunConfiguration, UNIMPLEMENTED_TRICKS};
use crate::config::tuning::Tuning;
use crate::data::data_model::RootData;
use crate::data::game_constants as gc;
use crate::data::loader::load_root_data;
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
    /// The trial the picture came from, so the window can redraw it at the zoom it is being shown at.
    shown: Option<Arc<TrialResult>>,
    /// Jobs the preset list offers, worked out once at startup.
    presets: Vec<String>,
    /// The scenarios the Test button queued, and how far through them the window is.
    tests: VecDeque<JobForm>,
    test_of: usize,
    test_total: usize,
    /// Every improvement of this launch, oldest first, with the trial behind it.
    history: Vec<Snapshot>,
    /// Which snapshot the preview is showing, when it is not simply the newest.
    picked: Option<usize>,
    /// Draw one character per entity in the picture: the switch that settles "is that a belt or an inserter".
    symbols: bool,
    /// Show the explanation of the window and how a run works.
    help: bool,
    /// Which run the window is on, for the history column.
    run_no: usize,
}

/// One improvement kept for the session: the trial it came from, and the picture drawn at the time.
struct Snapshot {
    run: usize,
    number: usize,
    fact: Improvement,
    trial: Arc<TrialResult>,
}

/// A run the window is watching: what the worker sent, and how to stop it.
struct Run {
    rx: Receiver<Event>,
    cancel: Arc<AtomicBool>,
    /// Where the run writes; the worker confirms it once the plan is built.
    out: String,
    started: Instant,
    log: Vec<String>,
    summary: Option<String>,
    error: Option<String>,
    done: bool,
}

impl GuiApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let data = Arc::new(load_root_data("recipes.json").expect("the built-in recipes.json parses"));
        // Everything 35% larger: the default egui sizes are small on a 1600 px-wide screen, and this window is read at a
        // glance rather than typed into. Ctrl with +/- or the wheel still adjusts it at run time.
        cc.egui_ctx.set_zoom_factor(1.35);
        let choices = Choices::new(&data);
        let (form, status) = match JobForm::default_job() {
            Ok(form) => {
                let loaded = format!("loaded {}", form.path);
                (form, Some(Status::info(loaded)))
            }
            Err(e) => (JobForm::new(RunConfiguration::default()), Some(Status::error(format!("{e:#}")))),
        };
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
            shown: None,
            presets: JobForm::presets(),
            tests: VecDeque::new(),
            test_of: 0,
            test_total: 0,
            history: Vec::new(),
            picked: None,
            symbols: false,
            help: false,
            run_no: 0,
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

    /// Draws what is on screen again, at the current zoom and with the current settings: the Symbols switch uses it,
    /// because the picture the worker sent was drawn without the switch.
    fn redraw(&mut self, ctx: &egui::Context) {
        let Some(trial) = self.shown.clone() else {
            return;
        };
        let px = worker::wanted_px(self.preview.zoom(), trial.w, trial.h);
        match worker::picture_of(&trial, px, self.symbols) {
            Ok(image) => self.preview.set(ctx, &image),
            Err(e) => self.status = Some(Status::error(format!("{e:#}"))),
        }
    }

    /// Shows a snapshot again: its trial takes over the picture (redrawn at the current zoom) and Copy blueprint.
    fn pick(&mut self, index: usize, ctx: &egui::Context) {
        let Some(snapshot) = self.history.get(index) else {
            return;
        };
        let trial = Arc::clone(&snapshot.trial);
        let label = format!("showing improvement {} of run {}", snapshot.number, snapshot.run);
        self.picked = Some(index);
        self.shown = Some(trial);
        // The picture is drawn from the trial, so a picked snapshot is as sharp as the live one.
        if let Some(layout) = self.shown.as_ref().and_then(|trial| trial.layout.as_ref()) {
            let px = worker::wanted_px(self.preview.zoom(), layout.w, layout.h);
            if let Ok(image) = worker::picture_of(self.shown.as_ref().expect("just set"), px, self.symbols) {
                self.preview.set(ctx, &image);
            }
        }
        self.status = Some(Status::info(label));
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
            let Self { run, preview, status, shown, history, picked, run_no, .. } = self;
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
                        *picked = None;
                        *run_no += 1;
                    }
                    Event::Log(line) => {
                        run.log.push(line);
                        if run.log.len() > 2000 {
                            run.log.drain(..500);
                        }
                    }
                    Event::Improvement { fact, trial, image } => {
                        // Kept for the session: the trial is what the picture is drawn from, so an entry in the list
                        // brings its layout back at whatever the camera is at.
                        let number = history.iter().filter(|snapshot| snapshot.run == *run_no).count() + 1;
                        history.push(Snapshot { run: *run_no, number, fact, trial: Arc::clone(&trial) });
                        if history.len() > 60 {
                            history.remove(0);
                        }
                        *shown = Some(trial);
                        *picked = None;
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
                    if ui.button("?").on_hover_text("what this window does").clicked() {
                        self.help = !self.help;
                    }
                    if self.run.as_ref().is_some_and(|run| !run.done) {
                        let elapsed = self.run.as_ref().map_or(0.0, |run| run.started.elapsed().as_secs_f32());
                        ui.label(format!("{elapsed:.1} s"));
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
            self.run_bar(ui);
            if self.help {
                egui::ScrollArea::vertical().id_salt("help").max_height(320.0).show(ui, |ui| self.help_ui(ui));
                ui.separator();
            }
            egui::ScrollArea::vertical().id_salt("form").auto_shrink([false, false]).show(ui, |ui| {
                self.form_ui(ui);
            });
        });
    }

    /// Run, Test and Stop, where the job is: a button this far from the form it starts is a button in the wrong place.
    fn run_bar(&mut self, ui: &mut egui::Ui) {
        let running = self.run.as_ref().is_some_and(|run| !run.done);
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if running {
                let stop = egui::Button::new(egui::RichText::new("Stop").size(18.0)).min_size(egui::vec2(120.0, 40.0));
                if ui.add(stop).clicked() {
                    self.stop();
                }
                let elapsed = self.run.as_ref().map_or(0.0, |run| run.started.elapsed().as_secs_f32());
                ui.label(egui::RichText::new(format!("{elapsed:.1} s")).size(16.0));
            } else {
                let ready = self.form.validate().is_ok();
                let run = egui::Button::new(egui::RichText::new("Run").size(20.0)).min_size(egui::vec2(140.0, 44.0));
                if ui.add_enabled(ready, run).on_disabled_hover_text("the job has to validate first").clicked() {
                    self.start();
                }
                let test = egui::Button::new(egui::RichText::new("Test").size(15.0)).min_size(egui::vec2(80.0, 32.0));
                if ui.add(test).on_hover_text("three small factories of growing size, under a minute").clicked() {
                    self.start_test();
                }
            }
        });
        ui.add_space(4.0);
    }

    /// The "?" page: what the window is for and what a run does, in the order a person meets it.
    fn help_ui(&mut self, ui: &mut egui::Ui) {
        ui.set_max_width(ui.available_width());
        for (heading, text) in [
            ("What this is", "A window on the generator. It plans a factory from your targets, searches for a layout, and draws the best one it has found so far."),
            ("The job", "Targets are what the block must produce: an item and how many per second. Everything below narrows the search: machine and belt tier, whether to split a big job into blocks, the time budget and the box it may grow to."),
            ("Run", "Plans, then searches until the budget runs out or the extra rounds are done. Every better layout appears in the picture as it is found."),
            ("Test", "Three small factories of growing size, randomised, one after another: a check that the whole chain works, in under a minute."),
            ("Reading the picture", "Blue boxes are machines with their recipe, coloured arrows are belts (one colour per item net, arrow shows travel), tinted squares are inserters, numbered circles are ports. The legend under the picture says the rest; Symbols replaces every mark with a letter (F machine, B belt, I inserter, C chest, P pole, R pipe, U underground, S splitter) and is the quickest way to settle a doubt."),
            ("Improvements", "Every improvement of this session is listed with its size and score. Click one to put it back on screen; Copy blueprint then copies that one, ready to paste into the game."),
            ("Where it writes", "Each run writes what the command line writes: the blueprint string, its PNG, ports.json, the log, and plan.json. The path is shown above the picture."),
        ] {
            ui.label(egui::RichText::new(heading).strong());
            ui.label(egui::RichText::new(text).small());
            ui.add_space(6.0);
        }
    }

    fn form_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Targets");
        ui.label(egui::RichText::new("item, then how many per second it must make").small());
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
                ui.add(egui::DragValue::new(&mut target.amount).speed(0.1).range(0.0..=1e6).fixed_decimals(2))
                    .on_hover_text("how many per second this block must produce");
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
        }
        // Every improvement of this launch, newest first, and clicking one brings it back: the trial is kept whole, so
        // the window can redraw it at whatever the camera is at and the string can be copied from it again.
        let mut picked = None;
        egui::Panel::right("improvements").resizable(true).default_size(320.0).min_size(180.0).show(ui, |ui| {
            ui.heading("Improvements");
            if self.history.is_empty() {
                ui.label(if self.run.is_some() { "no improvement yet" } else { "no run yet" });
                return;
            }
            ui.label(egui::RichText::new("click one to look at it again").small());
            egui::ScrollArea::vertical().id_salt("improvements").auto_shrink([false, false]).show(ui, |ui| {
                egui::Grid::new("improvement-rows").num_columns(5).striped(true).show(ui, |ui| {
                    for title in ["run", "#", "size", "score", "s"] {
                        ui.label(egui::RichText::new(title).strong());
                    }
                    ui.end_row();
                    for (index, snapshot) in self.history.iter().enumerate().rev() {
                        let fact = &snapshot.fact;
                        let here = self.picked == Some(index);
                        if ui.selectable_label(here, format!("{}", snapshot.run)).clicked() {
                            picked = Some(index);
                        }
                        ui.label(format!("{}", snapshot.number));
                        ui.label(format!("{}x{}", fact.w, fact.h));
                        ui.label(format!("{:.1}", fact.score));
                        ui.label(format!("{:.0}", fact.elapsed_ms as f64 / 1000.0));
                        ui.end_row();
                    }
                });
            });
        });
        if let Some(index) = picked {
            self.pick(index, ui.ctx());
        }
        egui::Panel::bottom("run-log").resizable(true).default_size(150.0).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("log").stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
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
            if let Some(run) = &self.run {
                ui.label(egui::RichText::new(&run.out).small());
            }
        });
        // The same string the command line writes to best_output_string.txt: the game's blueprint importer takes it.
        let copyable = self.shown.as_ref().and_then(|trial| trial.layout.as_ref()).map(|layout| layout.to_blueprint_string(&self.form.config.quality));
        ui.horizontal(|ui| {
            if ui.add_enabled(copyable.is_some(), egui::Button::new("Copy blueprint")).on_disabled_hover_text("no layout to copy yet").clicked() {
                if let Some(blueprint) = &copyable {
                    ui.ctx().copy_text(blueprint.clone());
                    self.status = Some(Status::info(format!("{} characters on the clipboard: paste it into the game", blueprint.len())));
                }
            }
            if let Some(blueprint) = &copyable {
                ui.label(egui::RichText::new(format!("{} characters", blueprint.len())).small());
            }
            ui.separator();
            // Characters instead of the marks: F machine, B belt, U underground, S splitter, I inserter, C chest,
            // P pole, R pipe, K and Q the infinity ones. The way to settle what a crowded plan is drawing.
            if ui
                .checkbox(&mut self.symbols, "Symbols")
                .on_hover_text("one character per entity: F machine, B belt, U underground, S splitter, I inserter, C chest, P pole, R pipe")
                .changed()
            {
                self.redraw(ui.ctx());
            }
        });
        self.preview.ui(ui);
        // The live pictures are drawn at 12 px/tile to stay cheap; on screen the camera usually wants more, so the
        // window draws the layout again at the size it is being shown at (bounded by `wanted_px`).
        let zoom = self.preview.zoom();
        if let Some(trial) = self.shown.clone() {
            let want = worker::wanted_px(zoom, trial.w, trial.h);
            if (want as f32 / self.preview.rendered_px().max(1.0) - 1.0).abs() > 0.2 {
                match worker::picture_of(&trial, want, self.symbols) {
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
            egui::ScrollArea::vertical().id_salt("combo").max_height(320.0).show(ui, |ui| {
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
