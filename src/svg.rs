//! SVG import (parse + flatten to polygons) and export (write a nested layout).

use crate::geometry::{Polygon, Pt, ring_signed_area, point_in_ring};
use crate::model::{NestResult, SheetSlot};

/// Parse SVG bytes into a single part polygon, scaled to millimetres.
///
/// `usvg` resolves coordinates to px at its configured DPi (96), with physical
/// units (`mm`/`cm`/`in`/`pt`) already converted. `mm_per_unit` scales those px
/// into millimetres — pass `25.4 / dpi`. For SVGs that declare physical units,
/// `dpi == 96` recovers their true real-world size.
///
/// All sub-paths are flattened to line segments; the largest ring becomes the
/// outer boundary and any rings fully contained within it become holes. Returns
/// `None` if no usable geometry is found.
pub fn import_svg(data: &[u8], mm_per_unit: f64) -> Option<Polygon> {
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_data(data, &opt).ok()?;

    let mut rings: Vec<Vec<Pt>> = Vec::new();
    collect_group(tree.root(), &mut rings);

    if mm_per_unit != 1.0 {
        for ring in &mut rings {
            for p in ring {
                p.x *= mm_per_unit;
                p.y *= mm_per_unit;
            }
        }
    }

    let mut rings: Vec<Vec<Pt>> = rings.into_iter().filter(|r| r.len() >= 3).collect();
    if rings.is_empty() {
        return None;
    }

    // Largest-area ring is the outer boundary.
    rings.sort_by(|a, b| {
        ring_signed_area(b)
            .abs()
            .partial_cmp(&ring_signed_area(a).abs())
            .unwrap()
    });
    let outer = rings.remove(0);

    // Ensure CCW outer winding for consistency.
    let mut outer = outer;
    if ring_signed_area(&outer) < 0.0 {
        outer.reverse();
    }

    let mut holes = Vec::new();
    for r in rings {
        // Treat as a hole only if it sits inside the outer ring.
        if r.iter().all(|&p| point_in_ring(&outer, p)) {
            holes.push(r);
        }
    }

    Some(Polygon { outer, holes })
}

fn collect_group(group: &usvg::Group, out: &mut Vec<Vec<Pt>>) {
    for node in group.children() {
        match node {
            usvg::Node::Group(g) => collect_group(g, out),
            usvg::Node::Path(path) => collect_path(path, out),
            _ => {}
        }
    }
}

fn collect_path(path: &usvg::Path, out: &mut Vec<Vec<Pt>>) {
    let t = path.abs_transform();
    let map = |x: f32, y: f32| -> Pt {
        let nx = t.sx * x + t.kx * y + t.tx;
        let ny = t.ky * x + t.sy * y + t.ty;
        Pt::new(nx as f64, ny as f64)
    };

    let mut current: Vec<Pt> = Vec::new();
    let mut last = Pt::new(0.0, 0.0);

    for seg in path.data().segments() {
        use usvg::tiny_skia_path::PathSegment;
        match seg {
            PathSegment::MoveTo(p) => {
                if current.len() >= 3 {
                    out.push(std::mem::take(&mut current));
                } else {
                    current.clear();
                }
                last = map(p.x, p.y);
                current.push(last);
            }
            PathSegment::LineTo(p) => {
                last = map(p.x, p.y);
                current.push(last);
            }
            PathSegment::QuadTo(c, p) => {
                let c = map(c.x, c.y);
                let e = map(p.x, p.y);
                flatten_quad(last, c, e, &mut current);
                last = e;
            }
            PathSegment::CubicTo(c1, c2, p) => {
                let c1 = map(c1.x, c1.y);
                let c2 = map(c2.x, c2.y);
                let e = map(p.x, p.y);
                flatten_cubic(last, c1, c2, e, &mut current);
                last = e;
            }
            PathSegment::Close => {
                if current.len() >= 3 {
                    out.push(std::mem::take(&mut current));
                } else {
                    current.clear();
                }
            }
        }
    }
    if current.len() >= 3 {
        out.push(current);
    }
}

const CURVE_STEPS: usize = 16;

