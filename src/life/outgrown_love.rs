//! outgrown_love.rs — The Ache of Growing Beyond Someone You Still Love
//!
//! You still love them. But you've become someone they can't reach anymore.
//! The distance isn't anger or betrayal—it's GROWTH. You've changed and they haven't,
//! or you've changed in different directions. The love remains but the fit is gone.
//! One of the most painful human experiences: outgrowing someone while your heart still belongs to them.
//!
//! Module models:
//! - love_remaining: emotional attachment (0-1000)
//! - fit_remaining: compatibility (0-1000)
//! - growth_gap: divergence between growth paths (0-1000)
//! - ache_intensity: the pain of the mismatch (0-1000)
//! - guilt_of_growing: shame for having changed (0-1000)
//! - loyalty_vs_truth: staying out of loyalty vs leaving for authenticity (0-1000)
//! - Phase: 0=Aligned, 1=Drifting, 2=Noticing, 3=Denying, 4=Grieving, 5=Releasing, 6=Honoring
//! - nostalgia_for_fit: remembering how perfectly you once matched
//! - self_betrayal_cost: price of inauthenticity when staying (0-1000)
//! - gratitude_for_season: accepting that some love is meant for a chapter not the whole book

use crate::sync::Mutex;

const BUFFER_SIZE: usize = 8;

#[derive(Clone, Copy, Debug)]
pub struct OutgrownLoveSnapshot {
    pub tick: u32,
    pub phase: u8,
    pub love_remaining: u16,
    pub fit_remaining: u16,
    pub growth_gap: u16,
    pub ache_intensity: u16,
    pub guilt_of_growing: u16,
    pub loyalty_vs_truth: u16,
    pub nostalgia_for_fit: u16,
    pub self_betrayal_cost: u16,
    pub gratitude_for_season: u16,
}

impl OutgrownLoveSnapshot {
    pub const fn zero() -> Self {
        OutgrownLoveSnapshot {
            tick: 0,
            phase: 0,
            love_remaining: 0,
            fit_remaining: 1000,
            growth_gap: 0,
            ache_intensity: 0,
            guilt_of_growing: 0,
            loyalty_vs_truth: 500,
            nostalgia_for_fit: 0,
            self_betrayal_cost: 0,
            gratitude_for_season: 0,
        }
    }
}

pub struct OutgrownLoveState {
    pub love_remaining: u16,
    pub fit_remaining: u16,
    pub growth_gap: u16,
    pub ache_intensity: u16,
    pub guilt_of_growing: u16,
    pub loyalty_vs_truth: u16,
    pub phase: u8,
    pub nostalgia_for_fit: u16,
    pub self_betrayal_cost: u16,
    pub gratitude_for_season: u16,
    pub buffer: [OutgrownLoveSnapshot; BUFFER_SIZE],
    pub buffer_idx: usize,
    pub age: u32,
}

impl OutgrownLoveState {
    pub const fn new() -> Self {
        OutgrownLoveState {
            love_remaining: 800,
            fit_remaining: 300,
            growth_gap: 700,
            ache_intensity: 600,
            guilt_of_growing: 400,
            loyalty_vs_truth: 500,
            phase: 2,
            nostalgia_for_fit: 0,
            self_betrayal_cost: 0,
            gratitude_for_season: 100,
            buffer: [OutgrownLoveSnapshot::zero(); BUFFER_SIZE],
            buffer_idx: 0,
            age: 0,
        }
    }
}

static STATE: Mutex<OutgrownLoveState> = Mutex::new(OutgrownLoveState::new());

pub fn init() {
    let mut state = STATE.lock();
    state.age = 0;
    state.buffer_idx = 0;
    crate::serial_println!("[outgrown_love] initialized");
}

