use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Instant;

pub struct VideoFrame {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub pts: f64,
}

enum Command {
    Stop,
    Seek(f64),
}

pub enum PlaybackState {
    Playing,
    Paused,
    Finished,
}

pub enum SeekResult {
    BeforeStart,
    PastEnd,
}

struct AudioClock {
    /// Current audio playback PTS (f64 bits stored as u64)
    pts_bits: AtomicU64,
    /// Whether audio clock is valid (audio has started playing)
    active: AtomicBool,
}

impl AudioClock {
    fn new() -> Self {
        Self {
            pts_bits: AtomicU64::new(0u64),
            active: AtomicBool::new(false),
        }
    }

    fn set(&self, pts: f64) {
        self.pts_bits.store(pts.to_bits(), Ordering::Relaxed);
        self.active.store(true, Ordering::Relaxed);
    }

    fn get(&self) -> Option<f64> {
        if self.active.load(Ordering::Relaxed) {
            Some(f64::from_bits(self.pts_bits.load(Ordering::Relaxed)))
        } else {
            None
        }
    }

    fn reset(&self) {
        self.active.store(false, Ordering::Relaxed);
    }
}

/// Audio sample chunk with PTS for clock tracking
struct AudioChunk {
    samples: Vec<f32>,
    /// PTS at the start of this chunk
    pts: f64,
    /// Duration of this chunk in seconds
    duration: f64,
}

pub struct VideoPlayer {
    frame_rx: Option<mpsc::Receiver<VideoFrame>>,
    cmd_tx: mpsc::Sender<Command>,
    thread: Option<thread::JoinHandle<()>>,
    pub state: PlaybackState,
    pub duration: f64,
    pub video_size: [u32; 2],
    pub diag_info: String,
    current_frame: Option<VideoFrame>,
    buffered_frame: Option<VideoFrame>,
    prebuffer_queue: std::collections::VecDeque<VideoFrame>,
    awaiting_first_frame: bool,
    prebuffer_deadline: Option<Instant>,
    /// Fallback wall clock for video-only files or before audio starts
    playback_start: Instant,
    playback_start_pts: f64,
    paused_elapsed: f64,
    /// Audio-master clock: updated by cpal callback
    audio_clock: Arc<AudioClock>,
    volume: Arc<AtomicU16>,
    audio_paused: Arc<AtomicBool>,
    audio_flush: Arc<AtomicBool>,
    has_audio: bool,
    _audio_stream: Option<cpal::Stream>,
}

