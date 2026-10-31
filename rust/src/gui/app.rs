//! The window: the job form on the left, tabs in the middle (preview, job JSON), a status line at the bottom.
//! G1 draws the form and the JSON tab; running a job and the live preview arrive with the worker in G2, which is why
//! Run is present but disabled for now.

use super::form::{EXAMPLE, JobForm};
use super::preview::Preview;
use super::worker::{self, Event, Job};
use crate::config::run_configuration::{ConfigTarget, DEFAULT_TRICKS, RunConfiguration, UNIMPLEMENTED_TRICKS};
use crate::config::tuning::Tuning;
use crate::data::data_model::RootData;
use crate::data::game_constants as gc;
use crate::data::loader::load_root_data;
use crate::search::search::Improvement;
use eframe::egui;
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
        let (form, status) = match JobForm::example() {
            Ok(form) => (form, Some(Status::info("loaded the built-in example job"))),
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
        self.status = Some(Status::info("stopping at the next size boundary"));
    }

    /// Drains what the worker sent since the last frame.
    fn poll(&mut self, ctx: &egui::Context) {
        let Self { run, preview, status, .. } = self;
        let Some(run) = run.as_mut() else {
            return;
        };
        while let Ok(event) = run.rx.try_recv() {
            match event {
                Event::Started { out, machines } => {
                    run.out = out.clone();
                    run.log.push(format!("plan: {machines} machines -> {out}"));
                }
                Event::Log(line) => {
                    run.log.push(line);
                    if run.log.len() > 2000 {
                        run.log.drain(..500);
                    }
                }
                Event::Improvement { fact, image, .. } => {
                    run.improvements.push(fact);
                    if let Some(image) = image {
                        preview.set(ctx, &image);
                    }
                }
                Event::Finished { summary, image } => {
                    run.summary = Some(summary.clone());
                    run.done = true;
                    *status = Some(Status::info(summary));
                    if let Some(image) = image {
                        preview.set(ctx, &image);
                    }
                }
                Event::Failed(error) => {
                    run.error = Some(error.clone());
                    run.done = true;
                    *status = Some(Status::error(error));
                }
            }
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
                if ui.button("Example").clicked() {
                    self.load(EXAMPLE);
                }
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
        }
        egui::Panel::right("improvements").resizable(true).default_size(300.0).min_size(180.0).show(ui, |ui| {
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
            ui.label(format!("{:.1} px/tile", self.preview.zoom()));
            if let Some(run) = &self.run {
                ui.label(egui::RichText::new(&run.out).small());
            }
        });
        self.preview.ui(ui);
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
    egui::ComboBox::from_id_salt(id).selected_text(value.as_str()).width(180.0).show_ui(ui, |ui| {
        ui.add(egui::TextEdit::singleline(&mut filter).hint_text("filter").desired_width(160.0));
        ui.separator();
        let needle = filter.to_lowercase();
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            for option in options {
                if !needle.is_empty() && !option.to_lowercase().contains(&needle) {
                    continue;
                }
                if ui.selectable_label(value.as_str() == option.as_str(), option).clicked() && value != option {
                    *value = option.clone();
                    changed = true;
                }
            }
        });
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
