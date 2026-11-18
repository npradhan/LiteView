//! Video playback with sound, using FFmpeg for decoding and cpal for audio output.
//!
//! A decoder thread demuxes the file, converts video frames to RGBA and audio to
//! interleaved f32 at the output device's rate. The UI thread shows each frame when
//! the playback clock reaches its timestamp. The clock is wall time, continually
//! re-synced to the audio actually played, so picture and sound stay together.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, Once};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use egui::{Color32, ColorImage, TextureHandle, TextureOptions};
use ffmpeg_next as ffmpeg;

use ffmpeg::format::Pixel;
use ffmpeg::media::Type as MediaType;
use ffmpeg::software::scaling;

/// Decoded frames are capped at this size (4K UHD); bigger videos are scaled down.
const MAX_FRAME_W: u32 = 3840;
const MAX_FRAME_H: u32 = 2160;
/// Video frames decoded ahead of the clock.
const QUEUE_FRAMES: usize = 12;
/// Never queue more than this, even when audio is starved (memory safety valve).
const QUEUE_FRAMES_HARD: usize = 48;
/// Audio buffered ahead, in seconds.
const AUDIO_AHEAD: f64 = 0.5;
/// Re-sync the picture clock to the audio clock when they drift further apart than this.
const MAX_DRIFT: f64 = 0.06;

pub fn init() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = ffmpeg::init();
        ffmpeg::util::log::set_level(ffmpeg::util::log::Level::Fatal);
    });
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Clockwise quarter turns requested by the stream's display matrix (phone videos).
fn stream_rotation(stream: &ffmpeg::format::stream::Stream) -> u8 {
    for sd in stream.side_data() {
        if sd.kind() == ffmpeg::codec::packet::side_data::Type::DisplayMatrix {
            let d = sd.data();
            if d.len() >= 36 {
                let m =
                    |i: usize| i32::from_le_bytes(d[i * 4..i * 4 + 4].try_into().unwrap()) as f64;
                let s0 = m(0).hypot(m(3));
                let s1 = m(1).hypot(m(4));
                if s0 > 0.0 && s1 > 0.0 {
                    let deg = (m(1) / s1).atan2(m(0) / s0).to_degrees();
                    let turns = (deg / 90.0).round() as i32;
                    return turns.rem_euclid(4) as u8;
                }
            }
        }
    }
    0
}

/// Display size of a decoded frame: applies the pixel aspect ratio, then the size cap.
fn display_size(w: u32, h: u32, sar: ffmpeg::Rational) -> (u32, u32) {
    let mut dw = w as f64;
    let dh = h as f64;
    if sar.numerator() > 0 && sar.denominator() > 0 {
        let r = sar.numerator() as f64 / sar.denominator() as f64;
        if (r - 1.0).abs() > 0.01 {
            dw *= r;
        }
    }
    let scale = (MAX_FRAME_W as f64 / dw)
        .min(MAX_FRAME_H as f64 / dh)
        .min(1.0);
    (
        ((dw * scale).round() as u32).max(2),
        ((dh * scale).round() as u32).max(2),
    )
}

/// Converts decoded frames to tightly packed RGBA at a given size.
struct Rgba {
    scaler: Option<(scaling::Context, Pixel, u32, u32, u32, u32)>,
    out: ffmpeg::frame::Video,
}

impl Rgba {
    fn new() -> Self {
        Rgba {
            scaler: None,
            out: ffmpeg::frame::Video::empty(),
        }
    }

