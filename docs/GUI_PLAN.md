# GUI for the Rust generator

Status: **in progress**. Built so far: G1 (the job form and its JSON tab), G2 (the worker, the live preview and
Stop) and level C of the preview (the In-game switch, last section). Next: G3 (inspect the result and take it out). Settled on review: eframe/egui for the window, preview levels
A + B + C, live updating during the search, the v1 widget list. D4 (feature-gated bin), D6 (blocks path) and D7
(outputs in `solves/`) stand as written unless someone objects.

## What is there to build on

The Rust port is a library plus a CLI and has no UI of any kind; neither does the Kotlin original, so there is no
upstream GUI to mirror and the GUI code has no file to map to. That matters for one house rule: the port keeps one
file per Kotlin file so upstream changes can be re-applied by hand. GUI code therefore lives outside that mirror
(`src/gui/`), which is a deliberate exception, documented at the top of `src/gui/mod.rs`.

The pieces a GUI can drive already exist and are public:

| Piece | Where | Shape |
|-------|-------|-------|
| Job config | `src/config/run_configuration.rs` | `RunConfiguration`, `serde` derived through `config_struct!`, `parse`/`load`, `validate()` |
| Tuning | `src/config/tuning.rs` | `Tuning::defaults()`, `with_overrides()` |
| Pipeline | `src/main.rs:57-105` | `load_root_data` → `build_plan` → `run_factory` if `blocks` and machines > `max_machines`, else `run_search` |
| Search | `src/search/search.rs:481` | `Search::run()`, deadline from `search.time_budget_s`, K seeds × threads, anytime output |
| Anytime output | `src/search/search.rs:363-396` | each improvement atomically rewrites `best_output_string.txt`, `best_blueprint.json`, `ports.json`, `best.png`, `renders/improvement_NNN.png`, `scores.tsv`, `allSolves/`, `log.txt` |
| Trial hook | `src/search/search.rs:134` | `with_on_trial(FnMut(&TrialResult))`, already used by calibration |
| Render | `src/viz/render.rs:330` | `render_image(&Layout, &RenderOptions) -> Pixmap` (in memory), `render_layout` writes a PNG |
| Blueprint in | `src/emit/blueprint.rs` | `decode_blueprint` + `to_layout(w, h)`, already used by `--render` |

`Improvement` (`search.rs:23`) already carries `elapsed_ms, w, h, score, belts, undergrounds, inserters, seed` — enough
for a results table and a score plot without touching the search.

`RootData` (`src/data/data_model.rs:10`) carries recipes, items, fluids, entities and qualities, so dropdowns can be
populated with real names (machines by `crafting_categories`, fuels by `fuel_category`, items and fluids for targets
and sources) instead of free-text fields.

## Goal

One window: choose the job, run it, watch the result appear, inspect it, take the blueprint out.

## Non-goals for v1

- Editing a layout by hand (move/rotate entities). The generator owns the layout.
- Reading or writing Factorio saves, importing/exporting to the game beyond the blueprint string that already exists.
- Calibration UI (`--calibrate`), verify kit (`--verify-kit`), mod support beyond what `recipes.json` already encodes.
- Replacing `docs/VERIFY.md` workflows.

## The preview question

The picture the program already draws is a top-down tile schematic: entity boxes, per-net belt arrows, inserters,
ports numbered and a legend, rasterised by tiny-skia. Three levels of fidelity, cheapest first:

