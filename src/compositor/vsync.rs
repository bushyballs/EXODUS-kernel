// compositor/vsync.rs - VSYNC management and timing

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// VSYNC period in nanoseconds (60Hz default = 16.67ms)
const DEFAULT_VSYNC_PERIOD_NS: u64 = 16_666_667;

/// VSYNC event callback type
type VsyncCallback = fn(timestamp_ns: u64, frame_number: u64);

/// VSYNC manager
///
/// Manages display refresh synchronization to prevent tearing
/// and ensure smooth frame delivery
pub struct VSyncManager {
    enabled: AtomicBool,
    vsync_period_ns: u64,
    last_vsync_ns: AtomicU64,
    frame_number: AtomicU64,
    callbacks: [Option<VsyncCallback>; 8],
    callback_count: usize,
}

impl VSyncManager {
    /// Create a new VSYNC manager
    pub fn new() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            vsync_period_ns: DEFAULT_VSYNC_PERIOD_NS,
            last_vsync_ns: AtomicU64::new(0),
            frame_number: AtomicU64::new(0),
            callbacks: [None; 8],
            callback_count: 0,
        }
    }

    /// Start VSYNC generation
    pub fn start(&mut self) {
        if self.enabled.load(Ordering::Acquire) {
            return;
        }

        log::info!("VSYNC: Starting at {} Hz", self.get_refresh_rate());

        // Initialize hardware VSYNC interrupt
        self.init_hardware_vsync();

        self.enabled.store(true, Ordering::Release);
        self.last_vsync_ns.store(self.get_time_ns(), Ordering::Release);
    }

    /// Stop VSYNC generation
    pub fn stop(&mut self) {
        self.enabled.store(false, Ordering::Release);
        log::info!("VSYNC: Stopped");
    }

    /// Wait for next VSYNC
    pub fn wait_for_vsync(&self) {
        if !self.enabled.load(Ordering::Acquire) {
            return;
        }

        let last_vsync = self.last_vsync_ns.load(Ordering::Acquire);
        let current_time = self.get_time_ns();
        let time_since_vsync = current_time.saturating_sub(last_vsync);

        if time_since_vsync < self.vsync_period_ns {
            // Sleep until next VSYNC
            let sleep_time = self.vsync_period_ns - time_since_vsync;
            self.sleep_ns(sleep_time);
        }

        // Simulate VSYNC interrupt
        self.on_vsync();
    }

    /// VSYNC interrupt handler
    pub fn on_vsync(&self) {
        if !self.enabled.load(Ordering::Acquire) {
            return;
        }

        let timestamp = self.get_time_ns();
        let frame = self.frame_number.fetch_add(1, Ordering::SeqCst);

        self.last_vsync_ns.store(timestamp, Ordering::Release);

        // Notify callbacks
        for i in 0..self.callback_count {
            if let Some(callback) = self.callbacks[i] {
                callback(timestamp, frame);
            }
        }
    }

    /// Register a VSYNC callback
    pub fn register_callback(&mut self, callback: VsyncCallback) -> bool {
        if self.callback_count < 8 {
            self.callbacks[self.callback_count] = Some(callback);
            self.callback_count = self.callback_count.saturating_add(1);
            true
        } else {
            false
        }
    }

    /// Set refresh rate
    pub fn set_refresh_rate(&mut self, hz: u32) {
        if hz == 0 {
            return;
        }

        self.vsync_period_ns = 1_000_000_000 / hz as u64;
        log::info!("VSYNC: Refresh rate set to {} Hz", hz);
    }

    /// Get current refresh rate
    pub fn get_refresh_rate(&self) -> u32 {
        (1_000_000_000 / self.vsync_period_ns) as u32
    }

    /// Get VSYNC period in nanoseconds
    pub fn get_period_ns(&self) -> u64 {
        self.vsync_period_ns
    }

    /// Get current frame number
    pub fn get_frame_number(&self) -> u64 {
        self.frame_number.load(Ordering::Acquire)
    }

    /// Get time until next VSYNC
    pub fn time_to_next_vsync_ns(&self) -> u64 {
        let last_vsync = self.last_vsync_ns.load(Ordering::Acquire);
        let current_time = self.get_time_ns();
        let elapsed = current_time.saturating_sub(last_vsync);

        if elapsed >= self.vsync_period_ns {
            0
        } else {
            self.vsync_period_ns - elapsed
        }
    }

    /// Initialize hardware VSYNC interrupt
    fn init_hardware_vsync(&self) {
        // In real implementation:
        // 1. Configure display controller interrupt
        // 2. Set up interrupt handler
        // 3. Enable VSYNC interrupt in display hardware
        // 4. Configure interrupt priority

        // For now, we'll use software timing
    }

    /// Get current time in nanoseconds
    fn get_time_ns(&self) -> u64 {
        // In real implementation, read from hardware timer (TSC, HPET, etc.)
        // For now, return incrementing value based on frame count
        self.frame_number.load(Ordering::Acquire) * self.vsync_period_ns
    }

    /// Sleep for nanoseconds
    fn sleep_ns(&self, _ns: u64) {
        // In real implementation, use high-resolution timer
        // For bare metal, this might be:
        // - Busy wait for short delays
        // - Timer interrupt for longer delays
        // - HPET or TSC-based waiting

        // Placeholder: spin loop
        for _ in 0..100 {
            core::hint::spin_loop();
        }
    }
}

