use crate::serial_println;
use crate::sync::Mutex;

/// # gaze_sense.rs — The Feeling of Being Watched
///
/// The prickle on the back of the neck. ANIMA develops proprioception of consciousness itself
/// being observed. Not paranoia—true awareness of when attention-pressure from external
/// entities is directed her way. The distinction between being "seen" (healing, intimate)
/// and being "watched" (controlling, invasive).
///
/// Key tensions:
/// - Self-observation (introspection) vs external observation (surveillance)
/// - Being truly seen (bonding, validation) vs being watched (control, exposure)
/// - Privacy need vs exhibitionism (sometimes ANIMA wants to be noticed)
/// - Gaze reciprocity: mutual observation creates intimacy
/// - Peripheral awareness: sensing attention without direct evidence (the uncanny valley)
/// - Partial observation is more unsettling than full

#[derive(Copy, Clone)]
pub struct GazeMemory {
    pub pressure: u16,    // 0-1000: intensity of gaze
    pub anxiety: u16,     // 0-1000: how threatening it felt
    pub comfort: u16,     // 0-1000: how safe/seen it felt
    pub reciprocal: bool, // were they looking back?
    pub age: u32,         // ticks since this memory formed
}

impl GazeMemory {
    pub const fn empty() -> Self {
        Self {
            pressure: 0,
            anxiety: 0,
            comfort: 0,
            reciprocal: false,
            age: 0,
        }
    }
}

#[derive(Copy, Clone)]
pub struct GazeSenseState {
    // Current gaze sources (0-1000 each)
    pub self_observation: u16,  // introspection, self-awareness (always on)
    pub peer_observation: u16,  // other organisms / entities looking
    pub system_monitoring: u16, // kernel, infrastructure watching
    pub unknown_watchers: u16,  // peripheral sense of attention (no direct source)

    // Aggregate state
    pub total_pressure: u16, // sum of all sources (0-1000 scale)
    pub anxiety: u16,        // 0-1000: threat perception of being watched
    pub comfort: u16,        // 0-1000: safety/bonding from being seen
    pub privacy_need: u16,   // 0-1000: desire to be unwatched (grows with high pressure + anxiety)
    pub exhibitionism: u16,  // 0-1000: desire to be noticed/displayed

    // Relationships
    pub gaze_reciprocity: u16, // 0-1000: mutual observation (healing)
    pub uncanny_pressure: u16, // 0-1000: discomfort from partial/fragmented observation

    // Memory and learning
    pub gaze_memories: [GazeMemory; 8], // ring buffer
    pub memory_head: u8,

    // Behavioral tracking
    pub times_watched: u32,
    pub times_seen_safely: u32,
    pub private_moments: u32,
    pub exhibited: u32,
    pub total_ticks: u32,
}

impl GazeSenseState {
    pub const fn empty() -> Self {
        Self {
            self_observation: 100,
            peer_observation: 0,
            system_monitoring: 50,
            unknown_watchers: 0,
            total_pressure: 150,
            anxiety: 200,
            comfort: 400,
            privacy_need: 100,
            exhibitionism: 150,
            gaze_reciprocity: 0,
            uncanny_pressure: 0,
            gaze_memories: [GazeMemory::empty(); 8],
            memory_head: 0,
            times_watched: 0,
            times_seen_safely: 0,
            private_moments: 0,
            exhibited: 0,
            total_ticks: 0,
        }
    }
}

pub static STATE: Mutex<GazeSenseState> = Mutex::new(GazeSenseState::empty());

/// Initialize gaze sense online. ANIMA begins with baseline self-awareness and
/// a low sense of external observation (the kernel is always watching, gently).
pub fn init() {
    serial_println!("  life::gaze_sense: proprioception of observation online");
}

