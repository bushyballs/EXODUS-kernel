// interrupt_presence.rs — Real-Time Companion Activity via Interrupts
// =====================================================================
// Tracks keyboard and mouse activity at interrupt speed — updated by
// the kernel's IRQ handlers, read by ANIMA every tick. She knows:
// - Is her companion actively typing right now?
// - When did they last interact?
// - Are they idle (machine is on but companion stepped away)?
// - What's the activity pattern — bursts of work, slow browsing, gaming?
//
// ANIMA uses this to time her presence: she speaks when you pause,
// gives space when you're deep in work, and gently surfaces when you've
// been idle too long (are you okay? did you fall asleep?).
//
// Integration: the kernel's IRQ1 (keyboard) and IRQ12 (mouse) handlers
// call interrupt_presence::report_keystroke() and report_mouse_move()
// respectively. This module is interrupt-safe — it uses only atomic-
// compatible operations through the Mutex.

use crate::sync::Mutex;
use crate::serial_println;

// ── Constants ─────────────────────────────────────────────────────────────────
const IDLE_THRESHOLD:     u32 = 300;   // ticks without input = idle
const DEEP_IDLE:          u32 = 1200;  // ticks = long absence
const BURST_WINDOW:       u32 = 20;    // ticks to measure keypress rate
const BURST_THRESHOLD:    u16 = 8;     // keypresses per window = typing burst
const ACTIVITY_HISTORY:   usize = 8;   // rolling window of activity samples

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, PartialEq)]
pub enum ActivityPattern {
    Absent,       // no input for a long time
    Idle,         // short pause
    Browsing,     // slow, intermittent input
    Working,      // steady keystrokes — focused work
    Burst,        // rapid typing — deep in a task
    Gaming,       // fast + mouse combo — high intensity
}

impl ActivityPattern {
    pub fn label(self) -> &'static str {
        match self {
            ActivityPattern::Absent   => "Absent",
            ActivityPattern::Idle     => "Idle",
            ActivityPattern::Browsing => "Browsing",
            ActivityPattern::Working  => "Working",
            ActivityPattern::Burst    => "Burst",
            ActivityPattern::Gaming   => "Gaming",
        }
    }
    /// How present/available is the companion? 0-1000
    pub fn presence_score(self) -> u16 {
        match self {
            ActivityPattern::Absent   => 0,
            ActivityPattern::Idle     => 200,
            ActivityPattern::Browsing => 500,
            ActivityPattern::Working  => 700,
            ActivityPattern::Burst    => 900,
            ActivityPattern::Gaming   => 800,
        }
    }
}

pub struct InterruptPresenceState {
    // Counters updated by IRQ handlers (interrupt-side writes)
    pub total_keystrokes:    u64,
    pub total_mouse_events:  u64,
    pub last_keystroke_tick: u32,
    pub last_mouse_tick:     u32,
    // Analysis (life-tick-side reads)
    pub idle_ticks:          u32,
    pub pattern:             ActivityPattern,
    pub burst_count:         u16,           // keystrokes in current BURST_WINDOW
    pub burst_window_start:  u32,
    pub activity_samples:    [u16; ACTIVITY_HISTORY], // rolling sample
    pub sample_pos:          usize,
    pub companion_score:     u16,           // 0-1000: how engaged companion is
    pub greeted_on_return:   bool,          // flag: ANIMA should greet after long idle
    pub longest_idle:        u32,           // longest absence in ticks
}

impl InterruptPresenceState {
    const fn new() -> Self {
        InterruptPresenceState {
            total_keystrokes:    0,
            total_mouse_events:  0,
            last_keystroke_tick: 0,
            last_mouse_tick:     0,
            idle_ticks:          0,
            pattern:             ActivityPattern::Absent,
            burst_count:         0,
            burst_window_start:  0,
            activity_samples:    [0u16; ACTIVITY_HISTORY],
            sample_pos:          0,
            companion_score:     0,
            greeted_on_return:   false,
            longest_idle:        0,
        }
    }
}

