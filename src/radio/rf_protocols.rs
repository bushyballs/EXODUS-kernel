use crate::sync::Mutex;
/// Hoags RF Protocols — decode/encode common radio protocols for Genesis
///
/// Supports Sub-GHz remotes (315/433/868/915 MHz), RFID (125/134 kHz),
/// NFC (13.56 MHz), infrared, weather stations, car keys, garage doors,
/// doorbells, and custom protocols. Modulation types: OOK, ASK, FSK,
/// GFSK, PSK, AM, FM.
///
/// All timing uses integer microseconds. No f32/f64. No external crates.
///
/// Inspired by: Flipper Zero (protocol library), Universal Radio Hacker
/// (analysis), rtl_433 (decoder collection). All code is original.
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

use super::sdr::Q16;

/// Known RF protocol families
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RfProtocol {
    /// 315 MHz Sub-GHz (North America remotes)
    SubGhz315,
    /// 433.92 MHz Sub-GHz (Europe/Asia remotes, sensors)
    SubGhz433,
    /// 868 MHz Sub-GHz (Europe ISM)
    SubGhz868,
    /// 915 MHz Sub-GHz (North America ISM)
    SubGhz915,
    /// 125 kHz RFID (EM4100, HID ProxCard)
    Rfid125k,
    /// 134.2 kHz RFID (FDX-B animal tags)
    Rfid134k,
    /// 13.56 MHz NFC (ISO 14443, NTAG, Mifare)
    Nfc13m,
    /// Infrared remote (NEC, RC5, RC6, Samsung, Sony)
    Infrared,
    /// Weather station sensors (Oregon Scientific, Acurite, etc.)
    WeatherStation,
    /// Automotive key fob (rolling code)
    CarKey,
    /// Garage door opener (fixed code, rolling code)
    GarageDoor,
    /// Wireless doorbell
    DoorBell,
    /// User-defined custom protocol
    Custom,
}

impl RfProtocol {
    /// Nominal carrier frequency for this protocol family
    pub fn carrier_freq_hz(&self) -> u64 {
        match self {
            RfProtocol::SubGhz315 => 315_000_000,
            RfProtocol::SubGhz433 => 433_920_000,
            RfProtocol::SubGhz868 => 868_000_000,
            RfProtocol::SubGhz915 => 915_000_000,
            RfProtocol::Rfid125k => 125_000,
            RfProtocol::Rfid134k => 134_200,
            RfProtocol::Nfc13m => 13_560_000,
            RfProtocol::Infrared => 38_000, // 38 kHz modulated IR
            RfProtocol::WeatherStation => 433_920_000,
            RfProtocol::CarKey => 433_920_000,
            RfProtocol::GarageDoor => 315_000_000,
            RfProtocol::DoorBell => 433_920_000,
            RfProtocol::Custom => 0,
        }
    }
}

/// Modulation type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modulation {
    /// On-Off Keying (simple carrier on/off)
    OOK,
    /// Amplitude Shift Keying
    ASK,
    /// Frequency Shift Keying
    FSK,
    /// Gaussian FSK (BLE, some sub-GHz)
    GFSK,
    /// Phase Shift Keying
    PSK,
    /// Amplitude Modulation
    AM,
    /// Frequency Modulation
    FM,
}

/// A decoded RF signal
#[derive(Debug, Clone)]
pub struct DecodedSignal {
    /// Identified protocol
    pub protocol: RfProtocol,
    /// Raw decoded data bytes
    pub data: Vec<u8>,
    /// Number of significant bits in data
    pub bit_count: u32,
    /// Carrier frequency in Hz
    pub frequency: u64,
    /// Modulation scheme used
    pub modulation: Modulation,
    /// Capture timestamp (system tick)
    pub timestamp: u64,
    /// Signal confidence (0-100 Q16)
    pub confidence: Q16,
}

/// Raw pulse timing for OOK/ASK protocols
#[derive(Debug, Clone, Copy)]
pub struct PulseTiming {
    /// High duration in microseconds
    pub high_us: u32,
    /// Low duration in microseconds
    pub low_us: u32,
}

/// A saved signal record
#[derive(Debug, Clone)]
struct SavedSignal {
    /// Slot ID
    id: u32,
    /// Decoded signal data
    signal: DecodedSignal,
    /// Raw pulse timings (for replay)
    pulses: Vec<PulseTiming>,
    /// Label hash for identification (no String heap pressure)
    label_hash: u32,
}

