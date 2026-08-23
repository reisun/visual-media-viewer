use eframe::egui;
#[cfg(target_os = "windows")]
use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
use std::collections::HashMap;
use std::path::Component;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;

use crate::cache::ImageCache;
use crate::file_list::{self, FileList, GroupBy, SortKey, SortOrder};
use crate::folder_ops::{self, FolderDeleteRequest};
use crate::image_decode::{self, DecodedImage};
use crate::settings::{FitModeSetting, GroupBySetting, Settings, SortKeySetting, SortOrderSetting};
use crate::video_player::{PlaybackState, VideoPlayer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FitMode {
    FitToWindow,
    OriginalSize,
}

struct ViewTransform {
    zoom: f32,
    pan: egui::Vec2,
    rotation: u16,
    right_drag_active: bool,
    right_drag_start_y: f32,
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

    fn is_rotated_90_or_270(&self) -> bool {
        self.rotation == 90 || self.rotation == 270
    }
}

struct TextureEntry {
    texture_id: egui::TextureId,
    _wgpu_texture: wgpu::Texture,
    size: [usize; 2],
}

fn normalize_slideshow_interval(interval: f64) -> f64 {
    ((interval.clamp(1.0, 30.0)) * 10.0).round() / 10.0
}

fn common_named_ancestor(a: &Path, b: &Path) -> Option<PathBuf> {
    let mut common = PathBuf::new();
    let mut matched = false;

    for (a_comp, b_comp) in a.components().zip(b.components()) {
        if a_comp != b_comp {
            break;
        }
        common.push(a_comp.as_os_str());
        matched = true;
    }

    if matched && common.file_name().is_some() {
        Some(common)
    } else {
        None
    }
}

fn update_title_root(root: &mut Option<PathBuf>, current_dir: &Path) {
    match root.take() {
        Some(existing) => {
            *root = Some(
                common_named_ancestor(&existing, current_dir)
                    .unwrap_or_else(|| current_dir.to_path_buf()),
            );
        }
        None => *root = Some(current_dir.to_path_buf()),
    }
}

fn path_component_display(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn build_title_path(title_root: &Path, current_dir: &Path, filename: &str) -> String {
    let root_name = title_root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| title_root.display().to_string());

    let relative_child = current_dir
        .strip_prefix(title_root)
        .ok()
        .map(path_component_display)
        .unwrap_or_default();

    if relative_child.is_empty() {
        format!("{root_name}/{filename}")
    } else {
        format!("{root_name}/{relative_child}/{filename}")
    }
}

#[derive(Debug, Clone)]
struct DialogMessage {
    title: String,
    body: String,
}

pub struct ViewerApp {
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
    title_root: Option<PathBuf>,
    is_maximized: bool,
    video_player: Option<VideoPlayer>,
    video_texture_id: Option<egui::TextureId>,
    video_wgpu_texture: Option<wgpu::Texture>,
    video_size: Option<[u32; 2]>,
    pending_slideshow_timer_reset: bool,
    finished_video_advance_handled: bool,
    settings: Settings,
    render_state: Arc<eframe::egui_wgpu::RenderState>,
    ipc_rx: mpsc::Receiver<PathBuf>,
    texture_cache: HashMap<PathBuf, TextureEntry>,
    pending_folder_delete: Option<FolderDeleteRequest>,
    dialog_message: Option<DialogMessage>,
}

impl ViewerApp {
    pub fn new(
        initial_path: Option<PathBuf>,
        render_state: Arc<eframe::egui_wgpu::RenderState>,
        ipc_rx: mpsc::Receiver<PathBuf>,
    ) -> Self {
        let settings = Settings::load();
        let mut transform = ViewTransform::default();
        transform.rotation = settings.rotation;
        let fit_mode = match settings.fit_mode {
            FitModeSetting::FitToWindow => FitMode::FitToWindow,
            FitModeSetting::OriginalSize => FitMode::OriginalSize,
        };

        let mut app = Self {
            image_size: None,
            file_list: None,
            cache: ImageCache::new(
                1_500_000_000,
                render_state.device.limits().max_texture_dimension_2d,
            ),
            transform,
            error_message: None,
            slideshow_active: false,
            slideshow_interval: normalize_slideshow_interval(settings.slideshow_interval_secs),
            slideshow_last_advance: 0.0,
            fit_mode,
            show_titlebar_menu: false,
            titlebar_menu_pos: egui::Pos2::ZERO,
            title_root: None,
            is_maximized: settings.maximized,
            video_player: None,
            video_texture_id: None,
            video_wgpu_texture: None,
            video_size: None,
            pending_slideshow_timer_reset: false,
            finished_video_advance_handled: false,
            settings,
            render_state,
            ipc_rx,
            texture_cache: HashMap::new(),
            pending_folder_delete: None,
            dialog_message: None,
        };

        if let Some(path) = initial_path {
            app.open_file(&path);
        }

        app
    }

    fn evict_distant_textures(&mut self) {
        let nearby: Vec<PathBuf> = self
            .file_list
            .as_ref()
            .map(|fl| fl.nearby_paths(3))
            .unwrap_or_default();
        let to_remove: Vec<PathBuf> = self
            .texture_cache
            .keys()
            .filter(|p| !nearby.contains(p))
            .cloned()
            .collect();
        if !to_remove.is_empty() {
            let mut renderer = self.render_state.renderer.write();
            for path in to_remove {
                if let Some(entry) = self.texture_cache.remove(&path) {
                    renderer.free_texture(&entry.texture_id);
                }
            }
        }
    }

    fn pre_upload_nearby_textures(&mut self) {
        let nearby = match self.file_list.as_ref() {
            Some(fl) => fl.nearby_paths(3),
            None => return,
        };
        for path in nearby {
            if file_list::is_video_file(&path) {
                continue;
            }
            if self.texture_cache.contains_key(&path) {
                continue;
            }
            if let Some(pixels) = self.cache.get(&path) {
                let (id, tex, actual_size) =
                    image_decode::create_mipmapped_texture(&self.render_state, pixels);
                self.texture_cache.insert(
                    path,
                    TextureEntry {
                        texture_id: id,
                        _wgpu_texture: tex,
                        size: actual_size,
                    },
                );
                return;
            }
        }
    }

    fn free_video_texture(&mut self) {
        if let Some(id) = self.video_texture_id.take() {
            let mut renderer = self.render_state.renderer.write();
            renderer.free_texture(&id);
        }
        self.video_wgpu_texture = None;
    }

    fn stop_video(&mut self) {
        if let Some(mut player) = self.video_player.take() {
            player.stop();
        }
        self.free_video_texture();
        self.video_size = None;
    }

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
            self.after_file_selection_changed();
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
            self.after_file_selection_changed();
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
        self.settings.slideshow_interval_secs = self.slideshow_interval;
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

    fn load_current_image(&mut self) {
        self.stop_video();
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

        if file_list::is_video_file(&path) {
            match VideoPlayer::open(&path, self.settings.normalize_audio) {
                Ok(player) => {
                    player.set_volume(self.settings.volume);
                    self.video_size = Some(player.video_size);
                    self.image_size =
                        Some([player.video_size[0] as usize, player.video_size[1] as usize]);
                    self.video_player = Some(player);
                }
                Err(e) => {
                    log::error!("{}", e);
                    self.error_message = Some(e);
                }
            }
        } else {
            match self.cache.get(&path) {
                Some(pixels) => {
                    self.image_size = Some(pixels.size);
                }
                None => {
                    match DecodedImage::load(
                        &path,
                        self.render_state.device.limits().max_texture_dimension_2d,
                    ) {
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
            self.start_preload();
        }
    }

    fn start_preload(&mut self) {
        if let Some(fl) = &self.file_list {
            let paths = fl.nearby_paths(3);
            self.cache.preload(paths);
        }
    }

    fn after_file_selection_changed(&mut self) {
        self.pending_slideshow_timer_reset = self.slideshow_active;
        self.finished_video_advance_handled = false;
        if let Some(dir) = self
            .file_list
            .as_ref()
            .map(|fl| fl.directory().to_path_buf())
        {
            update_title_root(&mut self.title_root, &dir);
        }
    }

    fn set_title_root_to_current_parent(&mut self) {
        self.title_root = self
            .file_list
            .as_ref()
            .map(|fl| fl.directory().to_path_buf());
    }

    fn set_dialog_message(&mut self, title: impl Into<String>, body: impl Into<String>) {
        self.dialog_message = Some(DialogMessage {
            title: title.into(),
            body: body.into(),
        });
    }

    fn maybe_reset_slideshow_timer(&mut self, now: f64) {
        if self.pending_slideshow_timer_reset {
            self.slideshow_last_advance = now;
            self.pending_slideshow_timer_reset = false;
        }
    }

    fn set_slideshow_active(&mut self, active: bool, now: f64) {
        self.slideshow_active = active;
        if active {
            self.slideshow_last_advance = now;
            self.pending_slideshow_timer_reset = false;
            self.finished_video_advance_handled = false;
        }
    }

    fn adjust_slideshow_interval(&mut self, delta: f64) {
        let updated = normalize_slideshow_interval(self.slideshow_interval + delta);
        if (updated - self.slideshow_interval).abs() > f64::EPSILON {
            self.slideshow_interval = updated;
            self.save_settings();
        }
    }

    fn next_image(&mut self) {
        let mut moved = false;
        if let Some(fl) = &mut self.file_list {
            moved = fl.next();
        }
        if moved {
            self.after_file_selection_changed();
            self.load_current_image();
        }
    }

    fn prev_image(&mut self) {
        let mut moved = false;
        if let Some(fl) = &mut self.file_list {
            moved = fl.prev();
        }
        if moved {
            self.after_file_selection_changed();
            self.load_current_image();
        }
    }

    fn skip_files_by_page(&mut self, delta: isize) {
        let mut moved = false;
        if let Some(fl) = &mut self.file_list {
            moved = fl.advance_clamped_with_edge_loop(delta);
        }
        if moved {
            self.after_file_selection_changed();
            self.load_current_image();
        }
    }

    fn title_text(&self) -> String {
        if let Some(fl) = &self.file_list {
            if let Some(path) = fl.current_path() {
                let filename = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let title_path = self
                    .title_root
                    .as_ref()
                    .map(|root| build_title_path(root, fl.directory(), &filename))
                    .unwrap_or_else(|| {
                        let parent_name = fl
                            .directory()
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        format!("{parent_name}/{filename}")
                    });
                let position = format!("{} / {}", fl.current_index() + 1, fl.file_count());
                let mut title = format!("{} ({})", title_path, position);
                if let Some(player) = &self.video_player {
                    let state_str = match player.state {
                        PlaybackState::Playing => "再生中",
                        PlaybackState::Paused => "一時停止",
                        PlaybackState::Finished => "終了",
                    };
                    let vol = player.volume();
                    let pos = player.current_pts().max(0.0);
                    let dur = player.duration.max(0.0);
                    let fmt = |s: f64| -> String {
                        let m = (s as u64) / 60;
                        let sec = (s as u64) % 60;
                        format!("{}:{:02}", m, sec)
                    };
                    title.push_str(&format!(
                        " [{} Vol:{}%] [{} / {}]",
                        state_str,
                        vol,
                        fmt(pos),
                        fmt(dur)
                    ));
                } else if self.slideshow_active {
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
    fn compute_display_rect(
        &self,
        img_size: &[usize; 2],
        available: egui::Vec2,
        center: egui::Pos2,
    ) -> egui::Rect {
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
        egui::Rect::from_center_size(
            center + self.transform.pan,
            egui::vec2(display_w, display_h),
        )
    }

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

    fn handle_zoom_input(&mut self, response: &egui::Response, _available_size: egui::Vec2) {
        let pointer_pos = response.hover_pos().unwrap_or(response.rect.center());
        let rect_center = response.rect.center();

        let secondary_pressed = response
            .ctx
            .input(|i| i.pointer.button_pressed(egui::PointerButton::Secondary));
        if secondary_pressed && response.hovered() && !self.show_titlebar_menu {
            self.transform.right_drag_active = true;
            self.transform.right_drag_start_y = pointer_pos.y;
            self.transform.right_drag_start_zoom = self.transform.zoom;
        }

        if self.transform.right_drag_active {
            let ctx = response.ctx.clone();
            if ctx.input(|i| i.pointer.secondary_down()) {
                let dy = self.transform.right_drag_start_y - pointer_pos.y;
                let zoom_factor = (dy / 200.0).exp();
                let new_zoom =
                    (self.transform.right_drag_start_zoom * zoom_factor).clamp(0.1, 50.0);

                let pointer_offset = pointer_pos - rect_center;
                let old_zoom = self.transform.zoom;
                self.transform.zoom = new_zoom;
                let zoom_ratio = new_zoom / old_zoom;
                self.transform.pan =
                    self.transform.pan * zoom_ratio + pointer_offset * (1.0 - zoom_ratio);
            } else {
                self.transform.right_drag_active = false;
            }
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
            (
                img_size[1] as f32 * fit * zoom,
                img_size[0] as f32 * fit * zoom,
            )
        } else {
            (
                img_size[0] as f32 * fit * zoom,
                img_size[1] as f32 * fit * zoom,
            )
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
        let mouse_offset_x = mouse_pos.x - rect.center().x;
        let mouse_offset_y = mouse_pos.y - rect.center().y;

        let mult_x = if available.x > 0.0 {
            (overflow_x / available.x).max(1.0)
        } else {
            1.0
        };
        let mult_y = if available.y > 0.0 {
            (overflow_y / available.y).max(1.0)
        } else {
            1.0
        };

        let pan_x = (-mouse_offset_x * mult_x).clamp(-overflow_x * 0.5, overflow_x * 0.5);
        let pan_y = (-mouse_offset_y * mult_y).clamp(-overflow_y * 0.5, overflow_y * 0.5);

        self.transform.pan = egui::vec2(pan_x, pan_y);
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

    fn navigate_prev_folder_unit(&mut self) {
        let dir = match &self.file_list {
            Some(fl) => match self.title_root.as_deref() {
                Some(root) => fl.prev_folder_unit_within(root),
                None => fl.prev_image_dir(),
            },
            None => None,
        };
        if let Some(d) = dir {
            self.open_directory(&d);
        }
    }

    fn navigate_next_folder_unit(&mut self) {
        let dir = match &self.file_list {
            Some(fl) => match self.title_root.as_deref() {
                Some(root) => fl.next_folder_unit_within(root),
                None => fl.next_image_dir(),
            },
            None => None,
        };
        if let Some(d) = dir {
            self.open_directory(&d);
        }
    }

    fn request_delete_current_folder(&mut self) {
        let Some(fl) = self.file_list.as_ref() else {
            return;
        };
        let Some(current_file) = fl.current_path().map(|path| path.to_path_buf()) else {
            return;
        };

        match folder_ops::prepare_folder_delete_request(
            &current_file,
            fl.next_image_dir_after_current_subtree(),
        ) {
            Ok(request) => {
                self.pending_folder_delete = Some(request);
            }
            Err(err) => {
                self.set_dialog_message("削除できません", err);
            }
        }
    }

    fn confirm_delete_current_folder(&mut self) {
        let Some(request) = self.pending_folder_delete.clone() else {
            return;
        };

        // FFmpeg keeps the current video open from its demuxer thread. Stop
        // and join the playback session before asking Windows to recycle the
        // containing folder so every file handle has been released.
        let restart_video_on_failure = self.video_player.is_some();
        if restart_video_on_failure {
            self.stop_video();
        }

        match folder_ops::move_folder_to_recycle_bin(&request.target_dir) {
            Ok(()) => {
                self.pending_folder_delete = None;
                self.open_directory(&request.next_dir);
            }
            Err(err) => {
                self.pending_folder_delete = None;
                if restart_video_on_failure {
                    self.load_current_image();
                }
                self.set_dialog_message("削除に失敗しました", err);
            }
        }
    }

    fn video_seek_clamped(&mut self, delta: f64) {
        const EDGE_THRESHOLD: f64 = 2.0;
        if let Some(player) = &mut self.video_player {
            let current = player.current_pts();
            let target = current + delta;
            let duration = player.duration;
            if target < 0.0 {
                if current > EDGE_THRESHOLD {
                    player.seek(0.0).ok();
                } else {
                    self.prev_image();
                }
            } else if target >= duration && duration > 0.0 {
                if (duration - current) > EDGE_THRESHOLD {
                    player.seek((duration - 0.1).max(0.0)).ok();
                } else {
                    self.next_image();
                }
            } else {
                player.seek(target).ok();
            }
        }
    }

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

                if bar_response.dragged_by(egui::PointerButton::Primary) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }

                if bar_response.double_clicked() {
                    self.is_maximized = !self.is_maximized;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(self.is_maximized));
                    self.settings.maximized = self.is_maximized;
                    self.settings.save();
                }

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

                        let close_btn = ui.add_sized(
                            btn_size,
                            egui::Button::new(
                                egui::RichText::new("X")
                                    .color(egui::Color32::from_gray(200))
                                    .size(13.0),
                            )
                            .frame(false),
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

                        let max_label = if self.is_maximized { "[ ]" } else { "[ ]" };
                        let max_btn = ui.add_sized(
                            btn_size,
                            egui::Button::new(
                                egui::RichText::new(max_label)
                                    .color(egui::Color32::from_gray(200))
                                    .size(11.0),
                            )
                            .frame(false),
                        );
                        if max_btn.clicked() {
                            self.is_maximized = !self.is_maximized;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(
                                self.is_maximized,
                            ));
                        }

                        let min_btn = ui.add_sized(
                            btn_size,
                            egui::Button::new(
                                egui::RichText::new("_")
                                    .color(egui::Color32::from_gray(200))
                                    .size(13.0),
                            )
                            .frame(false),
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
        let max_menu_height = ctx.screen_rect().height() - menu_pos.y - 20.0;
        let menu_response = egui::Window::new("titlebar_context_menu")
            .title_bar(false)
            .fixed_pos(menu_pos)
            .collapsible(false)
            .resizable(false)
            .default_width(220.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(max_menu_height.max(200.0))
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new("リスト").strong().size(13.0));
                        ui.separator();

                        if ui.button("前のファイル").clicked() {
                            self.prev_image();
                        }

                        if ui.button("次のファイル").clicked() {
                            self.next_image();
                        }

                        ui.separator();

                        ui.label(egui::RichText::new("フォルダ並び順").size(12.0));
                        {
                            let mut key = self
                                .file_list
                                .as_ref()
                                .map(|fl| fl.sort_key)
                                .unwrap_or(SortKey::Name);
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
                            let mut order = self
                                .file_list
                                .as_ref()
                                .map(|fl| fl.sort_order)
                                .unwrap_or(SortOrder::Ascending);
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
                            let mut group = self
                                .file_list
                                .as_ref()
                                .map(|fl| fl.group_by)
                                .unwrap_or(GroupBy::Off);
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
                            let mut key = self
                                .file_list
                                .as_ref()
                                .map(|fl| fl.file_sort_key)
                                .unwrap_or(SortKey::Name);
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
                            let mut order = self
                                .file_list
                                .as_ref()
                                .map(|fl| fl.file_sort_order)
                                .unwrap_or(SortOrder::Ascending);
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
                            let label = if self.slideshow_active {
                                "停止"
                            } else {
                                "開始"
                            };
                            if ui.button(label).clicked() {
                                self.set_slideshow_active(
                                    !self.slideshow_active,
                                    ui.ctx().input(|i| i.time),
                                );
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
                                self.slideshow_interval = normalize_slideshow_interval(interval);
                                self.save_settings();
                            }
                        });

                        ui.separator();

                        ui.label(egui::RichText::new("音声同期").size(12.0));
                        ui.horizontal(|ui| {
                            ui.label("オフセット:");
                            let mut offset = self.settings.audio_offset_ms;
                            let drag = egui::DragValue::new(&mut offset)
                                .range(-200..=200)
                                .speed(5)
                                .suffix("ms");
                            if ui.add(drag).changed() {
                                self.settings.audio_offset_ms = offset;
                                self.save_settings();
                            }
                        });
                        {
                            let mut norm = self.settings.normalize_audio;
                            if ui.checkbox(&mut norm, "音量ノーマライズ").changed() {
                                self.settings.normalize_audio = norm;
                                if let Some(player) = &self.video_player {
                                    player.set_normalize(norm);
                                }
                                self.save_settings();
                            }
                        }

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

    fn draw_folder_delete_dialog(&mut self, ctx: &egui::Context) {
        let Some(request) = self.pending_folder_delete.clone() else {
            return;
        };

        let mut cancel = false;
        let mut confirm = false;

        egui::Window::new("delete_folder_confirm")
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .show(ctx, |ui| {
                ui.label("現在のファイルの親フォルダを Windows のごみ箱へ移動します。");
                ui.add_space(6.0);
                ui.label(format!("対象: {}", request.target_dir.display()));
                ui.label(format!("削除後の移動先: {}", request.next_dir.display()));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("キャンセル").clicked() {
                        cancel = true;
                    }
                    if ui
                        .button(format!("「{}」を削除", request.target_name()))
                        .clicked()
                    {
                        confirm = true;
                    }
                });
            });

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            cancel = true;
        }

        if cancel {
            self.pending_folder_delete = None;
        } else if confirm {
            self.confirm_delete_current_folder();
        }
    }

    fn draw_dialog_message(&mut self, ctx: &egui::Context) {
        let Some(message) = self.dialog_message.clone() else {
            return;
        };

        let mut close = false;
        egui::Window::new(message.title.as_str())
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .show(ctx, |ui| {
                ui.label(message.body);
                ui.add_space(10.0);
                if ui.button("OK").clicked() {
                    close = true;
                }
            });

        if ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Enter)) {
            close = true;
        }

        if close {
            self.dialog_message = None;
        }
    }
}

#[cfg(target_os = "windows")]
fn activate_window_from_ipc(frame: &eframe::Frame, ctx: &egui::Context) {
    use std::ffi::c_void;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, BringWindowToTop, IsIconic, SetForegroundWindow, ShowWindow,
        SW_RESTORE,
    };

    if let Ok(window_handle) = frame.window_handle() {
        if let RawWindowHandle::Win32(handle) = window_handle.as_raw() {
            let hwnd = HWND(handle.hwnd.get() as *mut c_void);
            unsafe {
                AllowSetForegroundWindow(u32::MAX).ok();
                if IsIconic(hwnd).as_bool() {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                }
                let _ = ShowWindow(hwnd, SW_RESTORE);
                let _ = BringWindowToTop(hwnd);
                let _ = SetForegroundWindow(hwnd);
            }
        }
    }

    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
        egui::UserAttentionType::Informational,
    ));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
}

