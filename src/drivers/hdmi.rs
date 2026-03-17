use crate::sync::Mutex;
/// HDMI output driver for Genesis
///
/// Manages HDMI display output including EDID parsing, resolution and refresh
/// rate configuration, HDCP authentication state tracking, audio passthrough
/// control, and hotplug detection via status register polling.
///
/// Uses MMIO register interface to an HDMI transmitter block (generic Intel
/// HD Graphics / DRM-style register set).
///
/// Inspired by: Linux drm/i915 HDMI encoder, EDID parsing. All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Register offsets (HDMI transmitter block)
// ---------------------------------------------------------------------------

const REG_CONTROL: usize = 0x00; // Main control register
const REG_STATUS: usize = 0x04; // Status register
const REG_HPD: usize = 0x08; // Hotplug detect
const REG_EDID_CTRL: usize = 0x0C; // EDID/DDC control
const REG_EDID_DATA: usize = 0x10; // EDID data read port
const REG_H_ACTIVE: usize = 0x20; // Horizontal active pixels
const REG_H_BLANK: usize = 0x24; // Horizontal blanking
const REG_H_SYNC: usize = 0x28; // H sync start/end
const REG_V_ACTIVE: usize = 0x30; // Vertical active lines
const REG_V_BLANK: usize = 0x34; // Vertical blanking
const REG_V_SYNC: usize = 0x38; // V sync start/end
const REG_PIXEL_CLK: usize = 0x40; // Pixel clock in kHz
const REG_HDCP_CTRL: usize = 0x50; // HDCP control
const REG_HDCP_STATUS: usize = 0x54; // HDCP status
const REG_AUDIO_CTRL: usize = 0x60; // Audio control
const REG_AUDIO_CTS: usize = 0x64; // Audio CTS value
const REG_AUDIO_N: usize = 0x68; // Audio N value
const REG_INFOFRAME: usize = 0x80; // AVI InfoFrame data (32 bytes)

// Control bits
const CTRL_ENABLE: u32 = 1 << 0;
const CTRL_HDMI_MODE: u32 = 1 << 1; // 1=HDMI, 0=DVI
const CTRL_DEEP_COLOR: u32 = 1 << 4; // 10/12-bit color
const CTRL_SCRAMBLE: u32 = 1 << 5; // HDMI 2.0 scrambling

// Status bits
const STATUS_CONNECTED: u32 = 1 << 0;
const STATUS_LINK_OK: u32 = 1 << 1;
const STATUS_EDID_READY: u32 = 1 << 2;

// HPD bits
const HPD_DETECTED: u32 = 1 << 0;
const HPD_IRQ: u32 = 1 << 1;

// HDCP bits
const HDCP_ENABLE: u32 = 1 << 0;
const HDCP_AUTHENTICATED: u32 = 1 << 0;
const HDCP_REPEATER: u32 = 1 << 1;

// Audio bits
const AUDIO_ENABLE: u32 = 1 << 0;
const AUDIO_MUTE: u32 = 1 << 1;

// EDID constants
const EDID_BLOCK_SIZE: usize = 128;
const EDID_HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];

// DDC commands
const DDC_START: u32 = 1 << 0;
const DDC_STOP: u32 = 1 << 1;
const DDC_BLOCK_SHIFT: u32 = 8;

// Timeout
const TIMEOUT_SPINS: u32 = 100_000;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Parsed EDID display mode
#[derive(Debug, Clone, Copy)]
pub struct DisplayMode {
    pub width: u32,
    pub height: u32,
    pub refresh_hz: u32,
    pub pixel_clock_khz: u32,
    pub h_blank: u16,
    pub h_sync_start: u16,
    pub h_sync_end: u16,
    pub v_blank: u16,
    pub v_sync_start: u16,
    pub v_sync_end: u16,
    pub preferred: bool,
}

/// HDCP authentication state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HdcpState {
    Disabled,
    Authenticating,
    Authenticated,
    Failed,
}

/// HDMI connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connected,
    LinkActive,
}

