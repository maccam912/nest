//! Lightweight 2D geometry primitives used by the nesting engine.
//!
//! We deliberately roll our own (instead of pulling in a heavy geometry crate)
//! so the exact same code compiles unchanged for native and `wasm32`.

/// A 2D point / vector.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pt {
    pub x: f64,
    pub y: f64,
}

impl Pt {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn sub(self, o: Pt) -> Pt {
        Pt::new(self.x - o.x, self.y - o.y)
    }

    pub fn add(self, o: Pt) -> Pt {
        Pt::new(self.x + o.x, self.y + o.y)
    }

    pub fn dot(self, o: Pt) -> f64 {
        self.x * o.x + self.y * o.y
    }

    /// 2D cross product (z component).
    pub fn cross(self, o: Pt) -> f64 {
        self.x * o.y - self.y * o.x
    }

    pub fn len(self) -> f64 {
        self.dot(self).sqrt()
    }
}

/// An axis-aligned bounding box.
#[derive(Clone, Copy, Debug)]
pub struct Aabb {
    pub min: Pt,
    pub max: Pt,
}

impl Aabb {
    pub fn width(&self) -> f64 {
        self.max.x - self.min.x
    }
    pub fn height(&self) -> f64 {
        self.max.y - self.min.y
    }
    pub fn area(&self) -> f64 {
        self.width() * self.height()
    }

    /// True if the two boxes are separated by at least `gap` along either axis.
    pub fn separated_by(&self, o: &Aabb, gap: f64) -> bool {
        self.max.x + gap < o.min.x
            || o.max.x + gap < self.min.x
            || self.max.y + gap < o.min.y
            || o.max.y + gap < self.min.y
    }
}

/// A simple polygon: an outer ring plus zero or more holes.
///
/// Rings are stored as explicit point lists (not closed — the closing edge is
/// implicit between the last and first vertices).
#[derive(Clone, Debug, Default)]
pub struct Polygon {
    pub outer: Vec<Pt>,
    pub holes: Vec<Vec<Pt>>,
}

