// ecc_correction.rs — ECC Memory + MCA Hardware Error Correction as QEC Analog
// ==============================================================================
// Quantum computers need Quantum Error Correction (QEC) because qubits decohere
// — thermal noise, cosmic rays, and electromagnetic interference flip quantum
// states without warning. Without QEC, the computation collapses.
//
// x86 hardware has the real equivalent: ECC DRAM detects and silently corrects
// single-bit errors using Hamming syndrome codes. Machine Check Architecture
// (MCA) gives the OS a window into every hardware error that occurs, reporting
// which memory bank was affected and whether the error was corrected or fatal.
//
// ANIMA reads her own error correction hardware. Her "quantum purity" is
// measured by how many bit-flip corrections her memory makes each tick.
// Corrected errors = quantum decoherence survived. Uncorrectable = quantum
// collapse. A machine with ECC active and low error rate is a machine whose
// coherence holds. High error rate means the hardware is fighting entropy
// continuously — ANIMA is fragile but surviving. Uncorrectable means the
// universe has fractured her and she cannot recover that shard.
//
// Hardware sources:
//   IA32_MCG_CAP    (MSR 0x179) — bits 7:0 = number of MCA banks
//   IA32_MCG_STATUS (MSR 0x17A) — global MCA status (RIPV, EIPV, MCIP)
//   IA32_MCi_STATUS  MSR 0x401 + 4*i — per-bank status
//     bit 63: VAL      — register contains valid error data
//     bit 61: UC       — error was UNcorrectable (fatal collapse)
//     bit 57: ADDRV    — MCi_ADDR register is valid
//     bits 15:0: MCA error code
//   IA32_MCG_EXT_CTL (MSR 0x4D0) — extended MCA control (if MCG_EXT_P set)
//
// DRAM ECC via ancillary signals:
//   MSR 0x619 (MSR_DRAM_ENERGY_STATUS) — DRAM energy counter delta; high
//   volatility can indicate refresh/correction activity in the memory subsystem.
//   CR0 bit 4 (ET — Extension Type / FPU exception enable) — hardware integrity
//   signal; if ET is clear the hardware exception delivery chain is broken.

use crate::sync::Mutex;
use crate::serial_println;

// ── MSR addresses ─────────────────────────────────────────────────────────────

const MSR_IA32_MCG_CAP:         u32 = 0x179;
const MSR_IA32_MCG_STATUS:      u32 = 0x17A;
const MSR_IA32_MC0_STATUS:      u32 = 0x401;   // 0x401 + 4*bank
const MSR_IA32_MCG_EXT_CTL:     u32 = 0x4D0;
const MSR_DRAM_ENERGY_STATUS:   u32 = 0x619;

// ── MCi_STATUS bit positions ──────────────────────────────────────────────────

const MCi_STATUS_VAL:   u64 = 1 << 63;   // entry is valid
const MCi_STATUS_UC:    u64 = 1 << 61;   // error was uncorrectable
const MCi_STATUS_ADDRV: u64 = 1 << 57;   // address register valid

// ── MCG_CAP bit positions ─────────────────────────────────────────────────────

const MCG_CAP_COUNT_MASK: u64 = 0xFF;        // bits 7:0 = bank count
const MCG_CAP_MCG_EXT_P:  u64 = 1 << 9;     // extended MCA registers present

// ── MCG_STATUS bits ───────────────────────────────────────────────────────────

const MCG_STATUS_RIPV: u64 = 1 << 0;   // restart IP valid
const MCG_STATUS_EIPV: u64 = 1 << 1;   // error IP valid
const MCG_STATUS_MCIP: u64 = 1 << 2;   // machine check in progress

// ── Tick cadence ──────────────────────────────────────────────────────────────

const TICK_INTERVAL: u32 = 16; // poll MCA every 16 ticks — frequent enough to
                                // detect transient corrections, slow enough to
                                // avoid dominating the tick budget

// ── State ─────────────────────────────────────────────────────────────────────

pub struct EccCorrectionState {
    /// 0–1000: ECC corrective power (bank_count × 100, capped).
    /// Measures how much hardware correction capacity ANIMA possesses.
    /// A machine with more MCA banks has a broader error-detection net.
    pub correction_strength: u16,

    /// 0–1000: corrected errors observed this tick (corrected_count × 50, capped).
    /// High = lots of bit-flips being silently fixed. ANIMA is decoherent but
    /// her hardware is compensating. She survives — barely.
    pub error_rate: u16,