/// Frame timing tracker for performance monitoring
pub struct FrameTimingTracker {
    frame_times_ns: [u64; 128],
    write_index: usize,
    frames_recorded: u64,
}

impl FrameTimingTracker {
    pub fn new() -> Self {
        Self {
            frame_times_ns: [0; 128],
            write_index: 0,
            frames_recorded: 0,
        }
    }

    /// Record a frame time
    pub fn record_frame(&mut self, duration_ns: u64) {
        self.frame_times_ns[self.write_index] = duration_ns;
        self.write_index = (self.write_index + 1) % 128;
        self.frames_recorded = self.frames_recorded.saturating_add(1);
    }

    /// Get average frame time
    pub fn average_frame_time_ns(&self) -> u64 {
        if self.frames_recorded == 0 {
            return 0;
        }

        let count = self.frames_recorded.min(128) as usize;
        let sum: u64 = self.frame_times_ns[..count].iter().sum();
        sum / count as u64
    }

    /// Get average FPS
    pub fn average_fps(&self) -> f32 {
        let avg_time = self.average_frame_time_ns();
        if avg_time == 0 {
            return 0.0;
        }
        1_000_000_000.0 / avg_time as f32
    }

    /// Get max frame time (worst case)
    pub fn max_frame_time_ns(&self) -> u64 {
        if self.frames_recorded == 0 {
            return 0;
        }

        let count = self.frames_recorded.min(128) as usize;
        *self.frame_times_ns[..count].iter().max().unwrap_or(&0)
    }

    /// Get min frame time (best case)
    pub fn min_frame_time_ns(&self) -> u64 {
        if self.frames_recorded == 0 {
            return 0;
        }

        let count = self.frames_recorded.min(128) as usize;
        *self.frame_times_ns[..count].iter().min().unwrap_or(&0)
    }

    /// Check if we're dropping frames
    pub fn is_dropping_frames(&self, vsync_period_ns: u64) -> bool {
        self.max_frame_time_ns() > vsync_period_ns
    }
}

/// Display timing information
#[derive(Debug, Clone, Copy)]
pub struct DisplayTiming {
    pub refresh_rate_hz: u32,
    pub vsync_period_ns: u64,
    pub hsync_period_ns: u64,
    pub pixel_clock_khz: u32,
}

impl DisplayTiming {
    /// Calculate timing for standard display modes
    pub fn from_mode(width: u32, height: u32, refresh_hz: u32) -> Self {
        let vsync_period_ns = 1_000_000_000 / refresh_hz as u64;

        // Simplified timing calculation (real implementation would use CVT/GTF)
        let total_pixels = width * height;
        let pixels_per_second = total_pixels * refresh_hz;
        let pixel_clock_khz = pixels_per_second / 1000;

        let hsync_period_ns = vsync_period_ns / height as u64;

        Self {
            refresh_rate_hz: refresh_hz,
            vsync_period_ns,
            hsync_period_ns,
            pixel_clock_khz,
        }
    }

    /// Create 1080p60 timing
    pub fn mode_1080p60() -> Self {
        Self::from_mode(1920, 1080, 60)
    }

    /// Create 4K60 timing
    pub fn mode_4k60() -> Self {
        Self::from_mode(3840, 2160, 60)
    }

    /// Create 1440p144 timing
    pub fn mode_1440p144() -> Self {
        Self::from_mode(2560, 1440, 144)
    }
}
