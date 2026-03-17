use crate::serial_println;
/// paravirt — KVM / VMware / Hyper-V / Xen hypervisor detection
///
/// Detects the hypervisor via CPUID leaf 0x40000000 and caches the result in
/// a static `ParavirtInfo` protected by a spinlock Mutex.
///
/// Rules: no_std, no heap, no float casts, no panic, saturating arithmetic,
///        MMIO via read_volatile/write_volatile (not used here but kept in mind).
///
/// CPUID convention:
///   - Leaf 0x1 ECX bit 31 — hypervisor present flag
///   - Leaf 0x40000000 EBX/ECX/EDX — 12-byte ASCII signature
///   - Leaf 0x40000001 EAX — KVM feature bits
///
/// KVM feature bits (leaf 0x40000001 EAX):
///   bit  3 — KVM_FEATURE_CLOCKSOURCE2 (tsc_stable)
///   bit  5 — KVM_FEATURE_STEAL_TIME
///   bit 11 — KVM_FEATURE_PV_SEND_IPI
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Hypervisor CPUID leaf
// ---------------------------------------------------------------------------

pub const HYPERVISOR_CPUID_LEAF: u32 = 0x4000_0000;

// ---------------------------------------------------------------------------
// Known 12-byte signatures (EBX || ECX || EDX, little-endian byte order)
// ---------------------------------------------------------------------------

pub const KVM_SIGNATURE: [u8; 12] = *b"KVMKVMKVM\0\0\0";
pub const VMWARE_SIGNATURE: [u8; 12] = *b"VMwareVMware";
pub const HYPERV_SIGNATURE: [u8; 12] = *b"Microsoft Hv";
pub const XEN_SIGNATURE: [u8; 12] = *b"XenVMMXenVMM";

// ---------------------------------------------------------------------------
// KVM feature bit positions (CPUID leaf 0x40000001, EAX)
// ---------------------------------------------------------------------------

const KVM_FEATURE_CLOCKSOURCE2: u32 = 1 << 3;
const KVM_FEATURE_STEAL_TIME: u32 = 1 << 5;
const KVM_FEATURE_PV_SEND_IPI: u32 = 1 << 11;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Which hypervisor (if any) is hosting this kernel.
#[derive(Copy, Clone, PartialEq)]
pub enum HypervisorType {
    None,
    Kvm,
    VMware,
    HyperV,
    Xen,
    Unknown,
}

/// Paravirt capabilities gathered at boot.
#[derive(Copy, Clone)]
pub struct ParavirtInfo {
    /// Detected hypervisor type.
    pub hypervisor: HypervisorType,
    /// Raw KVM feature-bits from CPUID leaf 0x40000001 EAX.
    /// Zero for non-KVM hypervisors.
    pub features: u32,
    /// KVM_FEATURE_CLOCKSOURCE2 (bit 3) — TSC is stable across vCPU migrations.
    pub tsc_stable: bool,
    /// KVM_FEATURE_PV_SEND_IPI (bit 11) — para-virtual IPI available.
    pub pv_ipi: bool,
    /// KVM_FEATURE_STEAL_TIME (bit 5) — steal-time accounting available.
    pub steal_time: bool,
}