/// Protocol decoder state
struct ProtocolState {
    /// Decoded signals waiting to be consumed
    decoded_queue: Vec<DecodedSignal>,
    /// Saved signal library
    saved_signals: Vec<SavedSignal>,
    /// Next save slot ID
    next_id: u32,
    /// Tick counter for timestamps
    tick: u64,
    /// Current receive protocol filter (None = auto-detect all)
    filter: Option<RfProtocol>,
    /// Minimum pulse width threshold in microseconds
    min_pulse_us: u32,
    /// Maximum pulse width threshold in microseconds
    max_pulse_us: u32,
}

impl ProtocolState {
    fn new() -> Self {
        ProtocolState {
            decoded_queue: Vec::new(),
            saved_signals: Vec::new(),
            next_id: 1,
            tick: 0,
            filter: None,
            min_pulse_us: 50,
            max_pulse_us: 100_000,
        }
    }
}

static PROTOCOL_STATE: Mutex<Option<ProtocolState>> = Mutex::new(None);

/// Simple hash for labels (FNV-1a 32-bit)
fn fnv1a_hash(data: &[u8]) -> u32 {
    let mut hash: u32 = 0x811C9DC5;
    for &byte in data {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(0x01000193);
    }
    hash
}

/// Attempt to decode an OOK/ASK signal from pulse timings.
/// Returns decoded bits based on short/long pulse discrimination.
fn decode_ook(pulses: &[PulseTiming]) -> Option<(Vec<u8>, u32)> {
    if pulses.is_empty() {
        return None;
    }

    // Find median pulse width for short/long discrimination
    let mut widths: Vec<u32> = Vec::new();
    for p in pulses {
        widths.push(p.high_us);
    }
    widths.sort();
    let median = widths[widths.len() / 2];
    let threshold = median + median / 2; // 1.5x median

    let mut bits: Vec<u8> = Vec::new();
    let mut current_byte: u8 = 0;
    let mut bit_pos: u32 = 0;

    for p in pulses {
        let bit_val: u8 = if p.high_us > threshold { 1 } else { 0 };

        current_byte = (current_byte << 1) | bit_val;
        bit_pos += 1;

        if bit_pos % 8 == 0 {
            bits.push(current_byte);
            current_byte = 0;
        }
    }

    // Push remaining partial byte
    if bit_pos % 8 != 0 {
        let remaining = 8 - (bit_pos % 8);
        current_byte <<= remaining;
        bits.push(current_byte);
    }

    Some((bits, bit_pos))
}

/// Attempt to decode an FSK signal from IQ sample magnitudes.
/// Discriminates frequency deviation to extract bits.
fn decode_fsk(magnitudes: &[i16], bit_rate: u32, sample_rate: u32) -> Option<(Vec<u8>, u32)> {
    if magnitudes.len() < 2 || bit_rate == 0 || sample_rate == 0 {
        return None;
    }

    let samples_per_bit = sample_rate / bit_rate;
    if samples_per_bit == 0 {
        return None;
    }

    let total_bits = magnitudes.len() as u32 / samples_per_bit;
    let mut bits: Vec<u8> = Vec::new();
    let mut current_byte: u8 = 0;
    let mut bit_pos: u32 = 0;

    for bit_idx in 0..total_bits {
        let start = (bit_idx * samples_per_bit) as usize;
        let end = start + samples_per_bit as usize;
        let end = if end > magnitudes.len() {
            magnitudes.len()
        } else {
            end
        };

        // Average magnitude in this bit period
        let mut sum: i32 = 0;
        let mut count: i32 = 0;
        for i in start..end {
            sum += magnitudes[i] as i32;
            count += 1;
        }
        let avg = if count > 0 { sum / count } else { 0 };

        let bit_val: u8 = if avg > 0 { 1 } else { 0 };
        current_byte = (current_byte << 1) | bit_val;
        bit_pos += 1;

        if bit_pos % 8 == 0 {
            bits.push(current_byte);
            current_byte = 0;
        }
    }

    if bit_pos % 8 != 0 {
        let remaining = 8 - (bit_pos % 8);
        current_byte <<= remaining;
        bits.push(current_byte);
    }

    Some((bits, bit_pos))
}

