//! YAiST desktop client: modules on the left, the module's panel in the
//! middle, the conversation on the right.

mod app;
mod art;
mod theme;
mod widgets;

fn main() -> eframe::Result {
    let viewport = egui::ViewportBuilder::default()
        .with_title("YAiST")
        .with_app_id("org.opensuse.yaist")
        .with_inner_size([1360.0, 860.0])
        .with_min_inner_size([980.0, 600.0])
        // The header is the title bar, as in the ArtCraft launcher.
        .with_decorations(false)
        .with_icon(
            eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/icons/hicolor/256x256/apps/org.opensuse.yaist.png"))
                .unwrap_or_default(),
        );
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(
        "YAiST",
        options,
        Box::new(|cc| {
            theme::install_fonts(&cc.egui_ctx);
            // SVG from files: the system's distribution logos.
            egui_extras::install_image_loaders(&cc.egui_ctx);
            theme::apply(&cc.egui_ctx);
            // Ctrl+Plus/Minus/0 go to the session's text size (see App::text_size).
            cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
            let ctx = cc.egui_ctx.clone();
            let session = yaist_ui::Session::start_zoomable(move || ctx.request_repaint());
            Ok(Box::new(app::App::new(session)))
        }),
    )
}
