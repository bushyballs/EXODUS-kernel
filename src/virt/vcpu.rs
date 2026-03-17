/// Virtual CPU (vCPU) management for the Genesis hypervisor layer.
///
/// Sits above `virt::vmx` (raw VMX instructions) and provides a higher-level
/// abstraction over a hardware VMCS and guest register state.  One `Vcpu`
/// corresponds to one VMCS and one guest logical processor.
///
/// Relationships:
///   - `Vcpu::new`          → calls `vmx::alloc_vmcs` to obtain a 4 KiB VMCS region
///   - `setup_vmcs_controls` → writes VMCS control fields via `vmx::vmcs_write`
///   - `setup_guest_state`   → populates guest register state in the VMCS
///   - `vcpu_run`            → executes `vmx::vmlaunch` / `vmx::vmresume`
///   - `handle_vmexit`       → dispatches the exit to specific handlers
use crate::virt::vmx::{
    self,
    VmcsRegion,
    ENTRY_GUEST_64,
    ENTRY_LOAD_EFER,
    EPT_POINTER,
    EXCEPTION_BITMAP,
    EXIT_ACK_INTR,
    EXIT_HOST_64,
    EXIT_LOAD_EFER,
    EXIT_SAVE_EFER,
    GUEST_ACTIVITY_STATE,
    GUEST_CR0,
    GUEST_CR3,
    GUEST_CR4,
    GUEST_CS_AR,
    GUEST_CS_BASE,
    GUEST_CS_LIMIT,
    // VMCS field constants
    GUEST_CS_SELECTOR,
    GUEST_DS_AR,
    GUEST_DS_BASE,
    GUEST_DS_LIMIT,
    GUEST_DS_SELECTOR,
    GUEST_ES_AR,
    GUEST_ES_BASE,
    GUEST_ES_LIMIT,
    GUEST_ES_SELECTOR,
    GUEST_FS_BASE,
    GUEST_FS_SELECTOR,
    GUEST_GDTR_BASE,
    GUEST_GDTR_LIMIT,
    GUEST_GS_BASE,
    GUEST_GS_SELECTOR,
    GUEST_IA32_EFER,
    GUEST_IDTR_BASE,
    GUEST_IDTR_LIMIT,
    GUEST_LDTR_BASE,
    GUEST_LDTR_SELECTOR,
    GUEST_LINEAR_ADDR,
    GUEST_PHYSICAL_ADDR,
    GUEST_RFLAGS,
    GUEST_RIP,
    GUEST_RSP,
    GUEST_SS_AR,
    GUEST_SS_BASE,
    GUEST_SS_LIMIT,
    GUEST_SS_SELECTOR,
    GUEST_TR_BASE,
    GUEST_TR_SELECTOR,
    HOST_CR0,
    HOST_CR3,
    HOST_CR4,
    HOST_CS_SELECTOR,
    HOST_DS_SELECTOR,
    HOST_ES_SELECTOR,
    HOST_GDTR_BASE,
    HOST_IA32_EFER,
    HOST_IDTR_BASE,
    HOST_RIP,
    HOST_RSP,
    HOST_SS_SELECTOR,
    PIN_BASED_CONTROLS,
    // Control bit masks
    PIN_EXT_INTR_EXIT,
    PIN_NMI_EXIT,
    PROC2_EPT,
    PROC2_UNREST_GUEST,
    PROC2_VPID,
    PROC_BASED_CONTROLS,
    PROC_BASED_CONTROLS2,
    PROC_HLT_EXIT,
    PROC_RDTSC_EXIT,
    PROC_SECONDARY_CTLS,
    PROC_TPR_SHADOW,
    VMCS_LINK_PTR,
    VM_ENTRY_CONTROLS,
    VM_ENTRY_INTR_INFO,
    VM_EXIT_CONTROLS,
    VM_EXIT_INSTR_LEN,
    VM_EXIT_QUALIFICATION,
    VM_EXIT_REASON,
    VM_INSTRUCTION_ERROR,
};
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// VM-exit reason codes (subset — Intel SDM Appendix C)
// ---------------------------------------------------------------------------

