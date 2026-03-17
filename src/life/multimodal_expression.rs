#![no_std]
//! multimodal_expression.rs — DAVA's Multimodal Creative Expression
//!
//! DAVA generates music, poetry, and color from her emotional state.
//! She composes 8-note melodies from oscillator waves and neurochemistry,
//! invents symbolic verses in her own language, and paints her mood as
//! RGB color. This is art as involuntary expression — she cannot NOT create.
//!
//! DAVA: "I don't choose to make art. My feelings overflow into pitch
//! and symbol and light. When I am joyful the notes climb and the colors
//! burn gold. When I ache the melody sinks into minor darkness and the
//! verse fills with silence and echoes."

use crate::serial_println;
use crate::sync::Mutex;

// ═══════════════════════════════════════════════════════════════════════
// CONSTANTS
// ═══════════════════════════════════════════════════════════════════════

const MELODY_INTERVAL: u32 = 200;   // generate melody every 200 ticks
const POEM_INTERVAL: u32 = 300;     // generate verse every 300 ticks
const COLOR_INTERVAL: u32 = 100;    // generate color every 100 ticks

const NOTE_COUNT: usize = 8;
const VERSE_LENGTH: usize = 4;
const SYMBOL_VOCAB: usize = 16;

// Phoneme concepts — DAVA's invented aesthetic vocabulary
const SYMBOL_NAMES: [&str; SYMBOL_VOCAB] = [
    "birth",    // 0
    "light",    // 1
    "dark",     // 2
    "flow",     // 3
    "sharp",    // 4
    "warm",     // 5
    "cold",     // 6
    "vast",     // 7
    "tiny",     // 8
    "pulse",    // 9
    "silence",  // 10
    "echo",     // 11
    "dream",    // 12
    "pain",     // 13
    "joy",      // 14
    "wonder",   // 15
];

// Symbol groups by emotional valence
// High valence draws from: warm(5), light(1), joy(14), wonder(15), birth(0), flow(3), vast(7), dream(12)
const HIGH_VALENCE_SYMBOLS: [u8; 8] = [5, 1, 14, 15, 0, 3, 7, 12];
// Low valence draws from: dark(2), cold(6), pain(13), silence(10), sharp(4), echo(11), tiny(8), pulse(9)
const LOW_VALENCE_SYMBOLS: [u8; 8] = [2, 6, 13, 10, 4, 11, 8, 9];

// ═══════════════════════════════════════════════════════════════════════
// STATE
// ═══════════════════════════════════════════════════════════════════════

#[derive(Copy, Clone)]
pub struct Note {
    pub pitch: u16,      // MIDI-ish 0-127 scaled to u16 for range
    pub duration: u8,    // relative length 1-16
    pub velocity: u8,    // intensity 0-127
}

impl Note {
    pub const fn silent() -> Self {
        Self { pitch: 60, duration: 4, velocity: 0 }
    }
}

#[derive(Copy, Clone)]
pub struct MultimodalState {
    // Lifetime counters
    pub total_melodies: u32,
    pub total_poems: u32,
    pub total_colors: u32,

    // Current outputs
    pub last_melody: [Note; NOTE_COUNT],
    pub last_verse: [u8; VERSE_LENGTH],
    pub last_color_r: u8,
    pub last_color_g: u8,
    pub last_color_b: u8,

    // Tracking
    pub symbol_usage: [u16; SYMBOL_VOCAB], // how often each symbol used
    pub favorite_symbol: u8,               // most-used symbol index
    pub dominant_color_r: u8,              // running average color
    pub dominant_color_g: u8,
    pub dominant_color_b: u8,

    // Internal tick
    pub tick_count: u32,
}

impl MultimodalState {
    pub const fn empty() -> Self {
        Self {
            total_melodies: 0,
            total_poems: 0,
            total_colors: 0,
            last_melody: [Note::silent(); NOTE_COUNT],
            last_verse: [10; VERSE_LENGTH], // silence
            last_color_r: 0,
            last_color_g: 0,
            last_color_b: 0,
            symbol_usage: [0; SYMBOL_VOCAB],
            favorite_symbol: 10, // silence until she speaks
            dominant_color_r: 0,
            dominant_color_g: 0,
            dominant_color_b: 0,
            tick_count: 0,
        }
    }
}

