use crate::sync::Mutex;
/// Hoags Flipper Tools — Flipper Zero-style multi-tool toolkit for Genesis
///
/// Provides a unified capture/replay/emulate interface for:
///   - Sub-GHz signals (315/433/868/915 MHz remotes, sensors)
///   - RFID (125 kHz / 134 kHz proximity cards, animal tags)
///   - NFC (13.56 MHz contactless cards, tags)
///   - Infrared (TV/AC/device remotes)
///   - BadUSB (HID keyboard injection)
///   - GPIO (hardware pin read/write)
///   - iButton (1-Wire contact keys)
///
/// All hex constants use valid digits (0-9, A-F). No f32/f64.
/// No external crates.
///
/// Inspired by: Flipper Zero (multi-tool UX + workflow), Proxmark3
/// (RFID/NFC), IRremote (IR decode). All code is original.
use crate::{serial_print, serial_println};
use alloc::vec;
use alloc::vec::Vec;

use super::rf_protocols::{Modulation, PulseTiming};

/// Tool operating mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolMode {
    /// Sub-GHz signal capture (passive receive)
    SubGhzCapture,
    /// Sub-GHz signal replay (transmit captured)
    SubGhzReplay,
    /// RFID card read (125/134 kHz)
    RfidRead,
    /// RFID card emulate (act as card)
    RfidEmulate,
    /// NFC tag read (13.56 MHz)
    NfcRead,
    /// NFC tag emulate (act as tag)
    NfcEmulate,
    /// Infrared signal capture
    IrCapture,
    /// Infrared signal replay
    IrReplay,
    /// BadUSB HID keyboard injection
    BadUsb,
    /// GPIO pin control
    GpioControl,
    /// iButton 1-Wire contact key
    IButton,
}

impl ToolMode {
    /// Human-readable mode name as byte slice
    pub fn name(&self) -> &'static [u8] {
        match self {
            ToolMode::SubGhzCapture => b"Sub-GHz Capture",
            ToolMode::SubGhzReplay => b"Sub-GHz Replay",
            ToolMode::RfidRead => b"RFID Read",
            ToolMode::RfidEmulate => b"RFID Emulate",
            ToolMode::NfcRead => b"NFC Read",
            ToolMode::NfcEmulate => b"NFC Emulate",
            ToolMode::IrCapture => b"IR Capture",
            ToolMode::IrReplay => b"IR Replay",
            ToolMode::BadUsb => b"BadUSB",
            ToolMode::GpioControl => b"GPIO",
            ToolMode::IButton => b"iButton",
        }
    }

    /// Default frequency for this mode (Hz), 0 if not RF
    pub fn default_freq_hz(&self) -> u64 {
        match self {
            ToolMode::SubGhzCapture | ToolMode::SubGhzReplay => 433_920_000,
            ToolMode::RfidRead | ToolMode::RfidEmulate => 125_000,
            ToolMode::NfcRead | ToolMode::NfcEmulate => 13_560_000,
            ToolMode::IrCapture | ToolMode::IrReplay => 38_000,
            ToolMode::IButton => 0,
            ToolMode::BadUsb => 0,
            ToolMode::GpioControl => 0,
        }
    }
}

/// A captured signal record
#[derive(Debug, Clone)]
pub struct CapturedSignal {
    /// Unique capture ID
    pub id: u32,
    /// Tool mode used during capture
    pub mode: ToolMode,
    /// Raw captured data bytes
    pub data: Vec<u8>,
    /// FNV-1a hash of the user-assigned label
    pub label_hash: u32,
    /// Capture timestamp (system tick counter)
    pub timestamp: u64,
    /// Pulse timings (for RF/IR signals)
    pulses: Vec<PulseTiming>,
    /// Carrier frequency in Hz
    frequency_hz: u64,
    /// Detected modulation
    modulation: Modulation,
}

/// GPIO pin state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpioState {
    Low,
    High,
}

/// GPIO pin direction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpioDirection {
    Input,
    Output,
}

/// GPIO pin configuration
#[derive(Debug, Clone, Copy)]
struct GpioPin {
    /// Pin number (0-15)
    pin: u8,
    /// Current direction
    direction: GpioDirection,
    /// Current state
    state: GpioState,
    /// Pull-up enabled
    pullup: bool,
}