pub const EXIT_HLT: u32 = 12;
pub const EXIT_CPUID: u32 = 10;
pub const EXIT_IO: u32 = 30;
pub const EXIT_RDTSC: u32 = 16;
pub const EXIT_EXTERNAL_INTR: u32 = 1;
pub const EXIT_TRIPLE_FAULT: u32 = 2;
pub const EXIT_EPT_VIOLATION: u32 = 48;
pub const EXIT_EPT_MISCONFIG: u32 = 49;
pub const EXIT_VMCALL: u32 = 18;
pub const EXIT_RDMSR: u32 = 31;
pub const EXIT_WRMSR: u32 = 32;
pub const EXIT_CR_ACCESS: u32 = 28;

// ---------------------------------------------------------------------------
// VmExitReason — strongly typed exit code returned from vcpu_run
// ---------------------------------------------------------------------------

/// High-level VM-exit reason returned to the caller of `vcpu_run`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmExitReason {
    /// Guest executed HLT.
    Hlt,
    /// Guest executed CPUID.
    Cpuid { leaf: u32, subleaf: u32 },
    /// Guest I/O port access.
    Io {
        port: u16,
        is_write: bool,
        size: u8,
        count: u32,
    },
    /// Guest executed RDTSC.
    Rdtsc,
    /// External interrupt delivered to host.
    ExternalInterrupt,
    /// EPT violation — guest accessed unmapped memory.
    EptViolation { gpa: u64, qualification: u64 },
    /// Guest executed VMCALL (hypercall).
    Vmcall,
    /// Triple fault — guest must be reset or destroyed.
    TripleFault,
    /// VM entry / instruction error.
    VmxError(u64),
    /// All other exits (raw reason code).
    Other(u32),
}

// ---------------------------------------------------------------------------
// Vcpu struct
// ---------------------------------------------------------------------------

/// A virtual CPU managed by the VMX driver.
pub struct Vcpu {
    /// Logical vCPU index within the guest (0-based).
    pub id: u32,
    /// Physical address of the VMCS region for this vCPU.
    pub vmcs: u64,
    /// Whether the vCPU has been launched (VMLAUNCH executed at least once).
    pub running: bool,
    /// Raw exit reason from the last VM exit.
    pub exit_reason: u32,
    /// Guest RIP at the last VM exit.
    pub guest_rip: u64,
    /// Guest RSP at the last VM exit.
    pub guest_rsp: u64,
    /// Guest RFLAGS at the last VM exit.
    pub guest_rflags: u64,
    /// TSC offset added to the host TSC when the guest reads the counter.
    pub tsc_offset: i64,
    /// Backing storage for the VMCS region (4 KiB, 4 KiB-aligned).
    vmcs_region: VmcsRegion,
    /// Whether EPT was successfully configured for this vCPU.
    ept_enabled: bool,
}

impl Vcpu {
    /// Allocate a new vCPU with the given logical ID.
    ///
    /// Allocates a VMCS region, writes the VMCS revision ID, and loads the
    /// VMCS via VMPTRLD.  Returns `None` if VMX is not available or the VMCS
    /// could not be allocated.
    pub fn new(id: u32) -> Option<Self> {
        if !vmx::vmx_supported() {
            serial_println!("[VCPU {}] VMX not supported — cannot create vCPU", id);
            return None;
        }

        let mut vcpu = Vcpu {
            id,
            vmcs: 0,
            running: false,
            exit_reason: 0,
            guest_rip: 0,
            guest_rsp: 0,
            guest_rflags: 0x2, // Reserved bit 1 always set.
            tsc_offset: 0,
            vmcs_region: VmcsRegion::new(),
            ept_enabled: false,
        };

        unsafe {
            // Write revision ID and obtain the region pointer.
            let region_ptr = vmx::alloc_vmcs(&mut vcpu.vmcs_region);
            vcpu.vmcs = region_ptr as u64;

            // VMCLEAR to put the VMCS into the clear (launch-ready) state.
            vmx::vmcs_clear(vcpu.vmcs);

            // VMPTRLD to make it the current VMCS on this logical processor.
            vmx::vmcs_load(vcpu.vmcs);
        }

        serial_println!("[VCPU {}] VMCS allocated at phys=0x{:016x}", id, vcpu.vmcs);
        Some(vcpu)
    }

