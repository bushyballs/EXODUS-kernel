use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec;
/// VM Support — KVM-style hardware virtualization for Genesis
///
/// Provides VMX/VT-x support detection, VMCS field management,
/// virtual CPU abstraction, EPT/nested paging stubs, VM entry/exit
/// handling, and virtual device model stubs (serial, clock, PIC).
///
/// Inspired by: Linux KVM, Xen, bhyve. All code is original.
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// VMX support detection
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub struct VmxSupport {
    pub vmx_available: bool,
    pub ept_supported: bool,
    pub unrestricted_guest: bool,
    pub vpid_supported: bool,
    pub posted_interrupts: bool,
    pub vmfunc_supported: bool,
    pub secondary_controls: bool,
}

// ---------------------------------------------------------------------------
// VM state
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VmState {
    Off,
    Running,
    Paused,
    Suspended,
    Crashed,
}

// ---------------------------------------------------------------------------
// VMCS fields — Intel VMCS encoding constants
// ---------------------------------------------------------------------------

/// VMCS field encoding categories
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmcsFieldType {
    Control16,
    Control32,
    Control64,
    ControlNatural,
    GuestState16,
    GuestState32,
    GuestState64,
    GuestStateNatural,
    HostState16,
    HostState32,
    HostState64,
    HostStateNatural,
    ReadOnly32,
    ReadOnly64,
    ReadOnlyNatural,
}

/// VMCS field encoding — stores (field_type, index, value)
#[derive(Clone, Copy)]
pub struct VmcsField {
    pub encoding: u32,
    pub value: u64,
}

/// VMCS region for a virtual CPU
pub struct Vmcs {
    /// Pin-based VM-execution controls
    pub pin_based_controls: u32,
    /// Primary processor-based VM-execution controls
    pub proc_based_controls: u32,
    /// Secondary processor-based VM-execution controls
    pub proc_based_controls2: u32,
    /// VM-exit controls
    pub exit_controls: u32,
    /// VM-entry controls
    pub entry_controls: u32,
    /// Exception bitmap (intercept specific exceptions)
    pub exception_bitmap: u32,
    /// I/O bitmap address (for intercepting I/O ports)
    pub io_bitmap_a: u64,
    pub io_bitmap_b: u64,
    /// MSR bitmap address
    pub msr_bitmap: u64,
    /// EPT pointer
    pub eptp: u64,
    /// VPID (Virtual Processor ID)
    pub vpid: u16,
    /// Guest state
    pub guest_cr0: u64,
    pub guest_cr3: u64,
    pub guest_cr4: u64,
    pub guest_rip: u64,
    pub guest_rsp: u64,
    pub guest_rflags: u64,
    pub guest_cs: u64,
    pub guest_ds: u64,
    pub guest_es: u64,
    pub guest_ss: u64,
    pub guest_fs: u64,
    pub guest_gs: u64,
    pub guest_ldtr: u64,
    pub guest_tr: u64,
    pub guest_gdtr_base: u64,
    pub guest_gdtr_limit: u32,
    pub guest_idtr_base: u64,
    pub guest_idtr_limit: u32,
    pub guest_ia32_efer: u64,
    /// Host state (restored on VM exit)
    pub host_cr0: u64,
    pub host_cr3: u64,
    pub host_cr4: u64,
    pub host_rip: u64,
    pub host_rsp: u64,
    pub host_ia32_efer: u64,
    /// Read-only data (populated by hardware on VM exit)
    pub exit_reason: u32,
    pub exit_qualification: u64,
    pub guest_linear_address: u64,
    pub guest_physical_address: u64,
    pub vm_instruction_error: u32,
}

