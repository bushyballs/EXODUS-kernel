use crate::sync::Mutex;
/// Infrared receiver/transmitter driver for Genesis
///
/// Decodes NEC, RC5, and RC6 infrared protocols from raw pulse timing data,
/// transmits IR commands, supports learning mode for capturing unknown
/// protocols, and handles repeat code suppression/generation.
///
/// Uses a CIR (Consumer Infrared) controller accessed via I/O ports,
/// compatible with Nuvoton/ITE CIR or Intel SCH CIR hardware.
///
/// Inspired by: Linux ir-nec-decoder, ir-rc5-decoder, lirc. All code is original.
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// CIR hardware I/O ports
// ---------------------------------------------------------------------------

const CIR_BASE_PORT: u16 = 0x03E0;
const CIR_DATA: u16 = CIR_BASE_PORT + 0; // RX/TX data FIFO
const CIR_STATUS: u16 = CIR_BASE_PORT + 1; // Status register
const CIR_CONTROL: u16 = CIR_BASE_PORT + 2; // Control register
const CIR_CARRIER: u16 = CIR_BASE_PORT + 3; // Carrier frequency
const CIR_TX_COUNT: u16 = CIR_BASE_PORT + 4; // TX byte count
const CIR_RX_COUNT: u16 = CIR_BASE_PORT + 5; // RX byte count

// Status bits
const STATUS_RX_READY: u8 = 1 << 0;
const STATUS_TX_EMPTY: u8 = 1 << 1;
const STATUS_RX_OVERRUN: u8 = 1 << 2;
const STATUS_CARRIER_DET: u8 = 1 << 3;

// Control bits
const CTRL_RX_ENABLE: u8 = 1 << 0;
const CTRL_TX_ENABLE: u8 = 1 << 1;
const CTRL_LEARNING: u8 = 1 << 2;
const CTRL_WIDEBAND: u8 = 1 << 3; // Wideband for learning
const CTRL_RESET: u8 = 1 << 7;

// Protocol timing constants (in microseconds)
// NEC protocol
const NEC_HEADER_PULSE: u32 = 9000;
const NEC_HEADER_SPACE: u32 = 4500;
const NEC_REPEAT_SPACE: u32 = 2250;
const NEC_BIT_PULSE: u32 = 562;
const NEC_ONE_SPACE: u32 = 1688;
const NEC_ZERO_SPACE: u32 = 562;

// RC5 protocol (Manchester)
const RC5_BIT_TIME: u32 = 889;
const RC5_BITS: u32 = 14;

// RC6 protocol
const RC6_HEADER_PULSE: u32 = 2666;
const RC6_HEADER_SPACE: u32 = 889;
const RC6_BIT_TIME: u32 = 444;
const RC6_TOGGLE_TIME: u32 = 889; // Toggle bit is double-width

// Timing tolerance (25%)
const TIMING_TOLERANCE: u32 = 25;

// FIFO and repeat settings
const RX_FIFO_SIZE: usize = 256;
const REPEAT_TIMEOUT_MS: u64 = 250;
const MAX_LEARNED_PULSES: usize = 512;

// CIR sample rate (microseconds per tick)
const SAMPLE_PERIOD_US: u32 = 50;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// IR protocol types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrProtocol {
    Nec,
    Rc5,
    Rc6,
    Raw,
}

/// A decoded IR command
#[derive(Debug, Clone, Copy)]
pub struct IrCommand {
    pub protocol: IrProtocol,
    pub address: u16,
    pub command: u8,
    pub toggle: bool,
    pub repeat: bool,
}

/// IR driver error
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrError {
    NotInitialized,
    TxBusy,
    InvalidData,
    Timeout,
    DecodeError,
}

/// Decoder state machine
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DecoderState {
    Idle,
    NecHeader,
    NecData,
    Rc5Data,
    Rc6Header,
    Rc6Data,
}