impl VideoPlayer {
    pub fn open(path: &Path) -> Result<Self, String> {
        ffmpeg_next::init().map_err(|e| format!("FFmpeg init failed: {}", e))?;

        let mut opts = ffmpeg_next::Dictionary::new();
        opts.set("probesize", "500000");
        opts.set("analyzeduration", "500000");

        let ictx = ffmpeg_next::format::input_with_dictionary(&path, opts)
            .map_err(|e| format!("Cannot open {}: {}", path.display(), e))?;

        let video_stream = ictx
            .streams()
            .best(ffmpeg_next::media::Type::Video)
            .ok_or("No video stream found")?;
        let video_idx = video_stream.index();
        let tb = video_stream.time_base();
        let video_time_base = tb.0 as f64 / tb.1 as f64;
        let stream_duration = video_stream.duration();
        let duration = if stream_duration > 0 {
            stream_duration as f64 * video_time_base
        } else {
            ictx.duration() as f64 / f64::from(ffmpeg_next::ffi::AV_TIME_BASE)
        };

        let video_codec_par = video_stream.parameters();
        let video_codec_id = unsafe { (*video_codec_par.as_ptr()).codec_id };
        let mut video_decoder_ctx =
            ffmpeg_next::codec::context::Context::from_parameters(video_codec_par)
                .map_err(|e| format!("Video codec context failed: {}", e))?;
        video_decoder_ctx.set_threading(ffmpeg_next::threading::Config {
            kind: ffmpeg_next::threading::Type::Frame,
            count: 0,
        });
        let video_decoder = video_decoder_ctx
            .decoder()
            .video()
            .map_err(|e| format!("Video decoder failed: {}", e))?;

        let width = video_decoder.width();
        let height = video_decoder.height();

        let video_info = format!(
            "Video: codec_id={:?} {}x{} tb={}/{} duration={:.1}s",
            video_codec_id, width, height, tb.0, tb.1, duration
        );
        log::info!("{}", video_info);

        let audio_info = ictx
            .streams()
            .best(ffmpeg_next::media::Type::Audio)
            .map(|s| {
                let idx = s.index();
                let atb = s.time_base();
                let audio_time_base = atb.0 as f64 / atb.1 as f64;
                let par = s.parameters();
                (idx, audio_time_base, par)
            });

        let mut audio_info_str = String::new();
        let (audio_idx, audio_time_base, audio_decoder) = match audio_info {
            Some((idx, atb, par)) => {
                let audio_codec_id = unsafe { (*par.as_ptr()).codec_id };
                audio_info_str = format!("Audio: codec_id={:?} tb={}", audio_codec_id, atb);
                log::info!("{}", audio_info_str);
                let ctx = ffmpeg_next::codec::context::Context::from_parameters(par).ok();
                let dec = ctx.and_then(|c| c.decoder().audio().ok());
                match dec {
                    Some(d) => (Some(idx), atb, Some(d)),
                    None => (None, 0.0, None),
                }
            }
            None => (None, 0.0, None),
        };
        let diag_info = format!("{}\n{}", video_info, audio_info_str);

        let volume = Arc::new(AtomicU16::new(100));
        let audio_paused = Arc::new(AtomicBool::new(false));
        let audio_flush = Arc::new(AtomicBool::new(true));
        let audio_clock = Arc::new(AudioClock::new());

        let has_audio = audio_decoder.is_some();

        let (audio_sample_tx, audio_stream, device_sample_rate, device_channels) =
            if audio_decoder.is_some() {
                match setup_audio_output(
                    Arc::clone(&volume),
                    Arc::clone(&audio_paused),
                    Arc::clone(&audio_flush),
                    Arc::clone(&audio_clock),
                ) {
                    Ok((tx, stream, sr, ch)) => (Some(tx), Some(stream), sr, ch),
                    Err(e) => {
                        log::warn!("Audio output setup failed: {}", e);
                        (None, None, 48000, 2)
                    }
                }
            } else {
                (None, None, 48000, 2)
            };

        let (frame_tx, frame_rx) = mpsc::sync_channel(32);
        let (cmd_tx, cmd_rx) = mpsc::channel();

        let audio_clock_clone = Arc::clone(&audio_clock);
        let thread = thread::spawn(move || {
            decoder_loop(
                video_idx,
                video_time_base,
                video_decoder,
                audio_idx,
                audio_time_base,
                audio_decoder,
                ictx,
                frame_tx,
                audio_sample_tx,
                cmd_rx,
                device_sample_rate,
                device_channels,
                audio_clock_clone,
            );
        });

        Ok(Self {
            frame_rx: Some(frame_rx),
            cmd_tx,
            thread: Some(thread),
            state: PlaybackState::Playing,
            duration,
            video_size: [width, height],
            diag_info,
            current_frame: None,
            buffered_frame: None,
            prebuffer_queue: std::collections::VecDeque::new(),
            awaiting_first_frame: true,
            prebuffer_deadline: None,
            playback_start: Instant::now(),
            playback_start_pts: 0.0,
            paused_elapsed: 0.0,
            audio_clock,
            volume,
            audio_paused,
            audio_flush,
            has_audio,
            _audio_stream: audio_stream,
        })
    }

    const PREBUFFER_FRAMES: usize = 64;
    const PREBUFFER_TIMEOUT_MS: u64 = 2000;

