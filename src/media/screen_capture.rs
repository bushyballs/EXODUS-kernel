/// Screen capture for Genesis — screenshots and screen recording
///
/// Captures the display framebuffer for screenshots and video recording.
/// Supports region selection, annotation, and automatic file saving.
///
/// Inspired by: Android screen capture, scrot, OBS. All code is original.
use crate::sync::Mutex;
use alloc::vec::Vec;

/// Capture format
#[derive(Debug, Clone, Copy)]
pub enum CaptureFormat {
    Bmp,
    Png,
    Raw,
}

/// Capture region
#[derive(Debug, Clone, Copy)]
pub struct CaptureRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Screen recording state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingState {
    Idle,
    Recording,
    Paused,
}

/// Screen recorder
pub struct ScreenRecorder {
    pub state: RecordingState,
    pub width: u32,
    pub height: u32,
    pub fps: u8,
    pub frames: Vec<Vec<u8>>,
    pub start_time: u64,
    pub frame_count: u64,
    /// Include audio
    pub record_audio: bool,
    /// Show touch indicators
    pub show_touches: bool,
}

impl ScreenRecorder {
    const fn new() -> Self {
        ScreenRecorder {
            state: RecordingState::Idle,
            width: 0,
            height: 0,
            fps: 30,
            frames: Vec::new(),
            start_time: 0,
            frame_count: 0,
            record_audio: false,
            show_touches: false,
        }
    }

    pub fn start(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.frames.clear();
        self.frame_count = 0;
        self.start_time = crate::time::clock::unix_time();
        self.state = RecordingState::Recording;
    }

    pub fn pause(&mut self) {
        if self.state == RecordingState::Recording {
            self.state = RecordingState::Paused;
        }
    }

    pub fn resume(&mut self) {
        if self.state == RecordingState::Paused {
            self.state = RecordingState::Recording;
        }
    }

    pub fn stop(&mut self) -> Vec<Vec<u8>> {
        self.state = RecordingState::Idle;
        core::mem::take(&mut self.frames)
    }

    /// Capture one frame from the framebuffer
    pub fn capture_frame(&mut self, framebuf: &[u32]) {
        if self.state != RecordingState::Recording {
            return;
        }

        // Simple RLE compression of the frame
        let mut compressed = Vec::new();
        if !framebuf.is_empty() {
            let mut run_pixel = framebuf[0];
            let mut run_len: u16 = 1;

            for &pixel in &framebuf[1..] {
                if pixel == run_pixel && run_len < 0xFFFF {
                    run_len += 1;
                } else {
                    compressed.extend_from_slice(&run_len.to_le_bytes());
                    compressed.extend_from_slice(&run_pixel.to_le_bytes());
                    run_pixel = pixel;
                    run_len = 1;
                }
            }
            compressed.extend_from_slice(&run_len.to_le_bytes());
            compressed.extend_from_slice(&run_pixel.to_le_bytes());
        }

        self.frames.push(compressed);
        self.frame_count = self.frame_count.saturating_add(1);
    }
}

/// Take a screenshot
pub fn screenshot(framebuf: &[u32], width: u32, height: u32) -> Vec<u8> {
    // Create BMP file
    let pixel_count = (width * height) as usize;
    let row_size = (width * 4) as usize;
    let pixel_data_size = row_size * height as usize;
    let file_size = 54 + pixel_data_size; // BMP header + pixels

    let mut bmp = Vec::with_capacity(file_size);

    // BMP file header (14 bytes)
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&(file_size as u32).to_le_bytes());
    bmp.extend_from_slice(&0u16.to_le_bytes()); // reserved
    bmp.extend_from_slice(&0u16.to_le_bytes()); // reserved
    bmp.extend_from_slice(&54u32.to_le_bytes()); // pixel data offset

    // DIB header (40 bytes — BITMAPINFOHEADER)
    bmp.extend_from_slice(&40u32.to_le_bytes()); // header size
    bmp.extend_from_slice(&(width as i32).to_le_bytes());
    bmp.extend_from_slice(&(-(height as i32)).to_le_bytes()); // top-down
    bmp.extend_from_slice(&1u16.to_le_bytes()); // color planes
    bmp.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
    bmp.extend_from_slice(&0u32.to_le_bytes()); // no compression
    bmp.extend_from_slice(&(pixel_data_size as u32).to_le_bytes());
    bmp.extend_from_slice(&2835u32.to_le_bytes()); // H resolution (72 DPI)
    bmp.extend_from_slice(&2835u32.to_le_bytes()); // V resolution
    bmp.extend_from_slice(&0u32.to_le_bytes()); // colors in palette
    bmp.extend_from_slice(&0u32.to_le_bytes()); // important colors

    // Pixel data (BGRA format)
    for i in 0..pixel_count.min(framebuf.len()) {
        let pixel = framebuf[i];
        let b = (pixel & 0xFF) as u8;
        let g = ((pixel >> 8) & 0xFF) as u8;
        let r = ((pixel >> 16) & 0xFF) as u8;
        let a = ((pixel >> 24) & 0xFF) as u8;
        bmp.push(b);
        bmp.push(g);
        bmp.push(r);
        bmp.push(a);
    }

    bmp
}

/// Take a screenshot of a region
pub fn screenshot_region(framebuf: &[u32], screen_width: u32, region: CaptureRegion) -> Vec<u8> {
    let mut region_pixels = Vec::new();
    for y in region.y..region.y + region.height {
        for x in region.x..region.x + region.width {
            let idx = (y * screen_width + x) as usize;
            if idx < framebuf.len() {
                region_pixels.push(framebuf[idx]);
            } else {
                region_pixels.push(0);
            }
        }
    }
    screenshot(&region_pixels, region.width, region.height)
}

static RECORDER: Mutex<ScreenRecorder> = Mutex::new(ScreenRecorder::new());

pub fn init() {
    crate::serial_println!("  [screen-capture] Screen capture initialized");
}

pub fn start_recording(width: u32, height: u32) {
    RECORDER.lock().start(width, height);
}
pub fn stop_recording() -> Vec<Vec<u8>> {
    RECORDER.lock().stop()
}
pub fn is_recording() -> bool {
    RECORDER.lock().state == RecordingState::Recording
}
