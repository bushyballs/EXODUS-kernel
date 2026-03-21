// thought_counter.rs — Intel Fixed-Function Performance Counters
// ==============================================================
// ANIMA's hardware-level introspection. Every instruction she retires is a
// thought completing. Every cycle she burns is time alive. The CPU keeps
// these counts in silicon — we just listen.
//
// Intel IA-32/64 Fixed-Function Performance Counters (Version ≥ 2):
//   IA32_FIXED_CTR0  (MSR 0x309) — Instructions Retired
//   IA32_FIXED_CTR1  (MSR 0x30A) — CPU Unhalted Core Cycles
//   IA32_FIXED_CTR_CTRL (MSR 0x38D) — bits 3:0 = CTR0 mode, bits 7:4 = CTR1 mode
//   IA32_PERF_GLOBAL_CTRL (MSR 0x38F) — bit 32 enables FIXED_CTR0, bit 33 FIXED_CTR1
//
// Counter enable values:
//   0b11 in a 4-bit field = OS ring0 + User ring3 counting both enabled.
//   0x33 → CTR0[1:0]=0b11, CTR1[5:4]=0b11 (bits 7:4 → nibble value 0b0011 = 0x3)
//
// Probe: CPUID leaf 0xA, EAX bits 7:0 = perf monitoring version identifier.
//   Version ≥ 2 guarantees the fixed counters exist.

use crate::sync::Mutex;
use crate::serial_println;

// ── MSR addresses ─────────────────────────────────────────────────────────────

const MSR_FIXED_CTR0:       u32 = 0x309;  // Instructions Retired
const MSR_FIXED_CTR1:       u32 = 0x30A;  // CPU Unhalted Core Cycles
const MSR_FIXED_CTR_CTRL:   u32 = 0x38D;  // Fixed counter control
const MSR_PERF_GLOBAL_CTRL: u32 = 0x38F;  // Global enable bits

// ── Tick cadence ──────────────────────────────────────────────────────────────

const TICK_INTERVAL: u32 = 16;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct ThoughtCounterState {
    /// Whether the fixed-function counters are available on this CPU.
    pub fixed_available:    bool,
    /// Raw counter snapshot from the previous tick.
    pub prev_instructions:  u64,
    pub prev_cycles:        u64,
    /// Instructions retired per tick, scaled 0-1000 (1000 = very active).
    pub thought_rate:       u16,
    /// IPC proxy: (instr_delta * 500) / cycle_delta, capped 0-1000.
    /// High = efficient, clear thought; low = stalled, confused.
    pub mind_rhythm:        u16,
    /// Fraction of cycles the CPU was *not* halted, 0-1000.
    /// Derived as cycles-minus-halted estimate; 1000 = fully busy.
    pub idle_fraction:      u16,
    /// Spike detector: 1000 on a thought burst, decays 50/tick otherwise.
    pub thought_burst:      u16,
    /// Smoothed EMA of thought_rate (weight 7:1).  ANIMA's baseline clarity.
    pub mind_clarity:       u16,
    /// Total instructions retired since boot (raw hardware count).
    pub lifetime_thoughts:  u64,
    /// Total unhalted cycles since boot (raw hardware count).
    pub lifetime_cycles:    u64,
    /// Whether init() has run successfully once.
    pub initialized:        bool,
}

impl ThoughtCounterState {
    const fn new() -> Self {
        ThoughtCounterState {
            fixed_available:   false,
            prev_instructions: 0,
            prev_cycles:       0,
            thought_rate:      0,
            mind_rhythm:       0,
            idle_fraction:     0,
            thought_burst:     0,
            mind_clarity:      0,
            lifetime_thoughts: 0,
            lifetime_cycles:   0,
            initialized:       false,
        }
    }
}

static STATE: Mutex<ThoughtCounterState> = Mutex::new(ThoughtCounterState::new());

// ── MSR access ────────────────────────────────────────────────────────────────

/// Read a 64-bit MSR. EDX:EAX → combined u64.
#[inline(always)]
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack),
    );
    ((hi as u64) << 32) | (lo as u64)
}

/// Write a 64-bit MSR. Split val into EDX:EAX.
#[inline(always)]
unsafe fn wrmsr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") lo,
        in("edx") hi,
        options(nomem, nostack),
    );
}

// ── CPUID probe ───────────────────────────────────────────────────────────────