| Level | Shows | Needs | Effort | Risk |
|-------|-------|-------|--------|------|
| **A. Schematic** (v1) | exactly the picture `best.png` is today | nothing; `render_image` returns the pixmap | 1-2 days | none, it is the tested renderer |
| **B. Schematic + icons** | same, with the Factorio icon of each machine/furnace/chest drawn on its tiles, and item icons in the port legend | read `data/base/graphics/icons/<name>.png` from the install (the files are 120×64: a 64×64 icon with its mipmaps packed to the right — 64+32+16+8 — so crop to the first 64×64, to be confirmed while implementing), `image` crate to decode | +1 day | some prototypes have no icon file → fall back to the plain box; a setting for the Factorio path |
| **C. Real sprites** | belts with their belt sprites, inserters with arms, machines with their multi-frame sheets and correct `shift` | `factorio.exe --dump-data` → `%APPDATA%\Factorio\script-output\data-raw-dump.json`, then per prototype: sheet path, `frame_size`, `line_length`, `frame_count`, `direction_count`, `shift`, `scale` | 3-5 days | the most fragile part of the app: sprite geometry is an internal of the game and moves between versions; needs a curated table of ~12 entity types anyway |

All three are in scope, in that order: A is what v1 ships (it is the picture the CLI already draws), B is milestone G4,
C is milestone G5.

**Bevy or raylib?** Neither is needed, and I recommend against both for v1:

- The preview is a static top-down raster. `tiny-skia` already draws it, correctly, and a test (`tests/viz.rs`) pins
  the pixels. Freeing that up to redraw it in a game engine buys nothing and costs a rewrite of 573 lines plus the
  golden tests.
- An engine gives a canvas but not widgets. With raylib the whole control panel (dropdowns, sliders, tables, text
  fields, scrolling, clipboard) is hand-written; egui gives it for free, plus zoom/pan on a texture is ~50 lines.
- Bevy drags in an ECS, a schedule and a large dependency tree and a slower build for a tool window. eframe 0.37 ships
  wgpu or glow, both fine on this machine.
- The seam is `render_image`: if a game-like view is ever wanted (animated belts, 3D), level C plugs in behind the same
  interface and the UI does not change.

So: **eframe/egui for the window and the widgets, `render_image` for the map.** `Pixmap::data()` is premultiplied
RGBA, and `egui::ColorImage::from_rgba_premultiplied` takes exactly that, so a pixmap becomes a texture with no
conversion and no PNG round trip.

Level C does not change that verdict. Sprites are textured quads on a tile grid — exactly what raylib would be used
for, and what egui paints with a mesh in about the same amount of code. What an engine would add is the widget kit we
would then have to write ourselves.

### Level C in practice

`Kind` (`src/geom/layout.rs:7`) is the whole coverage list, which is what makes C tractable: `Machine`, `Belt`, `UgIn`,
`UgOut`, `Splitter`, `Inserter`, `Chest`, `Pole`, `Pipe`, `PipeUg`, `InfinityPipe`, `InfinityChest` — twelve kinds, and
every placed entity in every layout is one of them. Machines carry their prototype name, so the assembler, furnace,
chemical plant, refinery and centrifuge sheets are keyed by name from the same field.

What C needs, in order:

1. **The dump.** `factorio.exe --dump-data` writes `%APPDATA%\Factorio\script-output\data-raw-dump.json` (that folder
   already exists on this machine). A "Dump prototypes" button in the GUI runs it, reports the game version and the
   file size, and caches a *distilled* table next to the app settings. The raw dump is tens of megabytes and is never
   read again after distilling.
2. **A sprite table** (`src/gui/assets.rs`) as committed JSON **with no image data in it** — sheet paths into the
   player's install, frame geometry, `shift`, `scale`, layer order for inserters. Written once by the dump tool,
   reviewed by hand. For twelve kinds that is a page or two, and keeping it in the repo means the app works on a
   machine without the dump.
3. **Composition** — the part that takes the time: rotation (belt direction, inserter pickup side, machine fluid input
   side), layering (inserter base/platform/hand; machine shadow under body), and `shift`/`scale` into tile space. Every
   sprite is a textured quad on the tile grid the schematic already uses, so pan/zoom is shared with A and B.
4. **Fallback.** A kind with no table entry, or a PNG that is not readable, draws as the level A box. A missing, moved
   or updated install degrades the picture instead of blanking the map, and the UI says which sprites are missing.

Reading the game's own `data/base/prototypes/*.lua` is not on the table: the graphics definitions are spread over 105
files and built from computed tables, and reading them is also outside what the tools here are allowed to touch. The
dump is the source, which is what the modding docs point at anyway.


