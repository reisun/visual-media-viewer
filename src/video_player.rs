use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
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

pub struct VideoPlayer {
    frame_rx: Option<mpsc::Receiver<VideoFrame>>,
    cmd_tx: mpsc::Sender<Command>,
    thread: Option<thread::JoinHandle<()>>,
    pub state: PlaybackState,
    pub duration: f64,
    pub video_size: [u32; 2],
    current_frame: Option<VideoFrame>,
    buffered_frame: Option<VideoFrame>,
    playback_start: Instant,
    playback_start_pts: f64,
    paused_elapsed: f64,
    awaiting_seek_frame: bool,
    volume: Arc<AtomicU16>,
    audio_paused: Arc<AtomicBool>,
    audio_flush: Arc<AtomicBool>,
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
        let video_decoder_ctx =
            ffmpeg_next::codec::context::Context::from_parameters(video_codec_par)
                .map_err(|e| format!("Video codec context failed: {}", e))?;
        let video_decoder = video_decoder_ctx
            .decoder()
            .video()
            .map_err(|e| format!("Video decoder failed: {}", e))?;

        let width = video_decoder.width();
        let height = video_decoder.height();

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

        let volume = Arc::new(AtomicU16::new(100));
        let audio_paused = Arc::new(AtomicBool::new(false));
        let audio_flush = Arc::new(AtomicBool::new(false));

        let (audio_sample_tx, audio_stream) = if audio_decoder.is_some() {
            match setup_audio_output(
                Arc::clone(&volume),
                Arc::clone(&audio_paused),
                Arc::clone(&audio_flush),
            ) {
                Ok((tx, stream)) => (Some(tx), Some(stream)),
                Err(e) => {
                    log::warn!("Audio output setup failed: {}", e);
                    (None, None)
                }
            }
        } else {
            (None, None)
        };