    fn begin_prebuffer(&mut self) {
        if self.prebuffer_deadline.is_some() {
            return;
        }
        self.prebuffer_deadline =
            Some(Instant::now() + std::time::Duration::from_millis(Self::PREBUFFER_TIMEOUT_MS));
    }

    fn prebuffer_ready(&self) -> bool {
        if let Some(deadline) = self.prebuffer_deadline {
            self.prebuffer_queue.len() >= Self::PREBUFFER_FRAMES || Instant::now() >= deadline
        } else {
            false
        }
    }

    fn finish_prebuffer(&mut self) {
        self.awaiting_first_frame = false;
        self.prebuffer_deadline = None;
        if let Some(first) = self.prebuffer_queue.front() {
            self.playback_start_pts = first.pts;
        }
        self.playback_start = Instant::now();
        self.paused_elapsed = 0.0;
        // Start audio: stop flushing so cpal plays queued samples
        self.audio_flush.store(false, Ordering::Relaxed);
        self.audio_paused.store(false, Ordering::Relaxed);
    }

    /// Get the current playback clock position.
    /// Uses audio clock as master when available, falls back to wall clock.
    fn clock(&self) -> f64 {
        if self.has_audio {
            if let Some(apts) = self.audio_clock.get() {
                return apts;
            }
        }
        // Fallback: wall clock (for video-only files or before audio starts)
        self.playback_start.elapsed().as_secs_f64() - self.paused_elapsed
            + self.playback_start_pts
    }

    pub fn poll_frame(&mut self, _av_offset_secs: f64) -> Option<&VideoFrame> {
        if !matches!(self.state, PlaybackState::Playing) {
            return self.current_frame.as_ref();
        }

        // Pre-buffering phase: accumulate frames before starting playback
        if self.awaiting_first_frame {
            let mut disconnected = false;
            if let Some(ref rx) = self.frame_rx {
                loop {
                    match rx.try_recv() {
                        Ok(frame) => {
                            self.prebuffer_queue.push_back(frame);
                        }
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            disconnected = true;
                            break;
                        }
                    }
                }
            }
            if disconnected && self.prebuffer_queue.is_empty() {
                self.state = PlaybackState::Finished;
                return None;
            }
            if !self.prebuffer_queue.is_empty() {
                self.begin_prebuffer();
            }
            if self.prebuffer_ready() || disconnected {
                self.finish_prebuffer();
                self.current_frame = self.prebuffer_queue.pop_front();
                self.buffered_frame = self.prebuffer_queue.pop_front();
            } else {
                return None;
            }
        }

        let clock = self.clock();

        loop {
            if self.buffered_frame.is_none() {
                if let Some(queued) = self.prebuffer_queue.pop_front() {
                    self.buffered_frame = Some(queued);
                }
            }

            if let Some(ref buffered) = self.buffered_frame {
                if buffered.pts <= clock {
                    self.current_frame = self.buffered_frame.take();
                } else {
                    break;
                }
            }

            if !self.prebuffer_queue.is_empty() {
                continue;
            }
            let rx = match &self.frame_rx {
                Some(rx) => rx,
                None => break,
            };
            match rx.try_recv() {
                Ok(frame) => {
                    if frame.pts <= clock {
                        self.current_frame = Some(frame);
                    } else {
                        self.buffered_frame = Some(frame);
                        break;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.state = PlaybackState::Finished;
                    break;
                }
            }
        }

        self.current_frame.as_ref()
    }

    pub fn is_buffering(&self) -> bool {
        self.awaiting_first_frame
    }

    pub fn current_pts(&self) -> f64 {
        if self.awaiting_first_frame {
            return self.playback_start_pts;
        }
        if let Some(ref frame) = self.current_frame {
            return frame.pts;
        }
        self.clock()
    }