## Architecture

```
UI thread (eframe)                          Worker thread
  form widgets  ──── Job struct ────────►   build_plan
  preview panel ◄─── Event ─────────────    Search::run  (K seeds × N threads inside)
  log tail, table                           render_image on every improvement
  cancel button ──── AtomicBool ───────►    flush("cancelled")
```

- **One crate, one optional feature.** `eframe` and (for level B) `image` sit behind `[features] gui`, the binary is
  `src/bin/gui.rs` with `required-features = ["gui"]`. `cargo test`, `cargo build` and the CLI never see the GUI
  dependencies, so build times and the port's dependency surface stay as they are. Run with
  `cargo run --features gui --bin gui`.
- **The UI thread never runs a search.** One worker thread per job owns `RootData` (loaded once into an `Arc`), builds
  the plan, runs the search, and sends events over `std::sync::mpsc`. The UI drains the channel per frame with
  `try_recv`.
- **Events:** `Started{plan_summary, out_dir}`, `Log(String)`, `Improvement{Improvement, Arc<TrialResult>, preview:
  Arc<ColorImage>}`, `Finished(SearchResult)`, `Failed(String)`. Errors travel as strings, the UI shows them in a
  banner.
- **Cancellation** is an `Arc<AtomicBool>` the search checks (see library additions). On cancel the worker calls
  `flush("cancelled")` so the best result and its files are complete, exactly like a finished run.
- **Files are still written.** The GUI passes a real `out` directory (`solves/<stamp> - gui`), so every run can be
  reproduced with the CLI and re-rendered with `--render`. The GUI never becomes the only way to get a result.
- **Render cadence:** live previews at `px = 12..16` per tile (a 120×120 block is ~2.3 MB), the final picture at
  `px = 32` when the run ends. Older previews are dropped; only the newest texture is kept, plus the last 20
  thumbnails if a gallery is wanted.
- **One event, two painters.** In A and B the worker sends a finished `Arc<ColorImage>`; in C (and for the B icon
  overlay, if it is drawn as sprites rather than burned in) it sends `Arc<TrialResult>` and the UI builds the mesh, so
  zooming does not ask the worker to re-rasterise. The panel picks the painter from the mode switch and both share the
  same camera (offset + scale), zoom, pan and fit code.

## Library additions

Three, all small and all additive:

1. `Search::with_on_improvement(FnMut(&Improvement, &Arc<TrialResult>))` next to the existing `with_on_trial`
   (`search.rs:134`), called from `offer()` right after `write_best`. ~12 lines. Without it the UI would have to poll
   `best.png`'s mtime, which is uglier and cannot render at the GUI's own scale.
2. `Search::with_cancel(Arc<AtomicBool>)`, consulted in `time_left()` (`search.rs:405`) and once per size loop. On
   cancel, `run()` returns and `flush("cancelled")` writes the best result. ~10 lines.
3. Nothing else. `main.rs` is not refactored: the GUI calls `build_plan`, `run_search`, `run_factory` in the same
   order `main.rs` does. Fifteen lines of pipeline setup are duplicated rather than adding a shared driver file that
   would blur the port's file-per-Kotlin-file mapping.

## UI

```
┌────────────────────────────┬──────────────────────────────────────────────┐
│ JOB                        │ PREVIEW   schematic | icons | sprites         │
│ targets: [item ▾][rate]  + │   (texture or sprite mesh;                   │
│ quality [normal ▾]         │    scroll = zoom, drag = pan, fit)           │
│ belt tier [fast ▾]         │                                              │
│ machine [assembling-3 ▾]   │                                              │
│ fuel [coal ▾]              │                                              │
│ ☑ blocks  max machines 60  │                                              │
│ search: budget [120] s     │                                              │
│         threads [8] seed   │                                              │
│ [Run]  [Stop]  45.2 s      ├──────────────────────────────────────────────┤
│ ── advanced ─────────────  │ IMPROVEMENTS                                 │
│ tricks (31 toggles)        │  #  time   size    score  belts ug ins seed   │
│ fluids, power, score       │  3   12.4  62×58   88.1   402  61  140  4471  │
│ sources/outputs (table)    │                                              │
│ [JSON] tab = raw config    │ score plot (from Improvement, egui plot)     │
├────────────────────────────┴──────────────────────────────────────────────┤
│ log tail (last 200 lines of SearchResult.log / log.txt)                    │
└───────────────────────────────────────────────────────────────────────────┘
```

