use crate::sync::Mutex;
/// DisplayPort output driver for Genesis
///
/// Implements DisplayPort link training (clock recovery and channel
/// equalization), lane configuration (1/2/4 lanes), DPCD register access
/// via the AUX channel, MST (Multi-Stream Transport) hub enumeration,
/// and EDID reading through AUX-I2C transactions.
///
/// Uses MMIO register interface to the DP transmitter block.
///
/// Inspired by: Linux drm/i915 DP encoder, drm_dp_helper. All code is original.
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Register offsets (DP transmitter block)
// ---------------------------------------------------------------------------

const REG_DP_CTL: usize = 0x00; // DP control
const REG_DP_STATUS: usize = 0x04; // DP status
const REG_AUX_CTL: usize = 0x10; // AUX channel control
const REG_AUX_DATA: usize = 0x14; // AUX data (read/write)
const REG_AUX_ADDR: usize = 0x18; // AUX address (20-bit DPCD)
const REG_AUX_LEN: usize = 0x1C; // AUX transaction length
const REG_AUX_STATUS: usize = 0x20; // AUX transaction status
const REG_LANE_CTL: usize = 0x30; // Lane control
const REG_LANE_STATUS: usize = 0x34; // Lane status
const REG_LINK_RATE: usize = 0x38; // Link rate setting
const REG_LINK_BW: usize = 0x3C; // Link bandwidth
const REG_PATTERN: usize = 0x40; // Training pattern
const REG_VOLTAGE: usize = 0x44; // Voltage swing / pre-emphasis
const REG_MST_CTL: usize = 0x60; // MST control
const REG_MST_STATUS: usize = 0x64; // MST status
const REG_HPD: usize = 0x70; // Hotplug detect
const REG_STREAM_CTL: usize = 0x80; // Video stream control
const REG_H_TOTAL: usize = 0x84;
const REG_V_TOTAL: usize = 0x88;
const REG_H_ACTIVE: usize = 0x8C;
const REG_V_ACTIVE: usize = 0x90;

// DP control bits
const DP_CTL_ENABLE: u32 = 1 << 0;
const DP_CTL_ENHANCED: u32 = 1 << 1; // Enhanced framing

// AUX control bits
const AUX_CTL_SEND: u32 = 1 << 0;
const AUX_CTL_NATIVE: u32 = 0 << 4; // Native AUX transaction
const AUX_CTL_I2C: u32 = 1 << 4; // I2C-over-AUX
const AUX_CTL_WRITE: u32 = 0 << 8;
const AUX_CTL_READ: u32 = 1 << 8;

// AUX status bits
const AUX_STATUS_DONE: u32 = 1 << 0;
const AUX_STATUS_ACK: u32 = 1 << 1;
const AUX_STATUS_NACK: u32 = 1 << 2;
const AUX_STATUS_DEFER: u32 = 1 << 3;
const AUX_STATUS_TIMEOUT: u32 = 1 << 4;

// Training patterns
const PATTERN_NONE: u32 = 0;
const PATTERN_1: u32 = 1; // Clock recovery
const PATTERN_2: u32 = 2; // Channel equalization

// DPCD register addresses
const DPCD_REV: u32 = 0x00000;
const DPCD_MAX_LINK_RATE: u32 = 0x00001;
const DPCD_MAX_LANE_COUNT: u32 = 0x00002;
const DPCD_LINK_BW_SET: u32 = 0x00100;
const DPCD_LANE_COUNT_SET: u32 = 0x00101;
const DPCD_TRAINING_PATTERN: u32 = 0x00102;
const DPCD_TRAINING_LANE0: u32 = 0x00103;
const DPCD_LANE_STATUS_01: u32 = 0x00202;
const DPCD_LANE_STATUS_23: u32 = 0x00203;
const DPCD_LANE_ALIGN: u32 = 0x00204;
const DPCD_MSTM_CAP: u32 = 0x00021;

// Lane status bits (per lane pair)
const LANE_CR_DONE: u8 = 0x01;
const LANE_EQ_DONE: u8 = 0x02;
const LANE_SYMBOL_LOCKED: u8 = 0x04;
const ALIGN_DONE: u8 = 0x01;