/// HDMI error codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HdmiError {
    NotInitialized,
    NotConnected,
    EdidReadFailed,
    InvalidMode,
    HdcpFailed,
    Timeout,
}

/// Internal HDMI port state
struct HdmiInner {
    base_addr: usize,
    connection: ConnectionState,
    modes: Vec<DisplayMode>,
    active_mode: Option<DisplayMode>,
    hdcp: HdcpState,
    audio_enabled: bool,
    manufacturer: [u8; 4],
    monitor_name: String,
}

// ---------------------------------------------------------------------------
// MMIO helpers
// ---------------------------------------------------------------------------

#[inline]
fn mmio_read32(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

#[inline]
fn mmio_write32(addr: usize, val: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, val) }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static HDMI: Mutex<Option<HdmiInner>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// EDID parsing
// ---------------------------------------------------------------------------

/// Read EDID block from DDC channel
fn read_edid_block(base: usize, block: u8) -> Result<[u8; EDID_BLOCK_SIZE], HdmiError> {
    // Initiate DDC read for the requested block
    let ctrl = DDC_START | ((block as u32) << DDC_BLOCK_SHIFT);
    mmio_write32(base.saturating_add(REG_EDID_CTRL), ctrl);

    // Wait for EDID data ready
    for _ in 0..TIMEOUT_SPINS {
        if mmio_read32(base.saturating_add(REG_STATUS)) & STATUS_EDID_READY != 0 {
            break;
        }
    }
    if mmio_read32(base.saturating_add(REG_STATUS)) & STATUS_EDID_READY == 0 {
        mmio_write32(base.saturating_add(REG_EDID_CTRL), DDC_STOP);
        return Err(HdmiError::EdidReadFailed);
    }

    let mut edid = [0u8; EDID_BLOCK_SIZE];
    for i in 0..EDID_BLOCK_SIZE {
        edid[i] = (mmio_read32(base.saturating_add(REG_EDID_DATA)) & 0xFF) as u8;
    }

    mmio_write32(base.saturating_add(REG_EDID_CTRL), DDC_STOP);

    // Validate checksum
    let checksum: u8 = edid.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
    if checksum != 0 {
        return Err(HdmiError::EdidReadFailed);
    }

    Ok(edid)
}

/// Parse standard timing descriptors from EDID block 0
fn parse_edid_modes(edid: &[u8; EDID_BLOCK_SIZE]) -> Vec<DisplayMode> {
    let mut modes = Vec::new();

    // Check EDID header
    if edid[0..8] != EDID_HEADER {
        return modes;
    }

    // Parse detailed timing descriptors (bytes 54-125, 4 x 18-byte blocks)
    for desc_idx in 0..4usize {
        let offset = 54usize.saturating_add(desc_idx.saturating_mul(18));
        let pixel_clk_10khz = (edid[offset + 1] as u32) << 8 | edid[offset] as u32;
        if pixel_clk_10khz == 0 {
            continue; // Not a timing descriptor
        }

        let h_active = (edid[offset + 2] as u32) | (((edid[offset + 4] >> 4) as u32) << 8);
        let h_blank = (edid[offset + 3] as u32) | (((edid[offset + 4] & 0x0F) as u32) << 8);
        let v_active = (edid[offset + 5] as u32) | (((edid[offset + 7] >> 4) as u32) << 8);
        let v_blank = (edid[offset + 6] as u32) | (((edid[offset + 7] & 0x0F) as u32) << 8);
        let h_sync_off = (edid[offset + 8] as u16) | (((edid[offset + 11] >> 6) as u16) << 8);
        let h_sync_width =
            (edid[offset + 9] as u16) | ((((edid[offset + 11] >> 4) & 0x03) as u16) << 8);
        let v_sync_off =
            ((edid[offset + 10] >> 4) as u16) | ((((edid[offset + 11] >> 2) & 0x03) as u16) << 4);
        let v_sync_width =
            ((edid[offset + 10] & 0x0F) as u16) | (((edid[offset + 11] & 0x03) as u16) << 4);

        let pixel_clock_khz = pixel_clk_10khz.saturating_mul(10);
        let h_total = h_active.saturating_add(h_blank);
        let v_total = v_active.saturating_add(v_blank);
        let refresh = if h_total > 0 && v_total > 0 {
            let product = h_total.saturating_mul(v_total);
            if product > 0 {
                pixel_clock_khz.saturating_mul(1000) / product
            } else {
                0
            }
        } else {
            0
        };

        modes.push(DisplayMode {
            width: h_active,
            height: v_active,
            refresh_hz: refresh,
            pixel_clock_khz,
            h_blank: h_blank as u16,
            h_sync_start: h_sync_off,
            h_sync_end: h_sync_off.saturating_add(h_sync_width),
            v_blank: v_blank as u16,
            v_sync_start: v_sync_off,
            v_sync_end: v_sync_off.saturating_add(v_sync_width),
            preferred: desc_idx == 0,
        });
    }

    modes
}