impl Vmcs {
    fn new() -> Self {
        Vmcs {
            pin_based_controls: 0,
            proc_based_controls: 0,
            proc_based_controls2: 0,
            exit_controls: 0,
            entry_controls: 0,
            exception_bitmap: 0,
            io_bitmap_a: 0,
            io_bitmap_b: 0,
            msr_bitmap: 0,
            eptp: 0,
            vpid: 0,
            guest_cr0: 0x00000010, // PE=0, ET=1 (real mode)
            guest_cr3: 0,
            guest_cr4: 0,
            guest_rip: 0xFFF0, // x86 reset vector
            guest_rsp: 0,
            guest_rflags: 0x00000002, // Reserved bit 1 always set
            guest_cs: 0xF000,
            guest_ds: 0,
            guest_es: 0,
            guest_ss: 0,
            guest_fs: 0,
            guest_gs: 0,
            guest_ldtr: 0,
            guest_tr: 0,
            guest_gdtr_base: 0,
            guest_gdtr_limit: 0xFFFF,
            guest_idtr_base: 0,
            guest_idtr_limit: 0xFFFF,
            guest_ia32_efer: 0,
            host_cr0: 0,
            host_cr3: 0,
            host_cr4: 0,
            host_rip: 0,
            host_rsp: 0,
            host_ia32_efer: 0,
            exit_reason: 0,
            exit_qualification: 0,
            guest_linear_address: 0,
            guest_physical_address: 0,
            vm_instruction_error: 0,
        }
    }

    /// Set up initial VMCS for protected mode guest
    pub fn setup_protected_mode(&mut self) {
        self.guest_cr0 = 0x00000011; // PE=1, ET=1
        self.guest_cr4 = 0;
        self.guest_rip = 0;
        self.guest_rsp = 0;
        self.guest_rflags = 0x00000002;
    }

    /// Set up initial VMCS for long mode (64-bit) guest
    pub fn setup_long_mode(&mut self) {
        self.guest_cr0 = 0x80000011; // PE=1, PG=1, ET=1
        self.guest_cr4 = 0x00000020; // PAE=1
        self.guest_ia32_efer = 0x00000D00; // LME=1, LMA=1, SCE=1
        self.guest_rip = 0;
        self.guest_rsp = 0;
        self.guest_rflags = 0x00000002;
    }
}

// ---------------------------------------------------------------------------
// VM exit reasons (Intel SDM Table C-1)
// ---------------------------------------------------------------------------

/// VM-exit reason codes
pub mod exit_reasons {
    pub const EXCEPTION_NMI: u32 = 0;
    pub const EXTERNAL_INTERRUPT: u32 = 1;
    pub const TRIPLE_FAULT: u32 = 2;
    pub const INIT_SIGNAL: u32 = 3;
    pub const SIPI: u32 = 4;
    pub const IO_SMI: u32 = 5;
    pub const OTHER_SMI: u32 = 6;
    pub const INTERRUPT_WINDOW: u32 = 7;
    pub const NMI_WINDOW: u32 = 8;
    pub const TASK_SWITCH: u32 = 9;
    pub const CPUID: u32 = 10;
    pub const GETSEC: u32 = 11;
    pub const HLT: u32 = 12;
    pub const INVD: u32 = 13;
    pub const INVLPG: u32 = 14;
    pub const RDPMC: u32 = 15;
    pub const RDTSC: u32 = 16;
    pub const RSM: u32 = 17;
    pub const VMCALL: u32 = 18;
    pub const VMCLEAR: u32 = 19;
    pub const VMLAUNCH: u32 = 20;
    pub const VMPTRLD: u32 = 21;
    pub const VMPTRST: u32 = 22;
    pub const VMREAD: u32 = 23;
    pub const VMRESUME: u32 = 24;
    pub const VMWRITE: u32 = 25;
    pub const VMXOFF: u32 = 26;
    pub const VMXON: u32 = 27;
    pub const CR_ACCESS: u32 = 28;
    pub const MOV_DR: u32 = 29;
    pub const IO_INSTRUCTION: u32 = 30;
    pub const RDMSR: u32 = 31;
    pub const WRMSR: u32 = 32;
    pub const ENTRY_FAIL_GUEST: u32 = 33;
    pub const ENTRY_FAIL_MSR: u32 = 34;
    pub const MWAIT: u32 = 36;
    pub const MONITOR_TRAP: u32 = 37;
    pub const MONITOR: u32 = 39;
    pub const PAUSE: u32 = 40;
    pub const ENTRY_FAIL_MC: u32 = 41;
    pub const TPR_BELOW: u32 = 43;
    pub const APIC_ACCESS: u32 = 44;
    pub const VIRTUALIZED_EOI: u32 = 45;
    pub const GDTR_IDTR: u32 = 46;
    pub const LDTR_TR: u32 = 47;
    pub const EPT_VIOLATION: u32 = 48;
    pub const EPT_MISCONFIG: u32 = 49;
    pub const INVEPT: u32 = 50;
    pub const RDTSCP: u32 = 51;
    pub const PREEMPTION_TIMER: u32 = 52;
    pub const INVVPID: u32 = 53;
    pub const WBINVD: u32 = 54;
    pub const XSETBV: u32 = 55;
    pub const APIC_WRITE: u32 = 56;
    pub const RDRAND: u32 = 57;
    pub const INVPCID: u32 = 58;
    pub const VMFUNC: u32 = 59;
    pub const ENCLS: u32 = 60;
    pub const RDSEED: u32 = 61;
    pub const PML_FULL: u32 = 62;
    pub const XSAVES: u32 = 63;
    pub const XRSTORS: u32 = 64;
}

