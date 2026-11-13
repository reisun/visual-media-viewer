//! Isolated thumbnail decoding. The parent serves Explorer's stream over the pipes.
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::time::{Duration, Instant};

const MAX_READ: usize = 1024 * 1024;
const MAX_SOURCE: u64 = 128 * 1024 * 1024;
const MAX_DECODE: u64 = 256 * 1024 * 1024;

struct RemoteStream<R, W> {
    input: R,
    output: W,
    remaining: u64,
    deadline: Instant,
    size: Option<u64>,
}

impl<R: Read, W: Write> RemoteStream<R, W> {
    fn check_budget(&self) -> io::Result<()> {
        if Instant::now() >= self.deadline || self.remaining == 0 {
            return Err(io::Error::other("Thumbnail decode budget exhausted"));
        }
        Ok(())
    }

    fn size(&mut self) -> io::Result<u64> {
        self.check_budget()?;
        if let Some(size) = self.size {
            return Ok(size);
        }
        self.output.write_all(&[3])?;
        self.output.flush()?;
        let mut value = [0; 8];
        self.input.read_exact(&mut value)?;
        let value = u64::from_le_bytes(value);
        if value == u64::MAX {
            return Err(io::Error::other("Stream size unavailable"));
        }
        self.size = Some(value);
        Ok(value)
    }
}

impl<R: Read, W: Write> Read for RemoteStream<R, W> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        self.check_budget()?;
        let count = bytes.len().min(MAX_READ).min(self.remaining as usize);
        self.output.write_all(&[1])?;
        self.output.write_all(&(count as u32).to_le_bytes())?;
        self.output.flush()?;
        let mut length = [0; 4];
        self.input.read_exact(&mut length)?;
        let actual = u32::from_le_bytes(length) as usize;
        if actual > count {
            return Err(io::Error::other("Invalid stream read response"));
        }
        self.input.read_exact(&mut bytes[..actual])?;
        self.remaining -= actual as u64;
        Ok(actual)
    }
}

impl<R: Read, W: Write> Seek for RemoteStream<R, W> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.check_budget()?;
        let (offset, origin) = match position {
            SeekFrom::Start(offset) => (i64::try_from(offset).map_err(io::Error::other)?, 0u32),
            SeekFrom::Current(offset) => (offset, 1),
            SeekFrom::End(offset) => (offset, 2),
        };
        self.output.write_all(&[2])?;
        self.output.write_all(&offset.to_le_bytes())?;
        self.output.write_all(&origin.to_le_bytes())?;
        self.output.flush()?;
        let mut value = [0; 8];
        self.input.read_exact(&mut value)?;
        let value = u64::from_le_bytes(value);
        if value == u64::MAX {
            return Err(io::Error::other("Stream seek failed"));
        }
        Ok(value)
    }
}

type Thumbnail = (u32, u32, Vec<u8>);

pub fn run(size: u32) -> Result<(), String> {
    let mut source = RemoteStream {
        input: io::stdin().lock(),
        output: io::stdout().lock(),
        remaining: MAX_DECODE,
        deadline: Instant::now() + Duration::from_secs(8),
        size: None,
    };
    let result = decode(&mut source, size.clamp(1, 1024));
    match result {
        Ok((width, height, pixels)) => {
            source.output.write_all(&[4]).map_err(|e| e.to_string())?;
            source
                .output
                .write_all(&width.to_le_bytes())
                .map_err(|e| e.to_string())?;
            source
                .output
                .write_all(&height.to_le_bytes())
                .map_err(|e| e.to_string())?;
            source
                .output
                .write_all(&pixels)
                .map_err(|e| e.to_string())?;
            source.output.flush().map_err(|e| e.to_string())
        }
        Err(error) => {
            let _ = source.output.write_all(&[5]);
            let _ = source.output.flush();
            Err(error)
        }
    }
}