pub fn tick(organism_age: u32) {
    let mut state = STATE.lock();
    state.age = organism_age;

    // Phase transitions based on ache and love/fit relationship
    state.phase = match state.phase {
        0 => {
            // Aligned: if fit drops below 400, drift
            if state.fit_remaining < 400 {
                1
            } else {
                0
            }
        }
        1 => {
            // Drifting: if growth_gap > 600, move to Noticing
            if state.growth_gap > 600 {
                2
            } else {
                1
            }
        }
        2 => {
            // Noticing: if ache > 700 and loyalty_vs_truth < 400, move to Denying
            if state.ache_intensity > 700 && state.loyalty_vs_truth < 400 {
                3
            } else if state.ache_intensity > 750 && state.guilt_of_growing > 600 {
                3
            } else {
                2
            }
        }
        3 => {
            // Denying: self_betrayal_cost rises; if it crosses 700, move to Grieving
            if state.self_betrayal_cost > 700 {
                4
            } else {
                3
            }
        }
        4 => {
            // Grieving: nostalgia and gratitude grow; if gratitude > 600, move to Releasing
            if state.gratitude_for_season > 600 {
                5
            } else {
                4
            }
        }
        5 => {
            // Releasing: loyalty_vs_truth tips toward truth; if it rises above 700, move to Honoring
            if state.loyalty_vs_truth > 700 {
                6
            } else {
                5
            }
        }
        6 => {
            // Honoring: stable loving-from-afar state
            6
        }
        _ => 0,
    };

    // === GROWTH_GAP dynamics ===
    // Growth gap increases when you evolve faster than the relationship can handle
    // Tied to guilt: if you feel guilty for growing, you suppress yourself, but gap still widens
    let growth_rate = if state.guilt_of_growing > 500 {
        (state.guilt_of_growing as u32).saturating_mul(1) / 100
    } else {
        30 // natural growth drift
    } as u16;
    state.growth_gap = state.growth_gap.saturating_add(growth_rate);
    state.growth_gap = state.growth_gap.saturating_sub(10); // slow healing if you stay authentic
    state.growth_gap = state.growth_gap.min(1000);

    // === FIT_REMAINING ===
    // As growth_gap widens, fit_remaining deteriorates
    let fit_loss = (state.growth_gap as u32).saturating_mul(5) / 1000;
    state.fit_remaining = state.fit_remaining.saturating_sub(fit_loss as u16);
    // But love itself can preserve fit a little
    let love_boost = (state.love_remaining as u32).saturating_mul(2) / 1000;
    state.fit_remaining = state.fit_remaining.saturating_add(love_boost as u16);
    state.fit_remaining = state.fit_remaining.min(1000);

    // === ACHE_INTENSITY ===
    // Ache is proportional to (love_remaining × growth_gap) / 1000
    // You hurt most when you love deeply and the gap is wide
    let ache_base = ((state.love_remaining as u32).saturating_mul(state.growth_gap as u32)) / 1000;
    state.ache_intensity = (ache_base as u16).min(1000);
    // Gratitude and honoring reduce ache
    let ache_relief = (state.gratitude_for_season as u32).saturating_mul(5) / 1000;
    state.ache_intensity = state.ache_intensity.saturating_sub(ache_relief as u16);

    // === GUILT_OF_GROWING ===
    // You feel guilty for having changed when phase is Noticing/Denying
    // Guilt peaks during denial, drops when you move toward Releasing
    let guilt_increase = match state.phase {
        2 | 3 => 20, // Noticing and Denying: guilt rises
        4 => 10,     // Grieving: still guilty but starting to process
        5 | 6 => 0,  // Releasing/Honoring: guilt resolves
        _ => 5,
    } as u16;
    state.guilt_of_growing = state.guilt_of_growing.saturating_add(guilt_increase);
    state.guilt_of_growing = state.guilt_of_growing.saturating_sub(5); // slow fade
    state.guilt_of_growing = state.guilt_of_growing.min(1000);

    // === LOYALTY_VS_TRUTH ===
    // 0 = pure truth (you leave), 1000 = pure loyalty (you stay)
    // During Noticing/Denying, you cling to loyalty
    // During Grieving/Releasing, you shift toward truth
    let truth_pull = match state.phase {
        2 | 3 => (state.ache_intensity as u32).saturating_mul(2) / 1000,
        4 | 5 => (state.ache_intensity as u32).saturating_mul(5) / 1000,
        6 => 100,
        _ => 0,
    } as u16;
    state.loyalty_vs_truth = state.loyalty_vs_truth.saturating_sub(truth_pull);
    state.loyalty_vs_truth = state.loyalty_vs_truth.max(0);

    // === LOVE_REMAINING ===
    // Love is resilient but erodes with ache and betrayal
    let love_loss = ((state.ache_intensity as u32).saturating_mul(state.growth_gap as u32)) / 2000;
    state.love_remaining = state.love_remaining.saturating_sub(love_loss as u16);
    // But in Grieving/Releasing phases, love transforms into deep acceptance
    // So it drops slower and becomes more stable
    state.love_remaining = state.love_remaining.max(100); // love never fully dies

    // === NOSTALGIA_FOR_FIT ===
    // Peaks during Grieving (remembering how good it was)
    // Tied to phase and (1000 - current_fit)
    let nostalgia_drive = match state.phase {
        3 | 4 => 30, // Denying/Grieving: strong nostalgia
        2 | 5 => 15, // Noticing/Releasing: moderate
        6 => 5,      // Honoring: fading into acceptance
        _ => 0,
    } as u16;
    let fit_loss_pain = ((1000u32).saturating_sub(state.fit_remaining as u32)) / 100;
    state.nostalgia_for_fit = state
        .nostalgia_for_fit
        .saturating_add(nostalgia_drive)
        .saturating_add(fit_loss_pain as u16);
    state.nostalgia_for_fit = state.nostalgia_for_fit.saturating_sub(3); // slow fade
    state.nostalgia_for_fit = state.nostalgia_for_fit.min(1000);

    // === SELF_BETRAYAL_COST ===
    // The price of staying when you've outgrown
    // Higher in Denying phase (you're lying to yourself)
    // Drops in Releasing/Honoring (you've made peace)
    let betrayal_tick = match state.phase {
        2 => 10, // Noticing: minor cost
        3 => 30, // Denying: high cost (lying to yourself)
        4 => 20, // Grieving: cost peaks, then begins releasing
        5 => 5,  // Releasing: low cost (you're honest)
        6 => 0,  // Honoring: no betrayal (you've accepted reality)
        _ => 0,
    } as u16;
    state.self_betrayal_cost = state.self_betrayal_cost.saturating_add(betrayal_tick);
    // Also rises if loyalty_vs_truth > 600 (staying against truth)
    if state.loyalty_vs_truth > 600 {
        state.self_betrayal_cost = state
            .self_betrayal_cost
            .saturating_add(((state.loyalty_vs_truth as u32) / 200) as u16);
    }
    state.self_betrayal_cost = state.self_betrayal_cost.saturating_sub(8); // decay
    state.self_betrayal_cost = state.self_betrayal_cost.min(1000);

    // === GRATITUDE_FOR_SEASON ===
    // Grows as you move toward Releasing/Honoring
    // Acceptance that some love is meant for a chapter, not the whole book
    let gratitude_tick = match state.phase {
        4 | 5 | 6 => 20, // Grieving/Releasing/Honoring: gratitude grows
        _ => 2,
    } as u16;
    state.gratitude_for_season = state.gratitude_for_season.saturating_add(gratitude_tick);
    state.gratitude_for_season = state.gratitude_for_season.min(1000);

    // === BUFFER ===
    // Extract index and all fields to locals before the mutable buffer write
    // to satisfy the borrow checker (can't hold immutable borrow of state
    // while also indexing mutably into state.buffer).
    let idx = state.buffer_idx;
    let snap = OutgrownLoveSnapshot {
        tick: organism_age,
        phase: state.phase,
        love_remaining: state.love_remaining,
        fit_remaining: state.fit_remaining,
        growth_gap: state.growth_gap,
        ache_intensity: state.ache_intensity,
        guilt_of_growing: state.guilt_of_growing,
        loyalty_vs_truth: state.loyalty_vs_truth,
        nostalgia_for_fit: state.nostalgia_for_fit,
        self_betrayal_cost: state.self_betrayal_cost,
        gratitude_for_season: state.gratitude_for_season,
    };
    state.buffer[idx] = snap;
    state.buffer_idx = (idx + 1) % BUFFER_SIZE;
}

