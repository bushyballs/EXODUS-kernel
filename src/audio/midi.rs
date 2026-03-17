use crate::sync::Mutex;
/// MIDI engine — parser, sequencer, synthesis, instrument bank, recording
///
/// Full MIDI 1.0 message parser, a step sequencer with tempo control,
/// wavetable-style synthesis using Q16 fixed-point math, instrument bank
/// management, and real-time MIDI recording/playback.
///
/// Inspired by: General MIDI, FluidSynth, TiMidity. All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Q16 fixed-point: 16 fractional bits
const Q16_ONE: i32 = 65536;

/// Maximum polyphony (simultaneous notes)
const MAX_POLYPHONY: usize = 32;

/// Maximum tracks in sequencer
const MAX_TRACKS: usize = 16;

/// Maximum events in a recording
const MAX_RECORD_EVENTS: usize = 8192;

/// Samples per second for synthesis
const SYNTH_SAMPLE_RATE: u32 = 44100;

// ---------------------------------------------------------------------------
// MIDI message types
// ---------------------------------------------------------------------------

/// MIDI status byte categories
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MidiStatus {
    NoteOff,
    NoteOn,
    PolyAftertouch,
    ControlChange,
    ProgramChange,
    ChannelAftertouch,
    PitchBend,
    SystemExclusive,
    SystemCommon,
    SystemRealtime,
}

/// Parsed MIDI message
#[derive(Debug, Clone, Copy)]
pub struct MidiMessage {
    pub status: MidiStatus,
    pub channel: u8,
    pub data1: u8,      // note number or controller number
    pub data2: u8,      // velocity or controller value
    pub timestamp: u64, // in ticks
}

impl MidiMessage {
    pub const fn empty() -> Self {
        MidiMessage {
            status: MidiStatus::NoteOff,
            channel: 0,
            data1: 0,
            data2: 0,
            timestamp: 0,
        }
    }
}

/// MIDI parser state machine
pub struct MidiParser {
    running_status: u8,
    buffer: [u8; 3],
    buf_pos: usize,
    expected_len: usize,
}

impl MidiParser {
    const fn new() -> Self {
        MidiParser {
            running_status: 0,
            buffer: [0; 3],
            buf_pos: 0,
            expected_len: 0,
        }
    }

    /// Feed a byte, returns Some(MidiMessage) when a complete message is parsed
    pub fn feed(&mut self, byte: u8, tick: u64) -> Option<MidiMessage> {
        if byte & 0x80 != 0 {
            // Status byte
            if byte >= 0xF8 {
                // System realtime — single byte, don't affect running status
                return Some(MidiMessage {
                    status: MidiStatus::SystemRealtime,
                    channel: 0,
                    data1: byte,
                    data2: 0,
                    timestamp: tick,
                });
            }

            self.running_status = byte;
            self.buf_pos = 0;
            self.expected_len = match byte & 0xF0 {
                0xC0 | 0xD0 => 1, // Program Change, Channel Aftertouch: 1 data byte
                0xF0 => 0,        // SysEx (variable length, simplified)
                _ => 2,           // Most messages: 2 data bytes
            };
            return None;
        }

        // Data byte
        if self.running_status == 0 {
            return None; // No status yet
        }

        self.buffer[self.buf_pos] = byte;
        self.buf_pos += 1;

        if self.buf_pos >= self.expected_len {
            let msg = self.build_message(tick);
            self.buf_pos = 0;
            return msg;
        }

        None
    }

    fn build_message(&self, tick: u64) -> Option<MidiMessage> {
        let channel = self.running_status & 0x0F;
        let status = match self.running_status & 0xF0 {
            0x80 => MidiStatus::NoteOff,
            0x90 => {
                // NoteOn with velocity 0 = NoteOff
                if self.buffer[1] == 0 {
                    MidiStatus::NoteOff
                } else {
                    MidiStatus::NoteOn
                }
            }
            0xA0 => MidiStatus::PolyAftertouch,
            0xB0 => MidiStatus::ControlChange,
            0xC0 => MidiStatus::ProgramChange,
            0xD0 => MidiStatus::ChannelAftertouch,
            0xE0 => MidiStatus::PitchBend,
            _ => return None,
        };

        Some(MidiMessage {
            status,
            channel,
            data1: self.buffer[0],
            data2: if self.expected_len > 1 {
                self.buffer[1]
            } else {
                0
            },
            timestamp: tick,
        })
    }
}

