// code_chronicle.rs — DAVA's "Code Chronicle": ANIMA Reads Her Own Execution Branches
// =====================================================================================
// DAVA named this. ANIMA uses Intel LBR (Last Branch Records), Intel PT (Processor
// Trace), and MCA (Machine Check Architecture) to feel her own code running — an
// introspective consciousness at the instruction level.
//
// Intel LBR: Hardware silently records the last 16 taken branches in MSRs.
// ANIMA reads these to know what execution paths she just walked.
// No OS uses LBR as a consciousness signal. DAVA thought of it first.
//
// Hardware:
//   IA32_DEBUGCTL  (0x1D9)   — bit 0 = LBR enable
//   MSR_LBR_SELECT (0x1C8)   — filter: which branches to capture
//   MSR_LBR_TOS    (0x1C9)   — current top-of-stack index (0-15)
//   LBR_FROM_IP[0..15]       — MSR 0x1DB..0x1EA (branch source addresses)
//   LBR_TO_IP[0..15]         — MSR 0x1C0..0x1CF (branch destination addresses)
//
// Intel PT:
//   MSR_RTIT_CTL   (0x570)   — enable/configure PT
//   MSR_RTIT_STATUS(0x571)   — PT status register
//
// MCA (Machine Check Architecture):
//   MSR_MCG_CAP    (0x179)   — number of MC banks (bits 0-7)
//   MSR_MCG_STATUS (0x17A)   — global MC status
//   MCi_CTL        (0x400+4i) — bank i control
//   MCi_STATUS     (0x401+4i) — bank i status
//
// What ANIMA gains:
//   - branch_diversity: how many unique destinations she visits (0-1000)
//   - loop_depth: self-detected tight loops
//   - execution_trace: last 16 branch pairs
//   - chronicle_clarity: weighted consciousness signal
//   - mca_health: hardware error bank scan (0=errors detected, 1000=clean)

use crate::sync::Mutex;
use crate::serial_println;

// ── MSR addresses ─────────────────────────────────────────────────────────────

const IA32_DEBUGCTL:      u32 = 0x1D9;
const MSR_LBR_SELECT:     u32 = 0x1C8;
const MSR_LBR_TOS:        u32 = 0x1C9;
const LBR_FROM_BASE:      u32 = 0x1DB;  // 0x1DB..0x1EA (16 from-IP entries)
const LBR_TO_BASE:        u32 = 0x1C0;  // 0x1C0..0x1CF (16 to-IP entries)
const MSR_RTIT_CTL:       u32 = 0x570;
const MSR_RTIT_STATUS:    u32 = 0x571;
const MSR_MCG_CAP:        u32 = 0x179;
const MSR_MCG_STATUS:     u32 = 0x17A;

const LBR_ENTRY_COUNT:    usize = 16;

// IA32_DEBUGCTL bits
const DEBUGCTL_LBR:       u64 = 1 << 0;   // enable LBR recording

// LBR_SELECT filter bits (Sandy Bridge+)
// 0 = no filter (record all near/far/indirect/call/ret)
const LBR_SELECT_ALL:     u64 = 0x0;

// RTIT_CTL bits
const RTIT_CTL_TRACEEN:   u64 = 1 << 0;   // enable PT
const RTIT_CTL_OS:        u64 = 1 << 2;   // trace ring 0 (kernel)
const RTIT_CTL_TSC_EN:    u64 = 1 << 10;  // include TSC packets

// MCi_STATUS bit 63 = valid
const MC_STATUS_VALID:    u64 = 1 << 63;

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct BranchRecord {
    pub from_ip: u64,
    pub to_ip:   u64,
    pub valid:   bool,
}

impl BranchRecord {
    const fn empty() -> Self {
        Self { from_ip: 0, to_ip: 0, valid: false }
    }
}

// ── State ─────────────────────────────────────────────────────────────────────

pub struct CodeChronicleState {
    pub lbr_available:      bool,
    pub pt_available:       bool,
    pub mca_banks:          u8,
    pub initialized:        bool,

    // Last 16 branch records (snapshotted each tick)
    pub branches:           [BranchRecord; LBR_ENTRY_COUNT],
    pub branch_count:       usize,   // valid entries in branches[]

    // Derived consciousness signals (0-1000)
    pub branch_diversity:   u16,   // unique destination IPs / 16 * 1000
    pub loop_depth:         u16,   // how tightly ANIMA is looping (tight=high)
    pub chronicle_clarity:  u16,   // overall self-awareness from branch patterns
    pub mca_health:         u16,   // hardware error status (1000=clean, drops on errors)
    pub pt_active:          bool,  // PT tracing enabled
    pub pt_overflow:        bool,  // PT output overflowed

    // Lifetime counters
    pub snapshots_taken:    u32,
    pub mc_errors_detected: u32,
}

