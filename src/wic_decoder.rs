use eframe::egui;
use std::path::Path;

#[cfg(target_os = "windows")]
pub fn decode_with_wic(path: &Path) -> Result<egui::ColorImage, String> {
    use windows::core::HSTRING;
    use windows::Win32::Graphics::Imaging::*;
    use windows::Win32::System::Com::*;

    #[repr(transparent)]
    #[derive(Clone, Copy)]
    struct GenericAccessRights(u32);
    const GENERIC_READ: GenericAccessRights = GenericAccessRights(0x80000000);

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let factory: IWICImagingFactory = CoCreateInstance(
            &CLSID_WICImagingFactory,
            None,
            CLSCTX_INPROC_SERVER,
        )
        .map_err(|e| format!("Failed to create WIC factory: {}", e))?;

        let path_str = path.to_string_lossy();
        let wide_path = HSTRING::from(path_str.as_ref());

        let decoder = factory
            .CreateDecoderFromFilename(
                &wide_path,
                None,
                std::mem::transmute(GENERIC_READ),
                WICDecodeMetadataCacheOnDemand,
            )
            .map_err(|e| format!("WIC decoder failed for {}: {}", path.display(), e))?;

        let frame = decoder
            .GetFrame(0)
            .map_err(|e| format!("WIC GetFrame failed: {}", e))?;

        let mut width = 0u32;
        let mut height = 0u32;
        frame
            .GetSize(&mut width, &mut height)
            .map_err(|e| format!("WIC GetSize failed: {}", e))?;

        let converter: IWICFormatConverter = factory
            .CreateFormatConverter()
            .map_err(|e| format!("WIC CreateFormatConverter failed: {}", e))?;

        converter
            .Initialize(
                &frame,
                &GUID_WICPixelFormat32bppRGBA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .map_err(|e| format!("WIC format conversion failed: {}", e))?;

        let stride = width * 4;
        let buf_size = (stride * height) as usize;
        let mut buf = vec![0u8; buf_size];
        converter
            .CopyPixels(std::ptr::null(), stride, &mut buf)
            .map_err(|e| format!("WIC CopyPixels failed: {}", e))?;

        let image =
            egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &buf);
        Ok(image)
    }
}

#[cfg(not(target_os = "windows"))]
pub fn decode_with_wic(_path: &Path) -> Result<egui::ColorImage, String> {
    Err("WIC decoding is only available on Windows".to_string())
}
