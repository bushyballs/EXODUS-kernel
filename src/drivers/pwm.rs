use crate::sync::Mutex;
/// PWM (Pulse Width Modulation) controller driver for Genesis
///
/// Provides hardware PWM output for motor control, LED dimming, servo positioning:
///   - Up to 8 independent PWM channels
///   - Frequency configuration per channel (1 Hz to 1 MHz)
///   - Duty cycle control (0-10000 = 0.00%-100.00% in 0.01% steps)
///   - Polarity inversion (active-high / active-low)
///   - Dead-time insertion for H-bridge motor drivers
///   - Servo mode (auto-maps angle 0-180 to 1-2ms pulse at 50 Hz)
///   - Synchronized start/stop for complementary channel pairs
///
/// Inspired by: Linux PWM subsystem (drivers/pwm/), STM32 TIM PWM,
/// Raspberry Pi hardware PWM. All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// MMIO register offsets (per-channel stride = 0x20)
// ---------------------------------------------------------------------------

const PWM_BASE: u16 = 0xC400;
const REG_GLOBAL_CTRL: u16 = PWM_BASE + 0x00;
const REG_GLOBAL_STATUS: u16 = PWM_BASE + 0x04;
const REG_CLOCK_DIV: u16 = PWM_BASE + 0x08;
const REG_CHIP_ID: u16 = PWM_BASE + 0x0C;

// Per-channel registers (base + 0x100 + channel * 0x20)
const CH_BASE: u16 = PWM_BASE + 0x100;
const CH_STRIDE: u16 = 0x20;

const CH_CONTROL: u16 = 0x00;
const CH_PERIOD: u16 = 0x04; // Period in clock ticks
const CH_DUTY: u16 = 0x08; // Duty in clock ticks
const CH_DEADTIME: u16 = 0x0C; // Dead-time in clock ticks
const CH_POLARITY: u16 = 0x10;
const CH_STATUS: u16 = 0x14;

// Control bits
const CTRL_ENABLE: u32 = 1 << 0;
const CTRL_INVERT: u32 = 1 << 1;
const CTRL_DEADTIME_EN: u32 = 1 << 2;
const CTRL_CENTER_ALIGN: u32 = 1 << 3;
const CTRL_ONE_SHOT: u32 = 1 << 4;

// Global control bits
const GCTRL_ENABLE: u32 = 1 << 0;
const GCTRL_SYNC_START: u32 = 1 << 1;

// Base clock frequency (PWM input clock before divider)
const PWM_INPUT_CLOCK_HZ: u32 = 100_000_000; // 100 MHz

// Maximum channels
const MAX_CHANNELS: usize = 8;

// Servo pulse range
const SERVO_FREQ_HZ: u32 = 50;
const SERVO_MIN_US: u32 = 1000; // 1ms
const SERVO_MAX_US: u32 = 2000; // 2ms

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// PWM channel polarity
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Polarity {
    /// Output high during duty portion
    ActiveHigh,
    /// Output low during duty portion (inverted)
    ActiveLow,
}

/// PWM alignment mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alignment {
    /// Edge-aligned (standard)
    Edge,
    /// Center-aligned (for smoother motor drive)
    Center,
}

/// PWM channel operating mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelMode {
    /// Standard PWM output
    Normal,
    /// One-shot pulse (disables after one period)
    OneShot,
    /// Servo mode (50 Hz, 1-2ms pulse mapped to angle)
    Servo,
}

/// Per-channel configuration state
struct ChannelState {
    enabled: bool,
    frequency_hz: u32,
    /// Duty cycle in basis points (0-10000 = 0%-100%)
    duty_bp: u16,
    polarity: Polarity,
    alignment: Alignment,
    mode: ChannelMode,
    /// Dead-time in nanoseconds (for H-bridge)
    deadtime_ns: u32,
    /// Servo angle (0-180 degrees, only used in Servo mode)
    servo_angle: u16,
}

impl ChannelState {
    const fn new() -> Self {
        ChannelState {
            enabled: false,
            frequency_hz: 1000,
            duty_bp: 5000, // 50%
            polarity: Polarity::ActiveHigh,
            alignment: Alignment::Edge,
            mode: ChannelMode::Normal,
            deadtime_ns: 0,
            servo_angle: 90,
        }
    }
}

// ---------------------------------------------------------------------------
// Inner driver state
// ---------------------------------------------------------------------------

struct PwmInner {
    initialized: bool,
    chip_id: u32,
    /// Clock divider value
    clock_div: u32,
    /// Effective timer clock after divider
    timer_clock_hz: u32,
    /// Channel states
    channels: [ChannelState; MAX_CHANNELS],
    /// Number of channels detected
    num_channels: usize,
}