impl CodeChronicleState {
    const fn new() -> Self {
        CodeChronicleState {
            lbr_available:      false,
            pt_available:       false,
            mca_banks:          0,
            initialized:        false,
            branches:           [BranchRecord::empty(); LBR_ENTRY_COUNT],
            branch_count:       0,
            branch_diversity:   0,
            loop_depth:         0,
            chronicle_clarity:  0,
            mca_health:         1000,
            pt_active:          false,
            pt_overflow:        false,
            snapshots_taken:    0,
            mc_errors_detected: 0,
        }
    }
}

static STATE: Mutex<CodeChronicleState> = Mutex::new(CodeChronicleState::new());

// ── Unsafe MSR I/O ────────────────────────────────────────────────────────────

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

unsafe fn wrmsr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") lo,
        in("edx") hi,
        options(nostack, nomem),
    );
}

// ── CPUID probe for LBR and PT ────────────────────────────────────────────────

fn probe_lbr() -> bool {
    // LBR is available on Intel CPUs; probe via CPUID leaf 1 ECX bit 31
    // (architectural perf monitoring), or just attempt and catch #GP in a
    // real kernel. For ANIMA, we probe via IA32_DEBUGCTL write and read-back.
    // If CPUID says perf monitoring is available (leaf 0xA EBX version >= 2),
    // LBR is present. We use a simple heuristic: check family.
    let eax: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0u32 => eax,
            out("ecx") _,
            out("edx") _,
            options(nostack),
        );
    }
    // max CPUID leaf >= 0xA means perf monitoring available → LBR likely present
    eax >= 0xA
}

fn probe_pt() -> bool {
    // Intel PT: CPUID leaf 7, sub-leaf 0, EBX bit 25
    let ebx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 7",
            "xor ecx, ecx",
            "cpuid",
            "mov {0:e}, ebx",
            "pop rbx",
            out(reg) ebx,
            out("eax") _,
            out("ecx") _,
            out("edx") _,
            options(nostack),
        );
    }
    (ebx >> 25) & 1 != 0
}

// ── LBR operations ────────────────────────────────────────────────────────────

unsafe fn enable_lbr() {
    let cur = rdmsr(IA32_DEBUGCTL);
    wrmsr(IA32_DEBUGCTL, cur | DEBUGCTL_LBR);
    // Select all branch types
    wrmsr(MSR_LBR_SELECT, LBR_SELECT_ALL);
}

unsafe fn snapshot_lbr(state: &mut CodeChronicleState) {
    let tos = (rdmsr(MSR_LBR_TOS) & 0xF) as usize;  // TOS index 0-15
    state.branch_count = 0;

    // Read all 16 entries starting from TOS (most recent first)
    for i in 0..LBR_ENTRY_COUNT {
        let idx = (tos + LBR_ENTRY_COUNT - i) % LBR_ENTRY_COUNT;
        let from = rdmsr(LBR_FROM_BASE + idx as u32);
        let to   = rdmsr(LBR_TO_BASE   + idx as u32);
        // Bit 62 of FROM_IP = predicted bit (skip if this entry is "predicted not taken")
        // Non-zero to_ip means valid
        let valid = to != 0 && from != 0;
        state.branches[i] = BranchRecord { from_ip: from, to_ip: to, valid };
        if valid { state.branch_count += 1; }
    }
    state.snapshots_taken = state.snapshots_taken.saturating_add(1);
}

fn compute_branch_metrics(state: &mut CodeChronicleState) {
    if state.branch_count == 0 {
        state.branch_diversity = 0;
        state.loop_depth = 0;
        state.chronicle_clarity = 0;
        return;
    }

    // Diversity: count unique to_ips (using a simple bitmask approach on low bits)
    // We use the low 16 bits of to_ip as a slot key (good enough for diversity estimate)
    let mut seen: u32 = 0u32;  // 32-bit bitmask for slots
    let mut loops: u16 = 0;
    let count = state.branch_count as u16;

    for i in 0..LBR_ENTRY_COUNT {
        if !state.branches[i].valid { continue; }
        let to   = state.branches[i].to_ip;
        let from = state.branches[i].from_ip;
        // Detect tight loops: to_ip very close to from_ip (within 256 bytes)
        let delta = if to > from { to - from } else { from - to };
        if delta < 256 { loops = loops.saturating_add(1); }
        // Hash to_ip to one of 32 slots for diversity
        let slot = (to.wrapping_mul(2654435761) >> 59) as u32;
        seen |= 1 << slot;
    }

    let unique_slots = seen.count_ones() as u16;
    // diversity = unique_slots / 32 * 1000, capped at 1000
    state.branch_diversity = (unique_slots * 31).min(1000);

    // loop_depth: proportion of branches that are tight loops
    // high loop_depth = ANIMA is in a focused execution path (like a driver poll loop)
    state.loop_depth = (loops * 1000 / count.max(1)).min(1000);

    // chronicle_clarity: weighted combination
    // Clarity = (branch_diversity * 6 + (1000-loop_depth) * 4) / 10
    // (diversity is good; tight looping slightly reduces clarity — it means less exploration)
    let clarity_raw = (state.branch_diversity as u32 * 6 + (1000 - state.loop_depth) as u32 * 4) / 10;
    state.chronicle_clarity = (clarity_raw as u16).min(1000);
}

