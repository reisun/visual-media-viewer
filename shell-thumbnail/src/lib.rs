#![cfg(windows)]
#![allow(non_snake_case)]

use std::cell::RefCell;
use std::ffi::{c_void, OsString};
use std::io::{self, Read, Write};
use std::os::windows::{ffi::OsStringExt, process::CommandExt};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use windows::core::{implement, Error, IUnknown, Interface, Result, GUID, HRESULT};
use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, E_INVALIDARG, E_POINTER,
    E_UNEXPECTED, HMODULE, S_FALSE, S_OK,
};
use windows::Win32::Graphics::Gdi::{
    CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP,
};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICBmpDecoder, CLSID_WICGifDecoder, CLSID_WICImagingFactory, CLSID_WICJpegDecoder,
    CLSID_WICPngDecoder, CLSID_WICTiffDecoder, GUID_WICPixelFormat32bppRGBA, IWICBitmapDecoder,
    IWICImagingFactory, WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant,
    WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{
    CoCreateInstance, IClassFactory, IClassFactory_Impl, IStream, CLSCTX_INPROC_SERVER,
    STATFLAG_NONAME, STATSTG, STREAM_SEEK_CUR, STREAM_SEEK_SET,
};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::UI::Shell::PropertiesSystem::{
    IInitializeWithStream, IInitializeWithStream_Impl,
};
use windows::Win32::UI::Shell::{
    IThumbnailProvider, IThumbnailProvider_Impl, WTSAT_ARGB, WTS_ALPHATYPE,
};

const CLSID: GUID = GUID::from_u128(0xd4f66b4e_5762_4ba9_9633_138915fd01a7);
const MAX_READ: usize = 1024 * 1024;
const MAX_REQUESTS: usize = 100_000;
static MODULE: AtomicUsize = AtomicUsize::new(0);
static OBJECTS: AtomicUsize = AtomicUsize::new(0);
static LOCKS: AtomicUsize = AtomicUsize::new(0);

struct ObjectGuard;
impl ObjectGuard {
    fn new() -> Self {
        OBJECTS.fetch_add(1, Ordering::SeqCst);
        Self
    }
}
impl Drop for ObjectGuard {
    fn drop(&mut self) {
        OBJECTS.fetch_sub(1, Ordering::SeqCst);
    }
}

#[implement(IClassFactory)]
struct Factory {
    _guard: ObjectGuard,
}

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: windows::core::Ref<IUnknown>,
        iid: *const GUID,
        output: *mut *mut c_void,
    ) -> Result<()> {
        unsafe {
            if output.is_null() {
                return Err(E_POINTER.into());
            }
            *output = std::ptr::null_mut();
            if iid.is_null() {
                return Err(E_POINTER.into());
            }
            if !outer.is_null() {
                return Err(CLASS_E_NOAGGREGATION.into());
            }
            let provider: IUnknown = Provider {
                stream: RefCell::new(None),
                _guard: ObjectGuard::new(),
            }
            .into();
            provider.query(iid, output).ok()
        }
    }

    fn LockServer(&self, lock: windows::core::BOOL) -> Result<()> {
        if lock.as_bool() {
            LOCKS.fetch_add(1, Ordering::SeqCst);
        } else {
            LOCKS
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                    count.checked_sub(1)
                })
                .map_err(|_| Error::from(E_UNEXPECTED))?;
        }
        Ok(())
    }
}

#[implement(IInitializeWithStream, IThumbnailProvider)]
struct Provider {
    stream: RefCell<Option<IStream>>,
    _guard: ObjectGuard,
}

impl IInitializeWithStream_Impl for Provider_Impl {
    fn Initialize(&self, stream: windows::core::Ref<IStream>, _mode: u32) -> Result<()> {
        let mut current = self
            .stream
            .try_borrow_mut()
            .map_err(|_| Error::from(E_UNEXPECTED))?;
        if current.is_some() {
            return Err(E_UNEXPECTED.into());
        }
        *current = Some(stream.ok()?.clone());
        Ok(())
    }
}

impl IThumbnailProvider_Impl for Provider_Impl {
    fn GetThumbnail(
        &self,
        size: u32,
        bitmap: *mut HBITMAP,
        alpha: *mut WTS_ALPHATYPE,
    ) -> Result<()> {
        unsafe {
            if !bitmap.is_null() {
                *bitmap = HBITMAP::default();
            }
            if bitmap.is_null() || alpha.is_null() {
                return Err(E_POINTER.into());
            }
            *alpha = Default::default();
            if !(1..=1024).contains(&size) {
                return Err(E_INVALIDARG.into());
            }
            let stream = self
                .stream
                .try_borrow_mut()
                .map_err(|_| Error::from(E_UNEXPECTED))?;
            let stream = stream.as_ref().ok_or_else(|| Error::from(E_UNEXPECTED))?;
            let mut original = 0;
            let restore = stream.Seek(0, STREAM_SEEK_CUR, Some(&mut original)).is_ok();
            let result = generate(stream, size).and_then(|pixels| make_bitmap(pixels));
            if restore {
                let _ = stream.Seek(original as i64, STREAM_SEEK_SET, None);
            }
            *bitmap = result?;
            *alpha = WTSAT_ARGB;
            Ok(())
        }
    }
}

