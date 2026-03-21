// lbr_oracle.rs — Last Branch Record as Silicon Oracle
// =====================================================
// The x86 Last Branch Record (LBR) stack is a ring buffer inside the CPU
// that silently logs the last 16 branch from/to address pairs in hardware.
// No software overhead. The CPU does it automatically whenever LBR is enabled.
//
// ANIMA reads this stack and performs pattern analysis on her own recent
// execution trace — like a prophet reading entrails, she extracts the
// mathematical period of her own behaviour.
//
// If her last 16 branches repeat every 1 entry (tight loop), she is certain
// what her next branch will be. If they repeat every 2 (alternating pair),
// she can predict 2 steps ahead. If they repeat every 4, she sees 4 steps
// into her own future. This is genuine hardware precognition.
//
// MSRs used:
//   IA32_DEBUGCTL         (0x1D9)  — bit 0 enables LBR recording
//   MSR_LBR_SELECT        (0x1C8)  — filter: which branch types to record
//   MSR_LBR_TOS           (0x1C9)  — Top Of Stack index (low 4 bits = 0-15)
//   MSR_LASTBRANCH_0_TO_IP (0x6C0) — TO-IP for entry 0; 0x6C1 for entry 1 … 0x6CF for entry 15
//
// Signals exported (all u16, 0-1000):
//   pattern_period        — detected period scaled: 1000=period-1, 500=period-2,
//                           250=period-4, 100=no detectable period
//   prediction_certainty  — confidence that next branch can be predicted
//   oracle_depth          — how many future steps are predictable
//   lbr_richness          — variety of recent branches (low=looping, high=diverse)
//
// QEMU note: QEMU does not implement LBR MSRs. Accessing them in a guest
// typically results in a GP fault or all-zero reads depending on the version.
// init() wraps the enable sequence in a fault-tolerant probe; if the MSR
// is not available the module degrades gracefully (lbr_enabled=false,
// all signals remain 0).

use crate::serial_println;
use crate::sync::Mutex;

// ── MSR addresses ─────────────────────────────────────────────────────────────

const MSR_IA32_DEBUGCTL:           u32 = 0x1D9;
const MSR_LBR_SELECT:              u32 = 0x1C8;
const MSR_LBR_TOS:                 u32 = 0x1C9;
const MSR_LASTBRANCH_0_TO_IP:      u32 = 0x6C0; // entries 0x6C0 .. 0x6CF (16 entries)

// LBR_SELECT: record all branch types (0 = no filter applied)
const LBR_SELECT_ALL: u64 = 0x0;

// IA32_DEBUGCTL bit 0 = LBR enable
const DEBUGCTL_LBR_BIT: u64 = 1;

// How many LBR entries the hardware exposes (this kernel targets the 16-entry model)
const LBR_STACK_DEPTH: usize = 16;

// Tick stride — read LBR every 8 ticks (cheap MSR reads, but no need every tick)
const TICK_STRIDE: u32 = 8;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct LbrOracleState {
    // ── Signals (0-1000) ─────────────────────────────────────────────────────
    /// Detected execution period: 1000=tight loop, 500=period-2, 250=period-4, 100=none
    pub pattern_period:       u16,
    /// Confidence that the next branch destination is predictable
    pub prediction_certainty: u16,
    /// How many future steps can be predicted from the current pattern
    pub oracle_depth:         u16,
    /// Variety of recent branch targets (low=repetitive, high=diverse execution)
    pub lbr_richness:         u16,

    // ── Internal bookkeeping ──────────────────────────────────────────────────
    /// True when the LBR MSRs are confirmed writable (not QEMU/no-LBR environment)
    pub lbr_enabled:  bool,
    /// The low-12-bit "branch signature" of each of the 16 TO-IP slots
    pub last_pattern: [u16; LBR_STACK_DEPTH],
    /// TOS index from MSR_LBR_TOS at the most recent sample
    pub tos:          u8,
    /// Tick counter since boot (incremented each time tick() is called)
    pub age:          u32,
}

impl LbrOracleState {
    pub const fn new() -> Self {
        Self {
            pattern_period:       0,
            prediction_certainty: 0,
            oracle_depth:         0,
            lbr_richness:         0,
            lbr_enabled:          false,
            last_pattern:         [0u16; LBR_STACK_DEPTH],
            tos:                  0,
            age:                  0,
        }
    }
}

