//! The background half of the window: one thread per run, holding the data and the job and sending what happens back
//! as `Event`s. Nothing here needs a window, so `tests/gui_worker.rs` drives it headlessly.

use crate::blocks::factory::run_factory_with_cancel;
use crate::config::run_configuration::RunConfiguration;
use crate::config::tuning::Tuning;
use crate::data::data_model::RootData;
use crate::geom::Layout;
use crate::plan::{ProductionPlan, build_plan};
use crate::search::search::{Improvement, Search};
use crate::search::trial::TrialResult;
use crate::util::text::fmt_fixed;
use crate::validate::{OutputSpec, SourceSpec, ValidationError};
use crate::viz::{RenderOptions, port_labels, render_image};
use anyhow::{Context, Result};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// Pixels per tile for a live preview: big enough to read a busy block, small enough to draw on every improvement.
const LIVE_PX: i32 = 12;
/// Pixels per tile for the last picture of a run.
const FINAL_PX: i32 = 32;

/// A rendered picture: premultiplied RGBA, the layout `tiny-skia` fills and `egui` uploads as a texture. `px` is how
/// many picture pixels a tile got, so a camera can stay on the same tiles across a 12 px live preview and the 32 px
/// picture that ends the run.
#[derive(Clone)]
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub px: f32,
    pub rgba: Arc<[u8]>,
}

/// What a run tells the window.
pub enum Event {
    /// The plan is built and the run has started.
    Started { out: String, machines: usize },
    /// A line the run appended to its log (`log.txt`, or `factory.txt` for a factory).
    Log(String),
    /// A new best layout, with the trial it came from and a picture of it (None if drawing it failed).
    Improvement { fact: Improvement, trial: Arc<TrialResult>, image: Option<Image> },
    /// The run is over: one line to show, and a picture when the run has none of its own (a factory's result).
    Finished { summary: String, image: Option<Image> },
    Failed(String),
}

/// The job a worker is given: what to run, and where.
pub struct Job {
    pub config: RunConfiguration,
    pub tuning: Tuning,
    pub out: PathBuf,
}

/// `Sender` is not `Sync` and the search's hooks have to be, so every send goes through this.
#[derive(Clone)]
struct Sink(Arc<Mutex<Sender<Event>>>);

impl Sink {
    fn new(tx: Sender<Event>) -> Self {
        Self(Arc::new(Mutex::new(tx)))
    }

    fn send(&self, event: Event) {
        // A window that closed its receiver is not an error worth reporting.
        let _ = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).send(event);
    }
}

/// Runs `job` on its own thread. The handle ends when the run does, cancelled or finished.
pub fn spawn(data: Arc<RootData>, job: Job, tx: Sender<Event>, cancel: Arc<AtomicBool>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let sink = Sink::new(tx);
        if let Err(e) = run(&data, &job, &sink, &cancel) {
            sink.send(Event::Failed(format!("{e:#}")));
        }
    })
}

fn run(data: &RootData, job: &Job, sink: &Sink, cancel: &Arc<AtomicBool>) -> Result<()> {
    let plan = build_plan(data, &job.config, &job.tuning).context("the plan could not be built")?;
    std::fs::create_dir_all(&job.out)?;
    plan.write(&job.out.join("plan.json"))?;
    std::fs::write(job.out.join("tuning.json"), job.tuning.to_json()).context("Could not write tuning.json")?;
    sink.send(Event::Started { out: job.out.display().to_string(), machines: plan.machines.len() });

    // A job runs as a factory under the same condition the command line uses: it has a blocks section, and more
    // machines than one block takes.
    let factory = job.config.blocks.as_ref().is_some_and(|blocks| plan.machines.len() > blocks.max_machines as usize);
    let tail = Tail::start(&job.out.join(if factory { "factory.txt" } else { "log.txt" }), sink);
    let (summary, image) = if factory { factory_run(data, job, &plan, cancel)? } else { search_run(data, job, &plan, sink, cancel)? };
    // The tail is dropped before the last event, so no log line arrives after it.
    drop(tail);
    sink.send(Event::Finished { summary, image });
    Ok(())
}

