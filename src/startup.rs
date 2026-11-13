use eframe::egui;
use std::path::PathBuf;

pub struct DeferredFileOpen {
    path: Option<PathBuf>,
    frames_started: u8,
}

impl DeferredFileOpen {
    pub fn new(path: Option<PathBuf>) -> Self {
        Self {
            path,
            frames_started: 0,
        }
    }

    pub fn begin_frame(&mut self) {
        self.frames_started = self.frames_started.saturating_add(1);
    }

    pub fn take_ready(&mut self) -> Option<PathBuf> {
        (self.frames_started >= 2)
            .then(|| self.path.take())
            .flatten()
    }

    pub fn cancel(&mut self) {
        self.path = None;
    }

    pub fn is_pending(&self) -> bool {
        self.path.is_some()
    }
}

pub fn preview_dimension(size: egui::Vec2, pixels_per_point: f32) -> u32 {
    (size.x.max(size.y) * pixels_per_point).ceil().max(1.0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_open_waits_for_second_native_frame_not_layout_pass() {
        let path = PathBuf::from("initial.jpg");
        let mut pending = DeferredFileOpen::new(Some(path.clone()));
        pending.begin_frame();
        assert_eq!(pending.take_ready(), None);
        assert_eq!(pending.take_ready(), None);
        assert!(pending.is_pending());
        pending.begin_frame();
        assert_eq!(pending.take_ready(), Some(path));
        assert_eq!(pending.take_ready(), None);
    }

    #[test]
    fn newer_file_selection_cancels_initial_open() {
        let mut pending = DeferredFileOpen::new(Some(PathBuf::from("initial.mp4")));
        pending.begin_frame();
        pending.cancel();
        pending.begin_frame();
        assert_eq!(pending.take_ready(), None);
        assert!(!pending.is_pending());
    }

    #[test]
    fn preview_uses_physical_viewport_size_including_high_dpi() {
        assert_eq!(preview_dimension(egui::vec2(800.0, 600.0), 1.0), 800);
        assert_eq!(preview_dimension(egui::vec2(800.0, 600.0), 1.5), 1200);
        assert_eq!(preview_dimension(egui::vec2(600.0, 900.25), 2.0), 1801);
        assert_eq!(preview_dimension(egui::Vec2::ZERO, 1.0), 1);
    }
}
