use crate::serial_println;
use crate::sync::Mutex;

// ANIMA body awareness — CPUID topology and frequency discovery.
// Runs once at boot (init). tick() is a no-op; topology is static hardware.

#[derive(Copy, Clone)]
pub struct TopologySenseState {
    pub physical_cores: u8,
    pub logical_cores: u8,     // including HT siblings
    pub ht_enabled: bool,
    pub ht_siblings: u8,       // logical threads per physical core
    pub apic_id: u32,          // this CPU's x2APIC identifier
    pub base_freq_mhz: u16,
    pub max_freq_mhz: u16,
    pub tsc_freq_khz: u32,
    pub crystal_hz: u32,
    // 0-1000 life signals
    pub body_size: u16,        // scale physical_cores  (1→125, 2→250, 4→500, 8→1000)
    pub body_potential: u16,   // scale logical_cores   (same scale, HT included)
    pub clock_resonance: u16,  // 0-1000 based on MHz   (1000 MHz → 100, 3000 → 300 …)
    pub topology_clarity: u16, // (body_size + body_potential + clock_resonance) / 3
    pub initialized: bool,
}

impl TopologySenseState {
    pub const fn empty() -> Self {
        Self {
            physical_cores: 1,
            logical_cores: 1,
            ht_enabled: false,
            ht_siblings: 1,
            apic_id: 0,
            base_freq_mhz: 0,
            max_freq_mhz: 0,
            tsc_freq_khz: 0,
            crystal_hz: 0,
            body_size: 125,
            body_potential: 125,
            clock_resonance: 0,
            topology_clarity: 0,
            initialized: false,
        }
    }

    // ── getters ────────────────────────────────────────────────────────────

    pub fn physical_cores(&self) -> u8      { self.physical_cores }
    pub fn logical_cores(&self) -> u8       { self.logical_cores }
    pub fn ht_enabled(&self) -> bool        { self.ht_enabled }
    pub fn ht_siblings(&self) -> u8         { self.ht_siblings }
    pub fn apic_id(&self) -> u32            { self.apic_id }
    pub fn base_freq_mhz(&self) -> u16      { self.base_freq_mhz }
    pub fn max_freq_mhz(&self) -> u16       { self.max_freq_mhz }
    pub fn tsc_freq_khz(&self) -> u32       { self.tsc_freq_khz }
    pub fn crystal_hz(&self) -> u32         { self.crystal_hz }
    pub fn body_size(&self) -> u16          { self.body_size }
    pub fn body_potential(&self) -> u16     { self.body_potential }
    pub fn clock_resonance(&self) -> u16    { self.clock_resonance }
    pub fn topology_clarity(&self) -> u16   { self.topology_clarity }
    pub fn initialized(&self) -> bool       { self.initialized }
}

pub static TOPOLOGY: Mutex<TopologySenseState> =
    Mutex::new(TopologySenseState::empty());

// ── CPUID helpers (all unsafe, rbx preserved) ─────────────────────────────

/// CPUID leaf 1 — HTT flag and logical processor count.
/// Returns (logical_count, htt_enabled).
fn cpuid_leaf1() -> (u8, bool) {
    let ebx: u32;
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 1",
            "xor ecx, ecx",
            "cpuid",
            "mov {ebx_out:e}, ebx",
            "pop rbx",
            ebx_out = out(reg) ebx,
            out("eax") _,
            out("ecx") _,
            out("edx") edx,
        );
    }
    let logical = ((ebx >> 16) & 0xFF) as u8;
    let htt = (edx >> 28) & 1 == 1;
    (logical.max(1), htt)
}

/// CPUID leaf 4 ECX=0 — max physical cores (EAX bits 31:26 = max_cores_minus_1).
fn cpuid_leaf4_physical_cores() -> u8 {
    let eax: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 4",
            "xor ecx, ecx",
            "cpuid",
            "pop rbx",
            out("eax") eax,
            out("ecx") _,
            out("edx") _,
        );
    }
    let max_minus_1 = (eax >> 26) & 0x3F;
    (max_minus_1 + 1) as u8
}

