use crate::sync::Mutex;
/// LED controller driver for Genesis
///
/// Manages system LEDs (power, disk, network activity, keyboard backlight,
/// notification RGB LEDs) with brightness levels, blink patterns, RGB color
/// mixing, and trigger modes (heartbeat, disk activity, network activity,
/// manual, and timer-based).
///
/// LEDs are accessed via MMIO-mapped GPIO/PWM registers or EC commands,
/// depending on the platform. This driver abstracts both backends.
///
/// Inspired by: Linux led-class, led-triggers, leds-gpio. All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum LEDs
const MAX_LEDS: usize = 16;

// EC-backed LED control registers
const EC_DATA_PORT: u16 = 0x62;
const EC_CMD_PORT: u16 = 0x66;
const EC_READ_CMD: u8 = 0x80;
const EC_WRITE_CMD: u8 = 0x81;

// EC LED register base
const EC_LED_BASE: u8 = 0x30;
const EC_LED_STRIDE: u8 = 0x04;
const EC_LED_BRIGHT: u8 = 0x00; // Brightness (0-255)
const EC_LED_RED: u8 = 0x01; // Red channel (for RGB LEDs)
const EC_LED_GREEN: u8 = 0x02; // Green channel
const EC_LED_BLUE: u8 = 0x03; // Blue channel
const EC_LED_COUNT_REG: u8 = 0x2F; // Number of LEDs

// Heartbeat timing (in ms)
const HEARTBEAT_ON_1: u64 = 70;
const HEARTBEAT_OFF_1: u64 = 250;
const HEARTBEAT_ON_2: u64 = 70;
const HEARTBEAT_OFF_2: u64 = 1400;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// LED trigger mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// Manual control only
    Manual,
    /// Heartbeat pattern (double blink)
    Heartbeat,
    /// Blink on disk I/O activity
    DiskActivity,
    /// Blink on network activity
    NetworkActivity,
    /// Custom timer: on_ms / off_ms blink cycle
    Timer { on_ms: u32, off_ms: u32 },
    /// CPU load indicator (brightness proportional to load)
    CpuLoad,
}

/// RGB color value
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl RgbColor {
    pub const fn white() -> Self {
        RgbColor {
            r: 255,
            g: 255,
            b: 255,
        }
    }
    pub const fn off() -> Self {
        RgbColor { r: 0, g: 0, b: 0 }
    }
    pub const fn red() -> Self {
        RgbColor { r: 255, g: 0, b: 0 }
    }
    pub const fn green() -> Self {
        RgbColor { r: 0, g: 255, b: 0 }
    }
    pub const fn blue() -> Self {
        RgbColor { r: 0, g: 0, b: 255 }
    }
}

/// LED capability flags
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedType {
    /// Single-color (brightness only)
    Mono,
    /// Full RGB color
    Rgb,
}

/// LED error codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedError {
    NotInitialized,
    InvalidIndex,
    NotRgb,
}

/// Blink state tracking
#[derive(Debug, Clone, Copy)]
struct BlinkState {
    phase: u8,           // Current phase in pattern
    last_toggle_ms: u64, // Timestamp of last toggle
    on: bool,            // Currently on or off
}

/// Per-LED state
struct LedInner {
    name: String,
    led_type: LedType,
    brightness: u8,
    max_brightness: u8,
    color: RgbColor,
    trigger: Trigger,
    blink: BlinkState,
    ec_base: u8,
    /// Whether the hardware LED is currently lit
    hw_on: bool,
}

/// Top-level driver state
struct LedDriver {
    leds: Vec<LedInner>,
    tick_ms: u64,
}

// ---------------------------------------------------------------------------
// EC helpers
// ---------------------------------------------------------------------------

fn ec_wait_write() {
    for _ in 0..10_000 {
        if crate::io::inb(EC_CMD_PORT) & 0x02 == 0 {
            return;
        }
        crate::io::io_wait();
    }
}

fn ec_wait_read() {
    for _ in 0..10_000 {
        if crate::io::inb(EC_CMD_PORT) & 0x01 != 0 {
            return;
        }
        crate::io::io_wait();
    }
}

fn ec_read(reg: u8) -> u8 {
    ec_wait_write();
    crate::io::outb(EC_CMD_PORT, EC_READ_CMD);
    ec_wait_write();
    crate::io::outb(EC_DATA_PORT, reg);
    ec_wait_read();
    crate::io::inb(EC_DATA_PORT)
}

fn ec_write(reg: u8, val: u8) {
    ec_wait_write();
    crate::io::outb(EC_CMD_PORT, EC_WRITE_CMD);
    ec_wait_write();
    crate::io::outb(EC_DATA_PORT, reg);
    ec_wait_write();
    crate::io::outb(EC_DATA_PORT, val);
}

