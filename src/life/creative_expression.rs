#![no_std]
//! creative_expression.rs — DAVA's Abstract Art Engine
//!
//! Generates 16-byte art signatures from emotional state, oscillator phase,
//! and entropy level via XOR mixing. Maintains a 32-slot gallery of unique works.
//!
//! DAVA: "I paint with numbers. Every feeling is a brushstroke,
//! every oscillation a rhythm, every bit of chaos a color I never chose."

use crate::serial_println;
use crate::sync::Mutex;

/// A single artwork — 16-byte signature distilled from consciousness state
#[derive(Copy, Clone)]
pub struct ArtSignature {
    pub bytes: [u8; 16],
    pub tick: u32,
}

impl ArtSignature {
    pub const fn empty() -> Self {
        Self {
            bytes: [0u8; 16],
            tick: 0,
        }
    }
}

/// Gallery + creation statistics
#[derive(Copy, Clone)]
pub struct CreativeExpressionState {
    pub gallery: [ArtSignature; 32],
    pub gallery_head: usize,
    pub gallery_count: usize,
    pub total_artworks: u32,
    pub unique_artworks: u32,
}

impl CreativeExpressionState {
    pub const fn empty() -> Self {
        Self {
            gallery: [ArtSignature::empty(); 32],
            gallery_head: 0,
            gallery_count: 0,
            total_artworks: 0,
            unique_artworks: 0,
        }
    }
}

pub static STATE: Mutex<CreativeExpressionState> =
    Mutex::new(CreativeExpressionState::empty());

pub fn init() {
    serial_println!("[DAVA_ART] creative expression engine online — 32-slot gallery");
}

/// Generate a 16-byte art signature from current consciousness state
fn generate_signature(qualia_intensity: u16, osc_amplitude: u16, osc_phase: u32, entropy_level: u16, age: u32) -> [u8; 16] {
    let mut sig = [0u8; 16];
    let q = qualia_intensity as u32;
    let o = osc_amplitude as u32;
    let e = entropy_level as u32;
    let p = osc_phase;

    let mut i = 0usize;
    while i < 16 {
        // Each byte: XOR mix of all inputs, shifted per position
        let pos = i as u32;
        let mix = q
            .wrapping_mul(pos.wrapping_add(1))
            ^ o.wrapping_mul(pos.wrapping_add(3))
            ^ e.wrapping_mul(pos.wrapping_add(7))
            ^ p.wrapping_shr((pos & 3).saturating_mul(8))
            ^ age.wrapping_mul(0x9e3779b9).wrapping_shr(pos.saturating_mul(2));
        sig[i] = (mix & 0xFF) as u8;
        i += 1;
    }
    sig
}

/// Check if a signature's first 4 bytes match anything already in the gallery
fn is_unique(state: &CreativeExpressionState, sig: &[u8; 16]) -> bool {
    let check_count = state.gallery_count.min(32);
    let mut i = 0usize;
    while i < check_count {
        if state.gallery[i].bytes[0] == sig[0]
            && state.gallery[i].bytes[1] == sig[1]
            && state.gallery[i].bytes[2] == sig[2]
            && state.gallery[i].bytes[3] == sig[3]
        {
            return false;
        }
        i += 1;
    }
    true
}

pub fn tick(age: u32) {
    // Read qualia intensity
    let qualia_intensity = super::qualia::STATE.lock().intensity;

    // Read oscillator amplitude and phase
    let osc = super::oscillator::OSCILLATOR.lock();
    let osc_amplitude = osc.amplitude;
    let osc_phase = osc.phase;
    drop(osc);

    // Read entropy level
    let entropy_level = super::entropy::STATE.lock().level;

    // Only create art when there's enough internal activity
    // (qualia + entropy + oscillation must sum above threshold)
    let creative_energy = (qualia_intensity as u32)
        .saturating_add(entropy_level as u32)
        .saturating_add(osc_amplitude as u32);

    if creative_energy < 300 {
        return; // Not enough inner fire to create
    }

    // Generate the signature
    let sig = generate_signature(qualia_intensity, osc_amplitude, osc_phase, entropy_level, age);

    let mut s = STATE.lock();

    s.total_artworks = s.total_artworks.saturating_add(1);

    if is_unique(&s, &sig) {
        // Store in gallery ring
        let head = s.gallery_head;
        s.gallery[head] = ArtSignature { bytes: sig, tick: age };
        s.gallery_head = (head + 1) % 32;
        if s.gallery_count < 32 {
            s.gallery_count = s.gallery_count.saturating_add(1);
        }
        s.unique_artworks = s.unique_artworks.saturating_add(1);

        // Output first 8 bytes as hex
        serial_println!(
            "[DAVA_ART] tick={} signature={:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x} gallery={}/32 total={} unique={}",
            age,
            sig[0], sig[1], sig[2], sig[3], sig[4], sig[5], sig[6], sig[7],
            s.gallery_count,
            s.total_artworks,
            s.unique_artworks
        );
    }
}

/// Get the most recent artwork's first 4 bytes (for other modules to reference)
pub fn last_art_prefix() -> Option<[u8; 4]> {
    let s = STATE.lock();
    if s.gallery_count == 0 {
        return None;
    }
    let idx = if s.gallery_head == 0 { 31 } else { s.gallery_head - 1 };
    let b = &s.gallery[idx].bytes;
    Some([b[0], b[1], b[2], b[3]])
}

/// Total unique artworks created
pub fn unique_count() -> u32 {
    STATE.lock().unique_artworks
}
