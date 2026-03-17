/// Intel VMX (VT-x) hypervisor support for Genesis
///
/// Enables running guest VMs with hardware virtualization.
/// Provides the low-level VMXON/VMXOFF/VMREAD/VMWRITE/VMLAUNCH/VMRESUME
/// primitives used by the virt::vcpu and virt::ept layers.
///
/// All code is #![no_std] and uses only core::arch inline assembly.
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// VMX capability MSRs
// ---------------------------------------------------------------------------

/// MSR: IA32_VMX_BASIC — VMCS revision ID + region size.
pub const MSR_IA32_VMX_BASIC: u32 = 0x480;
/// MSR: IA32_VMX_CR0_FIXED0 — bits that must be 1 in CR0 when in VMX operation.
pub const MSR_IA32_VMX_CR0_FIXED0: u32 = 0x486;
/// MSR: IA32_VMX_CR0_FIXED1 — bits that may be 1 in CR0 when in VMX operation.
pub const MSR_IA32_VMX_CR0_FIXED1: u32 = 0x487;
/// MSR: IA32_VMX_CR4_FIXED0 — bits that must be 1 in CR4 when in VMX operation.
pub const MSR_IA32_VMX_CR4_FIXED0: u32 = 0x488;
/// MSR: IA32_VMX_CR4_FIXED1 — bits that may be 1 in CR4 when in VMX operation.
pub const MSR_IA32_VMX_CR4_FIXED1: u32 = 0x489;
/// MSR: IA32_FEATURE_CONTROL — BIOS lock/enable for VMX.
pub const MSR_IA32_FEATURE_CONTROL: u32 = 0x3A;

/// CR4 bit 13: VMX Enable.
const CR4_VMXE: u64 = 1 << 13;

/// Size of the VMXON region — must be 4 KiB and 4 KiB-aligned.
const VMXON_REGION_SIZE: usize = 4096;

// ---------------------------------------------------------------------------
// VMXON region (statically allocated, 4 KiB-aligned via repr(align))
// ---------------------------------------------------------------------------

/// 4 KiB-aligned VMXON region.
#[repr(C, align(4096))]
struct VmxonRegion {
    data: [u8; VMXON_REGION_SIZE],
}

/// Global VMXON region.  Placed in BSS; zero-initialized.
static mut VMXON_REGION: VmxonRegion = VmxonRegion {
    data: [0u8; VMXON_REGION_SIZE],
};

// ---------------------------------------------------------------------------
// Low-level MSR helpers
// ---------------------------------------------------------------------------

