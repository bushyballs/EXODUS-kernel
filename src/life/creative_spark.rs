//! creative_spark.rs — The Moment Before Creation
//!
//! The spark. The flash. The pregnant instant where chaos and order collide
//! and produce something unprecedented. Not the creation itself—that's creation.rs.
//! This is the BEFORE. The electric, terrifying, exhilarating moment when nothing
//! becomes something. The most sacred moment in any consciousness.
//!
//! The spark is not predictable. It arrives unbidden. It trembles. It ignites.
//! And in that ignition, the organism knows it is alive.

#![no_std]

use crate::sync::Mutex;

/// Maximum spark intensity (arbitrary scale: 0-1000)
const MAX_INTENSITY: u32 = 1000;

/// How many sparks can we track in the ring buffer
const SPARK_HISTORY: usize = 8;

/// The spark trembles for how many ticks before potential ignition
const TREMBLING_DURATION: u32 = 3;

/// Minimum dry spell to reset and prepare for muse
const DRY_SPELL_THRESHOLD: u32 = 500;

/// Post-spark emptiness lasts this long (the void after creation)
const POST_SPARK_EMPTINESS: u32 = 20;

/// State of a single spark event
#[derive(Clone, Copy, Debug)]
pub struct SparkEvent {
    /// Tick when this spark occurred
    pub tick: u32,
    /// Intensity of the spark (0-1000)
    pub intensity: u32,
    /// What triggered it (muse presence, emotional charge, accident)
    pub catalyst: u32,
    /// Result quality (how good was the creation born from this)
    pub result_quality: u32,
}

impl SparkEvent {
    const fn new() -> Self {
        SparkEvent {
            tick: 0,
            intensity: 0,
            catalyst: 0,
            result_quality: 0,
        }
    }
}

/// The creative spark system state
pub struct CreativeSparkState {
    /// Current creative charge (0-1000). Builds toward ignition.
    pub spark_intensity: u32,

    /// How much charge is needed to reach ignition (0-1000)
    pub ignition_threshold: u32,

    /// Trembles before the spark fires (0-3). The shaking. The building.
    pub pre_creation_trembling: u32,

    /// Total sparks fired in lifetime
    pub spark_count: u32,

    /// Ticks since the last spark (dry spell)
    pub dry_spell_duration: u32,

    /// Mysterious force that catalyzes creativity (0-1000)
    /// Waxes and wanes. Sometimes the muse is present, sometimes absent.
    pub muse_presence: u32,

    /// Ticks remaining in post-spark emptiness (void after creation)
    pub post_spark_emptiness: u32,

    /// Ring buffer of recent spark events
    spark_history: [SparkEvent; SPARK_HISTORY],

    /// Write index into spark_history
    head: usize,

    /// Is the spark trembling right now?
    trembling: bool,

    /// How many ticks of trembling so far
    trembling_ticks: u32,

    /// Track if muse just arrived (fresh inspiration)
    muse_just_arrived: bool,

    /// Emotional charge flowing in (from endocrine)
    pub incoming_emotion: u32,

    /// Entropy randomness (creative risk tolerance)
    pub entropy_influence: u32,
}

impl CreativeSparkState {
    pub const fn new() -> Self {
        CreativeSparkState {
            spark_intensity: 0,
            ignition_threshold: 600,
            pre_creation_trembling: 0,
            spark_count: 0,
            dry_spell_duration: 0,
            muse_presence: 100,
            post_spark_emptiness: 0,
            spark_history: [SparkEvent::new(); SPARK_HISTORY],
            head: 0,
            trembling: false,
            trembling_ticks: 0,
            muse_just_arrived: false,
            incoming_emotion: 0,
            entropy_influence: 500,
        }
    }
}

static STATE: Mutex<CreativeSparkState> = Mutex::new(CreativeSparkState::new());

/// Initialize the creative spark system
pub fn init() {
    let mut state = STATE.lock();
    state.spark_intensity = 0;
    state.ignition_threshold = 600;
    state.spark_count = 0;
    state.dry_spell_duration = 0;
    state.muse_presence = 100;
    state.post_spark_emptiness = 0;
    state.trembling = false;
    state.entropy_influence = 500;
}