#[cfg(not(target_os = "windows"))]
fn activate_window_from_ipc(_frame: &eframe::Frame, ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
        egui::UserAttentionType::Informational,
    ));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
}

impl ViewerApp {
    fn track_window_size(&mut self, ctx: &egui::Context) {
        if self.is_maximized {
            return;
        }
        let rect = ctx.screen_rect();
        let w = rect.width();
        let h = rect.height();
        if (w - self.settings.window_width).abs() > 1.0
            || (h - self.settings.window_height).abs() > 1.0
        {
            self.settings.window_width = w;
            self.settings.window_height = h;
            self.settings.save();
        }
    }

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
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.cache.poll();
        self.pre_upload_nearby_textures();
        self.evict_distant_textures();

        if let Ok(path) = self.ipc_rx.try_recv() {
            self.open_file(&path);
            activate_window_from_ipc(frame, ctx);
        }

        self.track_window_size(ctx);
        self.handle_window_resize(ctx);

        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if let Some(path) = dropped.into_iter().next() {
            self.open_file(&path);
        }

        {
            let modal_open = self.pending_folder_delete.is_some() || self.dialog_message.is_some();
            let is_video = self.video_player.is_some();
            let nav = if modal_open {
                None
            } else {
                ctx.input(|i| {
                    let ctrl = i.modifiers.ctrl || i.modifiers.mac_cmd;
                    let shift = i.modifiers.shift;
                    if i.key_pressed(egui::Key::ArrowRight) {
                        if is_video && !ctrl {
                            return Some("video_skip_forward");
                        }
                        return Some("right");
                    }
                    if i.key_pressed(egui::Key::ArrowLeft) {
                        if is_video && !ctrl {
                            return Some("video_skip_backward");
                        }
                        return Some("left");
                    }
                    if i.key_pressed(egui::Key::PageUp) {
                        if is_video {
                            return Some("video_skip_back_5min");
                        }
                        return Some("page_back_5");
                    }
                    if i.key_pressed(egui::Key::PageDown) {
                        if is_video {
                            return Some("video_skip_fwd_5min");
                        }
                        return Some("page_forward_5");
                    }
                    if i.key_pressed(egui::Key::ArrowUp) {
                        if shift {
                            return Some("prev_branch");
                        }
                        return Some("prev_folder");
                    }
                    if i.key_pressed(egui::Key::ArrowDown) {
                        if shift {
                            return Some("next_branch");
                        }
                        return Some("next_folder");
                    }
                    if i.key_pressed(egui::Key::Delete) {
                        return Some("delete_folder");
                    }
                    None
                })
            };
            match nav {
                Some("video_skip_forward") => {
                    self.video_seek_clamped(5.0);
                }
                Some("video_skip_backward") => {
                    self.video_seek_clamped(-5.0);
                }
                Some("video_skip_fwd_5min") => {
                    self.video_seek_clamped(300.0);
                }
                Some("video_skip_back_5min") => {
                    self.video_seek_clamped(-300.0);
                }
                Some("right") => self.next_image(),
                Some("left") => self.prev_image(),
                Some("page_back_5") => self.skip_files_by_page(-5),
                Some("page_forward_5") => self.skip_files_by_page(5),
                Some("prev_folder") => self.navigate_prev_folder(),
                Some("next_folder") => self.navigate_next_folder(),
                Some("prev_branch") => self.navigate_prev_folder_unit(),
                Some("next_branch") => self.navigate_next_folder_unit(),
                Some("delete_folder") => self.request_delete_current_folder(),
                _ => {}
            }

            let rotate = if modal_open {
                None
            } else {
                ctx.input(|i| {
                    if i.key_pressed(egui::Key::R) {
                        if i.modifiers.shift {
                            return Some(false); // CCW
                        }
                        return Some(true); // CW
                    }
                    None
                })
            };
            if let Some(cw) = rotate {
                if cw {
                    self.transform.rotate_cw();
                } else {
                    self.transform.rotate_ccw();
                }
                self.save_settings();
            }

            let slideshow_key_action = if modal_open {
                None
            } else {
                ctx.input(|i| {
                    if i.key_pressed(egui::Key::S) {
                        if i.modifiers.shift {
                            return Some(false);
                        }
                        return Some(true);
                    }
                    None
                })
            };
            if let Some(active) = slideshow_key_action {
                self.set_slideshow_active(active, ctx.input(|i| i.time));
            }

            let interval_change = if modal_open {
                None
            } else {
                ctx.input(|i| {
                    if i.key_pressed(egui::Key::D) && i.key_down(egui::Key::S) {
                        return Some(0.1_f64);
                    }
                    if i.key_pressed(egui::Key::F) && i.key_down(egui::Key::S) {
                        return Some(-0.1_f64);
                    }
                    if i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals) {
                        return Some(0.1_f64);
                    }
                    if i.key_pressed(egui::Key::Minus) {
                        return Some(-0.1_f64);
                    }
                    None
                })
            };
            if let Some(delta) = interval_change {
                self.adjust_slideshow_interval(delta);
            }

