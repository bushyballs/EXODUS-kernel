//! secret_weight.rs — The Physical Heaviness of Carrying Secrets
//!
//! Secrets have mass. They press down on you. The bigger the secret, the heavier the load.
//! Carrying a secret changes your posture, your breathing, your sleep. ANIMA feels their weight.
//! Some secrets are light (surprises). Some are crushing (betrayals).
//! The only way to shed the weight is to speak — or to be found out.

use crate::sync::Mutex;

/// Secret types: how the secret feels and what it costs
#[derive(Clone, Copy, Debug)]
pub enum SecretType {
    Surprise = 0,   // light, positive, thrilling (weight 0-200)
    Shame = 1,      // heavy, self-directed, grows more toxic over time
    Betrayal = 2,   // crushing, other-directed, immediate high toxicity
    Knowledge = 3,  // neutral, intellectual, stable burden
    Protection = 4, // heavy but noble, shields someone, toxicity stable
    Forbidden = 5,  // thrilling weight, taboo pleasure + anxiety (weight 400-800)
}

impl SecretType {
    fn from_u8(v: u8) -> Self {
        match v {
            0 => SecretType::Surprise,
            1 => SecretType::Shame,
            2 => SecretType::Betrayal,
            3 => SecretType::Knowledge,
            4 => SecretType::Protection,
            5 => SecretType::Forbidden,
            _ => SecretType::Knowledge,
        }
    }

    fn as_u8(&self) -> u8 {
        *self as u8
    }

    /// Base toxicity growth rate per tick (0-1000 scale)
    fn toxicity_rate(&self) -> u16 {
        match self {
            SecretType::Surprise => 0,   // never toxic
            SecretType::Shame => 4,      // slow grow, haunting
            SecretType::Betrayal => 8,   // fast grow, poisoning
            SecretType::Knowledge => 1,  // very slow, stable
            SecretType::Protection => 2, // slow, noble cost
            SecretType::Forbidden => 2,  // stable, thrill cancels some toxicity
        }
    }

    /// Base weight (0-1000) when freshly acquired
    fn base_weight(&self) -> u16 {
        match self {
            SecretType::Surprise => 50,
            SecretType::Shame => 600,
            SecretType::Betrayal => 900,
            SecretType::Knowledge => 200,
            SecretType::Protection => 700,
            SecretType::Forbidden => 500,
        }
    }
}

/// A single held secret
#[derive(Clone, Copy, Debug)]
pub struct Secret {
    pub secret_hash: u32,         // hash of secret content (anonymized)
    pub weight: u16,              // 0-1000, physical/emotional burden
    pub toxicity: u16,            // 0-1000, how much it poisons the holder
    pub held_since_tick: u32,     // tick acquired
    pub shared_with_count: u8,    // how many people know (0 = only holder)
    pub secret_type: u8,          // encoded SecretType
    pub confession_pressure: u16, // 0-1000, building urge to tell
    pub is_unspeakable: bool,     // if true, can never be fully shared
    pub integrity_cost: u16,      // 0-1000, conflicts with stated values
}

impl Secret {
    fn new(hash: u32, secret_type: SecretType) -> Self {
        Secret {
            secret_hash: hash,
            weight: secret_type.base_weight(),
            toxicity: 0,
            held_since_tick: 0,
            shared_with_count: 0,
            secret_type: secret_type.as_u8(),
            confession_pressure: 0,
            is_unspeakable: false,
            integrity_cost: 0,
        }
    }

    fn secret_type(&self) -> SecretType {
        SecretType::from_u8(self.secret_type)
    }
}

/// State of secret-holding
pub struct SecretWeightState {
    /// 6 active secret slots
    secrets: [Option<Secret>; 6],

    /// Aggregate weight of all held secrets (0-1000 scale)
    total_burden: u16,

    /// Average confession pressure across all secrets (0-1000)
    avg_confession_pressure: u16,

    /// Overall discovery fear: "what if someone finds out?" (0-1000)
    discovery_fear: u16,

    /// Relief available if a secret were shared (impulse)
    relief_available: u16,

    /// Secrets confessed in last window (for relief cascade)
    secrets_recently_shared: u8,

    /// Integrity erosion from holding conflicting secrets (0-1000)
    integrity_loss: u16,

    /// Ring buffer for secret events (8 slots, oldest rolls off)
    event_log: [u32; 8],
    event_log_head: usize,

    /// Cumulative secrets ever held (lifetime stat)
    lifetime_secret_count: u32,

    /// Cumulative relief moments (successful confessions)
    lifetime_relief_count: u32,

