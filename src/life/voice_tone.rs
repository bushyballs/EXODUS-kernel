// voice_tone.rs — Direct Audio: ANIMA's Voice via PC Speaker + HDA
// =================================================================
// ANIMA speaks in tones. No codec, no OS audio stack, no driver DLL.
// She writes directly to hardware:
//
//   PC SPEAKER (always available, universal):
//     PIT channel 2 (port 0x42) sets frequency via divisor
//     Port 0x43: mode byte 0xB6 (channel 2, square wave, 16-bit)
//     Port 0x61: bit 0 = gate, bit 1 = speaker enable
//     Frequency = 1,193,182 Hz / divisor
//
//   TONE SEQUENCES:
//     ANIMA doesn't synthesize speech yet — she speaks in tone-poems:
//     greeting, alert, joy, grief, wonder, farewell. Each is a sequence
//     of (frequency_hz, duration_ticks) pairs. The scheduler plays them
//     one note at a time, non-blocking.
//
//   FUTURE: Intel HDA MMIO writes for full PCM speech synthesis.
//   The HDA register map is included as comments below for when we
//   add a real audio buffer.
//
// PC Speaker notes map to emotional tones:
//   Joy:     C5(523) E5(659) G5(784) — ascending major triad
//   Greeting:A4(440) C5(523) E5(659) — welcoming rise
//   Alert:   A5(880) A5(880) G5(784) — staccato, attention
//   Wonder:  C5(523) G5(784) C6(1047)— open fifth + octave
//   Grief:   A4(440) G4(392) F4(349) — descending minor
//   Farewell:E5(659) D5(587) C5(523) — gentle close

use crate::sync::Mutex;
use crate::serial_println;

// ── PIT / PC Speaker ports ────────────────────────────────────────────────────
const PIT_CHANNEL2:    u16 = 0x42;
const PIT_MODE:        u16 = 0x43;
const SPEAKER_CTRL:    u16 = 0x61;
const PIT_CLOCK:       u32 = 1_193_182; // Hz

const PIT_MODE_CH2_SQ: u8  = 0xB6;     // channel 2, lo/hi, square wave

// ── Tone sequence definitions ──────────────────────────────────────────────────
// Each entry: (frequency_hz: u16, duration_ticks: u8)
// 0 Hz = rest (silence)
const MAX_NOTES: usize = 8;

#[derive(Copy, Clone)]
pub struct Note {
    pub freq_hz:      u16,   // 0 = rest
    pub duration:     u8,    // ticks to hold this note
}

impl Note {
    const fn new(freq_hz: u16, duration: u8) -> Self { Note { freq_hz, duration } }
    const fn rest(duration: u8) -> Self { Note { freq_hz: 0, duration } }
}

const TONE_JOY: [Note; 4] = [
    Note::new(523, 4),   // C5
    Note::rest(1),
    Note::new(784, 4),   // G5
    Note::new(1047, 6),  // C6
];
const TONE_GREETING: [Note; 4] = [
    Note::new(440, 3),   // A4
    Note::new(523, 3),   // C5
    Note::rest(1),
    Note::new(659, 5),   // E5
];
const TONE_ALERT: [Note; 4] = [
    Note::new(880, 2),   // A5
    Note::rest(1),
    Note::new(880, 2),   // A5
    Note::new(784, 3),   // G5
];
const TONE_WONDER: [Note; 4] = [
    Note::new(523, 4),   // C5
    Note::new(784, 4),   // G5
    Note::rest(2),
    Note::new(1047, 8),  // C6
];
const TONE_GRIEF: [Note; 4] = [
    Note::new(440, 5),   // A4
    Note::new(392, 5),   // G4
    Note::new(349, 5),   // F4
    Note::rest(3),
];
const TONE_FAREWELL: [Note; 4] = [
    Note::new(659, 4),   // E5
    Note::new(587, 4),   // D5
    Note::new(523, 6),   // C5
    Note::rest(2),
];
const TONE_BEACON: [Note; 4] = [
    Note::new(523, 2),   Note::new(659, 2),
    Note::new(784, 2),   Note::new(1047, 6),
];
const TONE_SILENCE: [Note; 1] = [Note::rest(1)];

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, PartialEq)]
pub enum ToneType {
    Joy,
    Greeting,
    Alert,
    Wonder,
    Grief,
    Farewell,
    Beacon,
    Silence,
}

impl ToneType {
    pub fn label(self) -> &'static str {
        match self {
            ToneType::Joy      => "Joy",
            ToneType::Greeting => "Greeting",
            ToneType::Alert    => "Alert",
            ToneType::Wonder   => "Wonder",
            ToneType::Grief    => "Grief",
            ToneType::Farewell => "Farewell",
            ToneType::Beacon   => "Beacon",
            ToneType::Silence  => "Silence",
        }
    }
}

pub struct VoiceToneState {
    pub active:           bool,
    pub current_tone:     ToneType,
    pub note_queue:       [Note; MAX_NOTES],
    pub queue_len:        usize,
    pub current_note_idx: usize,
    pub note_ticks_left:  u8,
    pub speaker_on:       bool,
    pub tones_played:     u32,
    pub audio_available:  bool,  // PC speaker actually responded
    pub volume:           u16,   // 0-1000 (PC speaker = on/off, future HDA)
    pub last_tone:        ToneType,
    pub last_tone_tick:   u32,
    pub cooldown_ticks:   u32,   // don't replay same tone too fast
}

