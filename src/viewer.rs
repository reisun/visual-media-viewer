use eframe::egui;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::cache::ImageCache;
use crate::file_list::{FileList, GroupBy, SortKey, SortOrder};
use crate::settings::{FitModeSetting, GroupBySetting, Settings, SortKeySetting, SortOrderSetting};

/// Decoded image data ready to be uploaded to GPU.
pub struct DecodedImage {
    pub pixels: egui::ColorImage,
}

impl DecodedImage {
    pub fn load(path: &Path) -> Result<Self, String> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();

        if ext == "heic" || ext == "heif" {
            return Self::load_wic(path);
        }

        match Self::load_image_crate(path) {
            Ok(img) => Ok(img),
            Err(_) => Self::load_wic(path),
        }
    }

    fn load_image_crate(path: &Path) -> Result<Self, String> {
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

    fn load_wic(path: &Path) -> Result<Self, String> {
        let pixels = crate::wic_decoder::decode_with_wic(path)?;
        Ok(Self { pixels })
    }
}

fn compute_mip_levels(width: u32, height: u32) -> u32 {
    (width.max(height) as f32).log2().floor() as u32 + 1
}

fn box_filter_mip(data: &[u8], width: u32, height: u32) -> (Vec<u8>, u32, u32) {
    let new_w = (width / 2).max(1);
    let new_h = (height / 2).max(1);
    let w = width as usize;
    let mut out = vec![0u8; (new_w * new_h * 4) as usize];
    for y in 0..new_h as usize {
        for x in 0..new_w as usize {
            let sx = x * 2;
            let sy = y * 2;
            let mut r = 0u32;
            let mut g = 0u32;
            let mut b = 0u32;
            let mut a = 0u32;
            for dy in 0..2usize {
                for dx in 0..2usize {
                    let px = (sx + dx).min(width as usize - 1);
                    let py = (sy + dy).min(height as usize - 1);
                    let idx = (py * w + px) * 4;
                    r += data[idx] as u32;
                    g += data[idx + 1] as u32;
                    b += data[idx + 2] as u32;
                    a += data[idx + 3] as u32;
                }
            }
            let oi = (y * new_w as usize + x) * 4;
            out[oi] = (r / 4) as u8;
            out[oi + 1] = (g / 4) as u8;
            out[oi + 2] = (b / 4) as u8;
            out[oi + 3] = (a / 4) as u8;
        }
    }
    (out, new_w, new_h)
}

fn color_image_to_rgba(img: &egui::ColorImage) -> Vec<u8> {
    let mut buf = Vec::with_capacity(img.pixels.len() * 4);
    for pixel in &img.pixels {
        buf.push(pixel.r());
        buf.push(pixel.g());
        buf.push(pixel.b());
        buf.push(pixel.a());
    }
    buf
}

fn nearest_half_from_pixels(src: &[egui::Color32], width: u32, height: u32) -> (Vec<u8>, u32, u32) {
    let new_w = width / 2;
    let new_h = height / 2;
    let stride = width as usize;
    let mut out = Vec::with_capacity((new_w as usize) * (new_h as usize) * 4);
    for y in 0..new_h as usize {
        let row = y * 2 * stride;
        for x in 0..new_w as usize {
            let p = src[row + x * 2];
            out.push(p.r());
            out.push(p.g());
            out.push(p.b());
            out.push(p.a());
        }
    }
    (out, new_w, new_h)
}

fn nearest_half_rgba(src: &[u8], width: u32, height: u32) -> (Vec<u8>, u32, u32) {
    let new_w = width / 2;
    let new_h = height / 2;
    let stride = width as usize * 4;
    let mut out = Vec::with_capacity((new_w as usize) * (new_h as usize) * 4);
    for y in 0..new_h as usize {
        let row = y * 2 * stride;
        for x in 0..new_w as usize {
            let idx = row + x * 2 * 4;
            out.push(src[idx]);
            out.push(src[idx + 1]);
            out.push(src[idx + 2]);
            out.push(src[idx + 3]);
        }
    }
    (out, new_w, new_h)
}