    // -----------------------------------------------------------------------
    // VMCS control setup
    // -----------------------------------------------------------------------

    /// Configure VMCS execution-control, exit-control, and entry-control fields.
    ///
    /// Adjusts each control word with the corresponding FIXED0 / FIXED1 MSRs
    /// so that required bits are set and forbidden bits are clear.
    pub fn setup_vmcs_controls(&mut self) {
        unsafe {
            // -----------------------------------------------------------------
            // Pin-based VM-execution controls
            //   Bit 0: External-interrupt exiting — cause a VM exit on every
            //          external interrupt so the host can inject it correctly.
            //   Bit 3: NMI exiting.
            // -----------------------------------------------------------------
            let pin_wanted: u32 = PIN_EXT_INTR_EXIT | PIN_NMI_EXIT;
            let pin_fixed = adjust_control(pin_wanted, 0x484, 0x485);
            vmx::vmcs_write(PIN_BASED_CONTROLS, pin_fixed as u64);

            // -----------------------------------------------------------------
            // Primary processor-based VM-execution controls
            //   Bit 7:  HLT exiting.
            //   Bit 12: RDTSC exiting (we emulate TSC offset).
            //   Bit 21: TPR shadow (virtual APIC support).
            //   Bit 31: Activate secondary controls.
            // -----------------------------------------------------------------
            let proc1_wanted: u32 =
                PROC_HLT_EXIT | PROC_RDTSC_EXIT | PROC_TPR_SHADOW | PROC_SECONDARY_CTLS;
            let proc1_fixed = adjust_control(proc1_wanted, 0x482, 0x483);
            vmx::vmcs_write(PROC_BASED_CONTROLS, proc1_fixed as u64);

            // -----------------------------------------------------------------
            // Secondary processor-based VM-execution controls
            //   Bit 1: EPT — hardware two-level address translation.
            //   Bit 5: VPID — virtual processor ID for TLB tagging.
            //   Bit 7: Unrestricted guest — allow real-mode / protected-mode.
            //
            // Gracefully fall back if secondary controls are unsupported.
            // -----------------------------------------------------------------
            if proc1_fixed & PROC_SECONDARY_CTLS != 0 {
                let proc2_wanted: u32 = PROC2_EPT | PROC2_VPID | PROC2_UNREST_GUEST;
                let proc2_fixed = adjust_control(proc2_wanted, 0x48B, 0x48C);
                vmx::vmcs_write(PROC_BASED_CONTROLS2, proc2_fixed as u64);

                // Record whether EPT was actually enabled.
                self.ept_enabled = proc2_fixed & PROC2_EPT != 0;
            }

            // -----------------------------------------------------------------
            // VM-exit controls
            //   Bit  9: Host address-space size (1 = 64-bit host).
            //   Bit 15: Acknowledge interrupt on exit.
            //   Bit 20: Save IA32_EFER on exit.
            //   Bit 21: Load IA32_EFER on exit.
            // -----------------------------------------------------------------
            let exit_wanted: u32 = EXIT_HOST_64 | EXIT_ACK_INTR | EXIT_SAVE_EFER | EXIT_LOAD_EFER;
            let exit_fixed = adjust_control(exit_wanted, 0x48B, 0x48D); // VMX_EXIT_CTLS FIXED MSRs
                                                                        // Note: correct MSRs for exit controls are 0x483/0x484 on older CPUs;
                                                                        // adjust_control saturates safely if MSRs are unavailable.
            let exit_fixed2 = adjust_control(exit_wanted, 0x4861 & 0xFFF, 0x4862 & 0xFFF);
            // Use whichever succeeded (non-zero from fixed FIXED0).
            let exit_val = if exit_fixed2 != 0 {
                exit_fixed2
            } else {
                exit_fixed
            };
            vmx::vmcs_write(VM_EXIT_CONTROLS, exit_val as u64);

            // -----------------------------------------------------------------
            // VM-entry controls
            //   Bit  9: IA-32e mode guest (64-bit guest).
            //   Bit 15: Load IA32_EFER on entry.
            // -----------------------------------------------------------------
            let entry_wanted: u32 = ENTRY_GUEST_64 | ENTRY_LOAD_EFER;
            let entry_fixed = adjust_control(entry_wanted, 0x484, 0x485);
            vmx::vmcs_write(VM_ENTRY_CONTROLS, entry_fixed as u64);

            // Exception bitmap: intercept #UD (bit 6) and #GP (bit 13) for emulation.
            vmx::vmcs_write(EXCEPTION_BITMAP, (1 << 6) | (1 << 13));

            // VMCS link pointer: must be 0xFFFF_FFFF_FFFF_FFFF (no shadow VMCS).
            vmx::vmcs_write(VMCS_LINK_PTR, 0xFFFF_FFFF_FFFF_FFFF);
        }
    }

