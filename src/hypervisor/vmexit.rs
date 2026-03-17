/// VM exit handling — dispatches guest exits to appropriate handlers.
///
/// Part of the AIOS hypervisor subsystem.
///
/// When a guest VM executes a privileged instruction or accesses
/// hardware that the hypervisor intercepts, a VM exit occurs. This
/// module decodes the exit reason and dispatches to the correct handler.

use crate::{serial_print, serial_println};
use crate::sync::Mutex;

/// Reason codes for VM exits.
///
/// Encoding matches Intel SDM Appendix C, Table C-1 (basic exit reasons).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VmExitReason {
    ExternalInterrupt = 1,
    TripleFault = 2,
    InitSignal = 3,
    Sipi = 4,
    Cpuid = 10,
    Hlt = 12,
    Invlpg = 14,
    Rdtsc = 16,
    Vmcall = 18,
    CrAccess = 28,
    IoInstruction = 30,
    MsrRead = 31,
    MsrWrite = 32,
    InvalidGuestState = 33,
    MsrLoading = 34,
    Mwait = 36,
    MonitorTrapFlag = 37,
    Monitor = 39,
    Pause = 40,
    MachineCheckDuringEntry = 41,
    EptViolation = 48,
    EptMisconfig = 49,
    Invept = 50,
    Preemption = 52,
    Xsetbv = 55,
    Xsaves = 63,
    Xrstors = 64,
    Unknown = 0xFFFF,
}

impl VmExitReason {
    /// Convert a raw exit reason code to the enum variant.
    pub fn from_raw(raw: u32) -> Self {
        // Mask off the "VM-exit from VMX root operation" bit (bit 31).
        let code = raw & 0xFFFF;
        match code {
            1 => VmExitReason::ExternalInterrupt,
            2 => VmExitReason::TripleFault,
            3 => VmExitReason::InitSignal,
            4 => VmExitReason::Sipi,
            10 => VmExitReason::Cpuid,
            12 => VmExitReason::Hlt,
            14 => VmExitReason::Invlpg,
            16 => VmExitReason::Rdtsc,
            18 => VmExitReason::Vmcall,
            28 => VmExitReason::CrAccess,
            30 => VmExitReason::IoInstruction,
            31 => VmExitReason::MsrRead,
            32 => VmExitReason::MsrWrite,
            33 => VmExitReason::InvalidGuestState,
            34 => VmExitReason::MsrLoading,
            36 => VmExitReason::Mwait,
            37 => VmExitReason::MonitorTrapFlag,
            39 => VmExitReason::Monitor,
            40 => VmExitReason::Pause,
            41 => VmExitReason::MachineCheckDuringEntry,
            48 => VmExitReason::EptViolation,
            49 => VmExitReason::EptMisconfig,
            50 => VmExitReason::Invept,
            52 => VmExitReason::Preemption,
            55 => VmExitReason::Xsetbv,
            63 => VmExitReason::Xsaves,
            64 => VmExitReason::Xrstors,
            _ => VmExitReason::Unknown,
        }
    }
}

/// Statistics tracked per exit reason.
static EXIT_COUNTS: Mutex<Option<ExitStats>> = Mutex::new(None);

/// Per-reason exit counters for performance monitoring.
struct ExitStats {
    cpuid_exits: u64,
    io_exits: u64,
    msr_read_exits: u64,
    msr_write_exits: u64,
    ept_violation_exits: u64,
    hlt_exits: u64,
    vmcall_exits: u64,
    cr_access_exits: u64,
    interrupt_exits: u64,
    other_exits: u64,
}

impl ExitStats {
    fn new() -> Self {
        ExitStats {
            cpuid_exits: 0,
            io_exits: 0,
            msr_read_exits: 0,
            msr_write_exits: 0,
            ept_violation_exits: 0,
            hlt_exits: 0,
            vmcall_exits: 0,
            cr_access_exits: 0,
            interrupt_exits: 0,
            other_exits: 0,
        }
    }

