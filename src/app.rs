//! The egui front-end: part library, settings, run controls and a live canvas.

use std::sync::{Arc, Mutex};

use eframe::egui;

use crate::geometry::{Aabb, Polygon, Pt};
use crate::model::{NestConfig, NestResult, Part, RotationMode, Unit};
use crate::svg;
use crate::worker::Engine;

/// A file the user picked, waiting to be turned into a part.
type ImportQueue = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

pub struct NestApp {
    parts: Vec<Part>,
    config: NestConfig,
    engine: Engine,
    next_id: u64,
    color_cycle: usize,

    // "Add rectangle" form (stored in mm).
    rect_w: f64,
    rect_h: f64,
    rect_as_sheet: bool,

    // Rotation UI helper.
    rot_steps: u32,
    rot_free: bool,

    // Units handling.
    units: Unit,
    /// DPI assumed for SVGs that don't declare physical units (default 96).
    import_dpi: f64,

    // Cached snapshot of the engine's best result, refreshed only when a new
    // generation is published — avoids cloning the whole layout every frame.
    cached_best: Option<NestResult>,
    cached_gen: u64,

    imports: ImportQueue,
    status: String,
}

const PALETTE: &[[u8; 3]] = &[
    [0x4e, 0x79, 0xa7],
    [0xf2, 0x8e, 0x2b],
    [0xe1, 0x57, 0x59],
    [0x76, 0xb7, 0xb2],
    [0x59, 0xa1, 0x4f],
    [0xed, 0xc9, 0x48],
    [0xb0, 0x7a, 0xa1],
    [0xff, 0x9d, 0xa7],
    [0x9c, 0x75, 0x5f],
    [0xba, 0xb0, 0xac],
];

impl Default for NestApp {
    fn default() -> Self {
        Self {
            parts: Vec::new(),
            config: NestConfig::default(),
            engine: Engine::default(),
            next_id: 1,
            color_cycle: 0,
            rect_w: 100.0,
            rect_h: 50.0,
            rect_as_sheet: false,
            rot_steps: 4,
            rot_free: false,
            units: Unit::Mm,
            import_dpi: 96.0,
            cached_best: None,
            cached_gen: u64::MAX,
            imports: Arc::new(Mutex::new(Vec::new())),
            status: "Add parts and a sheet, then press Start.".to_string(),
        }
    }
}

impl NestApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let mut app = Self::default();
        // Start with a usable default sheet so the user can press Start quickly.
        app.add_rectangle(600.0, 400.0, true, "Sheet".into());
        app
    }

    fn next_color(&mut self) -> [u8; 3] {
        let c = PALETTE[self.color_cycle % PALETTE.len()];
        self.color_cycle += 1;
        c
    }

    fn add_rectangle(&mut self, w: f64, h: f64, is_sheet: bool, name: String) {
        let poly = Polygon::from_points(vec![
            Pt::new(0.0, 0.0),
            Pt::new(w, 0.0),
            Pt::new(w, h),
            Pt::new(0.0, h),
        ]);
        let color = self.next_color();
        self.parts.push(Part {
            id: self.next_id,
            name,
            polygon: poly,
            quantity: 1,
            is_sheet,
            allow_mirror: false,
            color,
        });
        self.next_id += 1;
    }

    fn add_polygon_part(&mut self, mut poly: Polygon, name: String) {
        // Normalize so the bounding-box min corner is at the origin.
        let b = poly.bounds();
        poly.translate(-b.min.x, -b.min.y);
        let color = self.next_color();
        self.parts.push(Part {
            id: self.next_id,
            name,
            polygon: poly,
            quantity: 1,
            is_sheet: false,
            allow_mirror: false,
            color,
        });
        self.next_id += 1;
    }

    /// Millimetres per SVG user unit, given the assumed import DPI.
    fn mm_per_unit(&self) -> f64 {
        25.4 / self.import_dpi.max(1.0)
    }

    fn drain_imports(&mut self) {
        let drained: Vec<(String, Vec<u8>)> = {
            let mut q = self.imports.lock().unwrap();
            std::mem::take(&mut *q)
        };
        let scale = self.mm_per_unit();
        for (name, bytes) in drained {
            match svg::import_svg(&bytes, scale) {
                Some(poly) if !poly.is_empty() => {
                    self.add_polygon_part(poly, name.clone());
                    self.status = format!("Imported {name}");
                }
                _ => self.status = format!("Could not read geometry from {name}"),
            }
        }
    }

    fn sync_rotation(&mut self) {
        self.config.rotation = if self.rot_free {
            RotationMode::Free
        } else {
            RotationMode::Steps(self.rot_steps.max(1))
        };
    }

    fn start(&mut self) {
        self.sync_rotation();
        self.engine
            .start(self.parts.clone(), self.config.clone());
        // Force the cache to refresh against the fresh run.
        self.cached_best = None;
        self.cached_gen = u64::MAX;
        self.status = "Running…".into();
    }
}

