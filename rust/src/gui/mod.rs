//! The window on the generator: choose a job, run it, watch the result arrive, take the blueprint out.
//!
//! This is the one part of the tree with no Kotlin counterpart (upstream has no UI), so it does not follow the
//! file-per-Kotlin-file mirror everything else keeps. `form` holds what the window edits and has no window types in
//! it, so loading, saving and the JSON round trip are tested by plain `cargo test`; `app` draws it and needs the
//! `gui` feature. `belts` decides which belt picture each tile of the in-game view takes, also without a window;
//! `sprites` reads the game's pictures and `game_view` draws with them. The plan, and what each milestone adds, is
//! `docs/GUI_PLAN.md`.

pub mod belts;
pub mod form;
pub mod worker;

#[cfg(feature = "gui")]
pub mod app;

#[cfg(feature = "gui")]
pub mod preview;

#[cfg(feature = "gui")]
pub mod sprites;

#[cfg(feature = "gui")]
pub mod game_view;
