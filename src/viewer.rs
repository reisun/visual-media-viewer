use eframe::egui;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use crate::cache::ImageCache;
use crate::file_list::{FileList, GroupBy, SortKey, SortOrder};
use crate::settings::{FitModeSetting, GroupBySetting, Settings, SortKeySetting, SortOrderSetting};

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

/// Fit display mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FitMode {
    FitToWindow,
    OriginalSize,
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
    fn reset_zoom_pan(&mut self) {
        self.zoom = 1.0;
        self.pan = egui::Vec2::ZERO;
        self.right_drag_active = false;
    }

    fn reset_all(&mut self) {
        self.reset_zoom_pan();
        self.rotation = 0;
    }

    fn rotate_cw(&mut self) {
        self.rotation = (self.rotation + 90) % 360;
    }

    fn rotate_ccw(&mut self) {
        self.rotation = (self.rotation + 270) % 360;
    }

    /// Get UV coordinates for the four corners based on rotation.
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
    /// Fit display mode.
    fit_mode: FitMode,
    /// Whether the title bar right-click menu is open.
    show_titlebar_menu: bool,
    /// Position where the title bar menu should appear.
    titlebar_menu_pos: egui::Pos2,
    /// Whether the window is currently maximized (tracked for toggle).
    is_maximized: bool,
    /// Persisted settings.
    settings: Settings,
}

