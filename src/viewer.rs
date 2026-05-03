use eframe::egui;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use crate::cache::ImageCache;
use crate::file_list::FileList;

/// Decoded image data ready to be uploaded to GPU.
pub struct DecodedImage {
    pub pixels: egui::ColorImage,
}

impl DecodedImage {
    pub fn load(path: &Path) -> Result<Self, String> {
        let file = File::open(path)
            .map_err(|e| format!("Failed to open {}: {}", path.display(), e))?;
        let reader = image::ImageReader::new(BufReader::new(file))
            .with_guessed_format()
            .map_err(|e| format!("Failed to detect format {}: {}", path.display(), e))?;
        let img = reader
            .decode()
            .map_err(|e| format!("Failed to decode {}: {}", path.display(), e))?;
        let rgba = img.to_rgba8();
        let size = [rgba.width() as usize, rgba.height() as usize];
        let pixels = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
        Ok(Self { pixels })
    }
}

/// Zoom, pan, and rotation state for the viewer.
struct ViewTransform {
    zoom: f32,
    /// Offset in image pixels from center.
    pan: egui::Vec2,
    /// Display rotation in degrees (0, 90, 180, 270).
    rotation: u16,
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
            rotation: 0,
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
        self.rotation = 0;
        self.right_drag_active = false;
    }

    fn rotate_cw(&mut self) {
        self.rotation = (self.rotation + 90) % 360;
    }

    fn rotate_ccw(&mut self) {
        self.rotation = (self.rotation + 270) % 360;
    }

    /// Get UV coordinates for the four corners based on rotation.
    /// Returns (top-left, top-right, bottom-right, bottom-left) UV positions.
    fn rotated_uvs(&self) -> [egui::Pos2; 4] {
        match self.rotation {
            0 => [
                egui::pos2(0.0, 0.0),
                egui::pos2(1.0, 0.0),
                egui::pos2(1.0, 1.0),
                egui::pos2(0.0, 1.0),
            ],
            90 => [
                egui::pos2(0.0, 1.0),
                egui::pos2(0.0, 0.0),
                egui::pos2(1.0, 0.0),
                egui::pos2(1.0, 1.0),
            ],
            180 => [
                egui::pos2(1.0, 1.0),
                egui::pos2(0.0, 1.0),
                egui::pos2(0.0, 0.0),
                egui::pos2(1.0, 0.0),
            ],
            270 => [
                egui::pos2(1.0, 0.0),
                egui::pos2(1.0, 1.0),
                egui::pos2(0.0, 1.0),
                egui::pos2(0.0, 0.0),
            ],
            _ => [
                egui::pos2(0.0, 0.0),
                egui::pos2(1.0, 0.0),
                egui::pos2(1.0, 1.0),
                egui::pos2(0.0, 1.0),
            ],
        }
    }

    /// Returns true if the rotation swaps width and height.
    fn is_rotated_90_or_270(&self) -> bool {
        self.rotation == 90 || self.rotation == 270
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
    /// Slideshow active state.
    slideshow_active: bool,
    /// Slideshow interval in seconds.
    slideshow_interval: f64,
    /// Timestamp of the last slideshow advance.
    slideshow_last_advance: f64,
    /// Whether to show the property overlay.
    show_properties: bool,
    /// Right-click context menu state.
    right_press_pos: Option<egui::Pos2>,
    show_context_menu: bool,
    context_menu_pos: egui::Pos2,
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
            slideshow_active: false,
            slideshow_interval: 3.0,
            slideshow_last_advance: 0.0,
            show_properties: false,
            right_press_pos: None,
            show_context_menu: false,
            context_menu_pos: egui::Pos2::ZERO,
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
    /// Takes rotation into account (90/270 swaps dimensions).
    fn fit_scale(&self, available: egui::Vec2) -> f32 {
        if let Some(size) = self.image_size {
            let (img_w, img_h) = if self.transform.is_rotated_90_or_270() {
                (size[1] as f32, size[0] as f32)
            } else {
                (size[0] as f32, size[1] as f32)
            };
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

    /// Handle zoom interactions (right-click drag, scroll wheel, double-click)
    /// and detect right-click short press for context menu.
    fn handle_zoom_input(&mut self, response: &egui::Response, available_size: egui::Vec2) {
        let pointer_pos = response.hover_pos().unwrap_or(response.rect.center());
        let rect_center = response.rect.center();

        // Track right-click press position for context menu detection.
        let secondary_pressed = response.ctx.input(|i| i.pointer.secondary_pressed());
        if secondary_pressed {
            self.right_press_pos = Some(pointer_pos);
        }

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
                // Right button released: check if it was a short press (no drag).
                if let Some(press_pos) = self.right_press_pos.take() {
                    let dist = (pointer_pos - press_pos).length();
                    if dist < 3.0 {
                        self.show_context_menu = true;
                        self.context_menu_pos = pointer_pos;
                    }
                }
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

        // Handle file drop.
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw.dropped_files.iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if let Some(path) = dropped.into_iter().next() {
            self.open_file(&path);
        }

        // Handle keyboard input.
        {
            let nav = ctx.input(|i| {
                if i.key_pressed(egui::Key::ArrowRight) {
                    return Some(true);
                }
                if i.key_pressed(egui::Key::ArrowLeft) {
                    return Some(false);
                }
                None
            });
            if let Some(forward) = nav {
                if forward {
                    self.next_image();
                } else {
                    self.prev_image();
                }
            }

            // Rotation: R = clockwise, Shift+R = counter-clockwise.
            let rotate = ctx.input(|i| {
                if i.key_pressed(egui::Key::R) {
                    if i.modifiers.shift {
                        return Some(false); // CCW
                    }
                    return Some(true); // CW
                }
                None
            });
            if let Some(cw) = rotate {
                if cw {
                    self.transform.rotate_cw();
                } else {
                    self.transform.rotate_ccw();
                }
            }

            // Slideshow: S = toggle, +/= = increase interval, - = decrease interval.
            let slideshow_toggle = ctx.input(|i| i.key_pressed(egui::Key::S));
            if slideshow_toggle {
                self.slideshow_active = !self.slideshow_active;
                if self.slideshow_active {
                    self.slideshow_last_advance = ctx.input(|i| i.time);
                }
            }

            let interval_change = ctx.input(|i| {
                if i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals) {
                    return Some(1.0_f64);
                }
                if i.key_pressed(egui::Key::Minus) {
                    return Some(-1.0_f64);
                }
                None
            });
            if let Some(delta) = interval_change {
                self.slideshow_interval = (self.slideshow_interval + delta).clamp(1.0, 30.0);
            }

            // Property overlay: I = toggle.
            let toggle_props = ctx.input(|i| i.key_pressed(egui::Key::I));
            if toggle_props {
                self.show_properties = !self.show_properties;
            }
        }

        // Slideshow timer.
        if self.slideshow_active {
            let now = ctx.input(|i| i.time);
            if now - self.slideshow_last_advance >= self.slideshow_interval {
                self.slideshow_last_advance = now;
                self.next_image();
            }
            let remaining = self.slideshow_interval - (ctx.input(|i| i.time) - self.slideshow_last_advance);
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(remaining.max(0.01)));
        }

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

                        // For 90/270, swap displayed dimensions.
                        let (display_w, display_h) = if self.transform.is_rotated_90_or_270() {
                            (
                                img_size[1] as f32 * fit * self.transform.zoom,
                                img_size[0] as f32 * fit * self.transform.zoom,
                            )
                        } else {
                            (
                                img_size[0] as f32 * fit * self.transform.zoom,
                                img_size[1] as f32 * fit * self.transform.zoom,
                            )
                        };

                        let center = ui.available_rect_before_wrap().center();
                        let offset = self.transform.pan;

                        let image_rect = egui::Rect::from_center_size(
                            center + offset,
                            egui::vec2(display_w, display_h),
                        );

                        // Allocate the full area for interaction.
                        let (response, painter) =
                            ui.allocate_painter(available, egui::Sense::click_and_drag());

                        // Draw the image using a mesh with rotated UV coordinates.
                        let uvs = self.transform.rotated_uvs();
                        let mut mesh = egui::Mesh::with_texture(texture.id());
                        let tint = egui::Color32::WHITE;
                        // Vertices: top-left, top-right, bottom-right, bottom-left
                        mesh.vertices.push(egui::epaint::Vertex {
                            pos: image_rect.left_top(),
                            uv: uvs[0],
                            color: tint,
                        });
                        mesh.vertices.push(egui::epaint::Vertex {
                            pos: image_rect.right_top(),
                            uv: uvs[1],
                            color: tint,
                        });
                        mesh.vertices.push(egui::epaint::Vertex {
                            pos: image_rect.right_bottom(),
                            uv: uvs[2],
                            color: tint,
                        });
                        mesh.vertices.push(egui::epaint::Vertex {
                            pos: image_rect.left_bottom(),
                            uv: uvs[3],
                            color: tint,
                        });
                        // Two triangles: 0-1-2 and 0-2-3
                        mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
                        painter.add(egui::Shape::mesh(mesh));

                        // Handle zoom interactions.
                        self.handle_zoom_input(&response, available);
                    }
                } else if self.file_list.is_none() {
                    ui.centered_and_justified(|ui| {
                        ui.label("Visual Media Viewer - Drop an image or pass a file path as argument");
                    });
                }
            });

        // Context menu (rendered as a floating area).
        if self.show_context_menu {
            let menu_pos = self.context_menu_pos;
            let area_resp = egui::Area::new(egui::Id::new("context_menu"))
                .fixed_pos(menu_pos)
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_min_width(180.0);

                        if ui.button("Rotate Clockwise (R)").clicked() {
                            self.transform.rotate_cw();
                            self.show_context_menu = false;
                        }
                        if ui.button("Rotate Counter-clockwise (Shift+R)").clicked() {
                            self.transform.rotate_ccw();
                            self.show_context_menu = false;
                        }
                        ui.separator();
                        let slideshow_label = if self.slideshow_active {
                            "Stop Slideshow (S)"
                        } else {
                            "Start Slideshow (S)"
                        };
                        if ui.button(slideshow_label).clicked() {
                            self.slideshow_active = !self.slideshow_active;
                            if self.slideshow_active {
                                self.slideshow_last_advance = ctx.input(|i| i.time);
                            }
                            self.show_context_menu = false;
                        }
                        ui.separator();
                        if ui.button("Reset View").clicked() {
                            self.transform.reset();
                            self.show_context_menu = false;
                        }
                    });
                });

            // Close menu if clicking outside it.
            let clicked_elsewhere = ctx.input(|i| i.pointer.any_pressed())
                && !area_resp.response.hovered();
            if clicked_elsewhere {
                self.show_context_menu = false;
            }
        }
    }
}
