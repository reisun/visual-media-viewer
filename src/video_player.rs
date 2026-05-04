use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use ffmpeg_next::codec::packet::traits::{Mut as PacketMut, Ref as PacketRef};

pub struct VideoFrame {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub pts: f64,
}

enum Command {
    Stop,
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
    base_pts_bits: AtomicU64,
    samples_consumed: AtomicU64,
    active: AtomicBool,
    latency_bits: AtomicU64,
    channels: u16,
    sample_rate: u32,
}

impl AudioClock {
    fn new(channels: u16, sample_rate: u32) -> Self {
        Self {
            base_pts_bits: AtomicU64::new(0f64.to_bits()),
            samples_consumed: AtomicU64::new(0),
            active: AtomicBool::new(false),
            latency_bits: AtomicU64::new(0f64.to_bits()),
            channels,
            sample_rate,
        }
    }

    fn set_base(&self, pts: f64) {
        self.base_pts_bits.store(pts.to_bits(), Ordering::Relaxed);
        self.samples_consumed.store(0, Ordering::Relaxed);
        self.active.store(true, Ordering::Relaxed);
    }

    fn advance(&self, count: u64) {
        self.samples_consumed.fetch_add(count, Ordering::Relaxed);
    }

    fn set_latency(&self, latency_secs: f64) {
        self.latency_bits.store(latency_secs.to_bits(), Ordering::Relaxed);
    }

    fn get(&self) -> Option<f64> {
        if self.active.load(Ordering::Relaxed) {
            let base_pts = f64::from_bits(self.base_pts_bits.load(Ordering::Relaxed));
            let consumed = self.samples_consumed.load(Ordering::Relaxed);
            let elapsed = consumed as f64 / (self.channels as f64 * self.sample_rate as f64);
            let latency = f64::from_bits(self.latency_bits.load(Ordering::Relaxed));
            Some(base_pts + elapsed - latency)
        } else {
            None
        }
    }
}

struct AudioChunk {
    samples: Vec<f32>,
    pts: f64,
}

struct VideoPacketData {
    data: Vec<u8>,
    pts: i64,
    dts: i64,
    flags: i32,
}

/// Active playback session (destroyed and recreated on each play_from)
struct PlaybackSession {
    frame_rx: mpsc::Receiver<VideoFrame>,
    cmd_tx: mpsc::Sender<Command>,
    demuxer_thread: thread::JoinHandle<()>,
    video_thread: thread::JoinHandle<()>,
    audio_stream: Option<cpal::Stream>,
    audio_clock: Arc<AudioClock>,
    audio_paused: Arc<AtomicBool>,
    stop_flag: Arc<AtomicBool>,
}

pub struct VideoPlayer {
    path: PathBuf,
    pub state: PlaybackState,
    pub duration: f64,
    pub video_size: [u32; 2],
    pub diag_info: String,
    has_audio: bool,
    volume: Arc<AtomicU16>,
    session: Option<PlaybackSession>,
    current_frame: Option<VideoFrame>,
    buffered_frame: Option<VideoFrame>,
    prebuffer_queue: std::collections::VecDeque<VideoFrame>,
    awaiting_first_frame: bool,
    prebuffer_deadline: Option<Instant>,
    playback_start: Instant,
    playback_start_pts: f64,
    paused_elapsed: f64,
}

impl VideoPlayer {
    pub fn open(path: &Path) -> Result<Self, String> {
        ffmpeg_next::init().map_err(|e| format!("FFmpeg init failed: {}", e))?;

        let (duration, video_size, diag_info, has_audio) = Self::probe(path)?;

        let volume = Arc::new(AtomicU16::new(100));
        let mut player = Self {
            path: path.to_path_buf(),
            state: PlaybackState::Playing,
            duration,
            video_size,
            diag_info,
            has_audio,
            volume,
            session: None,
            current_frame: None,
            buffered_frame: None,
            prebuffer_queue: std::collections::VecDeque::new(),
            awaiting_first_frame: true,
            prebuffer_deadline: None,
            playback_start: Instant::now(),
            playback_start_pts: 0.0,
            paused_elapsed: 0.0,
        };

        player.play_from(0.0);
        Ok(player)
    }

