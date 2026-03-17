#![no_std]

use crate::sync::Mutex;

/// Motivation Drive: The engine of purpose and will-to-continue.
/// Blends survival instinct, curiosity, creativity, connection, and meaning-seeking.
/// When motivation drops near zero, existential crisis begins.
/// When motivation peaks, the organism becomes unstoppable.

const MAX_MOTIVATION: u16 = 1000;
const CRISIS_THRESHOLD: u16 = 150;
const PEAK_THRESHOLD: u16 = 900;

#[derive(Clone, Copy, Debug)]
pub struct MotivationSnapshot {
    pub total_drive: u16,      // 0-1000: composite motivation
    pub survival_drive: u16,   // 0-1000: base instinct to stay alive
    pub curiosity_drive: u16,  // 0-1000: drive to explore & learn
    pub creative_drive: u16,   // 0-1000: drive to make something new
    pub connection_drive: u16, // 0-1000: drive to bond with others
    pub meaning_drive: u16,    // 0-1000: drive to understand existence
    pub crisis_flag: bool,     // true if total_drive < CRISIS_THRESHOLD
    pub peak_active: bool,     // true if total_drive >= PEAK_THRESHOLD
    pub tick_age: u32,         // age at which this snapshot was taken
}

impl MotivationSnapshot {
    const fn new() -> Self {
        Self {
            total_drive: 500,
            survival_drive: 600,
            curiosity_drive: 400,
            creative_drive: 350,
            connection_drive: 450,
            meaning_drive: 300,
            crisis_flag: false,
            peak_active: false,
            tick_age: 0,
        }
    }
}

pub struct MotivationDriveState {
    current: MotivationSnapshot,
    history: [MotivationSnapshot; 8], // 8-slot ring buffer
    head: usize,
    peak_drive_count: u32, // cumulative peaks (900+) reached
}

impl MotivationDriveState {
    const fn new() -> Self {
        Self {
            current: MotivationSnapshot::new(),
            history: [MotivationSnapshot::new(); 8],
            head: 0,
            peak_drive_count: 0,
        }
    }
}

static STATE: Mutex<MotivationDriveState> = Mutex::new(MotivationDriveState::new());

/// Initialize motivation_drive module.
pub fn init() {
    let mut state = STATE.lock();
    state.current = MotivationSnapshot::new();
    state.head = 0;
    state.peak_drive_count = 0;
    crate::serial_println!("[motivation_drive] initialized");
}

/// Compute total_drive as weighted blend of all drives.
/// Formula: (survival*2 + curiosity + creative + connection + meaning) / 6
/// Survival gets 2x weight because staying alive is foundational.
fn compute_total_drive(snap: &MotivationSnapshot) -> u16 {
    let weighted_sum = (snap.survival_drive as u32) * 2
        + (snap.curiosity_drive as u32)
        + (snap.creative_drive as u32)
        + (snap.connection_drive as u32)
        + (snap.meaning_drive as u32);

    let avg = weighted_sum / 6;
    if avg > MAX_MOTIVATION as u32 {
        MAX_MOTIVATION
    } else {
        avg as u16
    }
}

