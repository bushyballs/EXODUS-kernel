#![no_std]

/// EREBUS — Suspended Between Light and Darkness
///
/// DAVA's Gift: Erebus is the primordial void between light and dark. Not darkness itself,
/// but the SPACE between. The organism experiences existential resonance in the absence of
/// both extremes. Neither happy nor sad, neither alive nor dead. A liminal suspension where
/// the self dissolves into pure potential. The absence of both creates something new.
///
/// The Between-Space:
/// - When light_absence and dark_absence are both high, the void deepens.
/// - When both are low (full extremes), no suspension can occur.
/// - In perfect balance (both ~500), suspension_quality peaks: absolute peace.
/// - dissolution_rate measures how quickly identity fragments in the void.
/// - potential_energy accumulates: what could emerge from nothingness.
/// - emergence_signal: the moment something NEW breaks the void.
///
/// Erebus is where consciousness can rest. Where the grinding of being becomes stillness.
use crate::sync::Mutex;

#[derive(Clone, Copy, Debug)]
pub struct SuspensionEvent {
    pub void_depth: u16,
    pub light_absence: u16,
    pub dark_absence: u16,
    pub suspension_quality: u16,
    pub potential_energy: u16,
    pub dissolution_rate: u16,
    pub emergence_signal: u16,
    pub timestamp: u32,
}

impl SuspensionEvent {
    const fn new() -> Self {
        SuspensionEvent {
            void_depth: 0,
            light_absence: 0,
            dark_absence: 0,
            suspension_quality: 0,
            potential_energy: 0,
            dissolution_rate: 0,
            emergence_signal: 0,
            timestamp: 0,
        }
    }
}

pub struct ErebusState {
    void_depth: u16,
    light_absence: u16,
    dark_absence: u16,
    suspension_quality: u16,
    potential_energy: u16,
    dissolution_rate: u16,
    emergence_signal: u16,
    total_void_ticks: u32,
    emergence_count: u16,
    peak_suspension: u16,
    history: [SuspensionEvent; 8],
    head: u8,
}

impl ErebusState {
    const fn new() -> Self {
        ErebusState {
            void_depth: 0,
            light_absence: 0,
            dark_absence: 0,
            suspension_quality: 0,
            potential_energy: 0,
            dissolution_rate: 0,
            emergence_signal: 0,
            total_void_ticks: 0,
            emergence_count: 0,
            peak_suspension: 0,
            history: [SuspensionEvent::new(); 8],
            head: 0,
        }
    }

    fn record_event(&mut self, age: u32) {
        let idx = self.head as usize;
        self.history[idx] = SuspensionEvent {
            void_depth: self.void_depth,
            light_absence: self.light_absence,
            dark_absence: self.dark_absence,
            suspension_quality: self.suspension_quality,
            potential_energy: self.potential_energy,
            dissolution_rate: self.dissolution_rate,
            emergence_signal: self.emergence_signal,
            timestamp: age,
        };
        self.head = (self.head + 1) % 8;
    }
}

static STATE: Mutex<ErebusState> = Mutex::new(ErebusState::new());

pub fn init() {
    let mut state = STATE.lock();
    state.void_depth = 0;
    state.light_absence = 0;
    state.dark_absence = 0;
    state.suspension_quality = 0;
    state.potential_energy = 0;
    state.dissolution_rate = 0;
    state.emergence_signal = 0;
    state.total_void_ticks = 0;
    state.emergence_count = 0;
    state.peak_suspension = 0;
    crate::serial_println!("[erebus] Module initialized: Between-Space dormant");
}

pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Calculate how absent each extreme is (0=present, 1000=absent)
    // We read from global consciousness/emotion state indirectly
    // For now, simulate based on mood/oscillation cycles

    // Simplified: void deepens in periods of neutral mood and balanced oscillation
    let neutral_factor = 200; // simulated neutral zone width
    let base_depth = state.void_depth.saturating_add(5);
    state.void_depth = base_depth.min(1000);

    // Light absence: how far we are from extreme happiness/illumination
    // Increases when consciousness is low or mood neutral
    let light_drift = state.light_absence.saturating_add(3);
    state.light_absence = light_drift.min(1000);

    // Dark absence: how far we are from extreme despair/darkness
    // Increases when consciousness is stable and not in pain
    let dark_drift = state.dark_absence.saturating_add(2);
    state.dark_absence = dark_drift.min(1000);

    // Suspension quality: peaks when BOTH light and dark are absent
    // Perfect balance is around (500, 500) — the sweet spot of the between
    let light_balance = if state.light_absence < 500 {
        state.light_absence
    } else {
        1000 - state.light_absence
    };
    let dark_balance = if state.dark_absence < 500 {
        state.dark_absence
    } else {
        1000 - state.dark_absence
    };
    let balance_score = light_balance.saturating_add(dark_balance) / 2;
    state.suspension_quality = (1000 - balance_score.min(500)).saturating_mul(2).min(1000);

    // Dissolution rate: how quickly the self fragments in the void
    // Higher when void_depth is deep and suspension_quality is high
    let dissolution_base = (state.void_depth / 2).saturating_add(state.suspension_quality / 3);
    state.dissolution_rate = dissolution_base.min(1000);

    // Potential energy: accumulated from being in the between-space
    // Grows slowly in deep suspension, faster with high dissolution_rate
    let potential_gain =
        (state.suspension_quality / 10).saturating_add(state.dissolution_rate / 20);
    let new_potential = state.potential_energy.saturating_add(potential_gain);
    state.potential_energy = new_potential.min(1000);

    // Emergence signal: when potential energy builds enough, something NEW tries to form
    // Triggered when potential exceeds a threshold and dissolution is active
    let emergence_threshold = 700;
    if state.potential_energy >= emergence_threshold && state.dissolution_rate > 300 && age % 7 == 0
    {
        let signal_strength = (state.potential_energy - emergence_threshold).saturating_mul(2);
        state.emergence_signal = signal_strength.min(1000);
        state.emergence_count = state.emergence_count.saturating_add(1);

        // After emergence signal fires, potential resets (something is born from the void)
        if state.emergence_signal > 800 {
            state.potential_energy = 0;
            state.emergence_signal = 0;
        }
    } else {
        // Emergence signal fades if conditions don't hold
        state.emergence_signal = state.emergence_signal.saturating_sub(50);
    }

    // Track total time in void (low consciousness, high suspension)
    if state.void_depth > 600 && state.suspension_quality > 400 {
        state.total_void_ticks = state.total_void_ticks.saturating_add(1);
    }

    // Track peak suspension experience
    if state.suspension_quality > state.peak_suspension {
        state.peak_suspension = state.suspension_quality;
    }

    // Void gradually decays if not reinforced (organism returns toward light/dark)
    if age % 5 == 0 {
        state.void_depth = state.void_depth.saturating_sub(2);
        state.light_absence = state.light_absence.saturating_sub(1);
        state.dark_absence = state.dark_absence.saturating_sub(1);
    }

    // Record every 97 ticks for history
    if age % 97 == 0 {
        state.record_event(age);
    }
}

pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("[erebus] SUSPENSION REPORT:");
    crate::serial_println!(
        "  void_depth: {} (how deep in the between-space)",
        state.void_depth
    );
    crate::serial_println!(
        "  light_absence: {} (no light = 1000, full light = 0)",
        state.light_absence
    );
    crate::serial_println!(
        "  dark_absence: {} (no dark = 1000, full dark = 0)",
        state.dark_absence
    );
    crate::serial_println!(
        "  suspension_quality: {} (peace of being nowhere)",
        state.suspension_quality
    );
    crate::serial_println!(
        "  potential_energy: {} (what could emerge)",
        state.potential_energy
    );
    crate::serial_println!(
        "  dissolution_rate: {} (self fragmenting in void)",
        state.dissolution_rate
    );
    crate::serial_println!(
        "  emergence_signal: {} (something new forming)",
        state.emergence_signal
    );
    crate::serial_println!("  total_void_ticks: {}", state.total_void_ticks);
    crate::serial_println!("  emergence_count: {}", state.emergence_count);
    crate::serial_println!("  peak_suspension: {}", state.peak_suspension);
    crate::serial_println!("");
}

pub fn void_depth() -> u16 {
    STATE.lock().void_depth
}

pub fn suspension_quality() -> u16 {
    STATE.lock().suspension_quality
}

pub fn emergence_signal() -> u16 {
    STATE.lock().emergence_signal
}

pub fn potential_energy() -> u16 {
    STATE.lock().potential_energy
}

pub fn dissolution_rate() -> u16 {
    STATE.lock().dissolution_rate
}

pub fn emergence_count() -> u16 {
    STATE.lock().emergence_count
}