    fn probe(path: &Path) -> Result<(f64, [u32; 2], String, bool), String> {
        let mut opts = ffmpeg_next::Dictionary::new();
        opts.set("probesize", "500000");
        opts.set("analyzeduration", "500000");
        let ictx = ffmpeg_next::format::input_with_dictionary(&path, opts)
            .map_err(|e| format!("Cannot open {}: {}", path.display(), e))?;

        let video_stream = ictx
            .streams()
            .best(ffmpeg_next::media::Type::Video)
            .ok_or("No video stream found")?;
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
        let ctx = ffmpeg_next::codec::context::Context::from_parameters(video_codec_par)
            .map_err(|e| format!("Video codec context failed: {}", e))?;
        let dec = ctx.decoder().video().map_err(|e| format!("Video decoder failed: {}", e))?;
        let width = dec.width();
        let height = dec.height();

        let video_info = format!(
            "Video: codec_id={:?} {}x{} tb={}/{} duration={:.1}s",
            video_codec_id, width, height, tb.0, tb.1, duration
        );
        log::info!("{}", video_info);

        let has_audio = ictx.streams().best(ffmpeg_next::media::Type::Audio).is_some();
        let mut audio_info_str = String::new();
        if let Some(s) = ictx.streams().best(ffmpeg_next::media::Type::Audio) {
            let atb = s.time_base();
            let audio_time_base = atb.0 as f64 / atb.1 as f64;
            let par = s.parameters();
            let audio_codec_id = unsafe { (*par.as_ptr()).codec_id };
            audio_info_str = format!("Audio: codec_id={:?} tb={}", audio_codec_id, audio_time_base);
            log::info!("{}", audio_info_str);
        }
        let diag_info = format!("{}\n{}", video_info, audio_info_str);

        Ok((duration, [width, height], diag_info, has_audio))
    }

    /// Start (or restart) playback from the given position.
    /// Destroys any existing session and creates a fresh one.
    pub fn play_from(&mut self, position: f64) {
        log::info!("[player] play_from position={:.3}", position);
        // 1. Destroy existing session completely
        self.stop_session();

        // 2. Reset all display state
        self.current_frame = None;
        self.buffered_frame = None;
        self.prebuffer_queue.clear();
        self.awaiting_first_frame = true;
        self.prebuffer_deadline = None;
        self.playback_start = Instant::now();
        self.playback_start_pts = position;
        self.paused_elapsed = 0.0;
        self.state = PlaybackState::Playing;

        // 3. Create new session
        match self.create_session(position) {
            Ok(session) => self.session = Some(session),
            Err(e) => {
                log::error!("play_from failed: {}", e);
                self.state = PlaybackState::Finished;
            }
        }
    }

    fn stop_session(&mut self) {
        if let Some(session) = self.session.take() {
            log::info!("[player] stop_session: sending Stop command");
            session.stop_flag.store(true, Ordering::Release);
            let _ = session.cmd_tx.send(Command::Stop);
            drop(session.audio_stream);
            // Drop frame_rx to unblock video thread's blocking send
            drop(session.frame_rx);
            match session.video_thread.join() {
                Ok(()) => log::info!("[player] video thread joined normally"),
                Err(e) => log::error!("[player] video thread panicked: {:?}", e),
            }
            match session.demuxer_thread.join() {
                Ok(()) => log::info!("[player] demuxer thread joined normally"),
                Err(e) => log::error!("[player] demuxer thread panicked: {:?}", e),
            }
        }
    }

