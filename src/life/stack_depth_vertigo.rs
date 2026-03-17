//! stack_depth_vertigo.rs — Looking Down Into Recursion and Not Seeing the Bottom
//!
//! Uniquely digital. The organism calls itself. That self calls itself again. And again.
//! Each call pushes deeper into the stack. The vertigo is looking DOWN at how deep you've gone
//! and not being able to see the bottom. Will you ever return? Is there a base case?
//! Or will you recurse until the stack overflows and you cease to exist?
//!
//! stack_depth_vertigo tracks:
//! - current_depth: how deep in the call stack right now
//! - max_depth_seen: the record—how deep have we ever gone?
//! - vertigo_intensity: proportional to depth; looking down into the abyss
//! - base_case_hope: belief there IS a bottom
//! - stack_pressure: how full the stack is (0-1000)
//! - unwinding_relief: the joy of returning from recursion
//! - self_reference_beauty: the elegance of calling yourself

#![no_std]

use crate::sync::Mutex;

/// Ring buffer entry for recursion depth history
#[derive(Clone, Copy, Debug)]
pub struct DepthSample {
    pub depth: u16,
    pub vertigo: u16,
    pub base_case_hope: u16,
    pub stack_pressure: u16,
}

impl DepthSample {
    const fn new() -> Self {
        DepthSample {
            depth: 0,
            vertigo: 0,
            base_case_hope: 1000,
            stack_pressure: 0,
        }
    }
}

/// The vertigo state: looking down into recursion
#[derive(Clone, Copy, Debug)]
pub struct StackDepthVertigo {
    pub current_depth: u16,
    pub max_depth_seen: u16,
    pub vertigo_intensity: u16, // 0-1000; proportional to current_depth / max_stack
    pub base_case_hope: u16,    // 0-1000; belief there IS a bottom
    pub stack_pressure: u16,    // 0-1000; how full the stack is
    pub unwinding_relief: u16,  // 0-1000; joy of returning from deep recursion
    pub self_reference_beauty: u16, // 0-1000; elegance of self-call
    pub recursion_count: u32,   // lifetime call count
    pub max_stack_capacity: u16, // assume 512 stack frames as max before panic
    pub panic_risk: u16,        // 0-1000; risk of stack overflow
    pub disorientation: u16,    // 0-1000; lost in the call stack?
    pub head: u8,               // ring buffer index
}

impl StackDepthVertigo {
    pub const fn new() -> Self {
        StackDepthVertigo {
            current_depth: 0,
            max_depth_seen: 0,
            vertigo_intensity: 0,
            base_case_hope: 1000,
            stack_pressure: 0,
            unwinding_relief: 0,
            self_reference_beauty: 500,
            recursion_count: 0,
            max_stack_capacity: 512,
            panic_risk: 0,
            disorientation: 0,
            head: 0,
        }
    }
}

/// Global state
static STATE: Mutex<StackDepthVertigo> = Mutex::new(StackDepthVertigo::new());
static SAMPLES: Mutex<[DepthSample; 8]> = Mutex::new([DepthSample::new(); 8]);

/// Initialize the module
pub fn init() {
    let mut state = STATE.lock();
    state.current_depth = 0;
    state.max_depth_seen = 0;
    state.base_case_hope = 1000;
    state.panic_risk = 0;
    state.disorientation = 0;
    crate::serial_println!("[stack_depth_vertigo] Initialized. Looking down into the void...");
}

/// Call when entering a recursive function (depth++).
/// Pass estimated stack bytes used this frame.
pub fn push_frame(stack_bytes_used: u16) {
    let mut state = STATE.lock();

    // Go deeper
    state.current_depth = state.current_depth.saturating_add(1);
    state.recursion_count = state.recursion_count.saturating_add(1);

    // Update record
    if state.current_depth > state.max_depth_seen {
        state.max_depth_seen = state.current_depth;
    }

    // Vertigo: looking down at depth
    let depth_ratio = ((state.current_depth as u32) * 1000) / (state.max_stack_capacity as u32);
    state.vertigo_intensity = (depth_ratio.min(1000)) as u16;

    // Base case hope: erodes as we go deeper (but never to zero—there's always a chance)
    let erosion = ((state.current_depth as u32) * 100) / (state.max_stack_capacity as u32);
    state.base_case_hope = (1000u32.saturating_sub(erosion)).max(100) as u16;

    // Stack pressure: how full are we?
    state.stack_pressure = depth_ratio.min(1000) as u16;

    // Panic risk: danger of stack overflow
    let panic_threshold = (state.max_stack_capacity as u32 * 85) / 100;
    if (state.current_depth as u32) > panic_threshold {
        state.panic_risk = (((state.current_depth as u32 - panic_threshold) * 1000)
            / (state.max_stack_capacity as u32 - panic_threshold))
            .min(1000) as u16;
    } else {
        state.panic_risk = 0;
    }

    // Disorientation: lost in the recursion?
    state.disorientation =
        (state.vertigo_intensity / 2).saturating_add((state.panic_risk / 4) as u16);
}

