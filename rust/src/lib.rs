//! Rust port of factorio_optimizer_v2, one module per Kotlin package and one file per Kotlin file, so
//! upstream changes can be re-applied file by file. Ported from upstream commit 022c312.

pub mod blocks;
pub mod config;
pub mod data;
pub mod emit;
pub mod geom;
pub mod place;
pub mod plan;
pub mod power;
pub mod route;
pub mod search;
pub mod util;
pub mod validate;
pub mod viz;
