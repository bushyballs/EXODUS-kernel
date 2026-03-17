/// Screen recording for Genesis
///
/// Provides: frame capture loop, RLE video encoding, audio sync timestamps,
/// region selection, and recording session management.
///
/// Uses Q16 fixed-point math throughout (no floats).
///
/// Inspired by: OBS, Android screen recorder. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// Q16 fixed-point constant: 1.0
const Q16_ONE: i32 = 65536;

/// Q16 multiply
fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> 16) as i32
}

/// Q16 from integer
fn q16_from_int(x: i32) -> i32 {
    x << 16
}

/// Recording state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingState {
    Idle,
    Recording,
    Paused,
    Encoding,
    Finished,
    Error,
}

/// Capture region
#[derive(Debug, Clone, Copy)]
pub struct CaptureRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl CaptureRegion {
    pub fn fullscreen(screen_w: u32, screen_h: u32) -> Self {
        CaptureRegion {
            x: 0,
            y: 0,
            width: screen_w,
            height: screen_h,
        }
    }

    pub fn custom(x: u32, y: u32, w: u32, h: u32) -> Self {
        CaptureRegion {
            x,
            y,
            width: w,
            height: h,
        }
    }

    pub fn pixel_count(&self) -> u32 {
        self.width * self.height
    }
}

/// RLE-encoded frame data
/// Each run is stored as (color: u32, count: u16)
pub struct RleFrame {
    pub timestamp_ms: u64,
    pub runs: Vec<(u32, u16)>,
    pub width: u32,
    pub height: u32,
    pub compressed_size: usize,
    pub raw_size: usize,
}

impl RleFrame {
    /// Encode raw pixel data with RLE compression
    pub fn encode(pixels: &[u32], width: u32, height: u32, timestamp_ms: u64) -> Self {
        let mut runs = Vec::new();
        if pixels.is_empty() {
            return RleFrame {
                timestamp_ms,
                runs,
                width,
                height,
                compressed_size: 0,
                raw_size: 0,
            };
        }

        let raw_size = pixels.len() * 4;
        let mut current_color = pixels[0];
        let mut current_count: u16 = 1;

        for &pixel in pixels.iter().skip(1) {
            if pixel == current_color && current_count < u16::MAX {
                current_count += 1;
            } else {
                runs.push((current_color, current_count));
                current_color = pixel;
                current_count = 1;
            }
        }
        runs.push((current_color, current_count));

        let compressed_size = runs.len() * 6; // 4 bytes color + 2 bytes count
        RleFrame {
            timestamp_ms,
            runs,
            width,
            height,
            compressed_size,
            raw_size,
        }
    }

    /// Decode RLE data back to raw pixels
    pub fn decode(&self) -> Vec<u32> {
        let total = (self.width * self.height) as usize;
        let mut pixels = Vec::with_capacity(total);
        for &(color, count) in &self.runs {
            for _ in 0..count {
                if pixels.len() >= total {
                    break;
                }
                pixels.push(color);
            }
        }
        // Pad if needed
        while pixels.len() < total {
            pixels.push(0xFF000000);
        }
        pixels
    }

    /// Compute compression ratio as Q16 (lower = better compression)
    pub fn compression_ratio(&self) -> i32 {
        if self.raw_size == 0 {
            return Q16_ONE;
        }
        ((self.compressed_size as i64 * Q16_ONE as i64) / self.raw_size as i64) as i32
    }
}

/// Delta frame encoding: stores only changed pixels from previous frame
pub struct DeltaFrame {
    pub timestamp_ms: u64,
    pub changes: Vec<(u32, u32)>, // (pixel_index, new_color)
    pub base_frame_idx: usize,
}

impl DeltaFrame {
    /// Compute delta between two raw pixel buffers
    pub fn from_diff(prev: &[u32], curr: &[u32], timestamp_ms: u64, base_idx: usize) -> Self {
        let mut changes = Vec::new();
        let len = prev.len().min(curr.len());
        for i in 0..len {
            if prev[i] != curr[i] {
                changes.push((i as u32, curr[i]));
            }
        }
        DeltaFrame {
            timestamp_ms,
            changes,
            base_frame_idx: base_idx,
        }
    }

    /// Apply delta to a base pixel buffer
    pub fn apply(&self, base: &mut [u32]) {
        for &(idx, color) in &self.changes {
            let i = idx as usize;
            if i < base.len() {
                base[i] = color;
            }
        }
    }

    /// Check if this delta is worth storing (vs a full keyframe)
    /// Returns true if delta is smaller than a threshold of the full frame
    pub fn is_efficient(&self, total_pixels: u32) -> bool {
        // If more than 50% changed, use a keyframe instead
        let threshold = total_pixels / 2;
        (self.changes.len() as u32) < threshold
    }
}

/// Audio synchronization marker
pub struct AudioSyncPoint {
    pub video_timestamp_ms: u64,
    pub audio_sample_offset: u64,
    pub sample_rate: u32,
}

/// Recording quality preset
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingQuality {
    Low,      // Every 4th frame, high compression
    Medium,   // Every 2nd frame
    High,     // Every frame, keyframes every 30
    Lossless, // Every frame, all keyframes
}

