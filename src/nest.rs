//! The nesting engine: a genetic algorithm over part order / rotation / mirror,
//! decoded with a bottom-left grid placement and an optional physics-style
//! compaction pass.

use crate::geometry::{
    Aabb, Polygon, Pt, contained_with_margin, polygon_gap, polygons_overlap,
};
use crate::model::{NestConfig, NestResult, Part, Placement, RotationMode, SheetSlot};
use crate::rng::Rng;

/// One expanded part copy that must be placed.
#[derive(Clone)]
struct Instance {
    part_idx: usize,
    color: [u8; 3],
}

/// A candidate solution.
#[derive(Clone)]
struct Genome {
    /// Order in which instances are placed (a permutation of instance indices).
    order: Vec<usize>,
    /// Rotation (radians) per instance index.
    rot: Vec<f64>,
    /// Mirror flag per instance index.
    mirror: Vec<bool>,
}

/// A part whose rotation/mirror has been applied and re-normalized to the origin.
struct Oriented {
    poly: Polygon,
    bounds: Aabb,
}

/// A polygon already placed on a sheet, with cached bounds for fast rejection.
struct Placed {
    poly: Polygon,
    bounds: Aabb,
}

pub struct Nester {
    parts: Vec<Part>,
    instances: Vec<Instance>,
    slots: Vec<SheetSlot>,
    config: NestConfig,
    population: Vec<Genome>,
    angles: Vec<f64>,
    rng: Rng,
    pub best: NestResult,
    pub generation: u64,
    /// True when there is actually something to nest.
    pub ready: bool,
}

impl Nester {
    pub fn new(parts: Vec<Part>, config: NestConfig) -> Self {
        let mut instances = Vec::new();
        let mut slots = Vec::new();

        for (idx, p) in parts.iter().enumerate() {
            if p.is_sheet {
                continue;
            }
            for _ in 0..p.quantity {
                instances.push(Instance {
                    part_idx: idx,
                    color: p.color,
                });
            }
        }

        // Lay out the available sheet copies left-to-right in one coordinate
        // space, separated by a wide gutter so placements never straddle sheets.
        let mut cursor_x = 0.0;
        for p in parts.iter().filter(|p| p.is_sheet) {
            let b = p.polygon.bounds();
            let w = b.width().max(1.0);
            for _ in 0..p.quantity.max(1) {
                let origin = Pt::new(cursor_x, 0.0);
                let poly = p.polygon.transformed(0.0, false, cursor_x - b.min.x, -b.min.y);
                let bounds = poly.bounds();
                slots.push(SheetSlot { polygon: poly, bounds, origin });
                cursor_x += w + w; // one sheet width of gutter
            }
        }

        let angles = config.rotation.angles();
        let ready = !instances.is_empty() && !slots.is_empty();
        let rng = Rng::new(config.seed);

        let mut nester = Self {
            parts,
            instances,
            slots,
            config,
            population: Vec::new(),
            angles,
            rng,
            best: NestResult::default(),
            generation: 0,
            ready,
        };
        if nester.ready {
            nester.init_population();
        }
        nester
    }

    fn random_orientation(&mut self, inst: usize) -> (f64, bool) {
        let rot = match self.config.rotation {
            RotationMode::Free => self.rng.range(0.0, std::f64::consts::TAU),
            RotationMode::Steps(_) => self.angles[self.rng.below(self.angles.len())],
        };
        let part = &self.parts[self.instances[inst].part_idx];
        let mirror = self.config.mirror_enabled && part.allow_mirror && self.rng.chance(0.5);
        (rot, mirror)
    }

    fn random_genome(&mut self) -> Genome {
        let n = self.instances.len();
        let mut order: Vec<usize> = (0..n).collect();
        self.rng.shuffle(&mut order);
        let mut rot = vec![0.0; n];
        let mut mirror = vec![false; n];
        for inst in 0..n {
            let (r, m) = self.random_orientation(inst);
            rot[inst] = r;
            mirror[inst] = m;
        }
        Genome { order, rot, mirror }
    }