Form coverage, in the order it gets built. Every field maps 1:1 to a `RunConfiguration` field, and the form's source of
truth is a `RunConfiguration` value, so save/load is the existing serde round trip and no second model exists:

| v1 widgets | Escape hatch for the rest |
|------------|---------------------------|
| `targets` (item + amount rows), `quality`, `machine`/`machines`, `fuel`, `belt_tier`, `forbidden_recipes`, `blocks` (enable, `max_machines`, `rows`, `corridor`, `repack`, `merge`), `search` (`time_budget_s`, `threads`, `seed`, `seeds_per_size`), `box` (`fixed_width/height`, `start_size`, `max_size`), `tricks` toggles, `out` directory | a **JSON tab** that shows and edits the exact `RunConfiguration` JSON with `parse()` validation errors shown inline. `sources`, `outputs`, `reserved`, `forbidden_tiles`, `score`, `power`, `fluids`, `sinks`, `research`, `inserters` stay in the JSON tab until someone asks for widgets |

Preview controls: zoom, pan, fit, toggle ports/labels, toggle validation errors (the layout carries them in
`TrialResult`), level B's icon toggle, and "show this improvement" for old thumbnails.

Result actions: copy the blueprint string to the clipboard, open the output folder, load `plan.json`-era job again,
and **view an existing blueprint** (`decode_blueprint` + `to_layout`, the same path `--render` uses) — cheap, and it
makes the app useful for looking at blueprints without a run.

Settings and presets:

- `%APPDATA%\factorio_optimizer\gui.json`: window size, last job path, Factorio data dir, px/tile, tab state. Written on
  exit, best-effort; a bad file is ignored, not fatal.
- Presets come from the repo: `src/main/resources/example_config.json` (compiled in, as the CLI does) plus the jobs in
  `docs/examples/*.json` and `docs/status/six_science_9_blocks.json`, listed in a preset dropdown by file name.
- The Factorio data directory (for level B and C) is a setting, defaulting to the path given here
  (`E:\SteamLibrary\steamapps\common\Factorio`). It is only ever read; nothing from the install is copied into the repo
  or written to disk by us, so the assets stay where the license puts them.

## Milestones

Each one ends in something runnable, in the project's style: a list of facts that can be checked.

**G1 — window and form.** `cargo run --features gui --bin gui` opens a window, loads the built-in `example_config.json`,
edits the v1 widgets, saves and reloads a job, and shows `validate()` errors instead of crashing. JSON tab shows the
same config. Test: `tests/gui_form.rs` (headless, no window) — the form's default equals `RunConfiguration::default()`,
a loaded job round-trips through `to_string`/`parse`, an invalid job surfaces the same message `validate()` gives, every
preset in `docs/examples` loads.

**G2 — run, watch, stop.** Run a 60 s job on a preset: the preview updates on each improvement, the log tail fills, the
improvements table grows, Stop ends the search at its next size boundary (seeds already running finish first) and
leaves `best.png`, `best_output_string.txt` and `ports.json` in the out directory. Test: `tests/gui_worker.rs` drives
the worker without a window, asserts the event sequence (Started → Log* → Improvement* → Finished), that a run leaves
the files the command line would leave, and that Stop reports `stopped` and logs `cancelled`.

**G3 — inspect and take it out.** Zoom/pan/fit, ports legend, error overlay, score plot, copy blueprint, open folder,
view an external blueprint. Test: extends `tests/gui_worker.rs` with a decoder round trip; pixel-level checks stay in
`tests/viz.rs`.

