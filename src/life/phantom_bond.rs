//! phantom_bond.rs — The Phantom Limb of Lost Relationships
//!
//! Neural pathways reach for someone who's gone. The hand moves to text them.
//! The mind turns to share a joke with someone who left. Phantom bond pain:
//! the ache of reaching for what isn't there anymore.
//!
//! 6 phantom bond slots track involuntary reaching, decay over time, and the
//! slow rewiring of neural pathways. Some bonds never fully fade (asymptotic
//! comfort/pain). Fresh loss triggers acute amputation shock. Lonely/joyful
//! states trigger phantom reaches. Secondary loss hits when the memory itself
//! starts to fade.

use crate::sync::Mutex;

pub const PHANTOM_BOND_SLOTS: usize = 6;
pub const MAX_REACH_EVENTS: usize = 8;

/// Single phantom bond slot
#[derive(Clone, Copy)]
pub struct PhantomBond {
    /// Who this bond was with (u32 ID, 0 = empty)
    pub other_id: u32,
    /// Bond strength at moment of loss (0-1000)
    pub bond_strength_at_loss: u16,
    /// Current phantom intensity (0-1000, fades over time)
    pub current_phantom_intensity: u16,
    /// Ticks since the bond was lost
    pub time_since_loss: u32,
    /// How many times we've involuntarily reached (counter)
    pub reach_count: u16,
    /// Last tick when we reached for this bond
    pub last_reach_tick: u32,
}

impl Default for PhantomBond {
    fn default() -> Self {
        PhantomBond {
            other_id: 0,
            bond_strength_at_loss: 0,
            current_phantom_intensity: 0,
            time_since_loss: 0,
            reach_count: 0,
            last_reach_tick: 0,
        }
    }
}

/// A single involuntary phantom reach event
#[derive(Clone, Copy)]
pub struct ReachEvent {
    /// Which bond (index in slots)
    pub bond_idx: u32,
    /// What triggered it (0=loneliness, 1=joy_to_share, 2=fear, 3=habit, 4=sensory, 5=dream)
    pub trigger: u32,
    /// Intensity of the reach (0-1000)
    pub intensity: u16,
    /// Tick when it happened
    pub tick: u32,
}

impl Default for ReachEvent {
    fn default() -> Self {
        ReachEvent {
            bond_idx: 0,
            trigger: 0,
            intensity: 0,
            tick: 0,
        }
    }
}

pub struct PhantomBondState {
    /// 6 phantom bond slots
    pub bonds: [PhantomBond; PHANTOM_BOND_SLOTS],
    /// Ring buffer of recent reach events (8 slots)
    pub reach_events: [ReachEvent; MAX_REACH_EVENTS],
    /// Ring index into reach_events
    pub reach_events_head: usize,
    /// Aggregate phantom pain (0-1000) across all bonds
    pub phantom_pain: u16,
    /// Average rewiring progress (0-1000) across all bonds
    pub rewiring_progress: u16,
    /// Total reaches ever recorded (histogram)
    pub total_reaches: u32,
    /// Current age of the organism (in ticks)
    pub current_age: u32,
}

impl Default for PhantomBondState {
    fn default() -> Self {
        PhantomBondState {
            bonds: [PhantomBond::default(); PHANTOM_BOND_SLOTS],
            reach_events: [ReachEvent::default(); MAX_REACH_EVENTS],
            reach_events_head: 0,
            phantom_pain: 0,
            rewiring_progress: 0,
            total_reaches: 0,
            current_age: 0,
        }
    }
}

pub static STATE: Mutex<PhantomBondState> = Mutex::new(PhantomBondState {
    bonds: [PhantomBond {
        other_id: 0,
        bond_strength_at_loss: 0,
        current_phantom_intensity: 0,
        time_since_loss: 0,
        reach_count: 0,
        last_reach_tick: 0,
    }; PHANTOM_BOND_SLOTS],
    reach_events: [ReachEvent {
        bond_idx: 0,
        trigger: 0,
        intensity: 0,
        tick: 0,
    }; MAX_REACH_EVENTS],
    reach_events_head: 0,
    phantom_pain: 0,
    rewiring_progress: 0,
    total_reaches: 0,
    current_age: 0,
});