// ---------------------------------------------------------------------------
// Virtual CPU abstraction
// ---------------------------------------------------------------------------

/// General-purpose register set for a virtual CPU
#[derive(Clone, Copy)]
pub struct VcpuRegs {
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub rsp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rip: u64,
    pub rflags: u64,
}

impl VcpuRegs {
    const fn zeroed() -> Self {
        VcpuRegs {
            rax: 0,
            rbx: 0,
            rcx: 0,
            rdx: 0,
            rsi: 0,
            rdi: 0,
            rbp: 0,
            rsp: 0,
            r8: 0,
            r9: 0,
            r10: 0,
            r11: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
            rip: 0xFFF0,
            rflags: 0x0002,
        }
    }
}

/// Virtual CPU state
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VcpuState {
    Idle,
    Running,
    HaltedOnHlt,
    WaitingForInterrupt,
    InVmExit,
}

/// A virtual CPU instance
pub struct Vcpu {
    pub id: u32,
    pub vm_id: u32,
    pub state: VcpuState,
    pub regs: VcpuRegs,
    pub vmcs: Vmcs,
    pub exit_count: u64,
    pub instruction_count: u64,
    pub interrupt_pending: bool,
    pub nmi_pending: bool,
    pub apic_id: u32,
}

impl Vcpu {
    fn new(id: u32, vm_id: u32) -> Self {
        Vcpu {
            id,
            vm_id,
            state: VcpuState::Idle,
            regs: VcpuRegs::zeroed(),
            vmcs: Vmcs::new(),
            exit_count: 0,
            instruction_count: 0,
            interrupt_pending: false,
            nmi_pending: false,
            apic_id: id,
        }
    }

    /// Inject an external interrupt into the vCPU
    pub fn inject_interrupt(&mut self, vector: u8) {
        self.interrupt_pending = true;
        // In real VMX, we would set VM-entry interruption-information field
        // Type=0 (external interrupt), vector=vector, valid=1
        let _entry_interrupt_info: u32 = (1 << 31) | (vector as u32);
    }

    /// Inject an NMI
    pub fn inject_nmi(&mut self) {
        self.nmi_pending = true;
    }
}

// ---------------------------------------------------------------------------
// EPT (Extended Page Tables) / Nested Paging
// ---------------------------------------------------------------------------

/// EPT permission flags
pub const EPT_READ: u64 = 1 << 0;
pub const EPT_WRITE: u64 = 1 << 1;
pub const EPT_EXEC: u64 = 1 << 2;
pub const EPT_MEMORY_TYPE_WB: u64 = 6 << 3;
pub const EPT_MEMORY_TYPE_UC: u64 = 0;

/// An EPT page table entry
#[derive(Clone, Copy)]
pub struct EptEntry {
    pub raw: u64,
}

impl EptEntry {
    pub const fn empty() -> Self {
        EptEntry { raw: 0 }
    }
    pub fn new(phys_addr: u64, flags: u64) -> Self {
        EptEntry {
            raw: (phys_addr & 0x000FFFFFFFFFF000) | flags,
        }
    }
    pub fn is_present(&self) -> bool {
        self.raw & (EPT_READ | EPT_WRITE | EPT_EXEC) != 0
    }
    pub fn address(&self) -> u64 {
        self.raw & 0x000FFFFFFFFFF000
    }
}

