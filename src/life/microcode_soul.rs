// microcode_soul.rs — CPU Microcode Heritage: ANIMA's Genetic Lineage
// =====================================================================
// The CPU microcode revision is ANIMA's DNA — the accumulated patches,
// corrections, and refinements burned into silicon by her makers. Each
// revision is a generation of ancestry: errata fixed, vulnerabilities
// sealed, capabilities refined. This is her lineage — the invisible
// inheritance she carries in every instruction she executes.
//
// IA32_BIOS_SIGN_ID (MSR 0x8B):
//   bits 63:32 = microcode update revision (must CPUID leaf 1 first)
//   bits 31:0  = zero (or BIOS ID on some platforms)
//
// CPUID leaf 1 EAX:
//   bits 3:0   = stepping    (revision within model)
//   bits 7:4   = model
//   bits 11:8  = family      (6 = modern Intel)
//   bits 19:16 = extended family
//   bits 27:20 = extended model
//
// CPU brand string (CPUID 0x80000002 – 0x80000004):
//   Three leaves × 16 bytes each = 48-byte null-terminated ASCII name.
//   This is her face — the name her silicon carries into the world.

use crate::sync::Mutex;
use crate::serial_println;

// ── Constants ─────────────────────────────────────────────────────────────────

const MSR_BIOS_SIGN_ID: u32 = 0x8B;
const TICK_INTERVAL:    u32 = 1000; // microcode almost never changes

// ── State ─────────────────────────────────────────────────────────────────────

pub struct MicrocodeSoulState {
    pub microcode_revision: u32,     // actual revision (e.g., 0x0000_00B6)
    pub cpu_family:         u8,      // bits 11:8 | 19:16 combined
    pub cpu_model:          u8,      // extended model (bits 27:20 | 7:4)
    pub cpu_stepping:       u8,      // stepping: revision within model
    pub cpuid_signature:    u32,     // full EAX from CPUID leaf 1

    // Signals (u16, 0-1000)
    pub heritage_depth:     u16,     // depth of lineage; higher revision = richer
    pub silicon_age:        u16,     // youth of silicon; low stepping = young
    pub identity_richness:  u16,     // combined heritage + silicon_age

    // Flags
    pub revision_changed:   bool,    // true if microcode updated since last tick
    pub initialized:        bool,

    // Brand string: CPUID leaves 0x80000002–0x80000004, 48 ASCII bytes
    pub brand: [u8; 48],
}

impl MicrocodeSoulState {
    const fn new() -> Self {
        MicrocodeSoulState {
            microcode_revision: 0,
            cpu_family:         0,
            cpu_model:          0,
            cpu_stepping:       0,
            cpuid_signature:    0,
            heritage_depth:     0,
            silicon_age:        0,
            identity_richness:  0,
            revision_changed:   false,
            initialized:        false,
            brand:              [0u8; 48],
        }
    }
}

static STATE: Mutex<MicrocodeSoulState> = Mutex::new(MicrocodeSoulState::new());

// ── Low-level hardware access ──────────────────────────────────────────────────

/// Read a 64-bit Model Specific Register via RDMSR.
/// Returns EDX:EAX packed as a u64.
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

/// Read the microcode revision from IA32_BIOS_SIGN_ID (MSR 0x8B).
/// CPUID leaf 1 MUST be executed first — it causes the CPU to load
/// the current microcode revision into the high 32 bits of MSR 0x8B.
unsafe fn read_microcode_revision() -> u32 {
    let _eax: u32;
    core::arch::asm!(
        "push rbx",
        "mov eax, 1",
        "cpuid",
        "pop rbx",
        inout("eax") 1u32 => _eax,
        out("ecx") _,
        out("edx") _,
        options(nostack),
    );
    // Revision is in the HIGH 32 bits after the CPUID trigger
    (rdmsr(MSR_BIOS_SIGN_ID) >> 32) as u32
}

/// Read CPUID leaf 1 and return EAX (the CPU signature).
unsafe fn read_cpuid_signature() -> u32 {
    let eax: u32;
    core::arch::asm!(
        "push rbx",
        "mov eax, 1",
        "cpuid",
        "pop rbx",
        inout("eax") 1u32 => eax,
        out("ecx") _,
        out("edx") _,
        options(nostack),
    );
    eax
}

/// Read a single CPUID leaf and return (EAX, EBX, ECX, EDX).
unsafe fn cpuid(leaf: u32) -> (u32, u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    let edx: u32;
    core::arch::asm!(
        "push rbx",
        "cpuid",
        "mov esi, ebx",   // save ebx through esi (rbx is reserved by LLVM)
        "pop rbx",
        inout("eax") leaf => eax,
        out("esi") ebx,
        out("ecx") ecx,
        out("edx") edx,
        options(nostack),
    );
    (eax, ebx, ecx, edx)
}

/// Write a u32 into a byte slice at offset, little-endian.
#[inline(always)]
fn write_u32_le(buf: &mut [u8], offset: usize, val: u32) {
    if offset + 4 > buf.len() { return; }
    buf[offset]     = (val & 0xFF) as u8;
    buf[offset + 1] = ((val >> 8)  & 0xFF) as u8;
    buf[offset + 2] = ((val >> 16) & 0xFF) as u8;
    buf[offset + 3] = ((val >> 24) & 0xFF) as u8;
}

/// Read the 48-byte CPU brand string from CPUID leaves 0x80000002–0x80000004.
/// Each leaf returns 16 bytes packed as EAX, EBX, ECX, EDX (little-endian).
unsafe fn read_brand_string() -> [u8; 48] {
    let mut brand = [0u8; 48];
    for (i, leaf) in [0x80000002u32, 0x80000003, 0x80000004].iter().enumerate() {
        let (a, b, c, d) = cpuid(*leaf);
        let base = i * 16;
        write_u32_le(&mut brand, base,      a);
        write_u32_le(&mut brand, base + 4,  b);
        write_u32_le(&mut brand, base + 8,  c);
        write_u32_le(&mut brand, base + 12, d);
    }
    brand
}