    fn convert(
        &mut self,
        frame: &ffmpeg::frame::Video,
        dw: u32,
        dh: u32,
    ) -> Result<ColorImage, String> {
        let key = (frame.format(), frame.width(), frame.height(), dw, dh);
        let stale = match &self.scaler {
            Some((_, f, w, h, ow, oh)) => (*f, *w, *h, *ow, *oh) != key,
            None => true,
        };
        if stale {
            let ctx = scaling::Context::get(
                frame.format(),
                frame.width(),
                frame.height(),
                Pixel::RGBA,
                dw,
                dh,
                scaling::Flags::BILINEAR,
            )
            .map_err(err)?;
            self.scaler = Some((ctx, key.0, key.1, key.2, key.3, key.4));
            self.out = ffmpeg::frame::Video::empty();
        }
        let (ctx, ..) = self.scaler.as_mut().unwrap();
        ctx.run(frame, &mut self.out).map_err(err)?;

        let (w, h) = (dw as usize, dh as usize);
        let stride = self.out.stride(0);
        let data = self.out.data(0);
        // The scaler outputs opaque RGBA, which is already valid premultiplied
        // Color32 data, so whole rows can be copied straight across.
        let mut pixels = vec![Color32::BLACK; w * h];
        // SAFETY: Color32 is a #[repr(C)] wrapper around [u8; 4].
        let bytes =
            unsafe { std::slice::from_raw_parts_mut(pixels.as_mut_ptr() as *mut u8, w * h * 4) };
        for (row, dst) in bytes.chunks_exact_mut(w * 4).enumerate() {
            dst.copy_from_slice(&data[row * stride..row * stride + w * 4]);
        }
        Ok(ColorImage {
            size: [w, h],
            pixels,
        })
    }
}

fn open_video_decoder(
    stream: &ffmpeg::format::stream::Stream,
) -> Result<ffmpeg::decoder::Video, String> {
    let mut ctx =
        ffmpeg::codec::context::Context::from_parameters(stream.parameters()).map_err(err)?;
    ctx.set_threading(ffmpeg::codec::threading::Config {
        kind: ffmpeg::codec::threading::Type::Frame,
        count: 0,
        ..Default::default()
    });
    ctx.decoder().video().map_err(err)
}

/// Grab a representative frame (about 10% in) for the thumbnail column.
pub fn thumbnail(path: &Path, max_side: u32) -> Result<ColorImage, String> {
    init();
    let mut input = ffmpeg::format::input(path).map_err(err)?;
    let stream = input
        .streams()
        .best(MediaType::Video)
        .ok_or("no video stream")?;
    let index = stream.index();
    let rotation = stream_rotation(&stream);
    let mut decoder = open_video_decoder(&stream)?;

    let duration = input.duration();
    if duration > 0 {
        let target = (duration / 10).min(5 * ffmpeg::ffi::AV_TIME_BASE as i64);
        let _ = input.seek(target, ..target);
    }

    let mut frame = ffmpeg::frame::Video::empty();
    let mut got = false;
    let mut packets = 0;
    for (s, packet) in input.packets() {
        if s.index() != index {
            continue;
        }
        packets += 1;
        if decoder.send_packet(&packet).is_ok() && decoder.receive_frame(&mut frame).is_ok() {
            got = true;
            break;
        }
        if packets > 500 {
            break;
        }
    }
    if !got {
        let _ = decoder.send_eof();
        decoder
            .receive_frame(&mut frame)
            .map_err(|_| "could not decode a video frame")?;
    }

    let (dw, dh) = display_size(frame.width(), frame.height(), frame.aspect_ratio());
    let scale = (max_side as f64 / dw.max(dh) as f64).min(1.0);
    let tw = ((dw as f64 * scale).round() as u32).max(2);
    let th = ((dh as f64 * scale).round() as u32).max(2);
    let img = Rgba::new().convert(&frame, tw, th)?;
    Ok(rotate_image(img, rotation))
}

fn rotate_image(img: ColorImage, quarter_turns: u8) -> ColorImage {
    if quarter_turns == 0 {
        return img;
    }
    let [w, h] = img.size;
    let src = &img.pixels;
    let (nw, nh) = if quarter_turns % 2 == 1 {
        (h, w)
    } else {
        (w, h)
    };
    let mut out = vec![Color32::BLACK; w * h];
    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = match quarter_turns {
                1 => (h - 1 - y, x),
                2 => (w - 1 - x, h - 1 - y),
                _ => (y, w - 1 - x),
            };
            out[ny * nw + nx] = src[y * w + x];
        }
    }
    ColorImage {
        size: [nw, nh],
        pixels: out,
    }
}