fn decode<R: Read, W: Write>(
    source: &mut RemoteStream<R, W>,
    size: u32,
) -> Result<Thumbnail, String> {
    let mut signature = [0; 32];
    source.rewind().map_err(|e| e.to_string())?;
    let mut count = 0;
    while count < signature.len() {
        let actual = source
            .read(&mut signature[count..])
            .map_err(|e| e.to_string())?;
        if actual == 0 {
            break;
        }
        count += actual;
    }
    source.rewind().map_err(|e| e.to_string())?;
    if image::guess_format(&signature[..count]).is_ok() {
        if signature.starts_with(&[0xff, 0xd8]) && source.size().unwrap_or(u64::MAX) <= MAX_SOURCE {
            let bytes = read_image_source(source)?;
            if let Ok(thumbnail) = jpeg_thumbnail(&bytes, size) {
                return Ok(thumbnail);
            }
            source.rewind().map_err(|e| e.to_string())?;
        }
        let mut reader = image::ImageReader::new(BufReader::with_capacity(256 * 1024, source))
            .with_guessed_format()
            .map_err(|e| e.to_string())?;
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(MAX_SOURCE);
        limits.max_image_width = Some(32768);
        limits.max_image_height = Some(32768);
        reader.limits(limits);
        let image = reader.decode().map_err(|e| e.to_string())?;
        let rgba = image.thumbnail(size, size).into_rgba8();
        return Ok((rgba.width(), rgba.height(), rgba.into_raw()));
    }
    #[cfg(windows)]
    if count >= 12
        && &signature[4..8] == b"ftyp"
        && matches!(
            &signature[8..12],
            b"heic" | b"heix" | b"hevc" | b"hevx" | b"mif1" | b"msf1" | b"avif"
        )
    {
        if source.size().map_err(|e| e.to_string())? > MAX_SOURCE {
            return Err("Image source exceeds size limit".into());
        }
        let bytes = read_image_source(source)?;
        return crate::wic_decoder::decode_thumbnail_from_bytes(&bytes, size);
    }
    video_thumbnail(source, size)
}

fn read_image_source<R: Read, W: Write>(
    source: &mut RemoteStream<R, W>,
) -> Result<Vec<u8>, String> {
    let capacity = source.size().map_err(|e| e.to_string())?;
    if capacity > MAX_SOURCE {
        return Err("Image source exceeds size limit".into());
    }
    // Fixed-size reads avoid read_to_end's small initial requests across the pipe.
    let mut bytes = Vec::with_capacity(capacity as usize);
    let mut buffer = vec![0; MAX_READ];
    loop {
        let limit = buffer
            .len()
            .min((MAX_SOURCE + 1 - bytes.len() as u64) as usize);
        let count = source
            .read(&mut buffer[..limit])
            .map_err(|e| e.to_string())?;
        if count == 0 {
            return Ok(bytes);
        }
        bytes.extend_from_slice(&buffer[..count]);
        if bytes.len() as u64 > MAX_SOURCE {
            return Err("Image source exceeds size limit".into());
        }
    }
}

fn fit(width: u32, height: u32, size: u32) -> Result<(u32, u32), String> {
    if width == 0
        || height == 0
        || width > 32768
        || height > 32768
        || u64::from(width) * u64::from(height) > 64 * 1024 * 1024
    {
        return Err("Invalid or oversized frame".into());
    }
    let longest = width.max(height);
    if longest <= size {
        return Ok((width, height));
    }
    Ok((
        ((u64::from(width) * u64::from(size)) / u64::from(longest)).max(1) as u32,
        ((u64::from(height) * u64::from(size)) / u64::from(longest)).max(1) as u32,
    ))
}

fn jpeg_thumbnail(bytes: &[u8], size: u32) -> Result<Thumbnail, String> {
    let mut decoder = turbojpeg::Decompressor::new().map_err(|e| e.to_string())?;
    let header = decoder.read_header(bytes).map_err(|e| e.to_string())?;
    fit(header.width as u32, header.height as u32, size)?;
    let mut factor = turbojpeg::ScalingFactor::ONE;
    if !header.is_lossless {
        for candidate in [
            turbojpeg::ScalingFactor::ONE_HALF,
            turbojpeg::ScalingFactor::ONE_QUARTER,
            turbojpeg::ScalingFactor::ONE_EIGHTH,
        ] {
            if candidate.scale(header.width.max(header.height)) >= size as usize {
                factor = candidate;
            }
        }
    }
    decoder
        .set_scaling_factor(factor)
        .map_err(|e| e.to_string())?;
    let width = factor.scale(header.width);
    let height = factor.scale(header.height);
    let length = width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(4))
        .filter(|n| *n as u64 <= MAX_SOURCE)
        .ok_or("JPEG exceeds decode limit")?;
    let mut pixels = vec![0; length];
    decoder
        .decompress(
            bytes,
            turbojpeg::Image {
                pixels: &mut pixels,
                width,
                height,
                pitch: width * 4,
                format: turbojpeg::PixelFormat::RGBA,
            },
        )
        .map_err(|e| e.to_string())?;
    let rgba = image::RgbaImage::from_raw(width as u32, height as u32, pixels)
        .ok_or("Invalid JPEG frame")?;
    let rgba = image::DynamicImage::ImageRgba8(rgba)
        .thumbnail(size, size)
        .into_rgba8();
    Ok((rgba.width(), rgba.height(), rgba.into_raw()))
}

