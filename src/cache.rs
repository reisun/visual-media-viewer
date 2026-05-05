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

fn image_bytes(img: &egui::ColorImage) -> usize {
    img.pixels.len() * 4
}

pub struct ImageCache {
    entries: HashMap<PathBuf, egui::ColorImage>,
    lru_order: VecDeque<PathBuf>,
    total_bytes: usize,
    max_bytes: usize,
    receiver: mpsc::Receiver<DecodeResult>,
    sender: mpsc::Sender<DecodeResult>,
    pending: std::collections::HashSet<PathBuf>,
    max_texture_dim: u32,
}

impl ImageCache {
    pub fn new(max_bytes: usize, max_texture_dim: u32) -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            entries: HashMap::new(),
            lru_order: VecDeque::new(),
            total_bytes: 0,
            max_bytes,
            receiver,
            sender,
            pending: std::collections::HashSet::new(),
            max_texture_dim,
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.lru_order.clear();
        self.total_bytes = 0;
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
        let new_bytes = image_bytes(&pixels);

        if let Some(old) = self.entries.remove(&path) {
            self.total_bytes -= image_bytes(&old);
            self.lru_order.retain(|p| p != &path);
        }

        while self.total_bytes + new_bytes > self.max_bytes {
            if let Some(oldest) = self.lru_order.pop_front() {
                if let Some(evicted) = self.entries.remove(&oldest) {
                    self.total_bytes -= image_bytes(&evicted);
                }
            } else {
                break;
            }
        }

        self.total_bytes += new_bytes;
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
            let max_texture_dim = self.max_texture_dim;

            thread::spawn(move || {
                let result = match DecodedImage::load(&path_clone, max_texture_dim) {
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
