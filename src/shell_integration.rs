//! Optional, per-user Windows shell integration. Playback never depends on this module.
#[path = "shell_registration_model.rs"]
#[cfg(any(windows, test))]
mod model;

#[derive(Clone, Debug, Default)]
pub struct ExtensionStatus {
    pub extension: String,
    pub is_default: bool,
    /// Our per-user registry entry exists; this does not guarantee decoding succeeds.
    pub thumbnail_registered: bool,
    /// Windows resolves this extension's shell thumbnail handler to our CLSID.
    pub thumbnail_effective: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Status {
    pub app_registered: bool,
    pub app_partial: bool,
    pub app_update_available: bool,
    pub default_count: usize,
    pub total_count: usize,
    pub thumbnail_count: usize,
    pub thumbnail_effective_count: usize,
    pub thumbnails_enabled: bool,
    pub thumbnail_update_available: bool,
    pub thumbnails_partial: bool,
    pub extensions: Vec<ExtensionStatus>,
}

pub const EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "bmp", "webp", "tiff", "tif", "heic", "heif", "mp4", "webm",
    "mkv", "avi", "mov", "wmv", "flv", "m4v", "mpg", "mpeg", "ts",
];

#[cfg(windows)]
mod platform {
    use super::{model::*, ExtensionStatus, Status, EXTENSIONS};
    use std::{
        ffi::c_void,
        path::{Path, PathBuf},
        ptr,
    };

    const HKCU: isize = 0x80000001u32 as i32 as isize;
    const QUERY: u32 = 0x20019;
    const WRITE: u32 = 0x20006;
    const APP: &str = "VisualMediaViewer";
    const APP_KEY: &str = "Software\\VisualMediaViewer\\Shell";
    const CAPABILITIES: &str = "Software\\VisualMediaViewer\\Capabilities";
    const THUMBNAIL_SLOT: &str = "{E357FCCD-A995-4576-B01F-234630154E96}";
    // Shared with the thumbnail provider.
    const PROVIDER: &str = "{D4F66B4E-5762-4BA9-9633-138915FD01A7}";
    const PROVIDER_FILE: &str = "VisualMediaViewerThumbnail.dll";
    const DEPENDENCIES: &[&str] = &[
        "avcodec-61.dll",
        "avdevice-61.dll",
        "avfilter-10.dll",
        "avformat-61.dll",
        "avutil-59.dll",
        "swresample-5.dll",
        "swscale-8.dll",
    ];

    #[link(name = "advapi32")]
    extern "system" {
        fn RegOpenKeyExW(
            key: isize,
            path: *const u16,
            options: u32,
            access: u32,
            result: *mut isize,
        ) -> i32;
        fn RegCreateKeyExW(
            key: isize,
            path: *const u16,
            reserved: u32,
            class: *const u16,
            options: u32,
            access: u32,
            security: *const c_void,
            result: *mut isize,
            disposition: *mut u32,
        ) -> i32;
        fn RegCloseKey(key: isize) -> i32;
        fn RegQueryValueExW(
            key: isize,
            name: *const u16,
            reserved: *mut u32,
            kind: *mut u32,
            bytes: *mut u8,
            len: *mut u32,
        ) -> i32;
        fn RegSetValueExW(
            key: isize,
            name: *const u16,
            reserved: u32,
            kind: u32,
            bytes: *const u8,
            len: u32,
        ) -> i32;
        fn RegDeleteValueW(key: isize, name: *const u16) -> i32;
    }
    #[link(name = "shell32")]
    extern "system" {
        fn SHChangeNotify(event: i32, flags: u32, item1: *const c_void, item2: *const c_void);
        fn ShellExecuteW(
            window: isize,
            operation: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show: i32,
        ) -> isize;
    }
    #[link(name = "shlwapi")]
    extern "system" {
        fn AssocQueryStringW(
            flags: u32,
            kind: u32,
            association: *const u16,
            extra: *const u16,
            output: *mut u16,
            len: *mut u32,
        ) -> i32;
    }

    #[link(name = "ole32")]
    extern "system" {
        fn CoInitializeEx(reserved: *const c_void, flags: u32) -> i32;
        fn CoUninitialize();
    }

    struct ComApartment(bool);

    impl ComApartment {
        fn enter() -> Result<Self, String> {
            let status = unsafe { CoInitializeEx(ptr::null(), 2) };
            match status {
                0 | 1 => Ok(Self(true)),
                // A caller's existing apartment must remain unchanged.
                status if status == 0x80010106u32 as i32 => Ok(Self(false)),
                status => Err(format!("Windows shell initialization failed ({status:#x})")),
            }
        }
    }