unsafe extern "C" fn read_packet<R: Read, W: Write>(
    opaque: *mut std::ffi::c_void,
    buffer: *mut u8,
    length: i32,
) -> i32 {
    if length <= 0 {
        return -5;
    }
    let source = &mut *(opaque as *mut RemoteStream<R, W>);
    match source.read(std::slice::from_raw_parts_mut(buffer, length as usize)) {
        Ok(0) => ffmpeg_next::ffi::AVERROR_EOF,
        Ok(count) => count as i32,
        Err(_) => -5,
    }
}

unsafe extern "C" fn seek_packet<R: Read, W: Write>(
    opaque: *mut std::ffi::c_void,
    offset: i64,
    whence: i32,
) -> i64 {
    let source = &mut *(opaque as *mut RemoteStream<R, W>);
    let result = if whence & ffmpeg_next::ffi::AVSEEK_SIZE != 0 {
        source.size()
    } else {
        match whence & !ffmpeg_next::ffi::AVSEEK_FORCE {
            0 if offset >= 0 => source.seek(SeekFrom::Start(offset as u64)),
            1 => source.seek(SeekFrom::Current(offset)),
            2 => source.seek(SeekFrom::End(offset)),
            _ => return -22,
        }
    };
    result
        .ok()
        .and_then(|n| i64::try_from(n).ok())
        .unwrap_or(-5)
}

struct CustomIo(*mut ffmpeg_next::ffi::AVIOContext);
impl Drop for CustomIo {
    fn drop(&mut self) {
        unsafe {
            ffmpeg_next::ffi::av_free((*self.0).buffer.cast());
            ffmpeg_next::ffi::avio_context_free(&mut self.0);
        }
    }
}

