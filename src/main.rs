#![windows_subsystem = "windows"]

mod cache;
mod file_list;
mod settings;
mod viewer;

use eframe::egui;
use std::path::PathBuf;
use viewer::ViewerApp;

fn configure_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    // Try to load a Japanese system font from Windows.
    let font_paths = [
        "C:/Windows/Fonts/msgothic.ttc",
        "C:/Windows/Fonts/YuGothR.ttc",
    ];
    for path in &font_paths {
        if let Ok(font_data) = std::fs::read(path) {
            fonts.font_data.insert(
                "system_jp".to_owned(),
                std::sync::Arc::new(egui::FontData::from_owned(font_data)),
            );
            fonts
                .families
                .get_mut(&egui::FontFamily::Proportional)
                .unwrap()
                .insert(0, "system_jp".to_owned());
            fonts
                .families
                .get_mut(&egui::FontFamily::Monospace)
                .unwrap()
                .push("system_jp".to_owned());
            break;
        }
    }

    ctx.set_fonts(fonts);
}

fn main() -> eframe::Result<()> {
    env_logger::init();

    let initial_path: Option<PathBuf> = std::env::args_os().nth(1).map(PathBuf::from);

    let title = match &initial_path {
        Some(path) => {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Visual Media Viewer".to_string());
            format!("{} - Visual Media Viewer", name)
        }
        None => "Visual Media Viewer".to_string(),
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 720.0])
            .with_title(&title)
            .with_drag_and_drop(true)
            .with_decorations(false),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };

    eframe::run_native(
        "Visual Media Viewer",
        options,
        Box::new(move |cc| {
            configure_fonts(&cc.egui_ctx);
            Ok(Box::new(ViewerApp::new(initial_path)))
        }),
    )
}
