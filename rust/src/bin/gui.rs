//! The window: `cargo run --features gui --bin gui`. See `docs/GUI_PLAN.md`.

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default().with_inner_size([1280.0, 820.0]).with_min_inner_size([900.0, 560.0]),
        ..eframe::NativeOptions::default()
    };
    eframe::run_native(
        "factorio_optimizer",
        options,
        Box::new(|cc| Ok(Box::new(factorio_optimizer::gui::app::GuiApp::new(cc)))),
    )
}
