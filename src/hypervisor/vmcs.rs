/// Virtual Machine Control Structure management.
///
/// Part of the AIOS hypervisor subsystem.
///
/// The VMCS is a hardware structure used by Intel VT-x to control
/// VM entries and exits. Each vCPU requires its own VMCS. This module
/// provides allocation, field read/write, and lifecycle management.

use crate::{serial_print, serial_println};
use crate::sync::Mutex;
use alloc::vec::Vec;

/// Size of a VMCS region (4 KiB, page-aligned).
const VMCS_REGION_SIZE: usize = 4096;

/// Maximum number of VMCS regions in the pool.
const MAX_VMCS_POOL: usize = 64;

// --- VMCS field encodings (Intel SDM Vol. 3, Appendix B) ---

/// Guest state fields.
pub const GUEST_ES_SELECTOR: u32 = 0x0800;
pub const GUEST_CS_SELECTOR: u32 = 0x0802;
pub const GUEST_SS_SELECTOR: u32 = 0x0804;
pub const GUEST_DS_SELECTOR: u32 = 0x0806;
pub const GUEST_CR0: u32 = 0x6800;
pub const GUEST_CR3: u32 = 0x6802;
pub const GUEST_CR4: u32 = 0x6804;
pub const GUEST_RSP: u32 = 0x681C;
pub const GUEST_RIP: u32 = 0x681E;
pub const GUEST_RFLAGS: u32 = 0x6820;

/// Host state fields.
pub const HOST_CR0: u32 = 0x6C00;
pub const HOST_CR3: u32 = 0x6C02;
pub const HOST_CR4: u32 = 0x6C04;
pub const HOST_RSP: u32 = 0x6C14;
pub const HOST_RIP: u32 = 0x6C16;
pub const HOST_CS_SELECTOR: u32 = 0x0C02;
pub const HOST_SS_SELECTOR: u32 = 0x0C04;
pub const HOST_DS_SELECTOR: u32 = 0x0C06;
pub const HOST_ES_SELECTOR: u32 = 0x0C00;

/// VM-execution control fields.
pub const PIN_BASED_CONTROLS: u32 = 0x4000;
pub const PROC_BASED_CONTROLS: u32 = 0x4002;
pub const PROC_BASED_CONTROLS2: u32 = 0x401E;
pub const VM_EXIT_CONTROLS: u32 = 0x400C;
pub const VM_ENTRY_CONTROLS: u32 = 0x4012;

/// EPT pointer field.
pub const EPT_POINTER: u32 = 0x201A;

/// VM-exit information fields.
pub const VM_EXIT_REASON: u32 = 0x4402;
pub const VM_EXIT_QUALIFICATION: u32 = 0x6400;
pub const VM_EXIT_INSTR_LEN: u32 = 0x440C;
pub const VM_EXIT_INSTR_INFO: u32 = 0x440E;

/// Global VMCS pool.
static VMCS_POOL: Mutex<Option<VmcsPool>> = Mutex::new(None);

/// Pool of pre-allocated VMCS regions.
struct VmcsPool {
    /// Available (unassigned) VMCS indices.
    free_list: Vec<usize>,
    /// Total number of allocated regions.
    total: usize,
}

/// Represents an Intel VMCS or AMD VMCB control block.
pub struct Vmcs {
    /// Raw VMCS region data (4 KiB, must be page-aligned in real use).
    region: [u8; VMCS_REGION_SIZE],
    /// VMCS revision ID (written into first 4 bytes).
    revision_id: u32,
    /// Whether this VMCS is currently active (VMPTRLD has been executed).
    active: bool,
    /// Pool index for returning to the free list.
    pool_index: usize,
}

impl Vmcs {
    pub fn new() -> Self {
        // Read the revision ID from the VMX state if available.
        let revision_id = Self::read_revision_id();

        let pool_index = {
            let mut pool = VMCS_POOL.lock();
            if let Some(ref mut p) = *pool {
                p.free_list.pop().unwrap_or(0)
            } else {
                0
            }
        };

        let mut vmcs = Vmcs {
            region: [0u8; VMCS_REGION_SIZE],
            revision_id,
            active: false,
            pool_index,
        };

        // Write revision ID into the first 31 bits of the VMCS region.
        let rev_bytes = (revision_id & 0x7FFF_FFFF).to_le_bytes();
        vmcs.region[0] = rev_bytes[0];
        vmcs.region[1] = rev_bytes[1];
        vmcs.region[2] = rev_bytes[2];
        vmcs.region[3] = rev_bytes[3];

        vmcs
    }

    /// Make this VMCS the current/active one (VMPTRLD).
    pub fn activate(&mut self) {
        let phys = self.region.as_ptr() as u64;
        unsafe {
            core::arch::asm!(
                "vmptrld [{}]",
                in(reg) &phys,
                options(nostack),
            );
        }
        self.active = true;
    }