impl Polygon {
    pub fn from_points(outer: Vec<Pt>) -> Self {
        Self {
            outer,
            holes: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.outer.len() < 3
    }

    /// Signed area of the outer ring (positive for CCW winding).
    pub fn signed_area(&self) -> f64 {
        ring_signed_area(&self.outer)
    }

    /// Net area (outer minus holes), always non-negative.
    pub fn area(&self) -> f64 {
        let mut a = self.signed_area().abs();
        for h in &self.holes {
            a -= ring_signed_area(h).abs();
        }
        a.max(0.0)
    }

    pub fn bounds(&self) -> Aabb {
        ring_bounds(&self.outer)
    }

    pub fn centroid(&self) -> Pt {
        let b = self.bounds();
        Pt::new((b.min.x + b.max.x) * 0.5, (b.min.y + b.max.y) * 0.5)
    }

    /// Apply an affine transform built from rotation (radians), optional X-mirror,
    /// then translation. Returns a new polygon.
    pub fn transformed(&self, rot: f64, mirror: bool, dx: f64, dy: f64) -> Polygon {
        let (s, c) = rot.sin_cos();
        let mx = if mirror { -1.0 } else { 1.0 };
        let map = |p: &Pt| {
            let x = p.x * mx;
            let y = p.y;
            Pt::new(c * x - s * y + dx, s * x + c * y + dy)
        };
        Polygon {
            outer: self.outer.iter().map(map).collect(),
            holes: self.holes.iter().map(|h| h.iter().map(map).collect()).collect(),
        }
    }

    /// Translate in place.
    pub fn translate(&mut self, dx: f64, dy: f64) {
        for p in &mut self.outer {
            p.x += dx;
            p.y += dy;
        }
        for h in &mut self.holes {
            for p in h {
                p.x += dx;
                p.y += dy;
            }
        }
    }

    /// True if `p` lies inside the outer ring and outside all holes.
    pub fn contains_point(&self, p: Pt) -> bool {
        if !point_in_ring(&self.outer, p) {
            return false;
        }
        for h in &self.holes {
            if point_in_ring(h, p) {
                return false;
            }
        }
        true
    }
}

pub fn ring_signed_area(ring: &[Pt]) -> f64 {
    if ring.len() < 3 {
        return 0.0;
    }
    let mut a = 0.0;
    let n = ring.len();
    for i in 0..n {
        let j = (i + 1) % n;
        a += ring[i].x * ring[j].y - ring[j].x * ring[i].y;
    }
    a * 0.5
}

pub fn ring_bounds(ring: &[Pt]) -> Aabb {
    let mut min = Pt::new(f64::INFINITY, f64::INFINITY);
    let mut max = Pt::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in ring {
        min.x = min.x.min(p.x);
        min.y = min.y.min(p.y);
        max.x = max.x.max(p.x);
        max.y = max.y.max(p.y);
    }
    Aabb { min, max }
}

/// Even-odd point-in-ring test.
pub fn point_in_ring(ring: &[Pt], p: Pt) -> bool {
    let n = ring.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let a = ring[i];
        let b = ring[j];
        if (a.y > p.y) != (b.y > p.y) {
            let t = (p.y - a.y) / (b.y - a.y);
            let x = a.x + t * (b.x - a.x);
            if p.x < x {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// Do segments `p1->p2` and `p3->p4` intersect (including touching)?
pub fn segments_intersect(p1: Pt, p2: Pt, p3: Pt, p4: Pt) -> bool {
    let d1 = orient(p3, p4, p1);
    let d2 = orient(p3, p4, p2);
    let d3 = orient(p1, p2, p3);
    let d4 = orient(p1, p2, p4);
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return true;
    }
    on_seg(p3, p4, p1, d1)
        || on_seg(p3, p4, p2, d2)
        || on_seg(p1, p2, p3, d3)
        || on_seg(p1, p2, p4, d4)
}

fn orient(a: Pt, b: Pt, c: Pt) -> f64 {
    (b.sub(a)).cross(c.sub(a))
}

fn on_seg(a: Pt, b: Pt, c: Pt, d: f64) -> bool {
    d.abs() < 1e-9
        && c.x.min_max_within(a.x, b.x)
        && c.y.min_max_within(a.y, b.y)
}

trait Within {
    fn min_max_within(self, a: f64, b: f64) -> bool;
}
impl Within for f64 {
    fn min_max_within(self, a: f64, b: f64) -> bool {
        self >= a.min(b) - 1e-9 && self <= a.max(b) + 1e-9
    }
}

/// Squared distance from point `p` to segment `a->b`.
pub fn point_seg_dist2(p: Pt, a: Pt, b: Pt) -> f64 {
    let ab = b.sub(a);
    let l2 = ab.dot(ab);
    if l2 <= 1e-18 {
        let d = p.sub(a);
        return d.dot(d);
    }
    let t = (p.sub(a).dot(ab) / l2).clamp(0.0, 1.0);
    let proj = Pt::new(a.x + t * ab.x, a.y + t * ab.y);
    let d = p.sub(proj);
    d.dot(d)
}

/// Minimum distance between two segments.
pub fn seg_seg_dist(p1: Pt, p2: Pt, p3: Pt, p4: Pt) -> f64 {
    if segments_intersect(p1, p2, p3, p4) {
        return 0.0;
    }
    let d = point_seg_dist2(p1, p3, p4)
        .min(point_seg_dist2(p2, p3, p4))
        .min(point_seg_dist2(p3, p1, p2))
        .min(point_seg_dist2(p4, p1, p2));
    d.sqrt()
}

fn ring_edges(ring: &[Pt]) -> impl Iterator<Item = (Pt, Pt)> + '_ {
    let n = ring.len();
    (0..n).map(move |i| (ring[i], ring[(i + 1) % n]))
}

/// Do two polygons overlap (share interior area)? Holes are honored: a part
/// nested entirely inside another part's hole does *not* overlap it.
pub fn polygons_overlap(a: &Polygon, b: &Polygon) -> bool {
    // Edge crossings between the two outer rings => overlap.
    for (a1, a2) in ring_edges(&a.outer) {
        for (b1, b2) in ring_edges(&b.outer) {
            if segments_intersect(a1, a2, b1, b2) {
                return true;
            }
        }
    }
    // No crossings: either disjoint, or one contains the other.
    if a.contains_point(b.outer[0]) {
        return true;
    }
    if b.contains_point(a.outer[0]) {
        return true;
    }
    false
}

/// Minimum gap between the boundaries of two non-overlapping polygons.
/// Returns 0.0 if they overlap.
pub fn polygon_gap(a: &Polygon, b: &Polygon) -> f64 {
    if polygons_overlap(a, b) {
        return 0.0;
    }
    let mut best = f64::INFINITY;
    for (a1, a2) in ring_edges(&a.outer) {
        for (b1, b2) in ring_edges(&b.outer) {
            best = best.min(seg_seg_dist(a1, a2, b1, b2));
        }
    }
    best
}

/// Is polygon `inner` fully contained inside `outer` with at least `margin`
/// clearance from the boundary (and from holes)?
pub fn contained_with_margin(inner: &Polygon, outer: &Polygon, margin: f64) -> bool {
    // Every inner vertex must be inside the outer region.
    for &p in &inner.outer {
        if !outer.contains_point(p) {
            return false;
        }
    }
    // No edge of inner may cross any ring of outer.
    let inner_edges: Vec<(Pt, Pt)> = ring_edges(&inner.outer).collect();
    let check_ring = |ring: &[Pt]| -> bool {
        for (b1, b2) in ring_edges(ring) {
            for &(a1, a2) in &inner_edges {
                if segments_intersect(a1, a2, b1, b2) {
                    return false;
                }
                if margin > 0.0 && seg_seg_dist(a1, a2, b1, b2) < margin {
                    return false;
                }
            }
        }
        true
    };
    if !check_ring(&outer.outer) {
        return false;
    }
    for h in &outer.holes {
        if !check_ring(h) {
            return false;
        }
    }
    true
}

/// Translate a copy of every point in `ring` and return a fresh ring.
pub fn translate_ring(ring: &[Pt], dx: f64, dy: f64) -> Vec<Pt> {
    ring.iter().map(|p| Pt::new(p.x + dx, p.y + dy)).collect()
}
