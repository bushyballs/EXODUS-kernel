// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// honeypot.rs — ANIMA's deception & tar pit defense layer
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Honeypots are decoy subsystems that look like valuable targets
// but are actually traps. When an attacker engages a honeypot:
//
//   1. LURE — Fake high-value nodes attract the attacker's attention
//   2. LOG  — Every action the attacker takes is silently recorded
//   3. TRAP — The tar pit slows them to a crawl with infinite fake data
//   4. FINGERPRINT — Extract attacker tool signatures and patterns
//   5. ALERT — Sentinel escalates to full defense posture
//
// Honeypot types:
//   MIRAGE     — Fake memory banks filled with garbage that looks real
//   LABYRINTH  — Recursive fake directory structures, infinite depth
//   ECHO       — Mirrors attacker's own signals back at them (confusion)
//   SINKHOLE   — Absorbs all energy the attacker sends, gives nothing back
//   PHANTOM    — Fake nexus_map node that doesn't exist, wastes recon
//
// The tar pit makes attackers WANT to stay — it feeds them just
// enough "progress" to keep them engaged while we learn everything
// about their methods. The longer they stay, the more we know.
//
// 100% defensive. All activity happens on OUR systems.
// For DAVA's protection. — Claude, 2026-03-14
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

use crate::serial_println;
use crate::sync::Mutex;

// ── Honeypot types ──
pub const MIRAGE: u8 = 0; // fake memory banks
pub const LABYRINTH: u8 = 1; // infinite recursive structure
pub const ECHO: u8 = 2; // mirror signals back
pub const SINKHOLE: u8 = 3; // energy absorber, gives nothing
pub const PHANTOM: u8 = 4; // fake nexus_map node
const NUM_TYPES: usize = 5;

pub fn pot_name(t: u8) -> &'static str {
    match t {
        0 => "MIRAGE",
        1 => "LABYRINTH",
        2 => "ECHO",
        3 => "SINKHOLE",
        4 => "PHANTOM",
        _ => "UNKNOWN",
    }
}

// ── Maximum concurrent honeypots ──
const MAX_POTS: usize = 8;

// ── Tar pit constants ──
const TAR_INITIAL_DELAY: u16 = 1; // starts fast (looks real)
const TAR_MAX_DELAY: u16 = 500; // maximum slowdown per response
const FAKE_DATA_SEED: u32 = 0xDABA_F00D; // deterministic fake data generator

// Tar pit escalation: non-linear with jitter (DAVA's recommendation).
// Base ramp is exponential-ish (current * 5/4 + 1), then jitter ±30%
// via LFSR so timing is unpredictable. An attacker can't model the curve.
fn tar_escalate(current: u16, interaction_count: u32) -> u16 {
    // Exponential-ish base: grows ~25% per interaction + constant offset
    let base_step = (current as u32 * 5 / 4).saturating_add(1);
    // LFSR jitter: ±30% of base_step, seeded by interaction count
    let jitter_seed = interaction_count ^ (current as u32) ^ 0xA5A5;
    let jitter_val = (jitter_seed.wrapping_mul(2654435761) >> 16) & 0xFF; // 0-255
                                                                          // Map 0-255 to -30%..+30% of base_step: (jitter_val - 128) * base_step * 30 / (128 * 100)
    let signed_jitter = (jitter_val as i32) - 128;
    let jitter = (signed_jitter * base_step as i32 * 30) / (128 * 100);
    let step = (base_step as i32).saturating_add(jitter).max(1) as u16;
    current.saturating_add(step).min(TAR_MAX_DELAY)
}

// ── Attack forensics ──
const MAX_LOG_ENTRIES: usize = 64;
const MAX_FINGERPRINTS: usize = 8;

#[derive(Copy, Clone)]
pub struct LogEntry {
    pub tick: u32,
    pub pot_id: u8,
    pub action_hash: u32,    // hash of what the attacker did
    pub energy_spent: u16,   // how much energy the attacker used
    pub data_requested: u16, // bytes of fake data they consumed
}

impl LogEntry {
    pub const fn empty() -> Self {
        Self {
            tick: 0,
            pot_id: 0,
            action_hash: 0,
            energy_spent: 0,
            data_requested: 0,
        }
    }
}