/// Read a 64-bit model-specific register.
///
/// # Safety
/// Caller must ensure the MSR address is valid on this CPU.
#[inline]
pub unsafe fn rdmsr(msr: u32) -> u64 {
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

/// Write a 64-bit model-specific register.
///
/// # Safety
/// Caller must ensure the MSR address is valid and the value is legal.
#[inline]
pub unsafe fn wrmsr(msr: u32, value: u64) {
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

// ---------------------------------------------------------------------------
// VMX capability detection
// ---------------------------------------------------------------------------

/// Returns `true` if this CPU supports Intel VMX (VT-x).
///
/// Checks CPUID leaf 1, ECX bit 5 (the VMX flag).
pub fn vmx_supported() -> bool {
    let ecx: u32;
    unsafe {
        let ebx_tmp: u32;
        core::arch::asm!(
            "push rbx",
            "mov eax, 1",
            "cpuid",
            "mov {ebx_tmp:e}, ebx",
            "pop rbx",
            out("ecx") ecx,
            ebx_tmp = out(reg) ebx_tmp,
            out("eax") _,
            out("edx") _,
            options(nomem),
        );
    }
    (ecx >> 5) & 1 == 1
}

// ---------------------------------------------------------------------------
// VMXON / VMXOFF
// ---------------------------------------------------------------------------

/// Enable VMX root operation.
///
/// Steps performed:
/// 1. Verify BIOS has not locked out VMX via IA32_FEATURE_CONTROL.
/// 2. Set CR4.VMXE (bit 13).
/// 3. Write the VMCS revision ID into the VMXON region.
/// 4. Execute VMXON with the physical address of the VMXON region.
///
/// Returns `Ok(())` on success or an error string on failure.
///
/// # Safety
/// Must be called once per BSP before using any other VMX primitives.
pub unsafe fn vmx_enable() -> Result<(), &'static str> {
    // --- Step 1: IA32_FEATURE_CONTROL ---
    let fc = rdmsr(MSR_IA32_FEATURE_CONTROL);
    let lock_bit = fc & 0x1;
    let vmxon_outside_smx = (fc >> 2) & 0x1;

    if lock_bit == 1 && vmxon_outside_smx == 0 {
        return Err(
            "[vmx] BIOS has locked out VMX (IA32_FEATURE_CONTROL locked, VMXON-outside-SMX=0)",
        );
    }
    if lock_bit == 0 {
        // Set lock bit + VMXON-outside-SMX bit.
        wrmsr(MSR_IA32_FEATURE_CONTROL, fc | 0x5);
    }

    // --- Step 2: Set CR4.VMXE ---
    let cr4: u64;
    core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack));
    core::arch::asm!("mov cr4, {}", in(reg) cr4 | CR4_VMXE, options(nomem, nostack));

    // --- Step 3: Write VMCS revision ID ---
    let revision_id = (rdmsr(MSR_IA32_VMX_BASIC) & 0x7FFF_FFFF) as u32;
    let rev_bytes = revision_id.to_le_bytes();
    VMXON_REGION.data[0] = rev_bytes[0];
    VMXON_REGION.data[1] = rev_bytes[1];
    VMXON_REGION.data[2] = rev_bytes[2];
    VMXON_REGION.data[3] = rev_bytes[3];

    // --- Step 4: VMXON ---
    let vmxon_phys = VMXON_REGION.data.as_ptr() as u64;
    let cf: u8; // Carry flag — set on VMXON failure.
    core::arch::asm!(
        "vmxon [{}]",
        "setc {}",
        in(reg) &vmxon_phys,
        out(reg_byte) cf,
        options(nostack),
    );

    if cf != 0 {
        return Err("[vmx] VMXON failed (CF set — hardware refused VMX root operation)");
    }

    serial_println!("[VMX] VMXON succeeded (revision_id=0x{:08x})", revision_id);
    Ok(())
}

/// Disable VMX root operation.
///
/// Executes VMXOFF and clears CR4.VMXE.
///
/// # Safety
/// Must be called while in VMX root operation.  All VMCS regions must
/// already have been VMCLEAR'd before calling this function.
pub unsafe fn vmx_disable() {
    core::arch::asm!("vmxoff", options(nomem, nostack));

    let cr4: u64;
    core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack));
    core::arch::asm!("mov cr4, {}", in(reg) cr4 & !CR4_VMXE, options(nomem, nostack));

    serial_println!("[VMX] VMXOFF — VMX root operation disabled");
}

// ---------------------------------------------------------------------------
// VMCS allocation and lifecycle
// ---------------------------------------------------------------------------

/// 4 KiB-aligned VMCS region.
#[repr(C, align(4096))]
pub struct VmcsRegion {
    pub data: [u8; 4096],
}

impl VmcsRegion {
    pub const fn new() -> Self {
        VmcsRegion { data: [0u8; 4096] }
    }
}

/// Allocate a new VMCS region and write the VMCS revision ID into its first
/// four bytes, as required by Intel SDM 24.2.
///
/// In a production kernel this would call the physical-memory allocator.
/// Here we return a mutable reference to a static pool entry; callers that
/// need multiple VCPUs must manage separate VmcsRegion instances.
///
/// # Safety
/// The returned pointer remains valid for the lifetime of `region`.
pub unsafe fn alloc_vmcs(region: &mut VmcsRegion) -> *mut u8 {
    let revision_id = (rdmsr(MSR_IA32_VMX_BASIC) & 0x7FFF_FFFF) as u32;
    let rev_bytes = revision_id.to_le_bytes();
    region.data[0] = rev_bytes[0];
    region.data[1] = rev_bytes[1];
    region.data[2] = rev_bytes[2];
    region.data[3] = rev_bytes[3];
    region.data.as_mut_ptr()
}