impl eframe::App for NestApp {
    #[cfg(target_arch = "wasm32")]
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(&mut *self)
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.engine.tick();
        self.drain_imports();

        if self.engine.invalid() {
            self.status =
                "Nothing to nest: add at least one part and one sheet.".into();
        }

        // Refresh the cached result only when a new generation is published.
        let generation = self.engine.generation();
        if generation != self.cached_gen {
            self.cached_best = self.engine.best();
            self.cached_gen = generation;
        }

        egui::Panel::top("top").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("nest");
                ui.separator();
                let running = self.engine.is_running();
                if !running {
                    if ui.button("▶ Start").clicked() {
                        self.start();
                    }
                } else if ui.button("⏹ Stop").clicked() {
                    self.engine.stop();
                    self.status = "Stopped.".into();
                }
                if ui.button("💾 Export SVG").clicked() {
                    self.export();
                }
                ui.separator();
                ui.label(format!("gen {}", self.engine.generation()));
                if let Some(best) = &self.cached_best {
                    ui.separator();
                    ui.label(format!(
                        "placed {} · unplaced {} · sheets {}",
                        best.placements.len(),
                        best.unplaced,
                        best.sheets_used
                    ));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(&self.status);
                });
            });
        });

        egui::Panel::left("controls")
            .resizable(true)
            .default_size(320.0)
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    self.controls_ui(ui);
                });
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            self.canvas_ui(ui);
        });

        if self.engine.is_running() {
            ui.ctx().request_repaint();
        }
    }
}