    impl Drop for ComApartment {
        fn drop(&mut self) {
            if self.0 {
                unsafe {
                    CoUninitialize();
                }
            }
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }
    fn string(value: &str) -> RegistryValue {
        RegistryValue {
            kind: 1,
            bytes: wide(value).into_iter().flat_map(u16::to_le_bytes).collect(),
        }
    }
    fn decode(value: &RegistryValue) -> Result<String, String> {
        if !matches!(value.kind, 1 | 2) || value.bytes.len() % 2 != 0 {
            return Err("Windows registration contains an invalid string".into());
        }
        let units: Vec<u16> = value
            .bytes
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .take_while(|&u| u != 0)
            .collect();
        String::from_utf16(&units).map_err(|e| e.to_string())
    }
    struct Key(isize);
    impl Drop for Key {
        fn drop(&mut self) {
            unsafe {
                RegCloseKey(self.0);
            }
        }
    }
    fn key(path: &str, create: bool) -> Result<Option<Key>, String> {
        let mut result = 0;
        let path_w = wide(path);
        let error = unsafe {
            if create {
                RegCreateKeyExW(
                    HKCU,
                    path_w.as_ptr(),
                    0,
                    ptr::null(),
                    0,
                    QUERY | WRITE,
                    ptr::null(),
                    &mut result,
                    ptr::null_mut(),
                )
            } else {
                RegOpenKeyExW(HKCU, path_w.as_ptr(), 0, QUERY, &mut result)
            }
        };
        match error {
            0 => Ok(Some(Key(result))),
            2 | 3 if !create => Ok(None),
            _ => Err(format!("Windows registration access failed ({error})")),
        }
    }
    fn read(path: &str, name: &str) -> Result<Option<RegistryValue>, String> {
        let Some(key) = key(path, false)? else {
            return Ok(None);
        };
        let name = wide(name);
        let mut len = 0;
        let mut kind = 0;
        let error = unsafe {
            RegQueryValueExW(
                key.0,
                name.as_ptr(),
                ptr::null_mut(),
                &mut kind,
                ptr::null_mut(),
                &mut len,
            )
        };
        if error == 2 {
            return Ok(None);
        }
        if error != 0 {
            return Err(format!("Windows registration read failed ({error})"));
        }
        if len > 1024 * 1024 {
            return Err("Windows registration value is too large".into());
        }
        let mut bytes = vec![0; len as usize];
        let error = unsafe {
            RegQueryValueExW(
                key.0,
                name.as_ptr(),
                ptr::null_mut(),
                &mut kind,
                bytes.as_mut_ptr(),
                &mut len,
            )
        };
        if error != 0 {
            return Err(format!("Windows registration read failed ({error})"));
        }
        bytes.truncate(len as usize);
        Ok(Some(RegistryValue { kind, bytes }))
    }
    fn write(path: &str, name: &str, value: Option<&RegistryValue>) -> Result<(), String> {
        if value.is_none() && key(path, false)?.is_none() {
            return Ok(());
        }
        let key = key(path, true)?.ok_or("Cannot create Windows registration")?;
        let name = wide(name);
        let error = unsafe {
            match value {
                Some(v) => RegSetValueExW(
                    key.0,
                    name.as_ptr(),
                    0,
                    v.kind,
                    v.bytes.as_ptr(),
                    v.bytes.len() as u32,
                ),
                None => RegDeleteValueW(key.0, name.as_ptr()),
            }
        };
        if error == 0 || (value.is_none() && error == 2) {
            Ok(())
        } else {
            Err(format!("Windows registration update failed ({error})"))
        }
    }
    fn text(path: &str, name: &str) -> Result<Option<String>, String> {
        read(path, name)?.as_ref().map(decode).transpose()
    }

    struct Transaction {
        changes: Vec<(String, String, Option<RegistryValue>)>,
        committed: bool,
    }
    impl Transaction {
        fn new() -> Self {
            Self {
                changes: Vec::new(),
                committed: false,
            }
        }
        fn set(
            &mut self,
            path: &str,
            name: &str,
            value: Option<&RegistryValue>,
        ) -> Result<(), String> {
            let old = read(path, name)?;
            if old.as_ref() == value {
                return Ok(());
            }
            self.changes.push((path.into(), name.into(), old));
            write(path, name, value)
        }
        fn put(&mut self, path: &str, name: &str, value: &str) -> Result<(), String> {
            self.set(path, name, Some(&string(value)))
        }
        fn commit(mut self) {
            self.committed = true;
        }
    }
    impl Drop for Transaction {
        fn drop(&mut self) {
            if !self.committed {
                for error in rollback_changes(&self.changes, write) {
                    log::error!("Shell registration rollback failed: {error}");
                }
                notify();
            }
        }
    }

