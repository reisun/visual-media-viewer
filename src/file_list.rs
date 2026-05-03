use std::path::{Path, PathBuf};
use std::time::SystemTime;

const SUPPORTED_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif", "bmp", "webp", "tiff", "tif"];

#[cfg(target_os = "windows")]
mod win_sort {
    use std::cmp::Ordering;

    #[link(name = "shlwapi")]
    extern "system" {
        fn StrCmpLogicalW(psz1: *const u16, psz2: *const u16) -> i32;
    }

    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn compare_logical(a: &str, b: &str) -> Ordering {
        let wa = to_wide(a);
        let wb = to_wide(b);
        let result = unsafe { StrCmpLogicalW(wa.as_ptr(), wb.as_ptr()) };
        result.cmp(&0)
    }
}

#[cfg(not(target_os = "windows"))]
mod win_sort {
    use std::cmp::Ordering;

    pub fn compare_logical(a: &str, b: &str) -> Ordering {
        natord::compare_ignore_case(a, b)
    }
}

/// Returns true if the path has a supported image extension (case-insensitive).
fn is_supported_image(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            let lower = ext.to_ascii_lowercase();
            SUPPORTED_EXTENSIONS.contains(&lower.as_str())
        })
        .unwrap_or(false)
}

/// Returns true if a directory contains at least one supported image file.
pub fn directory_has_images(dir: &Path) -> bool {
    match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .any(|e| {
                let p = e.path();
                p.is_file() && is_supported_image(&p)
            }),
        Err(_) => false,
    }
}

/// Sort key for file list ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    ModifiedDate,
}

/// Sort order (ascending or descending).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    Ascending,
    Descending,
}

/// Grouping mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy {
    Off,
    ModifiedDate,
}

/// Date group categories (ordered from newest to oldest).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum DateGroup {
    Today = 0,
    LastWeek = 1,
    LastMonth = 2,
    ThisYear = 3,
    LongAgo = 4,
}

fn days_since_epoch(time: SystemTime) -> i64 {
    match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => (d.as_secs() / 86400) as i64,
        Err(_) => 0,
    }
}

fn classify_date_group(file_time: SystemTime) -> DateGroup {
    let now_days = days_since_epoch(SystemTime::now());
    let file_days = days_since_epoch(file_time);
    let diff = now_days - file_days;

    if diff <= 0 {
        DateGroup::Today
    } else if diff <= 7 {
        DateGroup::LastWeek
    } else if diff <= 30 {
        DateGroup::LastMonth
    } else if diff <= 365 {
        DateGroup::ThisYear
    } else {
        DateGroup::LongAgo
    }
}

/// Manages a sorted list of image files in a directory for navigation.
pub struct FileList {
    files: Vec<PathBuf>,
    current_index: usize,
    directory: PathBuf,
    pub sort_key: SortKey,
    pub sort_order: SortOrder,
    pub group_by: GroupBy,
    pub file_sort_key: SortKey,
    pub file_sort_order: SortOrder,
}

impl FileList {
    pub fn from_directory(dir: &Path) -> Self {
        let mut fl = Self {
            files: Vec::new(),
            current_index: 0,
            directory: dir.to_path_buf(),
            sort_key: SortKey::Name,
            sort_order: SortOrder::Ascending,
            group_by: GroupBy::Off,
            file_sort_key: SortKey::Name,
            file_sort_order: SortOrder::Ascending,
        };
        fl.scan_and_sort();
        fl
    }

    fn scan_and_sort(&mut self) {
        let mut files: Vec<PathBuf> = match std::fs::read_dir(&self.directory) {
            Ok(entries) => entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|path| path.is_file() && is_supported_image(path))
                .collect(),
            Err(e) => {
                log::warn!("Failed to read directory {}: {}", self.directory.display(), e);
                Vec::new()
            }
        };

        let sort_key = self.file_sort_key;
        let sort_order = self.file_sort_order;

        files.sort_by(|a, b| {
            let cmp = match sort_key {
                SortKey::Name => {
                    let a_name = a.file_name().unwrap_or_default().to_string_lossy();
                    let b_name = b.file_name().unwrap_or_default().to_string_lossy();
                    win_sort::compare_logical(&a_name, &b_name)
                }
                SortKey::ModifiedDate => {
                    let a_time = file_modified_time(a);
                    let b_time = file_modified_time(b);
                    a_time.cmp(&b_time)
                }
            };
            match sort_order {
                SortOrder::Ascending => cmp,
                SortOrder::Descending => cmp.reverse(),
            }
        });