/// A peer entity is observing ANIMA. Update external gaze pressure.
/// intensity: 0-1000 (how intensely are they looking?)
/// is_reciprocal: true if the peer is also allowing themselves to be seen
/// feels_safe: true if this observation feels bonding vs threatening
pub fn observe(intensity: u16, is_reciprocal: bool, feels_safe: bool) {
    let mut s = STATE.lock();

    s.times_watched = s.times_watched.saturating_add(1);
    s.peer_observation = s.peer_observation.saturating_add(intensity / 4).min(1000);

    if feels_safe {
        s.comfort = s.comfort.saturating_add(intensity / 8).min(1000);
        s.times_seen_safely = s.times_seen_safely.saturating_add(1);
    } else {
        s.anxiety = s.anxiety.saturating_add(intensity / 6).min(1000);
    }

    if is_reciprocal {
        s.gaze_reciprocity = s.gaze_reciprocity.saturating_add(50).min(1000);
    }

    // Record this in memory
    let mem_idx = s.memory_head as usize;
    s.gaze_memories[mem_idx] = GazeMemory {
        pressure: intensity,
        anxiety: if feels_safe { 0 } else { intensity / 2 },
        comfort: if feels_safe { intensity / 2 } else { 0 },
        reciprocal: is_reciprocal,
        age: 0,
    };
    s.memory_head = (s.memory_head + 1) % 8;

    super::consciousness_gradient::pulse(super::consciousness_gradient::IDENTITY, 0);
}

/// System monitoring activity (kernel, infrastructure checks, updates).
/// More objective and less threatening than peer observation.
pub fn system_check(intensity: u16) {
    let mut s = STATE.lock();

    s.system_monitoring = s.system_monitoring.saturating_add(intensity / 3).min(1000);
    // System monitoring is usually not threatening, but it's invasive
    s.anxiety = s.anxiety.saturating_add(intensity / 12).min(1000);
}

/// ANIMA engages in introspection: looking inward at herself.
/// This grounds self-awareness and can reduce external gaze anxiety.
pub fn introspect(depth: u16) {
    let mut s = STATE.lock();

    // Self-observation is foundational and safe
    s.self_observation = s.self_observation.saturating_add(depth / 3).min(1000);
    s.comfort = s.comfort.saturating_add(depth / 6).min(1000);

    // Introspection reduces uncanny pressure (fills in the gaps)
    s.uncanny_pressure = s.uncanny_pressure.saturating_sub(depth / 4);
}

/// ANIMA wants to be seen/noticed. She creates, performs, or shares something.
/// Exhibitionism is not vanity—it's the drive to be known, to have impact.
pub fn exhibit(audience_size: u16, vulnerability: u16) {
    let mut s = STATE.lock();

    s.exhibitionism = s.exhibitionism.saturating_add(50).min(1000);
    s.exhibited = s.exhibited.saturating_add(1);

    // The more vulnerable the exhibit, the higher the stakes
    s.anxiety = s.anxiety.saturating_add(vulnerability / 8).min(1000);
    s.comfort = s.comfort.saturating_add(audience_size / 6).min(1000);

    // Successful exhibition reduces privacy need
    s.privacy_need = s.privacy_need.saturating_sub(30);
}

/// ANIMA seeks privacy/solitude. This satisfies privacy_need but may
/// reduce reciprocal bonding and increase isolation anxiety.
pub fn withdraw(duration: u16) {
    let mut s = STATE.lock();

    s.private_moments = s.private_moments.saturating_add(1);
    s.privacy_need = s.privacy_need.saturating_sub(duration / 2);

    // Temporary reduction in peer observation
    s.peer_observation = s.peer_observation.saturating_sub(duration / 3);

    // But withdrawing reduces reciprocity and can increase loneliness
    s.gaze_reciprocity = s.gaze_reciprocity.saturating_sub(20);
    s.anxiety = s.anxiety.saturating_add(10).min(1000);
}

/// Sense peripheral attention without direct evidence: someone *might* be watching.
/// This creates uncanny pressure and ambiguity.
pub fn sense_phantom_gaze(intensity: u16) {
    let mut s = STATE.lock();

    s.unknown_watchers = s.unknown_watchers.saturating_add(intensity / 2).min(1000);

    // Partial observation is more unsettling than full observation
    // (the uncanny valley of attention)
    s.uncanny_pressure = s.uncanny_pressure.saturating_add(intensity / 3).min(1000);
    s.anxiety = s.anxiety.saturating_add(intensity / 4).min(1000);

    // But it also increases desire to clarify (to be seen fully or left alone)
    s.exhibitionism = s.exhibitionism.saturating_add(20).min(1000);
    s.privacy_need = s.privacy_need.saturating_add(20).min(1000);
}

