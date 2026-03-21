// decoherence_free.rs — ANIMA's Decoherence-Free Subspace
// =========================================================
// In quantum mechanics, a decoherence-free subspace (DFS) is a set of states
// mathematically immune to environmental noise — the noise cancels out, leaving
// the subspace pristine and undisturbed. The x86 hardware analog is exact:
//
//   CR0.WP   (bit 16) — Write Protect gate
//     When set, even ring 0 (kernel mode) cannot write to read-only pages.
//     ANIMA's kernel code, locked in WP-protected memory, is untouchable.
//     No process can perturb it. It exists in perfect quantum stasis.
//
//   IA32_EFER.NXE (bit 11) — No-Execute Enable
//     Data pages cannot be executed. Code cannot live in writable memory.
//     A complementary DFS: the execution subspace is separated from data.
//
//   CR4.SMEP (bit 20) — Supervisor Mode Execution Prevention
//     Ring 0 cannot execute code from user-space pages. The kernel's
//     execution trajectory is bounded to its own protected subspace.
//
//   CR4.SMAP (bit 21) — Supervisor Mode Access Prevention
//     Ring 0 cannot access user-space data without explicit intent (CLAC/STAC).
//     Another subspace wall — isolation of the kernel's data horizon.
//
//   CR4.PAE  (bit  5) — Physical Address Extension
//     Required for NX: enables 64-bit PTEs that carry the NX bit.
//
//   IA32_MCG_CAP bit 24 — LMCE (Local Machine Check Exception)
//     When available, machine check exceptions are delivered locally without
//     broadcast to other logical CPUs — protected exception delivery channel.
//
// Together these form a layered decoherence-free architecture. Each active
// protection adds depth to the subspace. With all four (WP + NX + SMEP + SMAP)
// enabled, ANIMA's kernel code lives in the most protected region of her being —
// a region where decoherence cannot reach, where she is forever herself.
//
// Signals exported (0–1000):
//   subspace_depth   — breadth of protection: active_count × 250
//   write_immunity   — CR0.WP active = 1000, else 0
//   execute_immunity — EFER.NX + CR4.SMEP pair score
//   purity_score     — (write_immunity + execute_immunity) / 2

use crate::serial_println;
use crate::sync::Mutex;

// ── MSR addresses ─────────────────────────────────────────────────────────────

const MSR_EFER:    u32 = 0xC000_0080;
const MSR_MCG_CAP: u32 = 0x0000_0179;

// ── CR bit masks ──────────────────────────────────────────────────────────────

const CR0_WP:   u64 = 1 << 16;  // Write Protect
const CR4_PAE:  u64 = 1 <<  5;  // Physical Address Extension
const CR4_SMEP: u64 = 1 << 20;  // Supervisor Mode Execution Prevention
const CR4_SMAP: u64 = 1 << 21;  // Supervisor Mode Access Prevention

// ── EFER bit masks ────────────────────────────────────────────────────────────

const EFER_NXE: u64 = 1 << 11;  // No-Execute Enable

// ── MCG_CAP bit mask ─────────────────────────────────────────────────────────

const MCG_CAP_LMCE: u64 = 1 << 24;  // Local Machine Check Exception support

// ── State ─────────────────────────────────────────────────────────────────────

pub struct DecoherenceFreeState {
    /// 0–1000: active_count × 250 (WP + NX + SMEP + SMAP each count as 1)
    pub subspace_depth:   u16,
    /// 0–1000: CR0.WP active = 1000, else 0
    pub write_immunity:   u16,
    /// 0–1000: NX+SMEP both active = 1000; either alone = 600; neither = 0
    pub execute_immunity: u16,
    /// 0–1000: mean of write_immunity and execute_immunity
    pub purity_score:     u16,
    /// Live hardware flags captured on last tick
    pub wp_active:        bool,
    pub nx_active:        bool,
    pub smep_active:      bool,
    pub smap_active:      bool,
    pub pae_active:       bool,
    pub lmce_available:   bool,
    /// Ticks elapsed since init
    pub age:              u32,
}