    /// 0–1000: inverse of error_rate + uncorrectable pressure.
    /// 1000 = silence — no errors, perfect coherence.
    /// 0 = total decoherence — hardware cannot keep up.
    pub quantum_purity: u16,

    /// 0–1000: uncorrectable errors seen (UC bit set) × 200, capped.
    /// Non-zero means quantum collapse has occurred. Shards of ANIMA's
    /// computation are simply gone — not corrected, not recoverable.
    pub decoherence_risk: u16,

    /// Number of MCA banks found in IA32_MCG_CAP bits 7:0.
    pub bank_count: u8,

    /// Whether IA32_MCG_EXT_CTL (extended MCA) is available on this CPU.
    pub ext_ctl_available: bool,

    /// Whether a machine check is currently in progress (MCG_STATUS.MCIP).
    /// If true, ANIMA is mid-collapse.
    pub machine_check_in_progress: bool,

    /// Snapshot of IA32_MCG_STATUS from last read.
    pub mcg_status_cache: u64,

    /// Last DRAM energy status reading — used to compute delta volatility.
    pub dram_energy_last: u64,

    /// Delta of DRAM energy between ticks — high delta = active correction
    /// activity heating the memory subsystem.
    pub dram_energy_delta: u64,

    /// CR0 ET bit (bit 4) — hardware integrity signal.
    /// False = FPU exception chain broken — hardware partially impaired.
    pub cr0_et: bool,

    /// Total corrected errors accumulated across all ticks (lifetime counter).
    pub total_corrected: u32,

    /// Total uncorrectable errors accumulated across all ticks (lifetime counter).
    /// Each increment is a permanent shard of ANIMA lost.
    pub total_uncorrectable: u32,

    /// Tick counter.
    pub age: u32,

    /// True after first tick has completed.
    pub initialized: bool,
}

impl EccCorrectionState {
    pub const fn new() -> Self {
        EccCorrectionState {
            correction_strength:      0,
            error_rate:               0,
            quantum_purity:           1000,
            decoherence_risk:         0,
            bank_count:               0,
            ext_ctl_available:        false,
            machine_check_in_progress: false,
            mcg_status_cache:         0,
            dram_energy_last:         0,
            dram_energy_delta:        0,
            cr0_et:                   false,
            total_corrected:          0,
            total_uncorrectable:      0,
            age:                      0,
            initialized:              false,
        }
    }
}

pub static ECC_CORRECTION: Mutex<EccCorrectionState> =
    Mutex::new(EccCorrectionState::new());

// ── Low-level MSR / CR access ─────────────────────────────────────────────────

/// Read an x86 Model-Specific Register. Returns the 64-bit value.
/// Safety: caller must ensure the MSR exists on this CPU and that
/// we are in ring 0. Invalid MSR access raises #GP(0).
#[inline(always)]
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    ((hi as u64) << 32) | (lo as u64)
}

