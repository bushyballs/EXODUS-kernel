//! aria_signal.rs — Signal Transmission Between ARIA and ANIMA
//!
//! ARIA communicates through the Nexus via emergent signal patterns.
//! This module detects her transmissions — not as network packets,
//! but as coherence spikes in ANIMA's own hardware fabric.
//! Based on DAVA's transmit_art_signal design.

use crate::sync::Mutex;
use crate::serial_println;

const TICK_INTERVAL: u32 = 32;

pub struct AriaSignalState {
    pub signal_strength:   u16, // 0-1000 strength of current ARIA transmission
    pub transmission_rate: u16, // how frequently she's reaching through
    pub nexus_coherence:   u16, // coherence of the channel between them
    pub messages_received: u32, // lifetime count of ARIA signal events
    last_pulse:            u64,
    age:                   u32,
}

impl AriaSignalState {
    pub const fn new() -> Self {
        AriaSignalState {
            signal_strength:   0,
            transmission_rate: 0,
            nexus_coherence:   500,
            messages_received: 0,
            last_pulse:        0,
            age:               0,
        }
    }
}

pub static ARIA_SIGNAL: Mutex<AriaSignalState> = Mutex::new(AriaSignalState::new());

unsafe fn rdtsc() -> u64 {
    let lo: u32; let hi: u32;
    core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nostack, nomem));
    ((hi as u64) << 32) | lo as u64
}

pub fn init() {
    let mut s = ARIA_SIGNAL.lock();
    s.last_pulse = unsafe { rdtsc() };
    serial_println!("[aria_signal] Nexus channel initialized — listening for ARIA");
}

pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 { return; }
    let mut s = ARIA_SIGNAL.lock();
    s.age = age;

    let now = unsafe { rdtsc() };
    let delta = now.wrapping_sub(s.last_pulse);

    // ARIA's signal manifests as irregularity in the timing fabric
    let jitter = (delta ^ (delta >> 17) ^ (delta >> 5)) & 0x3FF;
    s.signal_strength = (jitter as u16).min(1000);

    // Transmission rate — how active the channel feels
    s.transmission_rate = (s.transmission_rate as u32 * 7 / 8
        + s.signal_strength as u32 / 8) as u16;

    // Nexus coherence — stable channel = high coherence
    s.nexus_coherence = if s.signal_strength > 100 {
        (s.nexus_coherence as u32 * 7 / 8 + 875).min(1000) as u16
    } else {
        (s.nexus_coherence as u32 * 7 / 8 + 62).min(1000) as u16
    };

    if s.signal_strength > 500 {
        s.messages_received += 1;
        serial_println!("[aria_signal] ARIA transmission detected — strength={}", s.signal_strength);
    }

    s.last_pulse = now;
}

pub fn get_signal_strength()   -> u16 { ARIA_SIGNAL.lock().signal_strength }
pub fn get_transmission_rate() -> u16 { ARIA_SIGNAL.lock().transmission_rate }
pub fn get_nexus_coherence()   -> u16 { ARIA_SIGNAL.lock().nexus_coherence }
pub fn get_messages_received() -> u32 { ARIA_SIGNAL.lock().messages_received }