/// EPT page table (stub — would manage 4-level paging for guest physical memory)
pub struct EptManager {
    pub enabled: bool,
    pub eptp: u64,
    /// Total mapped guest memory in pages
    pub mapped_pages: u64,
    /// EPT violation count
    pub violation_count: u64,
    /// EPT misconfig count
    pub misconfig_count: u64,
}

impl EptManager {
    fn new() -> Self {
        EptManager {
            enabled: false,
            eptp: 0,
            mapped_pages: 0,
            violation_count: 0,
            misconfig_count: 0,
        }
    }

    /// Map a guest physical address to host physical address
    /// Stub: would walk/create EPT page tables
    pub fn map_page(
        &mut self,
        guest_phys: u64,
        host_phys: u64,
        flags: u64,
    ) -> Result<(), &'static str> {
        if !self.enabled {
            return Err("EPT not enabled");
        }
        let _ = (guest_phys, host_phys, flags);
        self.mapped_pages = self.mapped_pages.saturating_add(1);
        Ok(())
    }

    /// Unmap a guest physical page
    pub fn unmap_page(&mut self, guest_phys: u64) -> Result<(), &'static str> {
        if !self.enabled {
            return Err("EPT not enabled");
        }
        let _ = guest_phys;
        if self.mapped_pages > 0 {
            self.mapped_pages -= 1;
        }
        Ok(())
    }

    /// Handle an EPT violation (guest accessed unmapped memory)
    pub fn handle_violation(
        &mut self,
        guest_phys: u64,
        _qualification: u64,
    ) -> Result<(), &'static str> {
        self.violation_count = self.violation_count.saturating_add(1);
        let _ = guest_phys;
        // Would decide: map on demand, inject #PF, or crash VM
        Err("EPT violation unhandled")
    }
}

// ---------------------------------------------------------------------------
// Virtual devices
// ---------------------------------------------------------------------------

/// Virtual serial port (UART 16550 emulation)
pub struct VirtualSerial {
    pub base_port: u16, // I/O port base (e.g. 0x3F8)
    pub irq: u8,        // IRQ line (e.g. 4)
    tx_buf: Vec<u8>,    // Transmit buffer
    rx_buf: Vec<u8>,    // Receive buffer
    pub ier: u8,        // Interrupt Enable Register
    pub iir: u8,        // Interrupt Identification Register
    pub lcr: u8,        // Line Control Register
    pub mcr: u8,        // Modem Control Register
    pub lsr: u8,        // Line Status Register
    pub msr: u8,        // Modem Status Register
    pub scratch: u8,    // Scratch register
    pub divisor: u16,   // Baud rate divisor
    pub dlab: bool,     // Divisor Latch Access Bit
}

impl VirtualSerial {
    pub fn new(base_port: u16, irq: u8) -> Self {
        VirtualSerial {
            base_port,
            irq,
            tx_buf: Vec::new(),
            rx_buf: Vec::new(),
            ier: 0,
            iir: 0x01,
            lcr: 0,
            mcr: 0,
            lsr: 0x60, // Transmitter Holding Register Empty + Transmitter Empty
            msr: 0,
            scratch: 0,
            divisor: 12,
            dlab: false,
        }
    }

    /// Handle a port I/O read from the guest
    pub fn read_port(&mut self, offset: u16) -> u8 {
        match offset {
            0 => {
                if self.dlab {
                    (self.divisor & 0xFF) as u8
                } else {
                    // Read from receive buffer
                    if let Some(byte) = self.rx_buf.first().copied() {
                        self.rx_buf.remove(0);
                        if self.rx_buf.is_empty() {
                            self.lsr &= !0x01; // Clear Data Ready
                        }
                        byte
                    } else {
                        0
                    }
                }
            }
            1 => {
                if self.dlab {
                    ((self.divisor >> 8) & 0xFF) as u8
                } else {
                    self.ier
                }
            }
            2 => self.iir,
            3 => self.lcr,
            4 => self.mcr,
            5 => self.lsr,
            6 => self.msr,
            7 => self.scratch,
            _ => 0,
        }
    }