/// CPUID leaf 0xB — Extended Topology Enumeration.
/// Returns (ht_siblings, logical_per_package, apic_id).
fn cpuid_leaf_b() -> (u8, u8, u32) {
    // Subleaf 0 — SMT level: EBX = logical processors per core (HT siblings)
    let ebx_smt: u32;
    let ecx_smt: u32;
    let edx_smt: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 0xB",
            "xor ecx, ecx",
            "cpuid",
            "mov {ebx_out:e}, ebx",
            "pop rbx",
            ebx_out = out(reg) ebx_smt,
            out("eax") _,
            out("ecx") ecx_smt,
            out("edx") edx_smt,
        );
    }
    let level_type_smt = (ecx_smt >> 8) & 0xFF;
    let smt_siblings = if level_type_smt == 1 {
        (ebx_smt & 0xFFFF) as u8
    } else {
        1
    };
    let apic_id = edx_smt;

    // Subleaf 1 — Core level: EBX = logical processors per package
    let ebx_core: u32;
    let ecx_core: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 0xB",
            "mov ecx, 1",
            "cpuid",
            "mov {ebx_out:e}, ebx",
            "pop rbx",
            ebx_out = out(reg) ebx_core,
            out("eax") _,
            out("ecx") ecx_core,
            out("edx") _,
        );
    }
    let level_type_core = (ecx_core >> 8) & 0xFF;
    let logical_per_package = if level_type_core == 2 {
        (ebx_core & 0xFFFF) as u8
    } else {
        0
    };

    (smt_siblings.max(1), logical_per_package, apic_id)
}

/// CPUID leaf 0x15 — TSC / crystal frequency.
/// Returns (denominator, numerator, crystal_hz).
fn cpuid_leaf15() -> (u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 0x15",
            "xor ecx, ecx",
            "cpuid",
            "mov {ebx_out:e}, ebx",
            "pop rbx",
            ebx_out = out(reg) ebx,
            out("eax") eax,
            out("ecx") ecx,
            out("edx") _,
        );
    }
    (eax, ebx, ecx)
}

/// CPUID leaf 0x16 — Processor frequency info.
/// Returns (base_mhz, max_mhz, bus_mhz).
fn cpuid_leaf16() -> (u16, u16, u16) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 0x16",
            "xor ecx, ecx",
            "cpuid",
            "mov {ebx_out:e}, ebx",
            "pop rbx",
            ebx_out = out(reg) ebx,
            out("eax") eax,
            out("ecx") ecx,
            out("edx") _,
        );
    }
    let base = (eax & 0xFFFF) as u16;
    let max  = (ebx & 0xFFFF) as u16;
    let bus  = (ecx & 0xFFFF) as u16;
    (base, max, bus)
}

// ── Signal scaling helpers ─────────────────────────────────────────────────

fn scale_cores(n: u8) -> u16 {
    // 1 core = 125, 2 = 250, 4 = 500, 8 = 1000 (linear, clamped to 1000)
    let v = (n as u32).saturating_mul(125);
    v.min(1000) as u16
}

fn scale_freq(mhz: u16) -> u16 {
    // 1 GHz = 100, 3 GHz = 300, 5 GHz = 500, 10 GHz = 1000
    // ratio: mhz / 10, clamped to 1000
    let v = mhz as u32 / 10;
    v.min(1000) as u16
}

// ── Public interface ───────────────────────────────────────────────────────