// ---------------------------------------------------------------------------
// Playback
// ---------------------------------------------------------------------------

struct VideoFrame {
    image: ColorImage,
    pts: f64,
    serial: u64,
}

#[derive(Default)]
struct DecodeState {
    frames: VecDeque<VideoFrame>,
    /// Incremented by every seek; frames from an older serial are stale.
    serial: u64,
    seek_to: Option<(f64, u64)>,
    eof: bool,
    quit: bool,
    error: Option<String>,
}

#[derive(Default)]
struct AudioState {
    samples: VecDeque<f32>,
    /// Timestamp of the first sample pushed since the last reset.
    base_pts: Option<f64>,
    /// Sample frames played since the last reset.
    consumed: u64,
    paused: bool,
    muted: bool,
}

struct Shared {
    decode: Mutex<DecodeState>,
    wake: Condvar,
    audio: Mutex<AudioState>,
}

struct AudioOut {
    _stream: cpal::Stream,
    rate: u32,
    channels: u16,
}

impl AudioOut {
    fn start(shared: Arc<Shared>) -> Option<AudioOut> {
        let device = cpal::default_host().default_output_device()?;
        let supported = device.default_output_config().ok()?;
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        let stream = match format {
            cpal::SampleFormat::F32 => build_stream::<f32>(&device, &config, shared),
            cpal::SampleFormat::I16 => build_stream::<i16>(&device, &config, shared),
            cpal::SampleFormat::U16 => build_stream::<u16>(&device, &config, shared),
            cpal::SampleFormat::I32 => build_stream::<i32>(&device, &config, shared),
            _ => None,
        }?;
        stream.play().ok()?;
        Some(AudioOut {
            _stream: stream,
            rate: config.sample_rate.0,
            channels: config.channels,
        })
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    shared: Arc<Shared>,
) -> Option<cpal::Stream>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = config.channels as usize;
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| {
                let mut a = shared.audio.lock().unwrap_or_else(|e| e.into_inner());
                if a.paused {
                    data.fill(T::from_sample(0.0));
                    return;
                }
                let mut popped = 0usize;
                let muted = a.muted;
                for out in data.iter_mut() {
                    let s = match a.samples.pop_front() {
                        Some(s) => {
                            popped += 1;
                            if muted {
                                0.0
                            } else {
                                s
                            }
                        }
                        None => 0.0,
                    };
                    *out = T::from_sample(s);
                }
                a.consumed += (popped / channels) as u64;
            },
            |e| eprintln!("audio output error: {e}"),
            None,
        )
        .ok()
}

/// Wall clock that can be paused and re-based.
struct Clock {
    base: f64,
    anchor: Option<Instant>,
}

impl Clock {
    fn now(&self) -> f64 {
        self.base + self.anchor.map_or(0.0, |a| a.elapsed().as_secs_f64())
    }
    fn set(&mut self, t: f64, running: bool) {
        self.base = t;
        self.anchor = running.then(Instant::now);
    }
}

pub struct Player {
    shared: Arc<Shared>,
    audio: Option<AudioOut>,
    duration: f64,
    rotation: u8,
    texture: Option<TextureHandle>,
    position: f64,
    clock: Clock,
    paused: bool,
    finished: bool,
    serial: u64,
    /// After opening or seeking: the next frame is shown at once and starts the clock.
    waiting_first: bool,
    /// Since when audio has been queued without the device playing any of it.
    audio_stalled_since: Option<Instant>,
    /// Cleared if the audio device never starts, so the picture doesn't wait forever.
    audio_sync: bool,
}