        self.files = files;
    }

    pub fn re_sort_dirs(&mut self, key: SortKey, order: SortOrder) {
        self.sort_key = key;
        self.sort_order = order;
    }

    pub fn re_sort_files(&mut self, key: SortKey, order: SortOrder) {
        let current_file = self.current_path().map(|p| p.to_path_buf());
        self.file_sort_key = key;
        self.file_sort_order = order;
        self.scan_and_sort();
        if let Some(path) = current_file {
            self.set_current(&path);
        }
    }

    pub fn set_group_by(&mut self, group: GroupBy) {
        self.group_by = group;
    }

    /// Set the current index to the file matching the given path.
    pub fn set_current(&mut self, path: &Path) {
        if let Some(idx) = self.files.iter().position(|f| f == path) {
            self.current_index = idx;
        }
    }

    /// Get the current file path.
    pub fn current_path(&self) -> Option<&Path> {
        self.files.get(self.current_index).map(|p| p.as_path())
    }

    /// Get the directory this file list was built from.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Move to the next file (wraps around to first). Returns true if the index changed.
    pub fn next(&mut self) -> bool {
        if self.files.len() <= 1 {
            return false;
        }
        if self.current_index + 1 < self.files.len() {
            self.current_index += 1;
        } else {
            self.current_index = 0;
        }
        true
    }

    /// Move to the previous file (wraps around to last). Returns true if the index changed.
    pub fn prev(&mut self) -> bool {
        if self.files.len() <= 1 {
            return false;
        }
        if self.current_index > 0 {
            self.current_index -= 1;
        } else {
            self.current_index = self.files.len() - 1;
        }
        true
    }

    /// Get the total number of files.
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// Get the current index (0-based).
    pub fn current_index(&self) -> usize {
        self.current_index
    }

    /// Get paths of nearby files (within `range` of current index).
    /// Used for preloading.
    pub fn nearby_paths(&self, range: usize) -> Vec<PathBuf> {
        let start = self.current_index.saturating_sub(range);
        let end = (self.current_index + range + 1).min(self.files.len());
        self.files[start..end].to_vec()
    }

    pub fn next_image_dir(&self) -> Option<PathBuf> {
        let opts = DirSortOpts {
            sort_key: self.sort_key,
            sort_order: self.sort_order,
            group_by: self.group_by,
        };
        next_image_dir_from(&self.directory, &opts)
    }

    pub fn prev_image_dir(&self) -> Option<PathBuf> {
        let opts = DirSortOpts {
            sort_key: self.sort_key,
            sort_order: self.sort_order,
            group_by: self.group_by,
        };
        prev_image_dir_from(&self.directory, &opts)
    }
}

fn file_modified_time(path: &Path) -> SystemTime {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

struct DirSortOpts {
    sort_key: SortKey,
    sort_order: SortOrder,
    group_by: GroupBy,
}

fn sorted_child_dirs(dir: &Path, opts: &DirSortOpts) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect(),
        Err(_) => return Vec::new(),
    };
    dirs.sort_by(|a, b| {
        if opts.group_by == GroupBy::ModifiedDate {
            let a_group = classify_date_group(file_modified_time(a));
            let b_group = classify_date_group(file_modified_time(b));
            let group_cmp = match opts.sort_order {
                SortOrder::Ascending => a_group.cmp(&b_group),
                SortOrder::Descending => b_group.cmp(&a_group),
            };
            if group_cmp != std::cmp::Ordering::Equal {
                return group_cmp;
            }
        }
        let cmp = match opts.sort_key {
            SortKey::Name => {
                let a_name = a.file_name().unwrap_or_default().to_string_lossy();
                let b_name = b.file_name().unwrap_or_default().to_string_lossy();
                win_sort::compare_logical(&a_name, &b_name)
            }
            SortKey::ModifiedDate => {
                file_modified_time(a).cmp(&file_modified_time(b))
            }
        };
        match opts.sort_order {
            SortOrder::Ascending => cmp,
            SortOrder::Descending => cmp.reverse(),
        }
    });
    dirs
}

const MAX_DEPTH: usize = 10;