/// Extract monitor name from EDID descriptor blocks
fn parse_monitor_name(edid: &[u8; EDID_BLOCK_SIZE]) -> String {
    for desc_idx in 0..4usize {
        let offset = 54usize.saturating_add(desc_idx.saturating_mul(18));
        // Monitor name descriptor: tag = 0xFC
        if edid[offset] == 0 && edid[offset + 1] == 0 && edid[offset + 3] == 0xFC {
            let name_bytes = &edid[offset + 5..offset + 18];
            let mut name = String::new();
            for &b in name_bytes {
                if b == 0x0A || b == 0 {
                    break;
                }
                name.push(b as char);
            }
            return name;
        }
    }
    String::from("Unknown Monitor")
}

// ---------------------------------------------------------------------------
// Internal implementation
// ---------------------------------------------------------------------------

impl HdmiInner {
    #[inline(always)]
    fn reg(&self, offset: usize) -> usize {
        self.base_addr.saturating_add(offset)
    }

    /// Check hotplug status
    fn check_hotplug(&mut self) -> bool {
        let hpd = mmio_read32(self.reg(REG_HPD));
        let connected = hpd & HPD_DETECTED != 0;

        if connected && self.connection == ConnectionState::Disconnected {
            self.connection = ConnectionState::Connected;
            serial_println!("  HDMI: display connected (hotplug)");
            // Clear HPD IRQ
            mmio_write32(self.reg(REG_HPD), HPD_IRQ);
            return true;
        } else if !connected && self.connection != ConnectionState::Disconnected {
            self.connection = ConnectionState::Disconnected;
            self.active_mode = None;
            self.hdcp = HdcpState::Disabled;
            self.audio_enabled = false;
            serial_println!("  HDMI: display disconnected");
        }
        false
    }

    /// Read and parse EDID
    fn read_edid(&mut self) -> Result<(), HdmiError> {
        let edid = read_edid_block(self.base_addr, 0)?;
        self.modes = parse_edid_modes(&edid);
        self.monitor_name = parse_monitor_name(&edid);

        // Extract manufacturer ID
        let mfg_id = ((edid[8] as u16) << 8) | edid[9] as u16;
        self.manufacturer[0] = b'@' + ((mfg_id >> 10) & 0x1F) as u8;
        self.manufacturer[1] = b'@' + ((mfg_id >> 5) & 0x1F) as u8;
        self.manufacturer[2] = b'@' + (mfg_id & 0x1F) as u8;
        self.manufacturer[3] = 0;

        serial_println!(
            "  HDMI: EDID parsed - '{}', {} mode(s)",
            self.monitor_name,
            self.modes.len()
        );
        for (i, mode) in self.modes.iter().enumerate() {
            serial_println!(
                "    mode {}: {}x{}@{} Hz (pclk {} kHz){}",
                i,
                mode.width,
                mode.height,
                mode.refresh_hz,
                mode.pixel_clock_khz,
                if mode.preferred { " [preferred]" } else { "" }
            );
        }
        Ok(())
    }