fn video_thumbnail<R: Read, W: Write>(
    source: &mut RemoteStream<R, W>,
    size: u32,
) -> Result<Thumbnail, String> {
    use ffmpeg_next::ffi as av;
    use ffmpeg_next::{codec, format, frame, media, software, Packet};
    ffmpeg_next::init().map_err(|e| e.to_string())?;
    unsafe {
        av::av_max_alloc(MAX_DECODE as usize);
        av::av_log_set_level(av::AV_LOG_QUIET);
    }
    let buffer = unsafe { av::av_malloc(32768).cast::<u8>() };
    if buffer.is_null() {
        return Err("AVIO allocation failed".into());
    }
    let io = unsafe {
        av::avio_alloc_context(
            buffer,
            32768,
            0,
            (source as *mut RemoteStream<R, W>).cast(),
            Some(read_packet::<R, W>),
            None,
            Some(seek_packet::<R, W>),
        )
    };
    if io.is_null() {
        unsafe {
            av::av_free(buffer.cast());
        }
        return Err("AVIO initialization failed".into());
    }
    let _io = CustomIo(io);
    let mut context = unsafe { av::avformat_alloc_context() };
    if context.is_null() {
        return Err("Format allocation failed".into());
    }
    unsafe {
        (*context).pb = io;
        (*context).flags |= av::AVFMT_FLAG_CUSTOM_IO;
        (*context).probesize = 1024 * 1024;
        (*context).max_analyze_duration = 1_000_000;
        let mut options = std::ptr::null_mut();
        av::av_dict_set(
            &mut options,
            c"protocol_whitelist".as_ptr(),
            c"none".as_ptr(),
            0,
        );
        av::av_dict_set(
            &mut options,
            c"format_whitelist".as_ptr(),
            c"mov,matroska,webm,avi,mpeg,mpegts,flv,asf,ogg".as_ptr(),
            0,
        );
        let status = av::avformat_open_input(
            &mut context,
            std::ptr::null(),
            std::ptr::null(),
            &mut options,
        );
        av::av_dict_free(&mut options);
        if status < 0 {
            if !context.is_null() {
                av::avformat_free_context(context);
            }
            return Err("Unsupported video stream".into());
        }
    }
    // Input owns the format context; its custom IO remains alive until Input drops.
    let mut input = unsafe { format::context::Input::wrap(context) };
    if !has_video_decode_metadata(&input)
        && unsafe { av::avformat_find_stream_info(context, std::ptr::null_mut()) } < 0
    {
        return Err("Video stream information unavailable".into());
    }
    let stream = input
        .streams()
        .best(media::Type::Video)
        .ok_or("No video stream")?;
    let index = stream.index();
    let mut decoder =
        codec::context::Context::from_parameters(stream.parameters()).map_err(|e| e.to_string())?;
    unsafe {
        (*decoder.as_mut_ptr()).max_pixels = 64 * 1024 * 1024;
    }
    decoder.set_threading(codec::threading::Config {
        kind: codec::threading::Type::Slice,
        count: 1,
        ..Default::default()
    });
    let mut decoder = decoder.decoder().video().map_err(|e| e.to_string())?;
    let mut packet = Packet::empty();
    let mut decoded = frame::Video::empty();
    for _ in 0..512 {
        source.check_budget().map_err(|e| e.to_string())?;
        let eof = match packet.read(&mut input) {
            Ok(()) => false,
            Err(ffmpeg_next::Error::Eof) => true,
            Err(_) => return Err("Video packet read failed".into()),
        };
        if eof {
            decoder.send_eof().map_err(|e| e.to_string())?;
        } else if packet.stream() == index {
            decoder.send_packet(&packet).map_err(|e| e.to_string())?;
        } else {
            continue;
        }
        if decoder.receive_frame(&mut decoded).is_ok() {
            let (width, height) = fit(decoded.width(), decoded.height(), size)?;
            let mut scaler = software::scaling::Context::get(
                decoded.format(),
                decoded.width(),
                decoded.height(),
                format::Pixel::RGBA,
                width,
                height,
                software::scaling::Flags::BILINEAR,
            )
            .map_err(|e| e.to_string())?;
            let mut rgba = frame::Video::empty();
            scaler.run(&decoded, &mut rgba).map_err(|e| e.to_string())?;
            let row = width as usize * 4;
            let mut pixels = vec![0; row * height as usize];
            for y in 0..height as usize {
                pixels[y * row..(y + 1) * row]
                    .copy_from_slice(&rgba.data(0)[y * rgba.stride(0)..y * rgba.stride(0) + row]);
            }
            return Ok((width, height, pixels));
        }
        if eof {
            break;
        }
    }
    Err("No decodable frame within thumbnail budget".into())
}

