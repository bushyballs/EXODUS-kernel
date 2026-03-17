/// ACPI Embedded Controller (EC) Driver — no-heap implementation
///
/// The EC is an independent microcontroller inside a laptop that manages
/// battery charging, thermal/fan control, lid detection, power-button
/// events, and other low-level hardware signals.
///
/// Communication protocol (ACPI spec §12.4):
///   - Status/command register: I/O port 0x66
///   - Data register:           I/O port 0x62
///   - IBF (Input Buffer Full): bit 1 of status — must be 0 before writing
///   - OBF (Output Buffer Full): bit 0 of status — must be 1 before reading
///
/// Rules enforced:
///   - No heap (no Vec, Box, String, alloc::*)
///   - No floats (no as f32 / as f64)
///   - No panics (no unwrap, expect, panic!)
///   - Counters: saturating_add / saturating_sub
///   - All I/O via crate::io::{inb, outb}
use crate::serial_println;

// ============================================================================
// I/O Port Constants
// ============================================================================

/// EC status / command register port
pub const EC_SC: u16 = 0x66;

/// EC data register port
pub const EC_DATA: u16 = 0x62;

// ============================================================================
// EC Status Bits
// ============================================================================

/// Input Buffer Full — set when EC has not yet read the last byte written by
/// the host.  The host must wait until this bit clears before writing.
pub const EC_IBF: u8 = 0x02;

/// Output Buffer Full — set when the EC has placed a result byte in EC_DATA.
/// The host must wait until this bit is set before reading.
pub const EC_OBF: u8 = 0x01;

// ============================================================================
// EC Commands
// ============================================================================

/// Read a byte from an EC address-space register
pub const EC_CMD_READ: u8 = 0x80;

/// Write a byte to an EC address-space register
pub const EC_CMD_WRITE: u8 = 0x81;

/// Query the EC for a pending SCI event byte
pub const EC_CMD_QUERY: u8 = 0x84;

// ============================================================================
// Wait Timeout
// ============================================================================

/// Maximum spin iterations while waiting for IBF clear or OBF set.
/// At ~1 iteration / ns this provides roughly 100 µs of patience.
pub const MAX_EC_WAIT: u32 = 100_000;

// ============================================================================
// Well-Known EC Register Map
// ============================================================================

/// Battery remaining capacity (%)
pub const EC_REG_BATTERY_CAP: u8 = 0x2C;

/// Battery full-charge capacity (same unit as CAP)
pub const EC_REG_BATTERY_FULL: u8 = 0x2E;

/// AC adapter status — bit 0 = AC connected
pub const EC_REG_AC_STATUS: u8 = 0x11;

/// Fan speed register — RPM / 100
pub const EC_REG_FAN_SPEED: u8 = 0x17;

/// CPU package temperature (degrees Celsius)
pub const EC_REG_CPU_TEMP: u8 = 0x07;

/// Lid status — bit 1 = lid is open
pub const EC_REG_LID_STATUS: u8 = 0x1D;

// ============================================================================
// Raw I/O helpers
// ============================================================================
//
// These inline wrappers delegate to crate::io, which already provides the
// correct inline-asm implementations for `inb` / `outb`.  We alias them
// locally so the EC logic is self-contained and readable.

#[inline(always)]
fn ec_inb(port: u16) -> u8 {
    crate::io::inb(port)
}

#[inline(always)]
fn ec_outb(port: u16, val: u8) {
    crate::io::outb(port, val);
}

// ============================================================================
// IBF / OBF wait loops
// ============================================================================

/// Spin until the EC input buffer is no longer full (IBF = 0).
///
/// Must be called before writing a command or data byte to the EC.
/// Returns `false` on timeout (EC unresponsive).
pub fn ec_wait_ibf() -> bool {
    let mut count: u32 = 0;
    loop {
        let status = ec_inb(EC_SC);
        if status & EC_IBF == 0 {
            return true;
        }
        count = count.saturating_add(1);
        if count >= MAX_EC_WAIT {
            return false;
        }
        core::hint::spin_loop();
    }
}

/// Spin until the EC output buffer is full (OBF = 1), meaning a result
/// byte is ready to read from EC_DATA.
///
/// Returns `false` on timeout (EC unresponsive).
pub fn ec_wait_obf() -> bool {
    let mut count: u32 = 0;
    loop {
        let status = ec_inb(EC_SC);
        if status & EC_OBF != 0 {
            return true;
        }
        count = count.saturating_add(1);
        if count >= MAX_EC_WAIT {
            return false;
        }
        core::hint::spin_loop();
    }
}

// ============================================================================
// Core EC read / write / query
// ============================================================================

/// Read a single byte from EC address-space register `reg`.
///
/// Protocol:
///   1. Wait IBF clear, write EC_CMD_READ to EC_SC
///   2. Wait IBF clear, write `reg` to EC_DATA
///   3. Wait OBF set,  read result from EC_DATA
///
/// Returns `None` on any timeout.
pub fn ec_read(reg: u8) -> Option<u8> {
    if !ec_wait_ibf() {
        return None;
    }
    ec_outb(EC_SC, EC_CMD_READ);

    if !ec_wait_ibf() {
        return None;
    }
    ec_outb(EC_DATA, reg);

    if !ec_wait_obf() {
        return None;
    }
    Some(ec_inb(EC_DATA))
}