fn first_image_dir_under(dir: &Path, depth: usize, opts: &DirSortOpts) -> Option<PathBuf> {
    if depth >= MAX_DEPTH {
        return None;
    }
    for child in sorted_child_dirs(dir, opts) {
        if directory_has_images(&child) {
            return Some(child);
        }
        if let Some(desc) = first_image_dir_under(&child, depth + 1, opts) {
            return Some(desc);
        }
    }
    None
}

fn last_image_dir_in_subtree(dir: &Path, depth: usize, opts: &DirSortOpts) -> Option<PathBuf> {
    if depth >= MAX_DEPTH {
        if directory_has_images(dir) {
            return Some(dir.to_path_buf());
        }
        return None;
    }
    let children = sorted_child_dirs(dir, opts);
    for child in children.iter().rev() {
        if let Some(desc) = last_image_dir_in_subtree(child, depth + 1, opts) {
            return Some(desc);
        }
    }
    if directory_has_images(dir) {
        return Some(dir.to_path_buf());
    }
    None
}

fn next_image_dir_from(current: &Path, opts: &DirSortOpts) -> Option<PathBuf> {
    if let Some(child) = first_image_dir_under(current, 0, opts) {
        return Some(child);
    }
    let mut dir = current.to_path_buf();
    for _ in 0..MAX_DEPTH {
        let parent = dir.parent()?;
        if parent == dir {
            return None;
        }
        let siblings = sorted_child_dirs(parent, opts);
        if let Some(idx) = siblings.iter().position(|d| d == &dir) {
            for sibling in &siblings[idx + 1..] {
                if directory_has_images(sibling) {
                    return Some(sibling.clone());
                }
                if let Some(desc) = first_image_dir_under(sibling, 0, opts) {
                    return Some(desc);
                }
            }
        }
        dir = parent.to_path_buf();
    }
    None
}

fn prev_image_dir_from(current: &Path, opts: &DirSortOpts) -> Option<PathBuf> {
    let mut dir = current.to_path_buf();
    for _ in 0..MAX_DEPTH {
        let parent = dir.parent()?;
        if parent == dir {
            return None;
        }
        let siblings = sorted_child_dirs(parent, opts);
        if let Some(idx) = siblings.iter().position(|d| d == &dir) {
            for sibling in siblings[..idx].iter().rev() {
                if let Some(desc) = last_image_dir_in_subtree(sibling, 0, opts) {
                    return Some(desc);
                }
            }
        }
        if directory_has_images(parent) {
            return Some(parent.to_path_buf());
        }
        dir = parent.to_path_buf();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_supported_image() {
        assert!(is_supported_image(Path::new("photo.jpg")));
        assert!(is_supported_image(Path::new("photo.JPEG")));
        assert!(is_supported_image(Path::new("photo.Png")));
        assert!(is_supported_image(Path::new("photo.gif")));
        assert!(is_supported_image(Path::new("photo.bmp")));
        assert!(is_supported_image(Path::new("photo.webp")));
        assert!(is_supported_image(Path::new("photo.tiff")));
        assert!(is_supported_image(Path::new("photo.TIF")));
        assert!(!is_supported_image(Path::new("photo.txt")));
        assert!(!is_supported_image(Path::new("noext")));
    }

    #[test]
    fn test_natural_sort_basic_numbers() {
        let mut names = vec!["file10.jpg", "file2.jpg", "file1.jpg", "file20.jpg"];
        names.sort_by(|a, b| natord::compare_ignore_case(a, b));
        assert_eq!(names, vec!["file1.jpg", "file2.jpg", "file10.jpg", "file20.jpg"]);
    }

    #[test]
    fn test_natural_sort_parenthesized_numbers() {
        let mut names = vec![
            "image(10).jpg",
            "image(2).jpg",
            "image(1).jpg",
        ];
        names.sort_by(|a, b| natord::compare_ignore_case(a, b));
        assert_eq!(
            names,
            vec!["image(1).jpg", "image(2).jpg", "image(10).jpg"]
        );
    }

    #[test]
    fn test_natural_sort_multiple_number_groups() {
        let mut names = vec!["vol2ch10.jpg", "vol2ch2.jpg", "vol1ch10.jpg"];
        names.sort_by(|a, b| natord::compare_ignore_case(a, b));
        assert_eq!(names, vec!["vol1ch10.jpg", "vol2ch2.jpg", "vol2ch10.jpg"]);
    }
}