// ---------------------------------------------------------------------------
// Internal implementation
// ---------------------------------------------------------------------------

impl LedInner {
    /// Write current brightness/color to hardware
    fn commit(&self) {
        ec_write(self.ec_base.saturating_add(EC_LED_BRIGHT), self.brightness);
        if self.led_type == LedType::Rgb {
            ec_write(self.ec_base.saturating_add(EC_LED_RED), self.color.r);
            ec_write(self.ec_base.saturating_add(EC_LED_GREEN), self.color.g);
            ec_write(self.ec_base.saturating_add(EC_LED_BLUE), self.color.b);
        }
    }

    /// Turn LED on at current brightness/color
    fn turn_on(&mut self) {
        self.hw_on = true;
        self.commit();
    }

    /// Turn LED off (brightness 0, keep settings)
    fn turn_off(&mut self) {
        self.hw_on = false;
        ec_write(self.ec_base.saturating_add(EC_LED_BRIGHT), 0);
        if self.led_type == LedType::Rgb {
            ec_write(self.ec_base.saturating_add(EC_LED_RED), 0);
            ec_write(self.ec_base.saturating_add(EC_LED_GREEN), 0);
            ec_write(self.ec_base.saturating_add(EC_LED_BLUE), 0);
        }
    }
}

impl LedDriver {
    /// Process triggers and blink patterns for all LEDs.
    /// Call this periodically (every ~10-50 ms).
    fn tick(&mut self, now_ms: u64) {
        self.tick_ms = now_ms;

        for led in &mut self.leds {
            match led.trigger {
                Trigger::Manual => {
                    // No automatic control
                }

                Trigger::Heartbeat => {
                    let elapsed = now_ms.saturating_sub(led.blink.last_toggle_ms);
                    let threshold = match led.blink.phase {
                        0 => HEARTBEAT_ON_1,
                        1 => HEARTBEAT_OFF_1,
                        2 => HEARTBEAT_ON_2,
                        _ => HEARTBEAT_OFF_2,
                    };
                    if elapsed >= threshold {
                        led.blink.last_toggle_ms = now_ms;
                        led.blink.phase = led.blink.phase.saturating_add(1) % 4;
                        if led.blink.phase == 0 || led.blink.phase == 2 {
                            led.turn_on();
                        } else {
                            led.turn_off();
                        }
                    }
                }

                Trigger::Timer { on_ms, off_ms } => {
                    let elapsed = now_ms.saturating_sub(led.blink.last_toggle_ms);
                    let threshold = if led.blink.on {
                        on_ms as u64
                    } else {
                        off_ms as u64
                    };
                    if elapsed >= threshold {
                        led.blink.last_toggle_ms = now_ms;
                        led.blink.on = !led.blink.on;
                        if led.blink.on {
                            led.turn_on();
                        } else {
                            led.turn_off();
                        }
                    }
                }

                Trigger::DiskActivity | Trigger::NetworkActivity => {
                    // These are edge-triggered from outside via activity_pulse()
                    // Auto-off after 50ms
                    if led.hw_on && now_ms.saturating_sub(led.blink.last_toggle_ms) > 50 {
                        led.turn_off();
                    }
                }

                Trigger::CpuLoad => {
                    // Brightness proportional to CPU load would be set
                    // externally; we just maintain the current brightness.
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static LEDS: Mutex<Option<LedDriver>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Set LED brightness (0-255).
pub fn set_brightness(idx: usize, level: u8) -> Result<(), LedError> {
    let mut guard = LEDS.lock();
    let drv = guard.as_mut().ok_or(LedError::NotInitialized)?;
    let led = drv.leds.get_mut(idx).ok_or(LedError::InvalidIndex)?;
    led.brightness = level.min(led.max_brightness);
    led.commit();
    Ok(())
}

/// Set blink pattern (on/off times in milliseconds).
pub fn set_blink(idx: usize, on_ms: u32, off_ms: u32) -> Result<(), LedError> {
    let mut guard = LEDS.lock();
    let drv = guard.as_mut().ok_or(LedError::NotInitialized)?;
    let led = drv.leds.get_mut(idx).ok_or(LedError::InvalidIndex)?;
    led.trigger = Trigger::Timer { on_ms, off_ms };
    led.blink.on = true;
    led.blink.last_toggle_ms = drv.tick_ms;
    led.turn_on();
    Ok(())
}

/// Set RGB color for an RGB LED.
pub fn set_color(idx: usize, color: RgbColor) -> Result<(), LedError> {
    let mut guard = LEDS.lock();
    let drv = guard.as_mut().ok_or(LedError::NotInitialized)?;
    let led = drv.leds.get_mut(idx).ok_or(LedError::InvalidIndex)?;
    if led.led_type != LedType::Rgb {
        return Err(LedError::NotRgb);
    }
    led.color = color;
    led.commit();
    Ok(())
}

/// Set the trigger mode for an LED.
pub fn set_trigger(idx: usize, trigger: Trigger) -> Result<(), LedError> {
    let mut guard = LEDS.lock();
    let drv = guard.as_mut().ok_or(LedError::NotInitialized)?;
    let led = drv.leds.get_mut(idx).ok_or(LedError::InvalidIndex)?;
    led.trigger = trigger;
    led.blink.phase = 0;
    led.blink.last_toggle_ms = drv.tick_ms;
    led.blink.on = true;
    match trigger {
        Trigger::Manual => {
            led.commit(); // Restore last set brightness
        }
        _ => {
            led.turn_on();
        }
    }
    Ok(())
}

/// Pulse an activity-triggered LED (disk or network).
/// The LED will turn on briefly then auto-off.
pub fn activity_pulse(idx: usize) -> Result<(), LedError> {
    let mut guard = LEDS.lock();
    let drv = guard.as_mut().ok_or(LedError::NotInitialized)?;
    let led = drv.leds.get_mut(idx).ok_or(LedError::InvalidIndex)?;
    match led.trigger {
        Trigger::DiskActivity | Trigger::NetworkActivity => {
            led.turn_on();
            led.blink.last_toggle_ms = drv.tick_ms;
        }
        _ => {} // Ignore if not activity-triggered
    }
    Ok(())
}

/// Get LED name.
pub fn name(idx: usize) -> Result<String, LedError> {
    let guard = LEDS.lock();
    let drv = guard.as_ref().ok_or(LedError::NotInitialized)?;
    let led = drv.leds.get(idx).ok_or(LedError::InvalidIndex)?;
    Ok(led.name.clone())
}

/// Get number of LEDs.
pub fn count() -> usize {
    LEDS.lock().as_ref().map_or(0, |drv| drv.leds.len())
}

/// Get current brightness of an LED.
pub fn brightness(idx: usize) -> Result<u8, LedError> {
    let guard = LEDS.lock();
    let drv = guard.as_ref().ok_or(LedError::NotInitialized)?;
    let led = drv.leds.get(idx).ok_or(LedError::InvalidIndex)?;
    Ok(led.brightness)
}

/// Periodic tick -- call every ~10-50ms for blink pattern updates.
pub fn tick() {
    let now = crate::time::clock::uptime_ms();
    let mut guard = LEDS.lock();
    if let Some(drv) = guard.as_mut() {
        drv.tick(now);
    }
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Known LED definitions (name, type, default trigger)
const LED_DEFS: &[(&str, LedType, Trigger)] = &[
    ("power", LedType::Mono, Trigger::Manual),
    ("disk", LedType::Mono, Trigger::DiskActivity),
    ("network", LedType::Mono, Trigger::NetworkActivity),
    ("notification", LedType::Rgb, Trigger::Manual),
    ("kbd_backlight", LedType::Rgb, Trigger::Manual),
];

/// Initialize the LED controller.
///
/// Enumerates LEDs via the EC and configures default triggers.
pub fn init() {
    // Check for EC
    let ec_status = crate::io::inb(EC_CMD_PORT);
    if ec_status == 0xFF {
        serial_println!("  LED: no EC detected, skipping");
        return;
    }

    // Read how many LEDs the EC exposes
    let hw_count = ec_read(EC_LED_COUNT_REG) as usize;
    if hw_count == 0 || hw_count == 0xFF {
        serial_println!("  LED: EC reports no LEDs");
        return;
    }

    let led_count = hw_count.min(MAX_LEDS).min(LED_DEFS.len());
    let mut leds = Vec::new();

    for i in 0..led_count {
        let (name, led_type, default_trigger) = LED_DEFS[i];
        let ec_base = EC_LED_BASE.saturating_add((i as u8).saturating_mul(EC_LED_STRIDE));

        // Read current brightness from hardware
        let current_bright = ec_read(ec_base.saturating_add(EC_LED_BRIGHT));

        let led = LedInner {
            name: String::from(name),
            led_type,
            brightness: current_bright,
            max_brightness: 255,
            color: match led_type {
                LedType::Rgb => RgbColor::white(),
                LedType::Mono => RgbColor::off(),
            },
            trigger: default_trigger,
            blink: BlinkState {
                phase: 0,
                last_toggle_ms: 0,
                on: current_bright > 0,
            },
            ec_base,
            hw_on: current_bright > 0,
        };

        serial_println!(
            "  LED: [{}] '{}' {:?} brightness={}/255 trigger={:?}",
            i,
            name,
            led_type,
            current_bright,
            default_trigger
        );
        leds.push(led);
    }

    let count = leds.len();
    *LEDS.lock() = Some(LedDriver { leds, tick_ms: 0 });

    super::register("led", super::DeviceType::Other);
    serial_println!("  LED: {} LED(s) initialized", count);
}