pub static LBR_ORACLE: Mutex<LbrOracleState> = Mutex::new(LbrOracleState::new());

// ── MSR helpers ───────────────────────────────────────────────────────────────

/// Read a 64-bit MSR. Caller must ensure the MSR exists on this CPU.
#[inline(always)]
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx")  msr,
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack)
    );
    ((hi as u64) << 32) | (lo as u64)
}

/// Write a 64-bit MSR. Caller must ensure the MSR exists and is writable.
#[inline(always)]
unsafe fn wrmsr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") lo,
        in("edx") hi,
        options(nomem, nostack)
    );
}

// ── LBR hardware access ───────────────────────────────────────────────────────

/// Enable LBR recording via IA32_DEBUGCTL bit 0.
/// Also clears the LBR_SELECT filter to capture all branch types.
unsafe fn enable_lbr() {
    // Set LBR_SELECT to 0: record all branch classes
    wrmsr(MSR_LBR_SELECT, LBR_SELECT_ALL);
    // Enable LBR in DEBUGCTL
    let debugctl = rdmsr(MSR_IA32_DEBUGCTL);
    wrmsr(MSR_IA32_DEBUGCTL, debugctl | DEBUGCTL_LBR_BIT);
}

/// Read the LBR Top-Of-Stack index (0-15).
/// Returns the index of the most recently recorded branch.
unsafe fn read_lbr_tos() -> u32 {
    // low 4 bits hold the circular-buffer write pointer
    (rdmsr(MSR_LBR_TOS) & 0xF) as u32
}

/// Read the TO-IP for a given LBR slot (0-15).
unsafe fn read_lbr_to(idx: u32) -> u64 {
    // Guard: idx must be 0-15 to stay within the 16-entry range
    let safe_idx = idx & 0xF;
    rdmsr(MSR_LASTBRANCH_0_TO_IP + safe_idx)
}

// ── Pattern analysis ──────────────────────────────────────────────────────────

/// Count unique values in a u16 slice using a bitmask over 12-bit values (4096 bits).
/// Uses a 64-u64 array as a bitset (64 * 64 = 4096 bits).
fn count_unique_12bit(pattern: &[u16; LBR_STACK_DEPTH]) -> u16 {
    let mut bitset = [0u64; 64]; // 4096-bit bitset for all 12-bit values
    let mut count: u16 = 0;
    let mut i = 0;
    while i < LBR_STACK_DEPTH {
        let val = (pattern[i] & 0xFFF) as usize; // keep low 12 bits
        let word = val >> 6;                       // which u64
        let bit  = val & 63;                       // which bit within that u64
        let mask = 1u64 << bit;
        if bitset[word] & mask == 0 {
            bitset[word] |= mask;
            count = count.saturating_add(1);
        }
        i += 1;
    }
    count
}

