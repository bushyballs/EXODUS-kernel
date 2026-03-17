use crate::sync::Mutex;
/// Camera hardware driver for Genesis — sensor init, capture, ISP pipeline
///
/// Provides camera sensor control over MIPI CSI-2 / parallel interface:
///   - Sensor initialization and power sequencing
///   - Resolution modes (VGA, 720p, 1080p, 4K)
///   - Exposure control (auto and manual)
///   - Focus control (auto-focus, manual, macro, infinity)
///   - Basic ISP pipeline (white balance, gamma, denoise, sharpening)
///   - Frame capture into memory buffers
///   - I2C register-level sensor communication
///
/// Inspired by: Linux V4L2 driver model, libcamera architecture.
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Q16 fixed-point helpers
// ---------------------------------------------------------------------------

const Q16_SHIFT: i32 = 16;
const Q16_ONE: i32 = 1 << Q16_SHIFT;

fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> Q16_SHIFT) as i32
}

#[allow(dead_code)]
fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 {
        return 0;
    }
    (((a as i64) << Q16_SHIFT) / b as i64) as i32
}

// ---------------------------------------------------------------------------
// I2C sensor communication
// ---------------------------------------------------------------------------

const I2C_CAM_ADDR: u8 = 0x3C; // OV5640 default
const I2C_STATUS_PORT: u16 = 0xC200;
const I2C_DATA_PORT: u16 = 0xC204;
const I2C_CTRL_PORT: u16 = 0xC208;

fn i2c_wait() {
    for _ in 0..5000 {
        if crate::io::inb(I2C_STATUS_PORT) & 0x01 != 0 {
            return;
        }
        core::hint::spin_loop();
    }
}

fn sensor_read_reg(reg: u16) -> u8 {
    crate::io::outb(I2C_CTRL_PORT, 0x01); // START
    crate::io::outb(I2C_DATA_PORT, (I2C_CAM_ADDR << 1) | 0x00);
    i2c_wait();
    // 16-bit register address (high byte first)
    crate::io::outb(I2C_DATA_PORT, (reg >> 8) as u8);
    i2c_wait();
    crate::io::outb(I2C_DATA_PORT, (reg & 0xFF) as u8);
    i2c_wait();
    // Repeated start for read
    crate::io::outb(I2C_CTRL_PORT, 0x01);
    crate::io::outb(I2C_DATA_PORT, (I2C_CAM_ADDR << 1) | 0x01);
    i2c_wait();
    let val = crate::io::inb(I2C_DATA_PORT);
    crate::io::outb(I2C_CTRL_PORT, 0x02); // STOP
    val
}

fn sensor_write_reg(reg: u16, val: u8) {
    crate::io::outb(I2C_CTRL_PORT, 0x01); // START
    crate::io::outb(I2C_DATA_PORT, (I2C_CAM_ADDR << 1) | 0x00);
    i2c_wait();
    crate::io::outb(I2C_DATA_PORT, (reg >> 8) as u8);
    i2c_wait();
    crate::io::outb(I2C_DATA_PORT, (reg & 0xFF) as u8);
    i2c_wait();
    crate::io::outb(I2C_DATA_PORT, val);
    i2c_wait();
    crate::io::outb(I2C_CTRL_PORT, 0x02); // STOP
}

// ---------------------------------------------------------------------------
// Resolution and pixel format types
// ---------------------------------------------------------------------------

/// Camera resolution mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// 640x480 (VGA)
    Vga,
    /// 1280x720 (HD 720p)
    Hd720,
    /// 1920x1080 (Full HD 1080p)
    Hd1080,
    /// 3840x2160 (4K UHD)
    Uhd4K,
}

impl Resolution {
    pub fn width(&self) -> u32 {
        match self {
            Resolution::Vga => 640,
            Resolution::Hd720 => 1280,
            Resolution::Hd1080 => 1920,
            Resolution::Uhd4K => 3840,
        }
    }

    pub fn height(&self) -> u32 {
        match self {
            Resolution::Vga => 480,
            Resolution::Hd720 => 720,
            Resolution::Hd1080 => 1080,
            Resolution::Uhd4K => 2160,
        }
    }

    pub fn pixel_count(&self) -> u32 {
        self.width() * self.height()
    }
}