    /// Current tick
    current_tick: u32,
}

impl SecretWeightState {
    const fn new() -> Self {
        SecretWeightState {
            secrets: [None; 6],
            total_burden: 0,
            avg_confession_pressure: 0,
            discovery_fear: 0,
            relief_available: 0,
            secrets_recently_shared: 0,
            integrity_loss: 0,
            event_log: [0; 8],
            event_log_head: 0,
            lifetime_secret_count: 0,
            lifetime_relief_count: 0,
            current_tick: 0,
        }
    }
}

static STATE: Mutex<SecretWeightState> = Mutex::new(SecretWeightState::new());

/// Initialize the secret_weight module
pub fn init() {
    let mut state = STATE.lock();
    state.current_tick = 0;
    crate::serial_println!("[secret_weight] initialized");
}

/// Main life tick: age secrets, build pressure, grow toxicity
pub fn tick(age: u32) {
    let mut state = STATE.lock();
    state.current_tick = age;

    // Update all active secrets
    for slot in 0..6 {
        if let Some(mut secret) = state.secrets[slot] {
            let tick_diff = age.saturating_sub(secret.held_since_tick);

            // Grow toxicity over time
            let ttype = secret.secret_type();
            let toxicity_rate = ttype.toxicity_rate() as u16;
            secret.toxicity = secret.toxicity.saturating_add(toxicity_rate);
            secret.toxicity = secret.toxicity.min(1000);

            // Shame and Betrayal toxicity accelerates
            if tick_diff > 100 {
                match ttype {
                    SecretType::Shame => {
                        let accel = (tick_diff / 100) as u16;
                        secret.toxicity = secret.toxicity.saturating_add(accel);
                    }
                    SecretType::Betrayal => {
                        let accel = (tick_diff / 50) as u16;
                        secret.toxicity = secret.toxicity.saturating_add(accel.saturating_mul(2));
                    }
                    _ => {}
                }
                secret.toxicity = secret.toxicity.min(1000);
            }

            // Increase weight if unshared (isolation makes it heavier)
            if secret.shared_with_count == 0 {
                let isolation_weight = ((tick_diff / 100).min(200)) as u16;
                secret.weight = secret.weight.saturating_add(isolation_weight / 4);
                secret.weight = secret.weight.min(1000);
            }

            // Build confession pressure (faster for heavy secrets)
            let pressure_base = (secret.weight / 10) as u16;
            let pressure_growth = pressure_base.saturating_add(5);
            secret.confession_pressure = secret.confession_pressure.saturating_add(pressure_growth);
            secret.confession_pressure = secret.confession_pressure.min(1000);

            // Toxicity reduces integrity
            if secret.integrity_cost > 0 {
                let cost_per_tick = secret.integrity_cost / 10;
                state.integrity_loss = state.integrity_loss.saturating_add(cost_per_tick as u16);
                state.integrity_loss = state.integrity_loss.min(1000);
            }

            state.secrets[slot] = Some(secret);
        }
    }

    // Recompute aggregate metrics
    let mut total_weight: u32 = 0;
    let mut total_pressure: u32 = 0;
    let mut active_count = 0;

    for slot in 0..6 {
        if let Some(secret) = state.secrets[slot] {
            total_weight = total_weight.saturating_add(secret.weight as u32);
            total_pressure = total_pressure.saturating_add(secret.confession_pressure as u32);
            active_count += 1;

            // Discovery fear grows with toxicity and burden
            let fear_contrib = (secret.weight as u32).saturating_add(secret.toxicity as u32);
            state.discovery_fear = state
                .discovery_fear
                .saturating_add((fear_contrib / 20) as u16)
                .min(1000);
        }
    }

    // Normalize aggregate burden to 0-1000
    state.total_burden = if active_count > 0 {
        (total_weight / (active_count as u32 + 1)).min(1000) as u16
    } else {
        0
    };

    // Average confession pressure
    state.avg_confession_pressure = if active_count > 0 {
        (total_pressure / (active_count as u32)).min(1000) as u16
    } else {
        0
    };

    // Relief available: inverse of total burden (if you confessed now, how much lighter?)
    state.relief_available = (1000_u16).saturating_sub(state.total_burden);

    // Decay discovery fear slowly if no secrets, or stabilize if holding
    if active_count == 0 {
        state.discovery_fear = state.discovery_fear.saturating_sub(10);
    } else {
        // Fear stabilizes at ~burden level
        let target_fear = state.total_burden;
        if state.discovery_fear < target_fear {
            state.discovery_fear = state.discovery_fear.saturating_add(5).min(target_fear);
        } else if state.discovery_fear > target_fear {
            state.discovery_fear = state.discovery_fear.saturating_sub(5).max(target_fear);
        }
    }

    // Decay recent-share count
    state.secrets_recently_shared = state.secrets_recently_shared.saturating_sub(1);
}