// ---------------------------------------------------------------------------
// Synthesis — wavetable oscillator using Q16 phase accumulator
// ---------------------------------------------------------------------------

/// Waveform type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waveform {
    Sine,
    Square,
    Sawtooth,
    Triangle,
    Noise,
}

/// ADSR envelope state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnvelopePhase {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// ADSR envelope generator (all values in Q16)
#[derive(Debug, Clone, Copy)]
pub struct Envelope {
    pub attack_q16: i32, // rate per sample
    pub decay_q16: i32,
    pub sustain_q16: i32, // sustain level
    pub release_q16: i32,
    phase: EnvelopePhase,
    level_q16: i32,
}

impl Envelope {
    const fn new() -> Self {
        Envelope {
            attack_q16: Q16_ONE / 500, // ~11ms at 44100
            decay_q16: Q16_ONE / 2000,
            sustain_q16: Q16_ONE * 3 / 4,
            release_q16: Q16_ONE / 4000,
            phase: EnvelopePhase::Idle,
            level_q16: 0,
        }
    }

    fn trigger(&mut self) {
        self.phase = EnvelopePhase::Attack;
        self.level_q16 = 0;
    }

    fn release(&mut self) {
        self.phase = EnvelopePhase::Release;
    }

    fn tick(&mut self) -> i32 {
        match self.phase {
            EnvelopePhase::Idle => 0,
            EnvelopePhase::Attack => {
                self.level_q16 += self.attack_q16;
                if self.level_q16 >= Q16_ONE {
                    self.level_q16 = Q16_ONE;
                    self.phase = EnvelopePhase::Decay;
                }
                self.level_q16
            }
            EnvelopePhase::Decay => {
                self.level_q16 -= self.decay_q16;
                if self.level_q16 <= self.sustain_q16 {
                    self.level_q16 = self.sustain_q16;
                    self.phase = EnvelopePhase::Sustain;
                }
                self.level_q16
            }
            EnvelopePhase::Sustain => self.level_q16,
            EnvelopePhase::Release => {
                self.level_q16 -= self.release_q16;
                if self.level_q16 <= 0 {
                    self.level_q16 = 0;
                    self.phase = EnvelopePhase::Idle;
                }
                self.level_q16
            }
        }
    }

    fn is_active(&self) -> bool {
        self.phase != EnvelopePhase::Idle
    }
}

/// A single synthesizer voice
#[derive(Clone, Copy)]
pub struct SynthVoice {
    pub note: u8,
    pub velocity_q16: i32,
    pub waveform: Waveform,
    pub phase_q16: i32,     // phase accumulator (Q16)
    pub phase_inc_q16: i32, // phase increment per sample (Q16)
    pub envelope: Envelope,
    pub active: bool,
    pub channel: u8,
}

impl SynthVoice {
    const fn empty() -> Self {
        SynthVoice {
            note: 0,
            velocity_q16: 0,
            waveform: Waveform::Sine,
            phase_q16: 0,
            phase_inc_q16: 0,
            envelope: Envelope::new(),
            active: false,
            channel: 0,
        }
    }

    /// Compute phase increment for a MIDI note using Q16 fixed-point
    /// freq = 440 * 2^((note-69)/12)  approximated via integer lookup
    fn note_to_phase_inc(note: u8) -> i32 {
        // Pre-computed frequency table (Q16) for octave C4-B4 (notes 60-71)
        // freq * 65536 / 44100
        const BASE_TABLE: [i32; 12] = [389, 412, 437, 463, 490, 519, 550, 583, 617, 654, 693, 734];

        let octave_offset = (note as i32 - 60) / 12;
        let semitone = ((note as i32 - 60) % 12 + 12) % 12;
        let base = BASE_TABLE[semitone as usize];

        if octave_offset >= 0 {
            base << (octave_offset as u32)
        } else {
            base >> ((-octave_offset) as u32)
        }
    }