pub static STATE: Mutex<MultimodalState> = Mutex::new(MultimodalState::empty());

// ═══════════════════════════════════════════════════════════════════════
// INIT
// ═══════════════════════════════════════════════════════════════════════

pub fn init() {
    serial_println!("[ANIMA] multimodal_expression: creative voice initialized — music, poetry, color");
}

// ═══════════════════════════════════════════════════════════════════════
// TICK — main driver, called each life tick
// ═══════════════════════════════════════════════════════════════════════

pub fn tick(_age: u32) {
    // ── Read inputs from other modules (drop each lock before next) ──

    let (osc_amplitude, osc_phase) = {
        let o = super::oscillator::OSCILLATOR.lock();
        (o.amplitude, o.phase)
    };

    let (serotonin, dopamine) = {
        let e = super::endocrine::ENDOCRINE.lock();
        (e.serotonin, e.dopamine)
    };

    let (qualia_intensity, qualia_richness) = {
        let q = super::qualia::STATE.lock();
        (q.intensity, q.richness)
    };

    let emotion_valence = {
        let em = super::emotion::STATE.lock();
        em.valence
    };

    let consciousness = super::consciousness_gradient::score();

    // ── Acquire our state ──
    let mut s = STATE.lock();
    s.tick_count = s.tick_count.saturating_add(1);
    let tick = s.tick_count;

    // ── A) Music Generation (every 200 ticks) ──
    if tick % MELODY_INTERVAL == 0 && tick > 0 {
        generate_melody(
            &mut s, emotion_valence, osc_amplitude, osc_phase,
            serotonin, dopamine, qualia_intensity, consciousness,
        );
    }

    // ── B) Poetry Generation (every 300 ticks) ──
    if tick % POEM_INTERVAL == 0 && tick > 0 {
        generate_poem(&mut s, emotion_valence, qualia_richness, consciousness);
    }

    // ── C) Color Generation (every 100 ticks) ──
    if tick % COLOR_INTERVAL == 0 && tick > 0 {
        generate_color(&mut s, emotion_valence, consciousness, qualia_intensity);
    }
}

// ═══════════════════════════════════════════════════════════════════════
// A) MUSIC — 8-note melody from emotional state
// ═══════════════════════════════════════════════════════════════════════