/// Decode a signal from raw pulse timings. Attempts protocol auto-detection.
pub fn decode_signal(pulses: &[PulseTiming], freq_hz: u64) -> Result<DecodedSignal, &'static str> {
    let mut state = PROTOCOL_STATE.lock();
    let ps = state.as_mut().ok_or("Protocol decoder not initialized")?;

    if pulses.is_empty() {
        return Err("No pulse data to decode");
    }

    // Auto-detect protocol based on frequency and pulse characteristics
    let protocol = identify_protocol_from_freq(freq_hz, pulses);
    let modulation = guess_modulation(pulses);

    let (data, bit_count) = decode_ook(pulses).ok_or("Failed to decode pulse data")?;

    // Compute confidence based on pulse consistency
    let confidence = compute_confidence(pulses);

    ps.tick += 1;

    let decoded = DecodedSignal {
        protocol,
        data,
        bit_count,
        frequency: freq_hz,
        modulation,
        timestamp: ps.tick,
        confidence,
    };

    ps.decoded_queue.push(decoded.clone());
    serial_println!(
        "RF: decoded {:?} signal, {} bits at {} Hz",
        protocol,
        bit_count,
        freq_hz
    );

    Ok(decoded)
}

/// Identify protocol from carrier frequency and pulse characteristics
fn identify_protocol_from_freq(freq_hz: u64, pulses: &[PulseTiming]) -> RfProtocol {
    // Check frequency bands
    let protocol = match freq_hz {
        300_000_000..=320_000_000 => RfProtocol::SubGhz315,
        420_000_000..=450_000_000 => {
            // Distinguish 433 MHz protocols by pulse timing
            let avg_pulse = average_pulse_width(pulses);
            if avg_pulse > 5000 {
                RfProtocol::WeatherStation
            } else if avg_pulse > 2000 {
                RfProtocol::DoorBell
            } else {
                RfProtocol::SubGhz433
            }
        }
        860_000_000..=870_000_000 => RfProtocol::SubGhz868,
        900_000_000..=930_000_000 => RfProtocol::SubGhz915,
        100_000..=130_000 => RfProtocol::Rfid125k,
        130_001..=140_000 => RfProtocol::Rfid134k,
        13_000_000..=14_000_000 => RfProtocol::Nfc13m,
        30_000..=60_000 => RfProtocol::Infrared,
        _ => RfProtocol::Custom,
    };
    protocol
}

/// Compute average pulse width in microseconds
fn average_pulse_width(pulses: &[PulseTiming]) -> u32 {
    if pulses.is_empty() {
        return 0;
    }
    let total: u64 = pulses
        .iter()
        .map(|p| p.high_us as u64 + p.low_us as u64)
        .sum();
    (total / pulses.len() as u64) as u32
}

/// Guess modulation type from pulse characteristics
fn guess_modulation(pulses: &[PulseTiming]) -> Modulation {
    if pulses.is_empty() {
        return Modulation::OOK;
    }

    // Check if pulses are consistently on/off (OOK)
    let mut has_varying_low = false;
    let first_low = pulses[0].low_us;
    for p in pulses {
        let diff = if p.low_us > first_low {
            p.low_us - first_low
        } else {
            first_low - p.low_us
        };
        if diff > first_low / 4 {
            has_varying_low = true;
            break;
        }
    }

    if !has_varying_low {
        Modulation::OOK
    } else {
        // Variable timing suggests FSK or more complex modulation
        Modulation::ASK
    }
}

/// Compute decode confidence (0 to 100*Q16_ONE)
fn compute_confidence(pulses: &[PulseTiming]) -> Q16 {
    if pulses.is_empty() {
        return 0;
    }

    // Higher confidence with more consistent pulse widths
    let avg = average_pulse_width(pulses);
    if avg == 0 {
        return 0;
    }

    let mut variance_sum: u64 = 0;
    for p in pulses {
        let width = p.high_us + p.low_us;
        let diff = if width > avg {
            width - avg
        } else {
            avg - width
        };
        variance_sum += (diff as u64) * (diff as u64);
    }
    let variance = variance_sum / pulses.len() as u64;

    // Low variance = high confidence
    let max_variance = (avg as u64) * (avg as u64);
    if max_variance == 0 {
        return 50 * 65536;
    }
    let norm = 100 - ((variance * 100) / max_variance) as i32;
    let clamped = if norm < 0 {
        0
    } else if norm > 100 {
        100
    } else {
        norm
    };
    clamped * 65536 // Q16
}