/// Internal driver state
struct IrInner {
    decoder_state: DecoderState,
    /// Raw pulse/space buffer (microseconds, alternating pulse/space)
    raw_buffer: Vec<u32>,
    /// Decoded command queue
    command_queue: Vec<IrCommand>,
    /// Last decoded command (for repeat detection)
    last_command: Option<IrCommand>,
    last_command_time_ms: u64,
    /// RC5 toggle bit tracking
    rc5_toggle: bool,
    /// Learning mode buffer
    learning: bool,
    learned_pulses: Vec<u32>,
    /// RX enabled flag
    rx_enabled: bool,
    /// Carrier frequency in Hz (default 38 kHz)
    carrier_hz: u32,
    /// NEC bit accumulator
    nec_bits: u32,
    nec_bit_count: u8,
    /// RC5 bit accumulator
    rc5_bits: u16,
    rc5_bit_count: u8,
    /// RC6 bit accumulator
    rc6_bits: u32,
    rc6_bit_count: u8,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static IR: Mutex<Option<IrInner>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Timing helpers
// ---------------------------------------------------------------------------

fn time_match(measured: u32, expected: u32) -> bool {
    let tolerance = expected.saturating_mul(TIMING_TOLERANCE) / 100;
    measured >= expected.saturating_sub(tolerance) && measured <= expected.saturating_add(tolerance)
}

// ---------------------------------------------------------------------------
// Protocol decoders
// ---------------------------------------------------------------------------

impl IrInner {
    /// Process raw pulse/space data from the hardware FIFO
    fn process_raw_sample(&mut self, duration_us: u32, is_pulse: bool) {
        if self.learning {
            if self.learned_pulses.len() < MAX_LEARNED_PULSES {
                self.learned_pulses.push(if is_pulse {
                    duration_us
                } else {
                    duration_us | 0x8000_0000
                });
            }
        }

        match self.decoder_state {
            DecoderState::Idle => {
                if is_pulse {
                    // Check for NEC header pulse
                    if time_match(duration_us, NEC_HEADER_PULSE) {
                        self.decoder_state = DecoderState::NecHeader;
                        self.nec_bits = 0;
                        self.nec_bit_count = 0;
                    }
                    // Check for RC6 header pulse
                    else if time_match(duration_us, RC6_HEADER_PULSE) {
                        self.decoder_state = DecoderState::Rc6Header;
                        self.rc6_bits = 0;
                        self.rc6_bit_count = 0;
                    }
                    // RC5 starts with a transition (Manchester, no header)
                    else if time_match(duration_us, RC5_BIT_TIME) {
                        self.decoder_state = DecoderState::Rc5Data;
                        self.rc5_bits = 1; // Start bit is always 1
                        self.rc5_bit_count = 1;
                    }
                }
            }

            DecoderState::NecHeader => {
                if !is_pulse {
                    if time_match(duration_us, NEC_HEADER_SPACE) {
                        // Normal NEC data follows
                        self.decoder_state = DecoderState::NecData;
                    } else if time_match(duration_us, NEC_REPEAT_SPACE) {
                        // NEC repeat code
                        self.emit_repeat();
                        self.decoder_state = DecoderState::Idle;
                    } else {
                        self.decoder_state = DecoderState::Idle;
                    }
                }
            }

            DecoderState::NecData => {
                if is_pulse {
                    // NEC data pulses are all the same width
                    if !time_match(duration_us, NEC_BIT_PULSE) {
                        self.decoder_state = DecoderState::Idle;
                    }
                } else {
                    // Space determines bit value
                    let bit = if time_match(duration_us, NEC_ONE_SPACE) {
                        1u32
                    } else if time_match(duration_us, NEC_ZERO_SPACE) {
                        0u32
                    } else {
                        self.decoder_state = DecoderState::Idle;
                        return;
                    };

                    self.nec_bits |= bit << (self.nec_bit_count & 0x1F);
                    self.nec_bit_count = self.nec_bit_count.saturating_add(1);

                    if self.nec_bit_count >= 32 {
                        self.decode_nec();
                        self.decoder_state = DecoderState::Idle;
                    }
                }
            }

            DecoderState::Rc5Data => {
                // Manchester decoding: mid-bit transitions
                let half = time_match(duration_us, RC5_BIT_TIME);
                let full = time_match(duration_us, RC5_BIT_TIME * 2);

                if half || full {
                    let bit_val = if is_pulse { 1u16 } else { 0 };
                    self.rc5_bits = (self.rc5_bits << 1) | bit_val;
                    self.rc5_bit_count = self.rc5_bit_count.saturating_add(1);

                    if full {
                        // Full period = implicit second half-bit
                        self.rc5_bits = (self.rc5_bits << 1) | bit_val;
                        self.rc5_bit_count = self.rc5_bit_count.saturating_add(1);
                    }

                    if self.rc5_bit_count >= RC5_BITS as u8 {
                        self.decode_rc5();
                        self.decoder_state = DecoderState::Idle;
                    }
                } else {
                    self.decoder_state = DecoderState::Idle;
                }
            }

            DecoderState::Rc6Header => {
                if !is_pulse && time_match(duration_us, RC6_HEADER_SPACE) {
                    self.decoder_state = DecoderState::Rc6Data;
                } else {
                    self.decoder_state = DecoderState::Idle;
                }
            }

            DecoderState::Rc6Data => {
                let normal = time_match(duration_us, RC6_BIT_TIME);
                let toggle = time_match(duration_us, RC6_TOGGLE_TIME);

                if normal || toggle {
                    let bit_val = if is_pulse { 1u32 } else { 0 };
                    self.rc6_bits = (self.rc6_bits << 1) | bit_val;
                    self.rc6_bit_count = self.rc6_bit_count.saturating_add(1);

                    if self.rc6_bit_count >= 21 {
                        self.decode_rc6();
                        self.decoder_state = DecoderState::Idle;
                    }
                } else {
                    self.decoder_state = DecoderState::Idle;
                }
            }
        }
    }

