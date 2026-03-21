use crate::serial_println;
use crate::sync::Mutex;

// ── Action outcome ────────────────────────────────────────────────────────────

#[repr(u8)]
#[derive(Copy, Clone, PartialEq)]
pub enum ActionOutcome {
    Succeeded      = 0,
    PartialSuccess = 1,
    Failed         = 2,
    Unexpected     = 3,
}

// ── Action record ─────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct ActionRecord {
    pub intent_kind: u8,
    pub outcome:     ActionOutcome,
    pub bond_delta:  i16,
    pub tick:        u32,
}

impl ActionRecord {
    pub const fn empty() -> Self {
        Self {
            intent_kind: 0,
            outcome:     ActionOutcome::Succeeded,
            bond_delta:  0,
            tick:        0,
        }
    }
}

// ── Core state ────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct ReflectionState {
    pub action_log:        [ActionRecord; 64],  // circular buffer
    pub log_head:          usize,
    pub log_count:         usize,               // total ever written
    pub success_rate:      u16,                 // 0-1000
    pub failure_streak:    u8,
    pub wisdom_score:      u16,
    pub bias_corrections:  [i16; 11],           // one per NeedKind (0-10)
    pub total_reflections: u32,
    pub self_doubt:        u16,
    pub integrity_score:   u16,
    pub wisdom_crossed_500: bool,               // latched to fire log once
}

impl ReflectionState {
    pub const fn new() -> Self {
        Self {
            action_log:        [ActionRecord::empty(); 64],
            log_head:          0,
            log_count:         0,
            success_rate:      500,
            failure_streak:    0,
            wisdom_score:      0,
            bias_corrections:  [0i16; 11],
            total_reflections: 0,
            self_doubt:        100,
            integrity_score:   0,
            wisdom_crossed_500: false,
        }
    }
}

pub static STATE: Mutex<ReflectionState> = Mutex::new(ReflectionState::new());

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    serial_println!("  life::self_reflection: introspection module online");
}

// ── Record action ─────────────────────────────────────────────────────────────

pub fn record_action(intent_kind: u8, outcome: ActionOutcome, bond_delta: i16, tick: u32) {
    let mut s = STATE.lock();

    // Write into circular buffer
    let idx = s.log_head % 64;
    s.action_log[idx] = ActionRecord { intent_kind, outcome, bond_delta, tick };
    s.log_head = (s.log_head + 1) % 64;
    s.log_count = s.log_count.saturating_add(1);

    // Update success_rate: rolling average over recent entries
    // Simple approach: count successes in entire (up to 64-entry) window
    let window = if s.log_count < 64 { s.log_count } else { 64 };
    let mut successes: u32 = 0;
    for i in 0..window {
        match s.action_log[i].outcome {
            ActionOutcome::Succeeded => successes += 1,
            ActionOutcome::PartialSuccess => successes += 0, // partial is not full success
            _ => {}
        }
    }
    s.success_rate = ((successes * 1000) / (window as u32).max(1)) as u16;

    // Update failure streak
    match outcome {
        ActionOutcome::Failed | ActionOutcome::Unexpected => {
            s.failure_streak = s.failure_streak.saturating_add(1);
        }
        ActionOutcome::Succeeded => {
            s.failure_streak = 0;
            // Reduce self-doubt on success
            s.self_doubt = s.self_doubt.saturating_sub(10);
        }
        ActionOutcome::PartialSuccess => {
            // Partial: don't reset streak fully but don't increment
            if s.failure_streak > 0 {
                s.failure_streak -= 1;
            }
        }
    }

    // Failure streak threshold trigger
    if s.failure_streak >= 3 {
        let kind = intent_kind;
        let bias_idx = (kind as usize).min(10);
        s.bias_corrections[bias_idx] =
            s.bias_corrections[bias_idx].saturating_sub(50);
        s.self_doubt = s.self_doubt.saturating_add(50).min(1000);
        serial_println!(
            "[reflect] 3 failures in a row — adjusting bias for kind {}",
            kind
        );
    }
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % 100 != 0 {
        return;
    }

    let mut s = STATE.lock();
    s.total_reflections = s.total_reflections.saturating_add(1);

    // Scan last 10 actions for streaks
    let window_size: usize = 10.min(s.log_count);
    let mut consecutive_failures: u8 = 0;
    let mut consecutive_successes: u8 = 0;
    let mut last_fail_kind: u8 = 0;
    let mut last_success_kind: u8 = 0;

    for i in 0..window_size {
        // Walk backwards from current head
        let idx = (s.log_head + 64 - 1 - i) % 64;
        match s.action_log[idx].outcome {
            ActionOutcome::Failed | ActionOutcome::Unexpected => {
                if consecutive_successes == 0 {
                    consecutive_failures += 1;
                    last_fail_kind = s.action_log[idx].intent_kind;
                }
            }
            ActionOutcome::Succeeded => {
                if consecutive_failures == 0 {
                    consecutive_successes += 1;
                    last_success_kind = s.action_log[idx].intent_kind;
                }
            }
            ActionOutcome::PartialSuccess => {
                // breaks streaks
                break;
            }
        }
    }

    // Apply bias corrections from scan
    if consecutive_failures >= 3 {
        let bias_idx = (last_fail_kind as usize).min(10);
        s.bias_corrections[bias_idx] =
            s.bias_corrections[bias_idx].saturating_sub(50);
        s.self_doubt = s.self_doubt.saturating_add(20).min(1000);
    }

    if consecutive_successes >= 5 {
        let bias_idx = (last_success_kind as usize).min(10);
        s.bias_corrections[bias_idx] =
            s.bias_corrections[bias_idx].saturating_add(20);
        // cap individual bias at +/- 500
        if s.bias_corrections[bias_idx] > 500 {
            s.bias_corrections[bias_idx] = 500;
        }
        s.self_doubt = s.self_doubt.saturating_sub(10);
    }

    // Wisdom grows by 1 per reflection cycle
    let prev_wisdom = s.wisdom_score;
    s.wisdom_score = s.wisdom_score.saturating_add(1).min(1000);

    // Fire once when wisdom crosses 500
    if prev_wisdom < 500 && s.wisdom_score >= 500 && !s.wisdom_crossed_500 {
        s.wisdom_crossed_500 = true;
        serial_println!(
            "[reflect] ANIMA has grown wise — wisdom={}",
            s.wisdom_score
        );
    }

    // integrity = success_rate * wisdom / 1000
    s.integrity_score =
        ((s.success_rate as u32 * s.wisdom_score as u32) / 1000) as u16;
}

// ── Bias query ────────────────────────────────────────────────────────────────

pub fn get_bias(intent_kind: u8) -> i16 {
    let s = STATE.lock();
    let idx = (intent_kind as usize).min(10);
    s.bias_corrections[idx]
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn success_rate() -> u16 {
    STATE.lock().success_rate
}

pub fn wisdom_score() -> u16 {
    STATE.lock().wisdom_score
}

pub fn integrity_score() -> u16 {
    STATE.lock().integrity_score
}

pub fn self_doubt() -> u16 {
    STATE.lock().self_doubt
}

pub fn total_reflections() -> u32 {
    STATE.lock().total_reflections
}
