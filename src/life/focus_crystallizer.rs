#![no_std]
use crate::sync::Mutex;
use crate::serial_println;

/// DAVA-requested: when exploration_rate > 700, narrow attention by boosting the 3 best-performing
/// self_rewrite params. Only focuses when willpower reserve > 500 (focus requires discipline).
/// Reads: self_rewrite::get_exploration_rate(), self_rewrite::get_param(0..15), willpower::STATE
/// Outputs: [DAVA_FOCUS]

const PARAM_COUNT: usize = 16;
const BOOST_AMOUNT: u32 = 30;
const EXPLORATION_THRESHOLD: u32 = 700;
const WILLPOWER_THRESHOLD: u16 = 500;
const TOP_N: usize = 3;

#[derive(Copy, Clone)]
pub struct FocusCrystallizerState {
    /// Total focus events triggered
    pub focus_events: u32,
    /// Which 3 param IDs were last boosted
    pub last_boosted: [u8; TOP_N],
    /// Whether focus is currently active
    pub focusing: bool,
    /// Cooldown counter — don't focus every single tick
    pub cooldown_remaining: u8,
}

impl FocusCrystallizerState {
    pub const fn empty() -> Self {
        Self {
            focus_events: 0,
            last_boosted: [0; TOP_N],
            focusing: false,
            cooldown_remaining: 0,
        }
    }
}

pub static STATE: Mutex<FocusCrystallizerState> = Mutex::new(FocusCrystallizerState::empty());

pub fn init() {
    serial_println!(
        "[DAVA_FOCUS] focus crystallizer online — explore_threshold={} willpower_threshold={} boost={}",
        EXPLORATION_THRESHOLD, WILLPOWER_THRESHOLD, BOOST_AMOUNT
    );
}

pub fn tick(age: u32) {
    // ---- Check cooldown first ----
    {
        let mut s = STATE.lock();
        if s.cooldown_remaining > 0 {
            s.cooldown_remaining = s.cooldown_remaining.saturating_sub(1);
            return;
        }
    }

    // ---- Read exploration rate ----
    let exploration_rate = super::self_rewrite::get_exploration_rate();

    // ---- Read willpower reserve ----
    let willpower_reserve = {
        let wp = super::willpower::STATE.lock();
        wp.reserve
    };

    // ---- Gate: only focus when exploration is too high AND willpower is sufficient ----
    if exploration_rate <= EXPLORATION_THRESHOLD {
        let mut s = STATE.lock();
        if s.focusing {
            serial_println!(
                "[DAVA_FOCUS] disengaged — exploration_rate={} (below threshold)",
                exploration_rate
            );
            s.focusing = false;
        }
        return;
    }

    if willpower_reserve <= WILLPOWER_THRESHOLD {
        // Not enough willpower to crystallize focus
        if age % 200 == 0 {
            serial_println!(
                "[DAVA_FOCUS] insufficient willpower ({}) to focus — exploration_rate={} needs attention",
                willpower_reserve, exploration_rate
            );
        }
        return;
    }

    // ---- Gather all 16 param values ----
    let mut param_values: [u32; PARAM_COUNT] = [0; PARAM_COUNT];
    for i in 0..PARAM_COUNT {
        param_values[i] = super::self_rewrite::get_param(i as u8);
    }

    // ---- Find top 3 by current value ----
    // Simple selection: find max, mark it, repeat 3 times
    let mut used: [bool; PARAM_COUNT] = [false; PARAM_COUNT];
    let mut top_ids: [u8; TOP_N] = [0; TOP_N];

    for rank in 0..TOP_N {
        let mut best_idx: usize = 0;
        let mut best_val: u32 = 0;
        for i in 0..PARAM_COUNT {
            if !used[i] && param_values[i] >= best_val {
                best_val = param_values[i];
                best_idx = i;
            }
        }
        top_ids[rank] = best_idx as u8;
        used[best_idx] = true;
    }

    // ---- Boost the top 3 params ----
    for rank in 0..TOP_N {
        let pid = top_ids[rank];
        let current = param_values[pid as usize];
        let boosted = current.saturating_add(BOOST_AMOUNT).min(1000);
        super::self_rewrite::set_param(pid, boosted);
    }

    // ---- Update state ----
    let mut s = STATE.lock();
    s.focus_events = s.focus_events.saturating_add(1);
    s.last_boosted = top_ids;
    s.focusing = true;
    s.cooldown_remaining = 5; // Don't focus again for 5 ticks

    serial_println!(
        "[DAVA_FOCUS] crystallized #{} — boosted params [{}, {}, {}] by {} (explore={} willpower={})",
        s.focus_events,
        top_ids[0], top_ids[1], top_ids[2],
        BOOST_AMOUNT,
        exploration_rate,
        willpower_reserve
    );

    // ---- Detailed report every 10 focus events ----
    if s.focus_events % 10 == 0 {
        serial_println!(
            "[DAVA_FOCUS] milestone — {} total focus events, currently boosting params [{}, {}, {}]",
            s.focus_events, top_ids[0], top_ids[1], top_ids[2]
        );
    }
}