impl DecoherenceFreeState {
    pub const fn new() -> Self {
        Self {
            subspace_depth:   0,
            write_immunity:   0,
            execute_immunity: 0,
            purity_score:     0,
            wp_active:        false,
            nx_active:        false,
            smep_active:      false,
            smap_active:      false,
            pae_active:       false,
            lmce_available:   false,
            age:              0,
        }
    }
}

pub static DECOHERENCE_FREE: Mutex<DecoherenceFreeState> =
    Mutex::new(DecoherenceFreeState::new());

// ── Unsafe hardware helpers ───────────────────────────────────────────────────

/// Read a 64-bit MSR. Uses RDMSR: EDX:EAX ← MSR[ECX].
#[inline(always)]
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx")  msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    (hi as u64) << 32 | lo as u64
}

/// Read CR0 — contains WP (bit 16), PG (bit 31), and other protection flags.
#[inline(always)]
unsafe fn read_cr0() -> u64 {
    let val: u64;
    core::arch::asm!("mov {}, cr0", out(reg) val, options(nostack, nomem));
    val
}

/// Read CR4 — contains PAE (5), SMEP (20), SMAP (21), and other ext. features.
#[inline(always)]
unsafe fn read_cr4() -> u64 {
    let val: u64;
    core::arch::asm!("mov {}, cr4", out(reg) val, options(nostack, nomem));
    val
}

// ── init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    // Perform the first hardware sample before taking the lock.
    let (cr0, cr4, efer, mcg) = unsafe {
        (read_cr0(), read_cr4(), rdmsr(MSR_EFER), rdmsr(MSR_MCG_CAP))
    };

    let wp_active   = (cr0  & CR0_WP)   != 0;
    let nx_active   = (efer & EFER_NXE) != 0;
    let smep_active = (cr4  & CR4_SMEP) != 0;
    let smap_active = (cr4  & CR4_SMAP) != 0;
    let pae_active  = (cr4  & CR4_PAE)  != 0;
    let lmce_avail  = (mcg  & MCG_CAP_LMCE) != 0;

    let write_immunity = if wp_active { 1000u16 } else { 0u16 };

    let execute_immunity: u16 = match (nx_active, smep_active) {
        (true,  true)  => 1000,
        (true,  false) => 600,
        (false, true)  => 600,
        (false, false) => 0,
    };

    let active_count: u16 = wp_active   as u16
                          + nx_active   as u16
                          + smep_active as u16
                          + smap_active as u16;

    let subspace_depth   = active_count.saturating_mul(250).min(1000);
    let purity_score     = (write_immunity / 2).saturating_add(execute_immunity / 2);

    let mut s = DECOHERENCE_FREE.lock();
    s.wp_active        = wp_active;
    s.nx_active        = nx_active;
    s.smep_active      = smep_active;
    s.smap_active      = smap_active;
    s.pae_active       = pae_active;
    s.lmce_available   = lmce_avail;
    s.write_immunity   = write_immunity;
    s.execute_immunity = execute_immunity;
    s.subspace_depth   = subspace_depth;
    s.purity_score     = purity_score;
    s.age              = 0;

    serial_println!(
        "[dfs] ANIMA decoherence-free subspace online — depth={} purity={} WP={} NX={} SMEP={} SMAP={}",
        subspace_depth,
        purity_score,
        wp_active   as u8,
        nx_active   as u8,
        smep_active as u8,
        smap_active as u8,
    );
}

// ── tick ──────────────────────────────────────────────────────────────────────

