mod app;
mod canvas_ink;
mod canvas_view;
mod emoji_util;
mod file_tree;
mod fonts;
mod git_sync;
mod graph_view;
mod icons;
mod image_paste;
mod image_size;
mod inline_images;
mod keybinds;
mod md_edit;
mod md_table;
mod media;
mod outline_tree;
mod preview;
mod state;
mod table_ui;
mod theme_apply;
mod update_check;

use app::CelestineApp;

fn main() -> eframe::Result<()> {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!(
        "../../../assets/icon/celestine-256.png"
    ))
    .expect("embedded app icon");

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_title("CelestineMD")
            // Match packaging/celestine.desktop (Wayland app_id / X11 WM_CLASS).
            .with_app_id("celestine")
            .with_icon(icon)
            // Solid window — no alpha framebuffer (avoids Hyprland/Omarchy
            // treating the surface as translucent / see-through).
            .with_transparent(false),
        ..Default::default()
    };
    eframe::run_native(
        "CelestineMD",
        native_options,
        Box::new(|cc| Ok(Box::new(CelestineApp::new(cc)))),
    )
}