impl ViewerApp {
    pub fn new(initial_path: Option<PathBuf>) -> Self {
        let settings = Settings::load();
        let mut transform = ViewTransform::default();
        transform.rotation = settings.rotation;
        let fit_mode = match settings.fit_mode {
            FitModeSetting::FitToWindow => FitMode::FitToWindow,
            FitModeSetting::OriginalSize => FitMode::OriginalSize,
        };

        let mut app = Self {
            texture: None,
            image_size: None,
            file_list: None,
            cache: ImageCache::new(10),
            transform,
            error_message: None,
            slideshow_active: false,
            slideshow_interval: 3.0,
            slideshow_last_advance: 0.0,
            fit_mode,
            show_titlebar_menu: false,
            titlebar_menu_pos: egui::Pos2::ZERO,
            is_maximized: false,
            settings,
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

        if let Some(parent) = canonical.parent() {
            let mut file_list = FileList::from_directory(parent);
            file_list.set_group_by(self.saved_group_by());
            file_list.re_sort(self.saved_sort_key(), self.saved_sort_order());
            file_list.set_current(&canonical);
            self.file_list = Some(file_list);
        }

        self.load_current_image();
    }

    fn open_directory(&mut self, dir: &Path) {
        let mut file_list = FileList::from_directory(dir);
        file_list.set_group_by(self.saved_group_by());
        file_list.re_sort(self.saved_sort_key(), self.saved_sort_order());
        if file_list.file_count() > 0 {
            self.cache.clear();
            self.file_list = Some(file_list);
            self.load_current_image();
        }
    }

    fn saved_sort_key(&self) -> SortKey {
        match self.settings.sort_key {
            SortKeySetting::Name => SortKey::Name,
            SortKeySetting::ModifiedDate => SortKey::ModifiedDate,
        }
    }

    fn saved_sort_order(&self) -> SortOrder {
        match self.settings.sort_order {
            SortOrderSetting::Ascending => SortOrder::Ascending,
            SortOrderSetting::Descending => SortOrder::Descending,
        }
    }

    fn save_settings(&mut self) {
        self.settings.rotation = self.transform.rotation;
        self.settings.fit_mode = match self.fit_mode {
            FitMode::FitToWindow => FitModeSetting::FitToWindow,
            FitMode::OriginalSize => FitModeSetting::OriginalSize,
        };
        if let Some(fl) = &self.file_list {
            self.settings.sort_key = match fl.sort_key {
                SortKey::Name => SortKeySetting::Name,
                SortKey::ModifiedDate => SortKeySetting::ModifiedDate,
            };
            self.settings.sort_order = match fl.sort_order {
                SortOrder::Ascending => SortOrderSetting::Ascending,
                SortOrder::Descending => SortOrderSetting::Descending,
            };
            self.settings.group_by = match fl.group_by {
                GroupBy::Off => GroupBySetting::Off,
                GroupBy::ModifiedDate => GroupBySetting::ModifiedDate,
            };
        }
        self.settings.save();
    }

    fn saved_group_by(&self) -> GroupBy {
        match self.settings.group_by {
            GroupBySetting::Off => GroupBy::Off,
            GroupBySetting::ModifiedDate => GroupBy::ModifiedDate,
        }
    }

    /// Load the current image from the file list (using cache if available).
    fn load_current_image(&mut self) {
        self.texture = None;
        self.image_size = None;
        self.error_message = None;
        self.transform.reset_zoom_pan();

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

    /// Build the title text: 親フォルダ/ファイル名 (現在 / 画像件数) [<自動: X.Xs>]
    fn title_text(&self) -> String {
        if let Some(fl) = &self.file_list {
            if let Some(path) = fl.current_path() {
                let filename = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let parent_name = fl
                    .directory()
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let position = format!(
                    "{} / {}",
                    fl.current_index() + 1,
                    fl.file_count()
                );
                let mut title = format!("{}/{} ({})", parent_name, filename, position);
                if self.slideshow_active {
                    title.push_str(&format!(" <自動: {:.1}s>", self.slideshow_interval));
                }
                return title;
            }
        }
        "Visual Media Viewer".to_string()
    }

    /// Compute the fit-to-window scale for the current image given available size.
    /// Takes rotation into account (90/270 swaps dimensions).
    /// Returns 1.0 in OriginalSize mode.
    fn fit_scale(&self, available: egui::Vec2) -> f32 {
        match self.fit_mode {
            FitMode::OriginalSize => 1.0,
            FitMode::FitToWindow => {
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
        }
    }

    /// Handle zoom interactions (right-click drag, scroll wheel, double-click).
    fn handle_zoom_input(&mut self, response: &egui::Response, _available_size: egui::Vec2) {
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

        if response.double_clicked() {
            self.transform.reset_all();
        }
    }

    fn navigate_prev_folder(&mut self) {
        let dir = match &self.file_list {
            Some(fl) => fl.prev_image_dir(),
            None => None,
        };
        if let Some(d) = dir {
            self.open_directory(&d);
        }
    }

    fn navigate_next_folder(&mut self) {
        let dir = match &self.file_list {
            Some(fl) => fl.next_image_dir(),
            None => None,
        };
        if let Some(d) = dir {
            self.open_directory(&d);
        }
    }

    /// Draw the custom title bar and return whether the menu should be shown.
    fn draw_title_bar(&mut self, ctx: &egui::Context) {
        let title_text = self.title_text();
        let title_bar_height = 28.0;

        egui::TopBottomPanel::top("title_bar")
            .exact_height(title_bar_height)
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(45, 45, 45))
                    .inner_margin(egui::Margin::symmetric(8, 0)),
            )
            .show(ctx, |ui| {
                let bar_response = ui.interact(
                    ui.available_rect_before_wrap(),
                    egui::Id::new("title_bar_bg"),
                    egui::Sense::click_and_drag(),
                );

                // Drag to move window on left-click drag anywhere on the title bar.
                if bar_response.dragged_by(egui::PointerButton::Primary) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }

                // Double-click to toggle maximize.
                if bar_response.double_clicked() {
                    self.is_maximized = !self.is_maximized;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(self.is_maximized));
                }

                // Right-click on title bar to show menu.
                if bar_response.secondary_clicked() {
                    self.show_titlebar_menu = !self.show_titlebar_menu;
                    if let Some(pos) = bar_response.hover_pos() {
                        self.titlebar_menu_pos = pos;
                    }
                }

                ui.horizontal_centered(|ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&title_text)
                                .color(egui::Color32::from_gray(220))
                                .size(13.0),
                        ),
                    );

                    // Right-align the window control buttons.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let btn_size = egui::vec2(36.0, 24.0);