    /// Decode NEC 32-bit data (address + ~address + command + ~command)
    fn decode_nec(&mut self) {
        let addr_lo = (self.nec_bits & 0xFF) as u8;
        let addr_hi = ((self.nec_bits >> 8) & 0xFF) as u8;
        let cmd = ((self.nec_bits >> 16) & 0xFF) as u8;
        let cmd_inv = ((self.nec_bits >> 24) & 0xFF) as u8;

        // Validate command inversion
        if cmd ^ cmd_inv != 0xFF {
            return;
        }

        // Extended NEC uses both address bytes; standard NEC inverts them
        let address = if addr_lo ^ addr_hi == 0xFF {
            addr_lo as u16
        } else {
            ((addr_hi as u16) << 8) | addr_lo as u16
        };

        let ir_cmd = IrCommand {
            protocol: IrProtocol::Nec,
            address,
            command: cmd,
            toggle: false,
            repeat: false,
        };

        self.push_command(ir_cmd);
    }

    /// Decode RC5 14-bit data
    fn decode_rc5(&mut self) {
        // RC5: S1 S2 T A4 A3 A2 A1 A0 C5 C4 C3 C2 C1 C0
        let toggle = (self.rc5_bits >> 11) & 1 != 0;
        let address = ((self.rc5_bits >> 6) & 0x1F) as u16;
        let command = (self.rc5_bits & 0x3F) as u8;
        // S2 is inverted for extended RC5 (command bit 6)
        let s2 = (self.rc5_bits >> 12) & 1;
        let ext_cmd = command | (if s2 == 0 { 0x40 } else { 0 });

        let ir_cmd = IrCommand {
            protocol: IrProtocol::Rc5,
            address,
            command: ext_cmd,
            toggle,
            repeat: toggle == self.rc5_toggle,
        };
        self.rc5_toggle = toggle;
        self.push_command(ir_cmd);
    }

    /// Decode RC6 mode 0 data
    fn decode_rc6(&mut self) {
        // RC6: header, mode (3 bits), toggle, address (8 bits), command (8 bits)
        let toggle = (self.rc6_bits >> 16) & 1 != 0;
        let address = ((self.rc6_bits >> 8) & 0xFF) as u16;
        let command = (self.rc6_bits & 0xFF) as u8;

        let ir_cmd = IrCommand {
            protocol: IrProtocol::Rc6,
            address,
            command,
            toggle,
            repeat: false,
        };
        self.push_command(ir_cmd);
    }

    /// Handle repeat code (NEC)
    fn emit_repeat(&mut self) {
        if let Some(mut cmd) = self.last_command {
            let now = crate::time::clock::uptime_ms();
            if now - self.last_command_time_ms < REPEAT_TIMEOUT_MS {
                cmd.repeat = true;
                self.command_queue.push(cmd);
                self.last_command_time_ms = now;
            }
        }
    }