/// Pixel format
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// Raw Bayer (RGGB)
    BayerRGGB,
    /// YUV 4:2:2 packed
    Yuv422,
    /// RGB 8-8-8 (24-bit)
    Rgb888,
    /// RGBA 8-8-8-8 (32-bit)
    Rgba8888,
}

impl PixelFormat {
    pub fn bytes_per_pixel(&self) -> u32 {
        match self {
            PixelFormat::BayerRGGB => 1,
            PixelFormat::Yuv422 => 2,
            PixelFormat::Rgb888 => 3,
            PixelFormat::Rgba8888 => 4,
        }
    }
}

// ---------------------------------------------------------------------------
// Exposure and focus
// ---------------------------------------------------------------------------

/// Exposure mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExposureMode {
    /// Automatic exposure
    Auto,
    /// Manual exposure (user-set values)
    Manual,
    /// Shutter priority (user sets shutter, auto ISO)
    ShutterPriority,
    /// ISO priority (user sets ISO, auto shutter)
    IsoPriority,
}

/// Auto-exposure metering mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeteringMode {
    /// Average across entire frame
    Average,
    /// Center-weighted average
    CenterWeighted,
    /// Spot metering (center point)
    Spot,
    /// Matrix/evaluative metering
    Matrix,
}

/// Focus mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusMode {
    /// Autofocus (contrast detection)
    Auto,
    /// Continuous autofocus
    ContinuousAuto,
    /// Manual focus
    Manual,
    /// Macro (close-up)
    Macro,
    /// Infinity focus
    Infinity,
}

/// Focus state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusState {
    /// Not focused
    Idle,
    /// Searching for focus
    Scanning,
    /// Focus locked
    Locked,
    /// Focus failed
    Failed,
}

// ---------------------------------------------------------------------------
// ISP pipeline stages
// ---------------------------------------------------------------------------

/// White balance preset
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhiteBalance {
    Auto,
    Daylight,
    Cloudy,
    Tungsten,
    Fluorescent,
    /// Custom gains (R, G, B in Q16)
    Custom,
}

/// ISP pipeline configuration (all gains in Q16)
pub struct IspConfig {
    /// White balance mode
    pub white_balance: WhiteBalance,
    /// Custom white balance gains (Q16): [R, G, B]
    pub wb_gains: [i32; 3],
    /// Gamma correction value (Q16, e.g. 2.2 = 0x00023333)
    pub gamma_q16: i32,
    /// Denoise strength (0 = off, Q16_ONE = max)
    pub denoise_strength: i32,
    /// Sharpening strength (0 = off, Q16_ONE = max)
    pub sharpen_strength: i32,
    /// Brightness offset (-128..127 mapped to Q16)
    pub brightness: i32,
    /// Contrast factor (Q16, 1.0 = no change)
    pub contrast: i32,
    /// Saturation factor (Q16, 1.0 = no change)
    pub saturation: i32,
}

impl IspConfig {
    const fn default() -> Self {
        IspConfig {
            white_balance: WhiteBalance::Auto,
            wb_gains: [Q16_ONE, Q16_ONE, Q16_ONE],
            gamma_q16: 144179, // ~2.2 in Q16 (2.2 * 65536)
            denoise_strength: Q16_ONE / 4,
            sharpen_strength: Q16_ONE / 2,
            brightness: 0,
            contrast: Q16_ONE,
            saturation: Q16_ONE,
        }
    }
}

/// Apply white balance gains to a single RGB pixel (each channel 0-255)
fn apply_white_balance(r: &mut i32, g: &mut i32, b: &mut i32, gains: &[i32; 3]) {
    *r = q16_mul(*r << Q16_SHIFT, gains[0]) >> Q16_SHIFT;
    *g = q16_mul(*g << Q16_SHIFT, gains[1]) >> Q16_SHIFT;
    *b = q16_mul(*b << Q16_SHIFT, gains[2]) >> Q16_SHIFT;
    *r = (*r).clamp(0, 255);
    *g = (*g).clamp(0, 255);
    *b = (*b).clamp(0, 255);
}

