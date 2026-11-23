//! slim: a fast terminal navigator for reading large Lean 4 projects.
//!
//! The crate is split into a library (index, importer, scanner, queries,
//! UI) and a thin binary (`src/main.rs`).

// Everything this crate prints outside the UI goes through
// `text::emit`, which makes control characters visible: names and paths from
// an inspected project, and the output of sandboxed processes, must not be
// able to send escape sequences to the terminal. These shadow the standard
// macros for every module below.
macro_rules! print {
    ($($t:tt)*) => { $crate::text::emit(false, format_args!($($t)*)) };
}
macro_rules! println {
    () => { $crate::text::emit(false, format_args!("\n")) };
    ($($t:tt)*) => { $crate::text::emit(false, format_args!("{}\n", format_args!($($t)*))) };
}
macro_rules! eprint {
    ($($t:tt)*) => { $crate::text::emit(true, format_args!($($t)*)) };
}
macro_rules! eprintln {
    () => { $crate::text::emit(true, format_args!("\n")) };
    ($($t:tt)*) => { $crate::text::emit(true, format_args!("{}\n", format_args!($($t)*))) };
}

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
pub mod lake;
pub mod graph;
pub mod reading;
pub mod contract;
pub mod overlay;
pub mod diff;
pub mod highlight;

/// Process start, recorded by `main` so start-up timings include index
/// resolution and terminal setup.
pub static PROCESS_START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