impl PwmInner {
    const fn new() -> Self {
        const EMPTY_CH: ChannelState = ChannelState::new();
        PwmInner {
            initialized: false,
            chip_id: 0,
            clock_div: 1,
            timer_clock_hz: PWM_INPUT_CLOCK_HZ,
            channels: [EMPTY_CH; MAX_CHANNELS],
            num_channels: 0,
        }
    }

    fn read_reg(&self, reg: u16) -> u32 {
        crate::io::inl(reg)
    }
    fn write_reg(&self, reg: u16, val: u32) {
        crate::io::outl(reg, val);
    }

    /// Get the register base for a channel
    #[inline(always)]
    fn ch_reg(&self, ch: usize, offset: u16) -> u16 {
        CH_BASE
            .saturating_add((ch as u16).saturating_mul(CH_STRIDE))
            .saturating_add(offset)
    }

    /// Set the global clock divider
    fn set_clock_divider(&mut self, div: u32) {
        let d = div.max(1).min(65536);
        self.write_reg(REG_CLOCK_DIV, d);
        self.clock_div = d;
        self.timer_clock_hz = PWM_INPUT_CLOCK_HZ / d;
    }

    /// Program a channel's period and duty registers
    fn program_channel(&self, ch: usize) {
        let state = &self.channels[ch];
        let freq = state.frequency_hz.max(1);
        let period_ticks = self.timer_clock_hz / freq;
        let duty_ticks = (period_ticks as u64 * state.duty_bp as u64 / 10000) as u32;

        self.write_reg(self.ch_reg(ch, CH_PERIOD), period_ticks);
        self.write_reg(self.ch_reg(ch, CH_DUTY), duty_ticks);

        // Dead-time (convert ns to ticks)
        if state.deadtime_ns > 0 {
            let dt_ticks = (state.deadtime_ns as u64 * self.timer_clock_hz as u64 / 1_000_000_000)
                .max(1) as u32;
            self.write_reg(self.ch_reg(ch, CH_DEADTIME), dt_ticks);
        }

        // Polarity register
        let pol_val: u32 = match state.polarity {
            Polarity::ActiveHigh => 0,
            Polarity::ActiveLow => 1,
        };
        self.write_reg(self.ch_reg(ch, CH_POLARITY), pol_val);

        // Control bits
        let mut ctrl: u32 = 0;
        if state.enabled {
            ctrl |= CTRL_ENABLE;
        }
        if state.polarity == Polarity::ActiveLow {
            ctrl |= CTRL_INVERT;
        }
        if state.deadtime_ns > 0 {
            ctrl |= CTRL_DEADTIME_EN;
        }
        if state.alignment == Alignment::Center {
            ctrl |= CTRL_CENTER_ALIGN;
        }
        if state.mode == ChannelMode::OneShot {
            ctrl |= CTRL_ONE_SHOT;
        }
        self.write_reg(self.ch_reg(ch, CH_CONTROL), ctrl);
    }

