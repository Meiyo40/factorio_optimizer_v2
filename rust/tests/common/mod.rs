#![allow(dead_code)]

use std::path::{Path, PathBuf};

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("the crate lives in the repo").to_path_buf()
}

/// Files with extension `ext` directly inside `dir` (relative to the repo root), sorted.
pub fn files_in(dir: &str, ext: &str) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(repo_root().join(dir))
        .unwrap_or_else(|e| panic!("cannot read {dir}: {e}"))
        .map(|entry| entry.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == ext))
        .collect();
    found.sort();
    found
}