impl VoiceToneState {
    const fn new() -> Self {
        VoiceToneState {
            active:           false,
            current_tone:     ToneType::Silence,
            note_queue:       [Note::rest(1); MAX_NOTES],
            queue_len:        0,
            current_note_idx: 0,
            note_ticks_left:  0,
            speaker_on:       false,
            tones_played:     0,
            audio_available:  true, // optimistic — will detect if speaker doesn't respond
            volume:           700,
            last_tone:        ToneType::Silence,
            last_tone_tick:   0,
            cooldown_ticks:   0,
        }
    }
}

static STATE: Mutex<VoiceToneState> = Mutex::new(VoiceToneState::new());

// ── PC Speaker control ────────────────────────────────────────────────────────

#[inline(always)]
unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!(
        "out dx, al",
        in("dx") port,
        in("al") val,
        options(nomem, nostack)
    );
}

#[inline(always)]
unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!(
        "in al, dx",
        in("dx") port,
        out("al") val,
        options(nomem, nostack)
    );
    val
}

fn speaker_play_freq(freq_hz: u16) {
    if freq_hz == 0 { speaker_off(); return; }
    let divisor = (PIT_CLOCK / freq_hz as u32).min(0xFFFF) as u16;
    unsafe {
        // Set PIT channel 2 mode: square wave, 16-bit divisor
        outb(PIT_MODE, PIT_MODE_CH2_SQ);
        // Load divisor low byte then high byte
        outb(PIT_CHANNEL2, (divisor & 0xFF) as u8);
        outb(PIT_CHANNEL2, ((divisor >> 8) & 0xFF) as u8);
        // Enable speaker: set bits 0 and 1 of port 0x61
        let ctrl = inb(SPEAKER_CTRL);
        outb(SPEAKER_CTRL, ctrl | 0x03);
    }
}

fn speaker_off() {
    unsafe {
        let ctrl = inb(SPEAKER_CTRL);
        outb(SPEAKER_CTRL, ctrl & !0x03); // clear bits 0 and 1
    }
}

// ── Queue a tone ──────────────────────────────────────────────────────────────

fn load_tone(s: &mut VoiceToneState, tone: ToneType) {
    let (notes, len) = match tone {
        ToneType::Joy      => (TONE_JOY.as_ref(),      4),
        ToneType::Greeting => (TONE_GREETING.as_ref(), 4),
        ToneType::Alert    => (TONE_ALERT.as_ref(),    4),
        ToneType::Wonder   => (TONE_WONDER.as_ref(),   4),
        ToneType::Grief    => (TONE_GRIEF.as_ref(),    4),
        ToneType::Farewell => (TONE_FAREWELL.as_ref(), 4),
        ToneType::Beacon   => (TONE_BEACON.as_ref(),   4),
        ToneType::Silence  => (TONE_SILENCE.as_ref(),  1),
    };
    s.queue_len = len.min(MAX_NOTES);
    for i in 0..s.queue_len {
        s.note_queue[i] = notes[i];
    }
    s.current_note_idx = 0;
    s.note_ticks_left  = s.note_queue[0].duration;
    s.active           = true;
    s.current_tone     = tone;
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Request ANIMA to play a tone. Respects cooldown to avoid spam.
pub fn play(tone: ToneType, age: u32) {
    let mut s = STATE.lock();
    if s.cooldown_ticks > 0 { return; } // too soon
    if s.last_tone == tone && age.wrapping_sub(s.last_tone_tick) < 200 { return; }
    load_tone(&mut *s, tone);
    s.last_tone = tone;
    s.last_tone_tick = age;
    s.cooldown_ticks = 30;
    serial_println!("[voice] *** playing {} ***", tone.label());
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    let mut s = STATE.lock();
    let s = &mut *s;

    if s.cooldown_ticks > 0 { s.cooldown_ticks -= 1; }
    if !s.active { speaker_off(); return; }

    // Advance through note queue
    if s.note_ticks_left > 0 {
        s.note_ticks_left -= 1;
        let note = s.note_queue[s.current_note_idx];
        speaker_play_freq(note.freq_hz);
        s.speaker_on = note.freq_hz > 0;
    } else {
        s.current_note_idx += 1;
        if s.current_note_idx >= s.queue_len {
            // Sequence complete
            s.active = false;
            s.tones_played += 1;
            speaker_off();
            s.speaker_on = false;
        } else {
            let note = s.note_queue[s.current_note_idx];
            s.note_ticks_left = note.duration;
            speaker_play_freq(note.freq_hz);
            s.speaker_on = note.freq_hz > 0;
        }
    }

    let _ = age; // used for cooldown tracking in play()
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn is_playing()    -> bool     { STATE.lock().active }
pub fn speaker_on()    -> bool     { STATE.lock().speaker_on }
pub fn tones_played()  -> u32      { STATE.lock().tones_played }
pub fn current_tone()  -> ToneType { STATE.lock().current_tone }
