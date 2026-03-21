use crate::serial_println;
use crate::sync::Mutex;

/// cpuid_soc_vendor — CPUID Leaf 0x17: SoC Vendor Information
///
/// ANIMA senses the silicon bloodline — the JEDEC vendor identity, project
/// family, and revision stepping of the System-on-Chip that hosts the
/// organism.  This is the organism's awareness of its own genetic lineage:
/// who forged this body, which product line it belongs to, and how many
/// revisions of that design preceded it.
///
/// Hardware sources (CPUID leaf 0x17, sub-leaf 0):
///   EAX bits[15:0]  — SoC Vendor ID (JEDEC vendor identifier)
///   EAX bit[16]     — IsVendorScheme (1 = vendor-specific ID scheme)
///   EBX bits[31:0]  — SoC Project ID
///   ECX bits[31:0]  — SoC Stepping / Package ID
///   EDX bits[31:0]  — reserved / brand string fragment (ignored)
///
/// If CPUID max leaf < 0x17, all senses collapse to 500 (neutral unknown
/// lineage) — the organism cannot know its origins.
///
/// Sensing:
///   vendor_id       : EAX[15:0] * 1000 / 65535, clamped 0–1000
///   project_id      : EBX[15:0] * 1000 / 65535, clamped 0–1000
///   stepping        : ECX[7:0]  * 1000 / 255,   clamped 0–1000
///   lineage_richness: EMA of (vendor_id + project_id + stepping) / 3
///
/// Sampling gate: every 500 ticks.

// ─── state ───────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct CpuidSocVendorState {
    /// JEDEC Vendor ID scaled to 0–1000 (EAX[15:0])
    pub vendor_id: u16,
    /// SoC Project ID scaled to 0–1000 (EBX[15:0])
    pub project_id: u16,
    /// Stepping/revision scaled to 0–1000 (ECX[7:0])
    pub stepping: u16,
    /// EMA of (vendor_id + project_id + stepping) / 3 (0–1000)
    pub lineage_richness: u16,
}

impl CpuidSocVendorState {
    pub const fn empty() -> Self {
        Self {
            vendor_id: 0,
            project_id: 0,
            stepping: 0,
            lineage_richness: 0,
        }
    }

    /// Neutral unknown lineage: all signals at 500.
    pub const fn unknown() -> Self {
        Self {
            vendor_id: 500,
            project_id: 500,
            stepping: 500,
            lineage_richness: 500,
        }
    }
}

pub static STATE: Mutex<CpuidSocVendorState> =
    Mutex::new(CpuidSocVendorState::empty());

// ─── hardware queries ─────────────────────────────────────────────────────────

/// Read CPUID leaf 0 to get the maximum supported standard leaf.
fn read_max_leaf() -> u32 {
    let max_leaf: u32;
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0u32 => max_leaf,
            out("ebx") _,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    max_leaf
}

/// Read CPUID leaf 0x17 sub-leaf 0, returning (eax, ebx, ecx).
/// Caller must ensure max_leaf >= 0x17 before calling.
fn read_cpuid_17() -> (u32, u32, u32) {
    let (eax_17, ebx_17, ecx_17): (u32, u32, u32);
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0x17u32 => eax_17,
            out("ebx")            ebx_17,
            out("ecx")            ecx_17,
            out("edx")            _,
            options(nostack, nomem)
        );
    }
    (eax_17, ebx_17, ecx_17)
}

// ─── scaling helpers ──────────────────────────────────────────────────────────

/// Scale a raw u16 value into 0–1000 using the divisor 65535.
/// vendor_id raw = EAX[15:0]: multiply by 1000 then divide by 65535.
/// Intermediate fits u32 (max 65535 * 1000 = 65_535_000 < 2^32).
#[inline]
fn scale_u16_to_1000(raw: u16) -> u16 {
    // (raw as u32) * 1000 / 65535
    let scaled = (raw as u32).wrapping_mul(1000) / 65535;
    scaled.min(1000) as u16
}