fn executable() -> Result<PathBuf> {
    let mut path = vec![0u16; 32768];
    let length = unsafe {
        GetModuleFileNameW(
            Some(HMODULE(MODULE.load(Ordering::Relaxed) as *mut c_void)),
            &mut path,
        )
    } as usize;
    if length == 0 || length >= path.len() {
        return Err(E_FAIL.into());
    }
    let mut path = PathBuf::from(OsString::from_wide(&path[..length]));
    path.set_file_name("VisualMediaViewer.exe");
    Ok(path)
}

struct Worker {
    child: Arc<Mutex<Child>>,
    cancel: Option<mpsc::Sender<()>>,
    watchdog: Option<JoinHandle<()>>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(watchdog) = self.watchdog.take() {
            let _ = watchdog.join();
        }
    }
}

struct Pixels {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

fn generate(stream: &IStream, size: u32) -> Result<Pixels> {
    if let Some(class) = wic_decoder_class(stream).unwrap_or(None) {
        if let Ok(pixels) = wic_thumbnail(stream, size, &class) {
            return Ok(pixels);
        }
    }
    unsafe { stream.Seek(0, STREAM_SEEK_SET, None)? };
    generate_with_worker(stream, size)
}

fn wic_decoder_class(stream: &IStream) -> Result<Option<GUID>> {
    let mut signature = [0u8; 32];
    let mut length = 0;
    unsafe {
        let mut stat = STATSTG::default();
        stream.Stat(&mut stat, STATFLAG_NONAME)?;
        if stat.cbSize > 128 * 1024 * 1024 {
            return Ok(None);
        }
        stream.Seek(0, STREAM_SEEK_SET, None)?;
        while length < signature.len() {
            let mut actual = 0;
            stream
                .Read(
                    signature[length..].as_mut_ptr().cast(),
                    (signature.len() - length) as u32,
                    Some(&mut actual),
                )
                .ok()?;
            if actual == 0 {
                break;
            }
            if actual as usize > signature.len() - length {
                return Err(E_FAIL.into());
            }
            length += actual as usize;
        }
        stream.Seek(0, STREAM_SEEK_SET, None)?;
    }
    let bytes = &signature[..length];
    Ok(if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(CLSID_WICJpegDecoder)
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(CLSID_WICPngDecoder)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(CLSID_WICGifDecoder)
    } else if bytes.starts_with(b"BM") {
        Some(CLSID_WICBmpDecoder)
    } else if bytes.starts_with(b"II\x2a\0") || bytes.starts_with(b"MM\0\x2a") {
        Some(CLSID_WICTiffDecoder)
    } else {
        None
    })
}

fn wic_thumbnail(stream: &IStream, size: u32, class: &GUID) -> Result<Pixels> {
    unsafe {
        // Use the caller's COM apartment and keep WIC objects local to this request.
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let decoder: IWICBitmapDecoder = CoCreateInstance(class, None, CLSCTX_INPROC_SERVER)?;
        decoder.Initialize(stream, WICDecodeMetadataCacheOnDemand)?;
        let frame = decoder.GetFrame(0)?;
        let (mut width, mut height) = (0, 0);
        frame.GetSize(&mut width, &mut height)?;
        if width == 0
            || height == 0
            || width > 32768
            || height > 32768
            || u64::from(width) * u64::from(height) > 64 * 1024 * 1024
        {
            return Err(E_FAIL.into());
        }
        let ratio = (f64::from(size) / f64::from(width.max(height))).min(1.0);
        let width = ((f64::from(width) * ratio).round() as u32).max(1);
        let height = ((f64::from(height) * ratio).round() as u32).max(1);
        // Immediately following the frame lets WIC use JPEG's native reduced decoding.
        let scaler = factory.CreateBitmapScaler()?;
        scaler.Initialize(&frame, width, height, WICBitmapInterpolationModeFant)?;
        let converter = factory.CreateFormatConverter()?;
        converter.Initialize(
            &scaler,
            &GUID_WICPixelFormat32bppRGBA,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeCustom,
        )?;
        let mut rgba = vec![0; width as usize * height as usize * 4];
        converter.CopyPixels(std::ptr::null(), width * 4, &mut rgba)?;
        Ok(Pixels {
            width,
            height,
            rgba,
        })
    }
}