            if !modal_open && ctx.input(|i| i.key_pressed(egui::Key::Space)) {
                if let Some(player) = &mut self.video_player {
                    player.toggle_pause();
                }
            }

            if !modal_open && ctx.input(|i| i.key_pressed(egui::Key::N)) {
                self.set_title_root_to_current_parent();
            }

            if !modal_open && ctx.input(|i| i.key_pressed(egui::Key::F12)) {
                self.dump_diagnostics();
            }
        }

        self.maybe_reset_slideshow_timer(ctx.input(|i| i.time));

        if self.slideshow_active {
            if let Some(player) = &self.video_player {
                if matches!(player.state, PlaybackState::Finished) {
                    if !self.finished_video_advance_handled {
                        self.finished_video_advance_handled = true;
                        self.next_image();
                    }
                } else {
                    self.finished_video_advance_handled = false;
                }
            } else {
                let now = ctx.input(|i| i.time);
                if now - self.slideshow_last_advance >= self.slideshow_interval {
                    self.slideshow_last_advance = now;
                    self.next_image();
                }
                let remaining =
                    self.slideshow_interval - (ctx.input(|i| i.time) - self.slideshow_last_advance);
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(remaining.max(0.01)));
            }
        }

        self.draw_title_bar(ctx);

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

                if self.video_player.is_some() {
                    self.render_video_frame(ui, available);
                    return;
                }

                let current_path = self
                    .file_list
                    .as_ref()
                    .and_then(|fl| fl.current_path())
                    .map(|p| p.to_path_buf());

                if let Some(path) = current_path {
                    let tex_info = if let Some(entry) = self.texture_cache.get(&path) {
                        Some((entry.texture_id, entry.size))
                    } else if let Some(pixels) = self.cache.get(&path) {
                        let (id, tex, actual_size) =
                            image_decode::create_mipmapped_texture(&self.render_state, pixels);
                        self.texture_cache.insert(
                            path.clone(),
                            TextureEntry {
                                texture_id: id,
                                _wgpu_texture: tex,
                                size: actual_size,
                            },
                        );
                        Some((id, actual_size))
                    } else {
                        None
                    };

                    if let Some((tex_id, img_size)) = tex_info {
                        self.image_size = Some(img_size);
                        let center = ui.available_rect_before_wrap().center();
                        let image_rect = self.compute_display_rect(&img_size, available, center);

                        let (response, painter) =
                            ui.allocate_painter(available, egui::Sense::click_and_drag());

                        let uvs = self.transform.rotated_uvs();
                        image_decode::paint_textured_rect(&painter, tex_id, image_rect, &uvs);

                        self.handle_zoom_input(&response, available);
                        self.handle_mouse_pan(&response, available);
                    }
                } else if self.file_list.is_none() {
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            "Visual Media Viewer - Drop an image or pass a file path as argument",
                        );
                    });
                }
            });

        self.draw_folder_delete_dialog(ctx);
        self.draw_dialog_message(ctx);
    }
}

