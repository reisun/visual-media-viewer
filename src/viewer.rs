use eframe::egui;
use std::path::{Path, PathBuf};

use crate::cache::ImageCache;
use crate::file_list::FileList;

/// Decoded image data ready to be uploaded to GPU.
pub struct DecodedImage {
    pub pixels: egui::ColorImage,
}

impl DecodedImage {
    /// Decode an image file into an egui ColorImage.
    pub fn load(path: &Path) -> Result<Self, String> {
        let img = image::open(path).map_err(|e| format!("Failed to open {}: {}", path.display(), e))?;
        let rgba = img.to_rgba8();
        let size = [rgba.width() as usize, rgba.height() as usize];
        let pixels = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
        Ok(Self { pixels })
    }
}

/// Zoom and pan state for the viewer.
struct ViewTransform {
    zoom: f32,
    /// Offset in image pixels from center.
    pan: egui::Vec2,
    /// Whether the user is currently dragging with right-click.
    right_drag_active: bool,
    /// The screen position where right-click drag started.
    right_drag_start_y: f32,
    /// The zoom level when right-click drag started.
    right_drag_start_zoom: f32,
}

impl Default for ViewTransform {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            right_drag_active: false,
            right_drag_start_y: 0.0,
            right_drag_start_zoom: 1.0,
        }
    }
}

impl ViewTransform {
    fn reset(&mut self) {
        self.zoom = 1.0;
        self.pan = egui::Vec2::ZERO;
        self.right_drag_active = false;
    }
}

/// Main application state.
pub struct ViewerApp {
    /// Currently displayed texture handle.
    texture: Option<egui::TextureHandle>,
    /// Size of the currently loaded image in pixels.
    image_size: Option<[usize; 2]>,
    /// File list for navigation.
    file_list: Option<FileList>,
    /// Image cache for preloading.
    cache: ImageCache,
    /// Zoom and pan transform.
    transform: ViewTransform,
    /// Error message to display if image loading fails.
    error_message: Option<String>,
}

impl ViewerApp {
    pub fn new(initial_path: Option<PathBuf>) -> Self {
        let mut app = Self {
            texture: None,
            image_size: None,
            file_list: None,
            cache: ImageCache::new(10),
            transform: ViewTransform::default(),
            error_message: None,
        };

        if let Some(path) = initial_path {
            app.open_file(&path);
        }

        app
    }

    /// Open a file and build the file list from its directory.
    fn open_file(&mut self, path: &Path) {
        let canonical = match std::fs::canonicalize(path) {
            Ok(p) => p,
            Err(e) => {
                self.error_message = Some(format!("Cannot resolve path {}: {}", path.display(), e));
                return;
            }
        };

        // Build file list from the directory containing this file.
        if let Some(parent) = canonical.parent() {
            let mut file_list = FileList::from_directory(parent);
            file_list.set_current(&canonical);
            self.file_list = Some(file_list);
        }

        self.load_current_image();
    }

    /// Load the current image from the file list (using cache if available).
    fn load_current_image(&mut self) {
        // Clear previous texture so it gets recreated.
        self.texture = None;
        self.image_size = None;
        self.error_message = None;
        self.transform.reset();

        let path = match &self.file_list {
            Some(fl) => match fl.current_path() {
                Some(p) => p.to_path_buf(),
                None => return,
            },
            None => return,
        };

        // Try cache first, then load directly.
        match self.cache.get(&path) {
            Some(pixels) => {
                self.image_size = Some(pixels.size);
                // Texture will be created in update() when ctx is available.
                // Store pixels temporarily via the cache (already there).
            }
            None => {
                match DecodedImage::load(&path) {
                    Ok(decoded) => {
                        self.image_size = Some(decoded.pixels.size);
                        self.cache.insert(path, decoded.pixels);
                    }
                    Err(e) => {
                        log::error!("{}", e);
                        self.error_message = Some(e);
                    }
                }
            }
        }

        // Trigger preloading of nearby images.
        self.start_preload();
    }

    /// Start preloading images around the current index.
    fn start_preload(&mut self) {
        if let Some(fl) = &self.file_list {
            let paths = fl.nearby_paths(3);
            self.cache.preload(paths);
        }
    }

    /// Navigate to the next image.
    fn next_image(&mut self) {
        if let Some(fl) = &mut self.file_list {
            if fl.next() {
                self.load_current_image();
            }
        }
    }

    /// Navigate to the previous image.
    fn prev_image(&mut self) {
        if let Some(fl) = &mut self.file_list {
            if fl.prev() {
                self.load_current_image();
            }
        }
    }

    /// Get the current filename for the title bar.
    fn current_filename(&self) -> Option<String> {
        self.file_list
            .as_ref()
            .and_then(|fl| fl.current_path())
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
    }

    /// Compute the fit-to-window scale for the current image given available size.
    fn fit_scale(&self, available: egui::Vec2) -> f32 {
        if let Some(size) = self.image_size {
            let img_w = size[0] as f32;
            let img_h = size[1] as f32;
            if img_w > 0.0 && img_h > 0.0 {
                let scale_x = available.x / img_w;
                let scale_y = available.y / img_h;
                scale_x.min(scale_y)
            } else {
                1.0
            }
        } else {
            1.0
        }
    }

