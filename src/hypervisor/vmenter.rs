/// VM entry setup — prepares and launches guest execution.
///
/// Part of the AIOS hypervisor subsystem.
///
/// This module handles the transition from hypervisor (VMX root)
/// into guest mode (VMX non-root). It configures entry controls,
/// event injection, and executes VMLAUNCH or VMRESUME.

use crate::{serial_print, serial_println};
use crate::sync::Mutex;

/// VM-entry interruption-info field bits (Intel SDM Vol. 3C, 24.8.3).
const ENTRY_INTR_INFO_VALID: u32 = 1 << 31;
const ENTRY_INTR_TYPE_EXT_INT: u32 = 0 << 8;
const ENTRY_INTR_TYPE_NMI: u32 = 2 << 8;
const ENTRY_INTR_TYPE_EXCEPTION: u32 = 3 << 8;
const ENTRY_INTR_TYPE_SW_INT: u32 = 4 << 8;

/// VM-entry control bits.
const ENTRY_CTRL_LOAD_DEBUG: u64 = 1 << 2;
const ENTRY_CTRL_IA32E_GUEST: u64 = 1 << 9;
const ENTRY_CTRL_LOAD_PAT: u64 = 1 << 14;
const ENTRY_CTRL_LOAD_EFER: u64 = 1 << 15;

/// Global default entry configuration.
static DEFAULT_ENTRY: Mutex<Option<VmEntryDefaults>> = Mutex::new(None);

/// Default entry control values applied to every VM launch.
struct VmEntryDefaults {
    /// Default VM-entry controls.
    entry_controls: u64,
    /// Whether to enter a 64-bit guest (IA-32e mode).
    ia32e_mode: bool,
}

/// Configuration for a VM entry.
pub struct VmEntryConfig {
    /// VM-entry interruption-information field (event injection).
    /// Set to 0 if no event should be injected.
    pub injection_info: u32,
    /// Error code to inject with an exception (valid only for certain vectors).
    pub injection_error_code: u32,
    /// Instruction length for injected software interrupts/exceptions.
    pub injection_instr_len: u32,
    /// VM-entry controls override. 0 = use defaults.
    pub entry_controls: u64,
    /// Whether this is a first launch (VMLAUNCH) vs. resume (VMRESUME).
    pub is_launch: bool,
    /// Guest ID for this entry.
    pub guest_id: u64,
}

impl VmEntryConfig {
    pub fn new() -> Self {
        let default_controls = {
            let defaults = DEFAULT_ENTRY.lock();
            if let Some(ref d) = *defaults {
                d.entry_controls
            } else {
                ENTRY_CTRL_LOAD_DEBUG | ENTRY_CTRL_IA32E_GUEST
            }
        };

        VmEntryConfig {
            injection_info: 0,
            injection_error_code: 0,
            injection_instr_len: 0,
            entry_controls: default_controls,
            is_launch: true,
            guest_id: 0,
        }
    }

    /// Configure injection of an external interrupt into the guest.
    pub fn inject_interrupt(&mut self, vector: u8) {
        self.injection_info = ENTRY_INTR_INFO_VALID
            | ENTRY_INTR_TYPE_EXT_INT
            | (vector as u32);
    }

    /// Configure injection of an exception into the guest.
    pub fn inject_exception(&mut self, vector: u8, error_code: Option<u32>) {
        let has_error = if error_code.is_some() { 1u32 << 11 } else { 0 };
        self.injection_info = ENTRY_INTR_INFO_VALID
            | ENTRY_INTR_TYPE_EXCEPTION
            | has_error
            | (vector as u32);
        self.injection_error_code = error_code.unwrap_or(0);
    }

    /// Configure injection of an NMI.
    pub fn inject_nmi(&mut self) {
        self.injection_info = ENTRY_INTR_INFO_VALID
            | ENTRY_INTR_TYPE_NMI
            | 2; // NMI vector = 2.
    }
}