impl NestApp {
    fn controls_ui(&mut self, ui: &mut egui::Ui) {
        let running = self.engine.is_running();
        let units = self.units;

        // Units & import scale (display units can change at any time).
        ui.horizontal(|ui| {
            ui.label("Units");
            egui::ComboBox::from_id_salt("units")
                .selected_text(self.units.label())
                .show_ui(ui, |ui| {
                    for u in Unit::ALL {
                        ui.selectable_value(&mut self.units, u, u.label());
                    }
                });
            ui.separator();
            ui.label("Import DPI")
                .on_hover_text(
                    "Pixels per inch assumed for SVGs without physical units.\n\
                     Leave at 96: SVGs that declare mm/cm/in/pt (OpenSCAD, Inkscape,\n\
                     Illustrator) import at their true real-world size.",
                );
            ui.add(
                egui::DragValue::new(&mut self.import_dpi)
                    .range(1.0..=100000.0)
                    .speed(1.0),
            );
            for (lbl, dpi) in [("96", 96.0), ("72", 72.0), ("90", 90.0)] {
                if ui.small_button(lbl).clicked() {
                    self.import_dpi = dpi;
                }
            }
        });
        ui.separator();

        ui.add_enabled_ui(!running, |ui| {
            ui.collapsing("Add geometry", |ui| {
                if ui.button("📂 Import SVG…").clicked() {
                    self.pick_svg();
                }
                ui.separator();
                ui.label("Add rectangle:");
                ui.horizontal(|ui| {
                    ui.label("w");
                    length_drag(ui, units, &mut self.rect_w, 1.0, 1.0e6, 1.0);
                    ui.label("h");
                    length_drag(ui, units, &mut self.rect_h, 1.0, 1.0e6, 1.0);
                });
                ui.checkbox(&mut self.rect_as_sheet, "Add as sheet");
                if ui.button("➕ Add rectangle").clicked() {
                    let name = if self.rect_as_sheet { "Sheet" } else { "Rect" };
                    let (w, h, is_sheet) = (self.rect_w, self.rect_h, self.rect_as_sheet);
                    self.add_rectangle(w, h, is_sheet, name.into());
                }
            });

            ui.collapsing("Settings", |ui| {
                egui::Grid::new("settings").num_columns(2).show(ui, |ui| {
                    ui.label("Part spacing");
                    length_drag(ui, units, &mut self.config.part_spacing, 0.0, 1000.0, 0.1);
                    ui.end_row();

                    ui.label("Edge spacing");
                    length_drag(ui, units, &mut self.config.edge_spacing, 0.0, 1000.0, 0.1);
                    ui.end_row();

                    ui.label("Population");
                    ui.add(egui::DragValue::new(&mut self.config.population).range(4..=512));
                    ui.end_row();

                    ui.label("Grid step")
                        .on_hover_text("Placement search resolution. Smaller = tighter packing but slower.");
                    length_drag(ui, units, &mut self.config.grid_step, 0.25, 100.0, 0.1);
                    ui.end_row();

                    ui.label("Threads (0 = all)");
                    ui.add(egui::DragValue::new(&mut self.config.threads).range(0..=256));
                    ui.end_row();
                });

                ui.separator();
                ui.label("Rotation");
                ui.checkbox(&mut self.rot_free, "Free (arbitrary angle)");
                ui.add_enabled_ui(!self.rot_free, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Steps");
                        ui.add(egui::Slider::new(&mut self.rot_steps, 1..=72));
                    });
                });

                ui.separator();
                ui.checkbox(&mut self.config.mirror_enabled, "Allow mirroring");
                ui.checkbox(&mut self.config.physics, "Physics compaction (jiggle/settle)");
            });
        });

        ui.separator();
        ui.heading("Parts & sheets");
        if self.parts.is_empty() {
            ui.label("Nothing added yet.");
        }

        let mut to_delete: Option<usize> = None;
        for i in 0..self.parts.len() {
            let id = self.parts[i].id;
            ui.push_id(id, |ui| {
                ui.add_enabled_ui(!running, |ui| {
                    let p = &mut self.parts[i];
                    let color = egui::Color32::from_rgb(p.color[0], p.color[1], p.color[2]);
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (rect, _) =
                                ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                            ui.painter().rect_filled(rect, 2.0, color);
                            ui.text_edit_singleline(&mut p.name);
                            if ui.button("🗑").clicked() {
                                to_delete = Some(i);
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut p.is_sheet, "Sheet");
                            ui.label(if p.is_sheet { "copies" } else { "qty" });
                            ui.add(egui::DragValue::new(&mut p.quantity).range(0..=10000));
                            ui.add_enabled(
                                !p.is_sheet,
                                egui::Checkbox::new(&mut p.allow_mirror, "mirror"),
                            );
                        });
                        let b = p.bounds();
                        let u = units;
                        ui.label(
                            egui::RichText::new(format!(
                                "{:.2} × {:.2} {}   area {:.2} {}²",
                                u.from_mm(b.width()),
                                u.from_mm(b.height()),
                                u.label(),
                                p.area() * u.per_mm() * u.per_mm(),
                                u.label(),
                            ))
                            .small()
                            .weak(),
                        );
                    });
                });
            });
        }
        if let Some(i) = to_delete {
            self.parts.remove(i);
        }
    }

    fn canvas_ui(&mut self, ui: &mut egui::Ui) {
        let best = self.cached_best.as_ref();
        let mut slots = self.engine.slots();

        // Before a run, preview the sheets defined in the current part list.
        if slots.is_empty() {
            let mut cursor_x = 0.0;
            for p in self.parts.iter().filter(|p| p.is_sheet) {
                let b = p.polygon.bounds();
                let w = b.width().max(1.0);
                for _ in 0..p.quantity.max(1) {
                    let poly = p.polygon.transformed(0.0, false, cursor_x - b.min.x, -b.min.y);
                    let bounds = poly.bounds();
                    slots.push(crate::model::SheetSlot {
                        polygon: poly,
                        bounds,
                        origin: Pt::new(cursor_x, 0.0),
                    });
                    cursor_x += w + w;
                }
            }
        }

        if slots.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label("Add a sheet to see the nesting canvas.");
            });
            return;
        }

        // World bounds across all sheets.
        let mut world = Aabb {
            min: Pt::new(f64::INFINITY, f64::INFINITY),
            max: Pt::new(f64::NEG_INFINITY, f64::NEG_INFINITY),
        };
        for s in &slots {
            world.min.x = world.min.x.min(s.bounds.min.x);
            world.min.y = world.min.y.min(s.bounds.min.y);
            world.max.x = world.max.x.max(s.bounds.max.x);
            world.max.y = world.max.y.max(s.bounds.max.y);
        }

        let avail = ui.available_rect_before_wrap();
        let painter = ui.painter_at(avail);
        painter.rect_filled(avail, 0.0, egui::Color32::from_gray(28));

        let margin = 16.0_f64;
        let ww = world.max.x - world.min.x;
        let wh = world.max.y - world.min.y;
        if ww <= 0.0 || wh <= 0.0 {
            return;
        }
        let avail_w = avail.width() as f64;
        let avail_h = avail.height() as f64;
        let scale = ((avail_w - 2.0 * margin) / ww)
            .min((avail_h - 2.0 * margin) / wh)
            .max(0.0001);
        let off_x = avail.min.x as f64 + margin + ((avail_w - 2.0 * margin) - ww * scale) * 0.5;
        let off_y = avail.min.y as f64 + margin + ((avail_h - 2.0 * margin) - wh * scale) * 0.5;

        let to_screen = |p: Pt| {
            egui::pos2(
                (off_x + (p.x - world.min.x) * scale) as f32,
                (off_y + (p.y - world.min.y) * scale) as f32,
            )
        };

        // Sheets.
        for s in &slots {
            draw_ring(&painter, &s.polygon.outer, to_screen, None, egui::Color32::from_gray(70), 1.5);
            for h in &s.polygon.holes {
                draw_ring(&painter, h, to_screen, None, egui::Color32::from_gray(50), 1.0);
            }
        }

        // Placed parts.
        if let Some(best) = best {
            for pl in &best.placements {
                let fill = egui::Color32::from_rgba_unmultiplied(
                    pl.color[0],
                    pl.color[1],
                    pl.color[2],
                    220,
                );
                draw_ring(
                    &painter,
                    &pl.polygon.outer,
                    to_screen,
                    Some(fill),
                    egui::Color32::BLACK,
                    1.0,
                );
                for h in &pl.polygon.holes {
                    draw_ring(
                        &painter,
                        h,
                        to_screen,
                        Some(egui::Color32::from_gray(28)),
                        egui::Color32::BLACK,
                        0.5,
                    );
                }
            }
        }

        // Scale bar: pick a "nice" round length close to ~90 px wide.
        let target_mm = 90.0 / scale;
        let target_disp = self.units.from_mm(target_mm);
        let nice_disp = nice_number(target_disp);
        let bar_mm = self.units.to_mm(nice_disp);
        let bar_px = (bar_mm * scale) as f32;
        let y = avail.max.y - 22.0;
        let x0 = avail.min.x + 16.0;
        let col = egui::Color32::from_gray(180);
        painter.line_segment(
            [egui::pos2(x0, y), egui::pos2(x0 + bar_px, y)],
            egui::Stroke::new(2.0, col),
        );
        for dx in [0.0, bar_px] {
            painter.line_segment(
                [egui::pos2(x0 + dx, y - 4.0), egui::pos2(x0 + dx, y + 4.0)],
                egui::Stroke::new(2.0, col),
            );
        }
        painter.text(
            egui::pos2(x0, y - 8.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{} {}", trim_float(nice_disp), self.units.label()),
            egui::FontId::proportional(12.0),
            col,
        );
    }

    fn export(&mut self) {
        let best = match self.engine.best() {
            Some(b) if !b.placements.is_empty() => b,
            _ => {
                self.status = "Nothing to export yet.".into();
                return;
            }
        };
        let slots = self.engine.slots();
        let out = svg::export_svg(&best, &slots);
        self.save_svg(out);
    }
}