impl Player {
    pub fn open(path: &Path) -> Result<Player, String> {
        init();
        let input = ffmpeg::format::input(path).map_err(err)?;
        let vstream = input
            .streams()
            .best(MediaType::Video)
            .ok_or("no video stream")?;
        let rotation = stream_rotation(&vstream);
        let vindex = vstream.index();
        let vdec = open_video_decoder(&vstream)?;
        let vtime_base = f64::from(vstream.time_base());

        let shared = Arc::new(Shared {
            decode: Mutex::new(DecodeState::default()),
            wake: Condvar::new(),
            audio: Mutex::new(AudioState::default()),
        });

        // Audio is optional: no audio track, an undecodable one, or no output device
        // all just mean silent playback.
        let mut audio = None;
        let mut adec = None;
        if let Some(astream) = input.streams().best(MediaType::Audio) {
            let dec = ffmpeg::codec::context::Context::from_parameters(astream.parameters())
                .and_then(|c| c.decoder().audio());
            if let Ok(dec) = dec {
                if let Some(out) = AudioOut::start(shared.clone()) {
                    adec = Some(AudioDecoder {
                        index: astream.index(),
                        time_base: f64::from(astream.time_base()),
                        decoder: dec,
                        resampler: None,
                        out_rate: out.rate,
                        out_channels: out.channels,
                    });
                    audio = Some(out);
                }
            }
        }

        let duration = if input.duration() > 0 {
            input.duration() as f64 / ffmpeg::ffi::AV_TIME_BASE as f64
        } else {
            0.0
        };

        let worker = Decoder {
            input,
            shared: shared.clone(),
            video: VideoDecoder {
                index: vindex,
                time_base: vtime_base,
                decoder: vdec,
            },
            audio: adec,
            skip_until: 0.0,
        };
        std::thread::Builder::new()
            .name("video-decoder".into())
            .spawn(move || worker.run())
            .map_err(err)?;

        Ok(Player {
            shared,
            audio,
            duration,
            rotation,
            texture: None,
            position: 0.0,
            clock: Clock {
                base: 0.0,
                anchor: None,
            },
            paused: false,
            finished: false,
            serial: 0,
            waiting_first: true,
            audio_stalled_since: None,
            audio_sync: true,
        })
    }

    pub fn texture(&self) -> Option<&TextureHandle> {
        self.texture.as_ref()
    }
    pub fn rotation(&self) -> u8 {
        self.rotation
    }
    pub fn duration(&self) -> f64 {
        self.duration
    }
    pub fn position(&self) -> f64 {
        self.position
    }
    pub fn is_paused(&self) -> bool {
        self.paused || self.finished
    }
    pub fn is_finished(&self) -> bool {
        self.finished
    }
    pub fn has_audio(&self) -> bool {
        self.audio.is_some()
    }
    pub fn is_muted(&self) -> bool {
        self.shared.audio.lock().unwrap().muted
    }
    pub fn error(&self) -> Option<String> {
        self.shared.decode.lock().unwrap().error.clone()
    }

    pub fn toggle_mute(&mut self) {
        let mut a = self.shared.audio.lock().unwrap();
        a.muted = !a.muted;
    }

    pub fn toggle_pause(&mut self) {
        if self.finished {
            self.paused = false;
            self.seek(0.0);
            return;
        }
        self.set_paused(!self.paused);
    }

    fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        let now = self.clock.now();
        self.clock.set(now, !paused && !self.waiting_first);
        self.shared.audio.lock().unwrap().paused = paused;
    }

    pub fn seek(&mut self, t: f64) {
        // Stop a little short of the very end so there is still a frame to show.
        let t = if self.duration > 0.0 {
            t.clamp(0.0, (self.duration - 0.5).max(0.0))
        } else {
            t.max(0.0)
        };
        self.serial += 1;
        self.finished = false;
        self.waiting_first = true;
        self.position = t;
        self.clock.set(t, false);
        let mut st = self.shared.decode.lock().unwrap();
        st.seek_to = Some((t, self.serial));
        st.frames.clear();
        drop(st);
        self.shared.wake.notify_all();
    }

    pub fn seek_relative(&mut self, delta: f64) {
        let base = if self.waiting_first {
            self.position
        } else {
            self.clock.now()
        };
        self.seek(base + delta);
    }

    /// Advance playback; uploads the frame due now. Returns when to repaint next.
    pub fn update(&mut self, ctx: &egui::Context) -> Duration {
        // Keep the picture clock locked to the audio that is actually being heard.
        if !self.paused && !self.waiting_first && !self.finished && self.audio_sync {
            let a = self.shared.audio.lock().unwrap();
            if let (Some(base), Some(out)) = (a.base_pts, &self.audio) {
                if a.consumed == 0 && !a.samples.is_empty() {
                    let since = *self.audio_stalled_since.get_or_insert_with(Instant::now);
                    if since.elapsed() > Duration::from_secs(2) {
                        self.audio_sync = false;
                    }
                } else {
                    self.audio_stalled_since = None;
                }
                if self.audio_sync && !a.samples.is_empty() {
                    let audio_clock = base + a.consumed as f64 / out.rate as f64;
                    if (audio_clock - self.clock.now()).abs() > MAX_DRIFT {
                        self.clock.set(audio_clock, true);
                    }
                }
            }
        }

        let mut due = None;
        let mut next_pts = None;
        let eof;
        {
            let mut st = self.shared.decode.lock().unwrap();
            let now = self.clock.now();
            while let Some(front) = st.frames.front() {
                if front.serial != self.serial {
                    st.frames.pop_front();
                    continue;
                }
                if self.waiting_first {
                    due = st.frames.pop_front();
                    break;
                }
                if front.pts <= now {
                    // Late frames are dropped; only the newest due frame is shown.
                    due = st.frames.pop_front();
                } else {
                    next_pts = Some(front.pts);
                    break;
                }
            }
            eof = st.eof && st.serial == self.serial && st.frames.is_empty();
        }
        self.shared.wake.notify_all();

        if let Some(frame) = due {
            self.position = frame.pts;
            match &mut self.texture {
                Some(t) => t.set(frame.image, TextureOptions::LINEAR),
                None => {
                    self.texture =
                        Some(ctx.load_texture("video", frame.image, TextureOptions::LINEAR))
                }
            }
            if self.waiting_first {
                self.waiting_first = false;
                self.clock.set(frame.pts, !self.paused);
            }
        }

        if eof && self.waiting_first {
            // Seeked past the last frame: nothing more to show.
            self.waiting_first = false;
            self.finished = true;
        }
        if eof && !self.finished && !self.waiting_first {
            let audio_left = self.shared.audio.lock().unwrap().samples.len();
            if audio_left == 0 || self.paused {
                self.finished = true;
                if self.duration > 0.0 {
                    self.position = self.position.max(self.duration.min(self.position + 0.1));
                }
            }
        }

        if self.paused || self.finished {
            return Duration::from_millis(250);
        }
        match next_pts {
            Some(pts) => Duration::from_secs_f64((pts - self.clock.now()).clamp(0.002, 0.1)),
            None => Duration::from_millis(10),
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.shared.decode.lock().unwrap().quit = true;
        self.shared.wake.notify_all();
    }
}

// ---------------------------------------------------------------------------
// Decoder thread
// ---------------------------------------------------------------------------

struct VideoDecoder {
    index: usize,
    time_base: f64,
    decoder: ffmpeg::decoder::Video,
}

struct AudioDecoder {
    index: usize,
    time_base: f64,
    decoder: ffmpeg::decoder::Audio,
    resampler: Option<ffmpeg::software::resampling::Context>,
    out_rate: u32,
    out_channels: u16,
}

struct Decoder {
    input: ffmpeg::format::context::Input,
    shared: Arc<Shared>,
    video: VideoDecoder,
    audio: Option<AudioDecoder>,
    /// After a seek, frames before this time are decoded but not shown.
    skip_until: f64,
}