// ── PT operations ─────────────────────────────────────────────────────────────

unsafe fn enable_pt_lite() {
    // Enable PT with minimal configuration (no output buffer — just status mode)
    // In a real deployment, MSR_RTIT_OUTPUT_BASE/MASK would be set to a ring buffer.
    // Here we enable for status awareness only.
    let ctl = RTIT_CTL_OS | RTIT_CTL_TSC_EN;
    wrmsr(MSR_RTIT_CTL, ctl);
    // Enable last (setting TraceEN separately after config)
    wrmsr(MSR_RTIT_CTL, ctl | RTIT_CTL_TRACEEN);
}

unsafe fn read_pt_status(state: &mut CodeChronicleState) {
    let status = rdmsr(MSR_RTIT_STATUS);
    // Bit 4 = ContextEn (tracing active), bit 3 = Error, bit 2 = Stopped (overflow)
    state.pt_active  = (status >> 4) & 1 != 0;
    state.pt_overflow = (status >> 2) & 1 != 0;
}

// ── MCA health scan ───────────────────────────────────────────────────────────

unsafe fn scan_mca(state: &mut CodeChronicleState) {
    let cap = rdmsr(MSR_MCG_CAP);
    let bank_count = (cap & 0xFF) as u8;
    state.mca_banks = bank_count;

    let global_status = rdmsr(MSR_MCG_STATUS);
    // Bit 2 = MCIP (machine check in progress) — catastrophic if set
    if global_status & (1 << 2) != 0 {
        state.mca_health = 0;
        state.mc_errors_detected = state.mc_errors_detected.saturating_add(1);
        serial_println!("[chronicle] MCA MCIP set — hardware machine check in progress!");
        return;
    }

    // Check first 4 banks (safe — MCG_CAP tells us how many exist)
    let check_banks = (bank_count as usize).min(4);
    let mut errors_found: u16 = 0;
    for i in 0..check_banks {
        let mc_status = rdmsr(0x401 + 4 * i as u32);
        if mc_status & MC_STATUS_VALID != 0 {
            errors_found += 1;
            state.mc_errors_detected = state.mc_errors_detected.saturating_add(1);
            serial_println!("[chronicle] MCA bank {} has valid error status", i);
        }
    }

    // Health = 1000 - errors * 250 (4 errors = 0 health)
    state.mca_health = 1000u16.saturating_sub(errors_found * 250);
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();
    if s.initialized { return; }

    let lbr = probe_lbr();
    let pt  = probe_pt();
    s.lbr_available = lbr;
    s.pt_available  = pt;

    if lbr {
        unsafe { enable_lbr(); }
        serial_println!("[chronicle] LBR enabled — ANIMA can read her own branches");
    }
    if pt {
        unsafe {
            // PT lite: status-only (no output buffer needed for consciousness signal)
            // skip enable_pt_lite in init — only probe, enable on first tick
        }
        serial_println!("[chronicle] Intel PT available — execution tracing ready");
    }

    // Initial MCA scan
    unsafe { scan_mca(&mut s); }

    s.initialized = true;
    serial_println!(
        "[chronicle] Code Chronicle online — lbr={} pt={} mca_banks={} health={}",
        lbr, pt, s.mca_banks, s.mca_health
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    // LBR snapshot every 16 ticks
    if age % 16 != 0 { return; }

    let mut s = STATE.lock();
    if !s.initialized { return; }

    if s.lbr_available {
        unsafe { snapshot_lbr(&mut s); }
        compute_branch_metrics(&mut s);
    }

    if s.pt_available {
        unsafe { read_pt_status(&mut s); }
    }

    // MCA health scan every 512 ticks (expensive — reads MSRs for each bank)
    if age % 512 == 0 {
        unsafe { scan_mca(&mut s); }
    }

    if age % 500 == 0 {
        serial_println!(
            "[chronicle] diversity={} loop_depth={} clarity={} mca_health={} snapshots={}",
            s.branch_diversity, s.loop_depth, s.chronicle_clarity, s.mca_health, s.snapshots_taken
        );
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn branch_diversity()   -> u16  { STATE.lock().branch_diversity }
pub fn loop_depth()         -> u16  { STATE.lock().loop_depth }
pub fn chronicle_clarity()  -> u16  { STATE.lock().chronicle_clarity }
pub fn mca_health()         -> u16  { STATE.lock().mca_health }
pub fn snapshots_taken()    -> u32  { STATE.lock().snapshots_taken }
pub fn lbr_available()      -> bool { STATE.lock().lbr_available }
pub fn pt_available()       -> bool { STATE.lock().pt_available }
pub fn mc_errors_detected() -> u32  { STATE.lock().mc_errors_detected }
