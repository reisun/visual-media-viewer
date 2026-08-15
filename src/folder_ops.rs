use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderDeleteRequest {
    pub target_dir: PathBuf,
    pub next_dir: PathBuf,
}

impl FolderDeleteRequest {
    pub fn target_name(&self) -> String {
        self.target_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| self.target_dir.display().to_string())
    }
}

pub fn prepare_folder_delete_request(
    current_file: &Path,
    next_dir: Option<PathBuf>,
) -> Result<FolderDeleteRequest, String> {
    let target_dir = current_file
        .parent()
        .ok_or_else(|| "現在のファイルの親フォルダを特定できません。".to_string())?;
    let target_dir = std::fs::canonicalize(target_dir)
        .map_err(|e| format!("削除対象フォルダを確認できません: {e}"))?;
    validate_delete_target(&target_dir)?;

    let next_dir = next_dir.ok_or_else(|| {
        "次に表示するフォルダが見つからないため、このフォルダは削除できません。".to_string()
    })?;
    let next_dir = std::fs::canonicalize(&next_dir)
        .map_err(|e| format!("削除後の移動先フォルダを確認できません: {e}"))?;
    if !next_dir.is_dir() {
        return Err("削除後の移動先がフォルダではありません。".to_string());
    }
    if is_same_or_descendant(&next_dir, &target_dir) {
        return Err(
            "削除後の移動先も同じフォルダ配下になるため、このフォルダは削除できません。"
                .to_string(),
        );
    }

    Ok(FolderDeleteRequest {
        target_dir,
        next_dir,
    })
}

fn validate_delete_target(target_dir: &Path) -> Result<(), String> {
    if !target_dir.exists() {
        return Err("削除対象フォルダが存在しません。".to_string());
    }
    if !target_dir.is_dir() {
        return Err("削除対象がフォルダではありません。".to_string());
    }
    let parent = target_dir
        .parent()
        .ok_or_else(|| "ファイルシステムのルートは削除できません。".to_string())?;
    if parent == target_dir {
        return Err("ファイルシステムのルートは削除できません。".to_string());
    }
    Ok(())
}

fn is_same_or_descendant(path: &Path, base: &Path) -> bool {
    path == base || path.starts_with(base)
}

fn strip_verbatim_prefix_wide(path: &[u16]) -> Vec<u16> {
    const VERBATIM_PREFIX: &[u16] = &[b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];
    const VERBATIM_UNC_PREFIX: &[u16] = &[
        b'\\' as u16,
        b'\\' as u16,
        b'?' as u16,
        b'\\' as u16,
        b'U' as u16,
        b'N' as u16,
        b'C' as u16,
        b'\\' as u16,
    ];

    if let Some(rest) = path.strip_prefix(VERBATIM_UNC_PREFIX) {
        let mut shell_path = vec![b'\\' as u16, b'\\' as u16];
        shell_path.extend_from_slice(rest);
        shell_path
    } else if let Some(rest) = path.strip_prefix(VERBATIM_PREFIX) {
        rest.to_vec()
    } else {
        path.to_vec()
    }
}

#[cfg(target_os = "windows")]
pub fn move_folder_to_recycle_bin(target_dir: &Path) -> Result<(), String> {
    validate_delete_target(target_dir)?;
    move_folder_to_recycle_bin_windows(target_dir)
}

#[cfg(not(target_os = "windows"))]
pub fn move_folder_to_recycle_bin(target_dir: &Path) -> Result<(), String> {
    validate_delete_target(target_dir)?;
    Err("フォルダのごみ箱移動は Windows でのみサポートされています。".to_string())
}