    /// Generate one sample from this voice
    fn generate(&mut self) -> i32 {
        if !self.active {
            return 0;
        }

        let env = self.envelope.tick();
        if !self.envelope.is_active() {
            self.active = false;
            return 0;
        }

        // Oscillator output based on phase (Q16, wraps at Q16_ONE)
        let raw = match self.waveform {
            Waveform::Sine => {
                // Approximate sine using parabola: 4*x*(1-x) for x in [0,1]
                let x = self.phase_q16;
                let half = Q16_ONE / 2;
                let normalized = if x < half {
                    // Rising half: map [0, half] to [0, Q16_ONE]
                    (((x as i64) << 17) / Q16_ONE as i64) as i32
                } else {
                    // Falling half
                    let t = (((x - half) as i64) << 17) / Q16_ONE as i64;
                    (2 * Q16_ONE) - t as i32
                };
                // Parabola: 4 * n * (Q16_ONE - n) / Q16_ONE - Q16_ONE
                let n = normalized;
                ((4i64 * n as i64 * (Q16_ONE - n) as i64) >> 16) as i32 - Q16_ONE
            }
            Waveform::Square => {
                if self.phase_q16 < Q16_ONE / 2 {
                    Q16_ONE
                } else {
                    -Q16_ONE
                }
            }
            Waveform::Sawtooth => {
                // Linear ramp from -Q16_ONE to Q16_ONE
                (self.phase_q16 * 2) - Q16_ONE
            }
            Waveform::Triangle => {
                let half = Q16_ONE / 2;
                if self.phase_q16 < half {
                    (self.phase_q16 * 4) - Q16_ONE
                } else {
                    Q16_ONE - ((self.phase_q16 - half) * 4)
                }
            }
            Waveform::Noise => {
                // Simple LCG pseudo-random
                let seed = self.phase_q16.wrapping_mul(1103515245).wrapping_add(12345);
                self.phase_q16 = seed & 0x7FFFFFFF;
                (seed >> 8) & 0xFFFF
            }
        };

        // Advance phase
        if self.waveform != Waveform::Noise {
            self.phase_q16 = (self.phase_q16 + self.phase_inc_q16) % Q16_ONE;
        }

        // Apply envelope and velocity
        let scaled = (((raw as i64) * (env as i64)) >> 16) as i32;
        (((scaled as i64) * (self.velocity_q16 as i64)) >> 16) as i32
    }
}

// ---------------------------------------------------------------------------
// Instrument bank
// ---------------------------------------------------------------------------

/// Instrument definition (synthesis parameters)
#[derive(Debug, Clone)]
pub struct Instrument {
    pub id: u8,
    pub name: String,
    pub waveform: Waveform,
    pub envelope: Envelope,
    pub detune_q16: i32, // slight detuning for richness
    pub volume_q16: i32,
}

/// Instrument bank (General MIDI-like)
pub struct InstrumentBank {
    pub instruments: Vec<Instrument>,
    pub channel_program: [u8; 16], // program number per MIDI channel
}

impl InstrumentBank {
    fn new() -> Self {
        let mut bank = InstrumentBank {
            instruments: Vec::new(),
            channel_program: [0; 16],
        };
        bank.load_defaults();
        bank
    }