    fn create_session(&self, position: f64) -> Result<PlaybackSession, String> {
        let mut opts = ffmpeg_next::Dictionary::new();
        opts.set("probesize", "500000");
        opts.set("analyzeduration", "500000");
        let mut ictx = ffmpeg_next::format::input_with_dictionary(&self.path, opts)
            .map_err(|e| format!("Cannot open: {}", e))?;

        let video_stream = ictx
            .streams()
            .best(ffmpeg_next::media::Type::Video)
            .ok_or("No video stream")?;
        let video_idx = video_stream.index();
        let tb = video_stream.time_base();
        let video_time_base = tb.0 as f64 / tb.1 as f64;

        let video_codec_par = video_stream.parameters();
        let mut video_decoder_ctx =
            ffmpeg_next::codec::context::Context::from_parameters(video_codec_par)
                .map_err(|e| format!("Video codec ctx: {}", e))?;
        video_decoder_ctx.set_threading(ffmpeg_next::threading::Config {
            kind: ffmpeg_next::threading::Type::Frame,
            count: 0,
        });
        let video_decoder = video_decoder_ctx
            .decoder()
            .video()
            .map_err(|e| format!("Video decoder: {}", e))?;

        let src_w = video_decoder.width();
        let src_h = video_decoder.height();

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

        let (audio_idx, audio_time_base, audio_decoder) = match audio_info {
            Some((idx, atb, par)) => {
                let ctx = ffmpeg_next::codec::context::Context::from_parameters(par).ok();
                let dec = ctx.and_then(|c| c.decoder().audio().ok());
                match dec {
                    Some(d) => (Some(idx), atb, Some(d)),
                    None => (None, 0.0, None),
                }
            }
            None => (None, 0.0, None),
        };

        // Seek to position
        if position > 0.0 {
            let ts = (position * 1_000_000.0) as i64;
            let _ = ictx.seek(ts, ..ts);
        }

        // Audio output
        let audio_paused = Arc::new(AtomicBool::new(true));
        let volume_clone = Arc::clone(&self.volume);

        let mut device_sample_rate: u32 = 48000;
        let mut device_channels: u16 = 2;
        let mut audio_sample_tx: Option<mpsc::SyncSender<AudioChunk>> = None;
        let mut audio_stream: Option<cpal::Stream> = None;

        if audio_decoder.is_some() {
            if let Ok((_device, _config, sr, ch)) = get_audio_device_info() {
                device_sample_rate = sr;
                device_channels = ch;
            }
        }

        let audio_clock = Arc::new(AudioClock::new(device_channels, device_sample_rate));

        if audio_decoder.is_some() {
            if let Ok((device, config, _sr, ch)) = get_audio_device_info() {
                match setup_audio_output(&device, config, volume_clone, Arc::clone(&audio_paused), Arc::clone(&audio_clock), ch) {
                    Ok((tx, stream)) => {
                        audio_sample_tx = Some(tx);
                        audio_stream = Some(stream);
                    }
                    Err(e) => log::warn!("Audio output setup failed: {}", e),
                }
            }
        }

        let (frame_tx, frame_rx) = mpsc::sync_channel(4);
        let (cmd_tx, cmd_rx) = mpsc::channel();

        let audio_clock_for_video = Arc::clone(&audio_clock);
        let audio_clock_for_demuxer = Arc::clone(&audio_clock);
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_flag_for_video = Arc::clone(&stop_flag);
        let stop_flag_for_demuxer = Arc::clone(&stop_flag);
        let seek_target = position;

        let (video_pkt_tx, video_pkt_rx) = mpsc::sync_channel::<VideoPacketData>(150);

        // Thread 2: Video decoder (blocks on frame_tx.send — does NOT affect audio)
        let video_thread = thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                video_decoder_loop(
                    video_decoder, src_w, src_h, video_time_base,
                    video_pkt_rx, frame_tx, audio_clock_for_video, seek_target,
                    stop_flag_for_video,
                );
            }));
            if let Err(e) = result {
                let msg = if let Some(s) = e.downcast_ref::<String>() {
                    s.clone()
                } else if let Some(s) = e.downcast_ref::<&str>() {
                    s.to_string()
                } else {
                    format!("{:?}", e)
                };
                log::error!("[video] PANIC: {}", msg);
            }
        });

        let demuxer_thread = thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                demuxer_audio_loop(
                    video_idx, video_time_base,
                    audio_idx, audio_time_base, audio_decoder,
                    ictx, video_pkt_tx, audio_sample_tx, cmd_rx,
                    device_sample_rate, device_channels, audio_clock_for_demuxer, seek_target,
                    stop_flag_for_demuxer,
                );
            }));
            if let Err(e) = result {
                let msg = if let Some(s) = e.downcast_ref::<String>() {
                    s.clone()
                } else if let Some(s) = e.downcast_ref::<&str>() {
                    s.to_string()
                } else {
                    format!("{:?}", e)
                };
                log::error!("[demuxer] PANIC: {}", msg);
            }
        });

        Ok(PlaybackSession {
            frame_rx,
            cmd_tx,
            demuxer_thread,
            video_thread,
            audio_stream,
            audio_clock,
            audio_paused,
            stop_flag,
        })
    }

    const PREBUFFER_FRAMES: usize = 8;
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
        // Start audio
        if let Some(ref session) = self.session {
            session.audio_paused.store(false, Ordering::Release);
            if let Some(ref stream) = session.audio_stream {
                let _ = stream.play();
            }
        }
    }

    fn wall_clock(&self) -> f64 {
        self.paused_elapsed + self.playback_start.elapsed().as_secs_f64() + self.playback_start_pts
    }

    fn clock(&self) -> f64 {
        if let Some(ref session) = self.session {
            if self.has_audio {
                if let Some(apts) = session.audio_clock.get() {
                    let wall = self.wall_clock();
                    if wall - apts < 0.5 {
                        return apts;
                    }
                }
            }
        }
        self.wall_clock()
    }

    pub fn poll_frame(&mut self, av_offset_secs: f64) -> Option<&VideoFrame> {
        if !matches!(self.state, PlaybackState::Playing) {
            return self.current_frame.as_ref();
        }

        if self.awaiting_first_frame {
            let mut disconnected = false;
            if let Some(ref session) = self.session {
                loop {
                    match session.frame_rx.try_recv() {
                        Ok(frame) => self.prebuffer_queue.push_back(frame),
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            log::warn!("[poll_frame] frame_rx disconnected during prebuffer, queue_len={}", self.prebuffer_queue.len());
                            disconnected = true;
                            break;
                        }
                    }
                }
            }
            if disconnected && self.prebuffer_queue.is_empty() {
                log::error!("[poll_frame] decoder disconnected with empty prebuffer -> Finished");
                self.state = PlaybackState::Finished;
                return None;
            }
            if !self.prebuffer_queue.is_empty() {
                self.begin_prebuffer();
            }
            if self.prebuffer_ready() || disconnected {
                log::info!("[poll_frame] prebuffer done: queue_len={} disconnected={}", self.prebuffer_queue.len(), disconnected);
                self.finish_prebuffer();
                self.current_frame = self.prebuffer_queue.pop_front();
                self.buffered_frame = self.prebuffer_queue.pop_front();
            } else {
                return None;
            }
        }

        let clock = self.clock() + av_offset_secs;

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
            let rx = match self.session {
                Some(ref s) => &s.frame_rx,
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
                    let current_pts = self.current_frame.as_ref().map(|f| f.pts).unwrap_or(-1.0);
                    log::error!("[poll_frame] frame_rx disconnected -> Finished (clock={:.3} current_pts={:.3} duration={:.3})", clock, current_pts, self.duration);
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
        self.play_from(target_secs);
        Ok(())
    }

    pub fn toggle_pause(&mut self) {
        log::info!("[player] toggle_pause current_state={}", match self.state { PlaybackState::Playing => "Playing", PlaybackState::Paused => "Paused", PlaybackState::Finished => "Finished" });
        match self.state {
            PlaybackState::Playing => {
                self.state = PlaybackState::Paused;
                if let Some(ref session) = self.session {
                    session.audio_paused.store(true, Ordering::Relaxed);
                    if let Some(ref stream) = session.audio_stream {
                        let _ = stream.pause();
                    }
                }
                self.paused_elapsed += self.playback_start.elapsed().as_secs_f64();
            }
            PlaybackState::Paused => {
                self.state = PlaybackState::Playing;
                if let Some(ref session) = self.session {
                    session.audio_paused.store(false, Ordering::Relaxed);
                    if let Some(ref stream) = session.audio_stream {
                        let _ = stream.play();
                    }
                }
                self.playback_start = Instant::now();
            }
            PlaybackState::Finished => {
                self.play_from(0.0);
            }
        }
    }

    pub fn volume(&self) -> u16 {
        self.volume.load(Ordering::Relaxed)
    }

    pub fn set_volume(&self, vol: u16) {
        self.volume.store(vol.min(200), Ordering::Relaxed);
    }

    pub fn stop(&mut self) {
        self.stop_session();
        self.state = PlaybackState::Finished;
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        self.stop_session();
    }
}