// ---- Native file I/O ----
#[cfg(not(target_arch = "wasm32"))]
impl NestApp {
    fn pick_svg(&mut self) {
        if let Some(files) = rfd::FileDialog::new()
            .add_filter("SVG", &["svg"])
            .pick_files()
        {
            let mut q = self.imports.lock().unwrap();
            for f in files {
                let name = f
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "part.svg".into());
                if let Ok(bytes) = std::fs::read(&f) {
                    q.push((name, bytes));
                }
            }
        }
    }

    fn save_svg(&mut self, data: String) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("SVG", &["svg"])
            .set_file_name("nest.svg")
            .save_file()
        {
            match std::fs::write(&path, data) {
                Ok(_) => self.status = format!("Exported {}", path.display()),
                Err(e) => self.status = format!("Export failed: {e}"),
            }
        }
    }
}

// ---- Wasm file I/O ----
#[cfg(target_arch = "wasm32")]
impl NestApp {
    fn pick_svg(&mut self) {
        let queue = self.imports.clone();
        wasm_bindgen_futures::spawn_local(async move {
            if let Some(files) = rfd::AsyncFileDialog::new()
                .add_filter("SVG", &["svg"])
                .pick_files()
                .await
            {
                for f in files {
                    let name = f.file_name();
                    let bytes = f.read().await;
                    queue.lock().unwrap().push((name, bytes));
                }
            }
        });
    }