/// Write entry control fields to the active VMCS, then execute VMLAUNCH or VMRESUME.
///
/// # Safety
/// This function does not return on success — execution transfers to the guest.
/// On failure (VM-entry failure), it returns via a VM exit to the host RIP.
///
/// # Diverging
/// Marked as `-> !` because on success we enter guest mode.
/// The hypervisor regains control only on the next VM exit.
pub fn enter_guest(config: &VmEntryConfig) -> ! {
    // Write VM-entry controls to VMCS.
    unsafe {
        core::arch::asm!(
            "vmwrite {}, {}",
            in(reg) super::vmcs::VM_ENTRY_CONTROLS as u64,
            in(reg) config.entry_controls,
            options(nostack),
        );
    }

    // If we need to inject an event, write the injection info fields.
    if config.injection_info & ENTRY_INTR_INFO_VALID != 0 {
        unsafe {
            // VM-entry interruption-information field (encoding 0x4016).
            core::arch::asm!(
                "vmwrite {}, {}",
                in(reg) 0x4016u64,
                in(reg) config.injection_info as u64,
                options(nostack),
            );
            // VM-entry exception error code (encoding 0x4018).
            core::arch::asm!(
                "vmwrite {}, {}",
                in(reg) 0x4018u64,
                in(reg) config.injection_error_code as u64,
                options(nostack),
            );
            // VM-entry instruction length (encoding 0x401A).
            core::arch::asm!(
                "vmwrite {}, {}",
                in(reg) 0x401Au64,
                in(reg) config.injection_instr_len as u64,
                options(nostack),
            );
        }
    }

    serial_println!(
        "    [vmenter] Entering guest {} (launch={})",
        config.guest_id, config.is_launch
    );

    if config.is_launch {
        // First entry — use VMLAUNCH.
        unsafe {
            core::arch::asm!(
                // Save host GPRs (callee-saved) for the VM exit path.
                "push rbx",
                "push rbp",
                "push r12",
                "push r13",
                "push r14",
                "push r15",
                "vmlaunch",
                // If VMLAUNCH fails, we fall through here.
                "pop r15",
                "pop r14",
                "pop r13",
                "pop r12",
                "pop rbp",
                "pop rbx",
                options(nostack),
            );
        }
    } else {
        // Subsequent entry — use VMRESUME.
        unsafe {
            core::arch::asm!(
                "push rbx",
                "push rbp",
                "push r12",
                "push r13",
                "push r14",
                "push r15",
                "vmresume",
                "pop r15",
                "pop r14",
                "pop r13",
                "pop r12",
                "pop rbp",
                "pop rbx",
                options(nostack),
            );
        }
    }

    // If we reach here, VMLAUNCH/VMRESUME failed.
    // Read the VM-instruction error field from VMCS (encoding 0x4400).
    let error: u64;
    unsafe {
        core::arch::asm!(
            "vmread {}, {}",
            out(reg) error,
            in(reg) 0x4400u64,
            options(nostack),
        );
    }
    serial_println!(
        "    [vmenter] VM entry FAILED for guest {} (error code={})",
        config.guest_id, error
    );

    // Cannot truly return from a diverging function, so loop.
    loop {
        crate::io::hlt();
    }
}

/// Resume guest execution after a VM exit.
///
/// Reads the saved guest state for `guest_id` and re-enters
/// with VMRESUME.
pub fn resume_guest(guest_id: u64) -> ! {
    let mut config = VmEntryConfig::new();
    config.is_launch = false;
    config.guest_id = guest_id;
    enter_guest(&config)
}

pub fn init() {
    let defaults = VmEntryDefaults {
        entry_controls: ENTRY_CTRL_LOAD_DEBUG | ENTRY_CTRL_IA32E_GUEST,
        ia32e_mode: true,
    };

    *DEFAULT_ENTRY.lock() = Some(defaults);
    serial_println!("    [vmenter] VM entry subsystem initialized");
}