    fn slot(extension: &str) -> String {
        format!("Software\\Classes\\.{extension}\\shellex\\{THUMBNAIL_SLOT}")
    }
    fn backup_key() -> String {
        format!("{APP_KEY}\\ThumbnailBackups")
    }
    fn class_key() -> String {
        format!("Software\\Classes\\CLSID\\{PROVIDER}\\InprocServer32")
    }
    fn prog_id(extension: &str) -> String {
        format!("{APP}.{extension}")
    }
    fn notify() {
        unsafe {
            SHChangeNotify(0x08000000, 0, ptr::null(), ptr::null());
        }
    }
    fn installed_path() -> Result<Option<PathBuf>, String> {
        Ok(text(APP_KEY, "InstalledDirectory")?.map(PathBuf::from))
    }
    fn ready(directory: &Path) -> bool {
        directory.join("VisualMediaViewer.exe").is_file()
            && DEPENDENCIES
                .iter()
                .all(|name| directory.join(name).is_file())
    }

    fn install(require_provider: bool) -> Result<PathBuf, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let source = exe
            .parent()
            .ok_or("Cannot locate the application directory")?;
        let provider_source = source.join(PROVIDER_FILE);
        if require_provider && !provider_source.is_file() {
            return Err(format!("{PROVIDER_FILE} is missing. Extract all distribution files before enabling thumbnails."));
        }
        for name in DEPENDENCIES {
            if !source.join(name).is_file() {
                return Err(format!("Required file is missing: {name}"));
            }
        }
        let root = std::env::var_os("LOCALAPPDATA")
            .ok_or("Cannot locate your Windows application data folder")?;
        let root = PathBuf::from(root).join(APP).join("Shell");
        let mut sources = vec![(exe.clone(), "VisualMediaViewer.exe")];
        sources.extend(DEPENDENCIES.iter().map(|name| (source.join(name), *name)));
        if provider_source.is_file() {
            sources.push((provider_source, PROVIDER_FILE));
        }
        for name in ["LICENSE.txt", "LGPL-2.1.txt", "THIRD_PARTY_LICENSES.txt"] {
            if source.join(name).is_file() {
                sources.push((source.join(name), name));
            }
        }
        let mut candidates: Vec<PathBuf> = [
            installed_path()?,
            text(APP_KEY, "ThumbnailDirectory")?.map(PathBuf::from),
        ]
        .into_iter()
        .flatten()
        .collect();
        if let Ok(entries) = std::fs::read_dir(&root) {
            candidates.extend(entries.flatten().map(|entry| entry.path()));
        }
        let prefix = format!("{}-", env!("CARGO_PKG_VERSION"));
        for candidate in candidates {
            if candidate.parent() != Some(root.as_path())
                || !candidate
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with(&prefix))
            {
                continue;
            }
            let files: Vec<_> = sources
                .iter()
                .map(|(source, name)| (source.clone(), candidate.join(name)))
                .collect();
            if deployment_matches(&files).unwrap_or(false) {
                return Ok(candidate);
            }
        }
        let generation = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let destination = root.join(format!("{}-{generation}", env!("CARGO_PKG_VERSION")));
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        std::fs::create_dir(&destination).map_err(|e| e.to_string())?;
        for (source, name) in &sources {
            if let Err(error) = std::fs::copy(source, destination.join(name)) {
                // Only this newly-created generation's known files may be removed.
                for (_, name) in &sources {
                    let _ = std::fs::remove_file(destination.join(name));
                }
                let _ = std::fs::remove_dir(&destination);
                return Err(error.to_string());
            }
        }
        Ok(destination)
    }

    fn register_at(transaction: &mut Transaction, directory: &Path) -> Result<(), String> {
        let exe = directory.join("VisualMediaViewer.exe");
        let command = format!("\"{}\" \"%1\"", exe.display());
        transaction.put(CAPABILITIES, "ApplicationName", "Visual Media Viewer")?;
        transaction.put(
            CAPABILITIES,
            "ApplicationDescription",
            "Visual Media Viewer",
        )?;
        transaction.put("Software\\RegisteredApplications", APP, CAPABILITIES)?;
        for extension in EXTENSIONS {
            let id = prog_id(extension);
            let path = format!("Software\\Classes\\{id}");
            transaction.put(&path, "", "Visual Media Viewer media")?;
            transaction.put(&format!("{path}\\shell\\open\\command"), "", &command)?;
            transaction.put(
                &format!("{path}\\DefaultIcon"),
                "",
                &format!("\"{}\",0", exe.display()),
            )?;
            transaction.put(
                &format!("{CAPABILITIES}\\FileAssociations"),
                &format!(".{extension}"),
                &id,
            )?;
            transaction.set(
                &format!("Software\\Classes\\.{extension}\\OpenWithProgids"),
                &id,
                Some(&RegistryValue {
                    kind: 0,
                    bytes: Vec::new(),
                }),
            )?;
        }
        transaction.put(APP_KEY, "InstalledDirectory", &directory.to_string_lossy())?;
        transaction.put(APP_KEY, "InstalledVersion", env!("CARGO_PKG_VERSION"))?;
        Ok(())
    }

    pub fn register_app() -> Result<(), String> {
        let directory = install(false)?;
        let mut transaction = Transaction::new();
        register_at(&mut transaction, &directory)?;
        transaction.commit();
        notify();
        Ok(())
    }

    pub fn unregister_app() -> Result<(), String> {
        let _apartment = ComApartment::enter()?;
        let mut transaction = Transaction::new();
        let installed = installed_path()?;
        let command = installed.as_ref().map(|directory| {
            format!(
                "\"{}\" \"%1\"",
                directory.join("VisualMediaViewer.exe").display()
            )
        });
        for extension in EXTENSIONS {
            let id = prog_id(extension);
            let class = format!("Software\\Classes\\{id}");
            let command_key = format!("{class}\\shell\\open\\command");
            let effective_default = installed.as_ref().is_some_and(|dir| {
                effective_executable(extension)
                    .as_ref()
                    .is_some_and(|exe| same_path(exe, &dir.join("VisualMediaViewer.exe")))
            });
            let user_choice = text(&format!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\FileExts\\.{extension}\\UserChoice"), "ProgId")?.is_some_and(|choice| choice.eq_ignore_ascii_case(&id));
            if may_remove_app_class(
                command.is_some() && text(&command_key, "")? == command,
                effective_default,
                user_choice,
            ) {
                transaction.set(&command_key, "", None)?;
                if text(&class, "")?.as_deref() == Some("Visual Media Viewer media") {
                    transaction.set(&class, "", None)?;
                }
                let icon_key = format!("{class}\\DefaultIcon");
                let expected_icon = installed
                    .as_ref()
                    .map(|dir| format!("\"{}\",0", dir.join("VisualMediaViewer.exe").display()));
                if text(&icon_key, "")? == expected_icon {
                    transaction.set(&icon_key, "", None)?;
                }
            }
            let capabilities_key = format!("{CAPABILITIES}\\FileAssociations");
            if text(&capabilities_key, &format!(".{extension}"))?.as_deref() == Some(&id) {
                transaction.set(&capabilities_key, &format!(".{extension}"), None)?;
            }
            let open_with = format!("Software\\Classes\\.{extension}\\OpenWithProgids");
            if read(&open_with, &id)?.as_ref()
                == Some(&RegistryValue {
                    kind: 0,
                    bytes: Vec::new(),
                })
            {
                transaction.set(&open_with, &id, None)?;
            }
        }
        if text("Software\\RegisteredApplications", APP)?.as_deref() == Some(CAPABILITIES) {
            transaction.set("Software\\RegisteredApplications", APP, None)?;
        }
        for name in ["ApplicationName", "ApplicationDescription"] {
            if text(CAPABILITIES, name)?.as_deref() == Some("Visual Media Viewer") {
                transaction.set(CAPABILITIES, name, None)?;
            }
        }
        transaction.commit();
        notify();
        Ok(())
    }

    pub fn set_thumbnails_enabled(enabled: bool) -> Result<(), String> {
        let ours = string(PROVIDER);
        let mut transaction = Transaction::new();
        if enabled {
            let directory = install(true)?;
            // Keep the existing default-app command untouched when enabling thumbnails.
            transaction.put(
                &class_key(),
                "",
                &directory.join(PROVIDER_FILE).to_string_lossy(),
            )?;
            transaction.put(&class_key(), "ThreadingModel", "Apartment")?;
            transaction.put(APP_KEY, "ThumbnailDirectory", &directory.to_string_lossy())?;
            transaction.put(APP_KEY, "ThumbnailVersion", env!("CARGO_PKG_VERSION"))?;
            for extension in EXTENSIONS {
                let path = slot(extension);
                let current = read(&path, "")?;
                if current.as_ref() != Some(&ours) {
                    let saved = serde_json::to_string(&Backup { original: current })
                        .map_err(|e| e.to_string())?;
                    transaction.put(&backup_key(), extension, &saved)?;
                }
                transaction.set(&path, "", Some(&ours))?;
            }
        } else {
            for extension in EXTENSIONS {
                let path = slot(extension);
                let current = read(&path, "")?;
                let backup: Option<Backup> = text(&backup_key(), extension)?
                    .map(|v| {
                        serde_json::from_str(&v)
                            .map_err(|e| format!("Cannot restore .{extension} registration: {e}"))
                    })
                    .transpose()?;
                if let Some(value) = restore_value(current.as_ref(), &ours, backup.as_ref()) {
                    transaction.set(&path, "", value.as_ref())?;
                }
                transaction.set(&backup_key(), extension, None)?;
            }
            let registered = text(&class_key(), "")?.map(PathBuf::from);
            let directory = text(APP_KEY, "ThumbnailDirectory")?.map(PathBuf::from);
            if directory
                .as_ref()
                .is_some_and(|dir| registered.as_ref() == Some(&dir.join(PROVIDER_FILE)))
                || registered
                    .as_ref()
                    .is_some_and(|path| own_provider_path(path))
            {
                transaction.set(&class_key(), "", None)?;
                transaction.set(&class_key(), "ThreadingModel", None)?;
            }
            transaction.set(APP_KEY, "ThumbnailDirectory", None)?;
            transaction.set(APP_KEY, "ThumbnailVersion", None)?;
        }
        transaction.commit();
        notify();
        Ok(())
    }

    fn same_path(left: &Path, right: &Path) -> bool {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    }

    fn own_provider_path(path: &Path) -> bool {
        path.file_name().is_some_and(|name| name == PROVIDER_FILE)
            && std::env::var_os("LOCALAPPDATA")
                .is_some_and(|root| path.starts_with(PathBuf::from(root).join(APP).join("Shell")))
    }

    fn association_string(extension: &str, kind: u32, extra: Option<&str>) -> Option<String> {
        let extension = wide(&format!(".{extension}"));
        let extra = extra.map(wide);
        let extra_ptr = extra.as_ref().map_or(ptr::null(), |v| v.as_ptr());
        let mut len = 0;
        unsafe {
            AssocQueryStringW(
                0,
                kind,
                extension.as_ptr(),
                extra_ptr,
                ptr::null_mut(),
                &mut len,
            );
        }
        if len == 0 || len > 32768 {
            return None;
        }
        let mut result = vec![0u16; len as usize];
        let error = unsafe {
            AssocQueryStringW(
                0,
                kind,
                extension.as_ptr(),
                extra_ptr,
                result.as_mut_ptr(),
                &mut len,
            )
        };
        if error < 0 {
            return None;
        }
        let end = result.iter().position(|&u| u == 0).unwrap_or(result.len());
        Some(String::from_utf16_lossy(&result[..end]))
    }

    fn effective_executable(extension: &str) -> Option<PathBuf> {
        association_string(extension, 2, None).map(PathBuf::from)
    }

    pub fn inspect() -> Result<Status, String> {
        let _apartment = ComApartment::enter()?;
        let installed = installed_path()?;
        let command_exe = installed
            .as_ref()
            .map(|dir| dir.join("VisualMediaViewer.exe"));
        let current_exe = std::env::current_exe().ok();
        let thumbnail_dir = text(APP_KEY, "ThumbnailDirectory")?.map(PathBuf::from);
        let provider_path = text(&class_key(), "")?.map(PathBuf::from);
        let provider_ready = thumbnail_dir.as_ref().is_some_and(|dir| {
            ready(dir)
                && provider_path.as_ref() == Some(&dir.join(PROVIDER_FILE))
                && dir.join(PROVIDER_FILE).is_file()
        });
        let mut result = Status {
            total_count: EXTENSIONS.len(),
            ..Status::default()
        };
        result.app_registered = text("Software\\RegisteredApplications", APP)?.as_deref()
            == Some(CAPABILITIES)
            && installed.as_ref().is_some_and(|dir| ready(dir));
        let mut has_app_registration =
            text("Software\\RegisteredApplications", APP)?.as_deref() == Some(CAPABILITIES);
        let mut has_backups = false;
        for extension in EXTENSIONS {
            has_backups |= read(&backup_key(), extension)?.is_some();
            has_app_registration |= text(
                &format!("{CAPABILITIES}\\FileAssociations"),
                &format!(".{extension}"),
            )?
            .as_deref()
                == Some(&prog_id(extension))
                || read(
                    &format!("Software\\Classes\\.{extension}\\OpenWithProgids"),
                    &prog_id(extension),
                )?
                .is_some();
            let expected_command = command_exe
                .as_ref()
                .map(|exe| format!("\"{}\" \"%1\"", exe.display()));
            result.app_registered &= text(
                &format!("{CAPABILITIES}\\FileAssociations"),
                &format!(".{extension}"),
            )?
            .as_deref()
                == Some(&prog_id(extension))
                && text(
                    &format!(
                        "Software\\Classes\\{}\\shell\\open\\command",
                        prog_id(extension)
                    ),
                    "",
                )? == expected_command
                && read(
                    &format!("Software\\Classes\\.{extension}\\OpenWithProgids"),
                    &prog_id(extension),
                )? == Some(RegistryValue {
                    kind: 0,
                    bytes: Vec::new(),
                });
            let effective = effective_executable(extension);
            let is_default = effective.as_ref().is_some_and(|path| {
                command_exe.as_ref().is_some_and(|exe| same_path(exe, path))
                    || current_exe.as_ref().is_some_and(|exe| same_path(exe, path))
            });
            let thumbnail_registered = text(&slot(extension), "")?
                .is_some_and(|value| value.eq_ignore_ascii_case(PROVIDER));
            let thumbnail_effective = association_string(extension, 16, Some(THUMBNAIL_SLOT))
                .is_some_and(|value| value.eq_ignore_ascii_case(PROVIDER));
            result.default_count += usize::from(is_default);
            result.thumbnail_count += usize::from(thumbnail_registered);
            result.thumbnail_effective_count += usize::from(thumbnail_effective);
            result.extensions.push(ExtensionStatus {
                extension: format!(".{extension}"),
                is_default,
                thumbnail_registered,
                thumbnail_effective,
            });
        }
        result.thumbnails_enabled = result.thumbnail_count == result.total_count && provider_ready;
        result.app_partial = has_app_registration && !result.app_registered;
        result.app_update_available = result.app_registered
            && text(APP_KEY, "InstalledVersion")?.as_deref() != Some(env!("CARGO_PKG_VERSION"));
        result.thumbnail_update_available = result.thumbnails_enabled
            && text(APP_KEY, "ThumbnailVersion")?.as_deref() != Some(env!("CARGO_PKG_VERSION"));
        let remaining_provider = provider_path
            .as_ref()
            .is_some_and(|path| own_provider_path(path));
        result.thumbnails_partial = (result.thumbnail_count > 0
            || has_backups
            || remaining_provider
            || thumbnail_dir.is_some())
            && !result.thumbnails_enabled;
        Ok(result)
    }

    pub fn open_default_apps() -> Result<(), String> {
        let _apartment = ComApartment::enter()?;
        let uri = "ms-settings:defaultapps?registeredAppUser=VisualMediaViewer";
        let launch = |uri: &str| unsafe {
            ShellExecuteW(
                0,
                wide("open").as_ptr(),
                wide(uri).as_ptr(),
                ptr::null(),
                ptr::null(),
                1,
            )
        };
        if launch(uri) > 32 || launch("ms-settings:defaultapps") > 32 {
            Ok(())
        } else {
            Err("Cannot open Windows default app settings".into())
        }
    }
}

#[cfg(windows)]
pub use platform::{
    inspect, open_default_apps, register_app, set_thumbnails_enabled, unregister_app,
};

#[cfg(not(windows))]
pub fn inspect() -> Result<Status, String> {
    Err("Windows only".into())
}
#[cfg(not(windows))]
pub fn register_app() -> Result<(), String> {
    Err("Windows only".into())
}
#[cfg(not(windows))]
pub fn unregister_app() -> Result<(), String> {
    Err("Windows only".into())
}
#[cfg(not(windows))]
pub fn open_default_apps() -> Result<(), String> {
    Err("Windows only".into())
}
#[cfg(not(windows))]
pub fn set_thumbnails_enabled(_enabled: bool) -> Result<(), String> {
    Err("Windows only".into())
}