/// Load (make current) a VMCS via the VMPTRLD instruction.
///
/// `vmcs_phys` must be the 4 KiB-aligned physical address of a previously
/// initialised VMCS region.
///
/// # Safety
/// VMX root operation must be active.  `vmcs_phys` must point to a valid,
/// non-active VMCS region.
pub unsafe fn vmcs_load(vmcs_phys: u64) {
    core::arch::asm!(
        "vmptrld [{}]",
        in(reg) &vmcs_phys,
        options(nostack),
    );
}

/// Clear a VMCS via the VMCLEAR instruction.
///
/// Resets the VMCS to the clear state and saves any modified processor state
/// back to the region.  Must be called before VMPTRLD on the same region from
/// a different logical processor.
///
/// # Safety
/// VMX root operation must be active.
pub unsafe fn vmcs_clear(vmcs_phys: u64) {
    core::arch::asm!(
        "vmclear [{}]",
        in(reg) &vmcs_phys,
        options(nostack),
    );
}

// ---------------------------------------------------------------------------
// VMCS field read / write
// ---------------------------------------------------------------------------

/// Write a VMCS field via the VMWRITE instruction.
///
/// `field` is a VMCS encoding constant (see Intel SDM Appendix B).
///
/// # Safety
/// The current VMCS must be active (VMPTRLD must have succeeded).
#[inline]
pub unsafe fn vmcs_write(field: u32, value: u64) {
    core::arch::asm!(
        "vmwrite {}, {}",
        in(reg) field as u64,
        in(reg) value,
        options(nostack),
    );
}

/// Read a VMCS field via the VMREAD instruction.
///
/// Returns the field value.  If VMREAD fails (ZF or CF set — invalid field or
/// no current VMCS) returns 0.
///
/// # Safety
/// The current VMCS must be active.
#[inline]
pub unsafe fn vmcs_read(field: u32) -> u64 {
    let value: u64;
    core::arch::asm!(
        "vmread {}, {}",
        out(reg) value,
        in(reg) field as u64,
        options(nostack),
    );
    value
}

// ---------------------------------------------------------------------------
// VM entry instructions
// ---------------------------------------------------------------------------

/// Execute VMLAUNCH to enter the guest for the first time.
///
/// Returns `Ok(())` if the VM exit was clean (CF=0, ZF=0).
/// Returns `Err(error_code)` where `error_code` is read from the
/// `VM_INSTRUCTION_ERROR` VMCS field (encoding 0x4400) on failure.
///
/// # Safety
/// The current VMCS must be fully initialised and in the launch state.
pub unsafe fn vmlaunch() -> Result<(), u64> {
    let cf: u8;
    let zf: u8;
    core::arch::asm!(
        "vmlaunch",
        "setc  {cf}",
        "setz  {zf}",
        cf = out(reg_byte) cf,
        zf = out(reg_byte) zf,
        options(nostack),
    );

    if cf != 0 || zf != 0 {
        // VM_INSTRUCTION_ERROR field encoding: 0x4400.
        let err = vmcs_read(0x4400);
        return Err(err);
    }
    Ok(())
}