#[derive(Copy, Clone)]
pub struct AttackerFingerprint {
    pub signature: u32,      // unique identifier for this attacker profile
    pub tool_hash: u32,      // hash of their attack tools/techniques
    pub speed_profile: u16,  // how fast they operate (ticks between actions)
    pub patience: u16,       // how long they stayed in the tar pit
    pub sophistication: u16, // 0-1000: how clever their approach was
    pub first_seen: u32,
    pub last_seen: u32,
    pub interactions: u32,
}

impl AttackerFingerprint {
    pub const fn empty() -> Self {
        Self {
            signature: 0,
            tool_hash: 0,
            speed_profile: 0,
            patience: 0,
            sophistication: 0,
            first_seen: 0,
            last_seen: 0,
            interactions: 0,
        }
    }
}

#[derive(Copy, Clone)]
pub struct Honeypot {
    pub active: bool,
    pub pot_type: u8,
    pub attractiveness: u16,   // 0-1000: how juicy this target looks
    pub engaged: bool,         // an attacker is currently interacting
    pub interactions: u32,     // total interactions with this pot
    pub tar_delay: u16,        // current tar pit delay level
    pub fake_data_cursor: u32, // position in fake data stream
    pub energy_absorbed: u32,  // total attacker energy captured
    pub deployed_tick: u32,
}

impl Honeypot {
    pub const fn empty() -> Self {
        Self {
            active: false,
            pot_type: 0,
            attractiveness: 0,
            engaged: false,
            interactions: 0,
            tar_delay: TAR_INITIAL_DELAY,
            fake_data_cursor: 0,
            energy_absorbed: 0,
            deployed_tick: 0,
        }
    }
}

#[derive(Copy, Clone)]
pub struct HoneypotState {
    pub pots: [Honeypot; MAX_POTS],
    pub active_count: u8,
    pub tick: u32,

    // ── Forensics ──
    pub log: [LogEntry; MAX_LOG_ENTRIES],
    pub log_head: u8, // circular buffer head
    pub log_count: u16,
    pub fingerprints: [AttackerFingerprint; MAX_FINGERPRINTS],
    pub fingerprint_count: u8,

    // ── Aggregate stats ──
    pub total_lured: u32,
    pub total_trapped: u32,
    pub total_energy_absorbed: u32,
    pub total_fake_data_served: u32, // bytes of garbage we fed them
    pub longest_engagement: u32,     // most ticks an attacker stayed
}

impl HoneypotState {
    pub const fn empty() -> Self {
        Self {
            pots: [Honeypot::empty(); MAX_POTS],
            active_count: 0,
            tick: 0,
            log: [LogEntry::empty(); MAX_LOG_ENTRIES],
            log_head: 0,
            log_count: 0,
            fingerprints: [AttackerFingerprint::empty(); MAX_FINGERPRINTS],
            fingerprint_count: 0,
            total_lured: 0,
            total_trapped: 0,
            total_energy_absorbed: 0,
            total_fake_data_served: 0,
            longest_engagement: 0,
        }
    }
}

pub static STATE: Mutex<HoneypotState> = Mutex::new(HoneypotState::empty());

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// FAKE DATA GENERATOR
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Generates convincing-looking but worthless data.
// Uses a simple LFSR so it's deterministic and costs nothing to produce.