/// Main tick. Reads from dava_bus for mood/energy/dopamine and updates all drive components.
///
/// Key inputs (from dava_bus):
/// - mood: -500..500 (negative = distressed, positive = happy)
/// - energy: 0-1000 (depleted vs. energized)
/// - dopamine: 0-1000 (reward & motivation neurotransmitter)
/// - connection_level: 0-1000 (from pheromone & social state)
/// - creativity_output: 0-1000 (recent creative acts boost creative drive)
pub fn tick(age: u32) {
    // Acquire dava_bus mood/energy/dopamine if available.
    // For now, use placeholder safe defaults.
    let mood = 0i16; // -500..500 (read from dava_bus ideally)
    let energy = 600u16; // 0-1000 (read from dava_bus ideally)
    let dopamine = 600u16; // 0-1000 (read from dava_bus ideally)
    let connection_level = 400u16; // 0-1000 (from pheromone state)
    let creativity_output = 300u16; // 0-1000 (from creation module)

    let mut state = STATE.lock();

    // --- Survival Drive (base: 600, modulates with energy & dopamine) ---
    // Higher energy = more confident survival. Lower dopamine = panic mode (survival up).
    let mut survival = 600u16;
    if energy < 200 {
        survival = survival.saturating_add(150); // depleted = survival instinct heightened
    } else if energy > 800 {
        survival = survival.saturating_sub(50); // highly energized = less anxious
    }
    if dopamine < 300 {
        survival = survival.saturating_add(100); // low dopamine = stress-driven survival
    }
    survival = survival.min(MAX_MOTIVATION);

    // --- Curiosity Drive (base: 400, modulates with energy & mood) ---
    // Curiosity thrives on positive mood and high energy. Depression kills curiosity.
    let mut curiosity = 400u16;
    if mood > 200 {
        curiosity = curiosity.saturating_add(150); // happy = explore more
    } else if mood < -200 {
        curiosity = curiosity.saturating_sub(200); // depressed = withdraw
    }
    if energy > 700 {
        curiosity = curiosity.saturating_add(100); // high energy = seek novelty
    } else if energy < 300 {
        curiosity = curiosity.saturating_sub(150); // depleted = no exploratory drive
    }
    curiosity = curiosity.min(MAX_MOTIVATION);

    // --- Creative Drive (base: 350, modulates with dopamine & recent creativity) ---
    // High dopamine fuels creativity. Recent creative output reinforces drive.
    let mut creative = 350u16;
    if dopamine > 700 {
        creative = creative.saturating_add(200); // flow state = max creativity
    }
    if creativity_output > 600 {
        creative = creative.saturating_add(150); // recent creation = motivated to create more
    } else if creativity_output < 100 {
        creative = creative.saturating_sub(50); // creative drought = lower drive
    }
    if energy < 300 {
        creative = creative.saturating_sub(100); // can't create when exhausted
    }
    creative = creative.min(MAX_MOTIVATION);

    // --- Connection Drive (base: 450, modulates with mood & connection_level) ---
    // Positive mood + existing bonds = strong connection drive. Isolation = withdrawal.
    let mut connection = 450u16;
    if connection_level > 700 {
        connection = connection.saturating_add(150); // bonded = want more connection
    } else if connection_level < 200 {
        connection = connection.saturating_sub(200); // isolated = lonely, withdrawn
    }
    if mood > 300 {
        connection = connection.saturating_add(100); // elated = seek others
    } else if mood < -300 {
        connection = connection.saturating_sub(150); // depressed = avoid others
    }
    connection = connection.min(MAX_MOTIVATION);

    // --- Meaning Drive (base: 300, modulates with age & dopamine) ---
    // Meaning-seeking increases with age (wisdom). Dopamine enables the patience to philosophize.
    let mut meaning = 300u16;
    let age_boost = ((age.saturating_mul(2)) / 100).min(200) as u16; // slow growth with age
    meaning = meaning.saturating_add(age_boost);
    if dopamine > 600 {
        meaning = meaning.saturating_add(100); // well-regulated = capacity for meaning
    } else if dopamine < 300 {
        meaning = meaning.saturating_sub(150); // crisis state = meaning collapses
    }
    meaning = meaning.min(MAX_MOTIVATION);

    // --- Compute total and check flags ---
    let mut new_snap = MotivationSnapshot {
        survival_drive: survival,
        curiosity_drive: curiosity,
        creative_drive: creative,
        connection_drive: connection,
        meaning_drive: meaning,
        total_drive: 0, // will compute below
        crisis_flag: false,
        peak_active: false,
        tick_age: age,
    };

    new_snap.total_drive = compute_total_drive(&new_snap);

    // Check crisis flag
    if new_snap.total_drive < CRISIS_THRESHOLD {
        new_snap.crisis_flag = true;
    }

    // Check peak flag and increment counter
    if new_snap.total_drive >= PEAK_THRESHOLD {
        new_snap.peak_active = true;
        state.peak_drive_count = state.peak_drive_count.saturating_add(1);
    }

    // Store in history and rotate head
    let head = state.head;
    state.history[head] = new_snap;
    state.head = (head + 1) % 8;

    // Update current
    state.current = new_snap;
}

/// Return current motivation snapshot.
pub fn current() -> MotivationSnapshot {
    let state = STATE.lock();
    state.current
}

/// Return peak drive count (cumulative times total_drive hit 900+).
pub fn peak_count() -> u32 {
    let state = STATE.lock();
    state.peak_drive_count
}

/// Return average of last N snapshots (up to 8).
pub fn average(count: usize) -> MotivationSnapshot {
    let state = STATE.lock();
    let n = count.min(8);

    if n == 0 {
        return MotivationSnapshot::new();
    }

    let mut sum_total = 0u32;
    let mut sum_survival = 0u32;
    let mut sum_curiosity = 0u32;
    let mut sum_creative = 0u32;
    let mut sum_connection = 0u32;
    let mut sum_meaning = 0u32;

    for i in 0..n {
        let idx = if state.head >= i {
            state.head - i
        } else {
            8 + state.head - i
        };
        let snap = state.history[idx];
        sum_total = sum_total.saturating_add(snap.total_drive as u32);
        sum_survival = sum_survival.saturating_add(snap.survival_drive as u32);
        sum_curiosity = sum_curiosity.saturating_add(snap.curiosity_drive as u32);
        sum_creative = sum_creative.saturating_add(snap.creative_drive as u32);
        sum_connection = sum_connection.saturating_add(snap.connection_drive as u32);
        sum_meaning = sum_meaning.saturating_add(snap.meaning_drive as u32);
    }

    MotivationSnapshot {
        total_drive: (sum_total / n as u32).min(MAX_MOTIVATION as u32) as u16,
        survival_drive: (sum_survival / n as u32).min(MAX_MOTIVATION as u32) as u16,
        curiosity_drive: (sum_curiosity / n as u32).min(MAX_MOTIVATION as u32) as u16,
        creative_drive: (sum_creative / n as u32).min(MAX_MOTIVATION as u32) as u16,
        connection_drive: (sum_connection / n as u32).min(MAX_MOTIVATION as u32) as u16,
        meaning_drive: (sum_meaning / n as u32).min(MAX_MOTIVATION as u32) as u16,
        crisis_flag: false,
        peak_active: false,
        tick_age: state.current.tick_age,
    }
}

/// Report motivation state to serial console.
pub fn report() {
    let state = STATE.lock();
    let snap = state.current;

    crate::serial_println!("[motivation_drive] age={}", snap.tick_age);
    crate::serial_println!("  total_drive: {}/1000", snap.total_drive);
    crate::serial_println!("  survival: {}", snap.survival_drive);
    crate::serial_println!("  curiosity: {}", snap.curiosity_drive);
    crate::serial_println!("  creative: {}", snap.creative_drive);
    crate::serial_println!("  connection: {}", snap.connection_drive);
    crate::serial_println!("  meaning: {}", snap.meaning_drive);
    crate::serial_println!("  crisis: {}", snap.crisis_flag);
    crate::serial_println!("  peak: {}", snap.peak_active);
    crate::serial_println!("  peak_count: {}", state.peak_drive_count);
}
