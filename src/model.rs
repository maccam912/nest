//! Problem definition (parts, sheets, configuration) and result types.

use crate::geometry::{Aabb, Polygon, Pt};

/// One library item the user has added. It is either a part to be nested
/// (`is_sheet == false`) or a sheet to nest *into* (`is_sheet == true`).
#[derive(Clone, Debug)]
pub struct Part {
    pub id: u64,
    pub name: String,
    /// Geometry normalized so its bounding-box min corner sits at the origin.
    pub polygon: Polygon,
    /// For parts: how many copies to nest. For sheets: how many copies are available.
    pub quantity: u32,
    /// Mark this item as a sheet (the container) rather than a part.
    pub is_sheet: bool,
    /// Allow this part to be mirrored (ignored for sheets).
    pub allow_mirror: bool,
    /// Display color (RGB).
    pub color: [u8; 3],
}

impl Part {
    pub fn bounds(&self) -> Aabb {
        self.polygon.bounds()
    }

    pub fn area(&self) -> f64 {
        self.polygon.area()
    }
}

/// How parts may be rotated when searching for a fit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RotationMode {
    /// A fixed set of `n` evenly-spaced angles (e.g. 4 → 0/90/180/270°).
    Steps(u32),
    /// Arbitrary continuous rotation chosen by the optimizer.
    Free,
}

impl RotationMode {
    /// Candidate angles (radians) for the discrete case. `Free` returns a fine
    /// sampling that the optimizer further perturbs continuously.
    pub fn angles(&self) -> Vec<f64> {
        match *self {
            RotationMode::Steps(n) => {
                let n = n.max(1);
                (0..n)
                    .map(|i| std::f64::consts::TAU * (i as f64) / (n as f64))
                    .collect()
            }
            RotationMode::Free => {
                // Seed the search with 12 starting angles; mutation refines.
                (0..12)
                    .map(|i| std::f64::consts::TAU * (i as f64) / 12.0)
                    .collect()
            }
        }
    }
}

/// Tunables for the nesting run.
#[derive(Clone, Debug)]
pub struct NestConfig {
    /// Minimum gap enforced between any two placed parts.
    pub part_spacing: f64,
    /// Minimum gap enforced between a part and the sheet edge (and holes).
    pub edge_spacing: f64,
    pub rotation: RotationMode,
    /// Global switch; a part also needs its own `allow_mirror` set.
    pub mirror_enabled: bool,
    /// Genetic-algorithm population size.
    pub population: usize,
    /// Placement search grid step, in sheet units. Smaller = tighter but slower.
    pub grid_step: f64,
    /// Run a physics-style "jiggle + compaction" pass on each candidate.
    pub physics: bool,
    /// Number of worker threads (native only; 0 = all cores).
    pub threads: usize,
    pub seed: u64,
}

impl Default for NestConfig {
    fn default() -> Self {
        Self {
            part_spacing: 2.0,
            edge_spacing: 2.0,
            rotation: RotationMode::Steps(4),
            mirror_enabled: false,
            population: 24,
            grid_step: 4.0,
            physics: false,
            threads: 0,
            seed: 0x9E3779B97F4A7C15,
        }
    }
}

/// A single concrete placement of a part copy on a sheet.
#[derive(Clone, Debug)]
pub struct Placement {
    /// Index into the expanded list of part instances.
    pub part_id: u64,
    /// Which sheet copy (0-based across all available sheet copies) it sits on.
    pub sheet_index: usize,
    pub rotation: f64,
    pub mirror: bool,
    pub dx: f64,
    pub dy: f64,
    pub color: [u8; 3],
    /// The fully transformed polygon, cached for drawing/export.
    pub polygon: Polygon,
}

/// The outcome of decoding one genome into an actual layout.
#[derive(Clone, Debug, Default)]
pub struct NestResult {
    pub placements: Vec<Placement>,
    pub unplaced: usize,
    pub sheets_used: usize,
    /// Higher is better.
    pub fitness: f64,
    pub generation: u64,
}

/// A resolved sheet copy: the container polygon and its position offset.
#[derive(Clone, Debug)]
pub struct SheetSlot {
    pub polygon: Polygon,
    pub bounds: Aabb,
    pub origin: Pt,
}