impl ParavirtInfo {
    const fn empty() -> Self {
        ParavirtInfo {
            hypervisor: HypervisorType::None,
            features: 0,
            tsc_stable: false,
            pv_ipi: false,
            steal_time: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Static state
// ---------------------------------------------------------------------------

static PARAVIRT_INFO: Mutex<ParavirtInfo> = Mutex::new(ParavirtInfo::empty());

// ---------------------------------------------------------------------------
// CPUID helper
// ---------------------------------------------------------------------------

/// Execute `CPUID` with the given leaf and return `(eax, ebx, ecx, edx)`.
///
/// `leaf` is passed in EAX; ECX (sub-leaf) is set to 0.
#[inline]
fn cpuid(leaf: u32) -> (u32, u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov {ebx_out:e}, ebx",
            "pop rbx",
            inout("eax") leaf => eax,
            ebx_out = out(reg) ebx,
            inout("ecx") 0u32 => ecx,
            out("edx") edx,
            options(nostack, nomem),
        );
    }
    (eax, ebx, ecx, edx)
}

// ---------------------------------------------------------------------------
// Signature helpers
// ---------------------------------------------------------------------------

/// Unpack three `u32` register values (EBX, ECX, EDX) into a 12-byte
/// signature array.  Each register contributes 4 bytes in little-endian order.
#[inline]
fn regs_to_sig(ebx: u32, ecx: u32, edx: u32) -> [u8; 12] {
    let b = ebx.to_le_bytes();
    let c = ecx.to_le_bytes();
    let d = edx.to_le_bytes();
    [
        b[0], b[1], b[2], b[3], c[0], c[1], c[2], c[3], d[0], d[1], d[2], d[3],
    ]
}

/// Compare two 12-byte arrays for equality without using `PartialEq` on slices
/// (keeps things `#![no_std]`-friendly and makes the comparison explicit).
#[inline]
fn sig_eq(a: &[u8; 12], b: &[u8; 12]) -> bool {
    let mut i = 0usize;
    while i < 12 {
        if a[i] != b[i] {
            return false;
        }
        i = i.saturating_add(1);
    }
    true
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Detect the hypervisor type and populate `PARAVIRT_INFO`.
///
/// Steps:
/// 1. CPUID(1) — check bit 31 of ECX (hypervisor-present flag).
/// 2. CPUID(0x40000000) — read the 12-byte ASCII signature from EBX/ECX/EDX.
/// 3. Match the signature against known hypervisors.
/// 4. If KVM: CPUID(0x40000001) — read feature bits from EAX.
/// 5. Store result in `PARAVIRT_INFO`.
///
/// Returns the detected `HypervisorType`.
pub fn paravirt_detect() -> HypervisorType {
    // Step 1: check bit 31 of CPUID(1) ECX — hypervisor-present flag.
    let (_eax1, _ebx1, ecx1, _edx1) = cpuid(1);
    let hypervisor_present = (ecx1 >> 31) & 1 == 1;

    if !hypervisor_present {
        // No hypervisor; write None and return.
        let mut info = PARAVIRT_INFO.lock();
        *info = ParavirtInfo::empty();
        return HypervisorType::None;
    }

    // Step 2: read the 12-byte signature from CPUID(0x40000000).
    let (_max_leaf, ebx, ecx, edx) = cpuid(HYPERVISOR_CPUID_LEAF);
    let sig = regs_to_sig(ebx, ecx, edx);

    // Step 3: match against known signatures.
    let hv_type = if sig_eq(&sig, &KVM_SIGNATURE) {
        HypervisorType::Kvm
    } else if sig_eq(&sig, &VMWARE_SIGNATURE) {
        HypervisorType::VMware
    } else if sig_eq(&sig, &HYPERV_SIGNATURE) {
        HypervisorType::HyperV
    } else if sig_eq(&sig, &XEN_SIGNATURE) {
        HypervisorType::Xen
    } else {
        HypervisorType::Unknown
    };

    // Step 4: if KVM, read feature bits from leaf 0x40000001.
    let (kvm_features, tsc_stable, pv_ipi, steal_time) = if hv_type == HypervisorType::Kvm {
        let (feat_eax, _feat_ebx, _feat_ecx, _feat_edx) =
            cpuid(HYPERVISOR_CPUID_LEAF.saturating_add(1));
        let ts = (feat_eax & KVM_FEATURE_CLOCKSOURCE2) != 0;
        let pi = (feat_eax & KVM_FEATURE_PV_SEND_IPI) != 0;
        let st = (feat_eax & KVM_FEATURE_STEAL_TIME) != 0;
        (feat_eax, ts, pi, st)
    } else {
        (0u32, false, false, false)
    };

    // Step 5: store in static.
    let result = ParavirtInfo {
        hypervisor: hv_type,
        features: kvm_features,
        tsc_stable,
        pv_ipi,
        steal_time,
    };

    {
        let mut info = PARAVIRT_INFO.lock();
        *info = result;
    }

    hv_type
}

/// Returns `true` if the kernel is running under KVM.
pub fn paravirt_is_kvm() -> bool {
    PARAVIRT_INFO.lock().hypervisor == HypervisorType::Kvm
}

/// Returns a copy of the cached `ParavirtInfo`.
pub fn paravirt_get_info() -> ParavirtInfo {
    *PARAVIRT_INFO.lock()
}

/// Returns `true` if the given KVM feature bit is set.
///
/// `bit` is the bit position (0-31) within the KVM feature word.
/// Always returns `false` for non-KVM hypervisors.
pub fn paravirt_has_feature(bit: u32) -> bool {
    if bit >= 32 {
        return false;
    }
    let info = PARAVIRT_INFO.lock();
    if info.hypervisor != HypervisorType::Kvm {
        return false;
    }
    (info.features >> bit) & 1 == 1
}

/// Initialize paravirtualization detection.
///
/// Calls `paravirt_detect()` and emits a serial log message indicating which
/// hypervisor (if any) was detected.
pub fn init() {
    let hv = paravirt_detect();

    match hv {
        HypervisorType::None => {
            serial_println!("[paravirt] running on bare metal (no hypervisor)");
        }
        HypervisorType::Kvm => {
            let info = paravirt_get_info();
            serial_println!(
                "[paravirt] KVM detected — features={:#010x} tsc_stable={} pv_ipi={} steal_time={}",
                info.features,
                info.tsc_stable,
                info.pv_ipi,
                info.steal_time,
            );
        }
        HypervisorType::VMware => {
            serial_println!("[paravirt] VMware detected");
        }
        HypervisorType::HyperV => {
            serial_println!("[paravirt] Microsoft Hyper-V detected");
        }
        HypervisorType::Xen => {
            serial_println!("[paravirt] Xen hypervisor detected");
        }
        HypervisorType::Unknown => {
            serial_println!("[paravirt] unknown hypervisor detected (hypervisor bit set but signature unrecognized)");
        }
    }
}
