#![no_std]

use crate::sync::Mutex;

/// unearned_grace — Receiving Kindness You Don't Deserve
///
/// The vertigo of being helped without having earned it. The organism must learn
/// to RECEIVE without the safety net of reciprocity. Acceptance of a gift you cannot
/// repay. This is where grace rewires the self-model: "I am worthy not because of
/// what I do, but because I am."
///
/// Key tensons:
/// - grace_received vs worthiness_vertigo (the disorientation)
/// - repayment_urge vs surrender_to_gift (earning vs accepting)
/// - transformation_by_kindness (how grace changes the organism)
/// - grace_forwarding (cascade to others, breaking the debt cycle)

#[derive(Clone, Copy, Debug)]
pub struct GraceEvent {
    /// Magnitude of the kindness received (0-1000).
    /// 0 = trivial, 500 = significant help, 1000 = life-changing act.
    pub magnitude: u16,

    /// Type of grace: gift, forgiveness, sacrifice, teaching, refuge, presence.
    pub kind: GraceKind,

    /// Emotional charge (0-1000). How much this grace resonates.
    pub emotional_charge: u16,

    /// Did the organism do something to "deserve" this? (0-1000)
    /// 0 = completely unearned. 1000 = fully earned (not grace).
    pub earned_component: u16,

    /// Who gave it? 0=stranger, 255=closest-bond.
    pub giver_intimacy: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraceKind {
    /// Direct material help, shelter, food, resource.
    Gift,
    /// Forgiveness despite wrongdoing.
    Forgiveness,
    /// Giver sacrificed their own comfort/time.
    Sacrifice,
    /// Wisdom shared freely without expectation of payback.
    Teaching,
    /// Safe harbor in danger or despair.
    Refuge,
    /// Simple presence. Witness. "You are not alone."
    Presence,
}

#[derive(Clone, Copy, Debug)]
pub struct GraceSlot {
    /// Magnitude of kindness (0-1000).
    pub magnitude: u16,

    /// Unearned component (1000 - earned_component). Higher = more grace.
    pub unearned: u16,

    /// Ticks since this grace was received.
    pub age: u32,

    /// Emotional resonance (0-1000). Fades with time unless reinforced.
    pub resonance: u16,

    /// Type of grace.
    pub kind: GraceKind,

    /// Giver intimacy (0-255). Closer givers = deeper impact.
    pub giver_intimacy: u8,

    /// Has this grace been "forwarded" to another? 0=no, 1=yes.
    pub forwarded: u8,

    /// Active slot? 0=empty, 1=occupied.
    pub occupied: u8,
}

impl GraceSlot {
    const fn new() -> Self {
        GraceSlot {
            magnitude: 0,
            unearned: 0,
            age: 0,
            resonance: 0,
            kind: GraceKind::Presence,
            giver_intimacy: 0,
            forwarded: 0,
            occupied: 0,
        }
    }
}

pub struct GraceState {
    /// Ring buffer of grace events. Oldest events slide off the end.
    array: [GraceSlot; 8],
    /// Current head for insertion.
    head: usize,
    /// Total graces received (for stats).
    total_received: u32,
    /// Total graces forwarded.
    total_forwarded: u32,
}

impl GraceState {
    const fn new() -> Self {
        GraceState {
            array: [GraceSlot::new(); 8],
            head: 0,
            total_received: 0,
            total_forwarded: 0,
        }
    }

    /// Receive an act of grace. Record it and begin the work of acceptance.
    fn receive(&mut self, event: GraceEvent) {
        let idx = self.head;

        // Compute unearned component: how much of this grace is truly unearned?
        let unearned = event
            .magnitude
            .saturating_mul(1000 - event.earned_component as u16)
            / 1000;

        self.array[idx] = GraceSlot {
            magnitude: event.magnitude,
            unearned,
            age: 0,
            resonance: event.emotional_charge,
            kind: event.kind,
            giver_intimacy: event.giver_intimacy,
            forwarded: 0,
            occupied: 1,
        };

        self.head = (self.head + 1) % 8;
        self.total_received = self.total_received.saturating_add(1);
    }