        let (frame_tx, frame_rx) = mpsc::sync_channel(4);
        let (cmd_tx, cmd_rx) = mpsc::channel();

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
            );
        });

        Ok(Self {
            frame_rx: Some(frame_rx),
            cmd_tx,
            thread: Some(thread),
            state: PlaybackState::Playing,
            duration,
            video_size: [width, height],
            current_frame: None,
            buffered_frame: None,
            playback_start: Instant::now(),
            playback_start_pts: 0.0,
            paused_elapsed: 0.0,
            awaiting_seek_frame: false,
            volume,
            audio_paused,
            audio_flush,
            _audio_stream: audio_stream,
        })
    }

    pub fn poll_frame(&mut self) -> Option<&VideoFrame> {
        if !matches!(self.state, PlaybackState::Playing) {
            return self.current_frame.as_ref();
        }

        let elapsed = self.playback_start.elapsed().as_secs_f64() - self.paused_elapsed
            + self.playback_start_pts;

        loop {
            if let Some(ref buffered) = self.buffered_frame {
                if buffered.pts <= elapsed + 0.005 {
                    let was_awaiting = self.awaiting_seek_frame;
                    self.current_frame = self.buffered_frame.take();
                    if was_awaiting {
                        self.awaiting_seek_frame = false;
                        self.audio_flush.store(false, Ordering::Relaxed);
                        if let Some(ref frame) = self.current_frame {
                            self.playback_start = Instant::now();
                            self.playback_start_pts = frame.pts;
                            self.paused_elapsed = 0.0;
                        }
                    }
                } else {
                    break;
                }
            }

            let rx = match &self.frame_rx {
                Some(rx) => rx,
                None => break,
            };
            match rx.try_recv() {
                Ok(frame) => {
                    if frame.pts <= elapsed + 0.005 {
                        let was_awaiting = self.awaiting_seek_frame;
                        self.current_frame = Some(frame);
                        if was_awaiting {
                            self.awaiting_seek_frame = false;
                            self.audio_flush.store(false, Ordering::Relaxed);
                            if let Some(ref frame) = self.current_frame {
                                self.playback_start = Instant::now();
                                self.playback_start_pts = frame.pts;
                                self.paused_elapsed = 0.0;
                            }
                        }
                    } else {
                        if self.awaiting_seek_frame {
                            self.awaiting_seek_frame = false;
                            self.audio_flush.store(false, Ordering::Relaxed);
                            self.current_frame = Some(frame);
                            if let Some(ref frame) = self.current_frame {
                                self.playback_start = Instant::now();
                                self.playback_start_pts = frame.pts;
                                self.paused_elapsed = 0.0;
                            }
                        } else {
                            self.buffered_frame = Some(frame);
                        }
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

    pub fn current_pts(&self) -> f64 {
        if let Some(ref frame) = self.current_frame {
            return frame.pts;
        }
        self.playback_start.elapsed().as_secs_f64() - self.paused_elapsed
            + self.playback_start_pts
    }

    pub fn seek(&mut self, target_secs: f64) -> Result<(), SeekResult> {
        if target_secs < 0.0 {
            return Err(SeekResult::BeforeStart);
        }
        if target_secs >= self.duration && self.duration > 0.0 {
            return Err(SeekResult::PastEnd);
        }
        self.audio_flush.store(true, Ordering::Relaxed);
        let _ = self.cmd_tx.send(Command::Seek(target_secs));
        self.current_frame = None;
        self.buffered_frame = None;
        if let Some(ref rx) = self.frame_rx {
            while rx.try_recv().is_ok() {}
        }
        self.awaiting_seek_frame = true;
        self.playback_start = Instant::now();
        self.playback_start_pts = target_secs;
        self.paused_elapsed = 0.0;
        if matches!(self.state, PlaybackState::Finished) {
            self.state = PlaybackState::Playing;
            self.audio_paused.store(false, Ordering::Relaxed);
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
) -> Result<(mpsc::SyncSender<Vec<f32>>, cpal::Stream), String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("No audio output device")?;
    let config = device
        .default_output_config()
        .map_err(|e| format!("Audio config error: {}", e))?;

    let sample_rate = config.sample_rate().0;
    let channels = config.channels() as usize;

    let (tx, rx) = mpsc::sync_channel::<Vec<f32>>(4);

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => build_audio_stream::<f32>(
            &device, &config.into(), rx, volume, paused, flush, sample_rate, channels,
        )?,
        cpal::SampleFormat::I16 => build_audio_stream::<i16>(
            &device, &config.into(), rx, volume, paused, flush, sample_rate, channels,
        )?,
        cpal::SampleFormat::U16 => build_audio_stream::<u16>(
            &device, &config.into(), rx, volume, paused, flush, sample_rate, channels,
        )?,
        _ => return Err("Unsupported audio sample format".to_string()),
    };

    stream
        .play()
        .map_err(|e| format!("Audio stream play error: {}", e))?;

    Ok((tx, stream))
}

fn build_audio_stream<T: cpal::SizedSample + cpal::FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    rx: mpsc::Receiver<Vec<f32>>,
    volume: Arc<AtomicU16>,
    paused: Arc<AtomicBool>,
    flush: Arc<AtomicBool>,
    _sample_rate: u32,
    _channels: usize,
) -> Result<cpal::Stream, String> {
    let mut buffer: Vec<f32> = Vec::new();
    let mut buf_pos: usize = 0;

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
                            Ok(new_buf) => {
                                buffer = new_buf;
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

fn decoder_loop(
    video_idx: usize,
    video_time_base: f64,
    mut video_decoder: ffmpeg_next::decoder::Video,
    audio_idx: Option<usize>,
    audio_time_base: f64,
    audio_decoder: Option<ffmpeg_next::decoder::Audio>,
    mut ictx: ffmpeg_next::format::context::Input,
    frame_tx: mpsc::SyncSender<VideoFrame>,
    audio_tx: Option<mpsc::SyncSender<Vec<f32>>>,
    cmd_rx: mpsc::Receiver<Command>,
) {
    let scaler_result = ffmpeg_next::software::scaling::Context::get(
        video_decoder.format(),
        video_decoder.width(),
        video_decoder.height(),
        ffmpeg_next::format::Pixel::RGBA,
        video_decoder.width(),
        video_decoder.height(),
        ffmpeg_next::software::scaling::Flags::BILINEAR,
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
        let out_rate = 48000;
        let out_layout = ffmpeg_next::ChannelLayout::STEREO;
        let out_format =
            ffmpeg_next::format::Sample::F32(ffmpeg_next::format::sample::Type::Packed);

        let r = ffmpeg_next::software::resampling::Context::get(
            adec.format(),
            adec.channel_layout(),
            adec.rate(),
            out_format,
            out_layout,
            out_rate,
        );
        match r {
            Ok(ctx) => resampler = Some(ctx),
            Err(e) => log::warn!("Resampler creation failed: {}", e),
        }
    }

    let mut decoded_frame = ffmpeg_next::frame::Video::empty();
    let mut rgba_frame = ffmpeg_next::frame::Video::empty();
    let mut audio_frame = ffmpeg_next::frame::Audio::empty();

    let frame_buf_size =
        (video_decoder.width() as usize) * (video_decoder.height() as usize) * 4;
    let mut reuse_buf: Vec<u8> = Vec::with_capacity(frame_buf_size);

    let mut seek_target: Option<f64> = None;

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
                let pts = decoded_frame.pts().unwrap_or(0) as f64 * video_time_base;
                if let Some(target) = seek_target {
                    if pts < target - 0.05 {
                        continue;
                    }
                    seek_target = None;
                }
                if scaler.run(&decoded_frame, &mut rgba_frame).is_err() {
                    continue;
                }
                let width = rgba_frame.width();
                let height = rgba_frame.height();
                let stride = rgba_frame.stride(0);
                let row_bytes = width as usize * 4;
                let data = rgba_frame.data(0);
                reuse_buf.clear();
                if stride == row_bytes {
                    reuse_buf.extend_from_slice(&data[..row_bytes * height as usize]);
                } else {
                    for y in 0..height as usize {
                        let row_start = y * stride;
                        reuse_buf.extend_from_slice(&data[row_start..row_start + row_bytes]);
                    }
                }
                if frame_tx
                    .send(VideoFrame {
                        rgba: reuse_buf.clone(),
                        width,
                        height,
                        pts,
                    })
                    .is_err()
                {
                    return;
                }
            }
        } else if Some(stream_idx) == audio_idx {
            if let Some(ref mut adec) = audio_decoder {
                if adec.send_packet(&packet).is_err() {
                    continue;
                }
                while adec.receive_frame(&mut audio_frame).is_ok() {
                    if let Some(target) = seek_target {
                        let apts = audio_frame.pts().unwrap_or(0) as f64 * audio_time_base;
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
                                let _ = atx.send(samples);
                            }
                        }
                    }
                }
            }
        }
    }

    // Flush video
    let _ = video_decoder.send_eof();
    while video_decoder.receive_frame(&mut decoded_frame).is_ok() {
        if scaler.run(&decoded_frame, &mut rgba_frame).is_ok() {
            let width = rgba_frame.width();
            let height = rgba_frame.height();
            let stride = rgba_frame.stride(0);
            let row_bytes = width as usize * 4;
            let data = rgba_frame.data(0);
            reuse_buf.clear();
            if stride == row_bytes {
                reuse_buf.extend_from_slice(&data[..row_bytes * height as usize]);
            } else {
                for y in 0..height as usize {
                    let row_start = y * stride;
                    reuse_buf.extend_from_slice(&data[row_start..row_start + row_bytes]);
                }
            }
            let pts = decoded_frame.pts().unwrap_or(0) as f64 * video_time_base;
            let _ = frame_tx.send(VideoFrame {
                rgba: reuse_buf.clone(),
                width,
                height,
                pts,
            });
        }
    }

    // Flush audio
    if let Some(ref mut adec) = audio_decoder {
        let _ = adec.send_eof();
        while adec.receive_frame(&mut audio_frame).is_ok() {
            if let (Some(ref mut resampler), Some(ref atx)) = (&mut resampler, &audio_tx) {
                let mut resampled = ffmpeg_next::frame::Audio::empty();
                if resampler.run(&audio_frame, &mut resampled).is_ok() {
                    let samples = extract_f32_samples(&resampled);
                    if !samples.is_empty() {
                        let _ = atx.send(samples);
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
