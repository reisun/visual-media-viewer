use std::cmp::Ordering;
use std::path::{Path, PathBuf};

/// Supported image file extensions (lowercase).
const SUPPORTED_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif", "bmp", "webp", "tiff", "tif"];

/// A segment of a filename for natural sort comparison.
#[derive(Debug, PartialEq, Eq)]
enum SortSegment {
    Text(String),
    Number(u64),
}

/// Parse a string into alternating text and number segments for natural sorting.
fn natural_sort_segments(s: &str) -> Vec<SortSegment> {
    let mut segments = Vec::new();
    let mut chars = s.chars().peekable();

    while chars.peek().is_some() {
        if chars.peek().map_or(false, |c| c.is_ascii_digit()) {
            let mut num_str = String::new();
            while chars.peek().map_or(false, |c| c.is_ascii_digit()) {
                num_str.push(chars.next().unwrap());
            }
            segments.push(SortSegment::Number(num_str.parse().unwrap_or(0)));
        } else {
            let mut text = String::new();
            while chars.peek().map_or(false, |c| !c.is_ascii_digit()) {
                text.push(chars.next().unwrap());
            }
            segments.push(SortSegment::Text(text.to_lowercase()));
        }
    }

    segments
}

/// Compare two filenames using natural sort order (numbers compared numerically).
fn natural_sort_cmp(a: &str, b: &str) -> Ordering {
    let a_segs = natural_sort_segments(a);
    let b_segs = natural_sort_segments(b);

    for (a_seg, b_seg) in a_segs.iter().zip(b_segs.iter()) {
        let ord = match (a_seg, b_seg) {
            (SortSegment::Number(an), SortSegment::Number(bn)) => an.cmp(bn),
            (SortSegment::Text(at), SortSegment::Text(bt)) => at.cmp(bt),
            (SortSegment::Number(_), SortSegment::Text(_)) => Ordering::Less,
            (SortSegment::Text(_), SortSegment::Number(_)) => Ordering::Greater,
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }

    a_segs.len().cmp(&b_segs.len())
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

/// Manages a sorted list of image files in a directory for navigation.
pub struct FileList {
    files: Vec<PathBuf>,
    current_index: usize,
}

impl FileList {
    /// Scan a directory for supported image files and sort them by name.
    pub fn from_directory(dir: &Path) -> Self {
        let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
            Ok(entries) => entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|path| path.is_file() && is_supported_image(path))
                .collect(),
            Err(e) => {
                log::warn!("Failed to read directory {}: {}", dir.display(), e);
                Vec::new()
            }
        };

        // Sort by filename using natural sort order (numbers compared numerically).
        files.sort_by(|a, b| {
            let a_name = a.file_name().unwrap_or_default().to_string_lossy();
            let b_name = b.file_name().unwrap_or_default().to_string_lossy();
            natural_sort_cmp(&a_name, &b_name)
        });

        Self {
            files,
            current_index: 0,
        }
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
        names.sort_by(|a, b| natural_sort_cmp(a, b));
        assert_eq!(names, vec!["file1.jpg", "file2.jpg", "file10.jpg", "file20.jpg"]);
    }

    #[test]
    fn test_natural_sort_mixed_case() {
        let mut names = vec!["Photo10.jpg", "photo2.jpg", "PHOTO1.jpg"];
        names.sort_by(|a, b| natural_sort_cmp(a, b));
        assert_eq!(names, vec!["PHOTO1.jpg", "photo2.jpg", "Photo10.jpg"]);
    }

    #[test]
    fn test_natural_sort_parenthesized_numbers() {
        let mut names = vec![
            "image(10).jpg",
            "image(2).jpg",
            "image(1).jpg",
        ];
        names.sort_by(|a, b| natural_sort_cmp(a, b));
        assert_eq!(
            names,
            vec!["image(1).jpg", "image(2).jpg", "image(10).jpg"]
        );
    }

    #[test]
    fn test_natural_sort_no_numbers() {
        let mut names = vec!["cherry.png", "apple.png", "banana.png"];
        names.sort_by(|a, b| natural_sort_cmp(a, b));
        assert_eq!(names, vec!["apple.png", "banana.png", "cherry.png"]);
    }

    #[test]
    fn test_natural_sort_multiple_number_groups() {
        let mut names = vec!["vol2ch10.jpg", "vol2ch2.jpg", "vol1ch10.jpg"];
        names.sort_by(|a, b| natural_sort_cmp(a, b));
        assert_eq!(names, vec!["vol1ch10.jpg", "vol2ch2.jpg", "vol2ch10.jpg"]);
    }
}