    /// Program display timing registers for a mode
    fn set_mode_inner(&mut self, mode: &DisplayMode) -> Result<(), HdmiError> {
        // Disable output while reprogramming
        let ctrl = mmio_read32(self.reg(REG_CONTROL));
        mmio_write32(self.reg(REG_CONTROL), ctrl & !CTRL_ENABLE);

        // Program timing
        mmio_write32(self.reg(REG_H_ACTIVE), mode.width);
        mmio_write32(self.reg(REG_H_BLANK), mode.h_blank as u32);
        mmio_write32(
            self.reg(REG_H_SYNC),
            (mode.h_sync_start as u32) | ((mode.h_sync_end as u32) << 16),
        );
        mmio_write32(self.reg(REG_V_ACTIVE), mode.height);
        mmio_write32(self.reg(REG_V_BLANK), mode.v_blank as u32);
        mmio_write32(
            self.reg(REG_V_SYNC),
            (mode.v_sync_start as u32) | ((mode.v_sync_end as u32) << 16),
        );
        mmio_write32(self.reg(REG_PIXEL_CLK), mode.pixel_clock_khz);

        // Re-enable in HDMI mode
        mmio_write32(self.reg(REG_CONTROL), CTRL_ENABLE | CTRL_HDMI_MODE);

        // Wait for link to stabilize
        for _ in 0..TIMEOUT_SPINS {
            if mmio_read32(self.reg(REG_STATUS)) & STATUS_LINK_OK != 0 {
                self.active_mode = Some(*mode);
                self.connection = ConnectionState::LinkActive;
                return Ok(());
            }
        }

        Err(HdmiError::Timeout)
    }

    /// Start HDCP authentication
    fn start_hdcp(&mut self) -> Result<(), HdmiError> {
        self.hdcp = HdcpState::Authenticating;
        mmio_write32(self.reg(REG_HDCP_CTRL), HDCP_ENABLE);

        for _ in 0..TIMEOUT_SPINS {
            let status = mmio_read32(self.reg(REG_HDCP_STATUS));
            if status & HDCP_AUTHENTICATED != 0 {
                self.hdcp = HdcpState::Authenticated;
                serial_println!(
                    "  HDMI: HDCP authenticated{}",
                    if status & HDCP_REPEATER != 0 {
                        " (repeater)"
                    } else {
                        ""
                    }
                );
                return Ok(());
            }
        }

        self.hdcp = HdcpState::Failed;
        serial_println!("  HDMI: HDCP authentication failed");
        Err(HdmiError::HdcpFailed)
    }