    fn save_svg(&mut self, data: String) {
        if download_text("nest.svg", &data, "image/svg+xml").is_some() {
            self.status = "Exported nest.svg (check downloads).".into();
        } else {
            self.status = "Export failed.".into();
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn download_text(filename: &str, text: &str, mime: &str) -> Option<()> {
    use wasm_bindgen::JsCast;
    let window = web_sys::window()?;
    let document = window.document()?;

    let array = js_sys::Array::new();
    array.push(&wasm_bindgen::JsValue::from_str(text));
    let bag = web_sys::BlobPropertyBag::new();
    bag.set_type(mime);
    let blob =
        web_sys::Blob::new_with_str_sequence_and_options(&array, &bag).ok()?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).ok()?;

    let anchor = document
        .create_element("a")
        .ok()?
        .dyn_into::<web_sys::HtmlAnchorElement>()
        .ok()?;
    anchor.set_href(&url);
    anchor.set_download(filename);
    anchor.click();
    let _ = web_sys::Url::revoke_object_url(&url);
    Some(())
}

/// Round a positive value down to the nearest 1/2/5 × 10ⁿ ("nice") number,
/// used for the scale bar.
fn nice_number(v: f64) -> f64 {
    if v <= 0.0 || !v.is_finite() {
        return 1.0;
    }
    let exp = v.log10().floor();
    let pow = 10f64.powf(exp);
    let frac = v / pow;
    let nice = if frac >= 5.0 {
        5.0
    } else if frac >= 2.0 {
        2.0
    } else {
        1.0
    };
    nice * pow
}

/// Format a float without trailing zeros (e.g. `2.5`, `100`).
fn trim_float(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

/// A [`egui::DragValue`] that edits a millimetre-valued field while displaying
/// and accepting input in the user's chosen [`Unit`]. Ranges and speed are given
/// in millimetres.
fn length_drag(
    ui: &mut egui::Ui,
    unit: Unit,
    value_mm: &mut f64,
    lo_mm: f64,
    hi_mm: f64,
    speed_mm: f64,
) {
    let mut disp = unit.from_mm(*value_mm);
    let lo = unit.from_mm(lo_mm);
    let hi = unit.from_mm(hi_mm);
    let speed = (speed_mm * unit.per_mm()).max(1.0e-4);
    let resp = ui.add(
        egui::DragValue::new(&mut disp)
            .range(lo..=hi)
            .speed(speed)
            .suffix(unit.suffix()),
    );
    if resp.changed() {
        *value_mm = unit.to_mm(disp);
    }
}

fn draw_ring(
    painter: &egui::Painter,
    ring: &[Pt],
    to_screen: impl Fn(Pt) -> egui::Pos2,
    fill: Option<egui::Color32>,
    stroke: egui::Color32,
    width: f32,
) {
    if ring.len() < 2 {
        return;
    }
    let pts: Vec<egui::Pos2> = ring.iter().map(|&p| to_screen(p)).collect();
    if let Some(fill) = fill {
        // Approximate fill (egui tessellates simple polygons; concave shapes
        // may show minor artifacts, acceptable for a live preview).
        painter.add(egui::Shape::convex_polygon(
            pts.clone(),
            fill,
            egui::Stroke::NONE,
        ));
    }
    painter.add(egui::Shape::closed_line(
        pts,
        egui::Stroke::new(width, stroke),
    ));
}