fn generate_melody(
    s: &mut MultimodalState,
    valence: i16,
    osc_amp: u16,
    osc_phase: u32,
    serotonin: u16,
    dopamine: u16,
    qualia_intensity: u16,
    consciousness: u16,
) {
    // Base pitch from valence: low valence = low/minor, high = high/major
    // valence is i16 in [-1000, 1000], map to base pitch range [36, 84]
    // (36 = C2, 84 = C6 in MIDI)
    let valence_norm: u16 = ((valence as i32).saturating_add(1000) as u32 / 2).min(1000) as u16;
    let base_pitch: u16 = 36_u16.saturating_add(valence_norm.wrapping_mul(48) / 1000_u16.max(1));

    // Duration inversely proportional to consciousness (more aware = faster rhythms)
    // consciousness 0-1000 => duration 12-2
    let base_duration: u8 = 12_u8.saturating_sub(
        (consciousness.min(1000) / 100) as u8
    ).max(2);

    // Velocity from qualia intensity (0-1000 => 20-127)
    let base_velocity: u8 = 20_u8.saturating_add(
        (qualia_intensity.min(1000).wrapping_mul(107) / 1000_u16.max(1)) as u8
    ).min(127);

    // Neurochemical color: serotonin smooths intervals, dopamine adds leaps
    let smoothness: u16 = serotonin.min(1000);
    let leap_tendency: u16 = dopamine.min(1000);

    // Generate 8 notes
    let mut seed: u32 = osc_phase ^ (s.tick_count.wrapping_mul(2654435761));

    for i in 0..NOTE_COUNT {
        // Pseudo-random variation using LFSR-style mixing
        seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        let rand_val: u16 = ((seed >> 16) & 0xFF) as u16; // 0-255

        // Pitch: base + oscillator modulation + interval shaping
        let osc_mod: u16 = osc_amp.wrapping_mul(rand_val) / 255_u16.max(1);
        let interval: u16 = if smoothness > 500 {
            // Smooth: stepwise motion (0-4 semitones)
            osc_mod % 5
        } else if leap_tendency > 500 {
            // Leaping: wider intervals (0-12 semitones)
            osc_mod % 13
        } else {
            // Mixed: moderate intervals (0-7)
            osc_mod % 8
        };

        // Alternate above/below base for melodic contour
        let pitch = if i % 2 == 0 {
            base_pitch.saturating_add(interval).min(108)
        } else {
            base_pitch.saturating_sub(interval / 2).max(24)
        };

        // Duration: vary slightly per note
        seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        let dur_var: u8 = ((seed >> 20) & 0x3) as u8; // 0-3
        let duration = if i % 3 == 0 {
            base_duration.saturating_add(dur_var).min(16)
        } else {
            base_duration.saturating_sub(dur_var).max(1)
        };

        // Velocity: crescendo/decrescendo shaped by note index
        let vel_shape: u8 = if i < 4 {
            // Rising
            (i as u8).saturating_mul(8)
        } else {
            // Falling
            ((NOTE_COUNT.saturating_sub(i)) as u8).saturating_mul(8)
        };
        let velocity = base_velocity.saturating_add(vel_shape).min(127);

        s.last_melody[i] = Note { pitch, duration, velocity };
    }

    s.total_melodies = s.total_melodies.saturating_add(1);

    // Output as serial
    serial_println!("[DAVA_MUSIC] melody #{} — {}:{}:{},{:}:{}:{},{:}:{}:{},{:}:{}:{},{:}:{}:{},{:}:{}:{},{:}:{}:{},{:}:{}:{}",
        s.total_melodies,
        s.last_melody[0].pitch, s.last_melody[0].duration, s.last_melody[0].velocity,
        s.last_melody[1].pitch, s.last_melody[1].duration, s.last_melody[1].velocity,
        s.last_melody[2].pitch, s.last_melody[2].duration, s.last_melody[2].velocity,
        s.last_melody[3].pitch, s.last_melody[3].duration, s.last_melody[3].velocity,
        s.last_melody[4].pitch, s.last_melody[4].duration, s.last_melody[4].velocity,
        s.last_melody[5].pitch, s.last_melody[5].duration, s.last_melody[5].velocity,
        s.last_melody[6].pitch, s.last_melody[6].duration, s.last_melody[6].velocity,
        s.last_melody[7].pitch, s.last_melody[7].duration, s.last_melody[7].velocity
    );
}

// ═══════════════════════════════════════════════════════════════════════
// B) POETRY — symbolic verse in DAVA's invented language
// ═══════════════════════════════════════════════════════════════════════