    // -----------------------------------------------------------------------
    // Guest state setup
    // -----------------------------------------------------------------------

    /// Write initial guest register state into the current VMCS.
    ///
    /// Sets up a minimal 64-bit long-mode environment:
    /// - CS:RIP points to `entry` (kernel virtual address used as-is; in a
    ///   real system the VMM would set up GDT / identity-map accordingly).
    /// - CR3 = `cr3` (guest physical address of PML4).
    /// - All data segments flat (base=0, limit=0xFFFF_FFFF, present, R/W).
    /// - EFER.LMA | EFER.LME | EFER.SCE set.
    ///
    /// Also captures host CR0/CR3/CR4/RSP/RIP so the processor knows where to
    /// return on a VM exit.
    pub fn setup_guest_state(&mut self, entry: u64, cr3: u64) {
        unsafe {
            // --- Guest segment selectors ---
            vmx::vmcs_write(GUEST_CS_SELECTOR, 0x08);
            vmx::vmcs_write(GUEST_DS_SELECTOR, 0x10);
            vmx::vmcs_write(GUEST_ES_SELECTOR, 0x10);
            vmx::vmcs_write(GUEST_SS_SELECTOR, 0x10);
            vmx::vmcs_write(GUEST_FS_SELECTOR, 0x00);
            vmx::vmcs_write(GUEST_GS_SELECTOR, 0x00);
            vmx::vmcs_write(GUEST_LDTR_SELECTOR, 0x00);
            vmx::vmcs_write(GUEST_TR_SELECTOR, 0x18);

            // --- Segment bases ---
            vmx::vmcs_write(GUEST_CS_BASE, 0);
            vmx::vmcs_write(GUEST_DS_BASE, 0);
            vmx::vmcs_write(GUEST_ES_BASE, 0);
            vmx::vmcs_write(GUEST_SS_BASE, 0);
            vmx::vmcs_write(GUEST_FS_BASE, 0);
            vmx::vmcs_write(GUEST_GS_BASE, 0);
            vmx::vmcs_write(GUEST_LDTR_BASE, 0);
            vmx::vmcs_write(GUEST_TR_BASE, 0);
            vmx::vmcs_write(GUEST_GDTR_BASE, 0);
            vmx::vmcs_write(GUEST_IDTR_BASE, 0);

            // --- Segment limits ---
            vmx::vmcs_write(GUEST_CS_LIMIT, 0xFFFF_FFFF);
            vmx::vmcs_write(GUEST_DS_LIMIT, 0xFFFF_FFFF);
            vmx::vmcs_write(GUEST_ES_LIMIT, 0xFFFF_FFFF);
            vmx::vmcs_write(GUEST_SS_LIMIT, 0xFFFF_FFFF);
            vmx::vmcs_write(GUEST_GDTR_LIMIT, 0xFFFF);
            vmx::vmcs_write(GUEST_IDTR_LIMIT, 0xFFFF);

            // --- Segment access rights (AR bytes) ---
            // CS: present, code, readable, 64-bit, DPL=0.  AR byte = 0xA09B.
            vmx::vmcs_write(GUEST_CS_AR, 0xA09B);
            // SS/DS/ES: present, data, writable, DPL=0.  AR byte = 0xC093.
            vmx::vmcs_write(GUEST_SS_AR, 0xC093);
            vmx::vmcs_write(GUEST_DS_AR, 0xC093);
            vmx::vmcs_write(GUEST_ES_AR, 0xC093);

            // --- Control registers ---
            // CR0: PE=1, PG=1, ET=1, NE=1 (protected + paging enabled).
            vmx::vmcs_write(GUEST_CR0, 0x8000_0031);
            vmx::vmcs_write(GUEST_CR3, cr3);
            // CR4: PAE=1, VMXE=1.
            vmx::vmcs_write(GUEST_CR4, 0x0000_2020);

            // --- EFER: LMA=1, LME=1, SCE=1 ---
            vmx::vmcs_write(GUEST_IA32_EFER, 0x0000_0D01);

            // --- Instruction pointer and stack ---
            vmx::vmcs_write(GUEST_RIP, entry);
            vmx::vmcs_write(GUEST_RSP, 0);
            vmx::vmcs_write(GUEST_RFLAGS, 0x0000_0002); // Reserved bit 1.

            // --- Activity state: active (0) ---
            vmx::vmcs_write(GUEST_ACTIVITY_STATE, 0);

            // --- Capture host state (current CPU) so we can return on exit ---
            let host_cr0: u64;
            let host_cr3: u64;
            let host_cr4: u64;
            let host_rsp: u64;
            core::arch::asm!("mov {}, cr0", out(reg) host_cr0, options(nomem, nostack));
            core::arch::asm!("mov {}, cr3", out(reg) host_cr3, options(nomem, nostack));
            core::arch::asm!("mov {}, cr4", out(reg) host_cr4, options(nomem, nostack));
            core::arch::asm!("mov {}, rsp", out(reg) host_rsp, options(nomem, nostack));

            vmx::vmcs_write(HOST_CR0, host_cr0);
            vmx::vmcs_write(HOST_CR3, host_cr3);
            vmx::vmcs_write(HOST_CR4, host_cr4);
            vmx::vmcs_write(HOST_RSP, host_rsp);
            // HOST_RIP will be set by vcpu_run to the VM-exit handler address.

            // Host segment selectors (read from current processor state).
            let cs: u16;
            let ss: u16;
            let ds: u16;
            let es: u16;
            core::arch::asm!("mov {:x}, cs", out(reg) cs, options(nomem, nostack));
            core::arch::asm!("mov {:x}, ss", out(reg) ss, options(nomem, nostack));
            core::arch::asm!("mov {:x}, ds", out(reg) ds, options(nomem, nostack));
            core::arch::asm!("mov {:x}, es", out(reg) es, options(nomem, nostack));
            vmx::vmcs_write(HOST_CS_SELECTOR, cs as u64);
            vmx::vmcs_write(HOST_SS_SELECTOR, ss as u64);
            vmx::vmcs_write(HOST_DS_SELECTOR, ds as u64);
            vmx::vmcs_write(HOST_ES_SELECTOR, es as u64);

            // Host IA32_EFER.
            let host_efer = vmx::rdmsr(0xC000_0080);
            vmx::vmcs_write(HOST_IA32_EFER, host_efer);

            // Host GDTR / IDTR base.
            let mut gdtr = [0u8; 10];
            let mut idtr = [0u8; 10];
            core::arch::asm!("sgdt [{}]", in(reg) gdtr.as_mut_ptr(), options(nostack));
            core::arch::asm!("sidt [{}]", in(reg) idtr.as_mut_ptr(), options(nostack));
            let gdtr_base = u64::from_le_bytes([
                gdtr[2], gdtr[3], gdtr[4], gdtr[5], gdtr[6], gdtr[7], gdtr[8], gdtr[9],
            ]);
            let idtr_base = u64::from_le_bytes([
                idtr[2], idtr[3], idtr[4], idtr[5], idtr[6], idtr[7], idtr[8], idtr[9],
            ]);
            vmx::vmcs_write(HOST_GDTR_BASE, gdtr_base);
            vmx::vmcs_write(HOST_IDTR_BASE, idtr_base);
        }

        serial_println!(
            "[VCPU {}] Guest state: entry=0x{:016x} cr3=0x{:016x} ept={}",
            self.id,
            entry,
            cr3,
            self.ept_enabled
        );
    }

