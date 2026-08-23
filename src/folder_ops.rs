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

fn shell_compatible_wide_path(path: &[u16]) -> Vec<u16> {
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
    let target_dir = target_dir.to_path_buf();
    std::thread::spawn(move || move_folder_to_recycle_bin_sta(&target_dir))
        .join()
        .map_err(|_| "ごみ箱処理中に予期しないエラーが発生しました。".to_string())?
}

#[cfg(target_os = "windows")]
fn move_folder_to_recycle_bin_sta(target_dir: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{
        FileOperation, IFileOperation, IShellItem, SHCreateItemFromParsingName,
        FOFX_RECYCLEONDELETE, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT,
    };

    struct ComGuard;

    impl Drop for ComGuard {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }

    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
        .ok()
        .map_err(|e| format!("ごみ箱処理を初期化できませんでした: {e}"))?;
    let _com_guard = ComGuard;

    // `canonicalize` returns a verbatim (`\\?\`) path on Windows. Shell item
    // parsing rejects that prefix, so convert only the parsing input back to a
    // regular absolute Win32 path. Keep `target_dir` canonical for validation.
    let encoded_path: Vec<u16> = target_dir.as_os_str().encode_wide().collect();
    let mut wide_path = shell_compatible_wide_path(&encoded_path);
    wide_path.push(0);

    let item: IShellItem = unsafe {
        SHCreateItemFromParsingName(PCWSTR(wide_path.as_ptr()), None)
            .map_err(|e| format!("削除対象フォルダをシェルで開けませんでした: {e}"))?
    };
    let operation: IFileOperation = unsafe {
        CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| format!("ごみ箱処理を開始できませんでした: {e}"))?
    };
    unsafe {
        operation
            .SetOperationFlags(
                FOFX_RECYCLEONDELETE | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT,
            )
            .map_err(|e| format!("ごみ箱処理を設定できませんでした: {e}"))?;
        operation
            .DeleteItem(&item, None)
            .map_err(|e| format!("フォルダの削除を予約できませんでした: {e}"))?;
        operation
            .PerformOperations()
            .map_err(|e| format!("フォルダをごみ箱へ移動できませんでした: {e}"))?;
    }
    let aborted = unsafe { operation.GetAnyOperationsAborted() }
        .map_err(|e| format!("ごみ箱処理の完了状態を確認できませんでした: {e}"))?;
    if aborted.as_bool() {
        return Err("フォルダ削除がキャンセルされました。".to_string());
    }
    if target_dir.exists() {
        return Err("フォルダをごみ箱へ移動できませんでした。削除対象が残っています。".to_string());
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
    fn test_shell_path_removes_verbatim_drive_prefix() {
        let input: Vec<u16> = r"\\?\D:\gallery\set01".encode_utf16().collect();
        let expected: Vec<u16> = r"D:\gallery\set01".encode_utf16().collect();

        assert_eq!(shell_compatible_wide_path(&input), expected);
    }

    #[test]
    fn test_shell_path_converts_verbatim_unc_prefix() {
        let input: Vec<u16> = r"\\?\UNC\server\share\set01".encode_utf16().collect();
        let expected: Vec<u16> = r"\\server\share\set01".encode_utf16().collect();

        assert_eq!(shell_compatible_wide_path(&input), expected);
    }

    #[test]
    fn test_shell_path_preserves_regular_absolute_path() {
        let input: Vec<u16> = r"D:\gallery\set01".encode_utf16().collect();

        assert_eq!(shell_compatible_wide_path(&input), input);
    }
}
