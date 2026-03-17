use super::modesetting::{MODE_1024x768_60, ModeInfo};
use crate::io::inb;
use crate::serial_println;
/// DRM connector abstraction for Genesis — built from scratch
///
/// A connector represents a physical display output (VGA, DVI, HDMI, DP, LVDS).
/// Each connector has a detected status and a preferred display mode.
///
/// Status detection for VGA uses the DAC sense bit in the VGA input status
/// register (port 0x3C2, bit 4): set when a monitor pulls down the DAC lines.
///
/// No heap, no floats, no panics. Fixed array of 4 connectors.
///
/// No external crates. All code is original.
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// ConnectorType
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ConnectorType {
    VGA,
    DVI,
    HDMI,
    DisplayPort,
    LVDS,
    Unknown,
}

// ---------------------------------------------------------------------------
// ConnectorStatus
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ConnectorStatus {
    Connected,
    Disconnected,
    Unknown,
}

// ---------------------------------------------------------------------------
// Connector
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug)]
pub struct Connector {
    pub id: u32,
    pub connector_type: ConnectorType,
    pub status: ConnectorStatus,
    pub preferred_mode: Option<ModeInfo>,
}

impl Connector {
    pub const fn empty() -> Self {
        Connector {
            id: 0,
            connector_type: ConnectorType::Unknown,
            status: ConnectorStatus::Unknown,
            preferred_mode: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Static connector table — up to 4 connectors
// ---------------------------------------------------------------------------

static CONNECTORS: Mutex<[Connector; 4]> = Mutex::new([
    Connector {
        id: 0,
        connector_type: ConnectorType::VGA,
        status: ConnectorStatus::Unknown,
        preferred_mode: None,
    },
    Connector {
        id: 1,
        connector_type: ConnectorType::HDMI,
        status: ConnectorStatus::Unknown,
        preferred_mode: None,
    },
    Connector {
        id: 2,
        connector_type: ConnectorType::DisplayPort,
        status: ConnectorStatus::Unknown,
        preferred_mode: None,
    },
    Connector {
        id: 3,
        connector_type: ConnectorType::Unknown,
        status: ConnectorStatus::Unknown,
        preferred_mode: None,
    },
]);

// ---------------------------------------------------------------------------
// VGA DAC sense — read port 0x3C2 bit 4
// ---------------------------------------------------------------------------

/// Returns true if a VGA monitor is detected via DAC sense bit.
///
/// Port 0x3C2 is the Miscellaneous Input register.
/// Bit 4 = "Sense" input: driven low by external monitor DAC load resistors.
/// On real hardware this is reliable; in QEMU/Bochs it may always read 0.
fn vga_dac_sense() -> bool {
    let status = inb(0x3C2);
    // Bit 4 set → monitor load present → connected
    status & 0x10 != 0
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Detect all connectors.
///
/// VGA (id=0): polled via DAC sense bit (port 0x3C2 bit 4).
///             If sense not set, mark as Unknown (not Disconnected, since
///             QEMU/Bochs may not implement sense).
/// Others: marked Unknown (no hardware present to sense them).
pub fn connector_detect_all() {
    let mut table = CONNECTORS.lock();

    // VGA connector (index 0)
    {
        let c = &mut table[0];
        if vga_dac_sense() {
            c.status = ConnectorStatus::Connected;
            // Assign preferred mode if not already set
            if c.preferred_mode.is_none() {
                c.preferred_mode = Some(MODE_1024x768_60);
            }
        } else {
            // On QEMU/Bochs the sense bit is unreliable; report Unknown
            // rather than Disconnected so the compositor still outputs video.
            c.status = ConnectorStatus::Unknown;
            if c.preferred_mode.is_none() {
                c.preferred_mode = Some(MODE_1024x768_60);
            }
        }
        serial_println!("  DRM: connector-{} (VGA) status={:?}", c.id, c.status);
    }

    // Remaining connectors — no sense hardware available in this build
    let mut i = 1usize;
    while i < 4 {
        let c = &mut table[i];
        c.status = ConnectorStatus::Unknown;
        i = i.saturating_add(1);
    }
}

/// Get the preferred mode for connector `id`.
/// Returns None if `id` is out of range or no preferred mode is set.
pub fn connector_get_preferred_mode(id: u32) -> Option<ModeInfo> {
    if id >= 4 {
        return None;
    }
    CONNECTORS.lock()[id as usize].preferred_mode
}

/// Set the active mode on connector `id`.
pub fn connector_set_mode(id: u32, mode: ModeInfo) {
    if id >= 4 {
        return;
    }
    CONNECTORS.lock()[id as usize].preferred_mode = Some(mode);
}

/// Get the current status of connector `id`.
pub fn connector_get_status(id: u32) -> ConnectorStatus {
    if id >= 4 {
        return ConnectorStatus::Unknown;
    }
    CONNECTORS.lock()[id as usize].status
}

// ---------------------------------------------------------------------------
// Module init
// ---------------------------------------------------------------------------

pub fn init() {
    connector_detect_all();
    serial_println!("  DRM/KMS: connector detection complete");
}