**G4 — icons (level B).** With a Factorio data dir set, machines, furnaces, chests and poles show their icon on their
tiles and the port legend shows item icons; with it unset or an icon missing, the plain schematic. Test: icon lookup is
a table unit test, missing file → no icon and no panic, an absent data dir → level A output unchanged.

**G5 — sprites (level C).** Four steps, each shippable on its own: (1) the "Dump prototypes" button and the distill
step, with the distilled table shown in the UI as text for review; (2) belts, undergrounds and splitters; (3) inserters
and poles; (4) machines, furnaces and chests. Acceptance: every `Kind` in `src/geom/layout.rs:7` that has a table entry
draws as a sprite with the correct rotation, and everything else still draws as a box — on all six presets, no panic
with the data dir unset, half-typed, or pointing at an older game version. `tests/viz.rs` and the `--render` PNG
output must stay byte-identical, since C is a GUI-side view and touches no renderer the CLI uses. If step 4 turns out to
be the fragile part, the milestone can stop after step 3 without leaving anything half-wired: the sprite table is data,
and a missing entry is the fallback.

**G6 — polish.** Presets dropdown, recent jobs, remembered window/settings, export button, gallery of past
improvements.

## Risks and things that will bite

- `[profile.release] panic = "abort"` (`Cargo.toml`) means a worker panic takes the window down with no message. Either
  keep the worker's own code panic-free (the search already reports errors as `Result`), or add
  `[profile.gui] inherit = "release"` with `panic = "unwind"` and document `cargo run --profile gui`.
- eframe 0.37 needs rustc ≥ 1.95; the sandbox has cargo/rustc 1.99, so fine here. Worth checking on the machine the
  window is actually used on.
- A 300 s default budget blocks nothing (worker thread) but the app must make "stop" obvious; a run left running while
  the user edits the form must not pick up the edits mid-flight — the job is cloned into the worker at start.
- Memory: 32 px/tile on a 140×140 layout is 20 MB per picture. Live previews are capped at 16 px/tile, finals rendered
  once.
- Level C is the only part with real uncertainty: sprite geometry tables. Treat G5 as a spike with four stopping points,
  not a promise, and keep the fallback exercised by a test.
- `--dump-data` runs the game's data stage: it takes a few seconds, writes into `%APPDATA%\Factorio\script-output`, and
  reflects whatever mods are enabled, so a modded install distils a different table from a clean one. The distilled
  table records the game version and the mod list from the dump so a mismatch warns instead of drawing wrong sprites.