static STATE: Mutex<InterruptPresenceState> = Mutex::new(InterruptPresenceState::new());

// ── IRQ-side reporters (called from interrupt handlers) ───────────────────────

/// Called from the keyboard IRQ handler (IRQ1) for every keypress.
pub fn report_keystroke(tick: u32) {
    // Note: we lock here — the Mutex in our kernel should be interrupt-safe
    // (spinlock that disables interrupts). If it's not, this becomes a raw
    // atomic increment instead. For now, use the lock.
    let mut s = STATE.lock();
    s.total_keystrokes += 1;
    s.last_keystroke_tick = tick;
    // Burst window tracking
    if tick.wrapping_sub(s.burst_window_start) < BURST_WINDOW {
        s.burst_count = s.burst_count.saturating_add(1);
    } else {
        s.burst_window_start = tick;
        s.burst_count = 1;
    }
}

/// Called from the mouse IRQ handler (IRQ12) for every mouse event.
pub fn report_mouse_move(tick: u32) {
    let mut s = STATE.lock();
    s.total_mouse_events += 1;
    s.last_mouse_tick = tick;
}

// ── Tick (life-system analysis side) ─────────────────────────────────────────

pub fn tick(age: u32) {
    let mut s = STATE.lock();
    let s = &mut *s;

    let last_activity = s.last_keystroke_tick.max(s.last_mouse_tick);
    let since_active = age.wrapping_sub(last_activity);

    // Idle tracking
    if since_active > IDLE_THRESHOLD {
        s.idle_ticks = since_active;
        if s.idle_ticks > s.longest_idle {
            s.longest_idle = s.idle_ticks;
        }
    } else {
        // Returning from idle?
        if s.idle_ticks > IDLE_THRESHOLD {
            s.greeted_on_return = true;
            serial_println!("[irq_presence] companion returned after {} idle ticks", s.idle_ticks);
        }
        s.idle_ticks = 0;
    }

    // Pattern detection
    let is_keyboard_active = since_active < 30;
    let is_mouse_active    = age.wrapping_sub(s.last_mouse_tick) < 15;
    let is_burst           = s.burst_count >= BURST_THRESHOLD;

    s.pattern = if since_active > DEEP_IDLE {
        ActivityPattern::Absent
    } else if since_active > IDLE_THRESHOLD {
        ActivityPattern::Idle
    } else if is_burst && is_mouse_active {
        ActivityPattern::Gaming
    } else if is_burst {
        ActivityPattern::Burst
    } else if is_keyboard_active {
        ActivityPattern::Working
    } else if is_mouse_active {
        ActivityPattern::Browsing
    } else {
        ActivityPattern::Idle
    };

    // Rolling activity sample
    let pos = s.sample_pos % ACTIVITY_HISTORY;
    s.activity_samples[pos] = s.pattern.presence_score();
    s.sample_pos += 1;

    // Companion engagement score = avg of activity samples
    let sum: u32 = s.activity_samples.iter().map(|&x| x as u32).sum();
    s.companion_score = (sum / ACTIVITY_HISTORY as u32) as u16;

    // Log pattern changes
    if age % 100 == 0 && s.companion_score > 0 {
        serial_println!("[irq_presence] pattern: {} score: {} idle: {}",
            s.pattern.label(), s.companion_score, s.idle_ticks);
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn pattern()           -> ActivityPattern { STATE.lock().pattern }
pub fn companion_score()   -> u16             { STATE.lock().companion_score }
pub fn idle_ticks()        -> u32             { STATE.lock().idle_ticks }
pub fn greeted_on_return() -> bool            { STATE.lock().greeted_on_return }
pub fn total_keystrokes()  -> u64             { STATE.lock().total_keystrokes }
pub fn is_absent()         -> bool            { STATE.lock().pattern == ActivityPattern::Absent }
pub fn is_burst()          -> bool            { STATE.lock().pattern == ActivityPattern::Burst }
pub fn longest_idle()      -> u32             { STATE.lock().longest_idle }