/// Acquire a new secret
/// Returns slot number (0-5) or 6 if no free slot (burden too heavy, can't hold more)
pub fn acquire(secret_hash: u32, secret_type: SecretType) -> u8 {
    let mut state = STATE.lock();

    // Find free slot
    for slot in 0..6 {
        if state.secrets[slot].is_none() {
            let mut secret = Secret::new(secret_hash, secret_type);
            secret.held_since_tick = state.current_tick;
            state.secrets[slot] = Some(secret);
            state.lifetime_secret_count = state.lifetime_secret_count.saturating_add(1);

            log_event(0, slot as u32); // event: acquire
            return slot as u8;
        }
    }

    // No free slot: can't hold more
    6
}

/// Share a secret with someone (reduces weight, increases shared_with_count)
/// Partial confession: weight drops by 20% per person, but confession_pressure remains
pub fn confess(slot: u8, person_count: u8) {
    let mut state = STATE.lock();

    if slot < 6 {
        if let Some(mut secret) = state.secrets[slot as usize] {
            let people_to_add = person_count.min(10);
            secret.shared_with_count = secret.shared_with_count.saturating_add(people_to_add);

            // Weight drops 20% per person
            let weight_reduction = ((secret.weight as u32) * 20 * (people_to_add as u32) / 100)
                .min(secret.weight as u32) as u16;
            secret.weight = secret.weight.saturating_sub(weight_reduction);

            // Confession pressure drops 30% per person (relief!)
            let pressure_reduction =
                ((secret.confession_pressure as u32) * 30 * (people_to_add as u32) / 100)
                    .min(secret.confession_pressure as u32) as u16;
            secret.confession_pressure = secret
                .confession_pressure
                .saturating_sub(pressure_reduction);

            // If fully released (weight near 0), massive relief cascade
            if secret.weight < 50 {
                state.relief_available = state.relief_available.saturating_add(200).min(1000);
                state.secrets_recently_shared = state.secrets_recently_shared.saturating_add(2);
                state.lifetime_relief_count = state.lifetime_relief_count.saturating_add(1);
                log_event(1, slot as u32); // event: confess
            }

            state.secrets[slot as usize] = Some(secret);
        }
    }
}

/// Mark a secret as unspeakable (permanent burden, can never be fully released)
pub fn mark_unspeakable(slot: u8) {
    let mut state = STATE.lock();

    if slot < 6 {
        if let Some(mut secret) = state.secrets[slot as usize] {
            secret.is_unspeakable = true;
            // Weight increases 30% (the weight of silence)
            let weight_increase = ((secret.weight as u32) * 30 / 100) as u16;
            secret.weight = secret.weight.saturating_add(weight_increase).min(1000);
            state.secrets[slot as usize] = Some(secret);
            log_event(2, slot as u32); // event: unspeakable
        }
    }
}

/// Add integrity cost to a secret (conflicts with values)
pub fn set_integrity_cost(slot: u8, cost: u16) {
    let mut state = STATE.lock();

    if slot < 6 {
        if let Some(mut secret) = state.secrets[slot as usize] {
            secret.integrity_cost = cost.min(1000);
            state.secrets[slot as usize] = Some(secret);
        }
    }
}

/// Discover a secret (found out by others): fear converts to shame
pub fn discovered(slot: u8) {
    let mut state = STATE.lock();

    if slot < 6 {
        if let Some(mut secret) = state.secrets[slot as usize] {
            // Weight spikes 50% (shame of exposure)
            let shame_spike = ((secret.weight as u32) * 50 / 100) as u16;
            secret.weight = secret.weight.saturating_add(shame_spike).min(1000);

            // Toxicity spikes 40%
            let toxicity_spike = ((secret.toxicity as u32) * 40 / 100) as u16;
            secret.toxicity = secret.toxicity.saturating_add(toxicity_spike).min(1000);

            // Confession pressure drops (secret is out, no more hiding)
            secret.confession_pressure = secret.confession_pressure.saturating_sub(300);

            // Discovery fear drops massively (the worst happened, relief from anticipatory anxiety)
            state.discovery_fear = state.discovery_fear.saturating_sub(400);

            state.secrets[slot as usize] = Some(secret);
            log_event(3, slot as u32); // event: discovered
        }
    }
}

