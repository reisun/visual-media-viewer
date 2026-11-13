use eframe::egui;
use std::path::Path;

#[cfg(target_os = "windows")]
pub fn decode_thumbnail_from_bytes(
    bytes: &[u8],
    max_side: u32,
) -> Result<(u32, u32, Vec<u8>), String> {
    use windows::Win32::Graphics::Imaging::*;
    use windows::Win32::System::Com::*;

    if bytes.is_empty() || bytes.len() > 256 * 1024 * 1024 || !(1..=1024).contains(&max_side) {
        return Err("Invalid thumbnail input".into());
    }
    unsafe {
        let initialized = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        struct Apartment(bool);
        impl Drop for Apartment {
            fn drop(&mut self) {
                if self.0 {
                    unsafe { CoUninitialize() };
                }
            }
        }
        let _apartment = Apartment(initialized);
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| e.to_string())?;
        // WIC borrows the memory for the lifetime of its stream and decoder.
        let mut input = bytes.to_vec();
        let stream = factory.CreateStream().map_err(|e| e.to_string())?;
        stream
            .InitializeFromMemory(&mut input)
            .map_err(|e| e.to_string())?;
        let decoder = factory
            .CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)
            .map_err(|e| e.to_string())?;
        let frame = decoder.GetFrame(0).map_err(|e| e.to_string())?;
        let (mut width, mut height) = (0, 0);
        frame
            .GetSize(&mut width, &mut height)
            .map_err(|e| e.to_string())?;
        if width == 0
            || height == 0
            || width > 32768
            || height > 32768
            || u64::from(width) * u64::from(height) > 64 * 1024 * 1024
        {
            return Err("Invalid thumbnail dimensions".into());
        }
        let ratio = (max_side as f64 / width.max(height) as f64).min(1.0);
        let out_width = ((width as f64 * ratio).round() as u32).max(1);
        let out_height = ((height as f64 * ratio).round() as u32).max(1);
        let scaler = factory.CreateBitmapScaler().map_err(|e| e.to_string())?;
        scaler
            .Initialize(
                &frame,
                out_width,
                out_height,
                WICBitmapInterpolationModeFant,
            )
            .map_err(|e| e.to_string())?;
        let converter = factory.CreateFormatConverter().map_err(|e| e.to_string())?;
        converter
            .Initialize(
                &scaler,
                &GUID_WICPixelFormat32bppRGBA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .map_err(|e| e.to_string())?;
        let mut pixels = vec![0; out_width as usize * out_height as usize * 4];
        converter
            .CopyPixels(std::ptr::null(), out_width * 4, &mut pixels)
            .map_err(|e| e.to_string())?;
        Ok((out_width, out_height, pixels))
    }
}

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

        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
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