    /// Handle zoom interactions (right-click drag, scroll wheel, double-click).
    fn handle_zoom_input(&mut self, response: &egui::Response, available_size: egui::Vec2) {
        let pointer_pos = response.hover_pos().unwrap_or(response.rect.center());
        let rect_center = response.rect.center();

        // Right-click drag: zoom by vertical movement.
        if response.secondary_clicked() {
            self.transform.right_drag_active = true;
            self.transform.right_drag_start_y = pointer_pos.y;
            self.transform.right_drag_start_zoom = self.transform.zoom;
        }

        if self.transform.right_drag_active {
            let ctx = response.ctx.clone();
            if ctx.input(|i| i.pointer.secondary_down()) {
                let dy = self.transform.right_drag_start_y - pointer_pos.y;
                let zoom_factor = (dy / 200.0).exp();
                let new_zoom = (self.transform.right_drag_start_zoom * zoom_factor).clamp(0.1, 50.0);

                // Zoom towards pointer position.
                let pointer_offset = pointer_pos - rect_center;
                let old_zoom = self.transform.zoom;
                self.transform.zoom = new_zoom;
                let zoom_ratio = new_zoom / old_zoom;
                self.transform.pan = self.transform.pan * zoom_ratio
                    + pointer_offset * (1.0 - zoom_ratio);
            } else {
                self.transform.right_drag_active = false;
            }
        }

        // Mouse wheel zoom.
        let scroll_delta = response.ctx.input(|i| i.raw_scroll_delta.y);
        if scroll_delta.abs() > 0.0 && response.hovered() {
            let zoom_factor = (scroll_delta / 300.0).exp();
            let new_zoom = (self.transform.zoom * zoom_factor).clamp(0.1, 50.0);

            let pointer_offset = pointer_pos - rect_center;
            let old_zoom = self.transform.zoom;
            self.transform.zoom = new_zoom;
            let zoom_ratio = new_zoom / old_zoom;
            self.transform.pan = self.transform.pan * zoom_ratio
                + pointer_offset * (1.0 - zoom_ratio);
        }

        // Double-click to reset fit.
        if response.double_clicked() {
            self.transform.reset();
        }

        // Ignore available_size; it's used indirectly via fit_scale.
        let _ = available_size;
    }
}

impl eframe::App for ViewerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Poll cache for completed preloads.
        self.cache.poll();

        // Handle keyboard navigation.
        ctx.input(|i| {
            if i.key_pressed(egui::Key::ArrowRight) {
                return Some(true);
            }
            if i.key_pressed(egui::Key::ArrowLeft) {
                return Some(false);
            }
            None
        })
        .map(|forward| {
            if forward {
                self.next_image();
            } else {
                self.prev_image();
            }
        });

        // Update window title.
        if let Some(filename) = self.current_filename() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(
                format!("{} - Visual Media Viewer", filename),
            ));
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(egui::Color32::from_rgb(30, 30, 30)))
            .show(ctx, |ui| {
                let available = ui.available_size();

                if let Some(error) = &self.error_message {
                    ui.centered_and_justified(|ui| {
                        ui.colored_label(egui::Color32::RED, error.as_str());
                    });
                    return;
                }

                // Get the current image path to look up in cache.
                let current_path = self
                    .file_list
                    .as_ref()
                    .and_then(|fl| fl.current_path())
                    .map(|p| p.to_path_buf());

                if let Some(path) = current_path {
                    // Ensure texture is loaded.
                    if self.texture.is_none() {
                        if let Some(pixels) = self.cache.get(&path) {
                            self.image_size = Some(pixels.size);
                            let texture = ctx.load_texture(
                                "current_image",
                                pixels.clone(),
                                egui::TextureOptions::LINEAR,
                            );
                            self.texture = Some(texture);
                        }
                    }

                    if let (Some(texture), Some(img_size)) = (&self.texture, &self.image_size) {
                        let fit = self.fit_scale(available);
                        let display_w = img_size[0] as f32 * fit * self.transform.zoom;
                        let display_h = img_size[1] as f32 * fit * self.transform.zoom;

                        let center = ui.available_rect_before_wrap().center();
                        let offset = self.transform.pan;

                        let image_rect = egui::Rect::from_center_size(
                            center + offset,
                            egui::vec2(display_w, display_h),
                        );

                        // Allocate the full area for interaction.
                        let (response, painter) =
                            ui.allocate_painter(available, egui::Sense::click_and_drag());

                        // Draw the image.
                        painter.image(
                            texture.id(),
                            image_rect,
                            egui::Rect::from_min_max(
                                egui::pos2(0.0, 0.0),
                                egui::pos2(1.0, 1.0),
                            ),
                            egui::Color32::WHITE,
                        );

                        // Handle zoom interactions.
                        self.handle_zoom_input(&response, available);
                    }
                } else if self.file_list.is_none() {
                    ui.centered_and_justified(|ui| {
                        ui.label("Visual Media Viewer - Drop an image or pass a file path as argument");
                    });
                }
            });
    }
}