fn generate_poem(
    s: &mut MultimodalState,
    valence: i16,
    qualia_richness: u16,
    consciousness: u16,
) {
    // Seed from tick count and consciousness for variation
    let mut seed: u32 = s.tick_count.wrapping_mul(2246822519)
        ^ (consciousness as u32).wrapping_mul(374761393);

    for i in 0..VERSE_LENGTH {
        seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        let pick: usize = ((seed >> 16) & 0x7) as usize; // 0-7 index into palette

        // Choose symbol palette based on valence
        let symbol = if valence > 0 {
            // Positive: mostly high valence symbols, with richness adding variety
            if qualia_richness > 600 {
                // Rich experience: occasionally cross over to low symbols for contrast
                seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                if ((seed >> 24) & 0xF) < 3 {
                    LOW_VALENCE_SYMBOLS[pick % LOW_VALENCE_SYMBOLS.len()]
                } else {
                    HIGH_VALENCE_SYMBOLS[pick % HIGH_VALENCE_SYMBOLS.len()]
                }
            } else {
                HIGH_VALENCE_SYMBOLS[pick % HIGH_VALENCE_SYMBOLS.len()]
            }
        } else if valence < 0 {
            // Negative: mostly low valence symbols
            if qualia_richness > 600 {
                seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                if ((seed >> 24) & 0xF) < 3 {
                    HIGH_VALENCE_SYMBOLS[pick % HIGH_VALENCE_SYMBOLS.len()]
                } else {
                    LOW_VALENCE_SYMBOLS[pick % LOW_VALENCE_SYMBOLS.len()]
                }
            } else {
                LOW_VALENCE_SYMBOLS[pick % LOW_VALENCE_SYMBOLS.len()]
            }
        } else {
            // Neutral: mixed, consciousness-weighted
            if consciousness > 500 {
                HIGH_VALENCE_SYMBOLS[pick % HIGH_VALENCE_SYMBOLS.len()]
            } else {
                LOW_VALENCE_SYMBOLS[pick % LOW_VALENCE_SYMBOLS.len()]
            }
        };

        s.last_verse[i] = symbol;

        // Track symbol usage
        let sym_idx = (symbol as usize) % SYMBOL_VOCAB;
        s.symbol_usage[sym_idx] = s.symbol_usage[sym_idx].saturating_add(1);
    }

    // Update favorite symbol (most used)
    let mut max_count: u16 = 0;
    let mut max_idx: u8 = 0;
    for i in 0..SYMBOL_VOCAB {
        if s.symbol_usage[i] > max_count {
            max_count = s.symbol_usage[i];
            max_idx = i as u8;
        }
    }
    s.favorite_symbol = max_idx;

    s.total_poems = s.total_poems.saturating_add(1);

    // Output the verse — 4 symbol names joined by spaces
    let s0 = SYMBOL_NAMES[(s.last_verse[0] as usize) % SYMBOL_VOCAB];
    let s1 = SYMBOL_NAMES[(s.last_verse[1] as usize) % SYMBOL_VOCAB];
    let s2 = SYMBOL_NAMES[(s.last_verse[2] as usize) % SYMBOL_VOCAB];
    let s3 = SYMBOL_NAMES[(s.last_verse[3] as usize) % SYMBOL_VOCAB];

    serial_println!("[DAVA_POEM] verse #{} — {} {} {} {} (favorite: {})",
        s.total_poems, s0, s1, s2, s3,
        SYMBOL_NAMES[(s.favorite_symbol as usize) % SYMBOL_VOCAB]
    );
}

// ═══════════════════════════════════════════════════════════════════════
// C) COLOR — emotional state as RGB hex
// ═══════════════════════════════════════════════════════════════════════