    /// Enable or disable audio passthrough
    fn set_audio(&mut self, enable: bool, sample_rate_hz: u32) {
        if enable {
            // Compute CTS and N values for audio clock regeneration
            // N = 6144 for 48 kHz (standard HDMI audio)
            let n = match sample_rate_hz {
                32000 => 4096u32,
                44100 => 6272,
                48000 => 6144,
                _ => 6144,
            };
            let pclk = self.active_mode.map_or(148500, |m| m.pixel_clock_khz);
            // CTS = pixel_clock * N / (128 * sample_rate / 1000)
            let cts = if sample_rate_hz > 0 {
                (pclk as u64 * n as u64 * 1000) / (128 * sample_rate_hz as u64)
            } else {
                0
            };

            mmio_write32(self.reg(REG_AUDIO_N), n);
            mmio_write32(self.reg(REG_AUDIO_CTS), cts as u32);
            mmio_write32(self.reg(REG_AUDIO_CTRL), AUDIO_ENABLE);
            self.audio_enabled = true;
        } else {
            mmio_write32(self.reg(REG_AUDIO_CTRL), AUDIO_MUTE);
            self.audio_enabled = false;
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Check for hotplug events. Returns true if a new display was connected.
pub fn check_hotplug() -> bool {
    let mut guard = HDMI.lock();
    match guard.as_mut() {
        Some(inner) => inner.check_hotplug(),
        None => false,
    }
}

/// Get available display modes.
pub fn get_modes() -> Vec<DisplayMode> {
    HDMI.lock()
        .as_ref()
        .map_or(Vec::new(), |inner| inner.modes.clone())
}

/// Set display mode by resolution and refresh rate.
pub fn set_mode(width: u32, height: u32, refresh: u32) -> Result<(), HdmiError> {
    let mut guard = HDMI.lock();
    let inner = guard.as_mut().ok_or(HdmiError::NotInitialized)?;
    if inner.connection == ConnectionState::Disconnected {
        return Err(HdmiError::NotConnected);
    }
    let mode = inner
        .modes
        .iter()
        .find(|m| m.width == width && m.height == height && m.refresh_hz == refresh)
        .copied()
        .ok_or(HdmiError::InvalidMode)?;
    inner.set_mode_inner(&mode)
}

/// Set the preferred (native) display mode.
pub fn set_preferred_mode() -> Result<(), HdmiError> {
    let mut guard = HDMI.lock();
    let inner = guard.as_mut().ok_or(HdmiError::NotInitialized)?;
    if inner.connection == ConnectionState::Disconnected {
        return Err(HdmiError::NotConnected);
    }
    let mode = inner
        .modes
        .iter()
        .find(|m| m.preferred)
        .or_else(|| inner.modes.first())
        .copied()
        .ok_or(HdmiError::InvalidMode)?;
    inner.set_mode_inner(&mode)
}

/// Enable HDCP content protection.
pub fn enable_hdcp() -> Result<(), HdmiError> {
    let mut guard = HDMI.lock();
    let inner = guard.as_mut().ok_or(HdmiError::NotInitialized)?;
    inner.start_hdcp()
}

/// Get HDCP state.
pub fn hdcp_state() -> HdcpState {
    HDMI.lock().as_ref().map_or(HdcpState::Disabled, |i| i.hdcp)
}

/// Enable audio passthrough at the given sample rate.
pub fn enable_audio(sample_rate_hz: u32) -> Result<(), HdmiError> {
    let mut guard = HDMI.lock();
    let inner = guard.as_mut().ok_or(HdmiError::NotInitialized)?;
    inner.set_audio(true, sample_rate_hz);
    serial_println!("  HDMI: audio enabled ({} Hz)", sample_rate_hz);
    Ok(())
}

/// Disable audio passthrough.
pub fn disable_audio() -> Result<(), HdmiError> {
    let mut guard = HDMI.lock();
    let inner = guard.as_mut().ok_or(HdmiError::NotInitialized)?;
    inner.set_audio(false, 0);
    Ok(())
}

/// Get current connection state.
pub fn connection_state() -> ConnectionState {
    HDMI.lock()
        .as_ref()
        .map_or(ConnectionState::Disconnected, |i| i.connection)
}

/// Get monitor name from EDID.
pub fn monitor_name() -> String {
    HDMI.lock()
        .as_ref()
        .map_or(String::from("N/A"), |i| i.monitor_name.clone())
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the HDMI output driver.
///
/// Probes for an HDMI transmitter at a known MMIO address, checks hotplug,
/// and reads EDID if a display is connected.
pub fn init() {
    const HDMI_BASE: usize = 0xFE04_0000;

    // Probe: read status register
    let status = mmio_read32(HDMI_BASE.saturating_add(REG_STATUS));
    if status == 0xFFFF_FFFF {
        serial_println!("  HDMI: no transmitter found at {:#010X}", HDMI_BASE);
        return;
    }

    let mut inner = HdmiInner {
        base_addr: HDMI_BASE,
        connection: ConnectionState::Disconnected,
        modes: Vec::new(),
        active_mode: None,
        hdcp: HdcpState::Disabled,
        audio_enabled: false,
        manufacturer: [0; 4],
        monitor_name: String::new(),
    };

    // Check if display is connected
    let hpd = mmio_read32(HDMI_BASE.saturating_add(REG_HPD));
    if hpd & HPD_DETECTED != 0 {
        inner.connection = ConnectionState::Connected;
        serial_println!("  HDMI: display detected, reading EDID...");
        if inner.read_edid().is_err() {
            serial_println!("  HDMI: EDID read failed, limited mode");
        }
    } else {
        serial_println!("  HDMI: no display connected (hotplug monitoring active)");
    }

    *HDMI.lock() = Some(inner);
    super::register("hdmi", super::DeviceType::Display);
}

// ---------------------------------------------------------------------------
// Output control API
// ---------------------------------------------------------------------------

/// Enable HDMI video output.
///
/// Sets `CTRL_ENABLE` and `CTRL_HDMI_MODE` in the main control register.
/// Idempotent — safe to call when the output is already active.
pub fn enable_output() {
    let guard = HDMI.lock();
    if let Some(ref inner) = *guard {
        let base = inner.base_addr;
        let ctrl = mmio_read32(base.saturating_add(REG_CONTROL));
        mmio_write32(
            base.saturating_add(REG_CONTROL),
            ctrl | CTRL_ENABLE | CTRL_HDMI_MODE,
        );
        serial_println!("  HDMI: output enabled");
    }
}

/// Disable HDMI video output.
///
/// Clears `CTRL_ENABLE` in the main control register.  HDCP and audio state
/// are preserved so the output can be re-enabled without full re-authentication.
pub fn disable_output() {
    let guard = HDMI.lock();
    if let Some(ref inner) = *guard {
        let base = inner.base_addr;
        let ctrl = mmio_read32(base.saturating_add(REG_CONTROL));
        mmio_write32(base.saturating_add(REG_CONTROL), ctrl & !CTRL_ENABLE);
        serial_println!("  HDMI: output disabled");
    }
}

/// Read the EDID from the connected HDMI display via DDC.
///
/// If no display is connected, the DDC read fails, or the header is invalid,
/// returns a fixed fallback EDID describing a generic 1920×1080 @ 60 Hz
/// monitor so the caller always receives a usable descriptor.
pub fn get_edid() -> [u8; EDID_BLOCK_SIZE] {
    // Fixed fallback: generic 1920×1080 @ 60 Hz.
    const FALLBACK: [u8; EDID_BLOCK_SIZE] = [
        0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x1E, 0x6D, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x01, 0x1B, 0x01, 0x03, 0x80, 0x35, 0x1E, 0x78, 0xEA, 0xEE, 0x91, 0xA3, 0x54, 0x4C,
        0x99, 0x26, 0x0F, 0x50, 0x54, 0xA1, 0x08, 0x00, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
        0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, // 1080p60 detailed timing
        0x02, 0x3A, 0x80, 0x18, 0x71, 0x38, 0x2D, 0x40, 0x58, 0x2C, 0x45, 0x00, 0xF4, 0x19, 0x11,
        0x00, 0x00, 0x1E, 0x00, 0x00, 0x00, 0xFC, 0x00, 0x47, 0x65, 0x6E, 0x65, 0x73, 0x69, 0x73,
        0x0A, 0x20, 0x20, 0x20, 0x20, 0x20, 0x00, 0x00, 0x00, 0xFD, 0x00, 0x32, 0x4B, 0x18, 0x53,
        0x11, 0x00, 0x0A, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, // padding
        0x00, 0x00, // extension count + checksum
    ];

    let guard = HDMI.lock();
    let inner = match *guard {
        Some(ref i) => i,
        None => return FALLBACK,
    };

    match read_edid_block(inner.base_addr, 0) {
        Ok(edid) => {
            // Validate EDID magic header before accepting.
            const MAGIC: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
            if edid[..8] == MAGIC {
                edid
            } else {
                FALLBACK
            }
        }
        Err(_) => FALLBACK,
    }
}

/// Set display backlight brightness via PWM channel 0.
///
/// `level` is a raw 0–255 brightness value (0 = off, 255 = maximum).
/// Delegates to the PWM driver's `set_duty_percent` function on channel 0.
/// No-op if the PWM subsystem is not initialized.
pub fn set_brightness(level: u8) {
    crate::drivers::pwm::set_duty_percent(0, level);
    serial_println!("  HDMI: brightness set to {}", level);
}