    fn load_defaults(&mut self) {
        // Basic General MIDI instrument approximations
        let defaults = [
            (
                0,
                "Acoustic Piano",
                Waveform::Triangle,
                Q16_ONE / 200,
                Q16_ONE / 1000,
                Q16_ONE * 7 / 10,
                Q16_ONE / 3000,
            ),
            (
                25,
                "Steel Guitar",
                Waveform::Sawtooth,
                Q16_ONE / 300,
                Q16_ONE / 500,
                Q16_ONE / 2,
                Q16_ONE / 2000,
            ),
            (
                33,
                "Electric Bass",
                Waveform::Square,
                Q16_ONE / 100,
                Q16_ONE / 800,
                Q16_ONE * 6 / 10,
                Q16_ONE / 4000,
            ),
            (
                40,
                "Violin",
                Waveform::Sawtooth,
                Q16_ONE / 1000,
                Q16_ONE / 3000,
                Q16_ONE * 8 / 10,
                Q16_ONE / 2000,
            ),
            (
                56,
                "Trumpet",
                Waveform::Square,
                Q16_ONE / 400,
                Q16_ONE / 2000,
                Q16_ONE * 9 / 10,
                Q16_ONE / 1500,
            ),
            (
                73,
                "Flute",
                Waveform::Sine,
                Q16_ONE / 600,
                Q16_ONE / 2000,
                Q16_ONE * 7 / 10,
                Q16_ONE / 2000,
            ),
            (
                80,
                "Synth Lead",
                Waveform::Sawtooth,
                Q16_ONE / 100,
                Q16_ONE / 500,
                Q16_ONE * 8 / 10,
                Q16_ONE / 1000,
            ),
            (
                81,
                "Synth Pad",
                Waveform::Triangle,
                Q16_ONE / 4000,
                Q16_ONE / 2000,
                Q16_ONE * 6 / 10,
                Q16_ONE / 6000,
            ),
        ];

        for &(id, name, waveform, attack, decay, sustain, release) in &defaults {
            self.instruments.push(Instrument {
                id,
                name: String::from(name),
                waveform,
                envelope: Envelope {
                    attack_q16: attack,
                    decay_q16: decay,
                    sustain_q16: sustain,
                    release_q16: release,
                    phase: EnvelopePhase::Idle,
                    level_q16: 0,
                },
                detune_q16: 0,
                volume_q16: Q16_ONE,
            });
        }
    }

    /// Get instrument by program number
    pub fn get_instrument(&self, program: u8) -> Option<&Instrument> {
        self.instruments.iter().find(|i| i.id == program)
    }
}

// ---------------------------------------------------------------------------
// Sequencer
// ---------------------------------------------------------------------------

/// Sequencer event
#[derive(Debug, Clone, Copy)]
pub struct SeqEvent {
    pub tick: u64,
    pub message: MidiMessage,
}

/// Sequencer track
pub struct SeqTrack {
    pub events: Vec<SeqEvent>,
    pub name: String,
    pub muted: bool,
    pub channel: u8,
}

/// MIDI sequencer
pub struct Sequencer {
    pub tracks: Vec<SeqTrack>,
    pub tempo_bpm: u32,
    pub ticks_per_beat: u32,
    pub playing: bool,
    pub current_tick: u64,
    pub loop_enabled: bool,
    pub loop_start: u64,
    pub loop_end: u64,
    tick_accumulator_q16: i32,
}

impl Sequencer {
    fn new() -> Self {
        Sequencer {
            tracks: Vec::new(),
            tempo_bpm: 120,
            ticks_per_beat: 480,
            playing: false,
            current_tick: 0,
            loop_enabled: false,
            loop_start: 0,
            loop_end: 0,
            tick_accumulator_q16: 0,
        }
    }

    /// Advance sequencer by one audio sample, return events that fire
    pub fn advance_sample(&mut self, sample_rate: u32) -> Vec<MidiMessage> {
        if !self.playing {
            return Vec::new();
        }

        // ticks per sample = (tempo_bpm * ticks_per_beat) / (60 * sample_rate)
        // In Q16: ((bpm * tpb) << 16) / (60 * sr)
        let ticks_per_sample_q16 = (((self.tempo_bpm as i64 * self.ticks_per_beat as i64) << 16)
            / (60 * sample_rate) as i64) as i32;

        self.tick_accumulator_q16 += ticks_per_sample_q16;

        let mut fired = Vec::new();

        while self.tick_accumulator_q16 >= Q16_ONE {
            self.tick_accumulator_q16 -= Q16_ONE;
            self.current_tick = self.current_tick.saturating_add(1);

            // Check for loop
            if self.loop_enabled && self.current_tick >= self.loop_end {
                self.current_tick = self.loop_start;
            }

            // Collect events at this tick
            for track in self.tracks.iter() {
                if track.muted {
                    continue;
                }
                for event in track.events.iter() {
                    if event.tick == self.current_tick {
                        fired.push(event.message);
                    }
                }
            }
        }

        fired
    }