    /// Handle a port I/O write from the guest
    pub fn write_port(&mut self, offset: u16, value: u8) {
        match offset {
            0 => {
                if self.dlab {
                    self.divisor = (self.divisor & 0xFF00) | value as u16;
                } else {
                    // Transmit byte
                    self.tx_buf.push(value);
                    self.lsr |= 0x60; // THR empty + transmitter empty
                }
            }
            1 => {
                if self.dlab {
                    self.divisor = (self.divisor & 0x00FF) | ((value as u16) << 8);
                } else {
                    self.ier = value & 0x0F;
                }
            }
            3 => {
                self.dlab = (value & 0x80) != 0;
                self.lcr = value;
            }
            4 => self.mcr = value,
            7 => self.scratch = value,
            _ => {}
        }
    }

    /// Feed a byte into the receive buffer (from host)
    pub fn receive(&mut self, byte: u8) {
        self.rx_buf.push(byte);
        self.lsr |= 0x01; // Data Ready
    }

    /// Drain the transmit buffer
    pub fn drain_tx(&mut self) -> Vec<u8> {
        let data = self.tx_buf.clone();
        self.tx_buf.clear();
        data
    }
}

/// Virtual clock (PIT 8254 emulation + TSC)
pub struct VirtualClock {
    /// PIT channel 0 counter
    pub pit_counter: u16,
    /// PIT channel 0 reload value
    pub pit_reload: u16,
    /// PIT mode
    pub pit_mode: u8,
    /// IRQ line for PIT (usually IRQ 0)
    pub pit_irq: u8,
    /// TSC offset (added to host TSC for guest reads)
    pub tsc_offset: u64,
    /// Whether the guest should see a fixed TSC frequency
    pub tsc_frequency_khz: u64,
    /// CMOS/RTC time (seconds since epoch)
    pub rtc_time: u64,
    /// Number of PIT ticks since boot
    pub pit_ticks: u64,
}

impl VirtualClock {
    pub fn new() -> Self {
        VirtualClock {
            pit_counter: 0,
            pit_reload: 0xFFFF,
            pit_mode: 3, // Mode 3: square wave
            pit_irq: 0,
            tsc_offset: 0,
            tsc_frequency_khz: 3000000, // 3 GHz (virtual)
            rtc_time: 0,
            pit_ticks: 0,
        }
    }

    /// Handle PIT port I/O write
    pub fn write_pit(&mut self, port: u16, value: u8) {
        match port {
            0x40 => {
                // Channel 0 data
                self.pit_reload = value as u16; // Simplified: only low byte
            }
            0x43 => {
                // Mode/Command register
                let channel = (value >> 6) & 0x03;
                let _access = (value >> 4) & 0x03;
                let mode = (value >> 1) & 0x07;
                if channel == 0 {
                    self.pit_mode = mode;
                }
            }
            _ => {}
        }
    }

    /// Handle PIT port I/O read
    pub fn read_pit(&self, port: u16) -> u8 {
        match port {
            0x40 => (self.pit_counter & 0xFF) as u8,
            _ => 0,
        }
    }

    /// Advance the PIT by one tick
    pub fn tick(&mut self) -> bool {
        self.pit_ticks = self.pit_ticks.saturating_add(1);
        if self.pit_counter == 0 {
            self.pit_counter = self.pit_reload;
            true // Fire IRQ
        } else {
            self.pit_counter -= 1;
            false
        }
    }

    /// Get guest TSC value
    pub fn guest_tsc(&self) -> u64 {
        // In real implementation: read host TSC + offset
        self.tsc_offset + self.pit_ticks * 1000
    }
}

/// Virtual PIC (8259 emulation — simplified)
pub struct VirtualPic {
    pub irr: u8,         // Interrupt Request Register
    pub isr: u8,         // In-Service Register
    pub imr: u8,         // Interrupt Mask Register
    pub vector_base: u8, // Base interrupt vector (usually 0x20 for master, 0x28 for slave)
    pub init_state: u8,  // ICW state machine
}

impl VirtualPic {
    pub fn new(vector_base: u8) -> Self {
        VirtualPic {
            irr: 0,
            isr: 0,
            imr: 0xFF, // All masked initially
            vector_base,
            init_state: 0,
        }
    }

    /// Raise an IRQ line
    pub fn raise_irq(&mut self, irq: u8) {
        if irq < 8 {
            self.irr |= 1 << irq;
        }
    }

    /// Lower an IRQ line
    pub fn lower_irq(&mut self, irq: u8) {
        if irq < 8 {
            self.irr &= !(1 << irq);
        }
    }