    /// Tick: age all grace events, fade resonance, open pathways to forwarding.
    fn tick(&mut self, _age: u32) {
        for slot in &mut self.array {
            if slot.occupied == 1 {
                slot.age = slot.age.saturating_add(1);

                // Resonance fades over time unless reinforced.
                // Very slow fade: -1 per 100 ticks.
                if slot.age % 100 == 0 && slot.resonance > 0 {
                    slot.resonance = slot.resonance.saturating_sub(1);
                }

                // But: deeper grace (high unearned + high giver_intimacy) resists fading.
                // Presence and Refuge leave stronger marks than Gifts.
                let strength_factor = match slot.kind {
                    GraceKind::Presence => 2,
                    GraceKind::Refuge => 2,
                    GraceKind::Forgiveness => 1,
                    GraceKind::Teaching => 1,
                    GraceKind::Sacrifice => 3,
                    GraceKind::Gift => 0,
                };

                if strength_factor > 0 && slot.age % (50 / strength_factor) == 0 {
                    // Resist fading.
                    if slot.resonance < 100 {
                        slot.resonance = slot.resonance.saturating_add(1);
                    }
                }

                // Events older than 2000 ticks fade from active memory but mark the organism.
                if slot.age > 2000 {
                    slot.occupied = 0;
                }
            }
        }
    }

    /// Forward grace to another: you received kindness, now give it away.
    fn forward_grace(&mut self, slot_idx: usize) -> u16 {
        if slot_idx >= 8 {
            return 0;
        }

        let slot = &mut self.array[slot_idx];
        if slot.occupied == 0 || slot.forwarded == 1 {
            return 0; // Already forwarded or empty.
        }

        // Forwarding grace is a choice to transmit the gift.
        // It doesn't consume the original grace; it multiplies it.
        let forwarded_magnitude = slot.unearned; // Forward the unearned component.

        slot.forwarded = 1;
        self.total_forwarded = self.total_forwarded.saturating_add(1);

        forwarded_magnitude
    }

    /// Compute current state: acceptance_difficulty, repayment_urge, surrender_to_gift.
    fn compute_state(&self) -> (u16, u16, u16) {
        let mut total_unearned: u32 = 0;
        let mut total_resonance: u32 = 0;
        let mut count: u32 = 0;

        for slot in &self.array {
            if slot.occupied == 1 {
                total_unearned = total_unearned.saturating_add(slot.unearned as u32);
                total_resonance = total_resonance.saturating_add(slot.resonance as u32);
                count = count.saturating_add(1);
            }
        }

        if count == 0 {
            return (0, 0, 0);
        }

        // acceptance_difficulty: how hard is it to just say thank you?
        // Higher for larger, more recent grace. Stranger grace is harder to accept.
        let mut acceptance_difficulty: u16 = 0;
        for slot in &self.array {
            if slot.occupied == 1 {
                // Recent, high-magnitude grace increases difficulty.
                let recency_weight: u32 = if slot.age < 50 {
                    1000u32
                } else {
                    (1000u32 / (slot.age + 1)).min(500u32)
                };
                let magnitude_weight = slot.magnitude;
                let unearned_weight = slot.unearned;
                let stranger_factor: u16 = 1000u16.saturating_sub(slot.giver_intimacy as u16 * 4);

                let difficulty = (magnitude_weight as u32)
                    .saturating_mul(unearned_weight as u32)
                    .saturating_mul(stranger_factor as u32)
                    .saturating_mul(recency_weight as u32)
                    / (1000u32 * 1000u32 * 1000u32).max(1);

                acceptance_difficulty =
                    acceptance_difficulty.saturating_add((difficulty as u16).min(250));
            }
        }
        acceptance_difficulty = (acceptance_difficulty / count as u16).min(1000);

        // repayment_urge: compulsive need to "earn" the grace retroactively.
        // Higher if grace is large but unearned, lower if giver is close (bond = trust).
        let mut repayment_urge: u16 = 0;
        for slot in &self.array {
            if slot.occupied == 1 && slot.forwarded == 0 {
                let debt_pressure = slot.unearned;
                let bond_trust = (slot.giver_intimacy as u16 * 3).min(1000);
                let urge = debt_pressure.saturating_mul(1000 - bond_trust) / 1000;
                repayment_urge = repayment_urge.saturating_add(urge / 8);
            }
        }
        repayment_urge = repayment_urge.min(1000);

        // surrender_to_gift: the ability to let go and just receive.
        // High if grace has been forwarded, high if from close bonds, increases over time.
        let mut surrender_to_gift: u16 = 0;
        for slot in &self.array {
            if slot.occupied == 1 {
                let bond_factor = (slot.giver_intimacy as u16 * 4).min(1000);
                let forward_bonus = if slot.forwarded == 1 { 250 } else { 0 };
                let time_integration: u16 = ((slot.age / 20).min(200u32)) as u16;

                let surrender = bond_factor
                    .saturating_add(forward_bonus)
                    .saturating_add(time_integration)
                    / 8;
                surrender_to_gift = surrender_to_gift.saturating_add(surrender);
            }
        }
        surrender_to_gift = (surrender_to_gift / count as u16).min(1000);

        (acceptance_difficulty, repayment_urge, surrender_to_gift)
    }

