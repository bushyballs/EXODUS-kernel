/// Guest VM management — create, destroy, and schedule guest VMs.
///
/// Part of the AIOS hypervisor subsystem.
///
/// Each guest VM has its own VMCS/VMCB, EPT root, vCPU register state,
/// virtual device list, and lifecycle state machine.

use alloc::vec::Vec;
use crate::{serial_print, serial_println};
use crate::sync::Mutex;

use super::vmcs::Vmcs;
use super::ept::EptRoot;

/// Maximum number of concurrent guest VMs.
const MAX_GUESTS: usize = 16;

/// Maximum number of vCPUs per guest.
const MAX_VCPUS_PER_GUEST: usize = 8;

/// Global guest VM pool.
static GUEST_POOL: Mutex<Option<GuestPool>> = Mutex::new(None);

/// Pool managing all active guest VMs.
struct GuestPool {
    /// All registered guests (sparse — entries may be None).
    guests: Vec<Option<GuestEntry>>,
    /// Next VM ID to assign.
    next_vm_id: u64,
    /// Number of currently active guests.
    active_count: usize,
}

/// Internal bookkeeping entry for a guest in the pool.
struct GuestEntry {
    vm_id: u64,
    state: GuestState,
}

/// Lifecycle state of a guest VM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestState {
    /// VM created but not yet booted.
    Created,
    /// VM is running guest code.
    Running,
    /// VM is paused (vCPU halted, state preserved).
    Paused,
    /// VM has been stopped and is awaiting destruction.
    Stopped,
}

/// Saved vCPU register state.
#[derive(Clone)]
pub struct VcpuState {
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
    pub cr0: u64,
    pub cr3: u64,
    pub cr4: u64,
}

impl VcpuState {
    fn new() -> Self {
        VcpuState {
            rax: 0, rbx: 0, rcx: 0, rdx: 0,
            rsi: 0, rdi: 0, rbp: 0, rsp: 0,
            r8: 0, r9: 0, r10: 0, r11: 0,
            r12: 0, r13: 0, r14: 0, r15: 0,
            rip: 0,
            rflags: 0x2, // Reserved bit 1.
            cr0: 0x0000_0021, // PE + NE
            cr3: 0,
            cr4: 0x2000, // VMXE
        }
    }
}

/// Represents a guest virtual machine.
pub struct GuestVm {
    /// Unique VM identifier.
    pub vm_id: u64,
    /// VMCS for the primary vCPU (Intel) or VMCB (AMD).
    vmcs: Vmcs,
    /// Extended Page Table root for this guest.
    ept_root: EptRoot,
    /// Saved vCPU states (one per virtual CPU).
    vcpus: Vec<VcpuState>,
    /// Current lifecycle state.
    state: GuestState,
    /// Guest physical memory size in bytes.
    memory_size: u64,
    /// TSC offset applied to the guest's RDTSC.
    tsc_offset: u64,
    /// Number of VM exits this guest has experienced.
    exit_count: u64,
    /// Whether the guest has been launched at least once.
    launched: bool,
}