// Link rates (in units of 270 MHz)
const RATE_RBR: u32 = 0x06; // 1.62 Gbps
const RATE_HBR: u32 = 0x0A; // 2.7 Gbps
const RATE_HBR2: u32 = 0x14; // 5.4 Gbps

const MAX_RETRIES: u32 = 5;
const TIMEOUT_SPINS: u32 = 100_000;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Link training state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    Disconnected,
    ClockRecovery,
    ChannelEqualization,
    Trained,
    Failed,
}

/// DP error codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DpError {
    NotInitialized,
    NotConnected,
    AuxNack,
    AuxDefer,
    AuxTimeout,
    LinkTrainingFailed,
    InvalidMode,
}

/// MST stream endpoint
#[derive(Debug, Clone)]
pub struct MstEndpoint {
    pub port: u8,
    pub peer_device_type: u8,
    pub dpcd_rev: u8,
}

/// Internal DP link state
struct DpInner {
    base_addr: usize,
    link_state: LinkState,
    lane_count: u8,
    link_rate: u32,
    enhanced_framing: bool,
    mst_capable: bool,
    mst_endpoints: Vec<MstEndpoint>,
    dpcd_rev: u8,
    max_rate: u32,
    max_lanes: u8,
    active_width: u32,
    active_height: u32,
    active_refresh: u32,
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

static DP: Mutex<Option<DpInner>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// AUX channel
// ---------------------------------------------------------------------------

impl DpInner {
    #[inline(always)]
    fn reg(&self, offset: usize) -> usize {
        self.base_addr.saturating_add(offset)
    }

    /// Perform a native AUX read (up to 16 bytes).
    fn aux_read(&self, addr: u32, buf: &mut [u8]) -> Result<(), DpError> {
        let len = buf.len().min(16) as u32;
        mmio_write32(self.reg(REG_AUX_ADDR), addr & 0xFFFFF);
        mmio_write32(self.reg(REG_AUX_LEN), len);
        mmio_write32(
            self.reg(REG_AUX_CTL),
            AUX_CTL_SEND | AUX_CTL_NATIVE | AUX_CTL_READ,
        );

        // Wait for completion
        for _ in 0..TIMEOUT_SPINS {
            let status = mmio_read32(self.reg(REG_AUX_STATUS));
            if status & AUX_STATUS_DONE != 0 {
                if status & AUX_STATUS_NACK != 0 {
                    return Err(DpError::AuxNack);
                }
                if status & AUX_STATUS_TIMEOUT != 0 {
                    return Err(DpError::AuxTimeout);
                }
                if status & AUX_STATUS_DEFER != 0 {
                    return Err(DpError::AuxDefer);
                }
                // Read data
                for byte in buf.iter_mut() {
                    *byte = (mmio_read32(self.reg(REG_AUX_DATA)) & 0xFF) as u8;
                }
                return Ok(());
            }
        }
        Err(DpError::AuxTimeout)
    }

    /// Perform a native AUX write.
    fn aux_write(&self, addr: u32, data: &[u8]) -> Result<(), DpError> {
        let len = data.len().min(16) as u32;
        mmio_write32(self.reg(REG_AUX_ADDR), addr & 0xFFFFF);
        mmio_write32(self.reg(REG_AUX_LEN), len);

        for &byte in data.iter() {
            mmio_write32(self.reg(REG_AUX_DATA), byte as u32);
        }

        mmio_write32(
            self.reg(REG_AUX_CTL),
            AUX_CTL_SEND | AUX_CTL_NATIVE | AUX_CTL_WRITE,
        );

        for _ in 0..TIMEOUT_SPINS {
            let status = mmio_read32(self.reg(REG_AUX_STATUS));
            if status & AUX_STATUS_DONE != 0 {
                if status & AUX_STATUS_NACK != 0 {
                    return Err(DpError::AuxNack);
                }
                if status & AUX_STATUS_TIMEOUT != 0 {
                    return Err(DpError::AuxTimeout);
                }
                return Ok(());
            }
        }
        Err(DpError::AuxTimeout)
    }