- Windows-only assumption: nothing in the plan needs it, but the app will first be used on Windows, so path handling
  (`\\?\` prefixes, non-UTF-8) and `cmd /C start` for "open folder" are the tested path.

## Decisions

| # | Question | Decision |
|---|----------|----------|
| D1 | Window/widget stack | **eframe/egui.** The map stays `render_image` (A) and later a sprite mesh (C) |
| D2 | Preview fidelity | **A + B + C**: schematic in v1, icons at G4, real sprites at G5 |
| D3 | Preview timing | **Live**: every improvement redraws the preview; the final picture is re-rendered at 32 px/tile at the end |
| D4 | Crate layout | **`gui` feature + `src/bin/gui.rs`** in the existing crate; CLI and tests never build the GUI dependencies |
| D5 | Form scope | **The v1 list** in the UI section; everything else through the JSON tab |
| D6 | Multi-block factory path | **Supported**: same `RunConfiguration`, worker picks the branch `main.rs` picks |
| D7 | Where things are stored | **Outputs in the repo** (`solves/<stamp> - gui`), app settings in `%APPDATA%\factorio_optimizer\`, sprite table in the repo (paths and geometry only, no images) |

## What "done" looks like overall

Start the app, pick a preset, knock the budget down to 60 s, hit Run, watch the picture get denser as improvements land,
hit Stop, zoom in to check a belt run, copy the blueprint, paste it in game. Nothing in that path needs a terminal, and
every run leaves the same files behind that the CLI would have left.


## The in-game view (level C)

The picture panel has a switch: **Schematic** (the default, the picture the command line draws) and **In-game**, the
same layout drawn with the game's own sprites. It was tried once before (`8bbb025`..`09bbd0c`, removed in `56f8021`)
and never got belts right; the second attempt started from the sheets themselves, and the cause turned out to be three
misreadings, each of which on its own breaks a belt run.

**What the first attempt got wrong.**

- It took the belt sheet's 20 rows for sixteen rotations, one every fourth row (`frame = dir * 4 * frame_count`). The
  rows are the named indices of a `TransportBeltAnimationSetWithCorners`, so north drew the east row and east drew a
  corner.
- It drew one sprite per entity direction, so a belt coming in from the west and one coming in from the east, both
  turning north, drew the same picture: the run broke at every turn. Open ends had no lip.
- It drew every underground with the entrance sprite, in the entity's own column. An exit's hood stands behind it,
  which is the opposite column.

**What the sheets say** (measured on game 2.1.24; `rust/tests/gui_sheets.rs` measures it again on the install). A
belt sheet has `frame_count` columns (16, 32, 32 and 64 for the four tiers) and 20 rows; frame 0 of a row is its first
column, a 128 px canvas at scale 0.5, two tiles centred on the belt's tile. The dump overrides none of the default
indices, and their names are side names:

| Row | Name | Meaning |
|-----|------|---------|
| 0-3 | `east`, `west`, `north`, `south` | straight, travelling that way |
| 4-11 | `X_to_Y` | a curve entering through side X and leaving through side Y, the belt's direction |
| 12, 14, 16, 18 | `starting_X` | the lip behind a run's first belt, drawn on the tile on side X of it |
| 13, 15, 17, 19 | `ending_X` | the lip past a run's last belt, drawn on the tile on side X of it |

The evidence: a curve's ink leaves bare only the corner away from its two open sides; the texture moves two pixels a
frame the way items travel, out through Y and in through X; a lip is a strip on the edge of its canvas that faces the
belt, its rounded side away from it. Example: a belt travelling north fed from the east is `east_to_north` (row 4), fed
from the west `west_to_north` (row 6).

The underground structure sheet holds 4 columns (north, east, south, west) and 4 rows of 192 px cells: exit, entrance,
then the side-loading variants (east and west only). In column c the hood stands on side c and its mouth faces away;
so an entrance travelling d takes column d, an exit travelling d column opposite(d). Pipe pictures are named after
the sides they connect (`ending_up` reaches past the tile's top edge).

**How a tile picks its picture** (`rust/src/gui/belts.rs`, no window needed, `rust/tests/gui_belts.rs`). A belt fed
from behind is straight; otherwise one fed from exactly one side curves from that side; otherwise straight — the rule
the validator models, with its own `accepts` deciding what feeds what. A straight belt, entrance or splitter lane not fed
from behind gets a start lip; a belt, exit or splitter lane whose front tile does not take it gets an end lip, so a
side-load has none. An underground draws only the half of the belt on its open side, under its hood.

**Where the pictures come from** (`rust/src/gui/sprites.rs`). The geometry is read from the prototype dump
(`factorio --dump-data`, `%APPDATA%\Factorio\script-output\data-raw-dump.json`, or `FACTORIO_DUMP`), the pixels from
the install (`FACTORIO_DIR`); nothing is copied into the repository, and both come from the same game version. A
background thread reads the dump once, cuts frame 0 of every picture the view can need out of the sheets, and the
window uploads the cuts as textures with mipmaps. `rust/src/gui/game_view.rs` draws in the game's layers: belt surfaces,
lips, shadows, objects north to south, inserter hands over their pickup tile, and each machine's product icon as in
alt-mode. An entity with no picture is a box with its item icon: in 2.1 the logistic chests have none in the dump.

**Not drawn:** copper wires, floor-tile markers, the error overlay (the schematic has them), and factory runs, which
send only their picture.