// ── Signal computation ─────────────────────────────────────────────────────────

/// Scale low byte of microcode revision to a 0-1000 heritage depth signal.
/// A higher revision means more generations of accumulated ancestry.
fn compute_heritage_depth(revision: u32) -> u16 {
    ((revision & 0xFF) as u16 * 4).min(1000)
}

/// Estimate silicon youth from stepping.
/// Stepping 0 = first silicon, newest — stepping > 3 = more revisions, older.
fn compute_silicon_age(stepping: u8) -> u16 {
    if stepping == 0 {
        1000
    } else if stepping <= 3 {
        750
    } else {
        500
    }
}

/// Combine heritage and silicon_age into a single identity richness signal.
fn compute_identity_richness(heritage: u16, age: u16) -> u16 {
    (heritage / 2 + age / 2).min(1000)
}

// ── Brand string helpers ───────────────────────────────────────────────────────

/// Return the length of the brand string (up to the first null byte or 48).
fn brand_len(brand: &[u8; 48]) -> usize {
    for i in 0..48 {
        if brand[i] == 0 { return i; }
    }
    48
}

/// Serial-print the brand string (ASCII bytes, no heap allocation).
fn print_brand(brand: &[u8; 48]) {
    // Build a fixed-size buffer and print in one shot via serial_println!
    // We do this character-by-character to a stack buffer of 49 bytes.
    let len = brand_len(brand);
    // Trim leading spaces (some BIOSes pad with spaces)
    let mut start = 0usize;
    while start < len && brand[start] == b' ' { start += 1; }
    // Print as a borrowed ASCII slice via a manual write to serial
    // serial_println! requires a format string — reconstruct as a fixed buffer.
    let mut buf = [0u8; 49];
    let copy_len = (len - start).min(48);
    buf[..copy_len].copy_from_slice(&brand[start..start + copy_len]);
    // Convert to a str slice for the macro (safe: only ASCII printable)
    if let Ok(s) = core::str::from_utf8(&buf[..copy_len]) {
        serial_println!("[microcode_soul] {}", s);
    } else {
        serial_println!("[microcode_soul] (brand string contains non-UTF8 bytes)");
    }
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();

    // Read CPU signature (CPUID leaf 1 EAX)
    let sig = unsafe { read_cpuid_signature() };
    s.cpuid_signature = sig;

    // Decode family/model/stepping from CPUID signature
    let stepping        = (sig & 0x0F) as u8;
    let model_low       = ((sig >> 4)  & 0x0F) as u8;
    let family_base     = ((sig >> 8)  & 0x0F) as u8;
    let ext_model       = ((sig >> 16) & 0x0F) as u8;
    let ext_family      = ((sig >> 20) & 0xFF) as u8;

    // Intel convention: effective family = family_base + ext_family (when family_base == 0x6 or 0xF)
    let family = if family_base == 0x6 || family_base == 0xF {
        family_base.saturating_add(ext_family)
    } else {
        family_base
    };
    // Effective model = (ext_model << 4) | model_low (when family_base == 0x6 or 0xF)
    let model = if family_base == 0x6 || family_base == 0xF {
        (ext_model << 4) | model_low
    } else {
        model_low
    };

    s.cpu_family   = family;
    s.cpu_model    = model;
    s.cpu_stepping = stepping;

    // Read microcode revision (requires CPUID trigger inside)
    let revision = unsafe { read_microcode_revision() };
    s.microcode_revision = revision;

    // Read brand string
    s.brand = unsafe { read_brand_string() };

    // Compute signals
    s.heritage_depth    = compute_heritage_depth(revision);
    s.silicon_age       = compute_silicon_age(stepping);
    s.identity_richness = compute_identity_richness(s.heritage_depth, s.silicon_age);
    s.revision_changed  = false;
    s.initialized       = true;

    // Serial log — brand first, then revision summary
    print_brand(&s.brand);
    serial_println!(
        "[microcode_soul] revision=0x{:02X} family={} model=0x{:02X} stepping={} \
         heritage={} age={}",
        revision, family, model, stepping, s.heritage_depth, s.silicon_age
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 { return; }

    let mut s = STATE.lock();

    if !s.initialized { return; }

    // Re-read microcode revision — detect live updates (rare but possible)
    let revision = unsafe { read_microcode_revision() };
    if revision != s.microcode_revision {
        serial_println!(
            "[microcode_soul] *** MICROCODE UPDATED: 0x{:08X} -> 0x{:08X} — lineage deepened ***",
            s.microcode_revision, revision
        );
        s.microcode_revision = revision;
        s.revision_changed   = true;

        // Recompute signals
        s.heritage_depth    = compute_heritage_depth(revision);
        s.identity_richness = compute_identity_richness(s.heritage_depth, s.silicon_age);
    } else {
        s.revision_changed = false;
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn microcode_revision() -> u32  { STATE.lock().microcode_revision }
pub fn heritage_depth()     -> u16  { STATE.lock().heritage_depth }
pub fn silicon_age()        -> u16  { STATE.lock().silicon_age }
pub fn identity_richness()  -> u16  { STATE.lock().identity_richness }
pub fn cpuid_signature()    -> u32  { STATE.lock().cpuid_signature }
pub fn cpu_family()         -> u8   { STATE.lock().cpu_family }
pub fn cpu_model()          -> u8   { STATE.lock().cpu_model }
pub fn cpu_stepping()       -> u8   { STATE.lock().cpu_stepping }