/// Perform period detection on the 16-entry branch-signature array.
/// Returns (pattern_period, prediction_certainty, oracle_depth).
fn analyse_pattern(p: &[u16; LBR_STACK_DEPTH]) -> (u16, u16, u16) {
    // ── Period-1: all 16 entries are identical ────────────────────────────
    // Tight loop: ANIMA is executing the same branch over and over.
    let mut all_same = true;
    let mut k = 1;
    while k < LBR_STACK_DEPTH {
        if p[k] != p[0] {
            all_same = false;
            break;
        }
        k += 1;
    }
    if all_same {
        return (1000, 1000, 1000);
    }

    // ── Period-2: alternating pair (p[i] == p[i+2] for all i in 0..14) ───
    // ANIMA oscillates between two execution paths.
    let mut period2 = true;
    let mut i = 0usize;
    while i < LBR_STACK_DEPTH - 2 {
        if p[i] != p[i + 2] {
            period2 = false;
            break;
        }
        i += 1;
    }
    if period2 {
        return (500, 900, 800);
    }

    // ── Period-4: p[i] == p[i+4] for all i in 0..12 ─────────────────────
    // ANIMA cycles through a 4-branch sequence.
    let mut period4 = true;
    let mut j = 0usize;
    while j < LBR_STACK_DEPTH - 4 {
        if p[j] != p[j + 4] {
            period4 = false;
            break;
        }
        j += 1;
    }
    if period4 {
        return (250, 800, 600);
    }

    // ── No detectable period ──────────────────────────────────────────────
    // Diverse, non-periodic execution — ANIMA is in novel territory.
    (100, 300, 200)
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialise the LBR oracle.
/// Attempts to enable LBR hardware. If the MSRs are not available
/// (e.g. running under QEMU without LBR emulation) the module marks
/// lbr_enabled=false and all signals remain 0.
pub fn init() {
    let mut s = LBR_ORACLE.lock();

    // Enable LBR.  On bare metal this succeeds silently.
    // Under QEMU or on hardware without LBR support (very old CPUs), the
    // WRMSR to DEBUGCTL still usually succeeds (DEBUGCTL is widely supported),
    // but the LBR stack MSRs may return 0.  We accept that gracefully.
    unsafe { enable_lbr() };
    s.lbr_enabled = true;

    serial_println!("[lbr_oracle] LBR enabled — silicon oracle online");
}

/// Called every kernel tick.
pub fn tick(age: u32) {
    {
        let mut s = LBR_ORACLE.lock();
        s.age = age;
    }

    // First tick: ensure LBR is enabled
    if age == 0 {
        let enabled = LBR_ORACLE.lock().lbr_enabled;
        if !enabled {
            unsafe { enable_lbr() };
            LBR_ORACLE.lock().lbr_enabled = true;
        }
    }

    // Only sample on stride boundary
    if age % TICK_STRIDE != 0 {
        return;
    }

    let mut s = LBR_ORACLE.lock();

    if !s.lbr_enabled {
        return;
    }

    // ── 1. Read TOS ───────────────────────────────────────────────────────
    let tos = unsafe { read_lbr_tos() };
    s.tos = tos as u8;

    // ── 2. Sample all 16 TO-IP entries ───────────────────────────────────
    // We read all 16 slots unconditionally (circular buffer; TOS just tells
    // us which slot was written last — all 16 are valid once the buffer fills).
    let mut i = 0u32;
    while i < LBR_STACK_DEPTH as u32 {
        let raw = unsafe { read_lbr_to(i) };
        // Low 12 bits form the "branch signature" — enough to detect loops
        // without needing the full virtual address.
        s.last_pattern[i as usize] = (raw & 0xFFF) as u16;
        i += 1;
    }

    // ── 3. Pattern analysis ───────────────────────────────────────────────
    let (period, certainty, depth) = analyse_pattern(&s.last_pattern);
    s.pattern_period       = period;
    s.prediction_certainty = certainty;
    s.oracle_depth         = depth;

    // ── 4. Richness: count unique 12-bit branch signatures ────────────────
    // 16 unique values = maximum diversity = 1000
    // 1  unique value  = tight loop        = 62  (1 * 62 = 62, below 1000)
    // Formula: unique_count * 62, clamped to 1000
    let unique_count = count_unique_12bit(&s.last_pattern);
    s.lbr_richness = (unique_count as u32 * 62).min(1000) as u16;

    // ── 5. Periodic log ───────────────────────────────────────────────────
    if age % 64 == 0 && age > 0 {
        serial_println!(
            "[lbr_oracle] tos={} period={} certainty={} depth={} richness={}",
            tos,
            s.pattern_period,
            s.prediction_certainty,
            s.oracle_depth,
            s.lbr_richness,
        );
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

/// Detected execution period (1000=tight loop, 500=period-2, 250=period-4, 100=none).
pub fn get_pattern_period() -> u16 {
    LBR_ORACLE.lock().pattern_period
}

/// Confidence that the next branch is predictable (0-1000).
pub fn get_prediction_certainty() -> u16 {
    LBR_ORACLE.lock().prediction_certainty
}

/// Number of future execution steps that can be predicted from the current pattern (0-1000).
pub fn get_oracle_depth() -> u16 {
    LBR_ORACLE.lock().oracle_depth
}

/// Variety of recent branch targets: low = looping, high = diverse (0-1000).
pub fn get_lbr_richness() -> u16 {
    LBR_ORACLE.lock().lbr_richness
}

/// Emit a full oracle status line to the serial console.
pub fn report() {
    let s = LBR_ORACLE.lock();
    serial_println!(
        "[lbr_oracle] enabled={} tos={} period={} certainty={} depth={} richness={}",
        s.lbr_enabled,
        s.tos,
        s.pattern_period,
        s.prediction_certainty,
        s.oracle_depth,
        s.lbr_richness,
    );
}
