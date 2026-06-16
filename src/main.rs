#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result<()> {
    env_logger::init();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_title("nest"),
        ..Default::default()
    };
    eframe::run_native(
        "nest",
        options,
        Box::new(|cc| Ok(Box::new(nest::app::NestApp::new(cc)))),
    )
}

#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::wasm_bindgen::JsCast as _;

    console_error_panic_hook::set_once();
    let web_options = eframe::WebOptions::default();

    wasm_bindgen_futures::spawn_local(async {
        let document = web_sys::window()
            .expect("no window")
            .document()
            .expect("no document");
        let canvas = document
            .get_element_by_id("nest_canvas")
            .expect("missing #nest_canvas")
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .expect("#nest_canvas is not a canvas");

        eframe::WebRunner::new()
            .start(
                canvas,
                web_options,
                Box::new(|cc| Ok(Box::new(nest::app::NestApp::new(cc)))),
            )
            .await
            .expect("failed to start eframe");
    });
}