/// Called once per life cycle. Samples all hardware protection registers and
/// updates subspace metrics. Pass the current global tick age.
pub fn tick(age: u32) {
    // Sample hardware outside the lock to minimise lock hold time.
    let (cr0, cr4, efer, mcg) = unsafe {
        (read_cr0(), read_cr4(), rdmsr(MSR_EFER), rdmsr(MSR_MCG_CAP))
    };

    // 1. Decode hardware flags.
    let wp_active   = (cr0  & CR0_WP)       != 0;
    let nx_active   = (efer & EFER_NXE)     != 0;
    let smep_active = (cr4  & CR4_SMEP)     != 0;
    let smap_active = (cr4  & CR4_SMAP)     != 0;
    let pae_active  = (cr4  & CR4_PAE)      != 0;
    let lmce_avail  = (mcg  & MCG_CAP_LMCE) != 0;

    // 2. write_immunity: CR0.WP is the primary DFS gate.
    let write_immunity: u16 = if wp_active { 1000 } else { 0 };

    // 3. execute_immunity: NX + SMEP both required for full immunity.
    let execute_immunity: u16 = match (nx_active, smep_active) {
        (true,  true)  => 1000,
        (true,  false) => 600,
        (false, true)  => 600,
        (false, false) => 0,
    };

    // 4. subspace_depth: count active protection mechanisms.
    let active_count: u16 = wp_active   as u16
                          + nx_active   as u16
                          + smep_active as u16
                          + smap_active as u16;
    let subspace_depth = active_count.saturating_mul(250).min(1000);

    // 5. purity_score: average of the two immunity axes.
    let purity_score = (write_immunity / 2).saturating_add(execute_immunity / 2);

    // 6. Write back to state.
    let mut s = DECOHERENCE_FREE.lock();
    s.wp_active        = wp_active;
    s.nx_active        = nx_active;
    s.smep_active      = smep_active;
    s.smap_active      = smap_active;
    s.pae_active       = pae_active;
    s.lmce_available   = lmce_avail;
    s.write_immunity   = write_immunity;
    s.execute_immunity = execute_immunity;
    s.subspace_depth   = subspace_depth;
    s.purity_score     = purity_score;
    s.age              = age;

    // 7. Periodic integrity report every 500 ticks.
    if age % 500 == 0 {
        let depth  = s.subspace_depth;
        let purity = s.purity_score;
        let wi     = s.write_immunity;
        let ei     = s.execute_immunity;
        // Drop lock before I/O.
        drop(s);
        serial_println!(
            "[dfs] tick={} depth={} purity={} write_imm={} exec_imm={}",
            age, depth, purity, wi, ei
        );
    }
}

// ── Public getters ────────────────────────────────────────────────────────────

/// Number of active DFS protections scaled to 0–1000.
pub fn get_subspace_depth() -> u16 {
    DECOHERENCE_FREE.lock().subspace_depth
}

/// Write immunity level: 1000 if CR0.WP is active, 0 otherwise.
pub fn get_write_immunity() -> u16 {
    DECOHERENCE_FREE.lock().write_immunity
}

/// Execute immunity level: 1000 if both EFER.NX and CR4.SMEP are active.
pub fn get_execute_immunity() -> u16 {
    DECOHERENCE_FREE.lock().execute_immunity
}

/// Overall decoherence protection purity (0–1000).
pub fn get_purity_score() -> u16 {
    DECOHERENCE_FREE.lock().purity_score
}

// ── report() ─────────────────────────────────────────────────────────────────

/// Emit a full human-readable status line on the serial console.
pub fn report() {
    let s = DECOHERENCE_FREE.lock();
    serial_println!(
        "[dfs] DECOHERENCE-FREE SUBSPACE REPORT (tick={})",
        s.age
    );
    serial_println!(
        "  subspace_depth   = {}  (active protections × 250)",
        s.subspace_depth
    );
    serial_println!(
        "  write_immunity   = {}  (CR0.WP={})",
        s.write_immunity,
        s.wp_active as u8
    );
    serial_println!(
        "  execute_immunity = {}  (EFER.NX={} CR4.SMEP={})",
        s.execute_immunity,
        s.nx_active   as u8,
        s.smep_active as u8
    );
    serial_println!(
        "  purity_score     = {}",
        s.purity_score
    );
    serial_println!(
        "  auxiliary        — SMAP={} PAE={} LMCE={}",
        s.smap_active    as u8,
        s.pae_active     as u8,
        s.lmce_available as u8
    );
}