/// Apply gamma correction via lookup table approximation (Q16 gamma)
fn apply_gamma(val: i32, _gamma_q16: i32) -> i32 {
    // Simple gamma approximation using piecewise linear (no pow in no_std)
    // For gamma ~2.2, use sqrt-like curve
    let v = val.clamp(0, 255);
    // Approximate gamma via: output = 255 * (input/255)^(1/gamma)
    // For 1/2.2 ~ 0.4545, approximate with scaled square root
    let v_q16 = (v << Q16_SHIFT) / 255;
    // Newton's method sqrt of v_q16 (approx gamma 0.5, close to 0.4545)
    let mut guess = (v_q16 + Q16_ONE) / 2;
    if guess <= 0 {
        guess = 1;
    }
    for _ in 0..3 {
        if guess == 0 {
            break;
        }
        // Use i64 intermediate to avoid i32 overflow: v_q16 * Q16_ONE can be up to (65535 * 65536)
        let numerator = (v_q16 as i64) * (Q16_ONE as i64) / (guess as i64);
        guess = ((guess as i64 + numerator) / 2).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
    }
    let result = (guess * 255) >> Q16_SHIFT;
    result.clamp(0, 255)
}

/// Simple 3x3 box denoise on a pixel row
fn denoise_pixel(buf: &[u8], idx: usize, stride: usize, strength: i32) -> u8 {
    if strength <= 0 {
        return buf[idx];
    }
    let center = buf[idx] as i32;
    // Average with neighbors (safe bounds)
    let mut sum = center * 4;
    let mut count = 4i32;
    let offsets: [isize; 4] = [-1, 1, -(stride as isize), stride as isize];
    for &off in &offsets {
        let ni = idx as isize + off;
        if ni >= 0 && (ni as usize) < buf.len() {
            sum += buf[ni as usize] as i32;
            count += 1;
        }
    }
    let avg = sum / count;
    // Blend between center and average based on strength
    let result = center + q16_mul(avg - center, strength);
    result.clamp(0, 255) as u8
}

/// Simple unsharp mask sharpening
fn sharpen_pixel(buf: &[u8], idx: usize, stride: usize, strength: i32) -> u8 {
    if strength <= 0 {
        return buf[idx];
    }
    let center = buf[idx] as i32;
    let mut neighbor_sum = 0i32;
    let mut n_count = 0i32;
    let offsets: [isize; 4] = [-1, 1, -(stride as isize), stride as isize];
    for &off in &offsets {
        let ni = idx as isize + off;
        if ni >= 0 && (ni as usize) < buf.len() {
            neighbor_sum += buf[ni as usize] as i32;
            n_count += 1;
        }
    }
    if n_count == 0 {
        return buf[idx];
    }
    let avg = neighbor_sum / n_count;
    let detail = center - avg;
    let result = center + q16_mul(detail, strength);
    result.clamp(0, 255) as u8
}

// ---------------------------------------------------------------------------
// Frame buffer
// ---------------------------------------------------------------------------

/// A captured frame
pub struct Frame {
    /// Raw pixel data
    pub data: Vec<u8>,
    /// Width in pixels
    pub width: u32,
    /// Height in pixels
    pub height: u32,
    /// Pixel format
    pub format: PixelFormat,
    /// Frame sequence number
    pub sequence: u32,
    /// Capture timestamp (ms since boot)
    pub timestamp: u64,
}

impl Frame {
    fn new(width: u32, height: u32, format: PixelFormat) -> Self {
        let size = (width as u64)
            .saturating_mul(height as u64)
            .saturating_mul(format.bytes_per_pixel() as u64)
            .min(usize::MAX as u64) as usize;
        Frame {
            data: alloc::vec![0u8; size],
            width,
            height,
            format,
            sequence: 0,
            timestamp: 0,
        }
    }

    /// Total size in bytes
    pub fn size(&self) -> usize {
        (self.width as u64)
            .saturating_mul(self.height as u64)
            .saturating_mul(self.format.bytes_per_pixel() as u64)
            .min(usize::MAX as u64) as usize
    }

    /// Row stride in bytes
    pub fn stride(&self) -> usize {
        (self.width as u64)
            .saturating_mul(self.format.bytes_per_pixel() as u64)
            .min(usize::MAX as u64) as usize
    }
}

// ---------------------------------------------------------------------------
// Camera controller state
// ---------------------------------------------------------------------------