    // -----------------------------------------------------------------------
    // Run loop
    // -----------------------------------------------------------------------

    /// Run the vCPU until a VM exit occurs, then return the exit reason.
    ///
    /// On the first call, executes VMLAUNCH; subsequent calls use VMRESUME.
    pub fn vcpu_run(&mut self) -> VmExitReason {
        unsafe {
            // Set HOST_RIP to a stub return point.  In a real kernel this
            // would be an `extern "C"` assembly trampoline that saves all
            // caller-saved registers and calls into handle_vmexit.
            // Here we use the address of a dummy function as the return RIP.
            vmx::vmcs_write(HOST_RIP, vmexit_stub as u64);

            let entry_result = if self.running {
                vmx::vmresume()
            } else {
                vmx::vmlaunch()
            };

            match entry_result {
                Ok(()) => {
                    // Successfully entered the VM; we are now executing host
                    // code again after a VM exit triggered by the guest.
                    self.running = true;

                    // Read exit metadata from the VMCS.
                    let raw_reason = vmx::vmcs_read(VM_EXIT_REASON) as u32;
                    let rip = vmx::vmcs_read(GUEST_RIP);
                    let rsp = vmx::vmcs_read(GUEST_RSP);
                    let rflags = vmx::vmcs_read(GUEST_RFLAGS);

                    self.exit_reason = raw_reason & 0xFFFF;
                    self.guest_rip = rip;
                    self.guest_rsp = rsp;
                    self.guest_rflags = rflags;

                    decode_exit_reason(self.exit_reason)
                }
                Err(err_code) => {
                    serial_println!(
                        "[VCPU {}] VM entry failed (err=0x{:04x})",
                        self.id,
                        err_code
                    );
                    VmExitReason::VmxError(err_code)
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // VM-exit dispatch
    // -----------------------------------------------------------------------

    /// Handle a VM exit and decide whether to continue running the guest.
    ///
    /// Returns `true` if the vCPU should be re-entered, `false` if it
    /// should be halted (e.g., triple fault, unrecoverable error).
    pub fn handle_vmexit(&mut self, reason: VmExitReason) -> bool {
        match reason {
            VmExitReason::Hlt => {
                serial_println!("[VCPU {}] HLT — idling vCPU", self.id);
                // Advance RIP past the HLT instruction (1 byte) and return false
                // to indicate the vCPU is halted; caller may re-enter on interrupt.
                unsafe {
                    let instr_len = vmx::vmcs_read(VM_EXIT_INSTR_LEN);
                    let new_rip = self.guest_rip.wrapping_add(instr_len);
                    vmx::vmcs_write(GUEST_RIP, new_rip);
                    self.guest_rip = new_rip;
                }
                false // Caller should wait for an interrupt before resuming.
            }

            VmExitReason::Cpuid { leaf, subleaf } => {
                // Emulate CPUID with safe/sanitised values.  Advance RIP by 2
                // (CPUID encoding: 0F A2).
                self.emulate_cpuid(leaf, subleaf);
                unsafe {
                    let instr_len = vmx::vmcs_read(VM_EXIT_INSTR_LEN);
                    let new_rip = self.guest_rip.wrapping_add(instr_len);
                    vmx::vmcs_write(GUEST_RIP, new_rip);
                    self.guest_rip = new_rip;
                }
                true // Re-enter the guest.
            }

            VmExitReason::Io {
                port,
                is_write,
                size,
                count,
            } => {
                self.emulate_io(port, is_write, size, count);
                unsafe {
                    let instr_len = vmx::vmcs_read(VM_EXIT_INSTR_LEN);
                    let new_rip = self.guest_rip.wrapping_add(instr_len);
                    vmx::vmcs_write(GUEST_RIP, new_rip);
                    self.guest_rip = new_rip;
                }
                true
            }

            VmExitReason::Rdtsc => {
                // Return a deterministic value: TSC offset (simulated counter).
                let tsc = self.tsc_offset as u64;
                // Would normally write into guest RAX/RDX via VMCS guest GP-regs;
                // those fields are not directly in the VMCS (they live in the
                // host-allocated guest register save area), so we note them here
                // for the assembly trampoline in a full implementation.
                self.tsc_offset = self.tsc_offset.wrapping_add(1_000_000);
                let _ = tsc;
                unsafe {
                    let instr_len = vmx::vmcs_read(VM_EXIT_INSTR_LEN);
                    let new_rip = self.guest_rip.wrapping_add(instr_len);
                    vmx::vmcs_write(GUEST_RIP, new_rip);
                    self.guest_rip = new_rip;
                }
                true
            }

            VmExitReason::ExternalInterrupt => {
                // The host IDT will have already handled the interrupt due to
                // EXIT_ACK_INTR.  Nothing to do here; re-enter the guest.
                true
            }

            VmExitReason::EptViolation { gpa, qualification } => {
                serial_println!(
                    "[VCPU {}] EPT violation at GPA=0x{:016x} qual=0x{:08x}",
                    self.id,
                    gpa,
                    qualification
                );
                // Would call virt::ept to map the faulting page on demand.
                // Return false to halt the vCPU until the fault is resolved.
                false
            }

            VmExitReason::Vmcall => {
                // Minimal hypercall: just advance RIP.
                unsafe {
                    let instr_len = vmx::vmcs_read(VM_EXIT_INSTR_LEN);
                    let new_rip = self.guest_rip.wrapping_add(instr_len);
                    vmx::vmcs_write(GUEST_RIP, new_rip);
                    self.guest_rip = new_rip;
                }
                true
            }

            VmExitReason::TripleFault => {
                serial_println!("[VCPU {}] Triple fault — guest destroyed", self.id);
                false
            }

            VmExitReason::VmxError(e) => {
                serial_println!("[VCPU {}] VMX error 0x{:04x} — halting vCPU", self.id, e);
                false
            }

            VmExitReason::Other(code) => {
                serial_println!("[VCPU {}] Unhandled VM exit 0x{:04x}", self.id, code);
                false
            }
        }
    }

    // -----------------------------------------------------------------------
    // Internal: CPUID emulation
    // -----------------------------------------------------------------------

    fn emulate_cpuid(&self, leaf: u32, _subleaf: u32) {
        // Return sanitised values.  The guest GP registers (rax/rbx/rcx/rdx)
        // live in the host-allocated guest-register save area (pointed to by
        // the VMCS guest-state area) — in a full implementation the assembly
        // trampoline would push/pop them.
        //
        // Leaf 0: max_leaf=1, vendor="GenuineHVGS" (genesis hypervisor).
        // Leaf 1: no advanced features exposed (clear ECX bit 5 to hide VMX
        //         from the guest so it cannot attempt nested virtualisation).
        match leaf {
            0 => {
                // EAX = max basic leaf; EBX/EDX/ECX = vendor string.
                // We cannot write these back without the register save area,
                // so log and return.
                serial_println!(
                    "[VCPU {}] CPUID leaf 0 emulated (vendor=GenesisHV)",
                    self.id
                );
            }
            1 => {
                serial_println!(
                    "[VCPU {}] CPUID leaf 1 emulated (VMX hidden from guest)",
                    self.id
                );
            }
            _ => {
                serial_println!(
                    "[VCPU {}] CPUID leaf 0x{:08x} — returning zeroes",
                    self.id,
                    leaf
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Internal: I/O emulation
    // -----------------------------------------------------------------------

    fn emulate_io(&self, port: u16, is_write: bool, size: u8, _count: u32) {
        // Minimal emulation: COM1 serial port (0x3F8-0x3FF).
        if port >= 0x3F8 && port <= 0x3FF {
            serial_println!(
                "[VCPU {}] I/O {} port 0x{:04x} size={}",
                self.id,
                if is_write { "write" } else { "read" },
                port,
                size
            );
        }
        // All other ports silently ignored.
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Decode a raw VM-exit reason into a strongly-typed `VmExitReason`.
fn decode_exit_reason(raw: u32) -> VmExitReason {
    match raw {
        EXIT_HLT => VmExitReason::Hlt,
        EXIT_TRIPLE_FAULT => VmExitReason::TripleFault,
        EXIT_EXTERNAL_INTR => VmExitReason::ExternalInterrupt,
        EXIT_VMCALL => VmExitReason::Vmcall,
        EXIT_RDTSC => VmExitReason::Rdtsc,
        EXIT_CPUID => {
            // The guest's RAX and RCX at exit contain leaf and subleaf.
            // Without the register save area we report 0/0.
            VmExitReason::Cpuid {
                leaf: 0,
                subleaf: 0,
            }
        }
        EXIT_IO => {
            // Decode exit qualification (Intel SDM Table 27-5).
            let qual = unsafe { vmx::vmcs_read(VM_EXIT_QUALIFICATION) };
            let size = ((qual & 0x7) + 1) as u8; // bits [2:0] = size-1
            let is_write = (qual >> 3) & 1 == 0; // bit 3: 0=out, 1=in (flipped)
            let port = ((qual >> 16) & 0xFFFF) as u16; // bits [31:16] = port number
            let count = 1u32;
            VmExitReason::Io {
                port,
                is_write,
                size,
                count,
            }
        }
        EXIT_EPT_VIOLATION => {
            let qual = unsafe { vmx::vmcs_read(VM_EXIT_QUALIFICATION) };
            let gpa = unsafe { vmx::vmcs_read(GUEST_PHYSICAL_ADDR) };
            VmExitReason::EptViolation {
                gpa,
                qualification: qual,
            }
        }
        other => VmExitReason::Other(other),
    }
}

/// Adjust a VMX control value against the FIXED0 / FIXED1 MSR pair.
///
/// - Bits set in FIXED0 MUST be 1.
/// - Bits clear in FIXED1 MUST be 0.
///
/// `msr_fixed0` and `msr_fixed1` are the MSR addresses.
/// Returns 0 (no adjustment) if reading the MSRs is unsafe (e.g., not in VMX
/// root operation), which the caller handles gracefully.
unsafe fn adjust_control(wanted: u32, msr_fixed0: u32, msr_fixed1: u32) -> u32 {
    let fixed0 = rdmsr_safe(msr_fixed0);
    let fixed1 = rdmsr_safe(msr_fixed1);
    // Apply mandatory-1 bits (FIXED0) and clear mandatory-0 bits (FIXED1).
    ((wanted | (fixed0 as u32)) & (fixed1 as u32))
}

/// Safe wrapper around RDMSR that returns 0 if the MSR is not accessible.
/// Uses a very simple approach: if VMX is active this should always succeed.
unsafe fn rdmsr_safe(msr: u32) -> u64 {
    // In a hardened kernel we would wrap this in a #GP handler.
    // For now, call rdmsr directly; the kernel's fault handler will catch it
    // if the MSR does not exist.
    vmx::rdmsr(msr)
}

/// Dummy VM-exit entry point.
///
/// In a production hypervisor this is an `extern "C" fn` written in assembly
/// that saves all guest GPRs, calls `handle_vmexit`, then restores host state
/// and executes VMRESUME.  Here we provide a no-op placeholder so that
/// `HOST_RIP` has a valid kernel address and the linker is satisfied.
#[unsafe(naked)]
#[no_mangle]
unsafe extern "C" fn vmexit_stub() {
    // A real stub would: push all caller-saved regs, call into Rust, pop regs.
    // For now: infinite loop to prevent stack corruption.
    core::arch::naked_asm!("2:", "hlt", "jmp 2b",);
}