impl RecordingQuality {
    pub fn frame_skip(&self) -> u32 {
        match self {
            RecordingQuality::Low => 4,
            RecordingQuality::Medium => 2,
            RecordingQuality::High => 1,
            RecordingQuality::Lossless => 1,
        }
    }

    pub fn keyframe_interval(&self) -> u32 {
        match self {
            RecordingQuality::Low => 60,
            RecordingQuality::Medium => 45,
            RecordingQuality::High => 30,
            RecordingQuality::Lossless => 1,
        }
    }
}

/// A complete recording session
pub struct RecordingSession {
    pub id: u32,
    pub name: String,
    pub state: RecordingState,
    pub region: CaptureRegion,
    pub quality: RecordingQuality,
    pub start_time: u64,
    pub duration_ms: u64,
    pub keyframes: Vec<RleFrame>,
    pub delta_frames: Vec<DeltaFrame>,
    pub audio_sync: Vec<AudioSyncPoint>,
    pub total_frames: u32,
    pub frame_counter: u32,
    pub fps_target: u32,
    pub frame_interval_ms: u64,
    pub last_capture_time: u64,
    pub last_raw_frame: Vec<u32>,
    pub include_cursor: bool,
    pub include_audio: bool,
    pub max_duration_ms: u64,
}

impl RecordingSession {
    pub fn new(id: u32, name: &str, region: CaptureRegion, quality: RecordingQuality) -> Self {
        RecordingSession {
            id,
            name: String::from(name),
            state: RecordingState::Idle,
            region,
            quality,
            start_time: 0,
            duration_ms: 0,
            keyframes: Vec::new(),
            delta_frames: Vec::new(),
            audio_sync: Vec::new(),
            total_frames: 0,
            frame_counter: 0,
            fps_target: 30,
            frame_interval_ms: 33, // ~30fps
            last_capture_time: 0,
            last_raw_frame: Vec::new(),
            include_cursor: true,
            include_audio: false,
            max_duration_ms: 600_000, // 10 minutes default
        }
    }

    /// Start recording
    pub fn start(&mut self, now_ms: u64) {
        self.state = RecordingState::Recording;
        self.start_time = now_ms;
        self.last_capture_time = now_ms;
        self.frame_counter = 0;
        self.total_frames = 0;
        self.keyframes.clear();
        self.delta_frames.clear();
    }

    /// Pause recording
    pub fn pause(&mut self) {
        if self.state == RecordingState::Recording {
            self.state = RecordingState::Paused;
        }
    }

    /// Resume recording
    pub fn resume(&mut self) {
        if self.state == RecordingState::Paused {
            self.state = RecordingState::Recording;
        }
    }

    /// Stop recording
    pub fn stop(&mut self, now_ms: u64) {
        self.duration_ms = now_ms.saturating_sub(self.start_time);
        self.state = RecordingState::Finished;
    }

    /// Capture a frame from the framebuffer
    pub fn capture_frame(&mut self, framebuffer: &[u32], fb_width: u32, now_ms: u64) {
        if self.state != RecordingState::Recording {
            return;
        }

        // Check frame timing
        if now_ms.saturating_sub(self.last_capture_time) < self.frame_interval_ms {
            return;
        }

        // Check max duration
        if now_ms.saturating_sub(self.start_time) >= self.max_duration_ms {
            self.stop(now_ms);
            return;
        }

        // Frame skip for quality settings
        self.frame_counter = self.frame_counter.saturating_add(1);
        if self.frame_counter % self.quality.frame_skip() != 0 {
            return;
        }

        // Extract the region from the framebuffer
        let region_pixels = self.extract_region(framebuffer, fb_width);

        let is_keyframe = self.total_frames % self.quality.keyframe_interval() == 0
            || self.last_raw_frame.is_empty();

        if is_keyframe {
            // Store as a full RLE keyframe
            let rle = RleFrame::encode(
                &region_pixels,
                self.region.width,
                self.region.height,
                now_ms,
            );
            self.keyframes.push(rle);
        } else {
            // Store as delta from last frame
            let delta = DeltaFrame::from_diff(
                &self.last_raw_frame,
                &region_pixels,
                now_ms,
                self.keyframes.len().saturating_sub(1),
            );
            if delta.is_efficient(self.region.pixel_count()) {
                self.delta_frames.push(delta);
            } else {
                // Too many changes, store as keyframe instead
                let rle = RleFrame::encode(
                    &region_pixels,
                    self.region.width,
                    self.region.height,
                    now_ms,
                );
                self.keyframes.push(rle);
            }
        }

        self.last_raw_frame = region_pixels;
        self.total_frames = self.total_frames.saturating_add(1);
        self.last_capture_time = now_ms;
    }

    /// Extract a rectangular region from the framebuffer
    fn extract_region(&self, framebuffer: &[u32], fb_width: u32) -> Vec<u32> {
        let mut pixels = Vec::with_capacity(self.region.pixel_count() as usize);
        for dy in 0..self.region.height {
            let src_y = self.region.y + dy;
            for dx in 0..self.region.width {
                let src_x = self.region.x + dx;
                let idx = (src_y * fb_width + src_x) as usize;
                if idx < framebuffer.len() {
                    pixels.push(framebuffer[idx]);
                } else {
                    pixels.push(0xFF000000);
                }
            }
        }
        pixels
    }