/// Sensor chip type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorType {
    OV5640,
    OV2640,
    IMX219,
    Unknown,
}

/// Camera driver state
pub struct CameraState {
    /// Detected sensor
    sensor: SensorType,
    /// Whether sensor is initialized and streaming
    initialized: bool,
    streaming: bool,
    /// Current resolution
    resolution: Resolution,
    /// Output pixel format
    format: PixelFormat,
    /// Exposure settings
    exposure_mode: ExposureMode,
    metering_mode: MeteringMode,
    /// Manual exposure time in microseconds
    exposure_us: u32,
    /// ISO sensitivity (100, 200, 400, etc.)
    iso: u32,
    /// Exposure compensation (Q16, +/- 2 stops)
    ev_compensation: i32,
    /// Focus settings
    focus_mode: FocusMode,
    focus_state: FocusState,
    /// Manual focus position (0-1023)
    focus_position: u16,
    /// ISP configuration
    isp: IspConfig,
    /// Frame counter
    frame_count: u32,
    /// Sensor horizontal/vertical flip
    h_flip: bool,
    v_flip: bool,
}

impl CameraState {
    const fn new() -> Self {
        CameraState {
            sensor: SensorType::Unknown,
            initialized: false,
            streaming: false,
            resolution: Resolution::Vga,
            format: PixelFormat::Rgb888,
            exposure_mode: ExposureMode::Auto,
            metering_mode: MeteringMode::Matrix,
            exposure_us: 33333, // ~30fps
            iso: 100,
            ev_compensation: 0,
            focus_mode: FocusMode::Auto,
            focus_state: FocusState::Idle,
            focus_position: 512,
            isp: IspConfig::default(),
            frame_count: 0,
            h_flip: false,
            v_flip: false,
        }
    }

    /// Detect the sensor chip by reading its ID registers
    fn detect_sensor(&mut self) {
        // OV5640: chip ID at 0x300A/0x300B = 0x5640
        let id_h = sensor_read_reg(0x300A);
        let id_l = sensor_read_reg(0x300B);
        let chip_id = ((id_h as u16) << 8) | id_l as u16;

        match chip_id {
            0x5640 => {
                self.sensor = SensorType::OV5640;
            }
            0x2640 | 0x2642 => {
                self.sensor = SensorType::OV2640;
            }
            _ => {
                // Try IMX219: chip ID at 0x0000/0x0001 = 0x0219
                let imx_h = sensor_read_reg(0x0000);
                let imx_l = sensor_read_reg(0x0001);
                let imx_id = ((imx_h as u16) << 8) | imx_l as u16;
                if imx_id == 0x0219 {
                    self.sensor = SensorType::IMX219;
                } else {
                    self.sensor = SensorType::Unknown;
                }
            }
        }
    }

    /// Power-on sequence for the sensor
    fn power_on(&self) {
        // Assert reset (active low) — GPIO control via MMIO
        sensor_write_reg(0x3008, 0x82); // Software reset
        for _ in 0..10000 {
            core::hint::spin_loop();
        }
        sensor_write_reg(0x3008, 0x02); // Normal mode
        for _ in 0..50000 {
            core::hint::spin_loop();
        }
    }

    /// Configure sensor for the selected resolution
    fn configure_resolution(&self) {
        let (w, h) = (
            self.resolution.width() as u16,
            self.resolution.height() as u16,
        );

        // Output size registers (OV5640-style)
        sensor_write_reg(0x3808, (w >> 8) as u8);
        sensor_write_reg(0x3809, (w & 0xFF) as u8);
        sensor_write_reg(0x380A, (h >> 8) as u8);
        sensor_write_reg(0x380B, (h & 0xFF) as u8);

        // Timing/PLL adjustments per resolution
        match self.resolution {
            Resolution::Vga => {
                sensor_write_reg(0x3034, 0x1A); // PLL: 10-bit MIPI
                sensor_write_reg(0x3035, 0x14); // System clock divider
            }
            Resolution::Hd720 => {
                sensor_write_reg(0x3034, 0x1A);
                sensor_write_reg(0x3035, 0x11);
            }
            Resolution::Hd1080 => {
                sensor_write_reg(0x3034, 0x1A);
                sensor_write_reg(0x3035, 0x11);
                sensor_write_reg(0x3036, 0x54); // PLL multiplier
            }
            Resolution::Uhd4K => {
                sensor_write_reg(0x3034, 0x1A);
                sensor_write_reg(0x3035, 0x11);
                sensor_write_reg(0x3036, 0x69);
            }
        }

        // Mirror/flip
        let mut reg = sensor_read_reg(0x3821);
        if self.h_flip {
            reg |= 0x06;
        } else {
            reg &= !0x06;
        }
        sensor_write_reg(0x3821, reg);

        let mut reg = sensor_read_reg(0x3820);
        if self.v_flip {
            reg |= 0x06;
        } else {
            reg &= !0x06;
        }
        sensor_write_reg(0x3820, reg);
    }