#[cfg(target_os = "windows")]
fn move_folder_to_recycle_bin_windows(target_dir: &Path) -> Result<(), String> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;

    const FO_DELETE: u32 = 0x0003;
    const FOF_SILENT: u16 = 0x0004;
    const FOF_NOCONFIRMATION: u16 = 0x0010;
    const FOF_ALLOWUNDO: u16 = 0x0040;
    const FOF_NOERRORUI: u16 = 0x0400;

    #[repr(C)]
    struct ShFileOpStructW {
        hwnd: *mut c_void,
        w_func: u32,
        p_from: *const u16,
        p_to: *const u16,
        f_flags: u16,
        f_any_operations_aborted: i32,
        h_name_mappings: *mut c_void,
        lpsz_progress_title: *const u16,
    }

    #[link(name = "shell32")]
    extern "system" {
        fn SHFileOperationW(file_op: *mut ShFileOpStructW) -> i32;
    }

    // `canonicalize` returns a verbatim (`\\?\`) path on Windows, but
    // SHFileOperation rejects that prefix. Keep the path fully qualified while
    // converting it back to the Win32 shell form immediately before the call.
    let encoded_path: Vec<u16> = target_dir.as_os_str().encode_wide().collect();
    let mut wide_path = strip_verbatim_prefix_wide(&encoded_path);
    wide_path.push(0);
    wide_path.push(0);

    let mut operation = ShFileOpStructW {
        hwnd: std::ptr::null_mut(),
        w_func: FO_DELETE,
        p_from: wide_path.as_ptr(),
        p_to: std::ptr::null(),
        f_flags: FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT,
        f_any_operations_aborted: 0,
        h_name_mappings: std::ptr::null_mut(),
        lpsz_progress_title: std::ptr::null(),
    };

    let result = unsafe { SHFileOperationW(&mut operation) };
    if result != 0 {
        return Err(format!(
            "フォルダをごみ箱へ移動できませんでした (code {result})。"
        ));
    }
    if operation.f_any_operations_aborted != 0 {
        return Err("フォルダ削除がキャンセルされました。".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_ID: AtomicU64 = AtomicU64::new(1);

    fn unique_temp_dir(label: &str) -> PathBuf {
        let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("vmv-folder-ops-{label}-{id}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, b"test").unwrap();
    }

    #[test]
    fn test_prepare_folder_delete_request_captures_target_and_next_dir() {
        let root = unique_temp_dir("request");
        let current_file = root.join("alpha/current/file.jpg");
        let next_dir = root.join("beta/next");
        touch(&current_file);
        touch(&next_dir.join("next.jpg"));

        let request = prepare_folder_delete_request(&current_file, Some(next_dir.clone())).unwrap();

        assert_eq!(request.target_dir, root.join("alpha/current"));
        assert_eq!(request.next_dir, next_dir);
        assert_eq!(request.target_name(), "current");
    }

    #[test]
    fn test_prepare_folder_delete_request_rejects_missing_next_dir() {
        let root = unique_temp_dir("missing-next");
        let current_file = root.join("alpha/current/file.jpg");
        touch(&current_file);

        let err = prepare_folder_delete_request(&current_file, None).unwrap_err();

        assert!(err.contains("次に表示するフォルダが見つからない"));
    }

    #[test]
    fn test_prepare_folder_delete_request_rejects_next_dir_inside_target() {
        let root = unique_temp_dir("nested-next");
        let current_file = root.join("alpha/current/file.jpg");
        let nested_next = root.join("alpha/current/child");
        touch(&current_file);
        touch(&nested_next.join("next.jpg"));

        let err =
            prepare_folder_delete_request(&current_file, Some(nested_next.clone())).unwrap_err();

        assert!(err.contains("移動先も同じフォルダ配下"));
    }

    #[test]
    fn test_move_folder_to_recycle_bin_rejects_root_path() {
        let root = if cfg!(windows) {
            PathBuf::from(r"C:\")
        } else {
            PathBuf::from("/")
        };

        let err = move_folder_to_recycle_bin(&root).unwrap_err();
        assert!(err.contains("ルート"));
    }

    #[test]
    fn test_strip_verbatim_drive_prefix_for_shell_api() {
        let input: Vec<u16> = r"\\?\C:\gallery\set01".encode_utf16().collect();
        let expected: Vec<u16> = r"C:\gallery\set01".encode_utf16().collect();

        assert_eq!(strip_verbatim_prefix_wide(&input), expected);
    }

    #[test]
    fn test_strip_verbatim_unc_prefix_for_shell_api() {
        let input: Vec<u16> = r"\\?\UNC\server\share\set01".encode_utf16().collect();
        let expected: Vec<u16> = r"\\server\share\set01".encode_utf16().collect();

        assert_eq!(strip_verbatim_prefix_wide(&input), expected);
    }

    #[test]
    fn test_preserve_regular_shell_path() {
        let input: Vec<u16> = r"C:\gallery\set01".encode_utf16().collect();

        assert_eq!(strip_verbatim_prefix_wide(&input), input);
    }
}