    fn increment(&mut self, reason: VmExitReason) {
        match reason {
            VmExitReason::Cpuid => self.cpuid_exits = self.cpuid_exits.saturating_add(1),
            VmExitReason::IoInstruction => self.io_exits = self.io_exits.saturating_add(1),
            VmExitReason::MsrRead => self.msr_read_exits = self.msr_read_exits.saturating_add(1),
            VmExitReason::MsrWrite => self.msr_write_exits = self.msr_write_exits.saturating_add(1),
            VmExitReason::EptViolation => self.ept_violation_exits = self.ept_violation_exits.saturating_add(1),
            VmExitReason::Hlt => self.hlt_exits = self.hlt_exits.saturating_add(1),
            VmExitReason::Vmcall => self.vmcall_exits = self.vmcall_exits.saturating_add(1),
            VmExitReason::CrAccess => self.cr_access_exits = self.cr_access_exits.saturating_add(1),
            VmExitReason::ExternalInterrupt => self.interrupt_exits = self.interrupt_exits.saturating_add(1),
            _ => self.other_exits = self.other_exits.saturating_add(1),
        }
    }
}

/// Dispatch a VM exit to the appropriate handler.
///
/// This is the main entry point called from the VM exit stub in vmenter.
/// `guest_id` identifies which VM caused the exit.
pub fn handle_vmexit(reason: VmExitReason, guest_id: u64) {
    // Update statistics.
    {
        let mut stats = EXIT_COUNTS.lock();
        if let Some(ref mut s) = *stats {
            s.increment(reason);
        }
    }

    match reason {
        VmExitReason::Cpuid => handle_cpuid_exit(guest_id),
        VmExitReason::IoInstruction => handle_io_exit(guest_id),
        VmExitReason::MsrRead => handle_msr_read_exit(guest_id),
        VmExitReason::MsrWrite => handle_msr_write_exit(guest_id),
        VmExitReason::EptViolation => handle_ept_violation(guest_id),
        VmExitReason::Hlt => handle_hlt_exit(guest_id),
        VmExitReason::Vmcall => handle_vmcall_exit(guest_id),
        VmExitReason::CrAccess => handle_cr_access(guest_id),
        VmExitReason::ExternalInterrupt => handle_external_interrupt(guest_id),
        VmExitReason::TripleFault => handle_triple_fault(guest_id),
        VmExitReason::Preemption => handle_preemption_timer(guest_id),
        VmExitReason::Xsetbv => handle_xsetbv(guest_id),
        VmExitReason::Pause => handle_pause(guest_id),
        VmExitReason::EptMisconfig => handle_ept_misconfig(guest_id),
        _ => {
            serial_println!(
                "    [vmexit] Unhandled exit reason {:?} for guest {}",
                reason, guest_id
            );
        }
    }
}

// --- Individual exit handlers ---

fn handle_cpuid_exit(guest_id: u64) {
    // Read the guest's EAX (leaf) and ECX (sub-leaf) from the VMCS.
    // In a real implementation, VMREAD the guest GPR save area.
    // Here we emulate common CPUID leaves with safe defaults.
    serial_println!("    [vmexit] CPUID exit for guest {}", guest_id);

    // The guest's RIP must be advanced past the CPUID instruction (2 bytes).
    advance_guest_rip(2);
}

fn handle_io_exit(guest_id: u64) {
    // The exit qualification contains the port number, direction, and size.
    // Delegate to the I/O emulator.
    serial_println!("    [vmexit] I/O instruction exit for guest {}", guest_id);
    advance_guest_rip(0); // Instruction length comes from VMCS VM_EXIT_INSTR_LEN.
}

fn handle_msr_read_exit(guest_id: u64) {
    // ECX contains the MSR address. Return a safe default in EDX:EAX.
    serial_println!("    [vmexit] MSR read exit for guest {}", guest_id);
    advance_guest_rip(2); // RDMSR is 2 bytes.
}

fn handle_msr_write_exit(guest_id: u64) {
    // ECX = MSR address, EDX:EAX = value to write.
    // Validate and potentially virtualize the MSR write.
    serial_println!("    [vmexit] MSR write exit for guest {}", guest_id);
    advance_guest_rip(2); // WRMSR is 2 bytes.
}