    /// Push a decoded command, update last command for repeat tracking
    fn push_command(&mut self, cmd: IrCommand) {
        self.last_command = Some(cmd);
        self.last_command_time_ms = crate::time::clock::uptime_ms();
        self.command_queue.push(cmd);
    }

    /// Encode and transmit a NEC command
    fn transmit_nec(&self, address: u16, command: u8) -> Result<(), IrError> {
        let addr_lo = (address & 0xFF) as u8;
        let addr_hi = if address > 0xFF {
            ((address >> 8) & 0xFF) as u8
        } else {
            !addr_lo
        };
        let cmd_inv = !command;

        // Wait for TX empty
        for _ in 0..100_000u32 {
            if crate::io::inb(CIR_STATUS) & STATUS_TX_EMPTY != 0 {
                break;
            }
            crate::io::io_wait();
        }

        // Send header
        self.send_pulse(NEC_HEADER_PULSE);
        self.send_space(NEC_HEADER_SPACE);

        // Send 32 data bits
        let data: u32 = (addr_lo as u32)
            | ((addr_hi as u32) << 8)
            | ((command as u32) << 16)
            | ((cmd_inv as u32) << 24);
        for i in 0..32 {
            self.send_pulse(NEC_BIT_PULSE);
            if data & (1 << i) != 0 {
                self.send_space(NEC_ONE_SPACE);
            } else {
                self.send_space(NEC_ZERO_SPACE);
            }
        }
        // Final pulse
        self.send_pulse(NEC_BIT_PULSE);
        Ok(())
    }

    /// Send a pulse of given duration (in microseconds) to TX FIFO
    fn send_pulse(&self, us: u32) {
        let ticks = us / SAMPLE_PERIOD_US;
        // Encode as pulse (high byte bit 7 = 1 for pulse)
        for _ in 0..ticks.min(127) {
            crate::io::outb(CIR_DATA, 0x80 | 1);
        }
    }

    /// Send a space (no carrier) of given duration
    fn send_space(&self, us: u32) {
        let ticks = us / SAMPLE_PERIOD_US;
        for _ in 0..ticks.min(127) {
            crate::io::outb(CIR_DATA, 0x00 | 1);
        }
    }