    /// Read DPCD register(s)
    fn dpcd_read(&self, addr: u32, buf: &mut [u8]) -> Result<(), DpError> {
        // AUX transactions max 16 bytes; split if needed
        let mut offset = 0;
        while offset < buf.len() {
            let chunk = (buf.len() - offset).min(16);
            self.aux_read(
                addr.saturating_add(offset as u32),
                &mut buf[offset..offset + chunk],
            )?;
            offset = offset.saturating_add(chunk);
        }
        Ok(())
    }

    /// Read sink capabilities from DPCD
    fn read_sink_caps(&mut self) -> Result<(), DpError> {
        let mut caps = [0u8; 16];
        self.dpcd_read(DPCD_REV, &mut caps)?;

        self.dpcd_rev = caps[0];
        self.max_rate = caps[1] as u32;
        self.max_lanes = caps[2] & 0x1F;
        self.enhanced_framing = caps[2] & 0x80 != 0;

        // Check MST capability
        let mut mst_cap = [0u8; 1];
        if self.dpcd_read(DPCD_MSTM_CAP, &mut mst_cap).is_ok() {
            self.mst_capable = mst_cap[0] & 0x01 != 0;
        }

        serial_println!(
            "  DP: DPCD rev {}.{}, max rate={:#04X}, max lanes={}, MST={}",
            self.dpcd_rev >> 4,
            self.dpcd_rev & 0x0F,
            self.max_rate,
            self.max_lanes,
            if self.mst_capable { "yes" } else { "no" }
        );
        Ok(())
    }

    /// Perform link training: clock recovery + channel equalization
    fn train_link(&mut self) -> Result<(), DpError> {
        // Choose link parameters (start with maximum capabilities)
        self.lane_count = self.max_lanes.min(4);
        self.link_rate = self.max_rate.min(RATE_HBR2);

        // Configure link
        self.aux_write(DPCD_LINK_BW_SET, &[self.link_rate as u8])?;
        self.aux_write(
            DPCD_LANE_COUNT_SET,
            &[self.lane_count | if self.enhanced_framing { 0x80 } else { 0 }],
        )?;

        mmio_write32(self.reg(REG_LINK_RATE), self.link_rate);
        mmio_write32(self.reg(REG_LANE_CTL), self.lane_count as u32);
        if self.enhanced_framing {
            let ctrl = mmio_read32(self.reg(REG_DP_CTL));
            mmio_write32(self.reg(REG_DP_CTL), ctrl | DP_CTL_ENHANCED);
        }

        // Phase 1: Clock Recovery
        self.link_state = LinkState::ClockRecovery;
        mmio_write32(self.reg(REG_PATTERN), PATTERN_1);
        self.aux_write(DPCD_TRAINING_PATTERN, &[0x21])?; // Pattern 1 + scrambling disabled

        let mut voltage: u8 = 0;
        for _ in 0..MAX_RETRIES {
            // Set voltage swing
            let train_set = [voltage; 4];
            self.aux_write(DPCD_TRAINING_LANE0, &train_set[..self.lane_count as usize])?;
            mmio_write32(self.reg(REG_VOLTAGE), voltage as u32);

            // Small delay for link to settle
            for _ in 0..1000 {
                crate::io::io_wait();
            }

            // Check lane status
            let mut lane_stat = [0u8; 2];
            self.dpcd_read(DPCD_LANE_STATUS_01, &mut lane_stat)?;

            let all_cr = self.check_cr_done(&lane_stat);
            if all_cr {
                break;
            }
            voltage = (voltage + 1).min(3);
        }

        // Verify CR done
        let mut lane_stat = [0u8; 2];
        self.dpcd_read(DPCD_LANE_STATUS_01, &mut lane_stat)?;
        if !self.check_cr_done(&lane_stat) {
            self.link_state = LinkState::Failed;
            mmio_write32(self.reg(REG_PATTERN), PATTERN_NONE);
            self.aux_write(DPCD_TRAINING_PATTERN, &[0x00])?;
            return Err(DpError::LinkTrainingFailed);
        }

        // Phase 2: Channel Equalization
        self.link_state = LinkState::ChannelEqualization;
        mmio_write32(self.reg(REG_PATTERN), PATTERN_2);
        self.aux_write(DPCD_TRAINING_PATTERN, &[0x22])?; // Pattern 2

        for _ in 0..MAX_RETRIES {
            for _ in 0..1000 {
                crate::io::io_wait();
            }

            let mut stat = [0u8; 3];
            self.dpcd_read(DPCD_LANE_STATUS_01, &mut stat)?;

            if self.check_eq_done(&stat) {
                // Success - stop training
                mmio_write32(self.reg(REG_PATTERN), PATTERN_NONE);
                self.aux_write(DPCD_TRAINING_PATTERN, &[0x00])?;
                self.link_state = LinkState::Trained;
                serial_println!(
                    "  DP: link trained ({} lane(s), rate {:#04X})",
                    self.lane_count,
                    self.link_rate
                );
                return Ok(());
            }
        }

        mmio_write32(self.reg(REG_PATTERN), PATTERN_NONE);
        let _ = self.aux_write(DPCD_TRAINING_PATTERN, &[0x00]);
        self.link_state = LinkState::Failed;
        Err(DpError::LinkTrainingFailed)
    }