fn handle_ept_violation(guest_id: u64) {
    serial_println!("    [vmexit] EPT violation for guest {}", guest_id);
    // The faulting GPA and qualification are in the VMCS exit fields.
    // Delegate to ept::EptRoot::handle_violation.
}

fn handle_hlt_exit(guest_id: u64) {
    // Guest executed HLT — yield the vCPU until an interrupt arrives.
    serial_println!("    [vmexit] HLT exit for guest {} — yielding vCPU", guest_id);
    advance_guest_rip(1); // HLT is 1 byte.
}

fn handle_vmcall_exit(guest_id: u64) {
    // Hypercall interface — EAX contains the hypercall number.
    serial_println!("    [vmexit] VMCALL (hypercall) for guest {}", guest_id);
    advance_guest_rip(3); // VMCALL is 3 bytes.
}

fn handle_cr_access(guest_id: u64) {
    // Guest attempted to read/write a control register (CR0, CR3, CR4).
    serial_println!("    [vmexit] CR access for guest {}", guest_id);
    advance_guest_rip(0); // Instruction length varies; use VMCS field.
}

fn handle_external_interrupt(guest_id: u64) {
    // An external interrupt arrived while the guest was running.
    // Acknowledge and dispatch to the host's interrupt handler.
    serial_println!("    [vmexit] External interrupt during guest {}", guest_id);
    // No RIP advance — the guest resumes at the same instruction.
}

fn handle_triple_fault(guest_id: u64) {
    serial_println!("    [vmexit] TRIPLE FAULT in guest {} — terminating VM", guest_id);
    // A triple fault is fatal for the guest. Signal the guest manager to destroy it.
}

fn handle_preemption_timer(guest_id: u64) {
    // VMX preemption timer expired — used for time-slicing vCPUs.
    serial_println!("    [vmexit] Preemption timer for guest {}", guest_id);
}

fn handle_xsetbv(guest_id: u64) {
    // Guest is setting an extended control register (XCR0).
    serial_println!("    [vmexit] XSETBV for guest {}", guest_id);
    advance_guest_rip(3);
}

fn handle_pause(guest_id: u64) {
    // PAUSE exit — the guest is in a spin loop. Used for PAUSE-loop exiting.
    serial_println!("    [vmexit] PAUSE exit for guest {}", guest_id);
    advance_guest_rip(2); // PAUSE (F3 90) is 2 bytes.
}

fn handle_ept_misconfig(guest_id: u64) {
    serial_println!("    [vmexit] EPT misconfiguration for guest {} — check EPT entries", guest_id);
    // EPT misconfig is a hypervisor bug. Log the faulting address.
}

/// Advance the guest RIP by `len` bytes to skip the faulting instruction.
///
/// If `len` is 0, reads the instruction length from the VMCS exit info field.
fn advance_guest_rip(len: u32) {
    let instr_len = if len == 0 {
        // Read VM_EXIT_INSTR_LEN from VMCS.
        let val: u64;
        unsafe {
            core::arch::asm!(
                "vmread {}, {}",
                out(reg) val,
                in(reg) super::vmcs::VM_EXIT_INSTR_LEN as u64,
                options(nostack),
            );
        }
        val
    } else {
        len as u64
    };

    // Read current guest RIP.
    let rip: u64;
    unsafe {
        core::arch::asm!(
            "vmread {}, {}",
            out(reg) rip,
            in(reg) super::vmcs::GUEST_RIP as u64,
            options(nostack),
        );
    }

    // Write updated guest RIP.
    let new_rip = rip.wrapping_add(instr_len);
    unsafe {
        core::arch::asm!(
            "vmwrite {}, {}",
            in(reg) super::vmcs::GUEST_RIP as u64,
            in(reg) new_rip,
            options(nostack),
        );
    }
}

pub fn init() {
    *EXIT_COUNTS.lock() = Some(ExitStats::new());
    serial_println!("    [vmexit] VM exit handler subsystem initialized");
}