impl Decoder {
    fn run(mut self) {
        // The scaler is not Send, so it is created here on the decoder thread.
        let mut rgba = Rgba::new();
        if let Err(e) = self.run_inner(&mut rgba) {
            self.shared.decode.lock().unwrap().error = Some(e);
        }
    }

    fn audio_buffered(&self) -> f64 {
        match &self.audio {
            Some(a) => {
                let len = self.shared.audio.lock().unwrap().samples.len();
                len as f64 / (a.out_rate as f64 * a.out_channels as f64)
            }
            None => f64::INFINITY,
        }
    }

    fn run_inner(&mut self, rgba: &mut Rgba) -> Result<(), String> {
        loop {
            // Handle commands and apply back-pressure.
            let seek = {
                let mut st = self.shared.decode.lock().unwrap();
                loop {
                    if st.quit {
                        return Ok(());
                    }
                    if st.seek_to.is_some() {
                        break st.seek_to.take();
                    }
                    let full = st.frames.len() >= QUEUE_FRAMES_HARD
                        || (st.frames.len() >= QUEUE_FRAMES
                            && self.audio_buffered() >= AUDIO_AHEAD);
                    if !st.eof && !full {
                        break None;
                    }
                    // Timed wait: the audio callback drains its buffer without notifying.
                    st = self
                        .shared
                        .wake
                        .wait_timeout(st, Duration::from_millis(15))
                        .unwrap()
                        .0;
                }
            };
            if let Some((t, serial)) = seek {
                self.seek(t, serial);
                continue;
            }

            let next = self.input.packets().next();
            match next {
                Some((stream, packet)) => {
                    let index = stream.index();
                    if index == self.video.index {
                        if self.video.decoder.send_packet(&packet).is_ok() {
                            self.drain_video(rgba)?;
                        }
                    } else if self.audio.as_ref().is_some_and(|a| a.index == index) {
                        let a = self.audio.as_mut().unwrap();
                        if a.decoder.send_packet(&packet).is_ok() {
                            self.drain_audio()?;
                        }
                    }
                }
                None => {
                    let _ = self.video.decoder.send_eof();
                    self.drain_video(rgba)?;
                    if let Some(a) = self.audio.as_mut() {
                        let _ = a.decoder.send_eof();
                        self.drain_audio()?;
                    }
                    self.shared.decode.lock().unwrap().eof = true;
                }
            }
        }
    }

    fn seek(&mut self, t: f64, serial: u64) {
        let ts = (t * ffmpeg::ffi::AV_TIME_BASE as f64) as i64;
        let _ = self.input.seek(ts, ..ts);
        self.video.decoder.flush();
        if let Some(a) = self.audio.as_mut() {
            a.decoder.flush();
        }
        self.skip_until = t;
        {
            let mut a = self.shared.audio.lock().unwrap();
            a.samples.clear();
            a.base_pts = None;
            a.consumed = 0;
        }
        let mut st = self.shared.decode.lock().unwrap();
        st.frames.clear();
        st.serial = serial;
        st.eof = false;
    }

    fn drain_video(&mut self, rgba: &mut Rgba) -> Result<(), String> {
        let mut frame = ffmpeg::frame::Video::empty();
        while self.video.decoder.receive_frame(&mut frame).is_ok() {
            let pts = frame.timestamp().or(frame.pts()).unwrap_or(0) as f64 * self.video.time_base;
            if pts + 0.001 < self.skip_until {
                continue;
            }
            let (dw, dh) = display_size(frame.width(), frame.height(), frame.aspect_ratio());
            let image = rgba.convert(&frame, dw, dh)?;
            let mut st = self.shared.decode.lock().unwrap();
            if st.quit || st.seek_to.is_some() {
                return Ok(());
            }
            let serial = st.serial;
            st.frames.push_back(VideoFrame { image, pts, serial });
        }
        Ok(())
    }