    fn init_population(&mut self) {
        let pop = self.config.population.max(4);
        let mut population = Vec::with_capacity(pop);
        // One area-descending seed helps the GA a lot: big parts first.
        let mut by_area: Vec<usize> = (0..self.instances.len()).collect();
        let areas: Vec<f64> = self
            .instances
            .iter()
            .map(|i| self.parts[i.part_idx].area())
            .collect();
        by_area.sort_by(|&a, &b| areas[b].partial_cmp(&areas[a]).unwrap());
        let mut rot = vec![0.0; self.instances.len()];
        let mirror = vec![false; self.instances.len()];
        for (inst, r) in rot.iter_mut().enumerate() {
            *r = self.angles.first().copied().unwrap_or(0.0);
            let _ = inst;
        }
        population.push(Genome { order: by_area, rot, mirror });
        for _ in 1..pop {
            let g = self.random_genome();
            population.push(g);
        }
        self.population = population;
        // Evaluate once so `best` is populated immediately.
        self.evaluate_and_select(true);
    }

    /// Run a single generation.
    pub fn step(&mut self) {
        if !self.ready {
            return;
        }
        self.breed();
        self.evaluate_and_select(false);
        self.generation += 1;
    }

    /// Produce the next population from the current one.
    fn breed(&mut self) {
        let pop = self.population.len();
        // Keep the current best two as elites (population[0..2] after a sort by
        // fitness done in evaluate_and_select).
        let elite = 2.min(pop);
        let mut next: Vec<Genome> = self.population[..elite].to_vec();
        while next.len() < pop {
            let a = self.tournament();
            let b = self.tournament();
            let mut child = self.crossover(a, b);
            self.mutate(&mut child);
            next.push(child);
        }
        self.population = next;
    }

    fn tournament(&mut self) -> usize {
        // Lower index = better (population kept sorted by fitness).
        let i = self.rng.below(self.population.len());
        let j = self.rng.below(self.population.len());
        i.min(j)
    }

    /// Order crossover (OX) for the permutation; uniform crossover for the
    /// rotation/mirror genes.
    fn crossover(&mut self, a: usize, b: usize) -> Genome {
        let pa = self.population[a].clone();
        let pb = &self.population[b];
        let n = pa.order.len();
        if n < 2 {
            return pa;
        }
        let mut lo = self.rng.below(n);
        let mut hi = self.rng.below(n);
        if lo > hi {
            std::mem::swap(&mut lo, &mut hi);
        }
        let mut child_order = vec![usize::MAX; n];
        let mut used = vec![false; n];
        for k in lo..=hi {
            child_order[k] = pa.order[k];
            used[pa.order[k]] = true;
        }
        let mut fill = (hi + 1) % n;
        for off in 0..n {
            let src = pb.order[(hi + 1 + off) % n];
            if !used[src] {
                child_order[fill] = src;
                used[src] = true;
                fill = (fill + 1) % n;
            }
        }

        let mut rot = vec![0.0; n];
        let mut mirror = vec![false; n];
        for inst in 0..n {
            if self.rng.chance(0.5) {
                rot[inst] = pa.rot[inst];
                mirror[inst] = pa.mirror[inst];
            } else {
                rot[inst] = pb.rot[inst];
                mirror[inst] = pb.mirror[inst];
            }
        }
        Genome { order: child_order, rot, mirror }
    }

