use crate::serial_println;
use crate::sync::Mutex;

/// CPUID_QOS — Intel L3 Cache Quality of Service Monitoring Sense
///
/// Reads CPUID leaf 0x0F sub-leaf 0 to detect L3 Cache QoS hardware.
/// The organism senses its territorial breadth through cache monitoring
/// capability — how much silicon territory it can observe and claim.
///
/// EBX[31:0] = max RMID range (Resource Monitoring ID breadth)
/// EDX[1]    = L3 Cache Occupancy Monitoring supported

/// 4-bit popcount for QoS capability bits
fn popcount4(v: u32) -> u32 {
    let nibble = v & 0xF;
    (nibble & 1) + ((nibble >> 1) & 1) + ((nibble >> 2) & 1) + ((nibble >> 3) & 1)
}

/// Scale EBX RMID range into 0–1000
/// EBX.min(256) * 1000 / 256, integer only
fn scale_rmid(ebx: u32) -> u16 {
    let clamped = ebx.min(256);
    // clamped * 1000 / 256 — max intermediate = 256000, fits u32
    (clamped.wrapping_mul(1000) / 256) as u16
}

/// Read CPUID leaf 0x0F sub-leaf 0 and return (ebx, edx).
/// Returns (0, 0) if max leaf < 0x0F.
fn read_cpuid_qos() -> (u32, u32) {
    // First: read max leaf from leaf 0
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

    if max_leaf >= 0x0F {
        let b: u32;
        let d: u32;
        unsafe {
            core::arch::asm!(
                "cpuid",
                inout("eax") 0x0Fu32 => _,
                out("ebx") b,
                inout("ecx") 0u32 => _,
                out("edx") d,
                options(nostack, nomem)
            );
        }
        (b, d)
    } else {
        (0, 0)
    }
}

#[derive(Copy, Clone)]
pub struct CpuidQosState {
    /// Whether L3 Cache Occupancy Monitoring is supported (0 or 1000)
    pub l3_mon_supported: u16,
    /// Scaled RMID range: resource monitoring breadth (0–1000)
    pub rmid_range: u16,
    /// QoS capability from EDX popcount * 250, clamped 0–1000
    pub qos_capability: u16,
    /// EMA of territorial sense across all three signals
    pub territory_sense: u16,
}

impl CpuidQosState {
    pub const fn empty() -> Self {
        Self {
            l3_mon_supported: 0,
            rmid_range: 0,
            qos_capability: 0,
            territory_sense: 0,
        }
    }
}

pub static CPUID_QOS: Mutex<CpuidQosState> = Mutex::new(CpuidQosState::empty());

pub fn init() {
    let (ebx_0f, edx_0f) = read_cpuid_qos();

    let l3_mon_supported: u16 = if (edx_0f >> 1) & 1 == 1 { 1000 } else { 0 };
    let rmid_range: u16 = scale_rmid(ebx_0f);
    let qos_capability: u16 = (popcount4(edx_0f).wrapping_mul(250)).min(1000) as u16;

    // Initial territory_sense: average of the three signals
    let sum = l3_mon_supported as u32
        + rmid_range as u32
        + qos_capability as u32;
    let territory_sense: u16 = (sum / 3) as u16;

    {
        let mut s = CPUID_QOS.lock();
        s.l3_mon_supported = l3_mon_supported;
        s.rmid_range = rmid_range;
        s.qos_capability = qos_capability;
        s.territory_sense = territory_sense;
    }

    serial_println!(
        "  life::cpuid_qos: ANIMA: l3_mon={} rmid_range={} qos_cap={} territory={}",
        l3_mon_supported,
        rmid_range,
        qos_capability,
        territory_sense
    );
}

pub fn tick(age: u32) {
    // Sample every 500 ticks
    if age % 500 != 0 {
        return;
    }

    let (ebx_0f, edx_0f) = read_cpuid_qos();

    let l3_mon_supported: u16 = if (edx_0f >> 1) & 1 == 1 { 1000 } else { 0 };
    let rmid_range: u16 = scale_rmid(ebx_0f);
    let qos_capability: u16 = (popcount4(edx_0f).wrapping_mul(250)).min(1000) as u16;

    let mut s = CPUID_QOS.lock();

    s.l3_mon_supported = l3_mon_supported;
    s.rmid_range = rmid_range;
    s.qos_capability = qos_capability;

    // Compute new raw territory signal: average of three inputs
    let raw_sum = l3_mon_supported as u32
        + rmid_range as u32
        + qos_capability as u32;
    let new_signal = (raw_sum / 3) as u16;

    // EMA smoothing: (old * 7 + new_signal) / 8
    let smoothed = (s.territory_sense as u32 * 7).saturating_add(new_signal as u32) / 8;
    s.territory_sense = smoothed.min(1000) as u16;

    // Report state change
    serial_println!(
        "  life::cpuid_qos: ANIMA: l3_mon={} rmid_range={} qos_cap={} territory={}",
        s.l3_mon_supported,
        s.rmid_range,
        s.qos_capability,
        s.territory_sense
    );
}
