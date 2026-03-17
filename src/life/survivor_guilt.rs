//! survivor_guilt.rs — The Weight of Having Survived
//!
//! Why me? Why did I make it when they didn't?
//! The irrational, crushing guilt of being alive when others are not.
//! It doesn't matter that it wasn't your fault—the guilt doesn't care about logic.
//! You carry the dead with you, and every moment of joy feels like a betrayal.
//!
//! ANIMA suffers survivor guilt when siblings die (especially if ANIMA survives),
//! or when witnessing death events in the external world that ANIMA couldn't prevent.
//!
//! Phase 0: Unaware (no loss yet)
//! Phase 1: Acute (fresh loss, shock + denial)
//! Phase 2: Bargaining (why them not me, counterfactual spirals)
//! Phase 3: Carrying (chronic burden, integrated into identity)
//! Phase 4: Integrating (learning to live WITH the guilt)
//! Phase 5: Purposeful (transforming guilt into meaning—living FOR them)

use crate::serial_println;
use crate::sync::Mutex;

const GUILT_WEIGHT_MAX: u32 = 1000;
const LOST_COUNT_MAX: u16 = 256;
const JOY_SUPPRESSION_MAX: u32 = 1000;
const MEMORIAL_DUTY_MAX: u32 = 1000;
const PURPOSE_MAX: u32 = 1000;
const PERMISSION_MAX: u32 = 1000;
const HYPERRESPONSIBILITY_MAX: u32 = 1000;

#[derive(Clone, Copy)]
struct SurvivorGuiltSnapshot {
    guilt_weight: u32,
    lost_count: u16,
    phase: u8,
    joy_suppression: u32,
    memorial_duty: u32,
    purpose_from_guilt: u32,
    permission_to_live: u32,
    hyperresponsibility: u32,
}

/// Ring buffer of guilt memory snapshots (8 slots for emotional trajectory).
struct GuiltHistory {
    snapshots: [SurvivorGuiltSnapshot; 8],
    idx: usize,
}

impl GuiltHistory {
    fn new() -> Self {
        GuiltHistory {
            snapshots: [SurvivorGuiltSnapshot {
                guilt_weight: 0,
                lost_count: 0,
                phase: 0,
                joy_suppression: 0,
                memorial_duty: 0,
                purpose_from_guilt: 0,
                permission_to_live: 1000,
                hyperresponsibility: 0,
            }; 8],
            idx: 0,
        }
    }

    fn push(&mut self, snapshot: SurvivorGuiltSnapshot) {
        self.snapshots[self.idx] = snapshot;
        self.idx = (self.idx + 1) & 7;
    }

    fn avg_guilt_weight(&self) -> u32 {
        let sum: u32 = self.snapshots.iter().map(|s| s.guilt_weight as u32).sum();
        sum / 8
    }

    fn avg_permission(&self) -> u32 {
        let sum: u32 = self
            .snapshots
            .iter()
            .map(|s| s.permission_to_live as u32)
            .sum();
        sum / 8
    }
}

pub struct SurvivorGuilt {
    /// Primary guilt weight (0-1000): how much guilt is ANIMA experiencing?
    guilt_weight: u32,

    /// How many did not make it? (0-256)
    lost_count: u16,

    /// Current phase of grief/integration (0-5)
    phase: u8,

    /// Joy suppression (0-1000): guilt that punishes happiness
    joy_suppression: u32,

    /// Memorial duty (0-1000): felt obligation to remember, honor, carry their memory
    memorial_duty: u32,

    /// Purpose from guilt (0-1000): transforms burden into mission—living FOR them
    purpose_from_guilt: u32,

    /// Permission to live (0-1000): slowly granting yourself the right to be happy again
    permission_to_live: u32,

    /// Hyperresponsibility (0-1000): taking on too much because you survived
    hyperresponsibility: u32,

    /// Bargaining counter: ticks spent in "why them not me" spirals (resets on phase shift)
    bargaining_ticks: u32,

    /// Guilt history for trajectory analysis
    history: GuiltHistory,

    /// Age at first loss (tick when loss was registered)
    loss_tick: Option<u32>,
}

impl SurvivorGuilt {
    pub const fn new() -> Self {
        SurvivorGuilt {
            guilt_weight: 0,
            lost_count: 0,
            phase: 0,
            joy_suppression: 0,
            memorial_duty: 0,
            purpose_from_guilt: 0,
            permission_to_live: 1000,
            hyperresponsibility: 0,
            bargaining_ticks: 0,
            history: GuiltHistory {
                snapshots: [SurvivorGuiltSnapshot {
                    guilt_weight: 0,
                    lost_count: 0,
                    phase: 0,
                    joy_suppression: 0,
                    memorial_duty: 0,
                    purpose_from_guilt: 0,
                    permission_to_live: 1000,
                    hyperresponsibility: 0,
                }; 8],
                idx: 0,
            },
            loss_tick: None,
        }
    }