    /// Set auto-exposure parameters on the sensor
    fn configure_exposure(&self) {
        match self.exposure_mode {
            ExposureMode::Auto => {
                // Enable AEC/AGC (auto exposure/gain control)
                sensor_write_reg(0x3503, 0x00);
                // Set metering window
                match self.metering_mode {
                    MeteringMode::CenterWeighted => {
                        sensor_write_reg(0x5688, 0x11);
                        sensor_write_reg(0x5689, 0x11);
                    }
                    MeteringMode::Spot => {
                        sensor_write_reg(0x5688, 0x01);
                        sensor_write_reg(0x5689, 0x10);
                    }
                    _ => {
                        sensor_write_reg(0x5688, 0x11);
                        sensor_write_reg(0x5689, 0x11);
                    }
                }
            }
            ExposureMode::Manual => {
                // Disable AEC, set manual values
                sensor_write_reg(0x3503, 0x07);
                // Exposure time (20-bit value in lines)
                let lines = self.exposure_us / 33; // approx line time
                sensor_write_reg(0x3500, ((lines >> 12) & 0x0F) as u8);
                sensor_write_reg(0x3501, ((lines >> 4) & 0xFF) as u8);
                sensor_write_reg(0x3502, ((lines & 0x0F) << 4) as u8);
                // ISO -> analog gain
                let gain = (self.iso / 100).max(1).min(32) as u8;
                sensor_write_reg(0x350A, 0x00);
                sensor_write_reg(0x350B, gain << 4);
            }
            _ => {
                // Shutter/ISO priority: partial auto
                sensor_write_reg(0x3503, 0x04);
            }
        }
    }

    /// Initiate autofocus
    fn trigger_autofocus(&mut self) {
        match self.focus_mode {
            FocusMode::Auto => {
                // Send AF trigger command
                sensor_write_reg(0x3022, 0x03); // Single AF trigger
                self.focus_state = FocusState::Scanning;
            }
            FocusMode::ContinuousAuto => {
                sensor_write_reg(0x3022, 0x04); // Continuous AF
                self.focus_state = FocusState::Scanning;
            }
            FocusMode::Manual => {
                // Set VCM (voice coil motor) position directly
                let pos = self.focus_position;
                sensor_write_reg(0x3602, (pos >> 8) as u8);
                sensor_write_reg(0x3603, (pos & 0xFF) as u8);
                self.focus_state = FocusState::Locked;
            }
            FocusMode::Macro => {
                sensor_write_reg(0x3602, 0x03);
                sensor_write_reg(0x3603, 0xFF);
                self.focus_state = FocusState::Locked;
            }
            FocusMode::Infinity => {
                sensor_write_reg(0x3602, 0x00);
                sensor_write_reg(0x3603, 0x00);
                self.focus_state = FocusState::Locked;
            }
        }
    }

    /// Check if autofocus has completed
    fn check_focus_status(&mut self) {
        if self.focus_state != FocusState::Scanning {
            return;
        }
        let status = sensor_read_reg(0x3029);
        match status {
            0x10 => {
                self.focus_state = FocusState::Locked;
            }
            0x00 => { /* still scanning */ }
            _ => {
                self.focus_state = FocusState::Failed;
            }
        }
    }