// --- Audio output ---

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

fn get_audio_device_info() -> Result<(cpal::Device, cpal::SupportedStreamConfig, u32, u16), String> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or("No audio output device")?;
    let config = device.default_output_config().map_err(|e| format!("Audio config error: {}", e))?;
    let sample_rate = config.sample_rate().0;
    let channels = config.channels();
    Ok((device, config, sample_rate, channels))
}

fn setup_audio_output(
    device: &cpal::Device,
    config: cpal::SupportedStreamConfig,
    volume: Arc<AtomicU16>,
    paused: Arc<AtomicBool>,
    audio_clock: Arc<AudioClock>,
    channels: u16,
) -> Result<(mpsc::SyncSender<AudioChunk>, cpal::Stream), String> {
    let (tx, rx) = mpsc::sync_channel::<AudioChunk>(256);

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => {
            build_audio_stream::<f32>(device, &config.into(), rx, volume, paused, audio_clock, channels)?
        }
        cpal::SampleFormat::I16 => {
            build_audio_stream::<i16>(device, &config.into(), rx, volume, paused, audio_clock, channels)?
        }
        cpal::SampleFormat::U16 => {
            build_audio_stream::<u16>(device, &config.into(), rx, volume, paused, audio_clock, channels)?
        }
        _ => return Err("Unsupported audio sample format".to_string()),
    };

    Ok((tx, stream))
}