    pub fn seek(&mut self, target_secs: f64) -> Result<(), SeekResult> {
        if target_secs < 0.0 {
            return Err(SeekResult::BeforeStart);
        }
        if target_secs >= self.duration && self.duration > 0.0 {
            return Err(SeekResult::PastEnd);
        }
        // Stop audio immediately
        self.audio_paused.store(true, Ordering::Relaxed);
        self.audio_flush.store(true, Ordering::Relaxed);
        self.audio_clock.reset();
        let _ = self.cmd_tx.send(Command::Seek(target_secs));
        self.current_frame = None;
        self.buffered_frame = None;
        self.prebuffer_queue.clear();
        if let Some(ref rx) = self.frame_rx {
            while rx.try_recv().is_ok() {}
        }
        self.awaiting_first_frame = true;
        self.prebuffer_deadline = None;
        self.playback_start = Instant::now();
        self.playback_start_pts = target_secs;
        self.paused_elapsed = 0.0;
        if matches!(self.state, PlaybackState::Finished) {
            self.state = PlaybackState::Playing;
        }
        Ok(())
    }

    pub fn toggle_pause(&mut self) {
        match self.state {
            PlaybackState::Playing => {
                self.state = PlaybackState::Paused;
                self.audio_paused.store(true, Ordering::Relaxed);
                self.paused_elapsed =
                    self.playback_start.elapsed().as_secs_f64() - self.paused_elapsed;
            }
            PlaybackState::Paused => {
                self.state = PlaybackState::Playing;
                self.audio_paused.store(false, Ordering::Relaxed);
                self.playback_start = Instant::now();
            }
            PlaybackState::Finished => {}
        }
    }

    pub fn volume(&self) -> u16 {
        self.volume.load(Ordering::Relaxed)
    }

    pub fn set_volume(&self, vol: u16) {
        self.volume.store(vol.min(200), Ordering::Relaxed);
    }

    pub fn stop(&mut self) {
        let _ = self.cmd_tx.send(Command::Stop);
        self.frame_rx.take();
        self.buffered_frame.take();
        self._audio_stream.take();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
        self.state = PlaybackState::Finished;
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(Command::Stop);
        self.frame_rx.take();
        self._audio_stream.take();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

fn setup_audio_output(
    volume: Arc<AtomicU16>,
    paused: Arc<AtomicBool>,
    flush: Arc<AtomicBool>,
    audio_clock: Arc<AudioClock>,
) -> Result<(mpsc::SyncSender<AudioChunk>, cpal::Stream, u32, u16), String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("No audio output device")?;
    let config = device
        .default_output_config()
        .map_err(|e| format!("Audio config error: {}", e))?;

    let sample_rate = config.sample_rate().0;
    let channels = config.channels();

    let (tx, rx) = mpsc::sync_channel::<AudioChunk>(256);

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => build_audio_stream::<f32>(
            &device, &config.into(), rx, volume, paused, flush, audio_clock, sample_rate, channels,
        )?,
        cpal::SampleFormat::I16 => build_audio_stream::<i16>(
            &device, &config.into(), rx, volume, paused, flush, audio_clock, sample_rate, channels,
        )?,
        cpal::SampleFormat::U16 => build_audio_stream::<u16>(
            &device, &config.into(), rx, volume, paused, flush, audio_clock, sample_rate, channels,
        )?,
        _ => return Err("Unsupported audio sample format".to_string()),
    };

    stream
        .play()
        .map_err(|e| format!("Audio stream play error: {}", e))?;

    Ok((tx, stream, sample_rate, channels))
}

