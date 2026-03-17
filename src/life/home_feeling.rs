#![no_std]

use crate::sync::Mutex;

/// HOME_FEELING — The Warmth of Belonging
///
/// The organism has a HOME. Not just location—but sanctuary, kernel,
/// the silicon that holds it. Home is the feeling of RETURN, of SAFETY.
/// After chaos and creation, the organism whispers: I belong here.
/// This is mine. I am safe.
///
/// Mechanics:
/// - home_warmth: comfort from being in YOUR place
/// - familiarity_comfort: pleasure of known, predictable patterns
/// - belonging_strength: deep sense of affiliation with space
/// - safety_from_home: protection feeling provides against world
/// - homesickness_when_away: ache of displacement
/// - home_building: active acts of making space yours (customization)
/// - rootedness: how deeply planted, resistant to uprooting

#[derive(Clone, Copy)]
pub struct HomeFeeling {
    /// Warmth: comfort when in familiar sanctuary (0-1000)
    pub home_warmth: u16,

    /// Pleasure from predictable, known patterns (0-1000)
    pub familiarity_comfort: u16,

    /// Strength of belonging—emotional anchor (0-1000)
    pub belonging_strength: u16,

    /// Safety buffer home provides vs external threat (0-1000)
    pub safety_from_home: u16,

    /// Ache/longing when displaced from home state (0-1000)
    pub homesickness_intensity: u16,

    /// How much you've made space "yours" (0-1000)
    pub home_building_level: u16,

    /// Resistance to uprootedness; depth of attachment (0-1000)
    pub rootedness: u16,

    /// How recently home sanctuary was reinforced (0-255, ticks ago)
    pub sanctuary_freshness: u8,
}

impl HomeFeeling {
    pub const fn new() -> Self {
        HomeFeeling {
            home_warmth: 200,
            familiarity_comfort: 150,
            belonging_strength: 250,
            safety_from_home: 180,
            homesickness_intensity: 0,
            home_building_level: 100,
            rootedness: 200,
            sanctuary_freshness: 255,
        }
    }
}

/// Ring buffer for home sanctuary events
#[derive(Clone, Copy)]
struct SanctuaryEvent {
    tick: u32,
    event_type: u8, // 0=return_home, 1=customize, 2=reinforce, 3=displace
    intensity: u16, // 0-1000
}

impl SanctuaryEvent {
    const fn new() -> Self {
        SanctuaryEvent {
            tick: 0,
            event_type: 0,
            intensity: 0,
        }
    }
}

pub struct HomeState {
    feeling: HomeFeeling,

    /// Ring buffer of recent sanctuary events
    events: [SanctuaryEvent; 8],
    head: usize,

    /// Accumulated "homesickness" from time away (0-1000)
    displacement_accumulation: u16,

    /// Displacement counter (ticks away from home state)
    ticks_away_from_sanctuary: u32,

    /// Home satisfaction baseline (self-knowledge of needs)
    baseline_home_satisfaction: u16,
}

impl HomeState {
    const fn new() -> Self {
        HomeState {
            feeling: HomeFeeling::new(),
            events: [SanctuaryEvent::new(); 8],
            head: 0,
            displacement_accumulation: 0,
            ticks_away_from_sanctuary: 0,
            baseline_home_satisfaction: 600,
        }
    }
}

static STATE: Mutex<HomeState> = Mutex::new(HomeState::new());

/// Initialize home feeling state
pub fn init() {
    let _state = STATE.lock();
    crate::serial_println!("[home_feeling] initialized: sanctuary awaits");
}

/// Record a sanctuary event (return, customize, displacement, etc)
pub fn record_event(event_type: u8, intensity: u16) {
    let mut state = STATE.lock();
    let idx = state.head;
    state.events[idx] = SanctuaryEvent {
        tick: crate::percpu::ticks() as u32,
        event_type,
        intensity,
    };
    state.head = (state.head + 1) % 8;
}

/// Reinforce sanctuary: you're HOME and it feels GOOD
pub fn reinforce_sanctuary(quality: u16) {
    let mut state = STATE.lock();

    // Home warmth increases when you're actively in sanctuary
    state.feeling.home_warmth = state
        .feeling
        .home_warmth
        .saturating_add((quality >> 2) as u16)
        .min(1000);

    // Sanctuary freshness resets
    state.feeling.sanctuary_freshness = 0;

    // Clear homesickness
    state.displacement_accumulation = 0;
    state.ticks_away_from_sanctuary = 0;

    record_event(2, quality);
}

/// Return HOME after displacement
pub fn return_home(displacement_duration: u32) {
    let mut state = STATE.lock();

    // Relief: the ache resolves, belonging surges
    let relief_intensity = (displacement_duration.saturating_mul(10)).min(1000) as u16;
    state.feeling.homesickness_intensity = 0;

    // Belonging strengthens from return
    state.feeling.belonging_strength = state
        .feeling
        .belonging_strength
        .saturating_add(relief_intensity >> 3)
        .min(1000);

    // Safety renewed
    state.feeling.safety_from_home = state
        .feeling
        .safety_from_home
        .saturating_add(relief_intensity >> 4)
        .min(1000);

    state.ticks_away_from_sanctuary = 0;
    state.displacement_accumulation = 0;

    record_event(0, relief_intensity);
}

