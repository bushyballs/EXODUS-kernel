/// Intel VT-x virtualization extensions.
///
/// Part of the AIOS hypervisor subsystem.

use crate::{serial_print, serial_println};
use crate::sync::Mutex;

/// IA32_FEATURE_CONTROL MSR address.
const IA32_FEATURE_CONTROL: u32 = 0x3A;
/// IA32_VMX_BASIC MSR — contains VMCS revision ID and region size.
const IA32_VMX_BASIC: u32 = 0x480;

/// CR4 bit for VMX enable.
const CR4_VMXE: u64 = 1 << 13;

/// Size of the VMXON region in bytes (4 KiB page-aligned).
const VMXON_REGION_SIZE: usize = 4096;

/// Global VMX state singleton.
static VMX_STATE: Mutex<Option<VmxState>> = Mutex::new(None);

/// State for Intel VT-x hardware virtualization.
pub struct VmxState {
    /// VMXON region — 4 KiB aligned memory that holds the revision ID.
    vmxon_region: [u8; VMXON_REGION_SIZE],
    /// VMCS revision identifier read from IA32_VMX_BASIC MSR.
    revision_id: u32,
    /// Whether VMX root operation is currently active.
    vmx_enabled: bool,
    /// Feature flags from IA32_FEATURE_CONTROL.
    feature_control: u64,
}

impl VmxState {
    pub fn new() -> Self {
        let revision_id = Self::read_vmx_revision();
        let feature_control = Self::read_feature_control();

        let mut state = VmxState {
            vmxon_region: [0u8; VMXON_REGION_SIZE],
            revision_id,
            vmx_enabled: false,
            feature_control,
        };

        // Write the revision ID into the first 4 bytes of the VMXON region.
        // Bit 31 must be clear per Intel SDM.
        let rev_bytes = (revision_id & 0x7FFF_FFFF).to_le_bytes();
        state.vmxon_region[0] = rev_bytes[0];
        state.vmxon_region[1] = rev_bytes[1];
        state.vmxon_region[2] = rev_bytes[2];
        state.vmxon_region[3] = rev_bytes[3];

        state
    }

    /// Enable VMX operation (execute VMXON).
    pub fn enable(&mut self) {
        if self.vmx_enabled {
            serial_println!("    [vmx] VMX already enabled");
            return;
        }

        // Check that BIOS has not locked out VMX.
        let lock_bit = self.feature_control & 0x1;
        let vmx_outside_smx = (self.feature_control >> 2) & 0x1;
        if lock_bit == 1 && vmx_outside_smx == 0 {
            serial_println!("    [vmx] VMX disabled by BIOS (IA32_FEATURE_CONTROL locked without VMXON-outside-SMX)");
            return;
        }

        // If the lock bit is not set, we need to configure IA32_FEATURE_CONTROL.
        if lock_bit == 0 {
            let new_val = self.feature_control | 0x5; // Set lock bit + VMXON-outside-SMX
            unsafe { Self::wrmsr(IA32_FEATURE_CONTROL, new_val); }
            self.feature_control = new_val;
        }

        // Set CR4.VMXE to enable VMX instructions.
        unsafe {
            let cr4: u64;
            core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack));
            core::arch::asm!("mov cr4, {}", in(reg) cr4 | CR4_VMXE, options(nomem, nostack));
        }

        // Execute VMXON with the physical address of our VMXON region.
        let vmxon_phys = self.vmxon_region.as_ptr() as u64;
        let result: u8;
        unsafe {
            core::arch::asm!(
                "vmxon [{}]",
                in(reg) &vmxon_phys,
                options(nostack),
            );
            // Check CF (failure) via LAHF or conditional set — use setc.
            core::arch::asm!(
                "setc {}",
                out(reg_byte) result,
                options(nomem, nostack),
            );
        }

        if result != 0 {
            serial_println!("    [vmx] VMXON failed (CF set)");
            return;
        }

        self.vmx_enabled = true;
        serial_println!("    [vmx] VMX root operation enabled (revision_id=0x{:08x})", self.revision_id);
    }

    /// Check if VT-x is supported on this CPU.
    pub fn is_supported() -> bool {
        // CPUID leaf 1, ECX bit 5 = VMX support.
        let ecx: u32;
        unsafe {
            core::arch::asm!(
                "mov eax, 1",
                "cpuid",
                out("ecx") ecx,
                out("eax") _,
                out("ebx") _,
                out("edx") _,
                options(nomem, nostack),
            );
        }
        (ecx >> 5) & 1 == 1
    }

    /// Disable VMX operation (execute VMXOFF).
    pub fn disable(&mut self) {
        if !self.vmx_enabled {
            return;
        }
        unsafe {
            core::arch::asm!("vmxoff", options(nomem, nostack));
        }
        self.vmx_enabled = false;
        serial_println!("    [vmx] VMX root operation disabled");
    }

    /// Read the VMCS revision ID from IA32_VMX_BASIC MSR.
    fn read_vmx_revision() -> u32 {
        let val = unsafe { Self::rdmsr(IA32_VMX_BASIC) };
        // Bits [30:0] contain the revision identifier.
        (val & 0x7FFF_FFFF) as u32
    }

    /// Read IA32_FEATURE_CONTROL MSR.
    fn read_feature_control() -> u64 {
        unsafe { Self::rdmsr(IA32_FEATURE_CONTROL) }
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
    if !VmxState::is_supported() {
        serial_println!("    [vmx] Intel VT-x not supported on this CPU");
        return;
    }

    let mut state = VmxState::new();
    state.enable();
    *VMX_STATE.lock() = Some(state);
    serial_println!("    [vmx] Intel VT-x subsystem initialized");
}