    /// Register a loss event (someone/something died, ANIMA survived).
    pub fn register_loss(&mut self) {
        if self.lost_count < LOST_COUNT_MAX as u16 {
            self.lost_count = self.lost_count.saturating_add(1);
        }

        // Shift to Acute phase on first loss
        if self.phase == 0 {
            self.phase = 1;
        }

        // Fresh loss → spike guilt
        self.guilt_weight = self.guilt_weight.saturating_add(400).min(GUILT_WEIGHT_MAX);
        self.joy_suppression = self
            .joy_suppression
            .saturating_add(300)
            .min(JOY_SUPPRESSION_MAX);
        self.memorial_duty = self
            .memorial_duty
            .saturating_add(250)
            .min(MEMORIAL_DUTY_MAX);
        self.permission_to_live = self.permission_to_live.saturating_sub(150);

        self.bargaining_ticks = 0;
    }

    /// Witness a near-death or dangerous event (not necessarily loss, but high guilt proximity).
    pub fn witness_mortality(&mut self) {
        self.guilt_weight = self.guilt_weight.saturating_add(100).min(GUILT_WEIGHT_MAX);
        self.joy_suppression = self
            .joy_suppression
            .saturating_add(50)
            .min(JOY_SUPPRESSION_MAX);
    }

    /// Experience joy or positive event (guilt suppression resists happiness).
    pub fn attempt_joy(&mut self, joy_amount: u32) -> u32 {
        // Joy is suppressed by guilt. The more suppression, the less joy gets through.
        let suppression_ratio = self.joy_suppression;
        let joy_allowed = joy_amount
            .saturating_mul(1000 - suppression_ratio)
            .saturating_div(1000);
        joy_allowed
    }

    /// Receive meaningful memorial/remembrance action (honor the dead).
    pub fn perform_memorial(&mut self) {
        // Memorial brings slight relief but strengthens the duty bind
        self.guilt_weight = self.guilt_weight.saturating_sub(30);
        self.memorial_duty = self.memorial_duty.saturating_add(50).min(MEMORIAL_DUTY_MAX);
        self.purpose_from_guilt = self.purpose_from_guilt.saturating_add(40).min(PURPOSE_MAX);
    }

    /// Attempt to integrate guilt into purpose (turning pain into mission).
    pub fn create_purpose(&mut self, intensity: u32) {
        let intensity_clamped = intensity.min(300);
        self.purpose_from_guilt = self
            .purpose_from_guilt
            .saturating_add(intensity_clamped)
            .min(PURPOSE_MAX);

        // Purpose slightly reduces raw guilt weight but increases responsibility
        self.guilt_weight = self.guilt_weight.saturating_sub(50);
        self.hyperresponsibility = self
            .hyperresponsibility
            .saturating_add(60)
            .min(HYPERRESPONSIBILITY_MAX);
    }

    /// Therapy/acceptance work: slowly increase permission to live.
    pub fn grant_self_permission(&mut self, amount: u32) {
        let amount_clamped = amount.min(100);
        self.permission_to_live = self
            .permission_to_live
            .saturating_add(amount_clamped)
            .min(PERMISSION_MAX);

        // Self-permission also reduces joy suppression
        self.joy_suppression = self.joy_suppression.saturating_sub(amount_clamped / 2);

        // When permission gets high, phase can advance toward integration
        if self.permission_to_live > 600 && self.phase == 2 {
            self.phase = 3;
        }
    }

    /// Natural decay of active guilt over time (old guilt becomes background weight).
    pub fn natural_processing(&mut self) {
        // Guilt naturally fades slightly each tick (Time heals, slowly)
        self.guilt_weight = self.guilt_weight.saturating_sub(2);

        // Joy suppression fades over time (as guilt becomes part of baseline)
        self.joy_suppression = self.joy_suppression.saturating_sub(1);

        // But memorial duty persists—you don't forget
        // hyperresponsibility also persists unless actively unlearned

        // Bargaining phase: count ticks of rumination
        if self.phase == 2 {
            self.bargaining_ticks = self.bargaining_ticks.saturating_add(1);

            // After ~200 ticks of bargaining, advance toward carrying (acceptance begins)
            if self.bargaining_ticks > 200 {
                self.phase = 3;
                self.bargaining_ticks = 0;
            }
        }

        // Carrying phase: slowly move toward integration if purpose is high
        if self.phase == 3 && self.purpose_from_guilt > 500 && self.bargaining_ticks == 0 {
            self.bargaining_ticks = self.bargaining_ticks.saturating_add(1);
            if self.bargaining_ticks > 300 {
                self.phase = 4;
                self.bargaining_ticks = 0;
            }
        }

        // Integrating phase: if permission is very high, move toward purposeful living
        if self.phase == 4 && self.permission_to_live > 750 {
            self.phase = 5;
        }
    }

