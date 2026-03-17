/// AMD-V (SVM) virtualization extensions.
///
/// Part of the AIOS hypervisor subsystem.

use crate::{serial_print, serial_println};
use crate::sync::Mutex;

/// MSR address for AMD EFER (Extended Feature Enable Register).
const MSR_EFER: u32 = 0xC000_0080;
/// EFER bit to enable SVM.
const EFER_SVME: u64 = 1 << 12;

/// MSR address for VM_CR (controls SVM lock).
const MSR_VM_CR: u32 = 0xC001_0114;
/// Bit in VM_CR that disables SVM when set.
const VM_CR_SVMDIS: u64 = 1 << 4;

/// MSR for SVM revision and VMCB physical address area.
const MSR_VM_HSAVE_PA: u32 = 0xC001_0117;

/// Size of a VMCB (Virtual Machine Control Block) — 4 KiB.
const VMCB_SIZE: usize = 4096;

/// Size of the host save area — 4 KiB.
const HOST_SAVE_SIZE: usize = 4096;

/// Global SVM state singleton.
static SVM_STATE: Mutex<Option<SvmState>> = Mutex::new(None);

/// State for AMD Secure Virtual Machine hardware.
pub struct SvmState {
    /// VMCB (Virtual Machine Control Block) — 4 KiB aligned.
    vmcb: [u8; VMCB_SIZE],
    /// Host save area for host state during guest execution.
    host_save_area: [u8; HOST_SAVE_SIZE],
    /// Current ASID (Address Space Identifier) for TLB tagging.
    current_asid: u32,
    /// Maximum ASID supported by the hardware.
    max_asid: u32,
    /// Whether SVM is currently enabled.
    svm_enabled: bool,
    /// SVM revision ID from CPUID.
    revision_id: u32,
}

impl SvmState {
    pub fn new() -> Self {
        let (revision_id, max_asid) = Self::query_svm_features();

        SvmState {
            vmcb: [0u8; VMCB_SIZE],
            host_save_area: [0u8; HOST_SAVE_SIZE],
            current_asid: 1,
            max_asid,
            svm_enabled: false,
            revision_id,
        }
    }

    /// Enable SVM operation (set EFER.SVME).
    pub fn enable(&mut self) {
        if self.svm_enabled {
            serial_println!("    [svm] SVM already enabled");
            return;
        }

        // Check if SVM is locked out in VM_CR.
        let vm_cr = unsafe { Self::rdmsr(MSR_VM_CR) };
        if vm_cr & VM_CR_SVMDIS != 0 {
            serial_println!("    [svm] SVM disabled by BIOS (VM_CR.SVMDIS set)");
            return;
        }

        // Set EFER.SVME to enable SVM instructions.
        let efer = unsafe { Self::rdmsr(MSR_EFER) };
        unsafe { Self::wrmsr(MSR_EFER, efer | EFER_SVME); }

        // Set the host save area physical address.
        let host_save_phys = self.host_save_area.as_ptr() as u64;
        unsafe { Self::wrmsr(MSR_VM_HSAVE_PA, host_save_phys); }

        self.svm_enabled = true;
        serial_println!(
            "    [svm] SVM enabled (revision={}, max_asid={})",
            self.revision_id,
            self.max_asid
        );
    }

    /// Disable SVM operation (clear EFER.SVME).
    pub fn disable(&mut self) {
        if !self.svm_enabled {
            return;
        }
        let efer = unsafe { Self::rdmsr(MSR_EFER) };
        unsafe { Self::wrmsr(MSR_EFER, efer & !EFER_SVME); }
        self.svm_enabled = false;
        serial_println!("    [svm] SVM disabled");
    }

    /// Check if AMD-V is supported on this CPU.
    pub fn is_supported() -> bool {
        // CPUID Fn8000_0001, ECX bit 2 = SVM support.
        let ecx: u32;
        unsafe {
            core::arch::asm!(
                "mov eax, 0x80000001",
                "cpuid",
                out("ecx") ecx,
                out("eax") _,
                out("ebx") _,
                out("edx") _,
                options(nomem, nostack),
            );
        }
        (ecx >> 2) & 1 == 1
    }

    /// Allocate an ASID for a new guest.
    pub fn allocate_asid(&mut self) -> Option<u32> {
        if self.current_asid >= self.max_asid {
            // ASID space exhausted — caller must flush TLBs and reset.
            serial_println!("    [svm] ASID space exhausted (max={})", self.max_asid);
            return None;
        }
        let asid = self.current_asid;
        self.current_asid = self.current_asid.saturating_add(1);
        Some(asid)
    }

    /// Reset ASID allocator (typically after a TLB flush of all ASIDs).
    pub fn reset_asids(&mut self) {
        self.current_asid = 1; // ASID 0 is reserved for the host.
    }

    /// Get a mutable pointer to the VMCB for guest configuration.
    pub fn vmcb_ptr(&mut self) -> *mut u8 {
        self.vmcb.as_mut_ptr()
    }

    /// Query SVM features from CPUID leaf 0x8000_000A.
    fn query_svm_features() -> (u32, u32) {
        let eax: u32;
        let ebx: u32;
        unsafe {
            core::arch::asm!(
                "mov eax, 0x8000000A",
                "cpuid",
                out("eax") eax,
                out("ebx") ebx,
                out("ecx") _,
                out("edx") _,
                options(nomem, nostack),
            );
        }
        let revision = eax & 0xFF;
        let max_asid = ebx;
        (revision, max_asid)
    }

    /// Read a model-specific register.
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

    /// Write a model-specific register.
    unsafe fn wrmsr(msr: u32, value: u64) {
        let lo = value as u32;
        let hi = (value >> 32) as u32;
        core::arch::asm!(
            "wrmsr",
            in("ecx") msr,
            in("eax") lo,
            in("edx") hi,
            options(nomem, nostack),
        );
    }
}

pub fn init() {
    if !SvmState::is_supported() {
        serial_println!("    [svm] AMD-V not supported on this CPU");
        return;
    }

    let mut state = SvmState::new();
    state.enable();
    *SVM_STATE.lock() = Some(state);
    serial_println!("    [svm] AMD-V subsystem initialized");
}
