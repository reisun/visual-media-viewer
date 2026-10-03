use eframe::egui;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use crate::image_decode::DecodedImage;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Request {
    path: PathBuf,
    preview_dim: Option<u32>,
    generation: u64,
}

struct DecodeResult {
    request: Request,
    result: Result<DecodedImage, String>,
}

struct Entry {
    image: DecodedImage,
    preview_dim: Option<u32>,
}

fn image_bytes(img: &egui::ColorImage) -> usize {
    img.pixels.len() * 4
}

pub struct ImageCache {
    entries: HashMap<PathBuf, Entry>,
    lru_order: VecDeque<PathBuf>,
    total_bytes: usize,
    max_bytes: usize,
    receiver: mpsc::Receiver<DecodeResult>,
    sender: mpsc::Sender<DecodeResult>,
    active: Option<Request>,
    current: Option<Request>,
    queue: VecDeque<Request>,
    errors: HashMap<PathBuf, String>,
    generation: u64,
    max_texture_dim: u32,
    preload_dim: Option<u32>,
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
            active: None,
            current: None,
            queue: VecDeque::new(),
            errors: HashMap::new(),
            generation: 0,
            max_texture_dim,
            preload_dim: None,
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.lru_order.clear();
        self.total_bytes = 0;
        self.errors.clear();
        self.generation = self.generation.wrapping_add(1);
        self.cancel_current();
        // Keep the active slot until its completion arrives, even across folders.
    }

    pub fn cancel_current(&mut self) {
        self.current = None;
        self.queue.clear();
    }

    pub fn get(&mut self, path: &Path) -> Option<&egui::ColorImage> {
        if self.entries.contains_key(path) {
            self.lru_order.retain(|p| p != path);
            self.lru_order.push_back(path.to_path_buf());
        }
        self.entries.get(path).map(|entry| &entry.image.pixels)
    }

    pub fn original_size(&self, path: &Path) -> Option<[usize; 2]> {
        self.entries
            .get(path)
            .map(|entry| entry.image.original_size)
    }

    pub fn is_preview(&self, path: &Path) -> bool {
        self.entries
            .get(path)
            .is_some_and(|entry| entry.preview_dim.is_some())
    }

    pub fn error(&self, path: &Path) -> Option<&str> {
        self.errors.get(path).map(String::as_str)
    }

    pub fn is_loading(&self) -> bool {
        self.active.is_some() || self.current.as_ref().is_some_and(|r| self.needed(r))
    }

    fn needed(&self, request: &Request) -> bool {
        if self.errors.contains_key(&request.path) {
            return false;
        }
        match self.entries.get(&request.path) {
            None => true,
            Some(entry) => match (entry.preview_dim, request.preview_dim) {
                (None, _) => false,
                (Some(_), None) => true,
                (Some(have), Some(want)) => have < want,
            },
        }
    }

    pub fn request(&mut self, path: PathBuf, preview_dim: Option<u32>) {
        if let Some(dim) = preview_dim {
            self.preload_dim = Some(dim.max(1).min(self.max_texture_dim));
        }
        if self.current.as_ref().is_none_or(|r| r.path != path) {
            self.queue.clear();
            self.errors.remove(&path);
        }
        self.current = Some(Request {
            path,
            preview_dim: preview_dim.map(|dim| dim.max(1).min(self.max_texture_dim)),
            generation: self.generation,
        });
        self.start_next();
    }

    pub fn preload(&mut self, paths: Vec<PathBuf>) {
        self.queue.clear();
        let preview_dim = Some(self.preload_dim.unwrap_or(2048).min(self.max_texture_dim));
        for path in paths {
            if self.queue.len() >= 8 {
                break;
            }
            if crate::file_list::is_video_file(&path)
                || self.entries.contains_key(&path)
                || self.active.as_ref().is_some_and(|r| r.path == path)
                || self.current.as_ref().is_some_and(|r| r.path == path)
                || self.queue.iter().any(|r| r.path == path)
            {
                continue;
            }
            self.queue.push_back(Request {
                path,
                preview_dim,
                generation: self.generation,
            });
        }
        self.start_next();
    }

    fn start_next(&mut self) {
        if self.active.is_some() {
            return;
        }
        let mut next = self.current.as_ref().filter(|r| self.needed(r)).cloned();
        while next.is_none() && self.total_bytes < self.max_bytes {
            let Some(candidate) = self.queue.pop_front() else {
                break;
            };
            if self.needed(&candidate) {
                next = Some(candidate);
            }
        }
        let Some(request) = next else { return };
        self.active = Some(request.clone());
        let sender = self.sender.clone();
        let max_texture_dim = self.max_texture_dim;
        thread::spawn(move || {
            let result = match request.preview_dim {
                Some(dim) => DecodedImage::load_preview(&request.path, dim, max_texture_dim),
                None => DecodedImage::load(&request.path, max_texture_dim),
            };
            let _ = sender.send(DecodeResult { request, result });
        });
    }

    fn insert(&mut self, request: &Request, image: DecodedImage) -> bool {
        let new_bytes = image_bytes(&image.pixels);
        let is_current = self
            .current
            .as_ref()
            .is_some_and(|r| r.path == request.path);
        let pinned_bytes = self
            .current
            .as_ref()
            .and_then(|r| self.entries.get(&r.path))
            .map_or(0, |entry| image_bytes(&entry.image.pixels));
        if !is_current && pinned_bytes.saturating_add(new_bytes) > self.max_bytes {
            return false;
        }
        if let Some(old) = self.entries.remove(&request.path) {
            self.total_bytes -= image_bytes(&old.image.pixels);
            self.lru_order.retain(|p| p != &request.path);
        }
        while self.total_bytes.saturating_add(new_bytes) > self.max_bytes {
            let Some(index) = self
                .lru_order
                .iter()
                .position(|path| self.current.as_ref().is_none_or(|r| &r.path != path))
            else {
                break;
            };
            let oldest = self.lru_order.remove(index).unwrap();
            if let Some(old) = self.entries.remove(&oldest) {
                self.total_bytes -= image_bytes(&old.image.pixels);
            }
        }
        let preview_dim = request
            .preview_dim
            .filter(|_| image.pixels.size != image.original_size);
        self.total_bytes += new_bytes;
        self.lru_order.push_back(request.path.clone());
        self.entries
            .insert(request.path.clone(), Entry { image, preview_dim });
        true
    }

    pub fn poll(&mut self) -> Vec<PathBuf> {
        let mut changed = Vec::new();
        while let Ok(completion) = self.receiver.try_recv() {
            self.active = None;
            if completion.request.generation != self.generation {
                continue;
            }
            match completion.result {
                Ok(image) => {
                    if self.insert(&completion.request, image) {
                        changed.push(completion.request.path);
                    }
                }
                Err(error) => {
                    log::warn!(
                        "Decode failed for {}: {}",
                        completion.request.path.display(),
                        error
                    );
                    self.errors.insert(completion.request.path, error);
                }
            }
        }
        self.start_next();
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(path: &str, preview_dim: Option<u32>) -> Request {
        Request {
            path: path.into(),
            preview_dim,
            generation: 0,
        }
    }

    fn image(side: usize, original: usize) -> DecodedImage {
        DecodedImage {
            pixels: egui::ColorImage::from_rgba_unmultiplied(
                [side, side],
                &vec![255; side * side * 4],
            ),
            original_size: [original, original],
        }
    }

    #[test]
    fn full_upgrade_replaces_preview_and_updates_budget() {
        let mut cache = ImageCache::new(1024, 8192);
        let preview = request("image.jpg", Some(2));
        cache.current = Some(preview.clone());
        assert!(cache.insert(&preview, image(2, 4)));
        assert!(cache.is_preview(&preview.path));
        assert!(!cache.needed(&preview));
        let full = request("image.jpg", None);
        assert!(cache.needed(&full));
        assert!(cache.insert(&full, image(4, 4)));
        assert!(!cache.is_preview(&full.path));
        assert!(!cache.needed(&preview));
        assert_eq!(cache.total_bytes, 64);
        assert_eq!(cache.original_size(&full.path), Some([4, 4]));
    }

    #[test]
    fn eviction_preserves_current_even_when_it_is_oldest() {
        let mut cache = ImageCache::new(32, 8192);
        let current = request("current", None);
        cache.current = Some(current.clone());
        cache.insert(&current, image(2, 2));
        cache.insert(&request("old", None), image(2, 2));
        cache.insert(&request("new", None), image(2, 2));
        assert!(cache.get(&current.path).is_some());
        assert!(cache.get(Path::new("old")).is_none());
        assert!(cache.get(Path::new("new")).is_some());
        assert!(!cache.insert(&request("large", None), image(3, 3)));
        assert_eq!(cache.total_bytes, 32);
    }

    #[test]
    fn clear_discards_inflight_completion_without_opening_another_slot() {
        let mut cache = ImageCache::new(1024, 8192);
        let old = request("old", None);
        cache.active = Some(old.clone());
        cache.clear();
        assert!(cache.is_loading());
        cache
            .sender
            .send(DecodeResult {
                request: old,
                result: Ok(image(2, 2)),
            })
            .unwrap();
        assert!(cache.poll().is_empty());
        assert!(cache.entries.is_empty());
        assert!(!cache.is_loading());
    }

    #[test]
    fn navigation_replaces_pending_work_and_keeps_one_active_decode() {
        let mut cache = ImageCache::new(1024, 8192);
        let active = request("busy", None);
        cache.active = Some(active.clone());
        cache.request("first.jpg".into(), Some(200));
        cache.preload((0..20).map(|i| PathBuf::from(format!("{i}.jpg"))).collect());
        assert_eq!(cache.queue.len(), 8);
        cache.request("latest.jpg".into(), None);
        assert!(cache.queue.is_empty());
        assert_eq!(cache.active, Some(active));
        assert_eq!(
            cache.current.as_ref().unwrap().path,
            PathBuf::from("latest.jpg")
        );
        assert_eq!(cache.preload_dim, Some(200));
    }

    #[test]
    fn failed_decode_does_not_retry_each_frame() {
        let mut cache = ImageCache::new(1024, 8192);
        let failed = request("broken", None);
        cache.current = Some(failed.clone());
        cache.active = Some(failed.clone());
        cache
            .sender
            .send(DecodeResult {
                request: failed.clone(),
                result: Err("invalid image".into()),
            })
            .unwrap();
        assert!(cache.poll().is_empty());
        cache.request(failed.path.clone(), None);
        assert!(!cache.is_loading());
        assert_eq!(cache.error(&failed.path), Some("invalid image"));
    }

    #[test]
    fn old_folder_error_is_discarded() {
        let mut cache = ImageCache::new(1024, 8192);
        let old = request("same-path", None);
        cache.active = Some(old.clone());
        cache.clear();
        cache
            .sender
            .send(DecodeResult {
                request: old.clone(),
                result: Err("old error".into()),
            })
            .unwrap();
        cache.poll();
        assert!(cache.error(&old.path).is_none());
    }

    #[test]
    fn repeated_request_does_not_duplicate_or_downgrade_completed_work() {
        let mut cache = ImageCache::new(1024, 8192);
        let active = request("current.jpg", Some(200));
        cache.active = Some(active.clone());
        for _ in 0..5 {
            cache.request(active.path.clone(), Some(200));
        }
        assert_eq!(cache.active, Some(active.clone()));
        assert!(cache.queue.is_empty());
        cache
            .sender
            .send(DecodeResult {
                request: active.clone(),
                result: Ok(image(2, 2)),
            })
            .unwrap();
        assert_eq!(cache.poll(), vec![active.path.clone()]);
        cache.request(active.path, None);
        assert!(!cache.is_loading());
    }

    #[test]
    fn current_request_takes_priority_over_queued_neighbors() {
        let mut cache = ImageCache::new(1024, 8192);
        let busy = request("busy", None);
        cache.active = Some(busy.clone());
        cache.request("latest.jpg".into(), None);
        cache.preload(vec!["neighbor.jpg".into()]);
        assert_eq!(cache.queue.front().unwrap().preview_dim, Some(2048));
        cache
            .sender
            .send(DecodeResult {
                request: busy,
                result: Ok(image(2, 2)),
            })
            .unwrap();
        cache.poll();
        assert_eq!(
            cache.active.as_ref().unwrap().path,
            PathBuf::from("latest.jpg")
        );
        assert_eq!(cache.queue.len(), 1);
    }
}
