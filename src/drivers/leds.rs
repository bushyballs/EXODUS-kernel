use crate::sync::Mutex;
/// LED framework for Genesis — no-heap, fixed-size static arrays
///
/// Manages up to 32 LEDs with configurable triggers:
///   - Heartbeat: double-pulse pattern indicating kernel liveness
///   - DiskActivity / NetworkTx / NetworkRx: edge-triggered, auto-off
///   - Timer(N Hz): symmetrical blink at N Hz (1-100)
///   - OneShot: blink once for a specified duration then off
///   - Panic: rapid blink (125ms toggle) for crash indication
///   - CpuLoad: brightness proportional to CPU utilisation
///   - Virtual LEDs: software-only (power, status indicators)
///   - GPIO-backed LEDs: toggled via gpio_set_value()
///   - MMIO-backed LEDs: single-bit write to a memory-mapped register
///
/// All rules strictly observed:
///   - No heap: no Vec, Box, String, alloc::*
///   - No panics: no unwrap(), expect(), panic!()
///   - No float casts: no as f64, as f32
///   - Saturating arithmetic for counters
///   - Wrapping arithmetic for sequence numbers
///   - MMIO via read_volatile / write_volatile only
use crate::{serial_print, serial_println};
use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of LED devices supported
const MAX_LEDS: usize = 32;

/// Auto-off timeout for activity-triggered LEDs (ms)
const ACTIVITY_AUTO_OFF_MS: u64 = 50;

/// Panic blink half-period (ms)
const PANIC_BLINK_MS: u64 = 125;

/// Heartbeat pattern timing (ms): on1, off1, on2, off2
/// fast-slow-fast cardiac pattern: 125ms on, 125ms off, 125ms on, 625ms off
const HEARTBEAT_ON1_MS: u64 = 125;
const HEARTBEAT_OFF1_MS: u64 = 125;
const HEARTBEAT_ON2_MS: u64 = 125;
const HEARTBEAT_OFF2_MS: u64 = 625;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Trigger mode for an LED
#[derive(Copy, Clone, PartialEq)]
pub enum LedTrigger {
    /// Manual brightness control only
    None,
    /// Heartbeat blink pattern (indicates kernel alive)
    Heartbeat,
    /// Blink on disk I/O events
    DiskActivity,
    /// Blink on network transmit events
    NetworkTx,
    /// Blink on network receive events
    NetworkRx,
    /// Blink at N Hz symmetrical (1-100); stored as half-period ms
    Timer(u32),
    /// Blink once for blink_delay_on ms then turn off
    OneShot,
    /// Rapid blink indicating kernel panic
    Panic,
    /// Brightness proportional to current CPU load (0-255)
    CpuLoad,
}

/// Hardware control method for an LED
#[derive(Copy, Clone, PartialEq)]
pub enum LedCtrlType {
    /// No hardware — software-only tracking
    None,
    /// Toggle via GPIO subsystem (gpio_set_value)
    Gpio,
    /// Toggle a single bit in an MMIO register
    Mmio,
    /// Purely virtual; no hardware action required
    Virtual,
}

/// A registered LED device
#[derive(Copy, Clone)]
pub struct LedDevice {
    /// Human-readable name (null-padded ASCII)
    pub name: [u8; 16],
    /// Current brightness (0-255)
    pub brightness: u8,
    /// Maximum brightness allowed (usually 255)
    pub max_brightness: u8,
    /// Active trigger mode
    pub trigger: LedTrigger,
    /// On-time in ms (used by Timer and OneShot triggers)
    pub blink_delay_on: u32,
    /// Off-time in ms (used by Timer trigger)
    pub blink_delay_off: u32,
    /// True when a OneShot blink is pending (turn off after blink_delay_on ms)
    pub oneshot_pending: bool,
    /// True when this slot is occupied
    pub active: bool,
    /// Hardware control type
    pub ctrl_type: LedCtrlType,
    /// GPIO number for Gpio-controlled LEDs
    pub gpio_num: u32,
    /// MMIO register base address for Mmio-controlled LEDs
    pub mmio_addr: u64,
    /// Bit position within the MMIO register for this LED
    pub mmio_bit: u8,
    // Internal tick state — not exposed publicly but stored here to avoid
    // a separate parallel array
    /// Timestamp of the last blink toggle (ms)
    last_toggle_ms: u64,
    /// Current blink state: true = LED is on
    blink_on: bool,
    /// Heartbeat sub-phase (0-3)
    heartbeat_phase: u8,
    /// Timestamp when a OneShot was armed (ms)
    oneshot_start_ms: u64,
}