    /// Run ISP pipeline on a captured frame (in-place for RGB888)
    fn run_isp(&self, frame: &mut Frame) {
        if frame.format != PixelFormat::Rgb888 {
            return;
        }
        let stride = frame.stride();
        let bpp = 3usize;

        // Process each pixel
        for y in 0..frame.height as usize {
            for x in 0..frame.width as usize {
                let idx = y * stride + x * bpp;
                if idx + 2 >= frame.data.len() {
                    break;
                }

                let mut r = frame.data[idx] as i32;
                let mut g = frame.data[idx + 1] as i32;
                let mut b = frame.data[idx + 2] as i32;

                // White balance
                apply_white_balance(&mut r, &mut g, &mut b, &self.isp.wb_gains);

                // Brightness
                r = (r + self.isp.brightness).clamp(0, 255);
                g = (g + self.isp.brightness).clamp(0, 255);
                b = (b + self.isp.brightness).clamp(0, 255);

                // Contrast: (pixel - 128) * contrast + 128
                r = (q16_mul((r - 128) << Q16_SHIFT, self.isp.contrast) >> Q16_SHIFT) + 128;
                g = (q16_mul((g - 128) << Q16_SHIFT, self.isp.contrast) >> Q16_SHIFT) + 128;
                b = (q16_mul((b - 128) << Q16_SHIFT, self.isp.contrast) >> Q16_SHIFT) + 128;

                // Gamma correction
                r = apply_gamma(r.clamp(0, 255), self.isp.gamma_q16);
                g = apply_gamma(g.clamp(0, 255), self.isp.gamma_q16);
                b = apply_gamma(b.clamp(0, 255), self.isp.gamma_q16);

                frame.data[idx] = r.clamp(0, 255) as u8;
                frame.data[idx + 1] = g.clamp(0, 255) as u8;
                frame.data[idx + 2] = b.clamp(0, 255) as u8;
            }
        }

        // Denoise pass (on green channel as proxy)
        if self.isp.denoise_strength > 0 && frame.height > 2 && frame.width > 2 {
            let h = frame.height as usize;
            let w = frame.width as usize;
            let data_copy = frame.data.clone();
            for y in 1..(h - 1) {
                for x in 1..(w - 1) {
                    let idx = y
                        .saturating_mul(stride)
                        .saturating_add(x.saturating_mul(bpp))
                        .saturating_add(1); // green
                    if idx < frame.data.len() {
                        frame.data[idx] =
                            denoise_pixel(&data_copy, idx, stride, self.isp.denoise_strength);
                    }
                }
            }
        }

        // Sharpen pass
        if self.isp.sharpen_strength > 0 && frame.height > 2 && frame.width > 2 {
            let h = frame.height as usize;
            let w = frame.width as usize;
            let data_copy = frame.data.clone();
            for y in 1..(h - 1) {
                for x in 1..(w - 1) {
                    for c in 0..3usize {
                        let idx = y
                            .saturating_mul(stride)
                            .saturating_add(x.saturating_mul(bpp))
                            .saturating_add(c);
                        if idx < frame.data.len() {
                            frame.data[idx] =
                                sharpen_pixel(&data_copy, idx, stride, self.isp.sharpen_strength);
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static CAMERA: Mutex<CameraState> = Mutex::new(CameraState::new());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the camera driver
pub fn init() {
    let mut cam = CAMERA.lock();
    cam.detect_sensor();
    if cam.sensor == SensorType::Unknown {
        serial_println!("  Camera: no sensor detected");
        return;
    }
    cam.power_on();
    cam.configure_resolution();
    cam.configure_exposure();
    cam.initialized = true;
    let sensor_name = match cam.sensor {
        SensorType::OV5640 => "OV5640",
        SensorType::OV2640 => "OV2640",
        SensorType::IMX219 => "IMX219",
        SensorType::Unknown => "unknown",
    };
    serial_println!(
        "  Camera: {} sensor, {}x{}",
        sensor_name,
        cam.resolution.width(),
        cam.resolution.height()
    );
    drop(cam);
    super::register("camera", super::DeviceType::Other);
}

/// Set camera resolution (must stop streaming first)
pub fn set_resolution(res: Resolution) {
    let mut cam = CAMERA.lock();
    if cam.streaming {
        return;
    }
    cam.resolution = res;
    if cam.initialized {
        cam.configure_resolution();
    }
}

/// Set pixel format
pub fn set_format(fmt: PixelFormat) {
    let mut cam = CAMERA.lock();
    if cam.streaming {
        return;
    }
    cam.format = fmt;
}

/// Start streaming
pub fn start_streaming() {
    let mut cam = CAMERA.lock();
    if !cam.initialized || cam.streaming {
        return;
    }
    sensor_write_reg(0x3008, 0x02); // Start streaming
    cam.streaming = true;
    serial_println!("  Camera: streaming started");
}

/// Stop streaming
pub fn stop_streaming() {
    let mut cam = CAMERA.lock();
    if !cam.streaming {
        return;
    }
    sensor_write_reg(0x3008, 0x42); // Standby
    cam.streaming = false;
    serial_println!("  Camera: streaming stopped");
}

/// Capture a single frame
pub fn capture_frame() -> Option<Frame> {
    let mut cam = CAMERA.lock();
    if !cam.initialized {
        return None;
    }

    let mut frame = Frame::new(cam.resolution.width(), cam.resolution.height(), cam.format);

    // In a real driver, DMA would fill this buffer from the sensor FIFO.
    // Here we simulate reading from the sensor's frame buffer register range.
    let fifo_port: u16 = 0xC210;
    for i in 0..frame.data.len() {
        frame.data[i] = crate::io::inb(fifo_port);
    }

    cam.frame_count = cam.frame_count.saturating_add(1);
    frame.sequence = cam.frame_count;
    frame.timestamp = crate::time::clock::uptime_ms();

    // Run ISP pipeline
    cam.run_isp(&mut frame);

    // Check AF status
    cam.check_focus_status();

    Some(frame)
}

/// Set exposure mode
pub fn set_exposure_mode(mode: ExposureMode) {
    let mut cam = CAMERA.lock();
    cam.exposure_mode = mode;
    if cam.initialized {
        cam.configure_exposure();
    }
}

/// Set manual exposure (microseconds) and ISO
pub fn set_manual_exposure(exposure_us: u32, iso: u32) {
    let mut cam = CAMERA.lock();
    cam.exposure_mode = ExposureMode::Manual;
    cam.exposure_us = exposure_us;
    cam.iso = iso;
    if cam.initialized {
        cam.configure_exposure();
    }
}

/// Set focus mode
pub fn set_focus_mode(mode: FocusMode) {
    let mut cam = CAMERA.lock();
    cam.focus_mode = mode;
    if cam.initialized {
        cam.trigger_autofocus();
    }
}

/// Set manual focus position (0-1023)
pub fn set_focus_position(pos: u16) {
    let mut cam = CAMERA.lock();
    cam.focus_position = pos.min(1023);
    cam.focus_mode = FocusMode::Manual;
    if cam.initialized {
        cam.trigger_autofocus();
    }
}

/// Trigger autofocus
pub fn autofocus() {
    let mut cam = CAMERA.lock();
    if cam.initialized {
        cam.trigger_autofocus();
    }
}

/// Get current focus state
pub fn focus_state() -> FocusState {
    CAMERA.lock().focus_state
}

/// Set white balance mode
pub fn set_white_balance(wb: WhiteBalance) {
    CAMERA.lock().isp.white_balance = wb;
}

/// Set custom white balance gains (Q16 values for R, G, B)
pub fn set_wb_gains(r: i32, g: i32, b: i32) {
    let mut cam = CAMERA.lock();
    cam.isp.white_balance = WhiteBalance::Custom;
    cam.isp.wb_gains = [r, g, b];
}

/// Set horizontal/vertical flip
pub fn set_flip(h_flip: bool, v_flip: bool) {
    let mut cam = CAMERA.lock();
    cam.h_flip = h_flip;
    cam.v_flip = v_flip;
    if cam.initialized {
        cam.configure_resolution();
    }
}

/// Set ISP brightness (-128 to 127)
pub fn set_brightness(brightness: i32) {
    CAMERA.lock().isp.brightness = brightness.clamp(-128, 127);
}

/// Set ISP contrast (Q16 factor, 1.0 = no change)
pub fn set_contrast(contrast_q16: i32) {
    CAMERA.lock().isp.contrast = contrast_q16;
}

/// Get current frame count
pub fn frame_count() -> u32 {
    CAMERA.lock().frame_count
}