                        // Close button.
                        let close_btn = ui.add_sized(
                            btn_size,
                            egui::Button::new(
                                egui::RichText::new("X").color(egui::Color32::from_gray(200)).size(13.0),
                            ).frame(false),
                        );
                        if close_btn.clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        if close_btn.hovered() {
                            ui.painter().rect_filled(
                                close_btn.rect,
                                0.0,
                                egui::Color32::from_rgba_unmultiplied(232, 17, 35, 180),
                            );
                        }

                        // Maximize/Restore button.
                        let max_label = if self.is_maximized { "[ ]" } else { "[ ]" };
                        let max_btn = ui.add_sized(
                            btn_size,
                            egui::Button::new(
                                egui::RichText::new(max_label).color(egui::Color32::from_gray(200)).size(11.0),
                            ).frame(false),
                        );
                        if max_btn.clicked() {
                            self.is_maximized = !self.is_maximized;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(self.is_maximized));
                        }

                        // Minimize button.
                        let min_btn = ui.add_sized(
                            btn_size,
                            egui::Button::new(
                                egui::RichText::new("_").color(egui::Color32::from_gray(200)).size(13.0),
                            ).frame(false),
                        );
                        if min_btn.clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                        }
                    });
                });
            });
    }

    fn draw_titlebar_menu(&mut self, ctx: &egui::Context) {
        let mut open = self.show_titlebar_menu;
        if !open {
            return;
        }

        let menu_pos = self.titlebar_menu_pos;
        egui::Window::new("メニュー")
            .open(&mut open)
            .fixed_pos(menu_pos)
            .collapsible(false)
            .resizable(false)
            .default_width(220.0)
            .show(ctx, |ui| {
                ui.label(egui::RichText::new("リスト").strong().size(13.0));
                ui.separator();

                if ui.button("前のファイル").clicked() {
                    if let Some(fl) = &mut self.file_list {
                        fl.prev();
                    }
                    self.load_current_image();
                }

                if ui.button("次のファイル").clicked() {
                    if let Some(fl) = &mut self.file_list {
                        fl.next();
                    }
                    self.load_current_image();
                }

                ui.separator();

                ui.label(egui::RichText::new("並び順（対象）").size(12.0));
                {
                    let mut key = self.file_list.as_ref().map(|fl| fl.sort_key).unwrap_or(SortKey::Name);
                    let prev = key;
                    ui.radio_value(&mut key, SortKey::Name, "名前");
                    ui.radio_value(&mut key, SortKey::ModifiedDate, "更新日時");
                    if key != prev {
                        if let Some(fl) = &mut self.file_list {
                            let order = fl.sort_order;
                            fl.re_sort(key, order);
                        }
                        self.save_settings();
                    }
                }

                ui.label(egui::RichText::new("並び順（順序）").size(12.0));
                {
                    let mut order = self.file_list.as_ref().map(|fl| fl.sort_order).unwrap_or(SortOrder::Ascending);
                    let prev = order;
                    ui.radio_value(&mut order, SortOrder::Ascending, "昇順");
                    ui.radio_value(&mut order, SortOrder::Descending, "降順");
                    if order != prev {
                        if let Some(fl) = &mut self.file_list {
                            let key = fl.sort_key;
                            fl.re_sort(key, order);
                        }
                        self.save_settings();
                    }
                }

                ui.separator();

                ui.label(egui::RichText::new("グループ化").size(12.0));
                {
                    let mut group = self.file_list.as_ref().map(|fl| fl.group_by).unwrap_or(GroupBy::Off);
                    let prev = group;
                    ui.radio_value(&mut group, GroupBy::Off, "オフ");
                    ui.radio_value(&mut group, GroupBy::ModifiedDate, "更新日時");
                    if group != prev {
                        if let Some(fl) = &mut self.file_list {
                            fl.set_group_by(group);
                        }
                        self.save_settings();
                    }
                }

                ui.separator();

                ui.label(egui::RichText::new("スライドショー").size(12.0));
                {
                    let label = if self.slideshow_active { "停止" } else { "開始" };
                    if ui.button(label).clicked() {
                        self.slideshow_active = !self.slideshow_active;
                        if self.slideshow_active {
                            self.slideshow_last_advance = ui.ctx().input(|i| i.time);
                        }
                    }
                }
                ui.horizontal(|ui| {
                    ui.label("時間:");
                    let mut interval = self.slideshow_interval;
                    let drag = egui::DragValue::new(&mut interval)
                        .range(1.0..=30.0)
                        .speed(0.1)
                        .suffix("s")
                        .fixed_decimals(1);
                    if ui.add(drag).changed() {
                        self.slideshow_interval = interval;
                    }
                });

                ui.add_space(8.0);

                ui.label(egui::RichText::new("表示").strong().size(13.0));
                ui.separator();

                ui.label(egui::RichText::new("回転オプション").size(12.0));
                {
                    let mut rot = self.transform.rotation;
                    let prev = rot;
                    ui.radio_value(&mut rot, 0, "オフ");
                    ui.radio_value(&mut rot, 270, "左回転");
                    ui.radio_value(&mut rot, 90, "右回転");
                    ui.radio_value(&mut rot, 180, "180度回転");
                    if rot != prev {
                        self.transform.rotation = rot;
                        self.save_settings();
                    }
                }

                ui.separator();

                if ui.button("ズームイン").clicked() {
                    self.transform.zoom = (self.transform.zoom * 1.25).clamp(0.1, 50.0);
                }
                if ui.button("ズームアウト").clicked() {
                    self.transform.zoom = (self.transform.zoom / 1.25).clamp(0.1, 50.0);
                }
                if ui.button("ズームリセット").clicked() {
                    self.transform.zoom = 1.0;
                    self.transform.pan = egui::Vec2::ZERO;
                }

                ui.separator();

                ui.label(egui::RichText::new("フィット表示").size(12.0));
                {
                    let mut mode = self.fit_mode;
                    let prev = mode;
                    ui.radio_value(&mut mode, FitMode::OriginalSize, "オリジナルサイズ");
                    ui.radio_value(&mut mode, FitMode::FitToWindow, "ウインドウに合わせる");
                    if mode != prev {
                        self.fit_mode = mode;
                        self.transform.zoom = 1.0;
                        self.transform.pan = egui::Vec2::ZERO;
                        self.save_settings();
                    }
                }
            });

        self.show_titlebar_menu = open;
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
                    return Some("right");
                }
                if i.key_pressed(egui::Key::ArrowLeft) {
                    return Some("left");
                }
                if i.key_pressed(egui::Key::ArrowUp) || i.key_pressed(egui::Key::PageUp) {
                    return Some("prev_folder");
                }
                if i.key_pressed(egui::Key::ArrowDown) || i.key_pressed(egui::Key::PageDown) {
                    return Some("next_folder");
                }
                None
            });
            match nav {
                Some("right") => self.next_image(),
                Some("left") => self.prev_image(),
                Some("prev_folder") => self.navigate_prev_folder(),
                Some("next_folder") => self.navigate_next_folder(),
                _ => {}
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
                self.save_settings();
            }

            // Slideshow: S = toggle, +/= = increase interval, - = decrease interval.
            let slideshow_toggle = ctx.input(|i| i.key_pressed(egui::Key::S));
            if slideshow_toggle {
                self.slideshow_active = !self.slideshow_active;
                if self.slideshow_active {
                    self.slideshow_last_advance = ctx.input(|i| i.time);
                }
            }

            // 0.1s increments for slideshow interval.
            let interval_change = ctx.input(|i| {
                if i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals) {
                    return Some(0.1_f64);
                }
                if i.key_pressed(egui::Key::Minus) {
                    return Some(-0.1_f64);
                }
                None
            });
            if let Some(delta) = interval_change {
                self.slideshow_interval = (self.slideshow_interval + delta).clamp(1.0, 30.0);
                // Round to 1 decimal to avoid floating point drift.
                self.slideshow_interval = (self.slideshow_interval * 10.0).round() / 10.0;
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

        // Draw custom title bar.
        self.draw_title_bar(ctx);

        // Draw the title bar right-click menu (if open).
        self.draw_titlebar_menu(ctx);

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
                                egui::TextureOptions {
                                    magnification: egui::TextureFilter::Linear,
                                    minification: egui::TextureFilter::Linear,
                                    mipmap_mode: Some(egui::TextureFilter::Linear),
                                    ..Default::default()
                                },
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
    }
}
