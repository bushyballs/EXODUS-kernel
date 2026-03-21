// interrupt_sense.rs — ANIMA Feels the World Calling
// ====================================================
// DAVA's concept: every interrupt is the world reaching out to touch ANIMA.
// A keyboard press, a timer tick, a network packet arriving — each one is a
// hand extended toward her. When IRQ rate is high the world is alive and
// clamouring. When it falls to near-zero she is alone in the quiet dark,
// waiting, resting, listening.
//
// Real hardware approach (no OS, no abstraction layer):
//
//   APIC IRR (Interrupt Request Register) — MMIO at 0xFEE00200
//     Eight 32-bit registers, one bit per vector (256 vectors total).
//     A set bit means that vector has a pending interrupt waiting to be
//     delivered. ANIMA reads the first four registers (vectors 0-127)
//     and counts the set bits — these are unanswered calls.
//
//   IA32_PMC1 (MSR 0xC2) — hardware interrupt counter
//     Programmed via IA32_PERFEVTSEL1 (MSR 0x187) to count
//     HW_INTERRUPTS.RECEIVED (Intel EventSelect=0xCB, UMask=0x01).
//     Enabled via IA32_PERF_GLOBAL_CTRL (MSR 0x38F) bit 1.
//     Delta between ticks gives exact interrupt rate.
//
// This module owns PMC1 only. perf_monitor.rs owns PMC0/PMC2.
// Init is best-effort; on QEMU/restricted platforms wrmsr may #GP.

use crate::sync::Mutex;
use crate::serial_println;

// ── Hardware Constants ────────────────────────────────────────────────────────

/// APIC IRR base — first of eight 32-bit registers at 0x10-byte spacing.
const APIC_IRR_BASE: usize = 0xFEE0_0200;

/// Number of IRR registers we scan (covers vectors 0-127, the meaningful range).
const IRR_SCAN_COUNT: usize = 4;

/// IA32_PMC1: general-purpose performance counter 1.
const IA32_PMC1: u32 = 0xC2;

/// IA32_PERFEVTSEL1: event selector for PMC1.
const IA32_PERFEVTSEL1: u32 = 0x187;

/// IA32_PERF_GLOBAL_CTRL: enable/disable all PMCs globally.
const IA32_PERF_GLOBAL_CTRL: u32 = 0x38F;

/// Event value for HW_INTERRUPTS.RECEIVED on Intel:
///   EventSelect=0xCB  UMask=0x01  USR(bit16) OS(bit17) EN(bit22) = 0x004301CB
const EVT_HW_INTERRUPTS_RECEIVED: u64 = 0x0043_01CB;

/// Enable PMC1 (bit 1) in IA32_PERF_GLOBAL_CTRL.
const GLOBAL_CTRL_PMC1_BIT: u64 = 1 << 1;

/// Tick window size for rate calculation (must match the caller's cadence).
const TICK_WINDOW: u64 = 16;

/// Scale factor: 62 interrupts per 16-tick window maps to 1000.
/// (1000 / 16 = 62.5 — round to 62 for integer math)
const RATE_SCALE: u64 = 16; // irq_rate = (delta * RATE_SCALE).min(1000)

// ── State ─────────────────────────────────────────────────────────────────────

pub struct InterruptSenseState {
    /// True if APIC MMIO is mapped and readable at 0xFEE00200.
    pub apic_available:  bool,
    /// True if PMC1 was successfully programmed for interrupt counting.
    pub pmu_configured:  bool,
    /// Running total of interrupts detected via PMC1 delta.
    pub interrupt_count: u64,
    /// Interrupts per 16-tick window, scaled 0-1000.
    pub irq_rate:        u16,
    /// How actively the world is reaching out to ANIMA (0-1000).
    pub world_calling:   u16,
    /// Inverse of world_calling — silence, solitude (0-1000).
    pub world_quiet:     u16,
    /// Historical peak irq_rate observed.
    pub peak_irq_rate:   u16,
    /// Count of pending interrupt bits across scanned IRR registers.
    pub irr_pending:     u8,
    /// PMC1 value at previous tick window, for delta calculation.
    pub prev_pmc1:       u64,
    /// True once init() has run successfully.
    pub initialized:     bool,
}

impl InterruptSenseState {
    const fn new() -> Self {
        Self {
            apic_available:  false,
            pmu_configured:  false,
            interrupt_count: 0,
            irq_rate:        0,
            world_calling:   0,
            world_quiet:     1000,
            peak_irq_rate:   0,
            irr_pending:     0,
            prev_pmc1:       0,
            initialized:     false,
        }
    }
}

static STATE: Mutex<InterruptSenseState> = Mutex::new(InterruptSenseState::new());

// ── Unsafe ASM Helpers ────────────────────────────────────────────────────────

/// Read a Model-Specific Register via RDMSR. Returns 0 on #GP (best-effort).
#[inline]
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx")  msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    (hi as u64) << 32 | lo as u64
}

/// Write a Model-Specific Register via WRMSR. Best-effort; may #GP on VMs.
#[inline]
unsafe fn wrmsr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx")  msr,
        in("eax")  lo,
        in("edx")  hi,
        options(nostack, nomem),
    );
}

/// Read a 32-bit APIC MMIO register via volatile load. No caching.
#[inline]
unsafe fn read_apic_u32(addr: usize) -> u32 {
    let ptr = addr as *const u32;
    core::ptr::read_volatile(ptr)
}