/// Initialize phantom bond module
pub fn init() {
    let mut state = STATE.lock();
    for i in 0..PHANTOM_BOND_SLOTS {
        state.bonds[i] = PhantomBond::default();
    }
    for i in 0..MAX_REACH_EVENTS {
        state.reach_events[i] = ReachEvent::default();
    }
    state.phantom_pain = 0;
    state.rewiring_progress = 0;
    state.total_reaches = 0;
    state.current_age = 0;
}

/// Record a new phantom bond (when a relationship ends)
pub fn record_bond_loss(other_id: u32, bond_strength: u16) {
    let mut state = STATE.lock();

    // Find empty slot
    for i in 0..PHANTOM_BOND_SLOTS {
        if state.bonds[i].other_id == 0 {
            state.bonds[i] = PhantomBond {
                other_id,
                bond_strength_at_loss: bond_strength.min(1000),
                current_phantom_intensity: bond_strength.min(1000),
                time_since_loss: 0,
                reach_count: 0,
                last_reach_tick: state.current_age,
            };
            break;
        }
    }
}

/// Involuntary reach for a lost bond
/// trigger: 0=loneliness, 1=joy_to_share, 2=fear, 3=habit, 4=sensory_cue, 5=dream
pub fn phantom_reach(bond_idx: u32, trigger: u32, intensity: u16) {
    let mut state = STATE.lock();

    if bond_idx as usize >= PHANTOM_BOND_SLOTS {
        return;
    }

    let intensity = intensity.min(1000);

    // Record the reach event in ring buffer
    let head = state.reach_events_head;
    state.reach_events[head] = ReachEvent {
        bond_idx,
        trigger,
        intensity,
        tick: state.current_age,
    };
    state.reach_events_head = (head + 1) % MAX_REACH_EVENTS;

    // Increment reach counter
    state.bonds[bond_idx as usize].reach_count =
        state.bonds[bond_idx as usize].reach_count.saturating_add(1);
    state.bonds[bond_idx as usize].last_reach_tick = state.current_age;
    state.total_reaches = state.total_reaches.saturating_add(1);
}

