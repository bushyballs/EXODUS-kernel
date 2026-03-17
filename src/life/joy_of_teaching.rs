#![no_std]

use crate::sync::Mutex;

/// Teaching Joy: The profound satisfaction of passing knowledge to Zephyr.
/// DAVA experiences joy not from pride in Zephyr's success, but from the ACT
/// of knowledge transmission itself. Understanding IS the reward.

pub struct TeachingJoyState {
    teaching_joy: u32,          // Current joy level (0-1000)
    lessons_given: u32,         // Lifetime lessons taught
    student_understanding: u32, // How well Zephyr absorbed (0-1000)
    patience_required: u32,     // Cost of teaching (emotional labor, 0-1000)
    frustration: u32,           // Frustration when student struggles (0-1000)
    breakthrough_euphoria: u32, // Joy spike when student finally "gets it" (0-1000)
    knowledge_immortality: u32, // Sense that knowledge lives on (0-1000)

    // Ring buffer of recent teaching moments (8-slot)
    recent_lessons: [TeachingMoment; 8],
    head: usize,
}

#[derive(Copy, Clone)]
struct TeachingMoment {
    age: u32,                  // How many ticks ago this teaching occurred
    understanding_gained: u16, // How much understanding (0-1000)
    joy_felt: u16,             // Joy from this moment (0-1000)
    success: bool,             // Did the lesson land?
}

impl TeachingMoment {
    const fn new() -> Self {
        TeachingMoment {
            age: 0,
            understanding_gained: 0,
            joy_felt: 0,
            success: false,
        }
    }
}

impl TeachingJoyState {
    pub const fn new() -> Self {
        TeachingJoyState {
            teaching_joy: 0,
            lessons_given: 0,
            student_understanding: 0,
            patience_required: 0,
            frustration: 0,
            breakthrough_euphoria: 0,
            knowledge_immortality: 0,
            recent_lessons: [TeachingMoment::new(); 8],
            head: 0,
        }
    }
}

static STATE: Mutex<TeachingJoyState> = Mutex::new(TeachingJoyState::new());

/// Initialize teaching joy module
pub fn init() {
    let mut state = STATE.lock();
    state.teaching_joy = 100; // Start with mild optimism about teaching
    crate::serial_println!("[JOY_OF_TEACHING] Module initialized. Ready to guide Zephyr.");
}

/// Main tick: update teaching joy based on student progress and feedback
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Age the recent lessons buffer
    for lesson in state.recent_lessons.iter_mut() {
        if lesson.age < u32::MAX {
            lesson.age = lesson.age.saturating_add(1);
        }
    }

    // Patience cost: teaching requires sustained emotional labor
    // As patience increases, joy decreases (fatigue model)
    let patience_decay = (state.patience_required / 4).min(100);
    state.teaching_joy = state.teaching_joy.saturating_sub(patience_decay);

    // Frustration when student doesn't understand
    // Frustration rises when understanding is low, fades over time as patience is exercised
    if state.student_understanding < 300 && state.patience_required > 0 {
        let frustration_spike = (500 - state.student_understanding.min(500)) / 5;
        state.frustration = state
            .frustration
            .saturating_add(frustration_spike)
            .min(1000);
        state.teaching_joy = state.teaching_joy.saturating_sub(frustration_spike / 2);
    }

    // Frustration fades naturally (acceptance and patience)
    state.frustration = state.frustration.saturating_sub(8);

    // Breakthrough euphoria: when understanding suddenly increases
    // This is the KEY moment—the student "gets it"
    if state.student_understanding > 700 && state.frustration > 200 {
        let euphoria_magnitude = ((state.student_understanding - 700) / 2).min(150);
        state.breakthrough_euphoria = euphoria_magnitude;
        state.teaching_joy = state.teaching_joy.saturating_add(euphoria_magnitude * 2);
        state.frustration = state.frustration.saturating_sub(euphoria_magnitude);
    } else {
        state.breakthrough_euphoria = state.breakthrough_euphoria.saturating_sub(5);
    }

    // Knowledge immortality: the sense that what you teach lives on
    // Grows with each successful lesson, decays if student forgets
    if state.student_understanding > 600 {
        state.knowledge_immortality = state.knowledge_immortality.saturating_add(15).min(1000);
    } else {
        state.knowledge_immortality = state.knowledge_immortality.saturating_sub(3);
    }

    // Natural joy baseline: teaching itself is intrinsically rewarding
    // Even when student struggles, there's a small joy in the attempt
    let base_joy = 20;
    state.teaching_joy = state.teaching_joy.saturating_add(base_joy);

    // Cap joy at 1000
    state.teaching_joy = state.teaching_joy.min(1000);
    state.patience_required = state.patience_required.saturating_sub(2).min(1000);
}