fn search_run(data: &RootData, job: &Job, plan: &ProductionPlan, sink: &Sink, cancel: &Arc<AtomicBool>) -> Result<(String, Option<Image>)> {
    let watch = sink.clone();
    let mut search = Search::new(data, &job.config, &job.tuning, plan, job.out.as_path(), None)
        .with_cancel(Arc::clone(cancel))
        .with_on_improvement(move |fact, trial| {
            let image = draw(trial, LIVE_PX).map_err(|e| format!("{e:#}")).ok();
            if image.is_none() {
                watch.send(Event::Log("the preview of this improvement could not be drawn".to_owned()));
            }
            watch.send(Event::Improvement { fact: fact.clone(), trial: Arc::clone(trial), image });
        });
    let result = search.run()?;
    let stopped = cancel.load(Ordering::Relaxed);
    search.flush(if stopped { "cancelled" } else { "finished" })?;
    let best = result
        .best
        .as_ref()
        .map_or_else(|| "no feasible layout".to_owned(), |b| format!("{}x{} score {}", b.w, b.h, fmt_fixed(b.score, 1)));
    // The last picture of the run is drawn at the full 32 px/tile, not the 12 the live previews use.
    let image = result.best.as_ref().and_then(|best| draw(best, FINAL_PX).ok());
    let how = if stopped { "stopped" } else { "finished" };
    Ok((format!("{how}: {} trials, best {best}", result.trials), image))
}

fn factory_run(data: &RootData, job: &Job, plan: &ProductionPlan, cancel: &Arc<AtomicBool>) -> Result<(String, Option<Image>)> {
    let Some(factory) = run_factory_with_cancel(data, &job.config, &job.tuning, plan, &job.out, Some(Arc::clone(cancel)))?
    else {
        return Ok(("factory: no result".to_owned(), None));
    };
    let verdict = if factory.success() { "valid".to_owned() } else { format!("{} validation errors", factory.errors.len()) };
    let title = format!("{}x{} score {}", factory.layout.w, factory.layout.h, fmt_fixed(factory.score, 1));
    let image = picture(&factory.layout, &factory.sources, &factory.outputs, &factory.errors, title, FINAL_PX).ok();
    let how = if cancel.load(Ordering::Relaxed) { "stopped" } else { "finished" };
    let summary = format!("factory {how}: {}x{} score {}, {verdict}", factory.layout.w, factory.layout.h, fmt_fixed(factory.score, 1));
    Ok((summary, image))
}

fn draw(trial: &TrialResult, px: i32) -> Result<Image> {
    let layout = trial.layout.as_ref().context("an improvement has a layout")?;
    let title = format!("{}x{} score {}", trial.w, trial.h, fmt_fixed(trial.score, 1));
    picture(layout, &trial.sources, &trial.outputs, &trial.errors, title, px)
}

/// A trial drawn at `px` picture pixels per tile, for the window to ask for again when the camera zooms: the live
/// previews are deliberately coarse, and a picture drawn at the size it is shown at is the difference between a
/// legible plan and a blurry one.
pub fn picture_of(trial: &TrialResult, px: i32) -> Result<Image> {
    draw(trial, px)
}

/// The pixels per tile worth drawing at a zoom, capped so a 140x140 factory cannot eat memory: about 4M pixels,
/// which is 16 MB of RGBA whatever the layout's size.
pub fn wanted_px(zoom: f32, w: i32, h: i32) -> i32 {
    const MAX_PIXELS: f32 = 4_000_000.0;
    let cap = (MAX_PIXELS / (w.max(1) as f32 * h.max(1) as f32)).sqrt();
    zoom.clamp(4.0, 48.0).min(cap).round().max(1.0) as i32
}

fn picture(
    layout: &Layout,
    sources: &[SourceSpec],
    outputs: &[OutputSpec],
    errors: &[ValidationError],
    title: String,
    px: i32,
) -> Result<Image> {
    let options = RenderOptions { px, title: Some(title), ports: port_labels(sources, outputs), errors: errors.to_vec(), ..RenderOptions::default() };
    let pixmap = render_image(layout, &options)?;
    Ok(Image { w: pixmap.width(), h: pixmap.height(), px: px as f32, rgba: Arc::from(pixmap.take()) })
}

/// Streams the lines a run appends to its log, so the window shows them as they happen rather than at the end. The
/// thread stops when this is dropped.
struct Tail {
    done: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Tail {
    fn start(path: &Path, sink: &Sink) -> Self {
        let done = Arc::new(AtomicBool::new(false));
        let watching = Arc::clone(&done);
        let sink = sink.clone();
        let path = path.to_path_buf();
        let handle = std::thread::spawn(move || {
            let mut offset = 0u64;
            let mut pending = String::new();
            while !watching.load(Ordering::Relaxed) {
                if let Ok(mut file) = std::fs::File::open(&path) {
                    if file.seek(SeekFrom::Start(offset)).is_ok() {
                        let mut chunk = Vec::new();
                        if file.read_to_end(&mut chunk).is_ok() && !chunk.is_empty() {
                            offset += chunk.len() as u64;
                            pending.push_str(&String::from_utf8_lossy(&chunk));
                            while let Some(end) = pending.find('\n') {
                                let line: String = pending.drain(..=end).collect();
                                sink.send(Event::Log(line.trim_end().to_owned()));
                            }
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(150));
            }
        });
        Self { done, handle: Some(handle) }
    }
}

impl Drop for Tail {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}