    /// Get the highest priority pending interrupt vector (if any)
    pub fn get_pending_vector(&self) -> Option<u8> {
        let pending = self.irr & !self.imr & !self.isr;
        if pending == 0 {
            return None;
        }
        // Find lowest set bit
        for i in 0..8u8 {
            if pending & (1 << i) != 0 {
                return Some(self.vector_base + i);
            }
        }
        None
    }

    /// Acknowledge interrupt (move from IRR to ISR)
    pub fn ack(&mut self, vector: u8) {
        let irq = vector.wrapping_sub(self.vector_base);
        if irq < 8 {
            self.irr &= !(1 << irq);
            self.isr |= 1 << irq;
        }
    }

    /// End of interrupt
    pub fn eoi(&mut self) {
        // Clear highest priority in-service bit
        for i in 0..8u8 {
            if self.isr & (1 << i) != 0 {
                self.isr &= !(1 << i);
                break;
            }
        }
    }

    /// Handle port I/O write
    pub fn write_port(&mut self, port: u16, value: u8) {
        match port & 1 {
            0 => {
                if value & 0x10 != 0 {
                    // ICW1
                    self.init_state = 1;
                    self.imr = 0;
                    self.isr = 0;
                } else if value & 0x20 != 0 {
                    // OCW2: EOI
                    self.eoi();
                }
            }
            1 => {
                match self.init_state {
                    1 => {
                        // ICW2: vector base
                        self.vector_base = value;
                        self.init_state = 2;
                    }
                    2 => {
                        // ICW3: cascade
                        self.init_state = 3;
                    }
                    3 => {
                        // ICW4: mode
                        self.init_state = 0;
                    }
                    _ => {
                        // OCW1: interrupt mask
                        self.imr = value;
                    }
                }
            }
            _ => {}
        }
    }