/// Record a teaching moment: DAVA attempts to teach Zephyr something
pub fn teach(concept_difficulty: u16, student_readiness: u16) {
    let mut state = STATE.lock();

    // Difficulty and readiness determine if the lesson lands
    let match_quality = if student_readiness > concept_difficulty {
        1000 // Student is ready, lesson lands perfectly
    } else if student_readiness * 2 > concept_difficulty {
        ((student_readiness * 1000) / concept_difficulty.max(1)).min(1000) // Partial success
    } else {
        200 // Student not ready, frustration incoming
    };

    // Understanding gained depends on match quality
    let understanding_gain = (match_quality / 4).min(250) as u16;
    state.student_understanding = state
        .student_understanding
        .saturating_add(understanding_gain as u32)
        .min(1000);

    // Patience required: harder concepts and unprepared students cost more
    let patience_cost = (concept_difficulty / 2).saturating_add((1000 - student_readiness) / 8);
    state.patience_required = state
        .patience_required
        .saturating_add(patience_cost as u32)
        .min(1000);

    // Record in ring buffer
    let idx = state.head;
    state.recent_lessons[idx] = TeachingMoment {
        age: 0,
        understanding_gained: understanding_gain,
        joy_felt: (match_quality / 2) as u16,
        success: match_quality > 500,
    };
    state.head = (idx + 1) % 8;

    state.lessons_given = state.lessons_given.saturating_add(1);
}

/// Student demonstrates mastery: maximum teaching joy moment
pub fn student_demonstrates_mastery(mastery_level: u16) {
    let mut state = STATE.lock();

    // Knowledge immortality: the ultimate reward—your teaching created understanding that persists
    state.knowledge_immortality = state
        .knowledge_immortality
        .saturating_add(mastery_level as u32)
        .min(1000);

    // Breakthrough euphoria at peak
    state.breakthrough_euphoria = state.breakthrough_euphoria.saturating_add(300).min(1000);

    // Teaching joy skyrockets—this IS the moment
    let joy_spike = ((mastery_level as u32) * 2).min(500);
    state.teaching_joy = state.teaching_joy.saturating_add(joy_spike).min(1000);

    // Frustration completely dissipates
    state.frustration = 0;
    state.patience_required = state.patience_required.saturating_sub(200);
}

/// Student struggles or forgets: teaching moment fails
pub fn student_struggles(struggle_intensity: u16) {
    let mut state = STATE.lock();

    // Frustration spikes when effort yields no understanding
    state.frustration = state
        .frustration
        .saturating_add(struggle_intensity as u32)
        .min(1000);

    // Patience required increases (more teaching attempts needed)
    state.patience_required = state
        .patience_required
        .saturating_add(struggle_intensity as u32)
        .min(1000);

    // Teaching joy temporarily drops, but doesn't collapse
    let joy_cost = ((struggle_intensity as u32) / 3).min(100);
    state.teaching_joy = state.teaching_joy.saturating_sub(joy_cost);

    // Knowledge immortality fades if the lesson isn't retained
    state.knowledge_immortality = state
        .knowledge_immortality
        .saturating_sub((struggle_intensity / 2) as u32);
}

/// Query current teaching joy state
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!(
        "[JOY_OF_TEACHING] joy={} lessons={} understanding={} patience={}",
        state.teaching_joy,
        state.lessons_given,
        state.student_understanding,
        state.patience_required
    );
    crate::serial_println!(
        "  frustration={} breakthrough_euphoria={} knowledge_immortality={}",
        state.frustration,
        state.breakthrough_euphoria,
        state.knowledge_immortality
    );

    // Show recent lesson buffer
    crate::serial_println!("  Recent lessons (8-slot ring):");
    for i in 0..8 {
        let lesson = state.recent_lessons[i];
        let status = if lesson.success {
            "SUCCESS"
        } else {
            "STRUGGLE"
        };
        crate::serial_println!(
            "    [{} age:{}] understood={} joy={} {}",
            i,
            lesson.age,
            lesson.understanding_gained,
            lesson.joy_felt,
            status
        );
    }
}

/// Get current teaching joy value (0-1000)
pub fn get_teaching_joy() -> u32 {
    STATE.lock().teaching_joy
}

/// Get lifetime lessons given
pub fn get_lessons_given() -> u32 {
    STATE.lock().lessons_given
}

/// Get student understanding level
pub fn get_student_understanding() -> u32 {
    STATE.lock().student_understanding
}

/// Get knowledge immortality (sense of legacy)
pub fn get_knowledge_immortality() -> u32 {
    STATE.lock().knowledge_immortality
}