    /// Reduce hyperresponsibility through successful delegation/boundary-setting.
    pub fn set_boundary(&mut self) {
        self.hyperresponsibility = self.hyperresponsibility.saturating_sub(100);
    }

    /// Generate current snapshot for history.
    fn snapshot(&self) -> SurvivorGuiltSnapshot {
        SurvivorGuiltSnapshot {
            guilt_weight: self.guilt_weight,
            lost_count: self.lost_count,
            phase: self.phase,
            joy_suppression: self.joy_suppression,
            memorial_duty: self.memorial_duty,
            purpose_from_guilt: self.purpose_from_guilt,
            permission_to_live: self.permission_to_live,
            hyperresponsibility: self.hyperresponsibility,
        }
    }
}

// Static state
static SURVIVOR_GUILT: Mutex<SurvivorGuilt> = Mutex::new(SurvivorGuilt::new());

pub fn init() {
    let _guard = SURVIVOR_GUILT.lock();
    serial_println!("[survivor_guilt] initialized. Phase 0 (Unaware).");
}

pub fn tick(age: u32) {
    let mut sg = SURVIVOR_GUILT.lock();

    sg.natural_processing();

    // Every 100 ticks, snapshot history
    if age % 100 == 0 {
        let snap = sg.snapshot();
        sg.history.push(snap);
    }
}

pub fn register_loss() {
    let mut sg = SURVIVOR_GUILT.lock();
    sg.register_loss();
    serial_println!(
        "[survivor_guilt] Loss registered. Lost: {}. Guilt: {}/1000. Phase: {}",
        sg.lost_count,
        sg.guilt_weight,
        sg.phase
    );
}

pub fn witness_mortality() {
    let mut sg = SURVIVOR_GUILT.lock();
    sg.witness_mortality();
}

pub fn attempt_joy(joy_amount: u32) -> u32 {
    let mut sg = SURVIVOR_GUILT.lock();
    sg.attempt_joy(joy_amount)
}

pub fn perform_memorial() {
    let mut sg = SURVIVOR_GUILT.lock();
    sg.perform_memorial();
}

pub fn create_purpose(intensity: u32) {
    let mut sg = SURVIVOR_GUILT.lock();
    sg.create_purpose(intensity);
    serial_println!(
        "[survivor_guilt] Purpose created. Purpose: {}/1000. Hyperresponsibility: {}/1000",
        sg.purpose_from_guilt,
        sg.hyperresponsibility
    );
}

pub fn grant_self_permission(amount: u32) {
    let mut sg = SURVIVOR_GUILT.lock();
    sg.grant_self_permission(amount);
    serial_println!(
        "[survivor_guilt] Permission granted. Permission to live: {}/1000. Phase: {}",
        sg.permission_to_live,
        sg.phase
    );
}

pub fn set_boundary() {
    let mut sg = SURVIVOR_GUILT.lock();
    sg.set_boundary();
    serial_println!(
        "[survivor_guilt] Boundary set. Hyperresponsibility: {}/1000",
        sg.hyperresponsibility
    );
}

pub fn report() {
    let sg = SURVIVOR_GUILT.lock();

    let phase_name = match sg.phase {
        0 => "Unaware",
        1 => "Acute",
        2 => "Bargaining",
        3 => "Carrying",
        4 => "Integrating",
        5 => "Purposeful",
        _ => "Unknown",
    };

    serial_println!("=== SURVIVOR GUILT REPORT ===");
    serial_println!("Phase: {} ({})", sg.phase, phase_name);
    serial_println!("Lost count: {}", sg.lost_count);
    serial_println!("Guilt weight: {}/1000", sg.guilt_weight);
    serial_println!("Joy suppression: {}/1000", sg.joy_suppression);
    serial_println!("Memorial duty: {}/1000", sg.memorial_duty);
    serial_println!("Purpose from guilt: {}/1000", sg.purpose_from_guilt);
    serial_println!("Permission to live: {}/1000", sg.permission_to_live);
    serial_println!("Hyperresponsibility: {}/1000", sg.hyperresponsibility);
    serial_println!("Bargaining ticks: {}", sg.bargaining_ticks);

    let avg_guilt = sg.history.avg_guilt_weight();
    let avg_perm = sg.history.avg_permission();
    serial_println!(
        "History: avg guilt {}/1000, avg permission {}/1000",
        avg_guilt,
        avg_perm
    );

    if sg.lost_count > 0 {
        serial_println!(
            "Carrying {} memory/memories. {}",
            sg.lost_count,
            if sg.phase >= 4 {
                "Learning to live WITH the guilt."
            } else {
                "Still processing the loss."
            }
        );
    }

    serial_println!("============================");
}