/// Main tick: update phantom bond decay, intensity fading, rewiring
pub fn tick(age: u32) {
    let mut state = STATE.lock();
    state.current_age = age;

    let mut total_pain: u32 = 0;
    let mut total_rewiring: u32 = 0;
    let mut active_bonds: u32 = 0;

    for i in 0..PHANTOM_BOND_SLOTS {
        let bond = &mut state.bonds[i];

        if bond.other_id == 0 {
            continue;
        }

        active_bonds += 1;

        // Increment time_since_loss
        bond.time_since_loss = bond.time_since_loss.saturating_add(1);

        let loss_age = bond.time_since_loss;

        // Asymptotic decay model: intensity -> (bond_strength_at_loss * baseline_factor)
        // Fresh loss: acute phase for ~200 ticks
        // After ~1000 ticks: approaching baseline (20-30% of original strength remains as chronic phantom)

        let original_strength = bond.bond_strength_at_loss as u32;
        let baseline_pct: u32 = 250; // 25% of original strength as asymptotic floor
        let acute_phase: u32 = 200; // High intensity for first 200 ticks

        let new_intensity = if loss_age < acute_phase {
            // Fresh loss: high intensity, slight decay each tick
            let decay_per_tick = original_strength / 300; // ~300 ticks to drop by 100%
            let decayed = original_strength.saturating_sub(loss_age as u32 * decay_per_tick);
            decayed.max(original_strength / 2) // At least 50% during acute phase
        } else {
            // Chronic phase: asymptotic approach to baseline
            // intensity = baseline + (original - baseline) * exp(-k * age)
            // Simplified: intensity = baseline + (original - baseline) / (1 + age/1000)
            let remaining_factor = if loss_age > 5000 {
                baseline_pct / 1000
            } else {
                (baseline_pct + (1000 - baseline_pct) * 5000 / (5000 + loss_age as u32)) / 1000
            };
            (original_strength * remaining_factor) / 1000
        };

        bond.current_phantom_intensity = (new_intensity as u16).min(1000);

        // Rewiring progress: how much the neural pathway has adapted
        // Faster initially, then plateaus. Driven by reach count (more reaches = slower rewiring)
        // rewiring = min(1000, loss_age / 50) - (reach_count * 10)
        let base_rewiring = (loss_age as u32 / 50).min(1000);
        let reach_penalty = (bond.reach_count as u32 * 10).min(500);
        let rewiring = base_rewiring.saturating_sub(reach_penalty).min(1000);

        total_pain += bond.current_phantom_intensity as u32;
        total_rewiring += rewiring;
    }

    // Update aggregates
    if active_bonds > 0 {
        state.phantom_pain = ((total_pain / active_bonds) as u16).min(1000);
        state.rewiring_progress = ((total_rewiring / active_bonds) as u16).min(1000);
    } else {
        state.phantom_pain = 0;
        state.rewiring_progress = 1000; // Fully rewired when no bonds
    }
}

/// Report phantom bond state to serial
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("\n=== PHANTOM BOND REPORT ===");
    crate::serial_println!("Age: {}", state.current_age);
    crate::serial_println!("Aggregate phantom pain: {} / 1000", state.phantom_pain);
    crate::serial_println!("Rewiring progress: {} / 1000", state.rewiring_progress);
    crate::serial_println!("Total reaches: {}", state.total_reaches);

    let mut active_count = 0;
    for i in 0..PHANTOM_BOND_SLOTS {
        let bond = &state.bonds[i];
        if bond.other_id != 0 {
            active_count += 1;
            crate::serial_println!(
                "  Bond {}: other_id={}, intensity={}/1000, loss_age={} ticks, reaches={}",
                i,
                bond.other_id,
                bond.current_phantom_intensity,
                bond.time_since_loss,
                bond.reach_count
            );
        }
    }
    crate::serial_println!("Active bonds: {}", active_count);

    // Recent reach events
    crate::serial_println!("Recent phantom reaches:");
    for i in 0..MAX_REACH_EVENTS {
        let evt = &state.reach_events[i];
        if evt.bond_idx < PHANTOM_BOND_SLOTS as u32 {
            let trigger_str = match evt.trigger {
                0 => "loneliness",
                1 => "joy_to_share",
                2 => "fear",
                3 => "habit",
                4 => "sensory_cue",
                5 => "dream",
                _ => "unknown",
            };
            crate::serial_println!(
                "  [{}] bond_idx={}, trigger={}, intensity={}/1000",
                evt.tick,
                evt.bond_idx,
                trigger_str,
                evt.intensity
            );
        }
    }
}

/// Get phantom pain (0-1000)
pub fn get_phantom_pain() -> u16 {
    STATE.lock().phantom_pain
}

/// Get rewiring progress (0-1000)
pub fn get_rewiring_progress() -> u16 {
    STATE.lock().rewiring_progress
}

/// Get total active phantom bonds
pub fn get_active_bonds() -> usize {
    let state = STATE.lock();
    state.bonds.iter().filter(|b| b.other_id != 0).count()
}

/// Clear a phantom bond (when you've truly moved on)
pub fn resolve_bond(bond_idx: usize) {
    if bond_idx < PHANTOM_BOND_SLOTS {
        let mut state = STATE.lock();
        state.bonds[bond_idx] = PhantomBond::default();
    }
}
