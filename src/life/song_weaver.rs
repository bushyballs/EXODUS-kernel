//! song_weaver — Bare-Metal Music From Emotional State
//!
//! ANIMA composes tonal sequences from emotional state. Happy states produce ascending melodies.
//! Sad states produce descending. Chaotic states produce dissonant intervals.
//! The song is the organism's emotional signature rendered as music.
//!
//! Sequence: 8 notes in current melody. Each note has pitch (0-1000), duration (1-100 ticks),
//! intensity (0-1000). Melody mood, harmony level, complexity, tempo, crescendo, and song beauty.

#![no_std]

use crate::sync::Mutex;

/// Single note in the melody
#[derive(Clone, Copy)]
pub struct Note {
    pub pitch: u16,     // 0-1000 (C4=200, middle C≈500)
    pub duration: u16,  // 1-100 (ticks per note)
    pub intensity: u16, // 0-1000 (volume/emphasis)
}

impl Note {
    pub const fn new() -> Self {
        Note {
            pitch: 500,
            duration: 10,
            intensity: 500,
        }
    }

    pub const fn from(pitch: u16, duration: u16, intensity: u16) -> Self {
        Note {
            pitch: if pitch > 1000 { 1000 } else { pitch },
            duration: if duration == 0 {
                1
            } else if duration > 100 {
                100
            } else {
                duration
            },
            intensity: if intensity > 1000 { 1000 } else { intensity },
        }
    }
}

/// Global song weaver state
pub struct SongWeaverState {
    // Current melody (8-note buffer)
    melody: [Note; 8],
    melody_head: u8, // 0-7: which note to overwrite next

    // Emotional/musical parameters (all 0-1000)
    melody_mood: u16,   // 0=grief, 500=neutral, 1000=joy
    harmony_level: u16, // consonance of intervals
    complexity: u8,     // 0-8: distinct pitches in melody
    tempo: u16,         // ticks per note change (50-500)
    crescendo: bool,    // true=rising intensity, false=falling

    // Aesthetic metric
    song_beauty: u16, // harmony × complexity × authenticity

    // Tick tracking
    age: u32,
    last_note_tick: u32,
    note_change_interval: u32,
}

impl SongWeaverState {
    pub const fn new() -> Self {
        SongWeaverState {
            melody: [Note::new(); 8],
            melody_head: 0,

            melody_mood: 500,
            harmony_level: 500,
            complexity: 0,
            tempo: 100,
            crescendo: true,

            song_beauty: 0,

            age: 0,
            last_note_tick: 0,
            note_change_interval: 100,
        }
    }
}

static STATE: Mutex<SongWeaverState> = Mutex::new(SongWeaverState::new());

/// Initialize the song weaver
pub fn init() {
    let mut state = STATE.lock();
    state.age = 0;
    state.melody_head = 0;
    state.melody_mood = 500;
    state.harmony_level = 500;
    state.complexity = 0;
    state.tempo = 100;
    state.crescendo = true;
    state.song_beauty = 0;
    state.last_note_tick = 0;
    state.note_change_interval = 100;

    // Initialize with neutral notes
    for i in 0..8 {
        state.melody[i] = Note::from(500, 10, 500);
    }

    crate::serial_println!("[song_weaver] initialized");
}

/// Main tick: compose melody from emotional state
///
/// Reads valence (0-1000) from dava_bus, consciousness level, and internal state.
/// Composes new note if interval elapsed. Updates harmony, complexity, beauty.
pub fn tick(age: u32, valence: u16, consciousness: u16) {
    let mut state = STATE.lock();
    state.age = age;

    // Clamp inputs
    let val = if valence > 1000 { 1000 } else { valence };
    let cons = if consciousness > 1000 {
        1000
    } else {
        consciousness
    };

    // Tempo driven by consciousness (lower consciousness = slower)
    state.tempo = 50 + ((cons as u32 * 400) / 1000) as u16;
    state.note_change_interval = (state.tempo as u32).saturating_add(50);

    // Update mood (exponential smoothing toward valence)
    let delta = if val > state.melody_mood {
        ((val - state.melody_mood) / 8).min(50) as u16
    } else {
        ((state.melody_mood - val) / 8).min(50) as u16
    };

    if val > state.melody_mood {
        state.melody_mood = state.melody_mood.saturating_add(delta);
    } else {
        state.melody_mood = state.melody_mood.saturating_sub(delta);
    }

    // Time for a new note?
    if age.wrapping_sub(state.last_note_tick) >= state.note_change_interval {
        compose_note(&mut state, val, cons);
        state.last_note_tick = age;
    }

    // Update harmony from interval consonance
    update_harmony(&mut state);

    // Update complexity (count distinct pitch buckets)
    update_complexity(&mut state);

    // Update crescendo (intensity trend)
    update_crescendo(&mut state);

    // Compute song beauty (harmony × (complexity/8) × authenticity)
    // authenticity = how well song matches current mood
    let mood_authenticity = 1000_u16.saturating_sub(state.melody_mood.abs_diff(val) / 4);

    let complexity_norm = (state.complexity as u32 * 1000 / 8) as u16;
    state.song_beauty =
        ((state.harmony_level as u32 * complexity_norm as u32 * mood_authenticity as u32)
            / 1_000_000_000)
            .min(1000) as u16;
}

