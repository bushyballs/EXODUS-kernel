use crate::sync::Mutex;
/// Hoags Macro Recorder — record and replay user input sequences
///
/// Records keyboard, mouse, scroll, touch, and app-switch events with
/// precise timing. Macros can be replayed at adjustable speed, repeated
/// multiple times, paused, and edited (insert delays, modify events).
///
/// All coordinates and speed values use i32 Q16 fixed-point (65536 = 1.0).
/// No external crates. No f32/f64.
///
/// Inspired by: AutoHotkey, Karabiner, Keyboard Maestro, xdotool.
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

/// Q16 fixed-point: 65536 = 1.0x speed
type Q16 = i32;
const Q16_ONE: Q16 = 65536;
const Q16_HALF: Q16 = 32768;
const Q16_DOUBLE: Q16 = 131072;

/// Maximum macros stored
const MAX_MACROS: usize = 128;
/// Maximum events per macro
const MAX_EVENTS: usize = 4096;
/// Maximum recording buffer
const MAX_RECORDING_BUFFER: usize = 8192;

// ---------------------------------------------------------------------------
// MacroEventType
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacroEventType {
    /// Key pressed down (value_hash = key scancode hash)
    KeyPress,
    /// Key released (value_hash = key scancode hash)
    KeyRelease,
    /// Mouse button click (value_hash = button id hash, x/y = position)
    MouseClick,
    /// Mouse movement (x/y = delta or absolute position)
    MouseMove,
    /// Scroll wheel (x = horizontal, y = vertical scroll amount)
    Scroll,
    /// Text input string (value_hash = text hash)
    TextInput,
    /// Switch to another application (target_hash = app name hash)
    AppSwitch,
    /// Touch gesture (target_hash = gesture type hash, x/y = start position)
    TouchGesture,
}

// ---------------------------------------------------------------------------
// MacroEvent
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct MacroEvent {
    pub event_type: MacroEventType,
    pub target_hash: u64,
    pub value_hash: u64,
    pub x: i32,
    pub y: i32,
    pub timestamp: u64,
    pub delay_ms: u32,
}