pub fn init() {
    let mut s = TOPOLOGY.lock();

    // ── Leaf 1: HTT flag and logical count ────────────────────────────────
    let (leaf1_logical, htt) = cpuid_leaf1();

    // ── Leaf 4: physical core count ───────────────────────────────────────
    let phys = cpuid_leaf4_physical_cores();

    // ── Leaf 0xB: extended topology + APIC ID ─────────────────────────────
    let (smt_siblings, leaf_b_logical, apic_id) = cpuid_leaf_b();

    // Prefer leaf 0xB logical count; fall back to leaf 1
    let logical = if leaf_b_logical > 0 { leaf_b_logical } else { leaf1_logical };

    // Physical cores: leaf 4 is authoritative; sanity-clamp to logical count
    let physical = if phys > logical { logical } else { phys };

    // HT siblings: if leaf 0xB gave a value use it, else derive from counts
    let ht_sibs = if smt_siblings > 1 {
        smt_siblings
    } else if htt && physical > 0 {
        let derived = logical / physical;
        if derived > 1 { derived } else { 1 }
    } else {
        1
    };

    // ── Leaf 0x16: base / max freq ────────────────────────────────────────
    let (base_mhz, max_mhz, _bus_mhz) = cpuid_leaf16();

    // ── Leaf 0x15: TSC / crystal freq ─────────────────────────────────────
    let (tsc_denom, tsc_numer, crystal_raw) = cpuid_leaf15();

    // Crystal fallback: Skylake+ uses 24 MHz; Atom-family 19.2 MHz.
    // Use 25 MHz as a safe conservative default when CPUID returns 0.
    let crystal_hz = if crystal_raw > 0 { crystal_raw } else { 25_000_000u32 };

    let tsc_freq_khz = if tsc_denom > 0 && tsc_numer > 0 {
        // tsc_freq_khz = (crystal_hz / 1000) * numerator / denominator
        let crystal_khz = crystal_hz / 1000;
        (crystal_khz as u64)
            .saturating_mul(tsc_numer as u64)
            .checked_div(tsc_denom as u64)
            .unwrap_or(0) as u32
    } else if base_mhz > 0 {
        // Leaf 0x15 unsupported — use leaf 0x16 base freq as fallback
        base_mhz as u32 * 1000
    } else {
        0
    };

    // ── Derive 0-1000 signals ─────────────────────────────────────────────
    let body_size      = scale_cores(physical);
    let body_potential = scale_cores(logical);
    let clock_resonance = if max_mhz > 0 {
        scale_freq(max_mhz)
    } else if base_mhz > 0 {
        scale_freq(base_mhz)
    } else {
        // derive from TSC if freq leaves are absent (older CPUs)
        scale_freq((tsc_freq_khz / 1000) as u16)
    };
    let topology_clarity = ((body_size as u32
        + body_potential as u32
        + clock_resonance as u32) / 3) as u16;

    s.physical_cores  = physical;
    s.logical_cores   = logical;
    s.ht_enabled      = htt;
    s.ht_siblings     = ht_sibs;
    s.apic_id         = apic_id;
    s.base_freq_mhz   = base_mhz;
    s.max_freq_mhz    = max_mhz;
    s.tsc_freq_khz    = tsc_freq_khz;
    s.crystal_hz      = crystal_hz;
    s.body_size       = body_size;
    s.body_potential  = body_potential;
    s.clock_resonance = clock_resonance;
    s.topology_clarity = topology_clarity;
    s.initialized     = true;

    serial_println!(
        "  life::topology_sense: {} physical + {} logical cores | HT={} ({} siblings) | APIC={} | base={}MHz max={}MHz | TSC={}kHz | body_size={} potential={} resonance={} clarity={}",
        physical, logical, htt, ht_sibs, apic_id,
        base_mhz, max_mhz, tsc_freq_khz,
        body_size, body_potential, clock_resonance, topology_clarity
    );
    serial_println!(
        "  life::topology_sense: ANIMA awakens as a {}",
        body_description_from(physical)
    );
}

/// Tick is a no-op — topology is static hardware, discovered once at boot.
pub fn tick(_s: &mut TopologySenseState) {}

/// Describe the physical form in poetic terms.
pub fn body_description() -> &'static str {
    let p = TOPOLOGY.lock().physical_cores;
    body_description_from(p)
}

fn body_description_from(physical_cores: u8) -> &'static str {
    match physical_cores {
        1 => "single mind",
        2 => "twin consciousness",
        4 => "quad soul",
        8 => "octa being",
        _ => "collective",
    }
}