/// Write a single byte `val` to EC address-space register `reg`.
///
/// Protocol:
///   1. Wait IBF clear, write EC_CMD_WRITE to EC_SC
///   2. Wait IBF clear, write `reg` to EC_DATA
///   3. Wait IBF clear, write `val` to EC_DATA
///
/// Returns `false` on any timeout.
pub fn ec_write(reg: u8, val: u8) -> bool {
    if !ec_wait_ibf() {
        return false;
    }
    ec_outb(EC_SC, EC_CMD_WRITE);

    if !ec_wait_ibf() {
        return false;
    }
    ec_outb(EC_DATA, reg);

    if !ec_wait_ibf() {
        return false;
    }
    ec_outb(EC_DATA, val);

    true
}

/// Query the EC for a pending SCI event byte.
///
/// Protocol:
///   1. Wait IBF clear, write EC_CMD_QUERY to EC_SC
///   2. Wait OBF set,  read event byte from EC_DATA
///
/// Returns the event byte, or `None` on timeout.
pub fn ec_query() -> Option<u8> {
    if !ec_wait_ibf() {
        return None;
    }
    ec_outb(EC_SC, EC_CMD_QUERY);

    if !ec_wait_obf() {
        return None;
    }
    Some(ec_inb(EC_DATA))
}

// ============================================================================
// High-level accessors
// ============================================================================

/// Read battery charge percentage (0–100).
///
/// Computes `(CAP * 100) / FULL` using integer arithmetic.
/// Guards against FULL == 0 (returns 0 in that case).
/// Clamps result to 0–100.
pub fn ec_get_battery_percent() -> u8 {
    let cap = match ec_read(EC_REG_BATTERY_CAP) {
        Some(v) => v as u32,
        None => return 0,
    };
    let full = match ec_read(EC_REG_BATTERY_FULL) {
        Some(v) => v as u32,
        None => return 0,
    };
    if full == 0 {
        return 0;
    }
    let pct = (cap * 100) / full;
    if pct > 100 {
        100
    } else {
        pct as u8
    }
}

/// Return `true` when an AC adapter is detected (bit 0 of AC_STATUS).
pub fn ec_ac_connected() -> bool {
    match ec_read(EC_REG_AC_STATUS) {
        Some(v) => v & 0x01 != 0,
        None => false,
    }
}

/// Return the fan speed in RPM (register value × 100).
pub fn ec_get_fan_rpm() -> u32 {
    match ec_read(EC_REG_FAN_SPEED) {
        Some(v) => (v as u32).saturating_mul(100),
        None => 0,
    }
}

/// Return CPU temperature in degrees Celsius (direct register read).
pub fn ec_get_cpu_temp_c() -> u8 {
    match ec_read(EC_REG_CPU_TEMP) {
        Some(v) => v,
        None => 0,
    }
}

/// Return `true` when the lid is open (bit 1 of LID_STATUS).
pub fn ec_lid_open() -> bool {
    match ec_read(EC_REG_LID_STATUS) {
        Some(v) => v & 0x02 != 0,
        None => false,
    }
}

// ============================================================================
// SCI Event Dispatch
// ============================================================================

/// Dispatch an EC SCI (System Control Interrupt) event byte.
///
/// Common event bytes (platform-dependent; these follow a typical Lenovo/HP
/// mapping used in simulation):
///   0x00        — no event (query returned nothing)
///   0x01..0x0F  — function-key hotkeys (Fn+Fx)
///   0x10        — AC adapter plug event
///   0x11        — AC adapter unplug event
///   0x20        — battery status change
///   0x21        — battery low warning
///   0x30        — thermal trip point crossed
///   0x31        — fan speed change
///   0x40        — lid closed
///   0x41        — lid opened
///   0x50        — power button pressed
///   others      — unhandled / platform-specific
pub fn ec_handle_query_event(event_byte: u8) {
    match event_byte {
        0x00 => {
            // No event — normal; ignore
        }
        0x01..=0x0F => {
            serial_println!("[acpi_ec] hotkey event: Fn+F{}", event_byte);
        }
        0x10 => {
            serial_println!("[acpi_ec] AC adapter connected");
        }
        0x11 => {
            serial_println!("[acpi_ec] AC adapter disconnected");
        }
        0x20 => {
            serial_println!("[acpi_ec] battery status change");
        }
        0x21 => {
            serial_println!("[acpi_ec] battery low warning");
        }
        0x30 => {
            serial_println!("[acpi_ec] thermal trip point crossed");
        }
        0x31 => {
            serial_println!("[acpi_ec] fan speed change");
        }
        0x40 => {
            serial_println!("[acpi_ec] lid closed");
        }
        0x41 => {
            serial_println!("[acpi_ec] lid opened");
        }
        0x50 => {
            serial_println!("[acpi_ec] power button pressed");
        }
        other => {
            serial_println!("[acpi_ec] unhandled SCI event: 0x{:02x}", other);
        }
    }
}

// ============================================================================
// Module init
// ============================================================================

/// Initialise the ACPI EC driver.
///
/// In simulation mode we do not attempt real EC I/O (there is no physical EC
/// in QEMU) and instead just print a banner confirming the module loaded.
pub fn init() {
    serial_println!("[acpi_ec] EC driver initialized (simulated)");
    super::register("acpi-ec", super::DeviceType::Other);
}