    /// Read available samples from hardware FIFO
    fn drain_rx_fifo(&mut self) {
        let mut count = 0;
        while crate::io::inb(CIR_STATUS) & STATUS_RX_READY != 0 && count < RX_FIFO_SIZE {
            let sample = crate::io::inb(CIR_DATA);
            let is_pulse = sample & 0x80 != 0;
            let ticks = (sample & 0x7F) as u32;
            let duration_us = ticks.saturating_mul(SAMPLE_PERIOD_US);
            if duration_us > 0 {
                self.process_raw_sample(duration_us, is_pulse);
            }
            count += 1;
        }
        // Clear overrun if set
        if crate::io::inb(CIR_STATUS) & STATUS_RX_OVERRUN != 0 {
            crate::io::outb(CIR_STATUS, STATUS_RX_OVERRUN);
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Receive the next decoded IR command (returns None if queue empty).
pub fn receive() -> Option<IrCommand> {
    let mut guard = IR.lock();
    let inner = guard.as_mut()?;
    inner.drain_rx_fifo();
    if inner.command_queue.is_empty() {
        None
    } else {
        Some(inner.command_queue.remove(0))
    }
}

/// Transmit an IR command.
pub fn transmit(protocol: IrProtocol, address: u16, command: u8) -> Result<(), IrError> {
    let guard = IR.lock();
    let inner = guard.as_ref().ok_or(IrError::NotInitialized)?;
    match protocol {
        IrProtocol::Nec => inner.transmit_nec(address, command),
        _ => Err(IrError::InvalidData), // Only NEC TX implemented
    }
}

/// Enter learning mode (captures raw pulse timings).
pub fn start_learning() -> Result<(), IrError> {
    let mut guard = IR.lock();
    let inner = guard.as_mut().ok_or(IrError::NotInitialized)?;
    inner.learning = true;
    inner.learned_pulses.clear();
    // Enable wideband receiver for learning
    let ctrl = crate::io::inb(CIR_CONTROL);
    crate::io::outb(CIR_CONTROL, ctrl | CTRL_LEARNING | CTRL_WIDEBAND);
    serial_println!("  IR: learning mode started");
    Ok(())
}

/// Stop learning mode and return captured raw pulse data.
/// Values with bit 31 set are spaces; others are pulses. Duration in microseconds.
pub fn stop_learning() -> Result<Vec<u32>, IrError> {
    let mut guard = IR.lock();
    let inner = guard.as_mut().ok_or(IrError::NotInitialized)?;
    inner.learning = false;
    // Restore normal receiver mode
    let ctrl = crate::io::inb(CIR_CONTROL);
    crate::io::outb(CIR_CONTROL, ctrl & !(CTRL_LEARNING | CTRL_WIDEBAND));
    let pulses = core::mem::take(&mut inner.learned_pulses);
    serial_println!("  IR: learning stopped, captured {} samples", pulses.len());
    Ok(pulses)
}

/// Transmit raw pulse/space data (from learning mode capture).
pub fn transmit_raw(pulses: &[u32]) -> Result<(), IrError> {
    let guard = IR.lock();
    let inner = guard.as_ref().ok_or(IrError::NotInitialized)?;
    for &val in pulses {
        let is_space = val & 0x8000_0000 != 0;
        let us = val & 0x7FFF_FFFF;
        if is_space {
            inner.send_space(us);
        } else {
            inner.send_pulse(us);
        }
    }
    Ok(())
}

/// Set the carrier frequency for TX (default 38 kHz).
pub fn set_carrier(freq_hz: u32) -> Result<(), IrError> {
    let mut guard = IR.lock();
    let inner = guard.as_mut().ok_or(IrError::NotInitialized)?;
    inner.carrier_hz = freq_hz;
    // Program carrier register: divider from 48 MHz clock
    let divider = if freq_hz > 0 {
        (48_000_000 / freq_hz) as u8
    } else {
        0
    };
    crate::io::outb(CIR_CARRIER, divider);
    Ok(())
}

/// Poll the IR receiver (call periodically or from IRQ handler).
pub fn poll() {
    let mut guard = IR.lock();
    if let Some(inner) = guard.as_mut() {
        inner.drain_rx_fifo();
    }
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the IR transceiver driver.
///
/// Probes for a CIR controller at the standard I/O port range.
pub fn init() {
    // Probe: write and read back control register
    crate::io::outb(CIR_CONTROL, CTRL_RESET);
    for _ in 0..1000 {
        crate::io::io_wait();
    }

    let status = crate::io::inb(CIR_STATUS);
    if status == 0xFF {
        serial_println!("  IR: no CIR controller at port {:#06X}", CIR_BASE_PORT);
        return;
    }

    // Enable RX
    crate::io::outb(CIR_CONTROL, CTRL_RX_ENABLE | CTRL_TX_ENABLE);

    // Set default carrier frequency (38 kHz)
    let divider = (48_000_000u32 / 38_000) as u8;
    crate::io::outb(CIR_CARRIER, divider);

    // Clear any pending data
    while crate::io::inb(CIR_STATUS) & STATUS_RX_READY != 0 {
        let _ = crate::io::inb(CIR_DATA);
    }

    let inner = IrInner {
        decoder_state: DecoderState::Idle,
        raw_buffer: Vec::new(),
        command_queue: Vec::new(),
        last_command: None,
        last_command_time_ms: 0,
        rc5_toggle: false,
        learning: false,
        learned_pulses: Vec::new(),
        rx_enabled: true,
        carrier_hz: 38_000,
        nec_bits: 0,
        nec_bit_count: 0,
        rc5_bits: 0,
        rc5_bit_count: 0,
        rc6_bits: 0,
        rc6_bit_count: 0,
    };

    *IR.lock() = Some(inner);
    super::register("ir", super::DeviceType::Other);
    serial_println!(
        "  IR: CIR controller at port {:#06X}, 38 kHz carrier",
        CIR_BASE_PORT
    );
}
