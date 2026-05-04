use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortKeySetting {
    Name,
    ModifiedDate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortOrderSetting {
    Ascending,
    Descending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FitModeSetting {
    FitToWindow,
    OriginalSize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroupBySetting {
    Off,
    ModifiedDate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub rotation: u16,
    pub fit_mode: FitModeSetting,
    pub sort_key: SortKeySetting,
    pub sort_order: SortOrderSetting,
    #[serde(default)]
    pub group_by: GroupBySetting,
    #[serde(default)]
    pub file_sort_key: SortKeySetting,
    #[serde(default)]
    pub file_sort_order: SortOrderSetting,
    #[serde(default = "default_volume")]
    pub volume: u16,
    #[serde(default = "default_audio_offset_ms")]
    pub audio_offset_ms: i32,
    #[serde(default = "default_window_width")]
    pub window_width: f32,
    #[serde(default = "default_window_height")]
    pub window_height: f32,
    #[serde(default)]
    pub maximized: bool,
}

fn default_volume() -> u16 {
    100
}

fn default_audio_offset_ms() -> i32 {
    60
}

fn default_window_width() -> f32 {
    1280.0
}

fn default_window_height() -> f32 {
    720.0
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            rotation: 0,
            fit_mode: FitModeSetting::FitToWindow,
            sort_key: SortKeySetting::Name,
            sort_order: SortOrderSetting::Ascending,
            group_by: GroupBySetting::Off,
            file_sort_key: SortKeySetting::Name,
            file_sort_order: SortOrderSetting::Ascending,
            volume: 100,
            audio_offset_ms: 60,
            window_width: 1280.0,
            window_height: 720.0,
            maximized: false,
        }
    }
}

impl Default for SortKeySetting {
    fn default() -> Self {
        Self::Name
    }
}

impl Default for SortOrderSetting {
    fn default() -> Self {
        Self::Ascending
    }
}

impl Default for GroupBySetting {
    fn default() -> Self {
        Self::Off
    }
}

impl Settings {
    fn path() -> PathBuf {
        if let Ok(appdata) = std::env::var("APPDATA") {
            PathBuf::from(appdata)
                .join("visual-media-viewer")
                .join("settings.json")
        } else if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home)
                .join(".config")
                .join("visual-media-viewer")
                .join("settings.json")
        } else {
            PathBuf::from("settings.json")
        }
    }

    pub fn load() -> Self {
        let path = Self::path();
        match std::fs::read_to_string(&path) {
            Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(data) = serde_json::to_string_pretty(self) {
            if let Err(e) = std::fs::write(&path, data) {
                log::warn!("Failed to save settings to {}: {}", path.display(), e);
            }
        }
    }
}