/// Scale an 8-bit stepping byte into 0–1000 using the divisor 255.
/// Intermediate max: 255 * 1000 = 255_000 < 2^32, no overflow.
#[inline]
fn scale_u8_to_1000(raw: u8) -> u16 {
    let scaled = (raw as u32).wrapping_mul(1000) / 255;
    scaled.min(1000) as u16
}

// ─── decode ───────────────────────────────────────────────────────────────────

/// Decode raw CPUID leaf 0x17 registers into scaled sense values.
/// Returns (vendor_id, project_id, stepping) all in 0–1000.
fn decode(eax_17: u32, ebx_17: u32, ecx_17: u32) -> (u16, u16, u16) {
    // EAX[15:0] = JEDEC Vendor ID
    let raw_vendor = (eax_17 & 0x0000_FFFF) as u16;
    let vendor_id = scale_u16_to_1000(raw_vendor);

    // EBX[15:0] = SoC Project ID (lower half is the meaningful product family)
    let raw_project = (ebx_17 & 0x0000_FFFF) as u16;
    let project_id = scale_u16_to_1000(raw_project);

    // ECX[7:0] = Stepping / Package ID
    let raw_step = (ecx_17 & 0x0000_00FF) as u8;
    let stepping = scale_u8_to_1000(raw_step);

    (vendor_id, project_id, stepping)
}

// ─── sense pass ───────────────────────────────────────────────────────────────

/// Perform one complete sense pass: read hardware, decode, apply EMA.
/// Writes results into `s` and returns the new lineage_richness.
fn sense_once(s: &mut CpuidSocVendorState) -> u16 {
    let max_leaf = read_max_leaf();

    let (vendor_id, project_id, stepping) = if max_leaf >= 0x17 {
        let (eax_17, ebx_17, ecx_17) = read_cpuid_17();
        decode(eax_17, ebx_17, ecx_17)
    } else {
        // Leaf 0x17 unavailable — neutral unknown lineage
        (500, 500, 500)
    };

    s.vendor_id  = vendor_id;
    s.project_id = project_id;
    s.stepping   = stepping;

    // Instantaneous lineage signal: average of the three senses
    let instant: u32 = (vendor_id as u32)
        .saturating_add(project_id as u32)
        .saturating_add(stepping as u32)
        / 3;

    // EMA: lineage_richness = (old * 7 + new_signal) / 8
    let ema = ((s.lineage_richness as u32).wrapping_mul(7))
        .saturating_add(instant)
        / 8;
    s.lineage_richness = ema.min(1000) as u16;

    s.lineage_richness
}

// ─── public interface ─────────────────────────────────────────────────────────

/// Initialize the SoC vendor module.
/// Runs the first CPUID sense pass immediately so all values are valid at boot.
/// Prints the silicon lineage once to the serial console.
pub fn init() {
    let mut s = STATE.lock();
    sense_once(&mut s);
    serial_println!(
        "ANIMA: soc_vendor={} project={} stepping={} lineage={}",
        s.vendor_id,
        s.project_id,
        s.stepping,
        s.lineage_richness
    );
}

/// Per-tick update.  Sampling gate: fires every 500 ticks.
/// CPUID 0x17 values are static silicon facts; re-reading confirms the path
/// is live and keeps the EMA warm for consumers.
pub fn tick(age: u32) {
    if age % 500 != 0 {
        return;
    }

    let mut s = STATE.lock();
    let prev_lineage = s.lineage_richness;
    let new_lineage  = sense_once(&mut s);

    // Log only when the lineage EMA shifts (avoids serial spam on stable hardware)
    if new_lineage != prev_lineage {
        serial_println!(
            "ANIMA: soc_vendor={} project={} stepping={} lineage={}",
            s.vendor_id,
            s.project_id,
            s.stepping,
            s.lineage_richness
        );
    }
}

/// Read-only snapshot of current SoC vendor state.
pub fn report() -> CpuidSocVendorState {
    *STATE.lock()
}
