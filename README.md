# nest

A parametric part-nesting tool — a Rust/[egui](https://github.com/emilk/egui) take on
[deepnest](https://deepnest.io). The same engine and UI compile to a native desktop
app **and** to WebAssembly for the browser.

## Features

- **Import SVGs** as parts (paths are flattened to polygons; the largest ring becomes
  the outline and inner rings become holes).
- **Add rectangles** manually as parts, or tick *Add as sheet* to make a container.
- **Quantities** per part, and per-sheet copy counts.
- **Multiple sheets at once** — the optimizer fills sheet 1, then 2, and so on.
- **Spacing constraints**: a minimum gap between parts, and a minimum gap from the
  sheet edge (and holes).
- **Rotation**: a fixed number of evenly-spaced angles, or free (arbitrary) rotation.
- **Mirroring**: allow parts to be flipped.
- **Genetic algorithm** over part order / rotation / mirror, decoded with a
  bottom-left placement. It runs continuously until you press *Stop*; you can then
  tweak settings or export.
- **Physics compaction** (optional): a jiggle/settle pass that slides parts toward the
  origin to consolidate free space.
- **Multicore**: native builds use a [rayon](https://github.com/rayon-rs/rayon) pool
  sized to the requested core count (0 = all cores). The browser build runs the same
  engine single-threaded, advancing each frame.
- **Export** the best layout to SVG.

## Units

Internally everything is stored in **millimetres**. Two controls in the UI:

- **Units** (mm / cm / in) — purely how values are shown and entered. A scale bar
  in the canvas shows the current unit.
- **Import DPI** (default 96) — only matters for SVGs that *don't* declare physical
  units.

How import scaling works: `usvg` resolves SVG coordinates to px at 96 DPI, converting
any physical unit (`mm`, `cm`, `in`, `pt`) it finds in the document. On import we
convert px → mm with `mm = px × 25.4 / DPI`. The upshot:

| Source | Behaviour at 96 DPI |
| --- | --- |
| **OpenSCAD** (exports `width="…mm"`, 1 user unit = 1 mm) | imports at true size ✅ |
| **Inkscape ≥ 0.92** (96 DPI, mm-aware) | true size ✅ |
| **Illustrator / PDF** (points, "72 units per inch" = `pt`) | true size ✅ |
| **Unit-less / px-only SVG** | treated as px; set Import DPI to taste |

So for almost all real CAD/vector exports you can leave Import DPI at 96 and parts
come in at their real-world dimensions. Only legacy unit-less files need adjustment
(common presets: 96, 72, 90).

## Run the native app

```bash
cargo run --release
```

## Run tests

```bash
cargo test
```

## Build / serve the web version

Requires [Trunk](https://trunkrs.dev):

```bash
rustup target add wasm32-unknown-unknown
cargo install --locked trunk
trunk serve            # dev server at http://localhost:8080
trunk build --release  # static site in ./dist
```

## Deploy to GitHub Pages

`.github/workflows/deploy-pages.yml` builds the web version with Trunk and publishes
`dist/` to GitHub Pages on every push to `main`. Enable it under
**Settings → Pages → Build and deployment → Source: GitHub Actions**.

## Project layout

| File | Purpose |
| --- | --- |
| `src/geometry.rs` | 2D primitives: transforms, overlap, distance, containment |
| `src/model.rs` | Parts, sheets, configuration, result types |
| `src/svg.rs` | SVG import (flatten to polygons) and layout export |
| `src/nest.rs` | Placement decoder + genetic algorithm + compaction |
| `src/worker.rs` | Continuous run driver (rayon thread on native, per-frame on wasm) |
| `src/app.rs` | egui UI: part library, settings, controls, live canvas |
| `src/rng.rs` | Tiny deterministic PRNG (SplitMix64) |

## Notes & limitations

- Placement uses a grid-based bottom-left search rather than full no-fit-polygon
  geometry, so the *Grid step* setting trades packing tightness for speed.
- The live canvas fills concave shapes approximately (correct outlines, minor fill
  artifacts on concave parts); SVG export is exact.