    fn drain_audio(&mut self) -> Result<(), String> {
        let a = self.audio.as_mut().unwrap();
        let mut frame = ffmpeg::frame::Audio::empty();
        while a.decoder.receive_frame(&mut frame).is_ok() {
            let pts = frame
                .timestamp()
                .or(frame.pts())
                .map(|p| p as f64 * a.time_base);
            let in_samples = frame.samples();
            if in_samples == 0 {
                continue;
            }
            if let Some(p) = pts {
                if p + in_samples as f64 / frame.rate().max(1) as f64 <= self.skip_until {
                    continue;
                }
            }
            if a.resampler.is_none() {
                let mut layout = frame.channel_layout();
                if layout.is_empty() || layout.channels() != frame.channels() as i32 {
                    layout = ffmpeg::ChannelLayout::default(frame.channels() as i32);
                }
                let out_layout = ffmpeg::ChannelLayout::default(a.out_channels as i32);
                a.resampler = Some(
                    ffmpeg::software::resampling::Context::get(
                        frame.format(),
                        layout,
                        frame.rate(),
                        ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed),
                        out_layout,
                        a.out_rate,
                    )
                    .map_err(err)?,
                );
            }
            let swr = a.resampler.as_mut().unwrap();
            let channels = a.out_channels as usize;
            let samples = unsafe {
                let cap = ffmpeg::ffi::swr_get_out_samples(swr.as_mut_ptr(), in_samples as i32)
                    .max(0) as usize
                    + 32;
                let mut buf = vec![0f32; cap * channels];
                let mut out_ptr = buf.as_mut_ptr() as *mut u8;
                let n = ffmpeg::ffi::swr_convert(
                    swr.as_mut_ptr(),
                    &mut out_ptr as *mut *mut u8 as _,
                    cap as i32,
                    (*frame.as_ptr()).extended_data as _,
                    in_samples as i32,
                );
                if n < 0 {
                    continue;
                }
                buf.truncate(n as usize * channels);
                buf
            };
            let mut st = self.shared.audio.lock().unwrap();
            if st.base_pts.is_none() {
                st.base_pts = Some(pts.unwrap_or(self.skip_until).max(self.skip_until));
            }
            st.samples.extend(samples);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_size_applies_aspect_and_cap() {
        let square = ffmpeg::Rational::new(1, 1);
        assert_eq!(display_size(1920, 1080, square), (1920, 1080));
        // Anamorphic DVD: 720x480 with 32:27 pixels displays as 853x480.
        assert_eq!(display_size(720, 480, ffmpeg::Rational::new(32, 27)), (853, 480));
        // 8K is capped to 4K.
        assert_eq!(display_size(7680, 4320, square), (3840, 2160));
    }

    #[test]
    fn rotate_quarter_turn() {
        let px = |v| Color32::from_rgb(v, 0, 0);
        // 2x1 image [a b] rotated clockwise becomes 1x2 [a; b].
        let img = ColorImage { size: [2, 1], pixels: vec![px(1), px(2)] };
        let r = rotate_image(img, 1);
        assert_eq!(r.size, [1, 2]);
        assert_eq!(r.pixels, vec![px(1), px(2)]);
        let img = ColorImage { size: [2, 1], pixels: vec![px(1), px(2)] };
        let r = rotate_image(img, 3);
        assert_eq!(r.pixels, vec![px(2), px(1)]);
    }

    #[test]
    fn plays_generated_clip() {
        // Uses the ffmpeg CLI to make a test clip; skipped where it isn't installed.
        let dir = std::env::temp_dir().join(format!("liteview-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let clip = dir.join("clip.mp4");
        let made = std::process::Command::new("ffmpeg")
            .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=320x240:rate=25"])
            .args(["-t", "2", "-pix_fmt", "yuv420p"])
            .arg(&clip)
            .status()
            .is_ok_and(|s| s.success());
        if !made {
            eprintln!("ffmpeg CLI not available; skipping");
            return;
        }
        let thumb = thumbnail(&clip, 100).unwrap();
        assert_eq!(thumb.size, [100, 75]);
        let player = Player::open(&clip).unwrap();
        assert!((player.duration() - 2.0).abs() < 0.1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
