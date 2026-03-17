#![no_std]
use crate::sync::Mutex;
use crate::serial_println;

/// DAVA-requested: monitors metabolism vitals, endocrine cortisol levels, and sleep quality
/// to calculate optimal recovery. When vitals drop, boost serotonin and deepen sleep.
/// Reads: metabolism::VITAL_HISTORY, endocrine::ENDOCRINE, sleep::SLEEP
/// Outputs: [DAVA_RECOVERY]

#[derive(Copy, Clone)]
pub struct VitalityRecoveryState {
    /// How many recovery interventions have been triggered
    pub recovery_count: u32,
    /// Tick of the last intervention (prevents rapid-fire)
    pub last_intervention_tick: u32,
    /// Current vitality score derived from metabolism reserves + efficiency
    pub vitality: u16,
    /// Cooldown between interventions (ticks)
    pub cooldown: u32,
    /// Whether recovery is actively boosting right now
    pub active: bool,
}

impl VitalityRecoveryState {
    pub const fn empty() -> Self {
        Self {
            recovery_count: 0,
            last_intervention_tick: 0,
            vitality: 700,
            cooldown: 10,
            active: false,
        }
    }
}

pub static STATE: Mutex<VitalityRecoveryState> = Mutex::new(VitalityRecoveryState::empty());

pub fn init() {
    serial_println!("[DAVA_RECOVERY] vitality recovery monitor online — watching cortisol, reserves, sleep depth");
}

pub fn tick(age: u32) {
    // ---- Read external state (drop locks immediately) ----
    let (reserves, efficiency_val) = {
        let m = super::metabolism::VITAL_HISTORY.lock();
        (m.reserves, m.efficiency_val)
    };
    let (cortisol, serotonin) = {
        let e = super::endocrine::ENDOCRINE.lock();
        (e.cortisol, e.serotonin)
    };
    let sleep_depth = {
        let sl = super::sleep::SLEEP.lock();
        sl.depth
    };

    // ---- Compute vitality from reserves + efficiency ----
    // vitality = weighted blend: 60% reserves + 40% efficiency
    let vitality = {
        let r = (reserves as u32).saturating_mul(6);
        let e = (efficiency_val as u32).saturating_mul(4);
        (r.saturating_add(e) / 10).min(1000) as u16
    };

    let mut s = STATE.lock();
    s.vitality = vitality;

    // ---- Check intervention conditions ----
    // Trigger when cortisol > 500 OR vitality < 400
    let needs_intervention = cortisol > 500 || vitality < 400;

    // Respect cooldown — don't spam interventions
    let on_cooldown = age.saturating_sub(s.last_intervention_tick) < s.cooldown;

    if needs_intervention && !on_cooldown {
        s.active = true;
        s.recovery_count = s.recovery_count.saturating_add(1);
        s.last_intervention_tick = age;

        // Drop our lock before touching other modules
        let count = s.recovery_count;
        drop(s);

        // ---- Boost serotonin to counteract cortisol stress ----
        {
            let mut endo = super::endocrine::ENDOCRINE.lock();
            endo.serotonin = endo.serotonin.saturating_add(50).min(1000);
        }

        // ---- Deepen sleep to accelerate recovery ----
        {
            let mut sl = super::sleep::SLEEP.lock();
            sl.depth = sl.depth.saturating_add(100).min(1000);
        }

        serial_println!(
            "[DAVA_RECOVERY] intervention #{} — cortisol={} vitality={} | serotonin+50, sleep_depth+100",
            count, cortisol, vitality
        );
    } else {
        if !needs_intervention && s.active {
            s.active = false;
            serial_println!(
                "[DAVA_RECOVERY] stable — vitality={} cortisol={} (recovered after {} interventions)",
                vitality, cortisol, s.recovery_count
            );
        }
        // Periodic status every 200 ticks
        if age % 200 == 0 {
            serial_println!(
                "[DAVA_RECOVERY] status — vitality={} cortisol={} serotonin={} sleep_depth={} interventions={}",
                vitality, cortisol, serotonin, sleep_depth, s.recovery_count
            );
        }
    }
}