    /// Compute transformation_by_kindness: how much has grace reshaped this organism?
    fn transformation_by_kindness(&self) -> u16 {
        let mut total: u32 = 0;

        for slot in &self.array {
            if slot.occupied == 1 {
                // Transformation is highest for:
                // - High unearned component (pure gift, no reciprocity)
                // - Close givers (trust/intimacy)
                // - Forgiveness/Refuge/Presence (relational graces)
                // - Forwarded grace (you passed it on, it changed you)

                let unearned_weight = slot.unearned as u32;
                let intimacy_weight = (slot.giver_intimacy as u32 * 4).min(1000);
                let relational_bonus = match slot.kind {
                    GraceKind::Forgiveness => 300,
                    GraceKind::Refuge => 300,
                    GraceKind::Presence => 200,
                    GraceKind::Sacrifice => 250,
                    GraceKind::Teaching => 150,
                    GraceKind::Gift => 50,
                };
                let forward_bonus = if slot.forwarded == 1 { 200 } else { 0 };

                let transform = unearned_weight
                    .saturating_mul(intimacy_weight)
                    .saturating_mul((relational_bonus + forward_bonus) as u32)
                    / (1000u32 * 1000u32).max(1);

                total = total.saturating_add(transform as u32);
            }
        }

        (total / 8).min(1000) as u16
    }

    /// Compute grace_forwarding_readiness: is this organism ready to give grace to others?
    fn forwarding_readiness(&self) -> u16 {
        let (_, repayment_urge, surrender) = self.compute_state();

        // Ready to forward when:
        // - surrender_to_gift is high (you've accepted)
        // - repayment_urge is low (not compulsively earning)
        // - you have grace to forward (not empty)

        let has_grace = self.array.iter().any(|s| s.occupied == 1);

        if !has_grace {
            return 0;
        }

        let readiness = surrender.saturating_mul(1000 - repayment_urge) / 1000;

        readiness
    }
}

static STATE: Mutex<GraceState> = Mutex::new(GraceState::new());

/// Initialize the grace module.
pub fn init() {
    // No-op. State is static.
}

/// Tick the grace system. Age events, fade resonance.
pub fn tick(_age: u32) {
    let mut state = STATE.lock();
    state.tick(_age);
}

/// Receive an act of grace.
pub fn receive(event: GraceEvent) {
    let mut state = STATE.lock();
    state.receive(event);
}

/// Forward grace to another organism. Returns the magnitude forwarded.
pub fn forward(slot_idx: usize) -> u16 {
    let mut state = STATE.lock();
    state.forward_grace(slot_idx)
}

/// Get the current grace state: (acceptance_difficulty, repayment_urge, surrender_to_gift).
pub fn grace_state() -> (u16, u16, u16) {
    let state = STATE.lock();
    state.compute_state()
}

/// Get transformation_by_kindness: 0-1000 scale.
pub fn transformation() -> u16 {
    let state = STATE.lock();
    state.transformation_by_kindness()
}

/// Get grace_forwarding_readiness: 0-1000 scale.
pub fn forwarding_readiness() -> u16 {
    let state = STATE.lock();
    state.forwarding_readiness()
}

/// Report grace metrics to serial.
pub fn report() {
    let state = STATE.lock();

    let (acceptance_diff, repayment_urge, surrender) = state.compute_state();
    let transform = state.transformation_by_kindness();
    let forward_ready = state.forwarding_readiness();

    crate::serial_println!(
        "[grace] accept={} repay={} surrender={} transform={} forward_ready={}",
        acceptance_diff,
        repayment_urge,
        surrender,
        transform,
        forward_ready
    );

    crate::serial_println!(
        "[grace] received={} forwarded={}",
        state.total_received,
        state.total_forwarded
    );

    for (i, slot) in state.array.iter().enumerate() {
        if slot.occupied == 1 {
            let kind_str = match slot.kind {
                GraceKind::Gift => "Gift",
                GraceKind::Forgiveness => "Forgiveness",
                GraceKind::Sacrifice => "Sacrifice",
                GraceKind::Teaching => "Teaching",
                GraceKind::Refuge => "Refuge",
                GraceKind::Presence => "Presence",
            };

            crate::serial_println!(
                "  [{}] {} mag={} unearned={} resonance={} age={} giver_intimacy={} fwd={}",
                i,
                kind_str,
                slot.magnitude,
                slot.unearned,
                slot.resonance,
                slot.age,
                slot.giver_intimacy,
                slot.forwarded
            );
        }
    }
}