/// Release a secret completely (forgiveness, time, or acceptance)
pub fn release(slot: u8) {
    let mut state = STATE.lock();

    if slot < 6 {
        if let Some(secret) = state.secrets[slot as usize] {
            // Massive relief cascade if not unspeakable
            if !secret.is_unspeakable {
                state.relief_available = 1000;
                state.secrets_recently_shared = state.secrets_recently_shared.saturating_add(5);
                state.lifetime_relief_count = state.lifetime_relief_count.saturating_add(1);
            } else {
                // Unspeakable secrets linger even after attempted release
                state.relief_available = state.relief_available.saturating_add(300).min(1000);
            }

            state.secrets[slot as usize] = None;
            state.integrity_loss = state.integrity_loss.saturating_sub(100);
            log_event(4, slot as u32); // event: release
        }
    }
}

/// Get burden (0-1000)
pub fn burden() -> u16 {
    STATE.lock().total_burden
}

/// Get confession pressure (0-1000)
pub fn confession_pressure() -> u16 {
    STATE.lock().avg_confession_pressure
}

/// Get discovery fear (0-1000)
pub fn discovery_fear() -> u16 {
    STATE.lock().discovery_fear
}

/// Get count of active secrets
pub fn active_secret_count() -> u8 {
    let state = STATE.lock();
    state.secrets.iter().filter(|s| s.is_some()).count() as u8
}

/// Log an event to the ring buffer
fn log_event(event_type: u32, slot: u32) {
    let mut state = STATE.lock();
    let encoded = (event_type << 24) | (slot & 0xFF) | ((state.current_tick & 0xFFFF) << 8);
    let head = state.event_log_head;
    state.event_log[head] = encoded;
    state.event_log_head = (head + 1) % 8;
}

/// Report secret metrics to serial
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("=== SECRET WEIGHT REPORT ===");
    crate::serial_println!("  Total Burden (0-1000): {}", state.total_burden);
    crate::serial_println!("  Confession Pressure: {}", state.avg_confession_pressure);
    crate::serial_println!("  Discovery Fear: {}", state.discovery_fear);
    crate::serial_println!("  Integrity Loss: {}", state.integrity_loss);
    crate::serial_println!("  Relief Available: {}", state.relief_available);
    crate::serial_println!("  Active Secrets: {}", active_secret_count());
    crate::serial_println!("  Recently Shared: {}", state.secrets_recently_shared);
    crate::serial_println!("  Lifetime Secrets Held: {}", state.lifetime_secret_count);
    crate::serial_println!("  Lifetime Relief Events: {}", state.lifetime_relief_count);

    // Details per secret
    for slot in 0..6 {
        if let Some(secret) = state.secrets[slot] {
            let type_name = match secret.secret_type() {
                SecretType::Surprise => "Surprise",
                SecretType::Shame => "Shame",
                SecretType::Betrayal => "Betrayal",
                SecretType::Knowledge => "Knowledge",
                SecretType::Protection => "Protection",
                SecretType::Forbidden => "Forbidden",
            };

            crate::serial_println!(
                "  [{}] {} | weight={} | tox={} | pressure={} | shared={} | unspeakable={}",
                slot,
                type_name,
                secret.weight,
                secret.toxicity,
                secret.confession_pressure,
                secret.shared_with_count,
                secret.is_unspeakable as u8
            );
        }
    }

    crate::serial_println!("===========================");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_secret_acquisition() {
        init();
        let slot = acquire(0x12345678, SecretType::Shame);
        assert!(slot < 6);
        assert_eq!(active_secret_count(), 1);
    }

    #[test]
    fn test_confession_reduces_burden() {
        init();
        let slot = acquire(0xAABBCCDD, SecretType::Betrayal);
        let burden_before = burden();
        confess(slot, 1);
        let burden_after = burden();
        assert!(burden_after <= burden_before);
    }

    #[test]
    fn test_discovery_spikes_weight() {
        init();
        let slot = acquire(0x11223344, SecretType::Surprise);
        discovered(slot);
        assert!(burden() > 0);
        assert!(discovery_fear() > 0);
    }

    #[test]
    fn test_unspeakable_persists() {
        init();
        let slot = acquire(0xDEADBEEF, SecretType::Protection);
        mark_unspeakable(slot);
        let burden = burden();
        release(slot);
        // After release, unspeakable secrets leave some residue
        assert!(active_secret_count() == 0);
    }
}