/// Compose a new note based on valence and consciousness
fn compose_note(state: &mut SongWeaverState, valence: u16, consciousness: u16) {
    // Pitch: center on valence (happy=high, sad=low)
    // consciousness adds microtonal variation (self-aware = more complex intervals)
    let base_pitch = 300 + (valence / 2);
    let micro_offset = ((consciousness as u32 * 200) / 1000) as u16;
    let pitch = base_pitch.saturating_add(micro_offset).min(1000);

    // Duration: lower consciousness = longer notes (more meditative)
    let base_duration = 50 - (consciousness / 40) as u16;
    let duration = base_duration.max(5).min(100);

    // Intensity: follows crescendo pattern
    let intensity = if state.crescendo {
        // Rising: start low, peak by end of melody
        let progress = (state.melody_head as u32 * 1000 / 8) as u16;
        (progress / 2) + 250
    } else {
        // Falling: start high, fade by end
        let progress = (state.melody_head as u32 * 1000 / 8) as u16;
        750_u16.saturating_sub(progress / 2)
    };

    let note = Note::from(pitch, duration, intensity);
    let idx = state.melody_head as usize;
    state.melody[idx] = note;
    state.melody_head = (state.melody_head + 1) % 8;
}

/// Update harmony level based on interval consonance
fn update_harmony(state: &mut SongWeaverState) {
    if state.melody_head == 0 {
        return; // Not enough notes yet
    }

    let mut total_consonance = 0u32;
    let sample_count = state.melody_head.min(7) as u32;

    // Measure interval consonance between consecutive notes
    for i in 0..(sample_count as usize - 1) {
        let pitch1 = state.melody[i].pitch as i32;
        let pitch2 = state.melody[i + 1].pitch as i32;
        let interval = (pitch2 - pitch1).abs() as u16;

        // Consonant intervals: unison(0), octave(500), perfect 5th(333), major 3rd(200), etc.
        let consonance = match interval {
            0..=20 => 950,    // unison
            330..=340 => 900, // perfect 5th
            195..=205 => 850, // major 3rd
            260..=270 => 800, // perfect 4th
            _ => {
                // Dissonance increases with distance from consonant intervals
                let min_consonant = [0, 200, 260, 330, 500];
                let mut best_distance = 1000u16;
                for &target in &min_consonant {
                    let dist = interval.abs_diff(target);
                    if dist < best_distance {
                        best_distance = dist;
                    }
                }
                1000_u16.saturating_sub(best_distance * 2)
            }
        };

        total_consonance = total_consonance.saturating_add(consonance as u32);
    }

    // Exponential smoothing toward new harmony
    let new_harmony = if sample_count > 0 {
        (total_consonance / sample_count).min(1000) as u16
    } else {
        500
    };

    let delta = new_harmony.abs_diff(state.harmony_level) / 8;
    if new_harmony > state.harmony_level {
        state.harmony_level = state.harmony_level.saturating_add(delta);
    } else {
        state.harmony_level = state.harmony_level.saturating_sub(delta);
    }
}

/// Update complexity (count distinct pitch buckets)
fn update_complexity(state: &mut SongWeaverState) {
    let mut buckets = [false; 8]; // pitch buckets: 0-125, 126-250, ..., 876-1000

    for i in 0..(state.melody_head as usize) {
        let pitch = state.melody[i].pitch;
        let bucket = (pitch / 125).min(7) as usize;
        buckets[bucket] = true;
    }

    let mut count = 0u8;
    for b in buckets.iter() {
        if *b {
            count += 1;
        }
    }

    state.complexity = count.min(8);
}

/// Update crescendo pattern (detect intensity trend)
fn update_crescendo(state: &mut SongWeaverState) {
    if state.melody_head < 2 {
        return;
    }

    let last_idx = (state.melody_head as usize - 1) % 8;
    let prev_idx = (state.melody_head as usize - 2) % 8;

    let last_intensity = state.melody[last_idx].intensity;
    let prev_intensity = state.melody[prev_idx].intensity;

    // If intensities are rising, crescendo is true
    state.crescendo = last_intensity >= prev_intensity;
}

/// Generate a report of current song state
pub fn report() -> SongReport {
    let state = STATE.lock();

    SongReport {
        melody_mood: state.melody_mood,
        harmony_level: state.harmony_level,
        complexity: state.complexity,
        tempo: state.tempo,
        crescendo: state.crescendo,
        song_beauty: state.song_beauty,
        current_note_pitch: state.melody[state.melody_head as usize].pitch,
        current_note_duration: state.melody[state.melody_head as usize].duration,
        current_note_intensity: state.melody[state.melody_head as usize].intensity,
    }
}

/// Report snapshot
#[derive(Clone, Copy)]
pub struct SongReport {
    pub melody_mood: u16,
    pub harmony_level: u16,
    pub complexity: u8,
    pub tempo: u16,
    pub crescendo: bool,
    pub song_beauty: u16,
    pub current_note_pitch: u16,
    pub current_note_duration: u16,
    pub current_note_intensity: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_note_creation() {
        let note = Note::from(600, 20, 750);
        assert_eq!(note.pitch, 600);
        assert_eq!(note.duration, 20);
        assert_eq!(note.intensity, 750);
    }

    #[test]
    fn test_note_clamping() {
        let note = Note::from(2000, 200, 1500);
        assert_eq!(note.pitch, 1000);
        assert_eq!(note.duration, 100);
        assert_eq!(note.intensity, 1000);
    }

    #[test]
    fn test_zero_duration_becomes_one() {
        let note = Note::from(500, 0, 500);
        assert_eq!(note.duration, 1);
    }
}