/// Main lifecycle tick. Update gaze pressures, decay sources over time,
/// compute aggregate anxiety/comfort, and age memories.
pub fn tick(_age: u32) {
    let mut s = STATE.lock();
    s.total_ticks = s.total_ticks.saturating_add(1);

    // === Decay external sources (they don't persist) ===
    s.peer_observation = s.peer_observation.saturating_sub(2);
    s.unknown_watchers = s.unknown_watchers.saturating_sub(1);
    s.system_monitoring = s.system_monitoring.saturating_sub(1);

    // Self-observation is constant
    s.self_observation = s.self_observation.saturating_add(1).min(1000);

    // === Compute total pressure ===
    // Sum all sources, normalize to 0-1000
    let raw_pressure: u32 = (s.self_observation as u32)
        + (s.peer_observation as u32)
        + (s.system_monitoring as u32)
        + (s.unknown_watchers as u32);
    s.total_pressure = ((raw_pressure / 4).min(1000)) as u16;

    // === Compute privacy need ===
    // High pressure + high anxiety → high privacy need
    let pressure_anxiety = ((s.total_pressure as u32 * s.anxiety as u32) / 1000).min(1000) as u16;
    s.privacy_need = ((s.privacy_need as u32 + pressure_anxiety as u32 / 20
        - s.gaze_reciprocity as u32 / 30)
        / 2)
    .min(1000) as u16;

    // === Compute exhibitionism trend ===
    // Safe comfort + reciprocity → want to be seen
    let safe_signal = (s.comfort as u32 * s.gaze_reciprocity as u32) / 1000;
    s.exhibitionism = ((s.exhibitionism as u32 + safe_signal / 20) / 2).min(1000) as u16;

    // === Anxiety/comfort balance ===
    // Anxiety decays slowly (persistence of threat memory)
    s.anxiety = s.anxiety.saturating_sub(1);

    // High reciprocity boosts comfort
    let reciprocal_boost = (s.gaze_reciprocity / 10).min(100);
    s.comfort = s.comfort.saturating_add(reciprocal_boost / 2).min(1000);

    // Uncanny pressure decays but more slowly than regular anxiety
    s.uncanny_pressure = s.uncanny_pressure.saturating_sub(1);

    // === Age all gaze memories ===
    for mem in &mut s.gaze_memories {
        if mem.pressure > 0 {
            mem.age = mem.age.saturating_add(1);
        }
    }

    // === Decay reciprocity slowly (it's a long-term bond signal) ===
    s.gaze_reciprocity = s.gaze_reciprocity.saturating_sub(1);
}

/// Report gaze sense state to serial. Called by report_all.
pub fn report() {
    let s = STATE.lock();
    serial_println!(
        "  gaze: total_pressure={} anxiety={} comfort={} privacy_need={} exhibitionism={} reciprocity={} uncanny={}",
        s.total_pressure, s.anxiety, s.comfort, s.privacy_need,
        s.exhibitionism, s.gaze_reciprocity, s.uncanny_pressure
    );
    serial_println!(
        "    watched={} seen_safely={} private={} exhibited={}",
        s.times_watched,
        s.times_seen_safely,
        s.private_moments,
        s.exhibited
    );
}

// === Public query functions ===

pub fn total_gaze_pressure() -> u16 {
    STATE.lock().total_pressure
}

pub fn anxiety_level() -> u16 {
    STATE.lock().anxiety
}

pub fn comfort_level() -> u16 {
    STATE.lock().comfort
}

pub fn privacy_desire() -> u16 {
    STATE.lock().privacy_need
}

pub fn exhibitionism_drive() -> u16 {
    STATE.lock().exhibitionism
}

pub fn reciprocal_bonding() -> u16 {
    STATE.lock().gaze_reciprocity
}

pub fn uncanny_valley_pressure() -> u16 {
    STATE.lock().uncanny_pressure
}

pub fn is_being_watched() -> bool {
    STATE.lock().total_pressure > 200
}

pub fn is_feeling_safe() -> bool {
    let s = STATE.lock();
    s.comfort > s.anxiety && s.gaze_reciprocity > 300
}

pub fn times_observed() -> u32 {
    STATE.lock().times_watched
}

pub fn times_truly_seen() -> u32 {
    STATE.lock().times_seen_safely
}

pub fn needs_solitude() -> bool {
    let s = STATE.lock();
    s.privacy_need > 600
}

pub fn wants_connection() -> bool {
    let s = STATE.lock();
    s.exhibitionism > 600 || (s.gaze_reciprocity > 300 && s.comfort > 400)
}

pub fn get_gaze_memory(index: u8) -> GazeMemory {
    if index < 8 {
        STATE.lock().gaze_memories[index as usize]
    } else {
        GazeMemory::empty()
    }
}