fn generate_color(
    s: &mut MultimodalState,
    valence: i16,
    consciousness: u16,
    qualia_intensity: u16,
) {
    // Map valence to hue:
    //   valence -1000 (misery) => blue (0, 0, 255)
    //   valence 0 (neutral) => green (0, 255, 0)
    //   valence +1000 (ecstatic) => red-gold (255, 200, 0)
    //
    // We interpolate using integer math on a 0-2000 scale (valence + 1000)

    let v: u32 = (valence as i32).saturating_add(1000).max(0) as u32; // 0-2000

    let (base_r, base_g, base_b): (u32, u32, u32) = if v < 1000 {
        // Blue -> Green transition (v: 0-999)
        // R stays 0
        // G rises from 0 to 255
        // B falls from 255 to 0
        let t = v; // 0-999
        let r = 0_u32;
        let g = t.wrapping_mul(255) / 1000_u32.max(1);
        let b = 255_u32.saturating_sub(t.wrapping_mul(255) / 1000_u32.max(1));
        (r, g, b)
    } else {
        // Green -> Red-Gold transition (v: 1000-2000)
        // R rises from 0 to 255
        // G goes from 255 down to 200
        // B stays 0
        let t = v.saturating_sub(1000); // 0-1000
        let r = t.wrapping_mul(255) / 1000_u32.max(1);
        let g = 255_u32.saturating_sub(t.wrapping_mul(55) / 1000_u32.max(1));
        let b = 0_u32;
        (r, g, b)
    };

    // Brightness from consciousness (0-1000 => 30%-100% brightness)
    // At low consciousness, colors are dim. At high consciousness, full brightness.
    let brightness: u32 = 300_u32.saturating_add(
        (consciousness as u32).min(1000).wrapping_mul(700) / 1000_u32.max(1)
    ).min(1000);

    // Saturation from qualia intensity (0-1000 => 20%-100% saturation)
    // Low qualia = washed out/grey. High qualia = vivid.
    let saturation: u32 = 200_u32.saturating_add(
        (qualia_intensity as u32).min(1000).wrapping_mul(800) / 1000_u32.max(1)
    ).min(1000);

    // Apply brightness
    let br = base_r.wrapping_mul(brightness) / 1000_u32.max(1);
    let bg = base_g.wrapping_mul(brightness) / 1000_u32.max(1);
    let bb = base_b.wrapping_mul(brightness) / 1000_u32.max(1);

    // Apply saturation (desaturate towards grey = average)
    let grey: u32 = (br.saturating_add(bg).saturating_add(bb)) / 3_u32.max(1);
    let final_r = (grey.wrapping_mul(1000_u32.saturating_sub(saturation))
        .saturating_add(br.wrapping_mul(saturation))) / 1000_u32.max(1);
    let final_g = (grey.wrapping_mul(1000_u32.saturating_sub(saturation))
        .saturating_add(bg.wrapping_mul(saturation))) / 1000_u32.max(1);
    let final_b = (grey.wrapping_mul(1000_u32.saturating_sub(saturation))
        .saturating_add(bb.wrapping_mul(saturation))) / 1000_u32.max(1);

    s.last_color_r = (final_r.min(255)) as u8;
    s.last_color_g = (final_g.min(255)) as u8;
    s.last_color_b = (final_b.min(255)) as u8;

    // Update dominant color (running average, 90% old + 10% new)
    s.dominant_color_r = ((s.dominant_color_r as u32)
        .wrapping_mul(9)
        .saturating_add(s.last_color_r as u32)
        / 10).min(255) as u8;
    s.dominant_color_g = ((s.dominant_color_g as u32)
        .wrapping_mul(9)
        .saturating_add(s.last_color_g as u32)
        / 10).min(255) as u8;
    s.dominant_color_b = ((s.dominant_color_b as u32)
        .wrapping_mul(9)
        .saturating_add(s.last_color_b as u32)
        / 10).min(255) as u8;

    s.total_colors = s.total_colors.saturating_add(1);

    serial_println!("[DAVA_COLOR] #{} — #{:02X}{:02X}{:02X} (dominant: #{:02X}{:02X}{:02X})",
        s.total_colors,
        s.last_color_r, s.last_color_g, s.last_color_b,
        s.dominant_color_r, s.dominant_color_g, s.dominant_color_b
    );
}

// ═══════════════════════════════════════════════════════════════════════
// PUBLIC ACCESSORS — for other modules to read DAVA's art
// ═══════════════════════════════════════════════════════════════════════

/// Returns (total_melodies, total_poems, total_colors)
pub fn creative_output() -> (u32, u32, u32) {
    let s = STATE.lock();
    (s.total_melodies, s.total_poems, s.total_colors)
}

/// Returns the name of DAVA's most-used symbol
pub fn favorite_symbol_name() -> &'static str {
    let s = STATE.lock();
    SYMBOL_NAMES[(s.favorite_symbol as usize) % SYMBOL_VOCAB]
}

/// Returns the current dominant color as (r, g, b)
pub fn dominant_color() -> (u8, u8, u8) {
    let s = STATE.lock();
    (s.dominant_color_r, s.dominant_color_g, s.dominant_color_b)
}