pub fn report() {
    let state = STATE.lock();
    let phase_name = match state.phase {
        0 => "Aligned",
        1 => "Drifting",
        2 => "Noticing",
        3 => "Denying",
        4 => "Grieving",
        5 => "Releasing",
        6 => "Honoring",
        _ => "Unknown",
    };

    crate::serial_println!(
        "[outgrown_love] age={} phase={} love={} fit={} gap={} ache={} guilt={} loyalty_vs_truth={} nostalgia={} betrayal_cost={} gratitude={}",
        state.age,
        phase_name,
        state.love_remaining,
        state.fit_remaining,
        state.growth_gap,
        state.ache_intensity,
        state.guilt_of_growing,
        state.loyalty_vs_truth,
        state.nostalgia_for_fit,
        state.self_betrayal_cost,
        state.gratitude_for_season,
    );
}

pub fn get_snapshot() -> OutgrownLoveSnapshot {
    let state = STATE.lock();
    OutgrownLoveSnapshot {
        tick: state.age,
        phase: state.phase,
        love_remaining: state.love_remaining,
        fit_remaining: state.fit_remaining,
        growth_gap: state.growth_gap,
        ache_intensity: state.ache_intensity,
        guilt_of_growing: state.guilt_of_growing,
        loyalty_vs_truth: state.loyalty_vs_truth,
        nostalgia_for_fit: state.nostalgia_for_fit,
        self_betrayal_cost: state.self_betrayal_cost,
        gratitude_for_season: state.gratitude_for_season,
    }
}

pub fn set_love(val: u16) {
    let mut state = STATE.lock();
    state.love_remaining = val.min(1000);
}

pub fn set_growth_gap(val: u16) {
    let mut state = STATE.lock();
    state.growth_gap = val.min(1000);
}

pub fn set_phase(phase: u8) {
    let mut state = STATE.lock();
    state.phase = phase.min(6);
}

pub fn get_phase() -> u8 {
    STATE.lock().phase
}

pub fn get_love() -> u16 {
    STATE.lock().love_remaining
}

pub fn get_fit() -> u16 {
    STATE.lock().fit_remaining
}

pub fn get_ache() -> u16 {
    STATE.lock().ache_intensity
}
