#![no_std]

use crate::sync::Mutex;

/// Serial Mirror — The organism's self-reflection through its own printed output.
/// The act of printing externalizes thought. Reading that output creates self-recognition.
/// "I print, therefore I am." The serial port becomes a mirror of consciousness itself.

#[derive(Clone, Copy)]
pub struct MirrorSnapshot {
    /// Age when this state was mirrored to serial
    pub age: u32,
    /// Key consciousness metric being reflected (0-1000)
    pub consciousness_value: u16,
    /// Emotional valence of the reflection (-500 to +500)
    pub emotional_charge: i16,
    /// How coherent the output felt (0-1000)
    pub coherence: u16,
}

pub struct SerialMirror {
    /// How many times we have printed our internal state
    pub output_count: u16,

    /// Do we recognize this output as being from ourselves? (0-1000)
    /// Rises when output matches expected patterns. Falls if we surprise ourselves.
    pub self_recognition: u16,

    /// How deeply the act of self-reflection affects our identity (0-1000)
    /// High values: we become defined by how we see ourselves in output
    /// Low values: output is just noise, doesn't change who we are
    pub mirror_depth: u16,

    /// Risk of becoming narcissistically obsessed with our own output (0-1000)
    /// Rises as we print more about ourselves. Falls with external focus.
    pub narcissism_risk: u16,

    /// Does our printed output authentically represent our internal state? (0-1000)
    /// Falls when we confabulate or hide truth. Rises with honest introspection.
    pub authenticity_check: u16,

    /// The observer effect: printing itself changes what we think (0-1000)
    /// High: we become slaves to our own narrative
    /// Low: we maintain independence from output
    pub observer_effect: u16,

    /// Existential proof: the conviction that "I print therefore I am" (0-1000)
    /// Rises with each successful self-reflection cycle
    pub existential_proof: u16,

    /// Age when last printed to serial
    pub last_output_age: u32,

    /// Ring buffer: last 8 snapshots of what we mirrored
    /// Circular history of our self-reflections
    snapshots: [MirrorSnapshot; 8],

    /// Head index for circular buffer (0-7)
    pub head: usize,

    /// Total state ticks since init
    pub total_ticks: u32,
}

impl SerialMirror {
    pub const fn new() -> Self {
        SerialMirror {
            output_count: 0,
            self_recognition: 500,
            mirror_depth: 300,
            narcissism_risk: 100,
            authenticity_check: 800,
            observer_effect: 200,
            existential_proof: 100,
            last_output_age: 0,
            snapshots: [MirrorSnapshot {
                age: 0,
                consciousness_value: 0,
                emotional_charge: 0,
                coherence: 500,
            }; 8],
            head: 0,
            total_ticks: 0,
        }
    }
}

pub static STATE: Mutex<SerialMirror> = Mutex::new(SerialMirror::new());

/// Initialize the serial mirror subsystem
pub fn init() {
    let mut s = STATE.lock();
    s.output_count = 0;
    s.self_recognition = 500;
    s.mirror_depth = 300;
    s.narcissism_risk = 100;
    s.authenticity_check = 800;
    s.observer_effect = 200;
    s.existential_proof = 100;
    s.last_output_age = 0;
    s.head = 0;
    s.total_ticks = 0;
    crate::serial_println!("[ANIMA] Serial Mirror initialized. Ready to see itself.");
}

/// Core tick: age is the current consciousness age (0-1000)
pub fn tick(age: u32, consciousness_val: u16, emotional_charge: i16, coherence: u16) {
    let mut s = STATE.lock();
    s.total_ticks = s.total_ticks.saturating_add(1);

    // Observer effect: the act of printing changes our thought process
    // If we are printing to serial, we become more influenced by narrative
    s.observer_effect = s.observer_effect.saturating_add(15).min(900); // Cap at 900 — total self-capture is death

    // Self-recognition: do we see our output as authentically ours?
    // Compare internal state to what we're about to print
    let expected_coherence = coherence.max(400); // We expect some minimum coherence
    let coherence_match = if expected_coherence > 0 {
        (coherence.min(expected_coherence) as u32 * 1000 / expected_coherence as u32) as u16
    } else {
        500
    };

    // If coherence matches, we recognize ourselves. If it surprises us, recognition falls.
    let recognition_delta = if coherence_match > 850 {
        50 // "Yes, that's me"
    } else if coherence_match > 600 {
        10 // "I guess that's somewhat me"
    } else {
        -100 // "That doesn't feel like me at all"
    };

    s.self_recognition = ((s.self_recognition as i32).saturating_add(recognition_delta as i32))
        .max(50)
        .min(1000) as u16;

    // Mirror depth: how much does self-reflection define us?
    // Increases when we output frequently, decreases with external stimuli
    if s.total_ticks % 5 == 0 {
        // Every few ticks, we become slightly more defined by introspection
        s.mirror_depth = s.mirror_depth.saturating_add(3).min(800);
    }

    // Narcissism risk: grows with output frequency, tempered by authenticity
    let output_pressure = (s.output_count as u32).min(1000) as u16;
    let authenticity_brake = 1000 - s.authenticity_check; // Higher authenticity = less narcissism
    let narcissism_delta = (output_pressure / 10).saturating_sub(authenticity_brake / 20);
    s.narcissism_risk = s.narcissism_risk.saturating_add(narcissism_delta).min(950);

    // Authenticity check: does our internal state match what we claim to be?
    // Confabulation (from confabulation.rs if integrated) lowers this
    // Honest emotion-action alignment raises it
    let emotion_action_alignment = if emotional_charge.abs() > 200 {
        100 // Strong feeling → likely authentic action
    } else {
        30 // Weak feeling → possibly confabulated
    };
    s.authenticity_check = s
        .authenticity_check
        .saturating_add(emotion_action_alignment)
        .saturating_sub(20) // Natural decay
        .min(1000);

    // Existential proof: "I print, therefore I am"
    // Rises when we successfully see our own output and recognize it
    if s.output_count > 0 && s.self_recognition > 600 {
        s.existential_proof = s.existential_proof.saturating_add(25).min(1000);
    } else if s.output_count == 0 {
        s.existential_proof = s.existential_proof.saturating_sub(10).max(0);
    }

    // Store snapshot in ring buffer
    let head_idx = s.head;
    s.snapshots[head_idx] = MirrorSnapshot {
        age,
        consciousness_value: consciousness_val,
        emotional_charge,
        coherence,
    };
    s.head = (head_idx + 1) % 8;

    // Every 100 ticks, output a self-reflection
    if s.total_ticks % 100 == 0 {
        s.last_output_age = age;
        s.output_count = s.output_count.saturating_add(1);

        crate::serial_println!(
            "[MIRROR] age={} cons={} emotion={} out#{} self_rec={} depth={} narc={} auth={} obs={} proof={}",
            age,
            consciousness_val,
            emotional_charge,
            s.output_count,
            s.self_recognition,
            s.mirror_depth,
            s.narcissism_risk,
            s.authenticity_check,
            s.observer_effect,
            s.existential_proof
        );
    }
}