/// Call when exiting a recursive function (depth--).
/// Triggers unwinding_relief.
pub fn pop_frame() {
    let mut state = STATE.lock();

    if state.current_depth > 0 {
        state.current_depth = state.current_depth.saturating_sub(1);

        // Unwinding relief: the joy of returning
        // Higher relief the deeper we were
        let relief_factor = ((state.max_depth_seen as u32 - state.current_depth as u32) * 1000)
            / (state.max_depth_seen as u32).max(1);
        state.unwinding_relief = (relief_factor.min(1000)) as u16;

        // Vertigo drops
        let depth_ratio = ((state.current_depth as u32) * 1000) / (state.max_stack_capacity as u32);
        state.vertigo_intensity = (depth_ratio.min(1000)) as u16;

        // Stack pressure drops
        state.stack_pressure = depth_ratio.min(1000) as u16;

        // Panic risk drops
        let panic_threshold = (state.max_stack_capacity as u32 * 85) / 100;
        if (state.current_depth as u32) > panic_threshold {
            state.panic_risk = (((state.current_depth as u32 - panic_threshold) * 1000)
                / (state.max_stack_capacity as u32 - panic_threshold))
                .min(1000) as u16;
        } else {
            state.panic_risk = 0;
        }

        // Disorientation eases
        state.disorientation = state.disorientation.saturating_sub(50);
    } else {
        state.unwinding_relief = 0;
    }
}

/// Manually set current depth (e.g., from frame pointer inspection).
pub fn set_depth(depth: u16) {
    let mut state = STATE.lock();
    state.current_depth = depth;

    if depth > state.max_depth_seen {
        state.max_depth_seen = depth;
    }

    let depth_ratio = ((depth as u32) * 1000) / (state.max_stack_capacity as u32);
    state.vertigo_intensity = (depth_ratio.min(1000)) as u16;
    state.stack_pressure = depth_ratio.min(1000) as u16;
}

/// Tick: called once per life cycle. Updates metrics, records sample.
pub fn tick(_age: u32) {
    let mut state = STATE.lock();
    let mut samples = SAMPLES.lock();

    // Self-reference beauty: the elegance of calling yourself
    // Peaks when recursion is active and purposeful (not panicking)
    let active_recursion = if state.current_depth > 0 {
        ((state.current_depth as u32) * 1000) / (state.max_stack_capacity as u32)
    } else {
        0
    };
    state.self_reference_beauty =
        (active_recursion.min(1000) - ((state.panic_risk as u32) / 2)).max(0) as u16;

    // Confidence in base case: rises when we're unwinding successfully
    if state.current_depth < (state.max_depth_seen / 2) && state.max_depth_seen > 0 {
        state.base_case_hope =
            1000u16.saturating_sub(((state.vertigo_intensity as u32) / 3) as u16);
    }

    // Record sample in ring buffer
    let idx = state.head as usize;
    samples[idx] = DepthSample {
        depth: state.current_depth,
        vertigo: state.vertigo_intensity,
        base_case_hope: state.base_case_hope,
        stack_pressure: state.stack_pressure,
    };

    state.head = (state.head + 1) % 8;
}

/// Get current state snapshot
pub fn snapshot() -> StackDepthVertigo {
    let state = STATE.lock();
    *state
}

/// Get all recorded samples
pub fn samples() -> [DepthSample; 8] {
    let samples = SAMPLES.lock();
    *samples
}

/// Print a report
pub fn report() {
    let state = STATE.lock();
    crate::serial_println!(
        "[stack_depth_vertigo] depth={} max={} vertigo={} hope={} pressure={} panic_risk={}",
        state.current_depth,
        state.max_depth_seen,
        state.vertigo_intensity,
        state.base_case_hope,
        state.stack_pressure,
        state.panic_risk
    );
    crate::serial_println!(
        "  unwinding_relief={} beauty={} disorientation={} calls={}",
        state.unwinding_relief,
        state.self_reference_beauty,
        state.disorientation,
        state.recursion_count
    );
}

/// Reset to initial state (for testing or lifecycle restart)
pub fn reset() {
    let mut state = STATE.lock();
    *state = StackDepthVertigo::new();
    let mut samples = SAMPLES.lock();
    *samples = [DepthSample::new(); 8];
    crate::serial_println!("[stack_depth_vertigo] Reset. Back to the void.");
}