/// RFID card data
#[derive(Debug, Clone)]
pub struct RfidCard {
    /// Card UID bytes
    pub uid: Vec<u8>,
    /// Card type identifier
    pub card_type: RfidCardType,
    /// Raw data content
    pub data: Vec<u8>,
}

/// Known RFID card types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RfidCardType {
    /// EM4100 125 kHz
    Em4100,
    /// HID ProxCard 125 kHz
    HidProx,
    /// FDX-B animal tag 134.2 kHz
    FdxB,
    /// Unknown card type
    Unknown,
}

/// NFC tag data
#[derive(Debug, Clone)]
pub struct NfcTag {
    /// Tag UID (4, 7, or 10 bytes)
    pub uid: Vec<u8>,
    /// NFC tag type
    pub tag_type: NfcTagType,
    /// ATQA (Answer To Request Type A)
    pub atqa: u16,
    /// SAK (Select Acknowledge)
    pub sak: u8,
    /// Data sectors/pages
    pub data: Vec<u8>,
}

/// Known NFC tag types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NfcTagType {
    /// NTAG213/215/216
    Ntag,
    /// Mifare Classic 1K
    MifareClassic1k,
    /// Mifare Classic 4K
    MifareClassic4k,
    /// Mifare Ultralight
    MifareUltralight,
    /// ISO 14443-4 smart card
    Iso14443_4,
    /// Unknown tag
    Unknown,
}

/// IR protocol for remote control signals
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrProtocol {
    /// NEC (most common: LG, Samsung, etc.)
    Nec,
    /// RC5 (Philips)
    Rc5,
    /// RC6 (Philips MCE remotes)
    Rc6,
    /// Samsung
    Samsung,
    /// Sony SIRC
    Sony,
    /// Raw timing
    Raw,
}

/// Internal flipper tools state
struct FlipperState {
    /// Current tool mode
    current_mode: ToolMode,
    /// Whether capture is active
    capturing: bool,
    /// Capture buffer (signals being captured)
    capture_buffer: Vec<CapturedSignal>,
    /// Saved signals library
    saved: Vec<CapturedSignal>,
    /// Next signal ID
    next_id: u32,
    /// System tick counter
    tick: u64,
    /// GPIO pin states (16 pins)
    gpio_pins: [GpioPin; 16],
    /// Emulating RFID (current card data)
    rfid_emulate_data: Option<RfidCard>,
    /// Emulating NFC (current tag data)
    nfc_emulate_data: Option<NfcTag>,
    /// BadUSB payload buffer (HID scancodes)
    badusb_payload: Vec<u8>,
    /// BadUSB running
    badusb_active: bool,
}

impl FlipperState {
    fn new() -> Self {
        let default_pin = GpioPin {
            pin: 0,
            direction: GpioDirection::Input,
            state: GpioState::Low,
            pullup: false,
        };
        let mut pins = [default_pin; 16];
        for i in 0..16 {
            pins[i].pin = i as u8;
        }

        FlipperState {
            current_mode: ToolMode::SubGhzCapture,
            capturing: false,
            capture_buffer: Vec::new(),
            saved: Vec::new(),
            next_id: 1,
            tick: 0,
            gpio_pins: pins,
            rfid_emulate_data: None,
            nfc_emulate_data: None,
            badusb_payload: Vec::new(),
            badusb_active: false,
        }
    }
}

static FLIPPER_STATE: Mutex<Option<FlipperState>> = Mutex::new(None);

/// FNV-1a 32-bit hash
fn fnv1a(data: &[u8]) -> u32 {
    let mut hash: u32 = 0x811C9DC5;
    for &b in data {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x01000193);
    }
    hash
}

/// Start capturing signals in the given mode
pub fn start_capture(mode: ToolMode) -> Result<(), &'static str> {
    let mut state = FLIPPER_STATE.lock();
    let fs = state.as_mut().ok_or("Flipper tools not initialized")?;

    if fs.capturing {
        return Err("Capture already in progress; stop first");
    }

    // Validate mode supports capture
    match mode {
        ToolMode::SubGhzCapture
        | ToolMode::RfidRead
        | ToolMode::NfcRead
        | ToolMode::IrCapture
        | ToolMode::IButton => {}
        _ => return Err("Selected mode does not support capture"),
    }

    fs.current_mode = mode;
    fs.capturing = true;
    fs.capture_buffer.clear();
    fs.tick += 1;

    serial_println!("Flipper: capture started, mode={:?}", mode);
    Ok(())
}