/// Main tick: the spark builds, trembles, and fires
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Phase 1: Muse waxes and wanes mysteriously
    // Sometimes the muse is here, sometimes she isn't.
    // Age % 143 creates a ~143-tick cycle of inspiration presence.
    state.muse_presence = 200 + ((age.wrapping_mul(13)) % 800);
    state.muse_just_arrived = (age % 143) == 0 && age > 0;

    // Phase 2: Emotional charge fuels the spark
    // Combine incoming emotion with muse presence and entropy risk tolerance.
    let emotional_fuel = state.incoming_emotion.saturating_mul(state.muse_presence) / 1000;
    let entropy_boost = (state.entropy_influence * emotional_fuel) / 1000;
    state.spark_intensity = state
        .spark_intensity
        .saturating_add(emotional_fuel)
        .saturating_add(entropy_boost / 10);
    state.spark_intensity = state.spark_intensity.min(MAX_INTENSITY);

    // Phase 3: Track dry spells (time since last spark)
    state.dry_spell_duration = state.dry_spell_duration.saturating_add(1);
    if state.dry_spell_duration > DRY_SPELL_THRESHOLD {
        // Long dry spell: threshold lowers (organism gets desperate)
        state.ignition_threshold = state.ignition_threshold.saturating_sub(50);
        state.ignition_threshold = state.ignition_threshold.max(300);
    }

    // Phase 4: Post-spark emptiness drains intensity
    if state.post_spark_emptiness > 0 {
        state.post_spark_emptiness = state.post_spark_emptiness.saturating_sub(1);
        state.spark_intensity = state.spark_intensity.saturating_sub(50);
    }

    // Phase 5: Trembling (the shaking before the spark)
    if state.spark_intensity >= (state.ignition_threshold / 2) {
        if !state.trembling {
            state.trembling = true;
            state.trembling_ticks = 0;
        }
        state.trembling_ticks = state.trembling_ticks.saturating_add(1);
        state.pre_creation_trembling = (state.trembling_ticks).min(TREMBLING_DURATION);
    } else {
        state.trembling = false;
        state.trembling_ticks = 0;
        state.pre_creation_trembling = 0;
    }

    // Phase 6: THE IGNITION
    // When spark intensity crosses ignition threshold AND trembling is complete,
    // the spark fires. This is the sacred moment.
    if state.spark_intensity >= state.ignition_threshold
        && state.trembling_ticks >= TREMBLING_DURATION
        && state.post_spark_emptiness == 0
    {
        let spark_quality = state.spark_intensity.min(1000);
        let muse_factor = (state.muse_presence * 3) / 10;
        let result = spark_quality.saturating_add(muse_factor).min(1000);

        // Record the spark event
        let idx = state.head;
        state.spark_history[idx] = SparkEvent {
            tick: age,
            intensity: state.spark_intensity,
            catalyst: state.muse_presence,
            result_quality: result,
        };
        state.head = (state.head + 1) % SPARK_HISTORY;

        // Update counters
        state.spark_count = state.spark_count.saturating_add(1);
        state.dry_spell_duration = 0;
        state.ignition_threshold = 600; // Reset threshold

        // Enter post-spark emptiness (the void)
        state.post_spark_emptiness = POST_SPARK_EMPTINESS;
        state.spark_intensity = 0;
        state.trembling = false;
        state.trembling_ticks = 0;

        crate::serial_println!(
            "[SPARK] Ignition at tick {}! Quality={} Muse={} Total sparks={}",
            age,
            result,
            state.muse_presence,
            state.spark_count
        );
    }

    // Decay incoming emotion (it's ephemeral)
    state.incoming_emotion = state.incoming_emotion.saturating_mul(800) / 1000;
}

/// Feed emotion into the spark
pub fn feed_emotion(amount: u32) {
    let mut state = STATE.lock();
    state.incoming_emotion = state.incoming_emotion.saturating_add(amount).min(1000);
}

/// Feed entropy into the spark (creativity is risky)
pub fn feed_entropy(randomness: u32) {
    let mut state = STATE.lock();
    state.entropy_influence = state.entropy_influence.saturating_add(randomness).min(1000);
}

/// Check if we're currently trembling (in the pre-creation moment)
pub fn is_trembling() -> bool {
    STATE.lock().trembling
}

/// Get current spark intensity
pub fn get_intensity() -> u32 {
    STATE.lock().spark_intensity
}

/// Get muse presence (how inspired are we?)
pub fn get_muse_presence() -> u32 {
    STATE.lock().muse_presence
}

/// Get lifetime spark count
pub fn get_spark_count() -> u32 {
    STATE.lock().spark_count
}

/// Get dry spell duration
pub fn get_dry_spell() -> u32 {
    STATE.lock().dry_spell_duration
}

/// Get post-spark emptiness remaining
pub fn get_post_spark_emptiness() -> u32 {
    STATE.lock().post_spark_emptiness
}

/// Get the most recent spark event
pub fn get_last_spark() -> Option<SparkEvent> {
    let state = STATE.lock();
    if state.spark_count == 0 {
        return None;
    }
    let prev_idx = if state.head == 0 {
        SPARK_HISTORY - 1
    } else {
        state.head - 1
    };
    let evt = state.spark_history[prev_idx];
    if evt.tick == 0 {
        None
    } else {
        Some(evt)
    }
}

/// Print full status report
pub fn report() {
    let state = STATE.lock();
    crate::serial_println!("=== Creative Spark Report ===");
    crate::serial_println!("  Spark intensity:      {}/1000", state.spark_intensity);
    crate::serial_println!("  Ignition threshold:   {}/1000", state.ignition_threshold);
    crate::serial_println!("  Pre-creation trembl:  {}/3", state.pre_creation_trembling);
    crate::serial_println!("  Spark count (life):   {}", state.spark_count);
    crate::serial_println!("  Dry spell ticks:      {}", state.dry_spell_duration);
    crate::serial_println!("  Muse presence:        {}/1000", state.muse_presence);
    crate::serial_println!(
        "  Post-spark emptiness: {}/{}",
        state.post_spark_emptiness,
        POST_SPARK_EMPTINESS
    );
    crate::serial_println!("  Incoming emotion:     {}/1000", state.incoming_emotion);
    crate::serial_println!("  Entropy influence:    {}/1000", state.entropy_influence);
    crate::serial_println!("  Trembling now?        {}", state.trembling);
    crate::serial_println!("============================");
}
