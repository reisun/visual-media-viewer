mod cache;
mod file_list;
mod viewer;

use eframe::egui;
use std::path::PathBuf;
use viewer::ViewerApp;

fn main() -> eframe::Result<()> {
    env_logger::init();

    // Parse command-line arguments: first argument is the image file path.
    let initial_path: Option<PathBuf> = std::env::args().nth(1).map(PathBuf::from);

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
            .with_title(&title),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };

    eframe::run_native(
        "Visual Media Viewer",
        options,
        Box::new(move |_cc| Ok(Box::new(ViewerApp::new(initial_path)))),
    )
}