fn build_audio_stream<T: cpal::SizedSample + cpal::FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    rx: mpsc::Receiver<AudioChunk>,
    volume: Arc<AtomicU16>,
    paused: Arc<AtomicBool>,
    flush: Arc<AtomicBool>,
    audio_clock: Arc<AudioClock>,
    _sample_rate: u32,
    channels: u16,
) -> Result<cpal::Stream, String> {
    let mut buffer: Vec<f32> = Vec::new();
    let mut buf_pos: usize = 0;
    let mut current_chunk_pts: f64 = 0.0;
    let mut current_chunk_duration: f64 = 0.0;
    let mut current_chunk_samples: usize = 0;
    let samples_per_frame = channels as usize;

    let stream = device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                if flush.load(Ordering::Relaxed) {
                    while rx.try_recv().is_ok() {}
                    buffer.clear();
                    buf_pos = 0;
                    for sample in data.iter_mut() {
                        *sample = T::from_sample(0.0f32);
                    }
                    return;
                }

                let vol = volume.load(Ordering::Relaxed) as f32 / 100.0;
                let is_paused = paused.load(Ordering::Relaxed);

                for sample in data.iter_mut() {
                    if is_paused {
                        *sample = T::from_sample(0.0f32);
                        continue;
                    }

                    if buf_pos >= buffer.len() {
                        match rx.try_recv() {
                            Ok(chunk) => {
                                current_chunk_pts = chunk.pts;
                                current_chunk_duration = chunk.duration;
                                current_chunk_samples = chunk.samples.len();
                                buffer = chunk.samples;
                                buf_pos = 0;
                            }
                            Err(_) => {
                                *sample = T::from_sample(0.0f32);
                                continue;
                            }
                        }
                    }

                    if buf_pos < buffer.len() {
                        *sample = T::from_sample(buffer[buf_pos] * vol);
                        buf_pos += 1;

                        // Update audio clock based on position within current chunk
                        if buf_pos % (samples_per_frame * 256) == 0 || buf_pos >= buffer.len() {
                            let progress = if current_chunk_samples > 0 {
                                buf_pos as f64 / current_chunk_samples as f64
                            } else {
                                0.0
                            };
                            let pts = current_chunk_pts + current_chunk_duration * progress;
                            audio_clock.set(pts);
                        }
                    } else {
                        *sample = T::from_sample(0.0f32);
                    }
                }
            },
            |err| {
                log::error!("Audio stream error: {}", err);
            },
            None,
        )
        .map_err(|e| format!("Audio stream build error: {}", e))?;

    Ok(stream)
}

fn fit_dimensions(src_w: u32, src_h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if src_w <= max_w && src_h <= max_h {
        return (src_w, src_h);
    }
    let scale = (max_w as f64 / src_w as f64).min(max_h as f64 / src_h as f64);
    let w = ((src_w as f64 * scale) as u32).max(2) & !1;
    let h = ((src_h as f64 * scale) as u32).max(2) & !1;
    (w, h)
}

fn copy_rgba_frame(rgba_frame: &ffmpeg_next::frame::Video, buf: &mut Vec<u8>) -> (u32, u32) {
    let width = rgba_frame.width();
    let height = rgba_frame.height();
    let stride = rgba_frame.stride(0);
    let row_bytes = width as usize * 4;
    let data = rgba_frame.data(0);
    buf.clear();
    if stride == row_bytes {
        buf.extend_from_slice(&data[..row_bytes * height as usize]);
    } else {
        for y in 0..height as usize {
            let row_start = y * stride;
            buf.extend_from_slice(&data[row_start..row_start + row_bytes]);
        }
    }
    (width, height)
}