fn create_mipmapped_texture(
    render_state: &eframe::egui_wgpu::RenderState,
    pixels: &egui::ColorImage,
) -> (egui::TextureId, wgpu::Texture, [usize; 2]) {
    let device = &render_state.device;
    let queue = &render_state.queue;
    let max_dim = device.limits().max_texture_dimension_2d;

    let orig_w = pixels.size[0] as u32;
    let orig_h = pixels.size[1] as u32;

    let (mut data, mut w, mut h) = if orig_w > max_dim || orig_h > max_dim {
        let (d, nw, nh) = nearest_half_from_pixels(&pixels.pixels, orig_w, orig_h);
        (d, nw, nh)
    } else {
        (color_image_to_rgba(pixels), orig_w, orig_h)
    };

    while w > max_dim || h > max_dim {
        let (shrunk, nw, nh) = nearest_half_rgba(&data, w, h);
        data = shrunk;
        w = nw;
        h = nh;
    }

    let actual_size = [w as usize, h as usize];
    let mip_levels = compute_mip_levels(w, h);

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("mipmapped_image"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: mip_levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * 4),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );

    let mut current_data = data;
    let mut current_w = w;
    let mut current_h = h;
    for level in 1..mip_levels {
        let (mip_data, mip_w, mip_h) = box_filter_mip(&current_data, current_w, current_h);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &mip_data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(mip_w * 4),
                rows_per_image: Some(mip_h),
            },
            wgpu::Extent3d {
                width: mip_w,
                height: mip_h,
                depth_or_array_layers: 1,
            },
        );
        current_data = mip_data;
        current_w = mip_w;
        current_h = mip_h;
    }

    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let mut renderer = render_state.renderer.write();
    let tex_id = renderer.register_native_texture(device, &view, wgpu::FilterMode::Linear);
    (tex_id, texture, actual_size)
}

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
    texture_id: Option<egui::TextureId>,
    wgpu_texture: Option<wgpu::Texture>,
    image_size: Option<[usize; 2]>,
    file_list: Option<FileList>,
    cache: ImageCache,
    transform: ViewTransform,
    error_message: Option<String>,
    slideshow_active: bool,
    slideshow_interval: f64,
    slideshow_last_advance: f64,
    fit_mode: FitMode,
    show_titlebar_menu: bool,
    titlebar_menu_pos: egui::Pos2,
    is_maximized: bool,
    settings: Settings,
    render_state: Arc<eframe::egui_wgpu::RenderState>,
}

impl ViewerApp {
    pub fn new(
        initial_path: Option<PathBuf>,
        render_state: Arc<eframe::egui_wgpu::RenderState>,
    ) -> Self {
        let settings = Settings::load();
        let mut transform = ViewTransform::default();
        transform.rotation = settings.rotation;
        let fit_mode = match settings.fit_mode {
            FitModeSetting::FitToWindow => FitMode::FitToWindow,
            FitModeSetting::OriginalSize => FitMode::OriginalSize,
        };

        let mut app = Self {
            texture_id: None,
            wgpu_texture: None,
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
            render_state,
        };

        if let Some(path) = initial_path {
            app.open_file(&path);
        }

        app
    }

    fn free_current_texture(&mut self) {
        if let Some(id) = self.texture_id.take() {
            let mut renderer = self.render_state.renderer.write();
            renderer.free_texture(&id);
        }
        self.wgpu_texture = None;
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
            file_list.re_sort_dirs(self.saved_sort_key(), self.saved_sort_order());
            file_list.re_sort_files(self.saved_file_sort_key(), self.saved_file_sort_order());
            file_list.set_current(&canonical);
            self.file_list = Some(file_list);
        }