impl LedDevice {
    /// Return a zeroed, inactive LED slot
    pub const fn empty() -> Self {
        LedDevice {
            name: [0u8; 16],
            brightness: 0,
            max_brightness: 255,
            trigger: LedTrigger::None,
            blink_delay_on: 500,
            blink_delay_off: 500,
            oneshot_pending: false,
            active: false,
            ctrl_type: LedCtrlType::None,
            gpio_num: 0,
            mmio_addr: 0,
            mmio_bit: 0,
            last_toggle_ms: 0,
            blink_on: false,
            heartbeat_phase: 0,
            oneshot_start_ms: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static LED_DEVICES: Mutex<[LedDevice; MAX_LEDS]> = Mutex::new([LedDevice::empty(); MAX_LEDS]);

/// Monotonic millisecond counter updated by led_tick()
pub static LED_TICK_MS: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Copy up to 16 bytes from `src` into `dst`, null-padding the remainder
fn copy_name(dst: &mut [u8; 16], src: &[u8]) {
    let len = if src.len() < 16 { src.len() } else { 16 };
    let mut i = 0usize;
    while i < len {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
    while i < 16 {
        dst[i] = 0;
        i = i.saturating_add(1);
    }
}

/// Apply brightness to hardware based on ctrl_type.
/// GPIO: treat any brightness > 0 as logic HIGH.
/// MMIO: set or clear mmio_bit in the 32-bit register at mmio_addr.
fn hw_apply(dev: &LedDevice, brightness: u8) {
    match dev.ctrl_type {
        LedCtrlType::Gpio => {
            crate::drivers::gpio::gpio_set_value(dev.gpio_num, brightness > 0);
        }
        LedCtrlType::Mmio => {
            if dev.mmio_addr != 0 {
                // Safety: bare-metal kernel; caller guarantees valid MMIO addr
                let cur = unsafe { core::ptr::read_volatile(dev.mmio_addr as *const u32) };
                let new = if brightness > 0 {
                    cur | (1u32 << (dev.mmio_bit as u32))
                } else {
                    cur & !(1u32 << (dev.mmio_bit as u32))
                };
                unsafe {
                    core::ptr::write_volatile(dev.mmio_addr as *mut u32, new);
                }
            }
        }
        LedCtrlType::None | LedCtrlType::Virtual => {}
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Register a new LED device.
///
/// `name`         — ASCII name (up to 16 bytes).
/// `max_brightness` — maximum allowed brightness (usually 255).
/// `ctrl`         — hardware control type.
/// `gpio_or_mmio` — GPIO number (Gpio) or MMIO address (Mmio); 0 otherwise.
///
/// Returns the index in the LED table on success, or `None` if the table is
/// full or a device with the same name already exists.
pub fn led_register(
    name: &[u8],
    max_brightness: u8,
    ctrl: LedCtrlType,
    gpio_or_mmio: u64,
) -> Option<usize> {
    let mut devs = LED_DEVICES.lock();

    // Reject duplicates
    let mut slot: Option<usize> = None;
    for (i, d) in devs.iter().enumerate() {
        if !d.active {
            if slot.is_none() {
                slot = Some(i);
            }
            continue;
        }
        // Compare name bytes
        let cmp_len = if name.len() < 16 { name.len() } else { 16 };
        let mut same = true;
        for j in 0..cmp_len {
            if d.name[j] != name[j] {
                same = false;
                break;
            }
        }
        // Also check that d.name ends at cmp_len (no longer name stored)
        if same && d.name[cmp_len] == 0 {
            return None; // duplicate name
        }
    }

    let idx = slot?; // return None if table is full

    let mut dev = LedDevice::empty();
    copy_name(&mut dev.name, name);
    dev.max_brightness = max_brightness;
    dev.brightness = 0;
    dev.ctrl_type = ctrl;
    dev.active = true;
    match ctrl {
        LedCtrlType::Gpio => {
            dev.gpio_num = gpio_or_mmio as u32;
        }
        LedCtrlType::Mmio => {
            dev.mmio_addr = gpio_or_mmio;
        }
        _ => {}
    }
    devs[idx] = dev;
    Some(idx)
}

/// Set the brightness of LED `idx`.
///
/// Clamps to `max_brightness`. Applies to hardware immediately.
pub fn led_set_brightness(idx: usize, brightness: u8) {
    let mut devs = LED_DEVICES.lock();
    if idx >= MAX_LEDS || !devs[idx].active {
        return;
    }
    let clamped = if brightness > devs[idx].max_brightness {
        devs[idx].max_brightness
    } else {
        brightness
    };
    devs[idx].brightness = clamped;
    let dev = devs[idx];
    drop(devs);
    hw_apply(&dev, clamped);
}

/// Get the current brightness of LED `idx`.
///
/// Returns `None` if `idx` is out of range or the slot is inactive.
pub fn led_get_brightness(idx: usize) -> Option<u8> {
    let devs = LED_DEVICES.lock();
    if idx >= MAX_LEDS || !devs[idx].active {
        return None;
    }
    Some(devs[idx].brightness)
}

/// Set the trigger mode for LED `idx`.
///
/// Resets internal blink state so the new trigger starts cleanly.
pub fn led_set_trigger(idx: usize, trigger: LedTrigger) {
    let now = LED_TICK_MS.load(Ordering::Relaxed);
    let mut devs = LED_DEVICES.lock();
    if idx >= MAX_LEDS || !devs[idx].active {
        return;
    }
    devs[idx].trigger = trigger;
    devs[idx].last_toggle_ms = now;
    devs[idx].blink_on = false;
    devs[idx].heartbeat_phase = 0;
    devs[idx].oneshot_pending = false;
}

/// Arm a one-shot blink: turn the LED on immediately for `ms_on` milliseconds,
/// then automatically turn it off.
pub fn led_blink_oneshot(idx: usize, ms_on: u32) {
    let now = LED_TICK_MS.load(Ordering::Relaxed);
    let mut devs = LED_DEVICES.lock();
    if idx >= MAX_LEDS || !devs[idx].active {
        return;
    }
    devs[idx].blink_delay_on = ms_on;
    devs[idx].oneshot_pending = true;
    devs[idx].oneshot_start_ms = now;
    let brightness = devs[idx].max_brightness;
    devs[idx].brightness = brightness;
    let dev = devs[idx];
    drop(devs);
    hw_apply(&dev, brightness);
}

/// Signal an event to all LEDs whose trigger matches `trigger`.
///
/// Typically used for DiskActivity, NetworkTx, NetworkRx — turns the matching
/// LED on; `led_tick()` will auto-off it after `ACTIVITY_AUTO_OFF_MS`.
pub fn led_trigger_event(trigger: LedTrigger) {
    let now = LED_TICK_MS.load(Ordering::Relaxed);
    let mut devs = LED_DEVICES.lock();
    for i in 0..MAX_LEDS {
        if !devs[i].active {
            continue;
        }
        let matches = match (devs[i].trigger, trigger) {
            (LedTrigger::DiskActivity, LedTrigger::DiskActivity) => true,
            (LedTrigger::NetworkTx, LedTrigger::NetworkTx) => true,
            (LedTrigger::NetworkRx, LedTrigger::NetworkRx) => true,
            _ => false,
        };
        if matches {
            devs[i].blink_on = true;
            devs[i].last_toggle_ms = now;
            let brightness = devs[i].max_brightness;
            devs[i].brightness = brightness;
            let dev = devs[i];
            drop(devs);
            hw_apply(&dev, brightness);
            devs = LED_DEVICES.lock();
        }
    }
}

/// Find the index of the first LED whose name equals `name`.
///
/// Returns `None` if no match is found.
pub fn led_find(name: &[u8]) -> Option<usize> {
    let devs = LED_DEVICES.lock();
    let cmp_len = if name.len() < 16 { name.len() } else { 16 };
    for (i, d) in devs.iter().enumerate() {
        if !d.active {
            continue;
        }
        let mut same = true;
        for j in 0..cmp_len {
            if d.name[j] != name[j] {
                same = false;
                break;
            }
        }
        if same && d.name[cmp_len] == 0 {
            return Some(i);
        }
    }
    None
}

/// Periodic tick — call with the current system uptime in milliseconds.
///
/// Processes all active triggers:
/// - Heartbeat: 4-phase cardiac pattern
/// - Timer: symmetrical on/off based on blink_delay_on/off
/// - OneShot: turn off after blink_delay_on ms
/// - DiskActivity / NetworkTx / NetworkRx: auto-off after ACTIVITY_AUTO_OFF_MS
/// - Panic: toggle every PANIC_BLINK_MS
/// - CpuLoad: set brightness proportional to CPU utilisation (from PMU)
pub fn led_tick(current_ms: u64) {
    LED_TICK_MS.store(current_ms, Ordering::Relaxed);

    // Collect CPU load once outside the per-LED loop to avoid repeated PMU reads.
    // Use fixed-point ratio (cycles elapsed vs max expected) from PMU snapshot.
    // Approach: read fixed counter 1 (cpu_clk_unhalted) — a larger delta means
    // higher load.  We use a simple two-snapshot difference stored in a local.
    // For CpuLoad LEDs: set brightness = (cycles_delta >> shift) clamped to 255.
    // Since we have no persistent prev_cycles in this function (no static here
    // to avoid a second Mutex), we read both fixed PMCs and derive a rough
    // busyness indicator: fixed1 (core cycles) vs fixed2 (ref cycles).
    // busyness = min(255, (fixed1 / fixed2) scaled to 0-255).
    // If both are 0 (PMU not running), default to 128.
    let cpu_brightness: u8 = {
        let snap = crate::kernel::pmu::pmu_snapshot();
        if snap.fixed2 == 0 {
            128u8
        } else {
            // ratio = fixed1 / fixed2 in [0, ~1.0 for unhalted], scale to 0-255
            // Use 256-step approximation: (fixed1 * 255) / fixed2, clamped
            let ratio = snap.fixed1 / (snap.fixed2 / 256).saturating_add(1);
            if ratio >= 255 {
                255u8
            } else {
                ratio as u8
            }
        }
    };

    let mut devs = LED_DEVICES.lock();

    for i in 0..MAX_LEDS {
        if !devs[i].active {
            continue;
        }

        match devs[i].trigger {
            LedTrigger::None => {
                // Manual control only — no automatic toggling
            }

            LedTrigger::Heartbeat => {
                let elapsed = current_ms.saturating_sub(devs[i].last_toggle_ms);
                let threshold: u64 = match devs[i].heartbeat_phase {
                    0 => HEARTBEAT_ON1_MS,
                    1 => HEARTBEAT_OFF1_MS,
                    2 => HEARTBEAT_ON2_MS,
                    _ => HEARTBEAT_OFF2_MS,
                };
                if elapsed >= threshold {
                    devs[i].last_toggle_ms = current_ms;
                    devs[i].heartbeat_phase = devs[i].heartbeat_phase.saturating_add(1) % 4;
                    // Phases 0 and 2 are "on" phases (first beat, second beat)
                    let is_on = devs[i].heartbeat_phase == 0 || devs[i].heartbeat_phase == 2;
                    let brightness = if is_on { devs[i].max_brightness } else { 0 };
                    devs[i].brightness = brightness;
                    let dev = devs[i];
                    drop(devs);
                    hw_apply(&dev, brightness);
                    devs = LED_DEVICES.lock();
                }
            }

            LedTrigger::Timer(hz) => {
                // hz is in [1, 100]; half-period = 1000 / (hz * 2) ms
                // To avoid division by zero, clamp hz to at least 1
                let hz_safe = if hz == 0 {
                    1
                } else if hz > 100 {
                    100
                } else {
                    hz
                };
                let half_period = 500u32 / hz_safe; // ms
                let threshold = half_period as u64;
                let elapsed = current_ms.saturating_sub(devs[i].last_toggle_ms);
                if elapsed >= threshold {
                    devs[i].last_toggle_ms = current_ms;
                    devs[i].blink_on = !devs[i].blink_on;
                    let brightness = if devs[i].blink_on {
                        devs[i].max_brightness
                    } else {
                        0
                    };
                    devs[i].brightness = brightness;
                    let dev = devs[i];
                    drop(devs);
                    hw_apply(&dev, brightness);
                    devs = LED_DEVICES.lock();
                }
            }

            LedTrigger::OneShot => {
                if devs[i].oneshot_pending {
                    let elapsed = current_ms.saturating_sub(devs[i].oneshot_start_ms);
                    if elapsed >= devs[i].blink_delay_on as u64 {
                        devs[i].oneshot_pending = false;
                        devs[i].brightness = 0;
                        let dev = devs[i];
                        drop(devs);
                        hw_apply(&dev, 0);
                        devs = LED_DEVICES.lock();
                    }
                }
            }

            LedTrigger::DiskActivity | LedTrigger::NetworkTx | LedTrigger::NetworkRx => {
                // Auto-off after ACTIVITY_AUTO_OFF_MS if LED is currently on
                if devs[i].blink_on {
                    let elapsed = current_ms.saturating_sub(devs[i].last_toggle_ms);
                    if elapsed >= ACTIVITY_AUTO_OFF_MS {
                        devs[i].blink_on = false;
                        devs[i].brightness = 0;
                        let dev = devs[i];
                        drop(devs);
                        hw_apply(&dev, 0);
                        devs = LED_DEVICES.lock();
                    }
                }
            }

            LedTrigger::Panic => {
                let elapsed = current_ms.saturating_sub(devs[i].last_toggle_ms);
                if elapsed >= PANIC_BLINK_MS {
                    devs[i].last_toggle_ms = current_ms;
                    devs[i].blink_on = !devs[i].blink_on;
                    let brightness = if devs[i].blink_on {
                        devs[i].max_brightness
                    } else {
                        0
                    };
                    devs[i].brightness = brightness;
                    let dev = devs[i];
                    drop(devs);
                    hw_apply(&dev, brightness);
                    devs = LED_DEVICES.lock();
                }
            }

            LedTrigger::CpuLoad => {
                // Update brightness proportional to CPU load every 250ms
                let elapsed = current_ms.saturating_sub(devs[i].last_toggle_ms);
                if elapsed >= 250 {
                    devs[i].last_toggle_ms = current_ms;
                    // Scale cpu_brightness to max_brightness range
                    let scaled = ((cpu_brightness as u32)
                        .saturating_mul(devs[i].max_brightness as u32)
                        / 255) as u8;
                    devs[i].brightness = scaled;
                    let dev = devs[i];
                    drop(devs);
                    hw_apply(&dev, scaled);
                    devs = LED_DEVICES.lock();
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the LED framework and register built-in virtual LEDs.
///
/// Built-in LEDs:
///   "power"     — Virtual, always on (steady 255 brightness)
///   "disk"      — Virtual, DiskActivity trigger
///   "net"       — Virtual, NetworkTx trigger
///   "heartbeat" — Virtual, Heartbeat trigger (kernel alive indicator)
pub fn init() {
    // Register power LED (always on)
    if let Some(idx) = led_register(b"power", 255, LedCtrlType::Virtual, 0) {
        led_set_brightness(idx, 255);
        led_set_trigger(idx, LedTrigger::None);
        serial_println!("  LEDs: 'power' registered (idx {})", idx);
    } else {
        serial_println!("  LEDs: 'power' registration failed");
    }

    // Register disk activity LED
    if let Some(idx) = led_register(b"disk", 255, LedCtrlType::Virtual, 0) {
        led_set_trigger(idx, LedTrigger::DiskActivity);
        serial_println!("  LEDs: 'disk' registered (idx {})", idx);
    } else {
        serial_println!("  LEDs: 'disk' registration failed");
    }

    // Register network LED
    if let Some(idx) = led_register(b"net", 255, LedCtrlType::Virtual, 0) {
        led_set_trigger(idx, LedTrigger::NetworkTx);
        serial_println!("  LEDs: 'net' registered (idx {})", idx);
    } else {
        serial_println!("  LEDs: 'net' registration failed");
    }

    // Register heartbeat LED
    if let Some(idx) = led_register(b"heartbeat", 255, LedCtrlType::Virtual, 0) {
        led_set_trigger(idx, LedTrigger::Heartbeat);
        serial_println!("  LEDs: 'heartbeat' registered (idx {})", idx);
    } else {
        serial_println!("  LEDs: 'heartbeat' registration failed");
    }

    super::register("leds", super::DeviceType::Other);
    serial_println!("  LEDs: framework initialized (max {} devices)", MAX_LEDS);
}