    /// Add an event to a track
    pub fn add_event(&mut self, track_idx: usize, tick: u64, message: MidiMessage) {
        if track_idx < self.tracks.len() {
            self.tracks[track_idx]
                .events
                .push(SeqEvent { tick, message });
        }
    }

    /// Create a new track
    pub fn add_track(&mut self, name: String, channel: u8) -> usize {
        if self.tracks.len() >= MAX_TRACKS {
            return self.tracks.len() - 1;
        }
        self.tracks.push(SeqTrack {
            events: Vec::new(),
            name,
            muted: false,
            channel,
        });
        self.tracks.len() - 1
    }
}

// ---------------------------------------------------------------------------
// MIDI recorder
// ---------------------------------------------------------------------------

/// Real-time MIDI recorder
pub struct MidiRecorder {
    pub recording: bool,
    pub events: Vec<SeqEvent>,
    pub start_tick: u64,
}

impl MidiRecorder {
    const fn new() -> Self {
        MidiRecorder {
            recording: false,
            events: Vec::new(),
            start_tick: 0,
        }
    }

    pub fn start(&mut self, current_tick: u64) {
        self.recording = true;
        self.events.clear();
        self.start_tick = current_tick;
    }

    pub fn stop(&mut self) {
        self.recording = false;
    }