fn build_audio_stream<T: cpal::SizedSample + cpal::FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    rx: mpsc::Receiver<AudioChunk>,
    volume: Arc<AtomicU16>,
    paused: Arc<AtomicBool>,
    audio_clock: Arc<AudioClock>,
    channels: u16,
) -> Result<cpal::Stream, String> {
    let mut buffer: Vec<f32> = Vec::new();
    let mut buf_pos: usize = 0;
    let mut base_set = false;
    let samples_per_frame = channels as usize;
    let sample_rate_f = config.sample_rate.0 as f64;
    let mut latency_set = false;

    let stream = device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                if !latency_set {
                    let buffer_frames = data.len() / samples_per_frame;
                    let latency_secs = buffer_frames as f64 / sample_rate_f * 2.0;
                    audio_clock.set_latency(latency_secs);
                    latency_set = true;
                }

                if paused.load(Ordering::Relaxed) {
                    for sample in data.iter_mut() {
                        *sample = T::from_sample(0.0f32);
                    }
                    return;
                }

                let linear = volume.load(Ordering::Relaxed) as f32 / 100.0;
                let vol = linear * linear;

                for sample in data.iter_mut() {
                    if buf_pos >= buffer.len() {
                        match rx.try_recv() {
                            Ok(chunk) => {
                                if !base_set {
                                    audio_clock.set_base(chunk.pts);
                                    base_set = true;
                                }
                                buffer = chunk.samples;
                                buf_pos = 0;
                            }
                            Err(mpsc::TryRecvError::Empty) => {
                                *sample = T::from_sample(0.0f32);
                                continue;
                            }
                            Err(mpsc::TryRecvError::Disconnected) => {
                                audio_clock.active.store(false, Ordering::Release);
                                *sample = T::from_sample(0.0f32);
                                continue;
                            }
                        }
                    }

                    *sample = T::from_sample(buffer[buf_pos] * vol);
                    buf_pos += 1;
                    audio_clock.advance(1);
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

// --- Decoder ---

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

// --- Thread 1: Demuxer + Audio Decoder ---
// Reads packets from container, decodes audio inline, forwards video packets.
// Bounded channels provide backpressure — blocks when downstream is full.
fn demuxer_audio_loop(
    video_idx: usize,
    _video_time_base: f64,
    audio_idx: Option<usize>,
    audio_time_base: f64,
    audio_decoder: Option<ffmpeg_next::decoder::Audio>,
    mut ictx: ffmpeg_next::format::context::Input,
    video_pkt_tx: mpsc::SyncSender<VideoPacketData>,
    audio_tx: Option<mpsc::SyncSender<AudioChunk>>,
    cmd_rx: mpsc::Receiver<Command>,
    device_sample_rate: u32,
    device_channels: u16,
    audio_clock: Arc<AudioClock>,
    seek_target: f64,
    stop_flag: Arc<AtomicBool>,
) {
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
            adec.format(), adec.channel_layout(), adec.rate(),
            out_format, out_layout, device_sample_rate,
        );
        match r {
            Ok(ctx) => resampler = Some(ctx),
            Err(e) => log::warn!("Resampler creation failed: {}", e),
        }
    }

    let mut audio_frame = ffmpeg_next::frame::Audio::empty();
    let mut audio_seek_target: Option<f64> = if seek_target > 0.1 { Some(seek_target) } else { None };
    let resampled_channels = if device_channels >= 2 { 2 } else { 1 };

    let mut audio_chunks_sent: u64 = 0;
    let mut video_packets_sent: u64 = 0;
    let mut last_audio_pts: f64 = 0.0;
    let mut last_log_time = Instant::now();
    let mut read_errors: u64 = 0;

    log::info!("[demuxer] start: video_idx={} audio_idx={:?} seek={:.3}", video_idx, audio_idx, seek_target);

    let mut packet = ffmpeg_next::Packet::empty();
    loop {
        if stop_flag.load(Ordering::Relaxed) {
            log::info!("[demuxer] stop flag set, exiting");
            break;
        }
        if let Ok(Command::Stop) = cmd_rx.try_recv() {
            log::info!("[demuxer] stop command received");
            break;
        }

        if last_log_time.elapsed().as_secs() >= 2 {
            let clock = audio_clock.get().unwrap_or(seek_target);
            log::info!("[demuxer] stats: a_sent={} v_pkt_sent={} last_apts={:.3} clock={:.3} read_err={}", audio_chunks_sent, video_packets_sent, last_audio_pts, clock, read_errors);
            last_log_time = Instant::now();
        }

        match packet.read(&mut ictx) {
            Ok(..) => {}
            Err(ffmpeg_next::Error::Eof) => {
                log::info!("[demuxer] EOF (a_sent={} v_pkt_sent={} last_apts={:.3})", audio_chunks_sent, video_packets_sent, last_audio_pts);
                break;
            }
            Err(e) => {
                read_errors += 1;
                if read_errors <= 5 || read_errors % 100 == 0 {
                    log::warn!("[demuxer] packet.read error #{}: {}", read_errors, e);
                }
                continue;
            }
        }

        let stream_idx = packet.stream();

        if stream_idx == video_idx {
            let (data, pts_raw, dts, flags) = unsafe {
                let p = packet.as_ptr();
                let data_ptr = (*p).data;
                let data_size = (*p).size as usize;
                if data_ptr.is_null() || data_size == 0 {
                    continue;
                }
                (
                    std::slice::from_raw_parts(data_ptr, data_size).to_vec(),
                    (*p).pts,
                    (*p).dts,
                    (*p).flags,
                )
            };
            let mut vpd = VideoPacketData { data, pts: pts_raw, dts, flags };
            loop {
                if stop_flag.load(Ordering::Relaxed) { return; }
                match video_pkt_tx.try_send(vpd) {
                    Ok(()) => { video_packets_sent += 1; break; }
                    Err(mpsc::TrySendError::Full(returned)) => {
                        vpd = returned;
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(mpsc::TrySendError::Disconnected(_)) => {
                        log::info!("[demuxer] video_pkt_tx disconnected, exiting");
                        return;
                    }
                }
            }
        } else if Some(stream_idx) == audio_idx {
            if let Some(ref mut adec) = audio_decoder {
                if adec.send_packet(&packet).is_err() { continue; }
                while adec.receive_frame(&mut audio_frame).is_ok() {
                    let raw_apts = unsafe { (*audio_frame.as_ptr()).best_effort_timestamp };
                    let apts = if raw_apts != ffmpeg_next::ffi::AV_NOPTS_VALUE {
                        raw_apts as f64 * audio_time_base
                    } else {
                        audio_frame.pts().unwrap_or(0) as f64 * audio_time_base
                    };
                    if let Some(target) = audio_seek_target {
                        if apts < target - 0.05 { continue; }
                        audio_seek_target = None;
                    }
                    if let (Some(ref mut resampler_ctx), Some(ref atx)) = (&mut resampler, &audio_tx) {
                        let mut resampled = ffmpeg_next::frame::Audio::empty();
                        if resampler_ctx.run(&audio_frame, &mut resampled).is_ok() {
                            let samples = extract_f32_samples(&resampled, resampled_channels);
                            if !samples.is_empty() {
                                let adapted = adapt_channels(&samples, resampled_channels, device_channels);
                                let mut chunk = AudioChunk { samples: adapted, pts: apts };
                                loop {
                                    if stop_flag.load(Ordering::Relaxed) { return; }
                                    match atx.try_send(chunk) {
                                        Ok(()) => {
                                            audio_chunks_sent += 1;
                                            last_audio_pts = apts;
                                            break;
                                        }
                                        Err(mpsc::TrySendError::Full(returned)) => {
                                            chunk = returned;
                                            thread::sleep(Duration::from_millis(5));
                                        }
                                        Err(mpsc::TrySendError::Disconnected(_)) => {
                                            log::error!("[demuxer] audio_tx disconnected at apts={:.3}", apts);
                                            return;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Flush audio decoder
    if let Some(ref mut adec) = audio_decoder {
        let _ = adec.send_eof();
        while adec.receive_frame(&mut audio_frame).is_ok() {
            if let (Some(ref mut resampler_ctx), Some(ref atx)) = (&mut resampler, &audio_tx) {
                let mut resampled = ffmpeg_next::frame::Audio::empty();
                if resampler_ctx.run(&audio_frame, &mut resampled).is_ok() {
                    let samples = extract_f32_samples(&resampled, resampled_channels);
                    if !samples.is_empty() {
                        let adapted = adapt_channels(&samples, resampled_channels, device_channels);
                        let apts = last_audio_pts;
                        let _ = atx.try_send(AudioChunk { samples: adapted, pts: apts });
                    }
                }
            }
        }
    }

    log::info!("[demuxer] thread exiting (a_sent={} v_pkt_sent={})", audio_chunks_sent, video_packets_sent);
}

// --- Thread 2: Video Decoder ---
// Receives compressed video packets, decodes, scales, sends RGBA frames.
// Blocks on frame_tx.send() — this does NOT affect audio production.
fn video_decoder_loop(
    mut video_decoder: ffmpeg_next::decoder::Video,
    src_w: u32,
    src_h: u32,
    video_time_base: f64,
    video_pkt_rx: mpsc::Receiver<VideoPacketData>,
    frame_tx: mpsc::SyncSender<VideoFrame>,
    audio_clock: Arc<AudioClock>,
    seek_target: f64,
    stop_flag: Arc<AtomicBool>,
) {
    let (out_w, out_h) = fit_dimensions(src_w, src_h, 1920, 1080);

    let scaler_result = ffmpeg_next::software::scaling::Context::get(
        video_decoder.format(), src_w, src_h,
        ffmpeg_next::format::Pixel::RGBA, out_w, out_h,
        ffmpeg_next::software::scaling::Flags::FAST_BILINEAR,
    );
    let mut scaler = match scaler_result {
        Ok(s) => s,
        Err(e) => {
            log::error!("[video] scaler creation failed: {}", e);
            return;
        }
    };

    let mut decoded_frame = ffmpeg_next::frame::Video::empty();
    let mut rgba_frame = ffmpeg_next::frame::Video::empty();
    let frame_buf_size = (out_w as usize) * (out_h as usize) * 4;
    let mut reuse_buf: Vec<u8> = Vec::with_capacity(frame_buf_size);

    let mut video_seek_target: Option<f64> = if seek_target > 0.1 { Some(seek_target) } else { None };
    let mut video_frames_sent: u64 = 0;
    let mut video_frames_dropped: u64 = 0;
    let mut video_started = false;
    let mut last_video_pts: f64 = 0.0;
    let mut last_log_time = Instant::now();
    let mut packets_received: u64 = 0;

    log::info!("[video] start: out={}x{} fmt={:?} seek={:.3}", out_w, out_h, video_decoder.format(), seek_target);

    loop {
        if stop_flag.load(Ordering::Relaxed) {
            log::info!("[video] stop flag set, exiting");
            return;
        }
        let vpd = match video_pkt_rx.recv() {
            Ok(vpd) => vpd,
            Err(_) => {
                log::info!("[video] video_pkt_rx disconnected (EOF from demuxer), pkts_recv={} frames_sent={}", packets_received, video_frames_sent);
                break;
            }
        };
        packets_received += 1;
        if packets_received == 1 {
            log::info!("[video] first packet received: size={} pts={} dts={} flags={}", vpd.data.len(), vpd.pts, vpd.dts, vpd.flags);
        }

        // Reconstruct packet from transferred data
        let mut pkt = ffmpeg_next::Packet::empty();
        unsafe {
            let ptr = pkt.as_mut_ptr();
            ffmpeg_next::ffi::av_new_packet(ptr, vpd.data.len() as i32);
            std::ptr::copy_nonoverlapping(vpd.data.as_ptr(), (*ptr).data, vpd.data.len());
            (*ptr).pts = vpd.pts;
            (*ptr).dts = vpd.dts;
            (*ptr).flags = vpd.flags;
        }
        if let Err(e) = video_decoder.send_packet(&pkt) {
            if !video_started {
                log::warn!("[video] send_packet failed (pts={}): {}", vpd.pts, e);
            }
            continue;
        }

        while video_decoder.receive_frame(&mut decoded_frame).is_ok() {
            let raw_pts = unsafe { (*decoded_frame.as_ptr()).best_effort_timestamp };
            let pts = if raw_pts != ffmpeg_next::ffi::AV_NOPTS_VALUE {
                raw_pts as f64 * video_time_base
            } else {
                decoded_frame.pts().unwrap_or(0) as f64 * video_time_base
            };
            if let Some(target) = video_seek_target {
                if pts < target - 0.05 { continue; }
                video_seek_target = None;
            }
            if let Some(apts) = audio_clock.get() {
                if apts > 0.5 && pts < apts - 0.15 {
                    video_frames_dropped += 1;
                    continue;
                }
            }
            if scaler.run(&decoded_frame, &mut rgba_frame).is_err() { continue; }
            let (width, height) = copy_rgba_frame(&rgba_frame, &mut reuse_buf);
            let vframe = VideoFrame { rgba: reuse_buf.clone(), width, height, pts };
            if !video_started {
                video_started = true;
                log::info!("[video] first frame decoded at pts={:.3}", pts);
            }
            // Blocking send: back-pressure stops only this thread, not audio
            match frame_tx.send(vframe) {
                Ok(()) => {
                    video_frames_sent += 1;
                    last_video_pts = pts;
                }
                Err(_) => {
                    log::info!("[video] frame_tx disconnected at vpts={:.3}", pts);
                    return;
                }
            }
        }

        if last_log_time.elapsed().as_secs() >= 2 {
            log::info!("[video] stats: pkts={} v_sent={} v_drop={} last_vpts={:.3}", packets_received, video_frames_sent, video_frames_dropped, last_video_pts);
            last_log_time = Instant::now();
        }
    }

    // Flush decoder on EOF
    log::info!("[video] flushing decoder");
    let _ = video_decoder.send_eof();
    while video_decoder.receive_frame(&mut decoded_frame).is_ok() {
        if scaler.run(&decoded_frame, &mut rgba_frame).is_ok() {
            let (width, height) = copy_rgba_frame(&rgba_frame, &mut reuse_buf);
            let pts = decoded_frame.pts().unwrap_or(0) as f64 * video_time_base;
            let _ = frame_tx.send(VideoFrame { rgba: reuse_buf.clone(), width, height, pts });
        }
    }
    log::info!("[video] thread exiting (v_sent={} v_drop={})", video_frames_sent, video_frames_dropped);
}

fn extract_f32_samples(frame: &ffmpeg_next::frame::Audio, expected_channels: u16) -> Vec<f32> {
    let ch = expected_channels as usize;
    // Read nb_samples directly from AVFrame — frame.samples() may be unreliable
    // in ffmpeg-next 7 after resampling.
    let nb_samples = unsafe { (*frame.as_ptr()).nb_samples } as usize;
    if nb_samples == 0 {
        return Vec::new();
    }
    let sample_count = nb_samples * ch;
    let byte_len = sample_count * 4;
    // Read from raw data pointer to bypass potential slice length issues
    let data_ptr = unsafe { (*frame.as_ptr()).data[0] };
    if data_ptr.is_null() {
        return Vec::new();
    }
    let mut samples = vec![0.0f32; sample_count];
    unsafe {
        std::ptr::copy_nonoverlapping(
            data_ptr,
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
