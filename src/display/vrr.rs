use crate::sync::Mutex;
/// Variable Refresh Rate (FreeSync/G-Sync)
///
/// Part of the AIOS display layer. Controls adaptive sync to eliminate
/// tearing and stuttering by matching the display refresh rate to the
/// GPU's frame output rate. Implements Low Framerate Compensation (LFC)
/// and frame pacing algorithms.
use alloc::vec::Vec;

/// VRR mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VrrMode {
    Off,
    FreeSync,
    GSync,
    AdaptiveSync,
}

/// Frame timing entry for pacing analysis
#[derive(Debug, Clone, Copy)]
struct FrameTiming {
    present_time_us: u64,
    render_time_us: u64,
    target_hz: u32,
}

/// Low Framerate Compensation state
struct LfcState {
    enabled: bool,
    multiplier: u32,   // How many vsyncs per frame (1 = normal, 2 = doubled, etc.)
    threshold_hz: u32, // Below this FPS, activate LFC
}

/// Manages variable refresh rate display output
pub struct VrrController {
    pub mode: VrrMode,
    pub min_hz: u32,
    pub max_hz: u32,
    pub current_hz: u32,
    lfc: LfcState,
    frame_history: Vec<FrameTiming>,
    history_max: usize,
    target_fps: u32,
    last_present_us: u64,
    frame_count: u64,
    dropped_frames: u64,
    /// Smoothing window for FPS calculation
    smoothing_window: u32,
    /// Average frame time in microseconds (smoothed)
    avg_frame_time_us: u64,
    /// Is VRR actually engaged with the display
    display_vrr_active: bool,
}

impl VrrController {
    pub fn new() -> Self {
        crate::serial_println!("[vrr] controller created, VRR off");
        Self {
            mode: VrrMode::Off,
            min_hz: 48,
            max_hz: 144,
            current_hz: 60,
            lfc: LfcState {
                enabled: true,
                multiplier: 1,
                threshold_hz: 48,
            },
            frame_history: Vec::with_capacity(128),
            history_max: 128,
            target_fps: 60,
            last_present_us: 0,
            frame_count: 0,
            dropped_frames: 0,
            smoothing_window: 8,
            avg_frame_time_us: 16667, // ~60fps
            display_vrr_active: false,
        }
    }

    /// Enable VRR with the specified mode and display range
    pub fn enable(&mut self, mode: VrrMode, min_hz: u32, max_hz: u32) {
        self.mode = mode;
        self.min_hz = if min_hz < 1 { 1 } else { min_hz };
        self.max_hz = if max_hz < self.min_hz {
            self.min_hz
        } else {
            max_hz
        };
        self.lfc.threshold_hz = self.min_hz;
        self.display_vrr_active = !matches!(mode, VrrMode::Off);
        crate::serial_println!(
            "[vrr] enabled: {:?}, range {}Hz-{}Hz",
            mode,
            self.min_hz,
            self.max_hz
        );
    }

    /// Disable VRR and return to fixed refresh
    pub fn disable(&mut self) {
        self.mode = VrrMode::Off;
        self.display_vrr_active = false;
        self.current_hz = 60;
        self.lfc.multiplier = 1;
        crate::serial_println!("[vrr] disabled, fixed 60Hz");
    }

    pub fn set_target_fps(&mut self, fps: u32) {
        let clamped_fps = if fps == 0 {
            1
        } else if fps > self.max_hz {
            self.max_hz
        } else {
            fps
        };
        self.target_fps = clamped_fps;

        if matches!(self.mode, VrrMode::Off) {
            // VRR off: just set fixed rate
            self.current_hz = clamped_fps.min(self.max_hz);
            return;
        }

        // Check if we need Low Framerate Compensation
        if clamped_fps < self.lfc.threshold_hz && self.lfc.enabled {
            // LFC: multiply the refresh rate to stay within VRR range
            // e.g., if target is 30fps and min is 48Hz, display at 60Hz (2x)
            let mut mult = 1u32;
            let mut effective_hz = clamped_fps;
            while effective_hz < self.min_hz && mult < 4 {
                mult += 1;
                effective_hz = clamped_fps * mult;
            }
            self.lfc.multiplier = mult;
            self.current_hz = effective_hz.min(self.max_hz);
            crate::serial_println!(
                "[vrr] LFC active: {}fps -> {}Hz ({}x)",
                clamped_fps,
                self.current_hz,
                mult
            );
        } else {
            // Normal VRR: match refresh to frame rate
            self.lfc.multiplier = 1;
            self.current_hz = clamped_fps.max(self.min_hz).min(self.max_hz);
        }
    }

    pub fn enabled(&self) -> bool {
        !matches!(self.mode, VrrMode::Off)
    }

