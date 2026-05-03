use eframe::egui;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use crate::viewer::DecodedImage;

/// Message sent from background decode threads to the main thread.
struct DecodeResult {
    path: PathBuf,
    result: Result<egui::ColorImage, String>,
}

/// LRU image cache with background preloading.
pub struct ImageCache {
    /// Cached decoded images, keyed by path.
    entries: HashMap<PathBuf, egui::ColorImage>,
    /// LRU order: most recently used at the back.
    lru_order: VecDeque<PathBuf>,
    /// Maximum number of cached images.
    max_size: usize,
    /// Receiver for background decode results.
    receiver: mpsc::Receiver<DecodeResult>,
    /// Sender for background decode tasks (cloned per thread).
    sender: mpsc::Sender<DecodeResult>,
    /// Paths currently being loaded in background.
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

    /// Get a cached image, updating LRU order.
    pub fn get(&mut self, path: &Path) -> Option<&egui::ColorImage> {
        if self.entries.contains_key(path) {
            // Move to back of LRU.
            self.lru_order.retain(|p| p != path);
            self.lru_order.push_back(path.to_path_buf());
            self.entries.get(path)
        } else {
            None
        }
    }

    /// Insert an image into the cache, evicting old entries if necessary.
    pub fn insert(&mut self, path: PathBuf, pixels: egui::ColorImage) {
        // Remove if already present (to update LRU position).
        if self.entries.contains_key(&path) {
            self.lru_order.retain(|p| p != &path);
        }

        // Evict oldest entries if at capacity.
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

    /// Start background preloading of the given paths.
    /// Skips paths that are already cached or pending.
    pub fn preload(&mut self, paths: Vec<PathBuf>) {
        for path in paths {
            if self.entries.contains_key(&path) || self.pending.contains(&path) {
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

    /// Poll for completed background decode results.
    /// Call this each frame from the main thread.
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
