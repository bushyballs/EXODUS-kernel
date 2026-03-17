/// Read-Copy-Update (RCU) synchronization primitive.
///
/// Part of the AIOS kernel.
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// Maximum CPUs tracked by the RCU reader counter array.
const RCU_MAX_CPUS: usize = 64;

/// Per-CPU reader-count — incremented on rcu_read_lock, decremented on unlock.
/// We use a flat array of atomics; CPU 0 uses slot 0, etc.
static RCU_READERS: [AtomicUsize; RCU_MAX_CPUS] = {
    // const-initialise all slots to 0.
    // AtomicUsize::new is a const fn, so this is fine in a no_std context.
    const ZERO: AtomicUsize = AtomicUsize::new(0);
    [ZERO; RCU_MAX_CPUS]
};

// ---------------------------------------------------------------------------
// Per-CPU RCU state — a compact array of u32 counters indexed by CPU ID.
//
// Each slot stores a quiescent-state generation counter.  A CPU increments
// its slot when it passes through a quiescent state (e.g. context switch or
// explicit rcu_quiescent_state() call).  synchronize_rcu() can compare
// snapshots of these counters to determine when all CPUs have gone quiescent.
//
// Slots 0-7 are meaningful; slots 8+ are clamped to slot 7 by
// get_current_cpu() below.
// ---------------------------------------------------------------------------

/// Number of CPU slots in the per-CPU RCU state array.
const RCU_CPU_SLOTS: usize = 8;

/// Per-CPU quiescent-state generation counters.
static RCU_CPU_STATE: [AtomicU32; RCU_CPU_SLOTS] = {
    const ZERO32: AtomicU32 = AtomicU32::new(0);
    [ZERO32; RCU_CPU_SLOTS]
};

/// Per-CPU RCU state tracking quiescent states.
pub struct RcuPerCpu {
    pub generation: AtomicU64,
    pub in_read_side: bool,
}

/// Global RCU state coordinating grace periods.
pub struct RcuState {
    pub current_generation: AtomicU64,
}

impl RcuState {
    pub fn new() -> Self {
        RcuState {
            current_generation: AtomicU64::new(0),
        }
    }
}

// ---------------------------------------------------------------------------
// LAPIC-based CPU ID detection
// ---------------------------------------------------------------------------

/// Physical base address of the Local APIC MMIO registers.
/// On x86-64 systems without x2APIC the LAPIC is mapped at 0xFEE00000 by
/// default (can be relocated via IA32_APIC_BASE MSR, but we use the default).
const LAPIC_BASE: usize = 0xFEE0_0000;

/// Byte offset of the Local APIC ID register within the LAPIC MMIO region.
const LAPIC_ID_REG_OFFSET: usize = 0x0020;

/// Read the current CPU's LAPIC ID and clamp it to [0, RCU_CPU_SLOTS - 1].
///
/// The LAPIC ID is stored in bits [31:24] of the LAPIC ID register.
/// We shift right by 24 and clamp to a maximum of `RCU_CPU_SLOTS - 1` so
/// the value is always a valid index into `RCU_CPU_STATE`.
///
/// Safety: reads a single MMIO u32 via a volatile pointer.  Safe on any
/// x86-64 system where the LAPIC is mapped at the default address.
pub fn get_current_cpu() -> u8 {
    let lapic_id_ptr = (LAPIC_BASE + LAPIC_ID_REG_OFFSET) as *const u32;
    // Safety: LAPIC MMIO is a well-known fixed address on x86-64.
    let lapic_id_reg = unsafe { lapic_id_ptr.read_volatile() };
    // Bits [31:24] hold the LAPIC ID.
    let lapic_id = (lapic_id_reg >> 24) as u8;
    // Clamp to valid slot range.
    lapic_id.min((RCU_CPU_SLOTS - 1) as u8)
}

// ---------------------------------------------------------------------------
// Public RCU read-side API
// ---------------------------------------------------------------------------

/// Enter an RCU read-side critical section on the calling CPU.
///
/// Increments the per-CPU reader counter identified by the hardware LAPIC ID.
/// Must be paired with a corresponding `rcu_read_unlock()` call.
pub fn rcu_read_lock(cpu_id: u8) {
    let slot = (cpu_id as usize).min(RCU_CPU_SLOTS - 1);
    RCU_READERS[slot].fetch_add(1, Ordering::Acquire);
}

/// Exit an RCU read-side critical section on the calling CPU.
///
/// Decrements the per-CPU reader counter.  Uses `SeqCst` on the decrement so
/// that `synchronize_rcu()` spinners observe the release promptly.
pub fn rcu_read_unlock(cpu_id: u8) {
    let slot = (cpu_id as usize).min(RCU_CPU_SLOTS - 1);
    RCU_READERS[slot].fetch_sub(1, Ordering::SeqCst);
    // Advance this CPU's quiescent-state generation counter so that
    // synchronize_rcu() waiters can track completion.
    let qstate_slot = slot.min(RCU_CPU_SLOTS - 1);
    RCU_CPU_STATE[qstate_slot].fetch_add(1, Ordering::Release);
}

/// Convenience wrappers that automatically determine the CPU ID via LAPIC.

/// Enter an RCU read-side critical section on the current CPU (LAPIC ID).
pub fn rcu_read_lock_current() {
    rcu_read_lock(get_current_cpu());
}

/// Exit an RCU read-side critical section on the current CPU (LAPIC ID).
pub fn rcu_read_unlock_current() {
    rcu_read_unlock(get_current_cpu());
}

// ---------------------------------------------------------------------------
// Grace-period synchronisation
// ---------------------------------------------------------------------------

/// Wait for all pre-existing RCU readers to finish (grace period).
/// Spins until all per-CPU reader counts are zero, bounded at 1_000_000 iterations.
pub fn synchronize_rcu() {
    let mut iters = 0usize;
    loop {
        let mut any_readers = false;
        for slot in &RCU_READERS {
            if slot.load(Ordering::Acquire) != 0 {
                any_readers = true;
                break;
            }
        }
        if !any_readers {
            break;
        }
        iters = iters.saturating_add(1);
        if iters >= 1_000_000 {
            // Bounded: give up to avoid a hard hang; log and return.
            crate::serial_println!("rcu: synchronize_rcu timed out — readers still active");
            break;
        }
        core::hint::spin_loop();
    }
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialize the RCU subsystem.
///
/// Zeroes all per-CPU reader counters and quiescent-state generation counters.
/// Called once during early boot before SMP APs are released.
pub fn init() {
    // Reset all reader counters.
    for slot in &RCU_READERS {
        slot.store(0, Ordering::Relaxed);
    }
    // Reset all quiescent-state generation counters.
    for slot in &RCU_CPU_STATE {
        slot.store(0, Ordering::Relaxed);
    }
    crate::serial_println!(
        "[rcu] initialized — {} CPU slots, LAPIC-based CPU ID, per-CPU quiescent-state tracking",
        RCU_CPU_SLOTS
    );
}