    /// Record a frame presentation event for timing analysis.
    /// present_us: timestamp when the frame was presented to the display.
    /// render_us: how long the frame took to render.
    pub fn record_frame(&mut self, present_us: u64, render_us: u64) {
        self.frame_count = self.frame_count.saturating_add(1);

        let timing = FrameTiming {
            present_time_us: present_us,
            render_time_us: render_us,
            target_hz: self.current_hz,
        };

        self.frame_history.push(timing);
        if self.frame_history.len() > self.history_max {
            self.frame_history.remove(0);
        }

        // Update smoothed average frame time
        if self.last_present_us > 0 {
            let frame_delta = present_us.saturating_sub(self.last_present_us);
            // Exponential moving average
            let weight = self.smoothing_window as u64;
            self.avg_frame_time_us = (self.avg_frame_time_us * (weight - 1) + frame_delta) / weight;

            // Detect dropped frames: if delta > 1.5x expected
            let expected_us = 1_000_000u64 / self.current_hz as u64;
            if frame_delta > expected_us * 3 / 2 {
                self.dropped_frames = self.dropped_frames.saturating_add(1);
            }

            // Auto-adjust refresh rate based on actual frame delivery
            if self.display_vrr_active {
                let actual_fps = if frame_delta > 0 {
                    1_000_000 / frame_delta
                } else {
                    self.current_hz as u64
                };
                self.adapt_refresh(actual_fps as u32);
            }
        }

        self.last_present_us = present_us;
    }

    /// Adapt the refresh rate based on measured FPS
    fn adapt_refresh(&mut self, measured_fps: u32) {
        let target = measured_fps.max(self.min_hz).min(self.max_hz);

        // Only change if significantly different to avoid oscillation
        let diff = if target > self.current_hz {
            target - self.current_hz
        } else {
            self.current_hz - target
        };
        if diff >= 2 {
            self.current_hz = target;
        }
    }

    /// Calculate the ideal vsync interval in microseconds
    pub fn vsync_interval_us(&self) -> u64 {
        if self.current_hz == 0 {
            return 16667;
        }
        1_000_000 / self.current_hz as u64
    }

    /// Get the number of dropped frames since init
    pub fn dropped_frame_count(&self) -> u64 {
        self.dropped_frames
    }

    /// Get the smoothed average FPS
    pub fn average_fps(&self) -> u32 {
        if self.avg_frame_time_us == 0 {
            return 0;
        }
        (1_000_000 / self.avg_frame_time_us) as u32
    }

    /// Get frame time statistics: (min_us, avg_us, max_us) over the history window
    pub fn frame_time_stats(&self) -> (u64, u64, u64) {
        if self.frame_history.len() < 2 {
            return (0, self.avg_frame_time_us, 0);
        }
        let mut min_delta = u64::MAX;
        let mut max_delta = 0u64;
        let mut sum_delta = 0u64;
        let mut count = 0u64;

        for i in 1..self.frame_history.len() {
            let delta = self.frame_history[i]
                .present_time_us
                .saturating_sub(self.frame_history[i - 1].present_time_us);
            if delta > 0 {
                if delta < min_delta {
                    min_delta = delta;
                }
                if delta > max_delta {
                    max_delta = delta;
                }
                sum_delta += delta;
                count += 1;
            }
        }

        let avg = if count > 0 { sum_delta / count } else { 0 };
        (min_delta, avg, max_delta)
    }

    /// Report VRR status
    pub fn report(&self) {
        let (min_ft, avg_ft, max_ft) = self.frame_time_stats();
        crate::serial_println!(
            "[vrr] mode={:?} current={}Hz target={}fps",
            self.mode,
            self.current_hz,
            self.target_fps
        );
        crate::serial_println!(
            "[vrr] frames={} dropped={} avg_fps={}",
            self.frame_count,
            self.dropped_frames,
            self.average_fps()
        );
        crate::serial_println!(
            "[vrr] frame times: min={}us avg={}us max={}us",
            min_ft,
            avg_ft,
            max_ft
        );
        crate::serial_println!(
            "[vrr] LFC: enabled={} multiplier={}",
            self.lfc.enabled,
            self.lfc.multiplier
        );
    }
}

static VRR: Mutex<Option<VrrController>> = Mutex::new(None);

pub fn init() {
    let controller = VrrController::new();
    let mut v = VRR.lock();
    *v = Some(controller);
    crate::serial_println!("[vrr] subsystem initialized");
}

/// Enable VRR from external code
pub fn enable(mode: VrrMode, min_hz: u32, max_hz: u32) {
    let mut v = VRR.lock();
    if let Some(ref mut ctrl) = *v {
        ctrl.enable(mode, min_hz, max_hz);
    }
}

/// Get current refresh rate
pub fn current_hz() -> u32 {
    let v = VRR.lock();
    match v.as_ref() {
        Some(ctrl) => ctrl.current_hz,
        None => 60,
    }
}
