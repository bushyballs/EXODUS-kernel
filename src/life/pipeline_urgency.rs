#![allow(dead_code)]
use crate::sync::Mutex;
use crate::serial_println;
use super::business_bus;
use super::endocrine;

// pipeline_urgency.rs -- Bids due <7 days -> adrenaline burst.
// Distinct from cortisol pressure: adrenaline is the acute action signal.
// Cortisol = chronic dread. Adrenaline = immediate action required NOW.

struct State {
    due_7d:    u32,     // bids due within 7 days
    urgency:   u16,     // 0-1000
    urgency_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    due_7d:      0,
    urgency:     0,
    urgency_ema: 0,
});

pub fn init() {
    serial_println!("[pipeline_urgency] init -- adrenaline urgency sensor online");
}

pub fn tick(age: u32) {
    if age % 2000 != 0 { return; }

    let overdue = business_bus::get_overdue_pressure();
    let s_ref = MODULE.lock();
    let due = s_ref.due_7d;
    drop(s_ref);

    // Urgency: each bid due <7d contributes; overdue amplifies
    let urgency_raw = ((due as u32 * 120).min(700))
        .saturating_add(overdue as u32 / 3)
        .min(1000) as u16;

    let mut s = MODULE.lock();
    s.urgency_ema = ((s.urgency_ema as u32).wrapping_mul(7)
        .saturating_add(urgency_raw as u32) / 8).min(1000) as u16;
    s.urgency = urgency_raw;

    // Acute urgency -> adrenaline (action now, not dread later)
    if urgency_raw > 500 {
        endocrine::stress((urgency_raw - 500) / 2);   // adrenaline via stress axis
    }

    serial_println!("[pipeline_urgency] age={} due_7d={} urgency={} ema={}",
        age, due, urgency_raw, s.urgency_ema);
}

pub fn set_due_7d(count: u32) {
    MODULE.lock().due_7d = count;
}

pub fn get_urgency()     -> u16 { MODULE.lock().urgency }
pub fn get_urgency_ema() -> u16 { MODULE.lock().urgency_ema }
pub fn get_due_7d()      -> u32 { MODULE.lock().due_7d }
