#![windows_subsystem = "windows"]

mod app_icon;
mod cache;
mod file_list;
mod folder_ops;
mod image_decode;
mod ipc;
mod media_overlay;
mod settings;
mod shell_integration;
mod shell_ui;
mod startup;
mod thumbnail_worker;
mod title_bar;
mod video_player;
mod viewer;
mod wic_decoder;

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
            fonts
                .font_data
                .insert("cjk_primary".to_owned(), std::sync::Arc::new(fd));
            fonts
                .families
                .get_mut(&egui::FontFamily::Proportional)
                .unwrap()
                .insert(0, "cjk_primary".to_owned());
            fonts
                .families
                .get_mut(&egui::FontFamily::Monospace)
                .unwrap()
                .push("cjk_primary".to_owned());
            break;
        }
    }

    let fallback_paths = ["C:/Windows/Fonts/msyh.ttc", "C:/Windows/Fonts/msjh.ttc"];
    for path in &fallback_paths {
        if let Some(fd) = load_font(path, 1.0, 0.0) {
            fonts
                .font_data
                .insert("cjk_fallback".to_owned(), std::sync::Arc::new(fd));
            fonts
                .families
                .get_mut(&egui::FontFamily::Proportional)
                .unwrap()
                .push("cjk_fallback".to_owned());
            break;
        }
    }

    ctx.set_fonts(fonts);
}

fn setup_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let msg = format!(
            "{}\n\n{:?}",
            info,
            std::backtrace::Backtrace::force_capture()
        );
        let log_path = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("crash.log")))
            .unwrap_or_else(|| std::path::PathBuf::from("crash.log"));
        let _ = std::fs::write(&log_path, &msg);
    }));
}

fn setup_file_logger() {
    use std::io::Write;
    let log_path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("debug.log")))
        .unwrap_or_else(|| std::path::PathBuf::from("debug.log"));
    let file = std::fs::File::create(&log_path).ok();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format(move |buf, record| {
            let ts = buf.timestamp_millis();
            writeln!(
                buf,
                "[{} {} {}] {}",
                ts,
                record.level(),
                record.target(),
                record.args()
            )
        })
        .target(if file.is_some() {
            env_logger::Target::Pipe(Box::new(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&log_path)
                    .unwrap(),
            ))
        } else {
            env_logger::Target::Stderr
        })
        .init();
}

fn main() -> eframe::Result<()> {
    if std::env::args_os().nth(1).as_deref()
        == Some(std::ffi::OsStr::new("--shell-thumbnail-worker"))
    {
        let size = std::env::args()
            .nth(2)
            .filter(|value| value == "--size")
            .and_then(|_| std::env::args().nth(3))
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(256);
        if thumbnail_worker::run(size).is_err() {
            std::process::exit(1);
        }
        return Ok(());
    }
    setup_panic_hook();
    setup_file_logger();

    let initial_path: Option<PathBuf> = std::env::args_os().nth(1).map(PathBuf::from);

    if let Some(ref path) = initial_path {
        if ipc::try_send_to_existing(path) {
            return Ok(());
        }
    }

    let ipc_rx = ipc::start_listener();

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

    let icon = app_icon::load();
    let saved_settings = settings::Settings::load();

    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([saved_settings.window_width, saved_settings.window_height])
        .with_maximized(saved_settings.maximized)
        .with_title(&title)
        .with_drag_and_drop(true)
        .with_decorations(false)
        .with_icon(std::sync::Arc::new(icon));

    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };

    eframe::run_native(
        "Visual Media Viewer",
        options,
        Box::new(move |cc| {
            #[cfg(windows)]
            app_icon::set_taskbar_icon(cc);
            configure_fonts(&cc.egui_ctx);
            let render_state = std::sync::Arc::new(
                cc.wgpu_render_state
                    .clone()
                    .expect("wgpu render state required"),
            );
            Ok(Box::new(ViewerApp::new(
                initial_path,
                render_state,
                ipc_rx,
                saved_settings,
            )))
        }),
    )
}
