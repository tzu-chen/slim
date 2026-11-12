//! slim: a fast terminal navigator for reading large Lean 4 projects.
//!
//! The crate is split into a library (index, importer, scanner, queries,
//! UI) and a thin binary (`src/main.rs`).

pub mod model;
pub mod source;
pub mod text;
pub mod axioms;
pub mod db;
pub mod nameindex;
pub mod paths;
pub mod process;
pub mod snapshot;
pub mod sourceview;
pub mod app;
pub mod axioms_check;
pub mod bench;
pub mod cli;
pub mod export;
pub mod userdata;
pub mod map;
pub mod relations;
pub mod imports;
pub mod skim;
pub mod scope;
pub mod leanrefs;
pub mod highlight;

/// Process start, recorded by `main` so start-up timings include index
/// resolution and terminal setup.
pub static PROCESS_START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