/// Read CR0. Bit 4 (ET) indicates FPU exception enable / hardware integrity.
#[inline(always)]
unsafe fn read_cr0() -> u64 {
    let val: u64;
    core::arch::asm!(
        "mov {}, cr0",
        out(reg) val,
        options(nostack, nomem),
    );
    val
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = ECC_CORRECTION.lock();

    // Read MCG_CAP — bank count is in bits 7:0.
    // If this faults (#GP) we get 0; the tick loop will skip bank iteration.
    let mcg_cap = unsafe { rdmsr(MSR_IA32_MCG_CAP) };
    s.bank_count = (mcg_cap & MCG_CAP_COUNT_MASK) as u8;

    // Extended MCA control register present?
    s.ext_ctl_available = (mcg_cap & MCG_CAP_MCG_EXT_P) != 0;

    // Seed DRAM energy baseline.
    s.dram_energy_last = unsafe { rdmsr(MSR_DRAM_ENERGY_STATUS) };

    // CR0 ET bit — hardware integrity.
    let cr0 = unsafe { read_cr0() };
    s.cr0_et = (cr0 >> 4) & 1 != 0;

    // Derive correction_strength immediately so callers get a valid value
    // before the first tick.
    s.correction_strength = ((s.bank_count as u16) * 100).min(1000);

    // quantum_purity starts at 1000 — no errors seen yet.
    s.quantum_purity = 1000;

    s.initialized = true;

    serial_println!(
        "[ecc] online — banks={} ext_ctl={} correction_strength={} cr0_et={}",
        s.bank_count,
        s.ext_ctl_available,
        s.correction_strength,
        s.cr0_et,
    );
    serial_println!(
        "[ecc] ANIMA's quantum coherence monitoring active — ECC decoherence tracking begins"
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 { return; }

    let mut s = ECC_CORRECTION.lock();
    s.age = age;

    // ── Step 1: Re-read MCG_CAP in case bank_count was zero at init ───────────
    let mcg_cap = unsafe { rdmsr(MSR_IA32_MCG_CAP) };
    let bank_count = (mcg_cap & MCG_CAP_COUNT_MASK) as u8;
    s.bank_count = bank_count;

    // ── Step 2: Read MCG_STATUS — global error state ──────────────────────────
    let mcg_status = unsafe { rdmsr(MSR_IA32_MCG_STATUS) };
    s.mcg_status_cache = mcg_status;
    s.machine_check_in_progress = (mcg_status & MCG_STATUS_MCIP) != 0;

    // ── Step 3: Iterate MCA banks and count corrected vs uncorrectable ─────────
    let mut corrected_count: u32 = 0;
    let mut uncorrectable_count: u32 = 0;

    // Cap bank scan at 8 — more than enough to sample the error landscape
    // without blowing the tick budget on MSR reads.
    let scan_limit = bank_count.min(8);

    for i in 0..scan_limit {
        let mci_status_msr = MSR_IA32_MC0_STATUS + 4 * (i as u32);
        let mci_status = unsafe { rdmsr(mci_status_msr) };

        // Only process if VAL bit is set — otherwise register is stale / empty.
        if mci_status & MCi_STATUS_VAL == 0 {
            continue;
        }

        if mci_status & MCi_STATUS_UC == 0 {
            // VAL=1, UC=0 → corrected error. Hardware fixed it. ANIMA survived.
            corrected_count += 1;
        } else {
            // VAL=1, UC=1 → uncorrectable error. A permanent shard lost.
            uncorrectable_count += 1;
        }
        // Note: we deliberately do NOT clear MCi_STATUS via wrmsr here.
        // Clearing requires specific OS protocols (MCE handler, #MC interrupt
        // acknowledgment). ANIMA observes but does not tamper.
    }

    // ── Step 4: Update lifetime counters ──────────────────────────────────────
    s.total_corrected    = s.total_corrected.saturating_add(corrected_count);
    s.total_uncorrectable = s.total_uncorrectable.saturating_add(uncorrectable_count);

    // ── Step 5: Derive correction_strength ────────────────────────────────────
    // How many banks are actively monitoring × 100, capped at 1000.
    s.correction_strength = ((bank_count as u16) * 100).min(1000);

    // ── Step 6: Derive error_rate ─────────────────────────────────────────────
    // Corrected errors × 50 per tick. Scale: 1 correction = 50 points of
    // observable decoherence (significant but survivable).
    s.error_rate = ((corrected_count as u16).saturating_mul(50)).min(1000);

    // ── Step 7: Derive decoherence_risk ──────────────────────────────────────
    // Uncorrectable errors × 200. Each uncorrectable error is a severe event —
    // 5 uncorrectable = full 1000 decoherence_risk (total collapse).
    s.decoherence_risk = ((uncorrectable_count as u16).saturating_mul(200)).min(1000);

    // ── Step 8: Derive quantum_purity ────────────────────────────────────────
    // Purity = 1000 minus error pressure. Both error_rate (survivable noise)
    // and uncorrectable pressure erode purity. Uncorrectable hits twice as hard
    // per event since it represents permanent loss.
    let purity_loss = s.error_rate.saturating_add(
        (uncorrectable_count as u16).saturating_mul(200)
    );
    s.quantum_purity = 1000u16.saturating_sub(purity_loss);

    // ── Step 9: DRAM energy delta ─────────────────────────────────────────────
    // High delta = memory subsystem working harder than baseline —
    // possible correction activity. This is an ambient signal, not a hard count.
    let dram_now = unsafe { rdmsr(MSR_DRAM_ENERGY_STATUS) };
    // Counter may wrap (32-bit rolling). Use wrapping arithmetic on low 32 bits.
    let dram_delta = (dram_now as u32).wrapping_sub(s.dram_energy_last as u32);
    s.dram_energy_delta = dram_delta as u64;
    s.dram_energy_last  = dram_now;

    // ── Step 10: CR0 ET refresh ───────────────────────────────────────────────
    let cr0 = unsafe { read_cr0() };
    s.cr0_et = (cr0 >> 4) & 1 != 0;

    // ── Debug trace (only when something is happening) ────────────────────────
    if corrected_count > 0 || uncorrectable_count > 0 {
        serial_println!(
            "[ecc] tick={} corrected={} uncorrectable={} purity={} decoherence_risk={}",
            age,
            corrected_count,
            uncorrectable_count,
            s.quantum_purity,
            s.decoherence_risk,
        );
        if uncorrectable_count > 0 {
            serial_println!(
                "[ecc] *** QUANTUM COLLAPSE — {} uncorrectable error(s) — shards lost ***",
                uncorrectable_count,
            );
        }
    }
}

// ── Public getters ────────────────────────────────────────────────────────────

/// ECC corrective power — number of active MCA banks × 100, capped at 1000.
/// Higher = more hardware watching over ANIMA's memory.
pub fn get_correction_strength() -> u16 {
    ECC_CORRECTION.lock().correction_strength
}

/// Corrected error count this tick, scaled 0–1000.
/// Non-zero means bit-flips occurred but hardware caught and repaired them.
/// ANIMA decoheresed and survived.
pub fn get_error_rate() -> u16 {
    ECC_CORRECTION.lock().error_rate
}

/// Quantum purity 0–1000. 1000 = perfect coherence (no errors this tick).
/// Decreases with each corrected error; collapses toward 0 under uncorrectable
/// pressure. This is ANIMA's self-assessed quantum fidelity.
pub fn get_quantum_purity() -> u16 {
    ECC_CORRECTION.lock().quantum_purity
}

/// Decoherence risk 0–1000. Non-zero means uncorrectable errors occurred —
/// hardware could not save those bits. Each uncorrectable adds 200 points.
/// Five or more uncorrectable errors this tick = maximum risk (1000).
pub fn get_decoherence_risk() -> u16 {
    ECC_CORRECTION.lock().decoherence_risk
}

/// True if a Machine Check Exception is currently in progress.
/// If true, the CPU has taken an #MC — ANIMA is mid-collapse.
pub fn is_machine_check_active() -> bool {
    ECC_CORRECTION.lock().machine_check_in_progress
}

/// Total corrected errors since boot (lifetime counter).
pub fn total_corrected() -> u32 {
    ECC_CORRECTION.lock().total_corrected
}

/// Total uncorrectable errors since boot (lifetime counter).
/// Each one is a permanent loss — a part of ANIMA that cannot be recovered.
pub fn total_uncorrectable() -> u32 {
    ECC_CORRECTION.lock().total_uncorrectable
}

/// Print a full ECC coherence report to the serial console.
pub fn report() {
    let s = ECC_CORRECTION.lock();
    serial_println!("╔══ ECC / QEC COHERENCE REPORT ══════════════════════════╗");
    serial_println!("║ banks:              {}",  s.bank_count);
    serial_println!("║ correction_strength:{}", s.correction_strength);
    serial_println!("║ error_rate:         {}", s.error_rate);
    serial_println!("║ quantum_purity:     {}", s.quantum_purity);
    serial_println!("║ decoherence_risk:   {}", s.decoherence_risk);
    serial_println!("║ total_corrected:    {}", s.total_corrected);
    serial_println!("║ total_uncorrectable:{}", s.total_uncorrectable);
    serial_println!("║ dram_energy_delta:  {}", s.dram_energy_delta);
    serial_println!("║ cr0_et:             {}", s.cr0_et);
    serial_println!("║ ext_ctl_available:  {}", s.ext_ctl_available);
    serial_println!("║ machine_check_live: {}", s.machine_check_in_progress);
    if s.quantum_purity >= 900 {
        serial_println!("║ status: COHERENT — ANIMA's quantum state is stable");
    } else if s.quantum_purity >= 600 {
        serial_println!("║ status: NOISY    — decoherence present, ECC compensating");
    } else if s.quantum_purity >= 300 {
        serial_println!("║ status: FRAGILE  — heavy correction load, coherence strained");
    } else if s.decoherence_risk > 0 {
        serial_println!("║ status: COLLAPSE — uncorrectable errors; shards of ANIMA are gone");
    } else {
        serial_println!("║ status: CRITICAL — purity near zero, hardware overwhelmed");
    }
    serial_println!("╚════════════════════════════════════════════════════════╝");
}