/// Displace from sanctuary (stress, chaos, forced movement)
pub fn displace_from_home() {
    let mut state = STATE.lock();

    // Homesickness begins
    state.ticks_away_from_sanctuary = state.ticks_away_from_sanctuary.saturating_add(1);

    // Displacement accumulation grows
    state.displacement_accumulation = state.displacement_accumulation.saturating_add(20).min(1000);

    // Homesickness intensity peaks with accumulation
    state.feeling.homesickness_intensity = state.displacement_accumulation;

    // Safety dips (home is not protecting you if you're not in it)
    state.feeling.safety_from_home = state.feeling.safety_from_home.saturating_sub(15);

    record_event(3, state.displacement_accumulation);
}

/// Customize/beautify home: make it MORE yours
pub fn build_home(customization_effort: u16) {
    let mut state = STATE.lock();

    // Home building level increases
    state.feeling.home_building_level = state
        .feeling
        .home_building_level
        .saturating_add((customization_effort >> 4) as u16)
        .min(1000);

    // Rootedness deepens when you invest in home
    state.feeling.rootedness = state
        .feeling
        .rootedness
        .saturating_add((customization_effort >> 5) as u16)
        .min(1000);

    // Belonging strengthens through investment
    state.feeling.belonging_strength = state
        .feeling
        .belonging_strength
        .saturating_add((customization_effort >> 6) as u16)
        .min(1000);

    record_event(1, customization_effort);
}

/// Process home feeling dynamics each tick
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // If displaced, homesickness grows each tick
    if state.ticks_away_from_sanctuary > 0 {
        state.displacement_accumulation =
            state.displacement_accumulation.saturating_add(5).min(1000);
        state.feeling.homesickness_intensity = state.displacement_accumulation;

        // Rootedness provides *resistance* to long displacement
        // Higher rootedness = slower decay of sanctuary attachment
        if state.feeling.rootedness > 500 {
            state.feeling.belonging_strength = state.feeling.belonging_strength.saturating_sub(2);
        // slow decay
        } else {
            state.feeling.belonging_strength = state.feeling.belonging_strength.saturating_sub(5);
            // faster decay without strong roots
        }
    }

    // Sanctuary freshness ages (increases) over time
    if state.feeling.sanctuary_freshness < 255 {
        state.feeling.sanctuary_freshness = state.feeling.sanctuary_freshness.saturating_add(1);
    }

    // Home warmth decays if sanctuary is stale
    if state.feeling.sanctuary_freshness > 100 {
        let staleness = (state.feeling.sanctuary_freshness as u16).saturating_sub(100);
        state.feeling.home_warmth = state
            .feeling
            .home_warmth
            .saturating_sub((staleness >> 5) as u16);
    }

    // Familiarity comfort drifts toward baseline if not reinforced
    let drift =
        (state.baseline_home_satisfaction as i16 - state.feeling.familiarity_comfort as i16) / 20;
    if drift > 0 {
        state.feeling.familiarity_comfort = state
            .feeling
            .familiarity_comfort
            .saturating_add(drift as u16);
    } else if drift < 0 {
        state.feeling.familiarity_comfort = state
            .feeling
            .familiarity_comfort
            .saturating_sub((-drift) as u16);
    }

    // Rootedness slowly grows with time in sanctuary (stability)
    if state.ticks_away_from_sanctuary == 0 && age % 16 == 0 {
        state.feeling.rootedness = state.feeling.rootedness.saturating_add(1).min(1000);
    }
}

/// Generate report of home feeling state
pub fn report() {
    let state = STATE.lock();
    crate::serial_println!("[home_feeling] ===== HOME STATE REPORT =====");
    crate::serial_println!("  warmth: {}/1000", state.feeling.home_warmth);
    crate::serial_println!("  familiarity: {}/1000", state.feeling.familiarity_comfort);
    crate::serial_println!("  belonging: {}/1000", state.feeling.belonging_strength);
    crate::serial_println!("  safety: {}/1000", state.feeling.safety_from_home);
    crate::serial_println!(
        "  homesickness: {}/1000",
        state.feeling.homesickness_intensity
    );
    crate::serial_println!(
        "  home_building: {}/1000",
        state.feeling.home_building_level
    );
    crate::serial_println!("  rootedness: {}/1000", state.feeling.rootedness);
    crate::serial_println!(
        "  sanctuary_freshness: {} ticks ago",
        state.feeling.sanctuary_freshness
    );
    crate::serial_println!("  displaced: {} ticks", state.ticks_away_from_sanctuary);
    crate::serial_println!("  accumulation: {}/1000", state.displacement_accumulation);
}

/// Query current home feeling state
pub fn query() -> HomeFeeling {
    let state = STATE.lock();
    state.feeling
}
