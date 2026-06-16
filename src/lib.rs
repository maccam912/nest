//! `nest` — a parametric part-nesting tool (a Rust/egui take on deepnest).
//!
//! The same engine and UI compile for the native desktop app and for the
//! browser (wasm). See `main.rs` for the per-target entry points.

pub mod app;
pub mod geometry;
pub mod model;
pub mod nest;
pub mod rng;
pub mod svg;
pub mod worker;