fn has_video_decode_metadata(input: &ffmpeg_next::format::context::Input) -> bool {
    use ffmpeg_next::ffi as av;
    // These containers carry complete codec setup in their headers. Other formats
    // keep the established analysis path, including MPEG-TS with in-band headers.
    if !input
        .format()
        .name()
        .split(',')
        .any(|name| matches!(name, "mov" | "matroska" | "webm"))
    {
        return false;
    }
    let Some(stream) = input.streams().best(ffmpeg_next::media::Type::Video) else {
        return false;
    };
    unsafe {
        let parameters = stream.parameters();
        let parameters = &*parameters.as_ptr();
        parameters.codec_type == av::AVMediaType::AVMEDIA_TYPE_VIDEO
            && parameters.codec_id != av::AVCodecID::AV_CODEC_ID_NONE
            && parameters.width > 0
            && parameters.height > 0
            && parameters.extradata_size > 0
            && !parameters.extradata.is_null()
            && !av::avcodec_find_decoder(parameters.codec_id).is_null()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    struct Peer {
        source: io::Cursor<Vec<u8>>,
        response: io::Cursor<Vec<u8>>,
        request: Vec<u8>,
        read_bytes: usize,
        read_requests: usize,
        size_requests: usize,
        chunk_limit: usize,
    }
    struct PipeReader(Rc<RefCell<Peer>>);
    struct PipeWriter(Rc<RefCell<Peer>>);
    impl Read for PipeReader {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            self.0.borrow_mut().response.read(bytes)
        }
    }
    impl Write for PipeWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().request.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            let mut peer = self.0.borrow_mut();
            let request = std::mem::take(&mut peer.request);
            let mut response = Vec::new();
            match request[0] {
                1 => {
                    peer.read_requests += 1;
                    let count = u32::from_le_bytes(request[1..5].try_into().unwrap()) as usize;
                    assert!(count <= MAX_READ);
                    let mut bytes = vec![0; count.min(peer.chunk_limit)];
                    let actual = peer.source.read(&mut bytes)?;
                    peer.read_bytes += actual;
                    response.extend_from_slice(&(actual as u32).to_le_bytes());
                    response.extend_from_slice(&bytes[..actual]);
                }
                2 => {
                    let offset = i64::from_le_bytes(request[1..9].try_into().unwrap());
                    let origin = u32::from_le_bytes(request[9..13].try_into().unwrap());
                    let position = match origin {
                        0 => SeekFrom::Start(offset as u64),
                        1 => SeekFrom::Current(offset),
                        2 => SeekFrom::End(offset),
                        _ => panic!("Invalid seek"),
                    };
                    response.extend_from_slice(&peer.source.seek(position)?.to_le_bytes());
                }
                3 => {
                    peer.size_requests += 1;
                    response.extend_from_slice(&(peer.source.get_ref().len() as u64).to_le_bytes())
                }
                _ => panic!("Invalid RPC"),
            }
            peer.response = io::Cursor::new(response);
            Ok(())
        }
    }
    fn remote(bytes: Vec<u8>) -> RemoteStream<PipeReader, PipeWriter> {
        let peer = Rc::new(RefCell::new(Peer {
            source: io::Cursor::new(bytes),
            response: io::Cursor::new(Vec::new()),
            request: Vec::new(),
            read_bytes: 0,
            read_requests: 0,
            size_requests: 0,
            chunk_limit: MAX_READ,
        }));
        RemoteStream {
            input: PipeReader(peer.clone()),
            output: PipeWriter(peer),
            remaining: MAX_DECODE,
            deadline: Instant::now() + Duration::from_secs(8),
            size: None,
        }
    }
    #[test]
    fn dimensions_are_bounded_without_upscaling() {
        assert_eq!(fit(3840, 2160, 256).unwrap(), (256, 144));
        assert_eq!(fit(10, 20, 256).unwrap(), (10, 20));
        assert_eq!(fit(32768, 1, 16).unwrap(), (16, 1));
        assert!(fit(0, 20, 256).is_err());
        assert!(fit(32768, 32768, 256).is_err());
    }
    #[test]
    fn remote_reads_reject_oversized_response() {
        let mut stream = RemoteStream {
            input: io::Cursor::new(8u32.to_le_bytes()),
            output: Vec::new(),
            remaining: 100,
            deadline: Instant::now() + Duration::from_secs(1),
            size: None,
        };
        assert!(stream.read(&mut [0; 4]).is_err());
        assert_eq!(stream.output, [1, 4, 0, 0, 0]);
    }
    #[test]
    fn remote_seeks_preserve_signed_offsets() {
        let mut stream = RemoteStream {
            input: io::Cursor::new(12u64.to_le_bytes()),
            output: Vec::new(),
            remaining: 100,
            deadline: Instant::now() + Duration::from_secs(1),
            size: None,
        };
        assert_eq!(stream.seek(SeekFrom::End(-4)).unwrap(), 12);
        assert_eq!(&stream.output[1..9], &(-4i64).to_le_bytes());
        assert_eq!(&stream.output[9..13], &2u32.to_le_bytes());
    }
    #[test]
    fn image_thumbnails_use_remote_stream_and_keep_alpha() {
        let image = image::RgbaImage::from_pixel(200, 100, image::Rgba([20, 40, 60, 80]));
        let mut bytes = io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let (width, height, pixels) = decode(&mut remote(bytes.into_inner()), 64).unwrap();
        assert_eq!((width, height), (64, 32));
        assert_eq!(&pixels[..4], &[20, 40, 60, 80]);
    }
    #[test]
    fn image_detection_handles_nonzero_position_and_short_reads() {
        let image = image::RgbImage::from_pixel(40, 20, image::Rgb([20, 40, 60]));
        let mut bytes = io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let mut source = remote(bytes.into_inner());
        source.input.0.borrow_mut().source.set_position(17);
        source.input.0.borrow_mut().chunk_limit = 2;
        let (width, height, _) = decode(&mut source, 20).unwrap();
        assert_eq!((width, height), (20, 10));
    }
    #[test]
    fn exhausted_stream_budget_fails_before_request() {
        let mut source = remote(vec![0; 8]);
        source.remaining = 0;
        assert!(source.read(&mut [0; 8]).is_err());
        assert!(source.input.0.borrow().request.is_empty());
    }
    #[test]
    fn jpeg_thumbnail_is_reduced_and_bounded() {
        let image = image::RgbImage::from_pixel(1200, 800, image::Rgb([20, 40, 60]));
        let mut bytes = io::Cursor::new(Vec::new());
        image
            .write_to(&mut bytes, image::ImageFormat::Jpeg)
            .unwrap();
        let mut source = remote(bytes.into_inner());
        let (width, height, pixels) = decode(&mut source, 120).unwrap();
        assert_eq!((width, height), (120, 80));
        assert_eq!(pixels.len(), 120 * 80 * 4);
        let peer = source.input.0.borrow();
        assert!(peer.read_requests <= 3, "{} reads", peer.read_requests);
        assert_eq!(peer.size_requests, 1);
    }
    #[test]
    fn bulk_image_reads_handle_short_reads_and_cache_size() {
        let expected = vec![42; 3 * MAX_READ + 17];
        let mut source = remote(expected.clone());
        assert_eq!(read_image_source(&mut source).unwrap(), expected);
        assert_eq!(source.size().unwrap(), expected.len() as u64);
        assert_eq!(source.input.0.borrow().read_requests, 5);
        assert_eq!(source.input.0.borrow().size_requests, 1);
        let mut source = remote(vec![1, 2, 3, 4, 5]);
        source.input.0.borrow_mut().chunk_limit = 2;
        assert_eq!(read_image_source(&mut source).unwrap(), [1, 2, 3, 4, 5]);
    }
    #[test]
    fn bulk_image_reads_reject_oversize_and_do_not_trust_small_reported_size() {
        let mut source = remote(Vec::new());
        source.size = Some(MAX_SOURCE + 1);
        assert!(read_image_source(&mut source).is_err());
        assert_eq!(source.input.0.borrow().read_requests, 0);
        // A stream whose declared size is too small is still read to EOF.
        let mut source = remote(vec![1; 123]);
        source.size = Some(1);
        assert_eq!(read_image_source(&mut source).unwrap().len(), 123);
    }
    #[test]
    fn mov_and_short_eof_frames_decode_through_custom_io() {
        let Ok(directory) = std::env::var("VMV_THUMBNAIL_FIXTURES") else {
            return;
        };
        for name in ["bframes.mov", "short.mp4", "fallback.ts"] {
            let bytes = std::fs::read(std::path::Path::new(&directory).join(name)).unwrap();
            let (width, height, pixels) = decode(&mut remote(bytes), 80).unwrap();
            assert_eq!((width, height), (80, 48), "{name}");
            assert_eq!(pixels.len(), 80 * 48 * 4);
            assert!(pixels.chunks_exact(4).any(|p| p[0] > 30 || p[1] > 30));
        }
    }
    #[test]
    fn header_complete_video_does_not_scan_whole_clip() {
        let Ok(directory) = std::env::var("VMV_THUMBNAIL_FIXTURES") else {
            return;
        };
        let bytes = std::fs::read(std::path::Path::new(&directory).join("faststart.mp4")).unwrap();
        let length = bytes.len();
        let mut source = remote(bytes);
        let (width, height, _) = decode(&mut source, 80).unwrap();
        assert_eq!((width, height), (80, 45));
        let peer = source.input.0.borrow();
        assert!(
            peer.read_bytes < length / 2,
            "read {} / {length} bytes",
            peer.read_bytes
        );
        assert_eq!(peer.size_requests, 1);
    }
    #[test]
    fn unsupported_and_audio_only_inputs_fail_cleanly() {
        assert!(decode(&mut remote(b"not a media file".to_vec()), 256).is_err());
        assert!(decode(
            &mut remote(b"#EXTM3U\nhttp://example.invalid/video.ts\n".to_vec()),
            256
        )
        .is_err());
        if let Ok(directory) = std::env::var("VMV_THUMBNAIL_FIXTURES") {
            let bytes = std::fs::read(std::path::Path::new(&directory).join("audio.m4a")).unwrap();
            assert!(decode(&mut remote(bytes), 256).is_err());
        }
    }
}
