#![windows_subsystem = "windows"]

mod cache;
mod file_list;
mod settings;
mod viewer;

use eframe::egui;
use std::path::PathBuf;
use viewer::ViewerApp;

fn load_font(path: &str, scale: f32, y_offset: f32) -> Option<egui::FontData> {
    std::fs::read(path).ok().map(|data| {
        let mut fd = egui::FontData::from_owned(data);
        fd.tweak.scale = scale;
        fd.tweak.y_offset_factor = y_offset;
        fd
    })
}

fn configure_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    let primary_paths = [
        "C:/Windows/Fonts/msgothic.ttc",
        "C:/Windows/Fonts/meiryo.ttc",
        "C:/Windows/Fonts/YuGothR.ttc",
    ];
    for path in &primary_paths {
        if let Some(fd) = load_font(path, 1.0, 0.0) {
            fonts.font_data.insert("cjk_primary".to_owned(), std::sync::Arc::new(fd));
            fonts.families.get_mut(&egui::FontFamily::Proportional).unwrap()
                .insert(0, "cjk_primary".to_owned());
            fonts.families.get_mut(&egui::FontFamily::Monospace).unwrap()
                .push("cjk_primary".to_owned());
            break;
        }
    }

    let fallback_paths = [
        "C:/Windows/Fonts/msyh.ttc",
        "C:/Windows/Fonts/msjh.ttc",
    ];
    for path in &fallback_paths {
        if let Some(fd) = load_font(path, 1.0, 0.0) {
            fonts.font_data.insert("cjk_fallback".to_owned(), std::sync::Arc::new(fd));
            fonts.families.get_mut(&egui::FontFamily::Proportional).unwrap()
                .push("cjk_fallback".to_owned());
            break;
        }
    }

    ctx.set_fonts(fonts);
}

fn setup_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("{}\n\n{:?}", info, std::backtrace::Backtrace::force_capture());
        let log_path = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("crash.log")))
            .unwrap_or_else(|| std::path::PathBuf::from("crash.log"));
        let _ = std::fs::write(&log_path, &msg);
    }));
}

fn main() -> eframe::Result<()> {
    setup_panic_hook();
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
            let render_state = std::sync::Arc::new(
                cc.wgpu_render_state
                    .clone()
                    .expect("wgpu render state required"),
            );
            Ok(Box::new(ViewerApp::new(initial_path, render_state)))
        }),
    )
}
