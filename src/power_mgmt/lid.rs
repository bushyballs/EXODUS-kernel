/// Lid switch detection and display backlight control.
///
/// Part of the AIOS power_mgmt subsystem.
///
/// Lid state is read from EC RAM (common offset 0x3B, bit 1 = open).
/// Backlight level is read/written via EC RAM at offset 0x40 (0-100).
///
/// All operations are side-effect-free at the function level; the only
/// shared state is the two atomics below. No heap, no floats, no panics.
use crate::io::{inb, outb};
use crate::serial_println;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

// ---------------------------------------------------------------------------
// EC constants (re-declared locally to keep the module self-contained)
// ---------------------------------------------------------------------------

const EC_DATA: u16 = 0x62;
const EC_CMD: u16 = 0x66;
const EC_CMD_READ: u8 = 0x80;
const EC_CMD_WRITE: u8 = 0x81;
const EC_STATUS_IBF: u8 = 0x02;
const EC_STATUS_OBF: u8 = 0x01;
const EC_TIMEOUT: u32 = 100_000;

// ---------------------------------------------------------------------------
// EC RAM offsets
// ---------------------------------------------------------------------------

/// Lid switch byte: bit 1 = lid open, bit 0 = lid closed.
/// Offset 0x3B is a common convention; some OEMs use bit 0 only.
const EC_LID_STATUS: u8 = 0x3B;

/// Backlight level 0-100 (percent).
pub const EC_BACKLIGHT: u8 = 0x40;

/// Minimum "dim" level applied after inactivity timeout.
const BACKLIGHT_DIM_LEVEL: u8 = 10;

// ---------------------------------------------------------------------------
// Shared atomic state
// ---------------------------------------------------------------------------

/// Last known lid state (true = open).
static LID_OPEN: AtomicBool = AtomicBool::new(true);

/// Saved backlight level, restored when lid reopens.
static SAVED_BACKLIGHT: AtomicU8 = AtomicU8::new(80);

// ---------------------------------------------------------------------------
// EC helpers (private — mirror of battery.rs to keep modules independent)
// ---------------------------------------------------------------------------

#[inline]
fn ec_wait_ibf() {
    let mut n = EC_TIMEOUT;
    while n > 0 {
        if inb(EC_CMD) & EC_STATUS_IBF == 0 {
            return;
        }
        core::hint::spin_loop();
        n = n.saturating_sub(1);
    }
    serial_println!("lid: ec IBF timeout");
}

#[inline]
fn ec_wait_obf() {
    let mut n = EC_TIMEOUT;
    while n > 0 {
        if inb(EC_CMD) & EC_STATUS_OBF != 0 {
            return;
        }
        core::hint::spin_loop();
        n = n.saturating_sub(1);
    }
    serial_println!("lid: ec OBF timeout");
}

fn ec_read_local(offset: u8) -> u8 {
    ec_wait_ibf();
    outb(EC_CMD, EC_CMD_READ);
    ec_wait_ibf();
    outb(EC_DATA, offset);
    ec_wait_obf();
    inb(EC_DATA)
}

fn ec_write_local(offset: u8, val: u8) {
    ec_wait_ibf();
    outb(EC_CMD, EC_CMD_WRITE);
    ec_wait_ibf();
    outb(EC_DATA, offset);
    ec_wait_ibf();
    outb(EC_DATA, val);
}

// ---------------------------------------------------------------------------
// Lid switch
// ---------------------------------------------------------------------------

/// Return true if the lid is currently open.
///
/// Reads EC RAM register 0x3B.  Bit 1 set = open on most ThinkPad/Dell/HP
/// firmware; falls back to the last-known atomic value if the EC byte is
/// ambiguous (both bits clear or both set — hardware glitch guard).
pub fn lid_is_open() -> bool {
    let byte = ec_read_local(EC_LID_STATUS);
    match byte & 0x03 {
        0x02 => {
            LID_OPEN.store(true, Ordering::Relaxed);
            true
        }
        0x01 => {
            LID_OPEN.store(false, Ordering::Relaxed);
            false
        }
        _ => {
            // Ambiguous reading — return cached value
            LID_OPEN.load(Ordering::Relaxed)
        }
    }
}

// ---------------------------------------------------------------------------
// Backlight control
// ---------------------------------------------------------------------------

/// Read current backlight level from EC (0-100).
pub fn backlight_get() -> u8 {
    let raw = ec_read_local(EC_BACKLIGHT);
    if raw > 100 {
        100
    } else {
        raw
    }
}

/// Set backlight to `level` (0-100). Values above 100 are clamped.
pub fn backlight_set(level: u8) {
    let clamped = if level > 100 { 100 } else { level };
    ec_write_local(EC_BACKLIGHT, clamped);
}

/// Reduce backlight to the dim level (10%) after an inactivity timeout.
pub fn backlight_dim() {
    backlight_set(BACKLIGHT_DIM_LEVEL);
}

/// Turn the backlight fully off (0%).
pub fn backlight_off() {
    backlight_set(0);
}

/// Restore backlight to the level saved before the last lid-close or dim.
pub fn backlight_restore() {
    let level = SAVED_BACKLIGHT.load(Ordering::Relaxed);
    backlight_set(level);
}

// ---------------------------------------------------------------------------
// Lid event handler
// ---------------------------------------------------------------------------

/// Called by the ACPI SCI or GPIO interrupt handler when the lid state changes.
///
/// On open: restore saved backlight, update cached state, log.
/// On close: save current level, blank display, update cached state, log.
pub fn lid_event(now_open: bool) {
    LID_OPEN.store(now_open, Ordering::Relaxed);

    if now_open {
        serial_println!("lid: opened — restoring backlight");
        backlight_restore();
    } else {
        // Save the current brightness before blanking
        let current = backlight_get();
        SAVED_BACKLIGHT.store(current, Ordering::Relaxed);
        serial_println!(
            "lid: closed — saving backlight={}, blanking display",
            current
        );
        backlight_off();
        // Notify suspend subsystem: lid-close is a common S3 trigger.
        // The suspend module decides whether to actually enter S3.
        super::suspend::on_lid_close();
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialise lid/backlight driver: read initial state, log.
pub fn init() {
    // Read and cache the current lid state
    let open = lid_is_open();
    let bl = backlight_get();
    // Pre-populate saved level so restore() works even before first lid-close
    SAVED_BACKLIGHT.store(bl, Ordering::Relaxed);
    serial_println!(
        "  lid: lid={} backlight={}%",
        if open { "open" } else { "closed" },
        bl
    );
}
