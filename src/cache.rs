use eframe::egui;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use crate::image_decode::DecodedImage;

struct DecodeResult {
    path: PathBuf,
    result: Result<egui::ColorImage, String>,
}

pub struct ImageCache {
    entries: HashMap<PathBuf, egui::ColorImage>,
    lru_order: VecDeque<PathBuf>,
    max_size: usize,
    receiver: mpsc::Receiver<DecodeResult>,
    sender: mpsc::Sender<DecodeResult>,
    pending: std::collections::HashSet<PathBuf>,
}

impl ImageCache {
    pub fn new(max_size: usize) -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            entries: HashMap::new(),
            lru_order: VecDeque::new(),
            max_size,
            receiver,
            sender,
            pending: std::collections::HashSet::new(),
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.lru_order.clear();
        self.pending.clear();
        while self.receiver.try_recv().is_ok() {}
    }

    pub fn get(&mut self, path: &Path) -> Option<&egui::ColorImage> {
        if self.entries.contains_key(path) {
            self.lru_order.retain(|p| p != path);
            self.lru_order.push_back(path.to_path_buf());
            self.entries.get(path)
        } else {
            None
        }
    }

    pub fn insert(&mut self, path: PathBuf, pixels: egui::ColorImage) {
        if self.entries.contains_key(&path) {
            self.lru_order.retain(|p| p != &path);
        }

        while self.entries.len() >= self.max_size {
            if let Some(oldest) = self.lru_order.pop_front() {
                self.entries.remove(&oldest);
            } else {
                break;
            }
        }

        self.lru_order.push_back(path.clone());
        self.entries.insert(path.clone(), pixels);
        self.pending.remove(&path);
    }

    pub fn preload(&mut self, paths: Vec<PathBuf>) {
        for path in paths {
            if self.entries.contains_key(&path) || self.pending.contains(&path) {
                continue;
            }
            if crate::file_list::is_video_file(&path) {
                continue;
            }

            self.pending.insert(path.clone());
            let sender = self.sender.clone();
            let path_clone = path.clone();

            thread::spawn(move || {
                let result = match DecodedImage::load(&path_clone) {
                    Ok(decoded) => Ok(decoded.pixels),
                    Err(e) => Err(e),
                };
                let _ = sender.send(DecodeResult {
                    path: path_clone,
                    result,
                });
            });
        }
    }

    pub fn poll(&mut self) {
        while let Ok(decode_result) = self.receiver.try_recv() {
            self.pending.remove(&decode_result.path);
            match decode_result.result {
                Ok(pixels) => {
                    self.insert(decode_result.path, pixels);
                }
                Err(e) => {
                    log::warn!("Preload failed for {}: {}", decode_result.path.display(), e);
                }
            }
        }
    }
}