impl ViewerApp {
    fn dump_diagnostics(&self) {
        let diag_path = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("diagnostics.txt")))
            .unwrap_or_else(|| std::path::PathBuf::from("diagnostics.txt"));
        let mut info = String::new();
        if let Some(ref fl) = self.file_list {
            if let Some(path) = fl.current_path() {
                info.push_str(&format!("File: {}\n", path.display()));
            }
        }
        if let Some(ref player) = self.video_player {
            info.push_str(&player.diag_info);
            info.push_str(&format!(
                "\nState: {:?}",
                match player.state {
                    crate::video_player::PlaybackState::Playing => "Playing",
                    crate::video_player::PlaybackState::Paused => "Paused",
                    crate::video_player::PlaybackState::Finished => "Finished",
                }
            ));
            info.push_str(&format!("\nPTS: {:.3}", player.current_pts()));
            info.push_str(&format!("\nDuration: {:.3}", player.duration));
        } else {
            info.push_str("No video player active\n");
        }
        let _ = std::fs::write(&diag_path, &info);
    }

    fn render_video_frame(&mut self, ui: &mut egui::Ui, available: egui::Vec2) {
        let is_buffering = self.video_player.as_ref().is_some_and(|p| p.is_buffering());

        let av_offset = self.settings.audio_offset_ms as f64 / 1000.0;
        let new_frame = self.video_player.as_mut().and_then(|p| {
            let f = p.poll_frame(av_offset)?;
            Some((f.rgba.clone(), f.width, f.height))
        });

        if let Some((rgba, width, height)) = new_frame {
            self.upload_video_frame(&rgba, width, height);
        }

        if is_buffering {
            ui.centered_and_justified(|ui| {
                ui.spinner();
            });
            ui.ctx().request_repaint();
            return;
        }

        let tex_id = match self.video_texture_id {
            Some(id) => id,
            None => return,
        };
        let img_size = match self.image_size {
            Some(s) => s,
            None => return,
        };

        let center = ui.available_rect_before_wrap().center();
        let image_rect = self.compute_display_rect(&img_size, available, center);

        let (response, painter) = ui.allocate_painter(available, egui::Sense::click_and_drag());

        let uvs = self.transform.rotated_uvs();
        image_decode::paint_textured_rect(&painter, tex_id, image_rect, &uvs);

        self.handle_zoom_input(&response, available);
        self.handle_mouse_pan(&response, available);

        let scroll_delta = response.ctx.input(|i| i.raw_scroll_delta.y);
        if scroll_delta.abs() > 0.0 && response.hovered() {
            if let Some(player) = &self.video_player {
                let current = player.volume() as i32;
                let step = if scroll_delta > 0.0 { 5 } else { -5 };
                let new_vol = (current + step).clamp(0, 200) as u16;
                player.set_volume(new_vol);
                self.settings.volume = new_vol;
                self.settings.save();
            }
        }

        if self
            .video_player
            .as_ref()
            .is_some_and(|p| matches!(p.state, PlaybackState::Playing))
        {
            response.ctx.request_repaint();
        }
    }

    fn upload_video_frame(&mut self, rgba: &[u8], width: u32, height: u32) {
        let render_state = Arc::clone(&self.render_state);
        let device = &render_state.device;
        let queue = &render_state.queue;

        let need_recreate = match &self.video_wgpu_texture {
            Some(tex) => tex.width() != width || tex.height() != height,
            None => true,
        };

        if need_recreate {
            self.free_video_texture();
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("video_frame"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let mut renderer = render_state.renderer.write();
            let tex_id = renderer.register_native_texture(device, &view, wgpu::FilterMode::Linear);
            self.video_texture_id = Some(tex_id);
            self.video_wgpu_texture = Some(texture);
        }

        if let Some(texture) = &self.video_wgpu_texture {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * 4),
                    rows_per_image: Some(height),
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_update_title_root_uses_cumulative_lca() {
        let mut title_root = None;
        update_title_root(&mut title_root, Path::new("series/vol01/ch01"));
        assert_eq!(title_root, Some(PathBuf::from("series/vol01/ch01")));

        update_title_root(&mut title_root, Path::new("series/vol01/ch02"));
        assert_eq!(title_root, Some(PathBuf::from("series/vol01")));

        update_title_root(&mut title_root, Path::new("series/vol02/ch03"));
        assert_eq!(title_root, Some(PathBuf::from("series")));
    }

    #[test]
    fn test_update_title_root_resets_when_no_common_root() {
        let mut title_root = Some(PathBuf::from("alpha/one"));
        update_title_root(&mut title_root, Path::new("beta/two"));
        assert_eq!(title_root, Some(PathBuf::from("beta/two")));
    }

    #[test]
    fn test_build_title_path_includes_relative_child_path() {
        let title = build_title_path(
            Path::new("gallery"),
            Path::new("gallery/day1/setA"),
            "frame001.jpg",
        );
        assert_eq!(title, "gallery/day1/setA/frame001.jpg");
    }

    #[test]
    fn test_normalize_slideshow_interval_rounds_and_clamps() {
        assert_eq!(normalize_slideshow_interval(0.94), 1.0);
        assert_eq!(normalize_slideshow_interval(3.14), 3.1);
        assert_eq!(normalize_slideshow_interval(45.0), 30.0);
    }
}