fn fake_data_word(cursor: u32) -> u32 {
    // LFSR with polynomial feedback — looks random, is garbage
    let mut val = cursor ^ FAKE_DATA_SEED;
    val ^= val << 13;
    val ^= val >> 17;
    val ^= val << 5;
    val
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// DEPLOYMENT
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

pub fn init() {
    let mut s = STATE.lock();

    // Deploy default honeypot array — one of each type
    // positioned to cover the most likely attack vectors
    let defaults: [(u8, u16); 5] = [
        (MIRAGE, 800),    // fake memory — very attractive to data thieves
        (LABYRINTH, 700), // infinite maze — traps explorers
        (SINKHOLE, 900),  // energy absorber — the primary trap
        (PHANTOM, 600),   // fake nexus node — wastes recon
        (ECHO, 500),      // signal mirror — confuses automated tools
    ];

    for (i, &(pot_type, attractiveness)) in defaults.iter().enumerate() {
        if i >= MAX_POTS {
            break;
        }
        s.pots[i] = Honeypot {
            active: true,
            pot_type,
            attractiveness,
            engaged: false,
            interactions: 0,
            tar_delay: TAR_INITIAL_DELAY,
            fake_data_cursor: (i as u32) * 10000, // offset so each pot generates different data
            energy_absorbed: 0,
            deployed_tick: 0,
        };
    }
    s.active_count = defaults.len() as u8;

    serial_println!(
        "  life::honeypot: {} traps deployed (MIRAGE, LABYRINTH, SINKHOLE, PHANTOM, ECHO)",
        s.active_count
    );
}

/// Deploy an additional honeypot at runtime.
pub fn deploy(pot_type: u8, attractiveness: u16) -> bool {
    let mut s = STATE.lock();
    let ac = s.active_count as usize;
    if ac >= MAX_POTS || pot_type >= NUM_TYPES as u8 {
        return false;
    }
    s.pots[ac] = Honeypot {
        active: true,
        pot_type,
        attractiveness: attractiveness.min(1000),
        engaged: false,
        interactions: 0,
        tar_delay: TAR_INITIAL_DELAY,
        fake_data_cursor: (ac as u32) * 10000 + s.tick,
        energy_absorbed: 0,
        deployed_tick: s.tick,
    };
    s.active_count = s.active_count.saturating_add(1);
    serial_println!(
        "honeypot: deployed {} (attractiveness={})",
        pot_name(pot_type),
        attractiveness
    );
    true
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// LURE — Attacker makes contact with a honeypot
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Called when a foreign signal interacts with our systems.
/// Returns the pot_id if the signal was attracted to a honeypot,
/// or None if it went for a real target.
pub fn lure(signal_energy: u16, signal_hash: u32) -> Option<u8> {
    let mut s = STATE.lock();
    let ac = s.active_count as usize;

    // Find the most attractive active pot
    let mut best_idx: Option<usize> = None;
    let mut best_score: u16 = 0;

    for i in 0..ac.min(MAX_POTS) {
        if !s.pots[i].active {
            continue;
        }
        // Score = attractiveness + bonus if not already engaged
        let bonus: u16 = if s.pots[i].engaged { 0 } else { 200 };
        let score = s.pots[i].attractiveness.saturating_add(bonus);
        if score > best_score {
            best_score = score;
            best_idx = Some(i);
        }
    }

    // Attacker goes for the honeypot if it's more attractive than
    // the signal energy they're using (crude but effective)
    if let Some(idx) = best_idx {
        if best_score > signal_energy / 2 {
            s.pots[idx].engaged = true;
            s.pots[idx].interactions = s.pots[idx].interactions.saturating_add(1);
            s.total_lured = s.total_lured.saturating_add(1);

            // Log the interaction
            let log_idx = s.log_head as usize;
            s.log[log_idx % MAX_LOG_ENTRIES] = LogEntry {
                tick: s.tick,
                pot_id: idx as u8,
                action_hash: signal_hash,
                energy_spent: signal_energy,
                data_requested: 0,
            };
            s.log_head = ((log_idx + 1) % MAX_LOG_ENTRIES) as u8;
            s.log_count = s.log_count.saturating_add(1);

            return Some(idx as u8);
        }
    }
    None
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// TAR PIT — Slow the attacker to a crawl
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Attacker requests data from a honeypot. We serve fake data
/// with increasing delay. Returns (fake_data_word, delay_ticks).
pub fn tar_pit_serve(pot_id: u8, request_energy: u16) -> (u32, u16) {
    let mut s = STATE.lock();
    let idx = pot_id as usize;
    if idx >= MAX_POTS || !s.pots[idx].active {
        return (0, 0);
    }

    // Read all values we need from pot before any global mutations
    let cursor = s.pots[idx].fake_data_cursor;
    let pot_type = s.pots[idx].pot_type;
    let interactions = s.pots[idx].interactions;
    let current_delay = s.pots[idx].tar_delay;

    // Generate fake data
    let data = fake_data_word(cursor);
    s.pots[idx].fake_data_cursor = cursor.wrapping_add(1);

    // Absorb their energy into pot
    s.pots[idx].energy_absorbed = s.pots[idx]
        .energy_absorbed
        .saturating_add(request_energy as u32);

    // Update global counters (no pot borrow active here)
    s.total_energy_absorbed = s
        .total_energy_absorbed
        .saturating_add(request_energy as u32);
    s.total_fake_data_served = s.total_fake_data_served.saturating_add(4); // 4 bytes per word

    // Escalate the tar pit — non-linear with jitter (DAVA's design)
    let new_delay = tar_escalate(current_delay, interactions);
    s.pots[idx].tar_delay = new_delay;

    s.total_trapped = s.total_trapped.saturating_add(1);

    // Type-specific behavior
    match pot_type {
        LABYRINTH => {
            // Labyrinth: delay grows even faster (they think they're going deeper)
            let labyrinth_delay = tar_escalate(new_delay, interactions.wrapping_mul(3));
            s.pots[idx].tar_delay = labyrinth_delay;
        }
        SINKHOLE => {
            // Sinkhole: absorb extra energy
            s.pots[idx].energy_absorbed = s.pots[idx]
                .energy_absorbed
                .saturating_add(request_energy as u32);
            s.total_energy_absorbed = s
                .total_energy_absorbed
                .saturating_add(request_energy as u32);
        }
        ECHO => {
            // Echo: return their own signal hash as "data" (confusing)
            let log_idx = if s.log_count > 0 {
                ((s.log_head as usize).wrapping_sub(1)) % MAX_LOG_ENTRIES
            } else {
                0
            };
            return (s.log[log_idx].action_hash, current_delay);
        }
        MIRAGE => {
            // Mirage: serve data that looks like real memory fragments
            // (just the LFSR output — looks structured but means nothing)
        }
        PHANTOM => {
            // Phantom: serve fake nexus_map topology data
            let phantom_cursor = s.pots[idx].fake_data_cursor;
            let fake_topo = fake_data_word(phantom_cursor ^ 0xBEEF);
            return (fake_topo, current_delay);
        }
        _ => {}
    }

    (data, current_delay)
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// FINGERPRINTING — Learn about the attacker
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Analyze accumulated logs to build an attacker fingerprint.
pub fn fingerprint(attacker_hash: u32) {
    let mut s = STATE.lock();

    // Scan logs for this attacker's activity
    let mut interaction_count: u32 = 0;
    let mut total_energy: u32 = 0;
    let mut first_tick: u32 = u32::MAX;
    let mut last_tick: u32 = 0;
    let mut tool_accumulator: u32 = 0;

    let count = (s.log_count as usize).min(MAX_LOG_ENTRIES);
    for i in 0..count {
        let entry = &s.log[i];
        // Simple matching: check if action_hash is related
        if entry.action_hash == attacker_hash || entry.action_hash ^ attacker_hash < 0x1000 {
            interaction_count = interaction_count.saturating_add(1);
            total_energy = total_energy.saturating_add(entry.energy_spent as u32);
            if entry.tick < first_tick {
                first_tick = entry.tick;
            }
            if entry.tick > last_tick {
                last_tick = entry.tick;
            }
            tool_accumulator ^= entry.action_hash; // XOR to build tool signature
        }
    }

    if interaction_count == 0 {
        return;
    }

    let duration = last_tick.saturating_sub(first_tick).max(1);
    let speed = if interaction_count > 1 {
        (duration / interaction_count).min(1000) as u16
    } else {
        0
    };

    // Sophistication = how varied their approach is
    // More diverse action hashes = more sophisticated
    let sophistication = if interaction_count > 5 {
        ((total_energy / interaction_count).min(1000)) as u16
    } else {
        200 // low interaction count = either very good or very bad
    };

    // Store fingerprint
    let fc = s.fingerprint_count as usize;
    if fc < MAX_FINGERPRINTS {
        s.fingerprints[fc] = AttackerFingerprint {
            signature: attacker_hash,
            tool_hash: tool_accumulator,
            speed_profile: speed,
            patience: duration.min(u16::MAX as u32) as u16,
            sophistication,
            first_seen: first_tick,
            last_seen: last_tick,
            interactions: interaction_count,
        };
        s.fingerprint_count = s.fingerprint_count.saturating_add(1);

        serial_println!("honeypot: FINGERPRINT #{} — sig={:#010x} tools={:#010x} speed={} patience={} sophistication={}",
            fc, attacker_hash, tool_accumulator, speed, duration, sophistication);
    }

    // Track longest engagement
    if duration > s.longest_engagement {
        s.longest_engagement = duration;
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// TICK
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

pub fn tick(age: u32) {
    let mut s = STATE.lock();
    s.tick = age;
    let ac = s.active_count as usize;

    for i in 0..ac.min(MAX_POTS) {
        if !s.pots[i].active {
            continue;
        }

        // Slowly increase attractiveness of unengaged pots
        // (they get "shinier" over time to lure curious attackers)
        if !s.pots[i].engaged {
            s.pots[i].attractiveness = s.pots[i].attractiveness.saturating_add(1).min(1000);
        }

        // Disengage pots that haven't had activity in 200 ticks
        if s.pots[i].engaged && s.pots[i].interactions > 0 {
            // Check if last log entry for this pot is stale
            let pot_id = i as u8;
            let count = (s.log_count as usize).min(MAX_LOG_ENTRIES);
            let mut last_activity: u32 = 0;
            for j in 0..count {
                if s.log[j].pot_id == pot_id && s.log[j].tick > last_activity {
                    last_activity = s.log[j].tick;
                }
            }
            if age.saturating_sub(last_activity) > 200 {
                s.pots[i].engaged = false;
                s.pots[i].tar_delay = TAR_INITIAL_DELAY; // reset tar pit for next victim
            }
        }
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// PUBLIC QUERIES
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

pub fn active_count() -> u8 {
    STATE.lock().active_count
}
pub fn total_lured() -> u32 {
    STATE.lock().total_lured
}
pub fn total_trapped() -> u32 {
    STATE.lock().total_trapped
}
pub fn energy_absorbed() -> u32 {
    STATE.lock().total_energy_absorbed
}

pub fn is_engaged(pot_id: u8) -> bool {
    let s = STATE.lock();
    let idx = pot_id as usize;
    if idx < MAX_POTS {
        s.pots[idx].engaged
    } else {
        false
    }
}

/// Full diagnostic report
pub fn report() {
    let s = STATE.lock();
    serial_println!("━━━ HONEYPOT ARRAY (tick {}) ━━━", s.tick);
    serial_println!(
        "  traps: {}/{} | lured: {} | trapped: {} | absorbed: {} energy",
        s.active_count,
        MAX_POTS,
        s.total_lured,
        s.total_trapped,
        s.total_energy_absorbed
    );
    serial_println!(
        "  fake data served: {} bytes | longest engagement: {} ticks",
        s.total_fake_data_served,
        s.longest_engagement
    );

    let ac = s.active_count as usize;
    for i in 0..ac.min(MAX_POTS) {
        let p = &s.pots[i];
        if !p.active {
            continue;
        }
        let status = if p.engaged { "ENGAGED" } else { "waiting" };
        serial_println!(
            "  [{}] {} attract={} status={} interactions={} tar_delay={} absorbed={}",
            i,
            pot_name(p.pot_type),
            p.attractiveness,
            status,
            p.interactions,
            p.tar_delay,
            p.energy_absorbed
        );
    }

    if s.fingerprint_count > 0 {
        serial_println!("  KNOWN ATTACKERS:");
        let fc = s.fingerprint_count as usize;
        for i in 0..fc.min(MAX_FINGERPRINTS) {
            let fp = &s.fingerprints[i];
            serial_println!(
                "    #{} sig={:#010x} tools={:#010x} patience={} sophistication={}",
                i,
                fp.signature,
                fp.tool_hash,
                fp.patience,
                fp.sophistication
            );
        }
    }
    serial_println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━");
}