    fn mutate(&mut self, g: &mut Genome) {
        let n = g.order.len();
        if n == 0 {
            return;
        }
        // Swap mutation on the order.
        if self.rng.chance(0.7) && n >= 2 {
            let i = self.rng.below(n);
            let j = self.rng.below(n);
            g.order.swap(i, j);
        }
        // Rotation mutation.
        if self.rng.chance(0.5) {
            let inst = self.rng.below(n);
            g.rot[inst] = match self.config.rotation {
                RotationMode::Free => {
                    // Small continuous nudge, occasionally a big jump.
                    if self.rng.chance(0.3) {
                        self.rng.range(0.0, std::f64::consts::TAU)
                    } else {
                        let d = self.rng.range(-0.4, 0.4);
                        (g.rot[inst] + d).rem_euclid(std::f64::consts::TAU)
                    }
                }
                RotationMode::Steps(_) => self.angles[self.rng.below(self.angles.len())],
            };
        }
        // Mirror mutation.
        if self.config.mirror_enabled && self.rng.chance(0.2) {
            let inst = self.rng.below(n);
            let part = &self.parts[self.instances[inst].part_idx];
            if part.allow_mirror {
                g.mirror[inst] = !g.mirror[inst];
            }
        }
    }

    /// Evaluate every genome, sort the population best-first, and update `best`.
    fn evaluate_and_select(&mut self, force_best: bool) {
        let results = self.evaluate_population();
        // Sort population by fitness, descending.
        let mut idx: Vec<usize> = (0..self.population.len()).collect();
        idx.sort_by(|&a, &b| results[b].fitness.partial_cmp(&results[a].fitness).unwrap());
        let new_pop: Vec<Genome> = idx.iter().map(|&i| self.population[i].clone()).collect();
        self.population = new_pop;

        let best_idx = idx[0];
        let mut best = results[best_idx].clone();
        best.generation = self.generation;
        if force_best || best.fitness > self.best.fitness {
            self.best = best;
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn evaluate_population(&self) -> Vec<NestResult> {
        use rayon::prelude::*;
        self.population
            .par_iter()
            .map(|g| self.decode(g))
            .collect()
    }

    #[cfg(target_arch = "wasm32")]
    fn evaluate_population(&self) -> Vec<NestResult> {
        self.population.iter().map(|g| self.decode(g)).collect()
    }

    /// Turn a genome into an actual layout and score it.
    fn decode(&self, g: &Genome) -> NestResult {
        let mut per_sheet: Vec<Vec<Placed>> = (0..self.slots.len()).map(|_| Vec::new()).collect();
        let mut placements: Vec<Placement> = Vec::new();
        let mut unplaced = 0usize;

        for &inst in &g.order {
            let oriented = self.orient(inst, g.rot[inst], g.mirror[inst]);
            let mut done = false;
            for (si, slot) in self.slots.iter().enumerate() {
                if let Some((dx, dy)) =
                    self.find_position(&oriented, slot, &per_sheet[si])
                {
                    let mut poly = oriented.poly.clone();
                    poly.translate(dx, dy);
                    let bounds = poly.bounds();
                    per_sheet[si].push(Placed { poly: poly.clone(), bounds });
                    placements.push(Placement {
                        part_id: self.parts[self.instances[inst].part_idx].id,
                        sheet_index: si,
                        rotation: g.rot[inst],
                        mirror: g.mirror[inst],
                        dx,
                        dy,
                        color: self.instances[inst].color,
                        polygon: poly,
                    });
                    done = true;
                    break;
                }
            }
            if !done {
                unplaced += 1;
            }
        }

        if self.config.physics {
            self.compact(&mut placements, &mut per_sheet);
        }

        self.score(placements, unplaced, per_sheet)
    }

    /// Apply rotation+mirror, then normalize so the part's bounding-box min
    /// corner sits at the origin (makes the grid scan straightforward).
    fn orient(&self, inst: usize, rot: f64, mirror: bool) -> Oriented {
        let base = &self.parts[self.instances[inst].part_idx].polygon;
        let mut poly = base.transformed(rot, mirror, 0.0, 0.0);
        let b = poly.bounds();
        poly.translate(-b.min.x, -b.min.y);
        let bounds = poly.bounds();
        Oriented { poly, bounds }
    }

    /// Bottom-left grid search for a feasible spot on one sheet slot.
    fn find_position(
        &self,
        oriented: &Oriented,
        slot: &SheetSlot,
        placed: &[Placed],
    ) -> Option<(f64, f64)> {
        let step = self.config.grid_step.max(0.25);
        let edge = self.config.edge_spacing;
        let pw = oriented.bounds.width();
        let ph = oriented.bounds.height();

        let x0 = slot.bounds.min.x + edge;
        let y0 = slot.bounds.min.y + edge;
        let x1 = slot.bounds.max.x - edge - pw;
        let y1 = slot.bounds.max.y - edge - ph;
        if x1 < x0 || y1 < y0 {
            return None;
        }

        let mut y = y0;
        while y <= y1 + 1e-9 {
            let mut x = x0;
            while x <= x1 + 1e-9 {
                if self.feasible(oriented, x, y, slot, placed) {
                    return Some((x, y));
                }
                x += step;
            }
            y += step;
        }
        None
    }

    fn feasible(
        &self,
        oriented: &Oriented,
        dx: f64,
        dy: f64,
        slot: &SheetSlot,
        placed: &[Placed],
    ) -> bool {
        // Candidate bounding box, used as a fast rejection gate below.
        let cand = Aabb {
            min: Pt::new(oriented.bounds.min.x + dx, oriented.bounds.min.y + dy),
            max: Pt::new(oriented.bounds.max.x + dx, oriented.bounds.max.y + dy),
        };

        let mut moved = oriented.poly.clone();
        moved.translate(dx, dy);

        // Must sit inside the sheet with the edge margin.
        if !contained_with_margin(&moved, &slot.polygon, self.config.edge_spacing) {
            return false;
        }

        // Must respect the part-to-part spacing against everything placed.
        for p in placed {
            if cand.separated_by(&p.bounds, self.config.part_spacing) {
                continue;
            }
            if polygons_overlap(&moved, &p.poly) {
                return false;
            }
            if self.config.part_spacing > 0.0
                && polygon_gap(&moved, &p.poly) < self.config.part_spacing
            {
                return false;
            }
        }
        true
    }

    /// Physics-style settling: nudge each placed part toward the sheet origin in
    /// small steps while it stays feasible. A crude but effective compaction /
    /// "annealing" pass.
    fn compact(&self, placements: &mut [Placement], per_sheet: &mut [Vec<Placed>]) {
        let step = (self.config.grid_step * 0.5).max(0.25);
        let passes = 4;
        for _ in 0..passes {
            for pl in placements.iter_mut() {
                let si = pl.sheet_index;
                let slot = &self.slots[si];
                // Find this placement's index within the sheet's Placed list.
                let me = per_sheet[si]
                    .iter()
                    .position(|p| same_origin(&p.poly, &pl.polygon))
                    .unwrap_or(0);

                for &(ddx, ddy) in &[(-step, 0.0), (0.0, -step), (-step, -step)] {
                    loop {
                        let mut moved = pl.polygon.clone();
                        moved.translate(ddx, ddy);
                        let bounds = moved.bounds();
                        if !contained_with_margin(&moved, &slot.polygon, self.config.edge_spacing) {
                            break;
                        }
                        let mut ok = true;
                        for (k, other) in per_sheet[si].iter().enumerate() {
                            if k == me {
                                continue;
                            }
                            if bounds.separated_by(&other.bounds, self.config.part_spacing) {
                                continue;
                            }
                            if polygons_overlap(&moved, &other.poly)
                                || (self.config.part_spacing > 0.0
                                    && polygon_gap(&moved, &other.poly) < self.config.part_spacing)
                            {
                                ok = false;
                                break;
                            }
                        }
                        if !ok {
                            break;
                        }
                        pl.dx += ddx;
                        pl.dy += ddy;
                        pl.polygon = moved.clone();
                        per_sheet[si][me] = Placed { poly: moved, bounds };
                    }
                }
            }
        }
    }

    fn score(
        &self,
        placements: Vec<Placement>,
        unplaced: usize,
        per_sheet: Vec<Vec<Placed>>,
    ) -> NestResult {
        let placed_count = placements.len();
        let mut sheets_used = 0usize;
        let mut placed_area = 0.0;
        let mut used_bbox_area = 0.0;
        let mut max_height = 0.0_f64;

        for parts in &per_sheet {
            if parts.is_empty() {
                continue;
            }
            sheets_used += 1;
            let mut minx = f64::INFINITY;
            let mut miny = f64::INFINITY;
            let mut maxx = f64::NEG_INFINITY;
            let mut maxy = f64::NEG_INFINITY;
            for p in parts {
                placed_area += p.poly.area();
                minx = minx.min(p.bounds.min.x);
                miny = miny.min(p.bounds.min.y);
                maxx = maxx.max(p.bounds.max.x);
                maxy = maxy.max(p.bounds.max.y);
            }
            used_bbox_area += (maxx - minx) * (maxy - miny);
            max_height = max_height.max(maxy - miny);
        }

        let util = if used_bbox_area > 0.0 {
            placed_area / used_bbox_area
        } else {
            0.0
        };

        // Maximize parts placed first, then minimize sheets, then maximize
        // material utilization, then prefer shorter packs.
        let fitness = placed_count as f64 * 1.0e6
            - sheets_used as f64 * 1.0e4
            + util * 1.0e3
            - max_height * 0.01;

        NestResult {
            placements,
            unplaced,
            sheets_used,
            fitness,
            generation: self.generation,
        }
    }

    pub fn sheet_slots(&self) -> &[SheetSlot] {
        &self.slots
    }
}

fn same_origin(a: &Polygon, b: &Polygon) -> bool {
    if a.outer.is_empty() || b.outer.is_empty() {
        return false;
    }
    (a.outer[0].x - b.outer[0].x).abs() < 1e-6 && (a.outer[0].y - b.outer[0].y).abs() < 1e-6
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{Polygon, Pt, polygon_gap, polygons_overlap};
    use crate::model::{NestConfig, Part, RotationMode};

    fn rect(id: u64, w: f64, h: f64, qty: u32, is_sheet: bool) -> Part {
        Part {
            id,
            name: format!("p{id}"),
            polygon: Polygon::from_points(vec![
                Pt::new(0.0, 0.0),
                Pt::new(w, 0.0),
                Pt::new(w, h),
                Pt::new(0.0, h),
            ]),
            quantity: qty,
            is_sheet,
            allow_mirror: false,
            color: [10, 20, 30],
        }
    }

    #[test]
    fn places_parts_and_respects_spacing() {
        // A 100x100 sheet and several 18x18 parts that should all fit.
        let parts = vec![rect(1, 100.0, 100.0, 1, true), rect(2, 18.0, 18.0, 9, false)];
        let config = NestConfig {
            part_spacing: 2.0,
            edge_spacing: 2.0,
            rotation: RotationMode::Steps(1),
            grid_step: 2.0,
            population: 8,
            ..Default::default()
        };
        let mut nester = Nester::new(parts, config);
        assert!(nester.ready);
        for _ in 0..10 {
            nester.step();
        }
        let best = &nester.best;
        assert!(best.placements.len() >= 4, "placed {}", best.placements.len());

        // No two placed parts may overlap or violate the spacing.
        for i in 0..best.placements.len() {
            for j in (i + 1)..best.placements.len() {
                let a = &best.placements[i];
                let b = &best.placements[j];
                if a.sheet_index != b.sheet_index {
                    continue;
                }
                assert!(!polygons_overlap(&a.polygon, &b.polygon), "overlap {i},{j}");
                assert!(polygon_gap(&a.polygon, &b.polygon) >= 2.0 - 1e-6, "too close {i},{j}");
            }
        }
    }

    #[test]
    fn empty_problem_is_not_ready() {
        let nester = Nester::new(vec![rect(1, 10.0, 10.0, 1, false)], NestConfig::default());
        assert!(!nester.ready, "no sheet => not ready");
    }
}