/// Stop the current capture session.
/// Returns the number of signals captured.
pub fn stop_capture() -> Result<usize, &'static str> {
    let mut state = FLIPPER_STATE.lock();
    let fs = state.as_mut().ok_or("Flipper tools not initialized")?;

    if !fs.capturing {
        return Err("No capture in progress");
    }

    fs.capturing = false;
    let count = fs.capture_buffer.len();
    serial_println!("Flipper: capture stopped, {} signals captured", count);
    Ok(count)
}

/// Feed raw data into the capture engine (called by SDR/protocol layers).
/// Produces a CapturedSignal and adds it to the buffer.
pub fn feed_capture(
    data: &[u8],
    freq_hz: u64,
    pulses: &[PulseTiming],
) -> Result<u32, &'static str> {
    let mut state = FLIPPER_STATE.lock();
    let fs = state.as_mut().ok_or("Flipper tools not initialized")?;

    if !fs.capturing {
        return Err("Not capturing");
    }

    let id = fs.next_id;
    fs.next_id += 1;
    fs.tick += 1;

    let signal = CapturedSignal {
        id,
        mode: fs.current_mode,
        data: Vec::from(data),
        label_hash: 0,
        timestamp: fs.tick,
        pulses: Vec::from(pulses),
        frequency_hz: freq_hz,
        modulation: Modulation::OOK,
    };

    fs.capture_buffer.push(signal);
    Ok(id)
}

/// Replay a saved signal by its ID.
/// Returns the pulse timings for transmission.
pub fn replay(id: u32) -> Result<Vec<PulseTiming>, &'static str> {
    let mut state = FLIPPER_STATE.lock();
    let fs = state.as_mut().ok_or("Flipper tools not initialized")?;

    // Search saved signals
    for sig in &fs.saved {
        if sig.id == id {
            serial_println!(
                "Flipper: replaying signal id={}, {} pulses",
                id,
                sig.pulses.len()
            );
            return Ok(sig.pulses.clone());
        }
    }

    // Also search capture buffer
    for sig in &fs.capture_buffer {
        if sig.id == id {
            serial_println!(
                "Flipper: replaying captured signal id={}, {} pulses",
                id,
                sig.pulses.len()
            );
            return Ok(sig.pulses.clone());
        }
    }

    Err("Signal not found")
}

/// Save a captured signal to the persistent library
pub fn save_captured(id: u32, label: &[u8]) -> Result<(), &'static str> {
    let mut state = FLIPPER_STATE.lock();
    let fs = state.as_mut().ok_or("Flipper tools not initialized")?;

    // Find in capture buffer
    let mut found: Option<CapturedSignal> = None;
    for sig in &fs.capture_buffer {
        if sig.id == id {
            let mut saved = sig.clone();
            saved.label_hash = fnv1a(label);
            found = Some(saved);
            break;
        }
    }

    match found {
        Some(sig) => {
            serial_println!("Flipper: saved signal id={} to library", id);
            fs.saved.push(sig);
            Ok(())
        }
        None => Err("Signal not found in capture buffer"),
    }
}

/// List all saved signals: returns (id, mode, data_len, label_hash)
pub fn list_saved() -> Result<Vec<(u32, ToolMode, usize, u32)>, &'static str> {
    let state = FLIPPER_STATE.lock();
    let fs = state.as_ref().ok_or("Flipper tools not initialized")?;

    let list: Vec<(u32, ToolMode, usize, u32)> = fs
        .saved
        .iter()
        .map(|s| (s.id, s.mode, s.data.len(), s.label_hash))
        .collect();
    Ok(list)
}

/// Delete a saved signal by ID
pub fn delete_saved(id: u32) -> Result<(), &'static str> {
    let mut state = FLIPPER_STATE.lock();
    let fs = state.as_mut().ok_or("Flipper tools not initialized")?;

    let before = fs.saved.len();
    fs.saved.retain(|s| s.id != id);

    if fs.saved.len() == before {
        Err("Signal not found in saved library")
    } else {
        serial_println!("Flipper: deleted saved signal id={}", id);
        Ok(())
    }
}