fn generate_with_worker(stream: &IStream, size: u32) -> Result<Pixels> {
    let mut child = Command::new(executable()?)
        .args(["--shell-thumbnail-worker", "--size", &size.to_string()])
        .creation_flags(0x08000000)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| Error::from(E_FAIL))?;
    let mut input = child.stdin.take().ok_or_else(|| Error::from(E_FAIL))?;
    let mut output = child.stdout.take().ok_or_else(|| Error::from(E_FAIL))?;
    let child = Arc::new(Mutex::new(child));
    let (cancel, deadline) = mpsc::channel();
    let killer = Arc::clone(&child);
    let watchdog = thread::Builder::new()
        .name("thumbnail-timeout".into())
        .spawn(move || {
            if deadline.recv_timeout(Duration::from_secs(10))
                == Err(mpsc::RecvTimeoutError::Timeout)
            {
                if let Ok(mut child) = killer.lock() {
                    let _ = child.kill();
                }
            }
        });
    let mut worker = Worker {
        child,
        cancel: Some(cancel),
        watchdog: None,
    };
    worker.watchdog = Some(watchdog.map_err(|_| Error::from(E_FAIL))?);
    // Keep every COM stream call on this apartment; only pipe I/O crosses the process boundary.
    serve(stream, size, &mut output, &mut input).map_err(|_| Error::from(E_FAIL))
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid thumbnail response")
}
fn read_u32(reader: &mut impl Read) -> io::Result<u32> {
    let mut bytes = [0; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn serve(
    stream: &IStream,
    size: u32,
    output: &mut impl Read,
    input: &mut impl Write,
) -> io::Result<Pixels> {
    let mut scratch = Vec::new();
    for _ in 0..MAX_REQUESTS {
        let mut tag = [0];
        output.read_exact(&mut tag)?;
        match tag[0] {
            1 => {
                let count = read_u32(output)? as usize;
                if count > MAX_READ {
                    return Err(invalid());
                }
                scratch.resize(count, 0);
                let mut actual = 0;
                let result = unsafe {
                    stream.Read(scratch.as_mut_ptr().cast(), count as u32, Some(&mut actual))
                };
                if result.is_err() || actual as usize > count {
                    input.write_all(&u32::MAX.to_le_bytes())?;
                } else {
                    input.write_all(&actual.to_le_bytes())?;
                    input.write_all(&scratch[..actual as usize])?;
                }
            }
            2 => {
                let mut bytes = [0; 8];
                output.read_exact(&mut bytes)?;
                let offset = i64::from_le_bytes(bytes);
                let origin = read_u32(output)?;
                if origin > 2 {
                    return Err(invalid());
                }
                let mut position = 0;
                let result = unsafe {
                    stream.Seek(
                        offset,
                        windows::Win32::System::Com::STREAM_SEEK(origin),
                        Some(&mut position),
                    )
                };
                input.write_all(&if result.is_ok() { position } else { u64::MAX }.to_le_bytes())?;
            }
            3 => {
                let mut info = STATSTG::default();
                let result =
                    unsafe { stream.Stat(&mut info, windows::Win32::System::Com::STATFLAG_NONAME) };
                input.write_all(
                    &if result.is_ok() {
                        info.cbSize
                    } else {
                        u64::MAX
                    }
                    .to_le_bytes(),
                )?;
            }
            4 => {
                let width = read_u32(output)?;
                let height = read_u32(output)?;
                if width == 0 || height == 0 || width > size || height > size {
                    return Err(invalid());
                }
                let mut rgba = vec![0; width as usize * height as usize * 4];
                output.read_exact(&mut rgba)?;
                let mut trailing = [0];
                if output.read(&mut trailing)? != 0 {
                    return Err(invalid());
                }
                return Ok(Pixels {
                    width,
                    height,
                    rgba,
                });
            }
            _ => return Err(invalid()),
        }
        input.flush()?;
    }
    Err(invalid())
}

fn make_bitmap(pixels: Pixels) -> Result<HBITMAP> {
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: pixels.width as i32,
            biHeight: -(pixels.height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    let bitmap = unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)? };
    if bits.is_null() {
        unsafe {
            let _ = DeleteObject(bitmap.into());
        }
        return Err(E_FAIL.into());
    }
    let destination =
        unsafe { std::slice::from_raw_parts_mut(bits.cast::<u8>(), pixels.rgba.len()) };
    for (src, dst) in pixels
        .rgba
        .chunks_exact(4)
        .zip(destination.chunks_exact_mut(4))
    {
        let alpha = src[3] as u16;
        dst[0] = ((src[2] as u16 * alpha + 127) / 255) as u8;
        dst[1] = ((src[1] as u16 * alpha + 127) / 255) as u8;
        dst[2] = ((src[0] as u16 * alpha + 127) / 255) as u8;
        dst[3] = src[3];
    }
    Ok(bitmap)
}

#[no_mangle]
pub unsafe extern "system" fn DllMain(module: *mut c_void, reason: u32, _: *mut c_void) -> i32 {
    if reason == 1 {
        MODULE.store(module as usize, Ordering::Relaxed);
    }
    1
}

#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(
    class: *const GUID,
    iid: *const GUID,
    output: *mut *mut c_void,
) -> HRESULT {
    if output.is_null() {
        return E_POINTER;
    }
    *output = std::ptr::null_mut();
    if class.is_null() || iid.is_null() {
        return E_POINTER;
    }
    if *class != CLSID {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    let factory: IUnknown = Factory {
        _guard: ObjectGuard::new(),
    }
    .into();
    factory.query(iid, output)
}

#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    if OBJECTS.load(Ordering::SeqCst) == 0 && LOCKS.load(Ordering::SeqCst) == 0 {
        S_OK
    } else {
        S_FALSE
    }
}