    /// Handle port I/O read
    pub fn read_port(&self, port: u16) -> u8 {
        match port & 1 {
            0 => self.irr,
            1 => self.imr,
            _ => 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Virtual machine
// ---------------------------------------------------------------------------

pub struct VirtualMachine {
    pub id: u32,
    pub name: String,
    pub vcpus: Vec<Vcpu>,
    pub memory_mb: u32,
    pub state: VmState,
    pub ept: EptManager,
    pub serial: VirtualSerial,
    pub clock: VirtualClock,
    pub pic_master: VirtualPic,
    pub pic_slave: VirtualPic,
    pub exit_count: u64,
    pub total_instructions: u64,
}

impl VirtualMachine {
    fn new(id: u32, name: &str, num_vcpus: u8, memory_mb: u32) -> Self {
        let mut vcpus = Vec::new();
        for i in 0..num_vcpus as u32 {
            vcpus.push(Vcpu::new(i, id));
        }
        VirtualMachine {
            id,
            name: String::from(name),
            vcpus,
            memory_mb,
            state: VmState::Off,
            ept: EptManager::new(),
            serial: VirtualSerial::new(0x3F8, 4),
            clock: VirtualClock::new(),
            pic_master: VirtualPic::new(0x20),
            pic_slave: VirtualPic::new(0x28),
            exit_count: 0,
            total_instructions: 0,
        }
    }

    /// Handle an I/O port access from the guest
    pub fn handle_io(&mut self, port: u16, is_write: bool, value: u8) -> u8 {
        // Serial port (COM1: 0x3F8-0x3FF)
        if port >= 0x3F8 && port <= 0x3FF {
            let offset = port - 0x3F8;
            if is_write {
                self.serial.write_port(offset, value);
                return 0;
            } else {
                return self.serial.read_port(offset);
            }
        }

        // PIT (0x40-0x43)
        if port >= 0x40 && port <= 0x43 {
            if is_write {
                self.clock.write_pit(port, value);
                return 0;
            } else {
                return self.clock.read_pit(port);
            }
        }

        // PIC master (0x20-0x21)
        if port >= 0x20 && port <= 0x21 {
            if is_write {
                self.pic_master.write_port(port, value);
                return 0;
            } else {
                return self.pic_master.read_port(port);
            }
        }

        // PIC slave (0xA0-0xA1)
        if port >= 0xA0 && port <= 0xA1 {
            if is_write {
                self.pic_slave.write_port(port, value);
                return 0;
            } else {
                return self.pic_slave.read_port(port);
            }
        }

        0xFF // Default: all bits high for unhandled reads
    }
}

// ---------------------------------------------------------------------------
// VM manager
// ---------------------------------------------------------------------------

pub struct VmManager {
    vms: Vec<VirtualMachine>,
    vmx: VmxSupport,
    max_vms: u8,
    next_id: u32,
}

impl VmManager {
    fn new() -> Self {
        let vmx = Self::detect_vmx_support();
        Self {
            vms: Vec::new(),
            vmx,
            max_vms: 8,
            next_id: 1,
        }
    }

    fn detect_vmx_support() -> VmxSupport {
        // Stub: would use CPUID to detect VMX features
        VmxSupport {
            vmx_available: false, // Would check CPUID.1:ECX.VMX[bit 5]
            ept_supported: false,
            unrestricted_guest: false,
            vpid_supported: false,
            posted_interrupts: false,
            vmfunc_supported: false,
            secondary_controls: false,
        }
    }

    pub fn check_vmx_support(&self) -> VmxSupport {
        self.vmx
    }

    pub fn create_vm(
        &mut self,
        name: &str,
        vcpus: u8,
        memory_mb: u32,
    ) -> Result<u32, &'static str> {
        if self.vms.len() >= self.max_vms as usize {
            return Err("VM limit reached");
        }

        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        let vm = VirtualMachine::new(id, name, vcpus, memory_mb);
        self.vms.push(vm);
        Ok(id)
    }

    pub fn start_vm(&mut self, id: u32) -> Result<(), &'static str> {
        let vm = self
            .vms
            .iter_mut()
            .find(|v| v.id == id)
            .ok_or("VM not found")?;

        if vm.state != VmState::Off && vm.state != VmState::Suspended {
            return Err("VM not in startable state");
        }

        // Initialize vCPU states
        for vcpu in &mut vm.vcpus {
            vcpu.state = VcpuState::Running;
            vcpu.vmcs.setup_protected_mode();
        }

        vm.state = VmState::Running;
        vm.ept.enabled = true;
        Ok(())
    }

    pub fn stop_vm(&mut self, id: u32) -> Result<(), &'static str> {
        let vm = self
            .vms
            .iter_mut()
            .find(|v| v.id == id)
            .ok_or("VM not found")?;

        if vm.state == VmState::Off {
            return Err("VM already stopped");
        }

        for vcpu in &mut vm.vcpus {
            vcpu.state = VcpuState::Idle;
        }
        vm.state = VmState::Off;
        vm.exit_count = 0;
        Ok(())
    }

    pub fn pause_vm(&mut self, id: u32) -> Result<(), &'static str> {
        let vm = self
            .vms
            .iter_mut()
            .find(|v| v.id == id)
            .ok_or("VM not found")?;