    /// Clear this VMCS (VMCLEAR) — deactivates and initializes to launch state.
    pub fn clear(&mut self) {
        let phys = self.region.as_ptr() as u64;
        unsafe {
            core::arch::asm!(
                "vmclear [{}]",
                in(reg) &phys,
                options(nostack),
            );
        }
        self.active = false;
    }

    /// Read a VMCS field by encoding.
    ///
    /// Uses the VMREAD instruction. The VMCS must be active.
    pub fn read_field(&self, encoding: u32) -> u64 {
        if !self.active {
            serial_println!("    [vmcs] WARNING: read_field on inactive VMCS (encoding=0x{:04x})", encoding);
            return 0;
        }

        let value: u64;
        unsafe {
            core::arch::asm!(
                "vmread {}, {}",
                out(reg) value,
                in(reg) encoding as u64,
                options(nostack),
            );
        }
        value
    }

    /// Write a VMCS field by encoding.
    ///
    /// Uses the VMWRITE instruction. The VMCS must be active.
    pub fn write_field(&mut self, encoding: u32, value: u64) {
        if !self.active {
            serial_println!("    [vmcs] WARNING: write_field on inactive VMCS (encoding=0x{:04x})", encoding);
            return;
        }

        unsafe {
            core::arch::asm!(
                "vmwrite {}, {}",
                in(reg) encoding as u64,
                in(reg) value,
                options(nostack),
            );
        }
    }

    /// Configure default guest state for a 64-bit guest.
    pub fn setup_guest_state(&mut self, entry_rip: u64, entry_rsp: u64) {
        self.write_field(GUEST_RIP, entry_rip);
        self.write_field(GUEST_RSP, entry_rsp);
        self.write_field(GUEST_RFLAGS, 0x2); // Reserved bit 1 always set.
        self.write_field(GUEST_CR0, 0x0000_0021); // PE + NE
        self.write_field(GUEST_CR4, 0x2000);      // VMXE
        self.write_field(GUEST_CS_SELECTOR, 0x08);
        self.write_field(GUEST_SS_SELECTOR, 0x10);
        self.write_field(GUEST_DS_SELECTOR, 0x10);
        self.write_field(GUEST_ES_SELECTOR, 0x10);
    }

    /// Configure host state from current CPU registers.
    pub fn setup_host_state(&mut self, vmexit_handler: u64) {
        let cr0: u64;
        let cr3: u64;
        let cr4: u64;
        unsafe {
            core::arch::asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack));
            core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack));
            core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack));
        }
        self.write_field(HOST_CR0, cr0);
        self.write_field(HOST_CR3, cr3);
        self.write_field(HOST_CR4, cr4);
        self.write_field(HOST_RIP, vmexit_handler);

        // Read current segment selectors.
        let cs: u16;
        let ss: u16;
        let ds: u16;
        let es: u16;
        unsafe {
            core::arch::asm!("mov {:x}, cs", out(reg) cs, options(nomem, nostack));
            core::arch::asm!("mov {:x}, ss", out(reg) ss, options(nomem, nostack));
            core::arch::asm!("mov {:x}, ds", out(reg) ds, options(nomem, nostack));
            core::arch::asm!("mov {:x}, es", out(reg) es, options(nomem, nostack));
        }
        self.write_field(HOST_CS_SELECTOR, cs as u64);
        self.write_field(HOST_SS_SELECTOR, ss as u64);
        self.write_field(HOST_DS_SELECTOR, ds as u64);
        self.write_field(HOST_ES_SELECTOR, es as u64);
    }

    /// Set the EPT pointer for nested page translation.
    pub fn set_ept_pointer(&mut self, ept_root_phys: u64) {
        // Memory type = write-back (6), page-walk length = 4 (value 3 in bits [5:3]).
        let eptp = (ept_root_phys & !0xFFF) | (3 << 3) | 6;
        self.write_field(EPT_POINTER, eptp);
    }

    /// Read the revision ID from IA32_VMX_BASIC MSR.
    fn read_revision_id() -> u32 {
        let lo: u32;
        let hi: u32;
        unsafe {
            core::arch::asm!(
                "rdmsr",
                in("ecx") 0x480u32, // IA32_VMX_BASIC
                out("eax") lo,
                out("edx") hi,
                options(nomem, nostack),
            );
        }
        lo & 0x7FFF_FFFF
    }

    /// Return this VMCS to the pool.
    pub fn release(mut self) {
        self.clear();
        let mut pool = VMCS_POOL.lock();
        if let Some(ref mut p) = *pool {
            p.free_list.push(self.pool_index);
        }
    }
}

pub fn init() {
    let pool = VmcsPool {
        free_list: (0..MAX_VMCS_POOL).rev().collect(),
        total: MAX_VMCS_POOL,
    };

    *VMCS_POOL.lock() = Some(pool);
    serial_println!("    [vmcs] VMCS region pool initialized ({} slots)", MAX_VMCS_POOL);
}