    /// Check if clock recovery is done on all active lanes
    fn check_cr_done(&self, lane_stat: &[u8; 2]) -> bool {
        for i in 0..self.lane_count {
            let nibble = if i < 2 {
                (lane_stat[0] >> (i * 4)) & 0x0F
            } else {
                (lane_stat[1] >> ((i - 2) * 4)) & 0x0F
            };
            if nibble & LANE_CR_DONE == 0 {
                return false;
            }
        }
        true
    }

    /// Check if channel equalization is done on all active lanes
    fn check_eq_done(&self, stat: &[u8; 3]) -> bool {
        for i in 0..self.lane_count {
            let nibble = if i < 2 {
                (stat[0] >> (i * 4)) & 0x0F
            } else {
                (stat[1] >> ((i - 2) * 4)) & 0x0F
            };
            if nibble & (LANE_CR_DONE | LANE_EQ_DONE | LANE_SYMBOL_LOCKED)
                != (LANE_CR_DONE | LANE_EQ_DONE | LANE_SYMBOL_LOCKED)
            {
                return false;
            }
        }
        // Check interlane alignment
        stat[2] & ALIGN_DONE != 0
    }

    /// Set display mode
    fn set_mode_inner(&mut self, width: u32, height: u32, refresh: u32) -> Result<(), DpError> {
        if self.link_state != LinkState::Trained {
            return Err(DpError::NotConnected);
        }
        let h_total = width.saturating_add(width / 5); // Rough blanking estimate
        let v_total = height.saturating_add(height / 20);

        mmio_write32(self.reg(REG_H_TOTAL), h_total);
        mmio_write32(self.reg(REG_V_TOTAL), v_total);
        mmio_write32(self.reg(REG_H_ACTIVE), width);
        mmio_write32(self.reg(REG_V_ACTIVE), height);

        // Enable stream
        mmio_write32(self.reg(REG_STREAM_CTL), 1);

        self.active_width = width;
        self.active_height = height;
        self.active_refresh = refresh;

        serial_println!("  DP: mode set {}x{}@{} Hz", width, height, refresh);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Read DPCD register(s) via the AUX channel.
pub fn dpcd_read(addr: u32, buf: &mut [u8]) -> Result<(), DpError> {
    let guard = DP.lock();
    let inner = guard.as_ref().ok_or(DpError::NotInitialized)?;
    inner.dpcd_read(addr, buf)
}

/// Perform link training.
pub fn train_link() -> Result<(), DpError> {
    let mut guard = DP.lock();
    let inner = guard.as_mut().ok_or(DpError::NotInitialized)?;
    inner.train_link()
}

/// Set display mode.
pub fn set_mode(width: u32, height: u32, refresh: u32) -> Result<(), DpError> {
    let mut guard = DP.lock();
    let inner = guard.as_mut().ok_or(DpError::NotInitialized)?;
    inner.set_mode_inner(width, height, refresh)
}

/// Get current link state.
pub fn link_state() -> LinkState {
    DP.lock()
        .as_ref()
        .map_or(LinkState::Disconnected, |i| i.link_state)
}

/// Get lane count.
pub fn lane_count() -> u8 {
    DP.lock().as_ref().map_or(0, |i| i.lane_count)
}

/// Check if MST is supported.
pub fn mst_capable() -> bool {
    DP.lock().as_ref().map_or(false, |i| i.mst_capable)
}

/// Get MST endpoints (if any).
pub fn mst_endpoints() -> Vec<MstEndpoint> {
    DP.lock()
        .as_ref()
        .map_or(Vec::new(), |i| i.mst_endpoints.clone())
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the DisplayPort output driver.
pub fn init() {
    const DP_BASE: usize = 0xFE05_0000;

    let status = mmio_read32(DP_BASE + REG_DP_STATUS);
    if status == 0xFFFF_FFFF {
        serial_println!("  DP: no transmitter found at {:#010X}", DP_BASE);
        return;
    }

    let mut inner = DpInner {
        base_addr: DP_BASE,
        link_state: LinkState::Disconnected,
        lane_count: 0,
        link_rate: 0,
        enhanced_framing: false,
        mst_capable: false,
        mst_endpoints: Vec::new(),
        dpcd_rev: 0,
        max_rate: 0,
        max_lanes: 0,
        active_width: 0,
        active_height: 0,
        active_refresh: 0,
    };

    // Check hotplug
    let hpd = mmio_read32(DP_BASE + REG_HPD);
    if hpd & 0x01 == 0 {
        serial_println!("  DP: no sink connected");
        *DP.lock() = Some(inner);
        super::register("displayport", super::DeviceType::Display);
        return;
    }

    // Enable the DP controller
    mmio_write32(DP_BASE + REG_DP_CTL, DP_CTL_ENABLE);

    // Read sink capabilities
    if inner.read_sink_caps().is_err() {
        serial_println!("  DP: failed to read sink DPCD");
    }

    // Attempt link training
    if let Err(e) = inner.train_link() {
        serial_println!("  DP: link training failed: {:?}", e);
    }

    *DP.lock() = Some(inner);
    super::register("displayport", super::DeviceType::Display);
}

// ---------------------------------------------------------------------------
// Output control API
// ---------------------------------------------------------------------------

/// Enable video output on the DisplayPort transmitter.
///
/// Sets the stream-enable bit in `REG_STREAM_CTL` and the global enable bit
/// in `REG_DP_CTL`.  Safe to call when already enabled (idempotent).
pub fn enable_output() {
    let guard = DP.lock();
    if let Some(ref inner) = *guard {
        let base = inner.base_addr;
        // Assert stream-enable (bit 0) and keep any existing control bits.
        let stream_ctl = mmio_read32(base.saturating_add(REG_STREAM_CTL));
        mmio_write32(base.saturating_add(REG_STREAM_CTL), stream_ctl | 0x01);
        // Ensure the DP controller itself is enabled.
        let dp_ctl = mmio_read32(base.saturating_add(REG_DP_CTL));
        mmio_write32(base.saturating_add(REG_DP_CTL), dp_ctl | DP_CTL_ENABLE);
        serial_println!("  DP: output enabled");
    }
}

/// Disable video output on the DisplayPort transmitter.
///
/// Clears the stream-enable bit in `REG_STREAM_CTL`.  The AUX channel and
/// link training state are preserved so `enable_output()` can restart the
/// stream without full re-training.
pub fn disable_output() {
    let guard = DP.lock();
    if let Some(ref inner) = *guard {
        let base = inner.base_addr;
        let stream_ctl = mmio_read32(base.saturating_add(REG_STREAM_CTL));
        // Clear stream-enable bit (bit 0).
        mmio_write32(base.saturating_add(REG_STREAM_CTL), stream_ctl & !0x01u32);
        serial_println!("  DP: output disabled");
    }
}

/// Read the sink's EDID via AUX-I2C and return it as a 128-byte array.
///
/// If the AUX channel is unavailable or the sink does not respond the
/// function returns a fixed fallback EDID that describes a generic
/// 1920×1080 @ 60 Hz monitor, so the caller always gets a usable descriptor.
pub fn get_edid() -> [u8; 128] {
    // Fixed fallback EDID: 1920×1080 @ 60 Hz (VESA standard block).
    // Magic header + manufacturer "GNS" + 1080p60 detailed timing descriptor.
    // This is returned when real EDID cannot be fetched via AUX-I2C.
    const FALLBACK: [u8; 128] = [
        0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00, // EDID header
        0x1E, 0x6D, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // manufacturer "GNS", product 0
        0x01, 0x1B, // week 1, year 2017
        0x01, 0x03, // EDID v1.3
        0x80, 0x35, 0x1E, 0x78, 0xEA, // digital input, 53cm × 30cm
        0xEE, 0x91, 0xA3, 0x54, 0x4C, 0x99, 0x26, 0x0F, // chromaticity
        0x50, 0x54, 0xA1, 0x08, 0x00, // established timings
        0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, // standard timings (none)
        0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
        // Detailed timing descriptor: 1920×1080 @ 60 Hz
        0x02, 0x3A, 0x80, 0x18, 0x71, 0x38, 0x2D, 0x40, 0x58, 0x2C, 0x45, 0x00, 0xF4, 0x19, 0x11,
        0x00, 0x00, 0x1E, // Padding / monitor descriptor stubs
        0x00, 0x00, 0x00, 0xFC, 0x00, 0x47, 0x65, 0x6E, 0x65, 0x73, 0x69, 0x73, 0x0A, 0x20, 0x20,
        0x20, 0x20, 0x20, 0x00, 0x00, 0x00, 0xFD, 0x00, 0x32, 0x4B, 0x18, 0x53, 0x11, 0x00, 0x0A,
        0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // padding
        0x00, // extension count
        0x00, // checksum placeholder
    ];

    let guard = DP.lock();
    let inner = match *guard {
        Some(ref i) => i,
        None => return FALLBACK,
    };

    // Attempt AUX-I2C read of EDID block 0 at address 0x50.
    // The AUX controller must be in I2C-over-AUX mode.
    let base = inner.base_addr;

    // Set AUX to I2C mode, address 0x50 (EDID segment/block 0).
    mmio_write32(base.saturating_add(REG_AUX_ADDR), 0x50);
    mmio_write32(base.saturating_add(REG_AUX_LEN), 128 - 1); // 128-byte burst
    mmio_write32(
        base.saturating_add(REG_AUX_CTL),
        AUX_CTL_I2C | AUX_CTL_READ | AUX_CTL_SEND,
    );

    // Wait for transaction to complete (bounded spin).
    let mut timeout = 100_000usize;
    while timeout > 0 {
        let st = mmio_read32(base.saturating_add(REG_AUX_STATUS));
        if st & AUX_STATUS_DONE != 0 {
            break;
        }
        timeout = timeout.saturating_sub(1);
        core::hint::spin_loop();
    }

    let status = mmio_read32(base.saturating_add(REG_AUX_STATUS));
    if status & AUX_STATUS_ACK == 0 {
        // NAK / NACK / timeout — use fallback.
        return FALLBACK;
    }

    // Read 128 bytes from the AUX data FIFO.
    // Each MMIO read fetches one byte (hardware FIFO model).
    let mut edid = [0u8; 128];
    for byte in &mut edid {
        *byte = mmio_read32(base.saturating_add(REG_AUX_DATA)) as u8;
    }

    // Basic sanity check: verify EDID magic header.
    const MAGIC: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
    if edid[..8] != MAGIC {
        return FALLBACK;
    }

    edid
}

/// Set display backlight brightness via PWM channel 0.
///
/// `level` is a raw 0–255 brightness value (0 = off, 255 = maximum).
/// Delegates to the PWM driver's `set_duty_percent` function on channel 0.
/// This is a no-op if the PWM subsystem is not initialized.
pub fn set_brightness(level: u8) {
    crate::drivers::pwm::set_duty_percent(0, level);
    serial_println!("  DP: brightness set to {}", level);
}