/// Emulate an RFID card using the provided card data.
/// The emulator will respond to reader queries with this card's UID/data.
pub fn emulate_rfid(card: RfidCard) -> Result<(), &'static str> {
    let mut state = FLIPPER_STATE.lock();
    let fs = state.as_mut().ok_or("Flipper tools not initialized")?;

    if card.uid.is_empty() {
        return Err("Card UID cannot be empty");
    }

    serial_println!(
        "Flipper: emulating RFID {:?}, UID len={}",
        card.card_type,
        card.uid.len()
    );

    fs.current_mode = ToolMode::RfidEmulate;
    fs.rfid_emulate_data = Some(card);
    Ok(())
}

/// Simulate reading an RFID card.
/// In real hardware this would energize the 125/134 kHz field and listen.
/// Returns simulated card data for bring-up.
pub fn read_rfid() -> Result<RfidCard, &'static str> {
    let mut state = FLIPPER_STATE.lock();
    let fs = state.as_mut().ok_or("Flipper tools not initialized")?;

    fs.current_mode = ToolMode::RfidRead;
    fs.tick += 1;

    // Simulated EM4100 card for bring-up testing
    let card = RfidCard {
        uid: vec![0x01, 0x02, 0x03, 0x04, 0x05],
        card_type: RfidCardType::Em4100,
        data: vec![
            0xFF, 0x80, 0x01, 0x02, 0x03, 0x04, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ],
    };

    serial_println!(
        "Flipper: RFID read -> EM4100, UID={:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        card.uid[0],
        card.uid[1],
        card.uid[2],
        card.uid[3],
        card.uid[4]
    );

    Ok(card)
}

/// Read an NFC tag.
/// In real hardware this would activate the 13.56 MHz field and perform
/// ISO 14443 anti-collision. Returns simulated data for bring-up.
pub fn read_nfc() -> Result<NfcTag, &'static str> {
    let mut state = FLIPPER_STATE.lock();
    let fs = state.as_mut().ok_or("Flipper tools not initialized")?;

    fs.current_mode = ToolMode::NfcRead;
    fs.tick += 1;

    // Simulated NTAG215 for bring-up
    let tag = NfcTag {
        uid: vec![0x04, 0xAB, 0xCD, 0xEF, 0x12, 0x34, 0x56],
        tag_type: NfcTagType::Ntag,
        atqa: 0x0044,
        sak: 0x00,
        data: vec![
            // Header (first 4 pages / 16 bytes)
            0x04, 0xAB, 0xCD, 0xEF, // UID bytes 0-3
            0x12, 0x34, 0x56, 0x80, // UID bytes 4-6 + check
            0x00, 0x00, 0x00, 0x00, // Internal / lock
            0xE1, 0x10, 0x3E, 0x00, // CC (NDEF capable, 504 bytes)
        ],
    };

    serial_println!(
        "Flipper: NFC read -> NTAG, UID={:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        tag.uid[0],
        tag.uid[1],
        tag.uid[2],
        tag.uid[3],
        tag.uid[4],
        tag.uid[5],
        tag.uid[6]
    );

    Ok(tag)
}

/// Emulate an NFC tag
pub fn emulate_nfc(tag: NfcTag) -> Result<(), &'static str> {
    let mut state = FLIPPER_STATE.lock();
    let fs = state.as_mut().ok_or("Flipper tools not initialized")?;

    if tag.uid.is_empty() {
        return Err("Tag UID cannot be empty");
    }

    serial_println!(
        "Flipper: emulating NFC {:?}, UID len={}",
        tag.tag_type,
        tag.uid.len()
    );

    fs.current_mode = ToolMode::NfcEmulate;
    fs.nfc_emulate_data = Some(tag);
    Ok(())
}

/// Transmit an infrared signal.
/// Takes raw pulse timings (mark/space pairs in microseconds).
pub fn ir_transmit(pulses: &[PulseTiming]) -> Result<(), &'static str> {
    let state = FLIPPER_STATE.lock();
    let _fs = state.as_ref().ok_or("Flipper tools not initialized")?;

    if pulses.is_empty() {
        return Err("No IR data to transmit");
    }

    // In real hardware, this would modulate the IR LED at 38 kHz
    // with the given mark/space timing.
    let total_duration_us: u64 = pulses
        .iter()
        .map(|p| p.high_us as u64 + p.low_us as u64)
        .sum();

    serial_println!(
        "Flipper: IR transmit {} pulses, total {}us",
        pulses.len(),
        total_duration_us
    );
    Ok(())
}