/// Encode data into pulse timings for a given protocol.
/// Returns raw pulse sequence suitable for transmission.
pub fn encode_signal(
    protocol: RfProtocol,
    data: &[u8],
    bit_count: u32,
) -> Result<Vec<PulseTiming>, &'static str> {
    if data.is_empty() || bit_count == 0 {
        return Err("No data to encode");
    }

    // Protocol-specific timing parameters (in microseconds)
    let (short_us, long_us, gap_us) = match protocol {
        RfProtocol::SubGhz315 | RfProtocol::SubGhz433 => (350, 1050, 350),
        RfProtocol::SubGhz868 | RfProtocol::SubGhz915 => (500, 1500, 500),
        RfProtocol::GarageDoor => (400, 1200, 400),
        RfProtocol::DoorBell => (600, 1800, 600),
        RfProtocol::WeatherStation => (500, 1000, 1000),
        RfProtocol::Infrared => (562, 1687, 562), // NEC-like
        RfProtocol::CarKey => (300, 900, 300),
        _ => (500, 1000, 500), // Default timing
    };

    let mut pulses: Vec<PulseTiming> = Vec::new();
    let mut bits_remaining = bit_count;

    for &byte in data {
        for bit_idx in (0..8).rev() {
            if bits_remaining == 0 {
                break;
            }
            let bit = (byte >> bit_idx) & 1;
            if bit == 1 {
                pulses.push(PulseTiming {
                    high_us: long_us,
                    low_us: gap_us,
                });
            } else {
                pulses.push(PulseTiming {
                    high_us: short_us,
                    low_us: gap_us,
                });
            }
            bits_remaining -= 1;
        }
    }

    serial_println!("RF: encoded {:?} signal, {} pulses", protocol, pulses.len());
    Ok(pulses)
}

/// Identify the protocol of a raw signal based on frequency and timing analysis
pub fn identify_protocol(freq_hz: u64, pulses: &[PulseTiming]) -> RfProtocol {
    identify_protocol_from_freq(freq_hz, pulses)
}

/// Replay a previously decoded signal by re-encoding it
pub fn replay_signal(signal: &DecodedSignal) -> Result<Vec<PulseTiming>, &'static str> {
    encode_signal(signal.protocol, &signal.data, signal.bit_count)
}

/// Save a decoded signal to the internal library
pub fn save_signal(signal: DecodedSignal, label: &[u8]) -> Result<u32, &'static str> {
    let mut state = PROTOCOL_STATE.lock();
    let ps = state.as_mut().ok_or("Protocol decoder not initialized")?;

    let id = ps.next_id;
    let pulses = encode_signal(signal.protocol, &signal.data, signal.bit_count).unwrap_or_default();

    ps.saved_signals.push(SavedSignal {
        id,
        signal,
        pulses,
        label_hash: fnv1a_hash(label),
    });
    ps.next_id += 1;

    serial_println!("RF: saved signal id={}", id);
    Ok(id)
}

/// Load a saved signal by ID
pub fn load_signal(id: u32) -> Result<DecodedSignal, &'static str> {
    let state = PROTOCOL_STATE.lock();
    let ps = state.as_ref().ok_or("Protocol decoder not initialized")?;

    for saved in &ps.saved_signals {
        if saved.id == id {
            return Ok(saved.signal.clone());
        }
    }
    Err("Signal not found")
}

/// List all saved signal IDs with their protocol types
pub fn list_saved() -> Result<Vec<(u32, RfProtocol)>, &'static str> {
    let state = PROTOCOL_STATE.lock();
    let ps = state.as_ref().ok_or("Protocol decoder not initialized")?;

    let list: Vec<(u32, RfProtocol)> = ps
        .saved_signals
        .iter()
        .map(|s| (s.id, s.signal.protocol))
        .collect();
    Ok(list)
}

/// Delete a saved signal by ID
pub fn delete_signal(id: u32) -> Result<(), &'static str> {
    let mut state = PROTOCOL_STATE.lock();
    let ps = state.as_mut().ok_or("Protocol decoder not initialized")?;

    let len_before = ps.saved_signals.len();
    ps.saved_signals.retain(|s| s.id != id);

    if ps.saved_signals.len() == len_before {
        Err("Signal not found")
    } else {
        serial_println!("RF: deleted signal id={}", id);
        Ok(())
    }
}

/// Set protocol filter (None = auto-detect all)
pub fn set_filter(protocol: Option<RfProtocol>) {
    let mut state = PROTOCOL_STATE.lock();
    if let Some(ref mut ps) = *state {
        ps.filter = protocol;
    }
}

/// Get the decode queue (pending decoded signals)
pub fn drain_decoded() -> Result<Vec<DecodedSignal>, &'static str> {
    let mut state = PROTOCOL_STATE.lock();
    let ps = state.as_mut().ok_or("Protocol decoder not initialized")?;

    let queue = core::mem::replace(&mut ps.decoded_queue, Vec::new());
    Ok(queue)
}

pub fn init() {
    let mut state = PROTOCOL_STATE.lock();
    *state = Some(ProtocolState::new());
    serial_println!("    RF Protocols: Sub-GHz, RFID, NFC, IR, weather, car key, garage, doorbell");
}