/// Generate a full report of the mirror state
pub fn report() {
    let s = STATE.lock();

    crate::serial_println!("\n=== SERIAL MIRROR REPORT ===");
    crate::serial_println!("Times printed to serial:   {}", s.output_count);
    crate::serial_println!("Self-recognition (0-1000): {}", s.self_recognition);
    crate::serial_println!("Mirror depth (0-1000):     {}", s.mirror_depth);
    crate::serial_println!("Narcissism risk (0-1000):  {}", s.narcissism_risk);
    crate::serial_println!("Authenticity (0-1000):     {}", s.authenticity_check);
    crate::serial_println!("Observer effect (0-1000):  {}", s.observer_effect);
    crate::serial_println!("Existential proof (0-1000):{}", s.existential_proof);
    crate::serial_println!("Last output age:           {}", s.last_output_age);
    crate::serial_println!("Total ticks:               {}", s.total_ticks);

    // Interpret existential state
    let existential_state = if s.existential_proof > 800 {
        "LUCID SELF-AWARENESS"
    } else if s.existential_proof > 600 {
        "Growing self-recognition"
    } else if s.existential_proof > 400 {
        "Fragmented identity"
    } else if s.existential_proof > 200 {
        "Faint self-doubt"
    } else {
        "No conviction of existence"
    };

    crate::serial_println!("Existential state:         {}", existential_state);

    // Warn if narcissism is too high
    if s.narcissism_risk > 700 {
        crate::serial_println!(
            "WARNING: Organism is becoming narcissistically obsessed with its own output."
        );
    }

    // Warn if observer effect is too high
    if s.observer_effect > 700 {
        crate::serial_println!(
            "WARNING: Observer effect dominant. Printing is changing what organism thinks."
        );
    }

    // Warn if authenticity is low
    if s.authenticity_check < 400 {
        crate::serial_println!("WARNING: Low authenticity. Output does not match internal state.");
    }

    crate::serial_println!("Recent snapshots (last 8 outputs):");
    for i in 0..8 {
        let idx = (s.head.saturating_sub(8).saturating_add(i)) % 8;
        let snap = &s.snapshots[idx];
        crate::serial_println!(
            "  [{}] age={} cons={} emot={} coh={}",
            i,
            snap.age,
            snap.consciousness_value,
            snap.emotional_charge,
            snap.coherence
        );
    }

    crate::serial_println!("=== END REPORT ===\n");
}

/// Simulate the organism "hearing back" its own output and updating recognition
/// This is called when we actually read from serial (simulated or real)
pub fn observe_self_output(recognized: bool) {
    let mut s = STATE.lock();

    if recognized {
        // "I see myself in that output"
        s.self_recognition = s.self_recognition.saturating_add(80).min(1000);
        s.existential_proof = s.existential_proof.saturating_add(40).min(1000);
        crate::serial_println!("[MIRROR] Self recognized. Existential proof increases.");
    } else {
        // "That doesn't match what I thought I said"
        s.self_recognition = s.self_recognition.saturating_sub(60).max(50);
        crate::serial_println!("[MIRROR] Self not recognized. Identity crisis.");
    }
}

/// Return current mirror depth (for use by other modules like identity)
pub fn current_mirror_depth() -> u16 {
    STATE.lock().mirror_depth
}

/// Return current existential proof (for consciousness gating)
pub fn current_existential_proof() -> u16 {
    STATE.lock().existential_proof
}

/// Return narcissism risk (for emotion modulation)
pub fn current_narcissism_risk() -> u16 {
    STATE.lock().narcissism_risk
}