        self.load_current_image();
    }

    fn open_directory(&mut self, dir: &Path) {
        let mut file_list = FileList::from_directory(dir);
        file_list.set_group_by(self.saved_group_by());
        file_list.re_sort_dirs(self.saved_sort_key(), self.saved_sort_order());
        file_list.re_sort_files(self.saved_file_sort_key(), self.saved_file_sort_order());
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
            self.settings.file_sort_key = match fl.file_sort_key {
                SortKey::Name => SortKeySetting::Name,
                SortKey::ModifiedDate => SortKeySetting::ModifiedDate,
            };
            self.settings.file_sort_order = match fl.file_sort_order {
                SortOrder::Ascending => SortOrderSetting::Ascending,
                SortOrder::Descending => SortOrderSetting::Descending,
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

    fn saved_file_sort_key(&self) -> SortKey {
        match self.settings.file_sort_key {
            SortKeySetting::Name => SortKey::Name,
            SortKeySetting::ModifiedDate => SortKey::ModifiedDate,
        }
    }

    fn saved_file_sort_order(&self) -> SortOrder {
        match self.settings.file_sort_order {
            SortOrderSetting::Ascending => SortOrder::Ascending,
            SortOrderSetting::Descending => SortOrder::Descending,
        }
    }

    /// Load the current image from the file list (using cache if available).
    fn load_current_image(&mut self) {
        self.free_current_texture();
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

    fn handle_mouse_pan(&mut self, response: &egui::Response, available: egui::Vec2) {
        if self.transform.right_drag_active || self.show_titlebar_menu {
            return;
        }
        let img_size = match self.image_size {
            Some(s) => s,
            None => return,
        };

        let fit = self.fit_scale(available);
        let zoom = self.transform.zoom;
        let (display_w, display_h) = if self.transform.is_rotated_90_or_270() {
            (img_size[1] as f32 * fit * zoom, img_size[0] as f32 * fit * zoom)
        } else {
            (img_size[0] as f32 * fit * zoom, img_size[1] as f32 * fit * zoom)
        };

        let overflow_x = (display_w - available.x).max(0.0);
        let overflow_y = (display_h - available.y).max(0.0);
        if overflow_x <= 0.0 && overflow_y <= 0.0 {
            return;
        }

        let mouse_pos = match response.hover_pos() {
            Some(p) => p,
            None => return,
        };

        let rect = response.rect;
        let norm_x = if available.x > 0.0 {
            ((mouse_pos.x - rect.center().x) / (available.x * 0.5)).clamp(-1.0, 1.0)
        } else {
            0.0
        };
        let norm_y = if available.y > 0.0 {
            ((mouse_pos.y - rect.center().y) / (available.y * 0.5)).clamp(-1.0, 1.0)
        } else {
            0.0
        };

        let target_x = -norm_x * overflow_x * 0.5;
        let target_y = -norm_y * overflow_y * 0.5;
        let target = egui::vec2(target_x, target_y);

        let dt = response.ctx.input(|i| i.stable_dt).min(0.1);
        let speed = 8.0;
        let t = (speed * dt).min(1.0);
        self.transform.pan = self.transform.pan + (target - self.transform.pan) * t;

        if (target - self.transform.pan).length() > 0.5 {
            response.ctx.request_repaint();
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
                    let text_rect = ui.available_rect_before_wrap();
                    ui.painter().text(
                        egui::pos2(text_rect.left(), text_rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        &title_text,
                        egui::FontId::proportional(13.0),
                        egui::Color32::from_gray(220),
                    );

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
        if !self.show_titlebar_menu {
            return;
        }

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.show_titlebar_menu = false;
            return;
        }

        let menu_pos = self.titlebar_menu_pos;
        let menu_response = egui::Window::new("titlebar_context_menu")
            .title_bar(false)
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

                ui.label(egui::RichText::new("フォルダ並び順").size(12.0));
                {
                    let mut key = self.file_list.as_ref().map(|fl| fl.sort_key).unwrap_or(SortKey::Name);
                    let prev = key;
                    ui.radio_value(&mut key, SortKey::Name, "名前");
                    ui.radio_value(&mut key, SortKey::ModifiedDate, "更新日時");
                    if key != prev {
                        if let Some(fl) = &mut self.file_list {
                            let order = fl.sort_order;
                            fl.re_sort_dirs(key, order);
                        }
                        self.save_settings();
                    }
                }
                {
                    let mut order = self.file_list.as_ref().map(|fl| fl.sort_order).unwrap_or(SortOrder::Ascending);
                    let prev = order;
                    ui.radio_value(&mut order, SortOrder::Ascending, "昇順");
                    ui.radio_value(&mut order, SortOrder::Descending, "降順");
                    if order != prev {
                        if let Some(fl) = &mut self.file_list {
                            let key = fl.sort_key;
                            fl.re_sort_dirs(key, order);
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

                ui.label(egui::RichText::new("ファイル並び順").size(12.0));
                {
                    let mut key = self.file_list.as_ref().map(|fl| fl.file_sort_key).unwrap_or(SortKey::Name);
                    let prev = key;
                    ui.radio_value(&mut key, SortKey::Name, "名前");
                    ui.radio_value(&mut key, SortKey::ModifiedDate, "更新日時");
                    if key != prev {
                        if let Some(fl) = &mut self.file_list {
                            let order = fl.file_sort_order;
                            fl.re_sort_files(key, order);
                        }
                        self.save_settings();
                    }
                }
                {
                    let mut order = self.file_list.as_ref().map(|fl| fl.file_sort_order).unwrap_or(SortOrder::Ascending);
                    let prev = order;
                    ui.radio_value(&mut order, SortOrder::Ascending, "昇順");
                    ui.radio_value(&mut order, SortOrder::Descending, "降順");
                    if order != prev {
                        if let Some(fl) = &mut self.file_list {
                            let key = fl.file_sort_key;
                            fl.re_sort_files(key, order);
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

        if let Some(inner) = menu_response {
            if ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Primary)) {
                if let Some(pos) = ctx.input(|i| i.pointer.latest_pos()) {
                    if !inner.response.rect.contains(pos) {
                        self.show_titlebar_menu = false;
                    }
                }
            }
        }
    }
}

impl ViewerApp {
    fn handle_window_resize(&self, ctx: &egui::Context) {
        if self.is_maximized {
            return;
        }
        const EDGE: f32 = 6.0;
        let rect = ctx.screen_rect();
        let pos = match ctx.input(|i| i.pointer.latest_pos()) {
            Some(p) => p,
            None => return,
        };

        let left = pos.x - rect.left() < EDGE;
        let right = rect.right() - pos.x < EDGE;
        let top = pos.y - rect.top() < EDGE;
        let bottom = rect.bottom() - pos.y < EDGE;

        let (direction, cursor) = match (left, right, top, bottom) {
            (true, _, true, _) => (
                egui::ResizeDirection::NorthWest,
                egui::CursorIcon::ResizeNwSe,
            ),
            (_, true, true, _) => (
                egui::ResizeDirection::NorthEast,
                egui::CursorIcon::ResizeNeSw,
            ),
            (true, _, _, true) => (
                egui::ResizeDirection::SouthWest,
                egui::CursorIcon::ResizeNeSw,
            ),
            (_, true, _, true) => (
                egui::ResizeDirection::SouthEast,
                egui::CursorIcon::ResizeNwSe,
            ),
            (true, _, _, _) => (
                egui::ResizeDirection::West,
                egui::CursorIcon::ResizeHorizontal,
            ),
            (_, true, _, _) => (
                egui::ResizeDirection::East,
                egui::CursorIcon::ResizeHorizontal,
            ),
            (_, _, true, _) => (
                egui::ResizeDirection::North,
                egui::CursorIcon::ResizeVertical,
            ),
            (_, _, _, true) => (
                egui::ResizeDirection::South,
                egui::CursorIcon::ResizeVertical,
            ),
            _ => return,
        };

        ctx.set_cursor_icon(cursor);
        if ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Primary)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
        }
    }
}

impl eframe::App for ViewerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Poll cache for completed preloads.
        self.cache.poll();

        // Handle window edge resize.
        self.handle_window_resize(ctx);

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
                    if self.texture_id.is_none() {
                        if let Some(pixels) = self.cache.get(&path) {
                            let (id, tex, actual_size) =
                                create_mipmapped_texture(&self.render_state, pixels);
                            self.image_size = Some(actual_size);
                            self.texture_id = Some(id);
                            self.wgpu_texture = Some(tex);
                        }
                    }

                    if let (Some(tex_id), Some(img_size)) = (self.texture_id, &self.image_size) {
                        let fit = self.fit_scale(available);

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

                        let (response, painter) =
                            ui.allocate_painter(available, egui::Sense::click_and_drag());

                        let uvs = self.transform.rotated_uvs();
                        let mut mesh = egui::Mesh::with_texture(tex_id);
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

                        self.handle_zoom_input(&response, available);
                        self.handle_mouse_pan(&response, available);
                    }
                } else if self.file_list.is_none() {
                    ui.centered_and_justified(|ui| {
                        ui.label("Visual Media Viewer - Drop an image or pass a file path as argument");
                    });
                }
            });
    }
}