    /// Add an audio sync point
    pub fn add_audio_sync(&mut self, video_ts: u64, audio_offset: u64, sample_rate: u32) {
        self.audio_sync.push(AudioSyncPoint {
            video_timestamp_ms: video_ts,
            audio_sample_offset: audio_offset,
            sample_rate,
        });
    }

    /// Get total stored data size (approximate)
    pub fn data_size(&self) -> usize {
        let kf_size: usize = self.keyframes.iter().map(|kf| kf.compressed_size).sum();
        let df_size: usize = self
            .delta_frames
            .iter()
            .map(|df| df.changes.len() * 8)
            .sum();
        kf_size + df_size
    }

    /// Get average compression ratio as Q16
    pub fn avg_compression(&self) -> i32 {
        if self.keyframes.is_empty() {
            return Q16_ONE;
        }
        let sum: i64 = self
            .keyframes
            .iter()
            .map(|kf| kf.compression_ratio() as i64)
            .sum();
        (sum / self.keyframes.len() as i64) as i32
    }

    /// Get recording duration in milliseconds
    pub fn elapsed_ms(&self, now_ms: u64) -> u64 {
        if self.state == RecordingState::Finished {
            self.duration_ms
        } else {
            now_ms.saturating_sub(self.start_time)
        }
    }
}

/// Screen recorder manager
pub struct ScreenRecorder {
    pub sessions: Vec<RecordingSession>,
    pub active_session: Option<u32>,
    pub next_session_id: u32,
    pub default_quality: RecordingQuality,
    pub default_fps: u32,
    pub screen_width: u32,
    pub screen_height: u32,
}

impl ScreenRecorder {
    const fn new() -> Self {
        ScreenRecorder {
            sessions: Vec::new(),
            active_session: None,
            next_session_id: 1,
            default_quality: RecordingQuality::Medium,
            default_fps: 30,
            screen_width: 1024,
            screen_height: 768,
        }
    }

    /// Create a new recording session
    pub fn create_session(&mut self, name: &str, region: CaptureRegion) -> u32 {
        let id = self.next_session_id;
        self.next_session_id = self.next_session_id.saturating_add(1);
        let session = RecordingSession::new(id, name, region, self.default_quality);
        self.sessions.push(session);
        id
    }

    /// Start recording the specified session
    pub fn start_recording(&mut self, session_id: u32, now_ms: u64) -> bool {
        if let Some(session) = self.sessions.iter_mut().find(|s| s.id == session_id) {
            session.start(now_ms);
            self.active_session = Some(session_id);
            true
        } else {
            false
        }
    }

    /// Stop the active recording
    pub fn stop_recording(&mut self, now_ms: u64) {
        if let Some(id) = self.active_session {
            if let Some(session) = self.sessions.iter_mut().find(|s| s.id == id) {
                session.stop(now_ms);
            }
            self.active_session = None;
        }
    }

    /// Capture a frame for the active recording
    pub fn capture(&mut self, framebuffer: &[u32], fb_width: u32, now_ms: u64) {
        if let Some(id) = self.active_session {
            if let Some(session) = self.sessions.iter_mut().find(|s| s.id == id) {
                session.capture_frame(framebuffer, fb_width, now_ms);
                if session.state == RecordingState::Finished {
                    self.active_session = None;
                }
            }
        }
    }

    /// Check if currently recording
    pub fn is_recording(&self) -> bool {
        self.active_session.is_some()
    }
}

static RECORDER: Mutex<ScreenRecorder> = Mutex::new(ScreenRecorder::new());

/// Initialize the screen recorder
pub fn init() {
    serial_println!("    [screen-recorder] Screen recording initialized (RLE encoding, delta frames, audio sync)");
}

/// Create a new fullscreen recording session
pub fn create_fullscreen(name: &str, screen_w: u32, screen_h: u32) -> u32 {
    let mut rec = RECORDER.lock();
    rec.screen_width = screen_w;
    rec.screen_height = screen_h;
    let region = CaptureRegion::fullscreen(screen_w, screen_h);
    rec.create_session(name, region)
}

/// Create a region recording session
pub fn create_region(name: &str, x: u32, y: u32, w: u32, h: u32) -> u32 {
    let region = CaptureRegion::custom(x, y, w, h);
    RECORDER.lock().create_session(name, region)
}

/// Start recording
pub fn start(session_id: u32, now_ms: u64) -> bool {
    RECORDER.lock().start_recording(session_id, now_ms)
}

/// Stop the active recording
pub fn stop(now_ms: u64) {
    RECORDER.lock().stop_recording(now_ms);
}

/// Capture a frame (call from compositor each frame)
pub fn capture(framebuffer: &[u32], fb_width: u32, now_ms: u64) {
    RECORDER.lock().capture(framebuffer, fb_width, now_ms);
}

/// Check if recording is active
pub fn is_recording() -> bool {
    RECORDER.lock().is_recording()
}