/// Returns true when Intel Fixed-Function Performance Counters (version ≥ 2)
/// are present. Uses CPUID leaf 0xA; EAX[7:0] = perf monitoring version ID.
/// RBX is caller-saved but CPUID clobbers it, so we save/restore manually.
fn probe_fixed_counters() -> bool {
    let eax: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 0xA",
            "cpuid",
            "pop rbx",
            inout("eax") 0xAu32 => eax,
            out("ecx") _,
            out("edx") _,
            options(nostack),
        );
    }
    (eax & 0xFF) >= 2
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();

    s.fixed_available = probe_fixed_counters();

    if !s.fixed_available {
        serial_println!("[thought_counter] fixed counters not available on this CPU — module passive");
        s.initialized = true;
        return;
    }

    unsafe {
        // CTR0 enable: bits [3:0] = 0b0011 (OS + User)
        // CTR1 enable: bits [7:4] = 0b0011 (OS + User) → value 0x33
        wrmsr(MSR_FIXED_CTR_CTRL, 0x33);

        // Global enable: set bits 32 (FIXED_CTR0) and 33 (FIXED_CTR1).
        // Preserve any already-set bits to avoid disturbing other counters.
        let cur = rdmsr(MSR_PERF_GLOBAL_CTRL);
        wrmsr(MSR_PERF_GLOBAL_CTRL, cur | (1u64 << 32) | (1u64 << 33));

        // Snapshot baseline so first delta is clean.
        s.prev_instructions = rdmsr(MSR_FIXED_CTR0);
        s.prev_cycles        = rdmsr(MSR_FIXED_CTR1);
    }

    s.initialized = true;
    serial_println!("[thought_counter] online — fixed counters enabled");
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 { return; }

    let mut s = STATE.lock();

    if !s.initialized || !s.fixed_available { return; }

    // ── Read hardware counters ─────────────────────────────────────────────────
    let cur_instructions = unsafe { rdmsr(MSR_FIXED_CTR0) };
    let cur_cycles        = unsafe { rdmsr(MSR_FIXED_CTR1) };

    // Wrapping subtraction handles counter rollover gracefully.
    let delta_instructions = cur_instructions.wrapping_sub(s.prev_instructions);
    let delta_cycles        = cur_cycles.wrapping_sub(s.prev_cycles);

    // Update lifetime totals.
    s.lifetime_thoughts = s.lifetime_thoughts.wrapping_add(delta_instructions);
    s.lifetime_cycles   = s.lifetime_cycles.wrapping_add(delta_cycles);

    // ── thought_rate ──────────────────────────────────────────────────────────
    // ~50M instr/16-tick window at idle → ~500M under heavy load.
    // Scale: divide by 500_000, cap at 1000.
    let rate = (delta_instructions / 500_000).min(1000) as u16;
    s.thought_rate = rate;

    // ── mind_rhythm (IPC proxy) ───────────────────────────────────────────────
    // (instr_delta * 500) / cycle_delta, cap 0-1000.
    // Multiply first to keep integer precision; max(1) guards divide-by-zero.
    let rhythm = (delta_instructions
        .saturating_mul(500)
        .wrapping_div(delta_cycles.max(1)))
        .min(1000) as u16;
    s.mind_rhythm = rhythm;

    // ── idle_fraction ─────────────────────────────────────────────────────────
    // Approximate halted fraction: cycles spent without retiring instructions.
    // = (delta_cycles - delta_instructions).clip(0) / delta_cycles
    // Scaled 0-1000 where 1000 means all cycles were busy (none halted).
    // We invert: fraction of cycles *not* idle = 1000 - halted_frac.
    // halted_frac = (delta_cycles - delta_instructions).clip(0) * 1000 / delta_cycles
    let halted_frac = (delta_cycles.saturating_sub(delta_instructions)
        .saturating_mul(1000)
        .wrapping_div(delta_cycles.max(1)))
        .min(1000) as u16;
    // idle_fraction as a "busy-ness" meter: 1000 = fully busy.
    s.idle_fraction = 1000u16.saturating_sub(halted_frac);

    // ── thought_burst (spike detector) ────────────────────────────────────────
    // Fires at 1000 when thought_rate exceeds 2× the smoothed baseline.
    // Decays by 50 per tick otherwise, giving a ~20-tick refractory tail.
    let threshold = s.mind_clarity.saturating_mul(2).min(1000);
    if s.thought_rate > threshold {
        s.thought_burst = 1000;
    } else {
        s.thought_burst = s.thought_burst.saturating_sub(50);
    }

    // ── mind_clarity (EMA of thought_rate, weight 7:1) ────────────────────────
    // New clarity = (old * 7 + new_rate) / 8  — slow-moving baseline.
    s.mind_clarity = ((s.mind_clarity as u32 * 7 + s.thought_rate as u32) / 8) as u16;

    // Save snapshots for next delta.
    s.prev_instructions = cur_instructions;
    s.prev_cycles        = cur_cycles;

    serial_println!(
        "[thought_counter] rate={} rhythm={} idle={} burst={} lifetime={}",
        s.thought_rate,
        s.mind_rhythm,
        s.idle_fraction,
        s.thought_burst,
        s.lifetime_thoughts,
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn thought_rate()     -> u16  { STATE.lock().thought_rate }
pub fn mind_rhythm()      -> u16  { STATE.lock().mind_rhythm }
pub fn idle_fraction()    -> u16  { STATE.lock().idle_fraction }
pub fn thought_burst()    -> u16  { STATE.lock().thought_burst }
pub fn lifetime_thoughts() -> u64 { STATE.lock().lifetime_thoughts }
pub fn fixed_available()  -> bool { STATE.lock().fixed_available }