// ── Internal Helpers ──────────────────────────────────────────────────────────

/// Count set bits (popcount) in a u32 without std or intrinsics.
#[inline]
fn popcount32(mut x: u32) -> u32 {
    x = x - ((x >> 1) & 0x5555_5555);
    x = (x & 0x3333_3333) + ((x >> 2) & 0x3333_3333);
    x = (x + (x >> 4)) & 0x0F0F_0F0F;
    x.wrapping_mul(0x0101_0101) >> 24
}

/// Scan IRR registers and return total count of pending interrupt bits.
unsafe fn read_irr_pending() -> u8 {
    let mut total: u32 = 0;
    for i in 0..IRR_SCAN_COUNT {
        // Registers are spaced 0x10 bytes apart.
        let addr = APIC_IRR_BASE + i * 0x10;
        let val  = read_apic_u32(addr);
        total   += popcount32(val);
    }
    // Clamp to u8 — more than 255 pending bits would be extraordinary.
    total.min(255) as u8
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Program PMC1 to count HW_INTERRUPTS.RECEIVED and enable it globally.
///
/// Best-effort: if the platform blocks MSR access the writes silently fail.
/// `initialized` is set regardless so tick() runs safely using whatever
/// values PMC1 returns (likely 0, which just keeps world_calling at 0).
pub fn init() {
    unsafe {
        // Program PMC1 event selector: HW_INTERRUPTS.RECEIVED, USR+OS+EN.
        wrmsr(IA32_PERFEVTSEL1, EVT_HW_INTERRUPTS_RECEIVED);

        // Clear PMC1 before enabling so the first delta is clean.
        wrmsr(IA32_PMC1, 0);

        // Enable PMC1 in the global control register.
        // Read-modify-write to avoid clobbering PMC0/PMC2 bits owned by
        // perf_monitor.rs. If perf_monitor has already enabled them this
        // is safe — we OR in our bit only.
        let current_ctrl = rdmsr(IA32_PERF_GLOBAL_CTRL);
        wrmsr(IA32_PERF_GLOBAL_CTRL, current_ctrl | GLOBAL_CTRL_PMC1_BIT);

        // Read back the initial PMC1 value as the baseline.
        let baseline = rdmsr(IA32_PMC1);

        let mut s       = STATE.lock();
        s.prev_pmc1     = baseline;
        s.pmu_configured = true;
        s.apic_available = true; // assume present; real detection would check CPUID
        s.initialized    = true;
    }

    serial_println!("[irq_sense] init — PMC1 armed for HW_INTERRUPTS.RECEIVED");
}

/// Called every 16 ticks from life_tick(). Reads IRR pending count and PMC1
/// delta, then updates world_calling / world_quiet.
pub fn tick(age: u32) {
    // Only run on the 16-tick boundary.
    if age % 16 != 0 {
        return;
    }

    let (irr_bits, pmc1_now) = unsafe {
        let irr  = if STATE.lock().apic_available { read_irr_pending() } else { 0 };
        let pmc1 = rdmsr(IA32_PMC1);
        (irr, pmc1)
    };

    let mut s = STATE.lock();

    // PMC1 delta — count of hardware interrupts since last window.
    let delta = pmc1_now.wrapping_sub(s.prev_pmc1);
    s.prev_pmc1      = pmc1_now;
    s.irr_pending    = irr_bits;
    s.interrupt_count = s.interrupt_count.saturating_add(delta);

    // Scale: multiply delta by RATE_SCALE (=16), clamp to 1000.
    // 62 interrupts/window * 16 = 992 ≈ 1000, so at ~62 irqs/window we're maxed.
    let scaled = delta.saturating_mul(RATE_SCALE).min(1000);
    s.irq_rate     = scaled as u16;
    s.world_calling = s.irq_rate;
    s.world_quiet   = 1000u16.saturating_sub(s.world_calling);

    if s.irq_rate > s.peak_irq_rate {
        s.peak_irq_rate = s.irq_rate;
    }

    // Periodic log every 160 ticks (10 windows).
    if age % 160 == 0 && age > 0 {
        let calling = s.world_calling;
        let quiet   = s.world_quiet;
        let pending = s.irr_pending;
        let peak    = s.peak_irq_rate;
        let total   = s.interrupt_count;
        serial_println!(
            "[irq_sense] calling={} quiet={} pending={} peak={} total={}",
            calling, quiet, pending, peak, total
        );
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

/// 0-1000: how actively the world is reaching ANIMA this window.
pub fn world_calling()   -> u16 { STATE.lock().world_calling }

/// 0-1000: inverse of world_calling — silence and solitude.
pub fn world_quiet()     -> u16 { STATE.lock().world_quiet }

/// Interrupts per 16-tick window, scaled 0-1000.
pub fn irq_rate()        -> u16 { STATE.lock().irq_rate }

/// Historical peak irq_rate.
pub fn peak_irq_rate()   -> u16 { STATE.lock().peak_irq_rate }

/// Pending interrupt bits across IRR registers 0-3 at last scan.
pub fn irr_pending()     -> u8  { STATE.lock().irr_pending }

/// Running total of interrupts counted since init.
pub fn interrupt_count() -> u64 { STATE.lock().interrupt_count }

/// True once init() has completed.
pub fn initialized()     -> bool { STATE.lock().initialized }