fn decoder_loop(
    video_idx: usize,
    video_time_base: f64,
    mut video_decoder: ffmpeg_next::decoder::Video,
    audio_idx: Option<usize>,
    audio_time_base: f64,
    audio_decoder: Option<ffmpeg_next::decoder::Audio>,
    mut ictx: ffmpeg_next::format::context::Input,
    frame_tx: mpsc::SyncSender<VideoFrame>,
    audio_tx: Option<mpsc::SyncSender<AudioChunk>>,
    cmd_rx: mpsc::Receiver<Command>,
    device_sample_rate: u32,
    device_channels: u16,
    audio_clock: Arc<AudioClock>,
) {
    let src_w = video_decoder.width();
    let src_h = video_decoder.height();
    let (out_w, out_h) = fit_dimensions(src_w, src_h, 1920, 1080);

    let scaler_result = ffmpeg_next::software::scaling::Context::get(
        video_decoder.format(),
        src_w,
        src_h,
        ffmpeg_next::format::Pixel::RGBA,
        out_w,
        out_h,
        ffmpeg_next::software::scaling::Flags::FAST_BILINEAR,
    );
    let mut scaler = match scaler_result {
        Ok(s) => s,
        Err(e) => {
            log::error!("scaler creation failed: {}", e);
            return;
        }
    };

    let mut audio_decoder = audio_decoder;
    let mut resampler: Option<ffmpeg_next::software::resampling::Context> = None;

    if let Some(ref adec) = audio_decoder {
        let out_layout = if device_channels >= 2 {
            ffmpeg_next::ChannelLayout::STEREO
        } else {
            ffmpeg_next::ChannelLayout::MONO
        };
        let out_format =
            ffmpeg_next::format::Sample::F32(ffmpeg_next::format::sample::Type::Packed);

        let r = ffmpeg_next::software::resampling::Context::get(
            adec.format(),
            adec.channel_layout(),
            adec.rate(),
            out_format,
            out_layout,
            device_sample_rate,
        );
        match r {
            Ok(ctx) => resampler = Some(ctx),
            Err(e) => log::warn!("Resampler creation failed: {}", e),
        }
    }

    let mut decoded_frame = ffmpeg_next::frame::Video::empty();
    let mut rgba_frame = ffmpeg_next::frame::Video::empty();
    let mut audio_frame = ffmpeg_next::frame::Audio::empty();

    let frame_buf_size = (out_w as usize) * (out_h as usize) * 4;
    let mut reuse_buf: Vec<u8> = Vec::with_capacity(frame_buf_size);

    let mut seek_target: Option<f64> = None;
    let mut seeking = true;

    let resampled_channels = if device_channels >= 2 { 2 } else { 1 };

    let mut packet = ffmpeg_next::Packet::empty();
    loop {
        match cmd_rx.try_recv() {
            Ok(Command::Stop) => break,
            Ok(Command::Seek(target)) => {
                let ts = (target * 1_000_000.0) as i64;
                let _ = ictx.seek(ts, ..ts);
                video_decoder.flush();
                if let Some(ref mut adec) = audio_decoder {
                    adec.flush();
                }
                seek_target = Some(target);
                seeking = true;
                continue;
            }
            Err(_) => {}
        }

        match packet.read(&mut ictx) {
            Ok(..) => {}
            Err(ffmpeg_next::Error::Eof) => break,
            Err(_) => continue,
        }

        let stream_idx = packet.stream();

        if stream_idx == video_idx {
            if video_decoder.send_packet(&packet).is_err() {
                continue;
            }
            while video_decoder.receive_frame(&mut decoded_frame).is_ok() {
                // Use best_effort_timestamp for B-frame reordering accuracy
                let raw_pts = unsafe {
                    (*decoded_frame.as_ptr()).best_effort_timestamp
                };
                let pts = if raw_pts != ffmpeg_next::ffi::AV_NOPTS_VALUE {
                    raw_pts as f64 * video_time_base
                } else {
                    decoded_frame.pts().unwrap_or(0) as f64 * video_time_base
                };
                if let Some(target) = seek_target {
                    if pts < target - 0.05 {
                        continue;
                    }
                    seek_target = None;
                }
                // Frame drop: if video is far behind audio clock, skip expensive scale+send
                // Grace period: only drop after audio clock is well-established (>0.5s)
                if let Some(apts) = audio_clock.get() {
                    if apts > 0.5 && pts < apts - 0.15 {
                        seeking = false;
                        continue;
                    }
                }
                if scaler.run(&decoded_frame, &mut rgba_frame).is_err() {
                    continue;
                }
                let (width, height) = copy_rgba_frame(&rgba_frame, &mut reuse_buf);
                let vframe = VideoFrame { rgba: reuse_buf.clone(), width, height, pts };
                if frame_tx.send(vframe).is_err() {
                    return;
                }
                seeking = false;
            }
        } else if Some(stream_idx) == audio_idx {
            if seeking {
                continue;
            }
            if let Some(ref mut adec) = audio_decoder {
                if adec.send_packet(&packet).is_err() {
                    continue;
                }
                while adec.receive_frame(&mut audio_frame).is_ok() {
                    let raw_apts = unsafe {
                        (*audio_frame.as_ptr()).best_effort_timestamp
                    };
                    let apts = if raw_apts != ffmpeg_next::ffi::AV_NOPTS_VALUE {
                        raw_apts as f64 * audio_time_base
                    } else {
                        audio_frame.pts().unwrap_or(0) as f64 * audio_time_base
                    };
                    if let Some(target) = seek_target {
                        if apts < target - 0.05 {
                            continue;
                        }
                    }
                    if let (Some(ref mut resampler), Some(ref atx)) = (&mut resampler, &audio_tx)
                    {
                        let mut resampled = ffmpeg_next::frame::Audio::empty();
                        if resampler.run(&audio_frame, &mut resampled).is_ok() {
                            let samples = extract_f32_samples(&resampled);
                            if !samples.is_empty() {
                                let adapted = adapt_channels(&samples, resampled_channels, device_channels);
                                let num_frames = adapted.len() / device_channels as usize;
                                let duration = num_frames as f64 / device_sample_rate as f64;
                                let chunk = AudioChunk { samples: adapted, pts: apts, duration };
                                let _ = atx.send(chunk);
                            }
                        }
                    }
                }
            }
        }
    }

    let _ = video_decoder.send_eof();
    while video_decoder.receive_frame(&mut decoded_frame).is_ok() {
        if scaler.run(&decoded_frame, &mut rgba_frame).is_ok() {
            let (width, height) = copy_rgba_frame(&rgba_frame, &mut reuse_buf);
            let pts = decoded_frame.pts().unwrap_or(0) as f64 * video_time_base;
            let _ = frame_tx.try_send(VideoFrame { rgba: reuse_buf.clone(), width, height, pts });
        }
    }

    if let Some(ref mut adec) = audio_decoder {
        let _ = adec.send_eof();
        while adec.receive_frame(&mut audio_frame).is_ok() {
            if let (Some(ref mut resampler), Some(ref atx)) = (&mut resampler, &audio_tx) {
                let mut resampled = ffmpeg_next::frame::Audio::empty();
                if resampler.run(&audio_frame, &mut resampled).is_ok() {
                    let samples = extract_f32_samples(&resampled);
                    if !samples.is_empty() {
                        let adapted = adapt_channels(&samples, resampled_channels, device_channels);
                        let pts = audio_frame.pts().unwrap_or(0) as f64 * audio_time_base;
                        let num_frames = adapted.len() / device_channels as usize;
                        let duration = num_frames as f64 / device_sample_rate as f64;
                        let chunk = AudioChunk { samples: adapted, pts, duration };
                        let _ = atx.send(chunk);
                    }
                }
            }
        }
    }
}

fn extract_f32_samples(frame: &ffmpeg_next::frame::Audio) -> Vec<f32> {
    let data = frame.data(0);
    let sample_count = frame.samples() * frame.channels() as usize;
    let byte_len = sample_count * 4;
    if data.len() < byte_len {
        return Vec::new();
    }
    let mut samples = vec![0.0f32; sample_count];
    unsafe {
        std::ptr::copy_nonoverlapping(
            data.as_ptr(),
            samples.as_mut_ptr() as *mut u8,
            byte_len,
        );
    }
    samples
}

fn adapt_channels(samples: &[f32], src_channels: u16, dst_channels: u16) -> Vec<f32> {
    if src_channels == dst_channels {
        return samples.to_vec();
    }
    let src_ch = src_channels as usize;
    let dst_ch = dst_channels as usize;
    let frame_count = samples.len() / src_ch;
    let mut out = Vec::with_capacity(frame_count * dst_ch);
    for i in 0..frame_count {
        let base = i * src_ch;
        for c in 0..dst_ch {
            if c < src_ch {
                out.push(samples[base + c]);
            } else {
                out.push(samples[base + src_ch - 1]);
            }
        }
    }
    out
}
