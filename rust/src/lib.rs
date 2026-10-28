//! Rust port of factorio_optimizer_v2, one module per Kotlin package and one file per Kotlin file, so
//! upstream changes can be re-applied file by file. Ported from upstream commit 022c312.

pub mod config;
pub mod data;
pub mod emit;
pub mod geom;
pub mod plan;
pub mod util;