fn flatten_quad(p0: Pt, c: Pt, p1: Pt, out: &mut Vec<Pt>) {
    for i in 1..=CURVE_STEPS {
        let t = i as f64 / CURVE_STEPS as f64;
        let mt = 1.0 - t;
        let x = mt * mt * p0.x + 2.0 * mt * t * c.x + t * t * p1.x;
        let y = mt * mt * p0.y + 2.0 * mt * t * c.y + t * t * p1.y;
        out.push(Pt::new(x, y));
    }
}

fn flatten_cubic(p0: Pt, c1: Pt, c2: Pt, p1: Pt, out: &mut Vec<Pt>) {
    for i in 1..=CURVE_STEPS {
        let t = i as f64 / CURVE_STEPS as f64;
        let mt = 1.0 - t;
        let a = mt * mt * mt;
        let b = 3.0 * mt * mt * t;
        let c = 3.0 * mt * t * t;
        let d = t * t * t;
        let x = a * p0.x + b * c1.x + c * c2.x + d * p1.x;
        let y = a * p0.y + b * c1.y + c * c2.y + d * p1.y;
        out.push(Pt::new(x, y));
    }
}

/// Render a nesting result to a standalone SVG string. Used sheets are stacked
/// vertically with a gap and labeled.
pub fn export_svg(result: &NestResult, slots: &[SheetSlot]) -> String {
    // Group placements by sheet.
    let mut by_sheet: std::collections::BTreeMap<usize, Vec<&crate::model::Placement>> =
        Default::default();
    for p in &result.placements {
        by_sheet.entry(p.sheet_index).or_default().push(p);
    }

    let gap = 20.0;
    let mut total_w: f64 = 0.0;
    let mut total_h: f64 = gap;
    for si in by_sheet.keys() {
        let b = slots[*si].bounds;
        total_w = total_w.max(b.width());
        total_h += b.height() + gap;
    }
    total_w = (total_w + 2.0 * gap).max(100.0);

    let mut s = String::new();
    s.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{:.2}\" height=\"{:.2}\" viewBox=\"0 0 {:.2} {:.2}\">\n",
        total_w, total_h, total_w, total_h
    ));
    s.push_str("<rect width=\"100%\" height=\"100%\" fill=\"white\"/>\n");

    let mut y_cursor = gap;
    for (si, parts) in &by_sheet {
        let slot = &slots[*si];
        let b = slot.bounds;
        let ox = gap - b.min.x;
        let oy = y_cursor - b.min.y;

        // Sheet outline.
        s.push_str(&ring_path(&slot.polygon.outer, ox, oy, "none", "#222", 1.0));
        for h in &slot.polygon.holes {
            s.push_str(&ring_path(h, ox, oy, "none", "#bbb", 0.5));
        }

        for p in parts {
            let [r, g, bl] = p.color;
            let fill = format!("#{:02x}{:02x}{:02x}", r, g, bl);
            s.push_str(&ring_path(&p.polygon.outer, ox, oy, &fill, "#000", 0.4));
            for h in &p.polygon.holes {
                s.push_str(&ring_path(h, ox, oy, "white", "#000", 0.4));
            }
        }

        s.push_str(&format!(
            "<text x=\"{:.2}\" y=\"{:.2}\" font-size=\"10\" fill=\"#444\">Sheet {}</text>\n",
            gap,
            y_cursor - 4.0,
            si + 1
        ));

        y_cursor += b.height() + gap;
    }

    s.push_str("</svg>\n");
    s
}

fn ring_path(ring: &[Pt], ox: f64, oy: f64, fill: &str, stroke: &str, sw: f64) -> String {
    if ring.is_empty() {
        return String::new();
    }
    let mut d = String::from("<path d=\"M ");
    for (i, p) in ring.iter().enumerate() {
        if i > 0 {
            d.push_str("L ");
        }
        d.push_str(&format!("{:.3} {:.3} ", p.x + ox, p.y + oy));
    }
    d.push_str(&format!(
        "Z\" fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\"/>\n",
        fill, stroke, sw
    ));
    d
}