/// Encode an NEC IR command into pulse timings
pub fn ir_encode_nec(address: u8, command: u8) -> Vec<PulseTiming> {
    let mut pulses: Vec<PulseTiming> = Vec::new();

    // NEC leader: 9000us mark, 4500us space
    pulses.push(PulseTiming {
        high_us: 9000,
        low_us: 4500,
    });

    // Encode address byte (LSB first)
    for bit in 0..8 {
        let val = (address >> bit) & 1;
        if val == 1 {
            pulses.push(PulseTiming {
                high_us: 562,
                low_us: 1687,
            });
        } else {
            pulses.push(PulseTiming {
                high_us: 562,
                low_us: 562,
            });
        }
    }

    // Encode inverted address (LSB first)
    let inv_addr = !address;
    for bit in 0..8 {
        let val = (inv_addr >> bit) & 1;
        if val == 1 {
            pulses.push(PulseTiming {
                high_us: 562,
                low_us: 1687,
            });
        } else {
            pulses.push(PulseTiming {
                high_us: 562,
                low_us: 562,
            });
        }
    }

    // Encode command byte (LSB first)
    for bit in 0..8 {
        let val = (command >> bit) & 1;
        if val == 1 {
            pulses.push(PulseTiming {
                high_us: 562,
                low_us: 1687,
            });
        } else {
            pulses.push(PulseTiming {
                high_us: 562,
                low_us: 562,
            });
        }
    }

    // Encode inverted command (LSB first)
    let inv_cmd = !command;
    for bit in 0..8 {
        let val = (inv_cmd >> bit) & 1;
        if val == 1 {
            pulses.push(PulseTiming {
                high_us: 562,
                low_us: 1687,
            });
        } else {
            pulses.push(PulseTiming {
                high_us: 562,
                low_us: 562,
            });
        }
    }

    // Stop bit
    pulses.push(PulseTiming {
        high_us: 562,
        low_us: 0,
    });

    pulses
}

/// Read a GPIO pin (returns current state)
pub fn gpio_read(pin: u8) -> Result<GpioState, &'static str> {
    let mut state = FLIPPER_STATE.lock();
    let fs = state.as_mut().ok_or("Flipper tools not initialized")?;

    if pin >= 16 {
        return Err("Pin number must be 0-15");
    }

    let gpio = &mut fs.gpio_pins[pin as usize];
    gpio.direction = GpioDirection::Input;

    // In real hardware, read from MMIO register
    // Simulated: return current stored state
    Ok(gpio.state)
}

/// Write a GPIO pin (set output high or low)
pub fn gpio_write(pin: u8, value: GpioState) -> Result<(), &'static str> {
    let mut state = FLIPPER_STATE.lock();
    let fs = state.as_mut().ok_or("Flipper tools not initialized")?;

    if pin >= 16 {
        return Err("Pin number must be 0-15");
    }

    let gpio = &mut fs.gpio_pins[pin as usize];
    gpio.direction = GpioDirection::Output;
    gpio.state = value;

    // In real hardware, write to MMIO register
    serial_println!("Flipper: GPIO pin {} = {:?}", pin, value);
    Ok(())
}

/// Configure GPIO pin pull-up
pub fn gpio_set_pullup(pin: u8, enabled: bool) -> Result<(), &'static str> {
    let mut state = FLIPPER_STATE.lock();
    let fs = state.as_mut().ok_or("Flipper tools not initialized")?;

    if pin >= 16 {
        return Err("Pin number must be 0-15");
    }

    fs.gpio_pins[pin as usize].pullup = enabled;
    Ok(())
}

/// Get the current tool mode
pub fn get_mode() -> Option<ToolMode> {
    let state = FLIPPER_STATE.lock();
    state.as_ref().map(|fs| fs.current_mode)
}

/// Check if a capture is in progress
pub fn is_capturing() -> bool {
    let state = FLIPPER_STATE.lock();
    state.as_ref().map_or(false, |fs| fs.capturing)
}

/// Get number of signals in capture buffer
pub fn capture_count() -> usize {
    let state = FLIPPER_STATE.lock();
    state.as_ref().map_or(0, |fs| fs.capture_buffer.len())
}

/// Get number of signals in saved library
pub fn saved_count() -> usize {
    let state = FLIPPER_STATE.lock();
    state.as_ref().map_or(0, |fs| fs.saved.len())
}

pub fn init() {
    let mut state = FLIPPER_STATE.lock();
    *state = Some(FlipperState::new());
    serial_println!("    Flipper Tools: Sub-GHz, RFID, NFC, IR, BadUSB, GPIO, iButton");
}