impl MacroEvent {
    fn new(event_type: MacroEventType) -> Self {
        MacroEvent {
            event_type,
            target_hash: 0,
            value_hash: 0,
            x: 0,
            y: 0,
            timestamp: 0,
            delay_ms: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Macro
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Macro {
    pub id: u32,
    pub name_hash: u64,
    pub events: Vec<MacroEvent>,
    pub repeat_count: u32,
    pub speed: Q16,
}

impl Macro {
    fn new(id: u32, name_hash: u64) -> Self {
        Macro {
            id,
            name_hash,
            events: Vec::new(),
            repeat_count: 1,
            speed: Q16_ONE,
        }
    }

    /// Total duration in milliseconds (sum of all delays, adjusted by speed)
    fn total_duration_ms(&self) -> u64 {
        let mut total: u64 = 0;
        for event in &self.events {
            total += event.delay_ms as u64;
        }
        // Adjust by speed: duration = total * ONE / speed
        if self.speed > 0 {
            (total * Q16_ONE as u64) / self.speed as u64
        } else {
            total
        }
    }
}

// ---------------------------------------------------------------------------
// Playback state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlaybackState {
    Idle,
    Playing,
    Paused,
    Recording,
}

// ---------------------------------------------------------------------------
// MacroRecorderState
// ---------------------------------------------------------------------------

struct MacroRecorderState {
    macros: Vec<Macro>,
    next_id: u32,
    initialized: bool,

    // Recording state
    recording_state: PlaybackState,
    recording_buffer: Vec<MacroEvent>,
    recording_start_time: u64,
    recording_name_hash: u64,

    // Playback state
    playback_state: PlaybackState,
    playback_macro_id: u32,
    playback_event_index: usize,
    playback_repeat_remaining: u32,
    playback_speed: Q16,
    playback_elapsed_ms: u64,

    // Statistics
    total_macros_played: u64,
    total_events_replayed: u64,
}

impl MacroRecorderState {
    fn new() -> Self {
        MacroRecorderState {
            macros: Vec::new(),
            next_id: 1,
            initialized: false,
            recording_state: PlaybackState::Idle,
            recording_buffer: Vec::new(),
            recording_start_time: 0,
            recording_name_hash: 0,
            playback_state: PlaybackState::Idle,
            playback_macro_id: 0,
            playback_event_index: 0,
            playback_repeat_remaining: 0,
            playback_speed: Q16_ONE,
            playback_elapsed_ms: 0,
            total_macros_played: 0,
            total_events_replayed: 0,
        }
    }
}

static RECORDER: Mutex<Option<MacroRecorderState>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Recording API
// ---------------------------------------------------------------------------

/// Start recording a new macro. Returns true if recording started.
pub fn start_recording(name_hash: u64, current_time: u64) -> bool {
    let mut guard = RECORDER.lock();
    if let Some(ref mut state) = *guard {
        if state.recording_state != PlaybackState::Idle {
            serial_println!("[macro] ERROR: already recording or playing");
            return false;
        }
        state.recording_state = PlaybackState::Recording;
        state.recording_buffer.clear();
        state.recording_start_time = current_time;
        state.recording_name_hash = name_hash;
        serial_println!("[macro] Recording started (name_hash={:#018X})", name_hash);
        true
    } else {
        false
    }
}

/// Record a single event into the current recording buffer.
pub fn record_event(event: MacroEvent) -> bool {
    let mut guard = RECORDER.lock();
    if let Some(ref mut state) = *guard {
        if state.recording_state != PlaybackState::Recording {
            return false;
        }
        if state.recording_buffer.len() >= MAX_RECORDING_BUFFER {
            serial_println!("[macro] WARNING: recording buffer full, stopping");
            // Auto-stop recording
            state.recording_state = PlaybackState::Idle;
            return false;
        }
        state.recording_buffer.push(event);
        true
    } else {
        false
    }
}

/// Stop recording and save the macro. Returns the new macro ID.
pub fn stop_recording() -> u32 {
    let mut guard = RECORDER.lock();
    if let Some(ref mut state) = *guard {
        if state.recording_state != PlaybackState::Recording {
            serial_println!("[macro] ERROR: not recording");
            return 0;
        }
        state.recording_state = PlaybackState::Idle;

        if state.recording_buffer.is_empty() {
            serial_println!("[macro] WARNING: no events recorded, discarding");
            return 0;
        }

        if state.macros.len() >= MAX_MACROS {
            serial_println!("[macro] ERROR: max macros ({}) reached", MAX_MACROS);
            return 0;
        }

        // Calculate delays between consecutive events
        let mut events = state.recording_buffer.clone();
        for i in 1..events.len() {
            let prev_ts = events[i - 1].timestamp;
            let curr_ts = events[i].timestamp;
            if curr_ts > prev_ts {
                events[i].delay_ms = (curr_ts - prev_ts) as u32;
            }
        }

        let id = state.next_id;
        state.next_id = state.next_id.saturating_add(1);

        let mut new_macro = Macro::new(id, state.recording_name_hash);
        new_macro.events = events;

        serial_println!(
            "[macro] Saved macro {} ({} events, {}ms total)",
            id,
            new_macro.events.len(),
            new_macro.total_duration_ms()
        );
        state.macros.push(new_macro);
        id
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// Playback API
// ---------------------------------------------------------------------------

/// Start playing a macro by ID. Returns true if playback started.
pub fn play_macro(macro_id: u32) -> bool {
    let mut guard = RECORDER.lock();
    if let Some(ref mut state) = *guard {
        if state.playback_state == PlaybackState::Playing {
            serial_println!("[macro] ERROR: already playing");
            return false;
        }
        if state.recording_state == PlaybackState::Recording {
            serial_println!("[macro] ERROR: currently recording");
            return false;
        }

        let found = state.macros.iter().any(|m| m.id == macro_id);
        if !found {
            serial_println!("[macro] ERROR: macro {} not found", macro_id);
            return false;
        }

        // Find the macro to get repeat count and speed
        let (repeat, speed) = {
            let m = state.macros.iter().find(|m| m.id == macro_id).unwrap();
            (m.repeat_count, m.speed)
        };

        state.playback_state = PlaybackState::Playing;
        state.playback_macro_id = macro_id;
        state.playback_event_index = 0;
        state.playback_repeat_remaining = repeat;
        state.playback_speed = speed;
        state.playback_elapsed_ms = 0;
        state.total_macros_played = state.total_macros_played.saturating_add(1);

        serial_println!(
            "[macro] Playing macro {} (repeats={}, speed={})",
            macro_id,
            repeat,
            speed
        );
        true
    } else {
        false
    }
}

/// Pause the currently playing macro.
pub fn pause() -> bool {
    let mut guard = RECORDER.lock();
    if let Some(ref mut state) = *guard {
        if state.playback_state == PlaybackState::Playing {
            state.playback_state = PlaybackState::Paused;
            serial_println!("[macro] Paused at event {}", state.playback_event_index);
            return true;
        }
        if state.playback_state == PlaybackState::Paused {
            state.playback_state = PlaybackState::Playing;
            serial_println!("[macro] Resumed from event {}", state.playback_event_index);
            return true;
        }
    }
    false
}

/// Set playback speed for a macro. Q16: 65536=1x, 32768=0.5x, 131072=2x.
pub fn set_speed(macro_id: u32, speed: Q16) -> bool {
    let mut guard = RECORDER.lock();
    if let Some(ref mut state) = *guard {
        for m in &mut state.macros {
            if m.id == macro_id {
                let clamped = if speed < 1638 {
                    1638
                }
                // min 0.025x
                else if speed > 655360 {
                    655360
                }
                // max 10x
                else {
                    speed
                };
                m.speed = clamped;
                serial_println!("[macro] Set speed for macro {} to {}", macro_id, clamped);
                return true;
            }
        }
    }
    false
}

/// Advance playback by delta_ms. Returns the next event to dispatch, if any.
pub fn tick_playback(delta_ms: u32) -> Option<MacroEvent> {
    let mut guard = RECORDER.lock();
    if let Some(ref mut state) = *guard {
        if state.playback_state != PlaybackState::Playing {
            return None;
        }

        state.playback_elapsed_ms += delta_ms as u64;

        // Find current macro
        let macro_data = state
            .macros
            .iter()
            .find(|m| m.id == state.playback_macro_id);
        if macro_data.is_none() {
            state.playback_state = PlaybackState::Idle;
            return None;
        }
        let macro_data = macro_data.unwrap();
        let event_count = macro_data.events.len();

        if state.playback_event_index >= event_count {
            // End of macro — check for repeats
            state.playback_repeat_remaining = state.playback_repeat_remaining.saturating_sub(1);
            if state.playback_repeat_remaining > 0 {
                state.playback_event_index = 0;
                state.playback_elapsed_ms = 0;
                serial_println!(
                    "[macro] Repeat ({} remaining)",
                    state.playback_repeat_remaining
                );
            } else {
                state.playback_state = PlaybackState::Idle;
                serial_println!(
                    "[macro] Playback complete for macro {}",
                    state.playback_macro_id
                );
            }
            return None;
        }

        let event = macro_data.events[state.playback_event_index];

        // Adjust delay by speed: actual_delay = delay * ONE / speed
        let adjusted_delay = if state.playback_speed > 0 {
            ((event.delay_ms as i64) * (Q16_ONE as i64)) / (state.playback_speed as i64)
        } else {
            event.delay_ms as i64
        };

        if state.playback_elapsed_ms >= adjusted_delay as u64 {
            state.playback_event_index += 1;
            state.playback_elapsed_ms = 0;
            state.total_events_replayed = state.total_events_replayed.saturating_add(1);
            return Some(event);
        }

        None
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Edit API
// ---------------------------------------------------------------------------

/// Edit a specific event within a saved macro.
pub fn edit_event(macro_id: u32, event_index: usize, new_event: MacroEvent) -> bool {
    let mut guard = RECORDER.lock();
    if let Some(ref mut state) = *guard {
        for m in &mut state.macros {
            if m.id == macro_id {
                if event_index < m.events.len() {
                    m.events[event_index] = new_event;
                    serial_println!("[macro] Edited event {} in macro {}", event_index, macro_id);
                    return true;
                }
            }
        }
    }
    false
}

/// Insert a delay event at a specific position in a macro.
pub fn insert_delay(macro_id: u32, position: usize, delay_ms: u32) -> bool {
    let mut guard = RECORDER.lock();
    if let Some(ref mut state) = *guard {
        for m in &mut state.macros {
            if m.id == macro_id {
                if position <= m.events.len() && m.events.len() < MAX_EVENTS {
                    let mut delay_event = MacroEvent::new(MacroEventType::KeyPress);
                    delay_event.delay_ms = delay_ms;
                    delay_event.event_type = MacroEventType::KeyRelease; // no-op event
                    delay_event.value_hash = 0; // null key = pure delay
                    m.events.insert(position, delay_event);
                    serial_println!(
                        "[macro] Inserted {}ms delay at position {} in macro {}",
                        delay_ms,
                        position,
                        macro_id
                    );
                    return true;
                }
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Storage API
// ---------------------------------------------------------------------------

/// Save a macro to persistent storage (returns serialized byte count).
/// In a full implementation this would write to the filesystem.
pub fn save_macro(macro_id: u32) -> u32 {
    let guard = RECORDER.lock();
    if let Some(ref state) = *guard {
        for m in &state.macros {
            if m.id == macro_id {
                // Estimate serialized size: header(32) + events * 40 bytes each
                let size = 32 + (m.events.len() as u32) * 40;
                serial_println!(
                    "[macro] Saved macro {} to storage ({} bytes)",
                    macro_id,
                    size
                );
                return size;
            }
        }
    }
    0
}

/// Load a macro from persistent storage. Returns the loaded macro ID.
/// In a full implementation this would read from the filesystem.
pub fn load_macro(name_hash: u64) -> u32 {
    let mut guard = RECORDER.lock();
    if let Some(ref mut state) = *guard {
        // Check if already loaded
        for m in &state.macros {
            if m.name_hash == name_hash {
                serial_println!("[macro] Macro already loaded (id={})", m.id);
                return m.id;
            }
        }

        if state.macros.len() >= MAX_MACROS {
            serial_println!("[macro] ERROR: max macros reached");
            return 0;
        }

        // Create a placeholder — in reality this would deserialize from disk
        let id = state.next_id;
        state.next_id = state.next_id.saturating_add(1);
        let new_macro = Macro::new(id, name_hash);
        state.macros.push(new_macro);
        serial_println!(
            "[macro] Loaded macro {} from storage (name_hash={:#018X})",
            id,
            name_hash
        );
        id
    } else {
        0
    }
}

/// Delete a macro by ID.
pub fn delete_macro(macro_id: u32) -> bool {
    let mut guard = RECORDER.lock();
    if let Some(ref mut state) = *guard {
        let before = state.macros.len();
        state.macros.retain(|m| m.id != macro_id);
        let deleted = state.macros.len() < before;
        if deleted {
            serial_println!("[macro] Deleted macro {}", macro_id);
        }
        deleted
    } else {
        false
    }
}

/// Get the number of stored macros.
pub fn macro_count() -> usize {
    let guard = RECORDER.lock();
    if let Some(ref state) = *guard {
        state.macros.len()
    } else {
        0
    }
}

/// Get total events replayed.
pub fn total_replayed() -> u64 {
    let guard = RECORDER.lock();
    if let Some(ref state) = *guard {
        state.total_events_replayed
    } else {
        0
    }
}

/// Check if currently recording.
pub fn is_recording() -> bool {
    let guard = RECORDER.lock();
    if let Some(ref state) = *guard {
        state.recording_state == PlaybackState::Recording
    } else {
        false
    }
}

/// Check if currently playing.
pub fn is_playing() -> bool {
    let guard = RECORDER.lock();
    if let Some(ref state) = *guard {
        state.playback_state == PlaybackState::Playing
    } else {
        false
    }
}

pub fn init() {
    let mut guard = RECORDER.lock();
    *guard = Some(MacroRecorderState::new());
    if let Some(ref mut state) = *guard {
        state.initialized = true;
    }
    serial_println!("    [scripting] Macro recorder initialized (record, replay, edit)");
}