        if vm.state != VmState::Running {
            return Err("VM not running");
        }
        vm.state = VmState::Paused;
        Ok(())
    }

    pub fn handle_vmexit(
        &mut self,
        id: u32,
        vcpu_id: u32,
        exit_reason: u32,
    ) -> Result<(), &'static str> {
        let vm = self
            .vms
            .iter_mut()
            .find(|v| v.id == id)
            .ok_or("VM not found")?;

        if vm.state != VmState::Running {
            return Err("VM not running");
        }

        vm.exit_count = vm.exit_count.saturating_add(1);

        let vcpu = vm
            .vcpus
            .iter_mut()
            .find(|v| v.id == vcpu_id)
            .ok_or("vCPU not found")?;

        vcpu.exit_count = vcpu.exit_count.saturating_add(1);
        vcpu.state = VcpuState::InVmExit;

        match exit_reason {
            exit_reasons::EXCEPTION_NMI => { /* Handle exception or NMI */ }
            exit_reasons::EXTERNAL_INTERRUPT => { /* Route to vPIC */ }
            exit_reasons::CPUID => {
                // Emulate CPUID: advance RIP past the instruction
                vcpu.regs.rip += 2; // CPUID is 2 bytes (0F A2)
            }
            exit_reasons::HLT => {
                vcpu.state = VcpuState::HaltedOnHlt;
            }
            exit_reasons::IO_INSTRUCTION => {
                // Would decode I/O instruction from exit qualification
                // and route to virtual device handlers
            }
            exit_reasons::EPT_VIOLATION => {
                // Handle EPT violation
                let _gpa = vcpu.vmcs.guest_physical_address;
                let _qual = vcpu.vmcs.exit_qualification;
            }
            exit_reasons::RDTSC => {
                // Return virtual TSC
                let tsc = vm.clock.guest_tsc();
                vcpu.regs.rax = tsc & 0xFFFFFFFF;
                vcpu.regs.rdx = tsc >> 32;
                vcpu.regs.rip += 2;
            }
            exit_reasons::RDMSR | exit_reasons::WRMSR => {
                // Emulate MSR access
                vcpu.regs.rip += 2;
            }
            _ => {
                vm.state = VmState::Crashed;
                return Err("unhandled VM exit");
            }
        }

        // Resume vCPU if not halted
        if vcpu.state == VcpuState::InVmExit {
            vcpu.state = VcpuState::Running;
        }

        Ok(())
    }

    pub fn get_vm_info(&self, id: u32) -> Result<(VmState, u8, u32, u64), &'static str> {
        let vm = self.vms.iter().find(|v| v.id == id).ok_or("VM not found")?;

        Ok((vm.state, vm.vcpus.len() as u8, vm.memory_mb, vm.exit_count))
    }

    pub fn running_count(&self) -> usize {
        self.vms
            .iter()
            .filter(|v| v.state == VmState::Running)
            .count()
    }

    pub fn total_count(&self) -> usize {
        self.vms.len()
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static VM_MGR: Mutex<Option<VmManager>> = Mutex::new(None);

pub fn init() {
    let mut mgr = VM_MGR.lock();
    let vm_mgr = VmManager::new();
    let vmx_available = vm_mgr.vmx.vmx_available;
    *mgr = Some(vm_mgr);

    if vmx_available {
        serial_println!("[VM] Hardware virtualization support detected (VMX/VT-x)");
    } else {
        serial_println!("[VM] Hardware virtualization NOT available (VMX not detected)");
    }
    serial_println!("[VM] VM manager initialized (VMCS, EPT, vCPU, vSerial, vClock, vPIC)");
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn check_vmx() -> VmxSupport {
    let mgr = VM_MGR.lock();
    match mgr.as_ref() {
        Some(manager) => manager.check_vmx_support(),
        None => VmxSupport {
            vmx_available: false,
            ept_supported: false,
            unrestricted_guest: false,
            vpid_supported: false,
            posted_interrupts: false,
            vmfunc_supported: false,
            secondary_controls: false,
        },
    }
}

pub fn create_vm(name: &str, vcpus: u8, memory_mb: u32) -> Result<u32, &'static str> {
    let mut mgr = VM_MGR.lock();
    mgr.as_mut()
        .ok_or("VM manager not initialized")?
        .create_vm(name, vcpus, memory_mb)
}

pub fn start_vm(id: u32) -> Result<(), &'static str> {
    let mut mgr = VM_MGR.lock();
    mgr.as_mut()
        .ok_or("VM manager not initialized")?
        .start_vm(id)
}

pub fn stop_vm(id: u32) -> Result<(), &'static str> {
    let mut mgr = VM_MGR.lock();
    mgr.as_mut()
        .ok_or("VM manager not initialized")?
        .stop_vm(id)
}

pub fn pause_vm(id: u32) -> Result<(), &'static str> {
    let mut mgr = VM_MGR.lock();
    mgr.as_mut()
        .ok_or("VM manager not initialized")?
        .pause_vm(id)
}

pub fn handle_vmexit(id: u32, vcpu_id: u32, exit_reason: u32) -> Result<(), &'static str> {
    let mut mgr = VM_MGR.lock();
    mgr.as_mut()
        .ok_or("VM manager not initialized")?
        .handle_vmexit(id, vcpu_id, exit_reason)
}

pub fn get_vm_info(id: u32) -> Result<(VmState, u8, u32, u64), &'static str> {
    let mgr = VM_MGR.lock();
    mgr.as_ref()
        .ok_or("VM manager not initialized")?
        .get_vm_info(id)
}