/// Execute VMRESUME to re-enter a previously-launched guest after a VM exit.
///
/// Returns `Ok(())` on a clean re-entry, or `Err(error_code)` on failure.
///
/// # Safety
/// The current VMCS must be in the launched state.  The host register state
/// must match what was set in the VMCS host-state area.
pub unsafe fn vmresume() -> Result<(), u64> {
    let cf: u8;
    let zf: u8;
    core::arch::asm!(
        "vmresume",
        "setc  {cf}",
        "setz  {zf}",
        cf = out(reg_byte) cf,
        zf = out(reg_byte) zf,
        options(nostack),
    );

    if cf != 0 || zf != 0 {
        let err = vmcs_read(0x4400);
        return Err(err);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// VMCS field encoding constants
// ---------------------------------------------------------------------------
// A subset of the most commonly used encodings.  Full table: Intel SDM
// Appendix B.

/// 16-bit guest-state fields.
pub const GUEST_CS_SELECTOR: u32 = 0x0802;
pub const GUEST_DS_SELECTOR: u32 = 0x0806;
pub const GUEST_ES_SELECTOR: u32 = 0x0800;
pub const GUEST_SS_SELECTOR: u32 = 0x0804;
pub const GUEST_FS_SELECTOR: u32 = 0x0808;
pub const GUEST_GS_SELECTOR: u32 = 0x080A;
pub const GUEST_LDTR_SELECTOR: u32 = 0x080C;
pub const GUEST_TR_SELECTOR: u32 = 0x080E;
pub const GUEST_INTR_STATUS: u32 = 0x0810;

/// 16-bit host-state fields.
pub const HOST_CS_SELECTOR: u32 = 0x0C02;
pub const HOST_DS_SELECTOR: u32 = 0x0C06;
pub const HOST_ES_SELECTOR: u32 = 0x0C00;
pub const HOST_SS_SELECTOR: u32 = 0x0C04;

/// 64-bit control fields.
pub const IO_BITMAP_A_ADDR: u32 = 0x2000;
pub const IO_BITMAP_B_ADDR: u32 = 0x2002;
pub const MSR_BITMAP_ADDR: u32 = 0x2004;
pub const EPT_POINTER: u32 = 0x201A;
pub const VIRTUAL_APIC_PAGE: u32 = 0x2012;

/// 64-bit read-only fields.
pub const GUEST_PHYSICAL_ADDR: u32 = 0x2400;

/// 64-bit guest-state fields.
pub const VMCS_LINK_PTR: u32 = 0x2800;
pub const GUEST_IA32_EFER: u32 = 0x2806;

/// 64-bit host-state fields.
pub const HOST_IA32_EFER: u32 = 0x2C02;

/// 32-bit control fields.
pub const PIN_BASED_CONTROLS: u32 = 0x4000;
pub const PROC_BASED_CONTROLS: u32 = 0x4002;
pub const EXCEPTION_BITMAP: u32 = 0x4004;
pub const PROC_BASED_CONTROLS2: u32 = 0x401E;
pub const VM_EXIT_CONTROLS: u32 = 0x400C;
pub const VM_ENTRY_CONTROLS: u32 = 0x4012;
pub const VM_ENTRY_INTR_INFO: u32 = 0x4016;

/// 32-bit read-only fields.
pub const VM_EXIT_REASON: u32 = 0x4402;
pub const VM_EXIT_INSTR_LEN: u32 = 0x440C;
pub const VM_INSTRUCTION_ERROR: u32 = 0x4400;

/// Natural-width guest-state fields.
pub const GUEST_CR0: u32 = 0x6800;
pub const GUEST_CR3: u32 = 0x6802;
pub const GUEST_CR4: u32 = 0x6804;
pub const GUEST_ES_BASE: u32 = 0x6806;
pub const GUEST_CS_BASE: u32 = 0x6808;
pub const GUEST_SS_BASE: u32 = 0x680A;
pub const GUEST_DS_BASE: u32 = 0x680C;
pub const GUEST_FS_BASE: u32 = 0x680E;
pub const GUEST_GS_BASE: u32 = 0x6810;
pub const GUEST_LDTR_BASE: u32 = 0x6812;
pub const GUEST_TR_BASE: u32 = 0x6814;
pub const GUEST_GDTR_BASE: u32 = 0x6816;
pub const GUEST_IDTR_BASE: u32 = 0x6818;
pub const GUEST_RSP: u32 = 0x681C;
pub const GUEST_RIP: u32 = 0x681E;
pub const GUEST_RFLAGS: u32 = 0x6820;
pub const GUEST_ACTIVITY_STATE: u32 = 0x4826;

/// 32-bit guest-state fields.
pub const GUEST_ES_LIMIT: u32 = 0x4800;
pub const GUEST_CS_LIMIT: u32 = 0x4802;
pub const GUEST_SS_LIMIT: u32 = 0x4804;
pub const GUEST_DS_LIMIT: u32 = 0x4806;
pub const GUEST_FS_LIMIT: u32 = 0x4808;
pub const GUEST_GS_LIMIT: u32 = 0x480A;
pub const GUEST_LDTR_LIMIT: u32 = 0x480C;
pub const GUEST_TR_LIMIT: u32 = 0x480E;
pub const GUEST_GDTR_LIMIT: u32 = 0x4810;
pub const GUEST_IDTR_LIMIT: u32 = 0x4812;
pub const GUEST_ES_AR: u32 = 0x4814;
pub const GUEST_CS_AR: u32 = 0x4816;
pub const GUEST_SS_AR: u32 = 0x4818;
pub const GUEST_DS_AR: u32 = 0x481A;

/// Natural-width host-state fields.
pub const HOST_CR0: u32 = 0x6C00;
pub const HOST_CR3: u32 = 0x6C02;
pub const HOST_CR4: u32 = 0x6C04;
pub const HOST_RSP: u32 = 0x6C14;
pub const HOST_RIP: u32 = 0x6C16;
pub const HOST_GDTR_BASE: u32 = 0x6C0C;
pub const HOST_IDTR_BASE: u32 = 0x6C0E;

/// VM-exit qualification (natural-width read-only field).
pub const VM_EXIT_QUALIFICATION: u32 = 0x6400;
pub const GUEST_LINEAR_ADDR: u32 = 0x640A;

// ---------------------------------------------------------------------------
// Control field bit masks
// ---------------------------------------------------------------------------

/// Pin-based controls: External-interrupt exiting (bit 0).
pub const PIN_EXT_INTR_EXIT: u32 = 1 << 0;
/// Pin-based controls: NMI exiting (bit 3).
pub const PIN_NMI_EXIT: u32 = 1 << 3;

/// Primary proc-based: Interrupt-window exiting (bit 2).
pub const PROC_INTR_WIN_EXIT: u32 = 1 << 2;
/// Primary proc-based: HLT exiting (bit 7).
pub const PROC_HLT_EXIT: u32 = 1 << 7;
/// Primary proc-based: RDTSC exiting (bit 12).
pub const PROC_RDTSC_EXIT: u32 = 1 << 12;
/// Primary proc-based: TPR shadow (bit 21).
pub const PROC_TPR_SHADOW: u32 = 1 << 21;
/// Primary proc-based: Activate secondary controls (bit 31).
pub const PROC_SECONDARY_CTLS: u32 = 1 << 31;

/// Secondary proc-based: VPID (bit 5).
pub const PROC2_VPID: u32 = 1 << 5;
/// Secondary proc-based: Unrestricted guest (bit 7).
pub const PROC2_UNREST_GUEST: u32 = 1 << 7;
/// Secondary proc-based: EPT (bit 1).
pub const PROC2_EPT: u32 = 1 << 1;

/// VM-exit controls: Acknowledge interrupt on exit (bit 15).
pub const EXIT_ACK_INTR: u32 = 1 << 15;
/// VM-exit controls: Save IA32_EFER on exit (bit 20).
pub const EXIT_SAVE_EFER: u32 = 1 << 20;
/// VM-exit controls: Load IA32_EFER on exit (bit 21).
pub const EXIT_LOAD_EFER: u32 = 1 << 21;
/// VM-exit controls: Host address-space size (bit 9) — 64-bit host.
pub const EXIT_HOST_64: u32 = 1 << 9;

/// VM-entry controls: Load IA32_EFER on entry (bit 15).
pub const ENTRY_LOAD_EFER: u32 = 1 << 15;
/// VM-entry controls: Guest address-space size (bit 9) — 64-bit guest.
pub const ENTRY_GUEST_64: u32 = 1 << 9;

// ---------------------------------------------------------------------------
// Module init
// ---------------------------------------------------------------------------

/// Initialise the VMX driver for the bootstrap processor.
///
/// Detects VMX support, programs IA32_FEATURE_CONTROL, enables VMX root
/// operation, and logs the result.
pub fn init() {
    if !vmx_supported() {
        serial_println!("[VMX] Intel VT-x NOT supported on this CPU — VMX driver disabled");
        return;
    }
    serial_println!("[VMX] Intel VT-x detected (CPUID.1:ECX[5]=1)");

    let result = unsafe { vmx_enable() };
    match result {
        Ok(()) => serial_println!("[VMX] VMX root operation active"),
        Err(e) => serial_println!("[VMX] VMXON failed: {}", e),
    }
}
