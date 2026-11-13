use eframe::egui;

pub fn load() -> egui::IconData {
    let image = image::load_from_memory_with_format(
        include_bytes!("../assets/icon.ico"),
        image::ImageFormat::Ico,
    )
    .expect("embedded application icon must decode")
    .into_rgba8();
    let (width, height) = image.dimensions();
    egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    }
}

#[cfg(windows)]
pub fn set_taskbar_icon(window: &impl raw_window_handle::HasWindowHandle) {
    use std::ffi::c_void;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        LoadImageW, SendMessageW, ICON_BIG, IMAGE_ICON, LR_DEFAULTSIZE, LR_SHARED, WM_SETICON,
    };

    let Ok(handle) = window.window_handle() else {
        log::warn!("Taskbar icon: window handle unavailable");
        return;
    };
    let raw_window_handle::RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    unsafe {
        let result = GetModuleHandleW(None).and_then(|module| {
            // Resource 1 is embedded by build.rs. LR_SHARED keeps ownership with Windows.
            LoadImageW(
                Some(HINSTANCE(module.0)),
                PCWSTR(1usize as *const u16),
                IMAGE_ICON,
                0,
                0,
                LR_DEFAULTSIZE | LR_SHARED,
            )
        });
        match result {
            Ok(icon) => {
                // winit's window icon sets ICON_SMALL only; the taskbar needs ICON_BIG.
                SendMessageW(
                    HWND(handle.hwnd.get() as *mut c_void),
                    WM_SETICON,
                    Some(WPARAM(ICON_BIG as usize)),
                    Some(LPARAM(icon.0 as isize)),
                );
            }
            Err(error) => log::warn!("Taskbar icon could not be loaded: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn embedded_icon_has_visible_and_transparent_pixels() {
        let icon = super::load();
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
        assert!(icon.rgba.chunks_exact(4).any(|pixel| pixel[3] > 0));
        assert!(icon.rgba.chunks_exact(4).any(|pixel| pixel[3] == 0));
    }
}