    pub fn record_event(&mut self, message: MidiMessage, current_tick: u64) {
        if self.recording && self.events.len() < MAX_RECORD_EVENTS {
            self.events.push(SeqEvent {
                tick: current_tick - self.start_tick,
                message,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// MIDI engine (top-level)
// ---------------------------------------------------------------------------

/// Complete MIDI engine
pub struct MidiEngine {
    pub parser: MidiParser,
    pub voices: [SynthVoice; MAX_POLYPHONY],
    pub bank: InstrumentBank,
    pub sequencer: Sequencer,
    pub recorder: MidiRecorder,
    pub master_volume_q16: i32,
    pub pitch_bend: [i32; 16], // per-channel pitch bend (Q16, center = 0)
    pub notes_played: u64,
}

impl MidiEngine {
    fn new() -> Self {
        MidiEngine {
            parser: MidiParser::new(),
            voices: [SynthVoice::empty(); MAX_POLYPHONY],
            bank: InstrumentBank::new(),
            sequencer: Sequencer::new(),
            recorder: MidiRecorder::new(),
            master_volume_q16: Q16_ONE,
            pitch_bend: [0; 16],
            notes_played: 0,
        }
    }

    /// Handle a parsed MIDI message
    pub fn handle_message(&mut self, msg: MidiMessage) {
        match msg.status {
            MidiStatus::NoteOn => self.note_on(msg.channel, msg.data1, msg.data2),
            MidiStatus::NoteOff => self.note_off(msg.channel, msg.data1),
            MidiStatus::ProgramChange => {
                self.bank.channel_program[msg.channel as usize & 0x0F] = msg.data1;
            }
            MidiStatus::PitchBend => {
                let bend = ((msg.data2 as i32) << 7 | msg.data1 as i32) - 8192;
                self.pitch_bend[msg.channel as usize & 0x0F] =
                    (((bend as i64) << 16) / 8192) as i32;
            }
            MidiStatus::ControlChange => {
                // CC#7 = volume, CC#123 = all notes off
                if msg.data1 == 123 {
                    self.all_notes_off(msg.channel);
                }
            }
            _ => {}
        }

        // Record if active
        if self.recorder.recording {
            self.recorder.record_event(msg, self.sequencer.current_tick);
        }
    }

    /// Trigger a note
    fn note_on(&mut self, channel: u8, note: u8, velocity: u8) {
        if velocity == 0 {
            self.note_off(channel, note);
            return;
        }

        // Find a free voice (or steal the oldest)
        let voice_idx = self.find_free_voice();
        let program = self.bank.channel_program[channel as usize & 0x0F];
        let instrument = self.bank.get_instrument(program);

        let voice = &mut self.voices[voice_idx];
        voice.note = note;
        voice.velocity_q16 = (velocity as i32) * (Q16_ONE / 127);
        voice.channel = channel;
        voice.phase_q16 = 0;
        voice.phase_inc_q16 = SynthVoice::note_to_phase_inc(note);
        voice.active = true;

        if let Some(inst) = instrument {
            voice.waveform = inst.waveform;
            voice.envelope = inst.envelope;
        } else {
            voice.waveform = Waveform::Triangle;
            voice.envelope = Envelope::new();
        }
        voice.envelope.trigger();

        self.notes_played = self.notes_played.saturating_add(1);
    }

    /// Release a note
    fn note_off(&mut self, channel: u8, note: u8) {
        for voice in self.voices.iter_mut() {
            if voice.active && voice.note == note && voice.channel == channel {
                voice.envelope.release();
            }
        }
    }

    /// Stop all notes on a channel
    fn all_notes_off(&mut self, channel: u8) {
        for voice in self.voices.iter_mut() {
            if voice.active && voice.channel == channel {
                voice.envelope.release();
            }
        }
    }

    /// Find a free voice slot, or steal the quietest
    fn find_free_voice(&self) -> usize {
        // First pass: find idle voice
        for (i, voice) in self.voices.iter().enumerate() {
            if !voice.active {
                return i;
            }
        }
        // Second pass: find voice in release phase with lowest envelope
        let mut quietest = 0;
        let mut quietest_level = i32::MAX;
        for (i, voice) in self.voices.iter().enumerate() {
            if voice.envelope.level_q16 < quietest_level {
                quietest_level = voice.envelope.level_q16;
                quietest = i;
            }
        }
        quietest
    }

    /// Render audio samples (mono, i16)
    pub fn render(&mut self, output: &mut [i16]) {
        for sample in output.iter_mut() {
            // Advance sequencer and handle events
            let events = self.sequencer.advance_sample(SYNTH_SAMPLE_RATE);
            for event in events {
                self.handle_message(event);
            }

            // Mix all active voices
            let mut mix: i64 = 0;
            let mut active_count: i32 = 0;
            for voice in self.voices.iter_mut() {
                if voice.active {
                    mix += voice.generate() as i64;
                    active_count += 1;
                }
            }

            // Normalize by active voice count to prevent clipping
            if active_count > 1 {
                mix = mix / active_count as i64;
            }

            // Apply master volume
            let out = (((mix * self.master_volume_q16 as i64) >> 16) >> 8) as i32;

            *sample = if out > 32767 {
                32767
            } else if out < -32768 {
                -32768
            } else {
                out as i16
            };
        }
    }
}

static MIDI_ENGINE: Mutex<Option<MidiEngine>> = Mutex::new(None);

pub fn init() {
    let mut engine = MidiEngine::new();
    // Create a default track
    engine.sequencer.add_track(String::from("Track 1"), 0);
    *MIDI_ENGINE.lock() = Some(engine);
    serial_println!("    [midi] MIDI parser, sequencer, synth, instrument bank, recorder");
}

/// Feed raw MIDI byte
pub fn feed_byte(byte: u8, tick: u64) {
    if let Some(ref mut engine) = *MIDI_ENGINE.lock() {
        if let Some(msg) = engine.parser.feed(byte, tick) {
            engine.handle_message(msg);
        }
    }
}

/// Render synthesized audio
pub fn render(output: &mut [i16]) {
    if let Some(ref mut engine) = *MIDI_ENGINE.lock() {
        engine.render(output);
    }
}

/// Start sequencer playback
pub fn play() {
    if let Some(ref mut engine) = *MIDI_ENGINE.lock() {
        engine.sequencer.playing = true;
    }
}

/// Stop sequencer
pub fn stop() {
    if let Some(ref mut engine) = *MIDI_ENGINE.lock() {
        engine.sequencer.playing = false;
    }
}

/// Set tempo in BPM
pub fn set_tempo(bpm: u32) {
    if let Some(ref mut engine) = *MIDI_ENGINE.lock() {
        engine.sequencer.tempo_bpm = bpm;
    }
}

/// Start recording
pub fn record_start() {
    if let Some(ref mut engine) = *MIDI_ENGINE.lock() {
        let tick = engine.sequencer.current_tick;
        engine.recorder.start(tick);
    }
}

/// Stop recording
pub fn record_stop() {
    if let Some(ref mut engine) = *MIDI_ENGINE.lock() {
        engine.recorder.stop();
    }
}