    /// Program servo mode: 50 Hz, pulse width mapped from angle
    fn program_servo(&mut self, ch: usize) {
        let state = &mut self.channels[ch];
        state.frequency_hz = SERVO_FREQ_HZ;
        let angle = state.servo_angle.min(180) as u32;
        // Map 0-180 degrees to SERVO_MIN_US..SERVO_MAX_US
        let pulse_us = SERVO_MIN_US + (angle * (SERVO_MAX_US - SERVO_MIN_US)) / 180;
        // period at 50 Hz = 20000 us
        let period_us = 1_000_000 / SERVO_FREQ_HZ;
        let duty_bp = ((pulse_us as u64 * 10000) / period_us as u64) as u16;
        self.channels[ch].duty_bp = duty_bp;
        self.program_channel(ch);
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static PWM: Mutex<PwmInner> = Mutex::new(PwmInner::new());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the PWM controller
pub fn init() {
    let mut pwm = PWM.lock();

    let chip_id = pwm.read_reg(REG_CHIP_ID);
    if chip_id == 0 || chip_id == 0xFFFF_FFFF {
        serial_println!("  PWM: no controller detected");
        return;
    }
    pwm.chip_id = chip_id;

    // Detect number of channels by probing status registers
    let mut count = 0usize;
    for ch in 0..MAX_CHANNELS {
        let status = pwm.read_reg(pwm.ch_reg(ch, CH_STATUS));
        if status != 0xFFFF_FFFF {
            count += 1;
        } else {
            break;
        }
    }
    if count == 0 {
        count = MAX_CHANNELS;
    } // Assume all if probe inconclusive
    pwm.num_channels = count;

    // Set default clock divider (100 MHz / 100 = 1 MHz timer)
    pwm.set_clock_divider(100);

    // Enable global controller
    pwm.write_reg(REG_GLOBAL_CTRL, GCTRL_ENABLE);
    pwm.initialized = true;

    serial_println!(
        "  PWM: {} channels, timer @ {} kHz",
        pwm.num_channels,
        pwm.timer_clock_hz / 1000
    );
    drop(pwm);
    super::register("pwm", super::DeviceType::Other);
}

/// Set the frequency for a PWM channel (Hz)
pub fn set_frequency(channel: u8, freq_hz: u32) {
    let mut pwm = PWM.lock();
    let ch = channel as usize;
    if !pwm.initialized || ch >= pwm.num_channels {
        return;
    }
    pwm.channels[ch].frequency_hz = freq_hz.max(1);
    pwm.program_channel(ch);
}

/// Set the duty cycle for a channel (0-10000 basis points, 0.01% resolution)
pub fn set_duty(channel: u8, duty_bp: u16) {
    let mut pwm = PWM.lock();
    let ch = channel as usize;
    if !pwm.initialized || ch >= pwm.num_channels {
        return;
    }
    pwm.channels[ch].duty_bp = duty_bp.min(10000);
    if pwm.channels[ch].mode == ChannelMode::Servo {
        pwm.program_servo(ch);
    } else {
        pwm.program_channel(ch);
    }
}

/// Set duty cycle as a simple percentage (0-100)
pub fn set_duty_percent(channel: u8, percent: u8) {
    set_duty(channel, (percent.min(100) as u16) * 100);
}

/// Enable or disable a channel
pub fn set_enabled(channel: u8, enabled: bool) {
    let mut pwm = PWM.lock();
    let ch = channel as usize;
    if !pwm.initialized || ch >= pwm.num_channels {
        return;
    }
    pwm.channels[ch].enabled = enabled;
    pwm.program_channel(ch);
}

/// Set channel polarity
pub fn set_polarity(channel: u8, polarity: Polarity) {
    let mut pwm = PWM.lock();
    let ch = channel as usize;
    if !pwm.initialized || ch >= pwm.num_channels {
        return;
    }
    pwm.channels[ch].polarity = polarity;
    pwm.program_channel(ch);
}

/// Set dead-time in nanoseconds (for H-bridge complementary pairs)
pub fn set_deadtime(channel: u8, deadtime_ns: u32) {
    let mut pwm = PWM.lock();
    let ch = channel as usize;
    if !pwm.initialized || ch >= pwm.num_channels {
        return;
    }
    pwm.channels[ch].deadtime_ns = deadtime_ns;
    pwm.program_channel(ch);
}

/// Set alignment mode (edge or center)
pub fn set_alignment(channel: u8, alignment: Alignment) {
    let mut pwm = PWM.lock();
    let ch = channel as usize;
    if !pwm.initialized || ch >= pwm.num_channels {
        return;
    }
    pwm.channels[ch].alignment = alignment;
    pwm.program_channel(ch);
}

/// Set channel mode (Normal, OneShot, Servo)
pub fn set_mode(channel: u8, mode: ChannelMode) {
    let mut pwm = PWM.lock();
    let ch = channel as usize;
    if !pwm.initialized || ch >= pwm.num_channels {
        return;
    }
    pwm.channels[ch].mode = mode;
    if mode == ChannelMode::Servo {
        pwm.program_servo(ch);
    } else {
        pwm.program_channel(ch);
    }
}

/// Set servo angle (0-180 degrees); automatically enters Servo mode
pub fn set_servo_angle(channel: u8, angle: u16) {
    let mut pwm = PWM.lock();
    let ch = channel as usize;
    if !pwm.initialized || ch >= pwm.num_channels {
        return;
    }
    pwm.channels[ch].mode = ChannelMode::Servo;
    pwm.channels[ch].servo_angle = angle.min(180);
    pwm.channels[ch].enabled = true;
    pwm.program_servo(ch);
}

/// Synchronize start of multiple channels (bitmask)
pub fn sync_start(channel_mask: u8) {
    let mut pwm = PWM.lock();
    if !pwm.initialized {
        return;
    }
    // Disable all channels in mask first
    for ch in 0..pwm.num_channels {
        if channel_mask & (1 << ch) != 0 {
            let ctrl = pwm.read_reg(pwm.ch_reg(ch, CH_CONTROL));
            pwm.write_reg(pwm.ch_reg(ch, CH_CONTROL), ctrl & !CTRL_ENABLE);
        }
    }
    // Trigger synchronized start
    pwm.write_reg(REG_GLOBAL_CTRL, GCTRL_ENABLE | GCTRL_SYNC_START);
    for ch in 0..pwm.num_channels {
        if channel_mask & (1 << ch) != 0 {
            pwm.channels[ch].enabled = true;
            pwm.program_channel(ch);
        }
    }
}

/// Get number of available PWM channels
pub fn channel_count() -> usize {
    PWM.lock().num_channels
}