impl GuestVm {
    pub fn new(vm_id: u64) -> Self {
        let mut vmcs = Vmcs::new();
        vmcs.clear();
        vmcs.activate();

        let ept_root = EptRoot::new();
        let mut vcpus = Vec::new();
        vcpus.push(VcpuState::new());

        // Read TSC for offset calculation.
        let tsc: u64;
        unsafe {
            core::arch::asm!("rdtsc", out("eax") _, out("edx") _, options(nomem, nostack));
            let lo: u32;
            let hi: u32;
            core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack));
            tsc = ((hi as u64) << 32) | (lo as u64);
        }

        serial_println!("    [guest] Created VM {} with 1 vCPU", vm_id);

        GuestVm {
            vm_id,
            vmcs,
            ept_root,
            vcpus,
            state: GuestState::Created,
            memory_size: 0,
            tsc_offset: tsc,
            exit_count: 0,
            launched: false,
        }
    }

    /// Allocate guest physical memory and set up identity-mapped EPT entries.
    pub fn setup_memory(&mut self, size_bytes: u64) {
        self.memory_size = size_bytes;

        // Create identity-mapped EPT entries for the guest's physical address space.
        // Use 2 MiB large pages where possible for efficiency.
        let two_mb = 2 * 1024 * 1024u64;
        let mut addr = 0u64;
        while addr < size_bytes {
            if size_bytes - addr >= two_mb && (addr & (two_mb - 1)) == 0 {
                self.ept_root.map_large_page(
                    addr,
                    addr,
                    super::ept::EPT_RWX,
                );
                addr += two_mb;
            } else {
                self.ept_root.map_page(
                    addr,
                    addr,
                    super::ept::EPT_RWX,
                );
                addr += 4096;
            }
        }

        // Point the VMCS at our EPT root.
        self.vmcs.set_ept_pointer(self.ept_root.pml4_physical_address());

        serial_println!(
            "    [guest] VM {} memory configured: {} bytes ({} MiB)",
            self.vm_id, size_bytes, size_bytes / (1024 * 1024)
        );
    }

    /// Add a vCPU to this guest (up to MAX_VCPUS_PER_GUEST).
    pub fn add_vcpu(&mut self) -> Option<usize> {
        if self.vcpus.len() >= MAX_VCPUS_PER_GUEST {
            serial_println!("    [guest] VM {} at maximum vCPU count ({})", self.vm_id, MAX_VCPUS_PER_GUEST);
            return None;
        }
        let idx = self.vcpus.len();
        self.vcpus.push(VcpuState::new());
        serial_println!("    [guest] VM {} added vCPU {}", self.vm_id, idx);
        Some(idx)
    }

    /// Boot the guest VM from a given entry point.
    pub fn boot(&mut self, entry_point: u64) {
        if self.state != GuestState::Created && self.state != GuestState::Stopped {
            serial_println!("    [guest] VM {} cannot boot from state {:?}", self.vm_id, self.state);
            return;
        }

        // Set the primary vCPU's RIP to the entry point.
        if let Some(vcpu) = self.vcpus.first_mut() {
            vcpu.rip = entry_point;
            vcpu.rsp = self.memory_size.saturating_sub(4096); // Stack at top of memory.
        }

        // Configure the VMCS guest state from vCPU 0.
        if let Some(vcpu) = self.vcpus.first() {
            self.vmcs.setup_guest_state(vcpu.rip, vcpu.rsp);
        }

        self.state = GuestState::Running;
        self.launched = false;

        // Register in the global pool.
        {
            let mut pool = GUEST_POOL.lock();
            if let Some(ref mut p) = *pool {
                // Find a free slot or push a new one.
                let mut found = false;
                for entry in p.guests.iter_mut() {
                    if entry.is_none() {
                        *entry = Some(GuestEntry {
                            vm_id: self.vm_id,
                            state: GuestState::Running,
                        });
                        found = true;
                        break;
                    }
                }
                if !found && p.guests.len() < MAX_GUESTS {
                    p.guests.push(Some(GuestEntry {
                        vm_id: self.vm_id,
                        state: GuestState::Running,
                    }));
                }
                p.active_count = p.active_count.saturating_add(1);
            }
        }

        serial_println!(
            "    [guest] VM {} booted at entry point 0x{:016x}",
            self.vm_id, entry_point
        );
    }

    /// Pause guest execution.
    pub fn pause(&mut self) {
        if self.state != GuestState::Running {
            serial_println!("    [guest] VM {} not running, cannot pause (state={:?})", self.vm_id, self.state);
            return;
        }

        // Save the current vCPU state from the VMCS.
        if let Some(vcpu) = self.vcpus.first_mut() {
            vcpu.rip = self.vmcs.read_field(super::vmcs::GUEST_RIP);
            vcpu.rsp = self.vmcs.read_field(super::vmcs::GUEST_RSP);
            vcpu.rflags = self.vmcs.read_field(super::vmcs::GUEST_RFLAGS);
            vcpu.cr0 = self.vmcs.read_field(super::vmcs::GUEST_CR0);
            vcpu.cr3 = self.vmcs.read_field(super::vmcs::GUEST_CR3);
            vcpu.cr4 = self.vmcs.read_field(super::vmcs::GUEST_CR4);
        }

        self.state = GuestState::Paused;

        // Update pool state.
        {
            let mut pool = GUEST_POOL.lock();
            if let Some(ref mut p) = *pool {
                for entry in p.guests.iter_mut().flatten() {
                    if entry.vm_id == self.vm_id {
                        entry.state = GuestState::Paused;
                        break;
                    }
                }
            }
        }

        serial_println!("    [guest] VM {} paused", self.vm_id);
    }

    /// Resume a paused guest.
    pub fn resume(&mut self) {
        if self.state != GuestState::Paused {
            serial_println!("    [guest] VM {} not paused, cannot resume (state={:?})", self.vm_id, self.state);
            return;
        }

        // Restore vCPU state into the VMCS.
        if let Some(vcpu) = self.vcpus.first() {
            self.vmcs.setup_guest_state(vcpu.rip, vcpu.rsp);
        }

        self.state = GuestState::Running;

        // Update pool state.
        {
            let mut pool = GUEST_POOL.lock();
            if let Some(ref mut p) = *pool {
                for entry in p.guests.iter_mut().flatten() {
                    if entry.vm_id == self.vm_id {
                        entry.state = GuestState::Running;
                        break;
                    }
                }
            }
        }

        serial_println!("    [guest] VM {} resumed", self.vm_id);
    }

    /// Destroy the guest and release resources.
    pub fn destroy(mut self) {
        serial_println!("    [guest] Destroying VM {}", self.vm_id);

        // Clear the VMCS.
        self.vmcs.clear();

        // Remove from global pool.
        {
            let mut pool = GUEST_POOL.lock();
            if let Some(ref mut p) = *pool {
                for entry in p.guests.iter_mut() {
                    if let Some(ref e) = entry {
                        if e.vm_id == self.vm_id {
                            *entry = None;
                            if p.active_count > 0 {
                                p.active_count = p.active_count.saturating_sub(1);
                            }
                            break;
                        }
                    }
                }
            }
        }

        // vCPU states, EPT pages, etc. are dropped automatically.
        serial_println!("    [guest] VM {} destroyed ({} exits total)", self.vm_id, self.exit_count);
    }

    /// Record a VM exit for statistics.
    pub fn record_exit(&mut self) {
        self.exit_count = self.exit_count.saturating_add(1);
    }

    /// Get the current lifecycle state.
    pub fn state(&self) -> GuestState {
        self.state
    }
}

/// Allocate a new unique VM ID.
pub fn allocate_vm_id() -> u64 {
    let mut pool = GUEST_POOL.lock();
    if let Some(ref mut p) = *pool {
        let id = p.next_vm_id;
        p.next_vm_id = p.next_vm_id.saturating_add(1);
        id
    } else {
        0
    }
}

/// Get the number of currently active guest VMs.
pub fn active_guest_count() -> usize {
    let pool = GUEST_POOL.lock();
    if let Some(ref p) = *pool {
        p.active_count
    } else {
        0
    }
}

pub fn init() {
    let pool = GuestPool {
        guests: Vec::new(),
        next_vm_id: 1,
        active_count: 0,
    };

    *GUEST_POOL.lock() = Some(pool);
    serial_println!("    [guest] Guest VM pool initialized (max {} VMs)", MAX_GUESTS);
}
