use crate::sync::Mutex;
/// EDAC (Error Detection and Correction) framework for Genesis — no-heap
///
/// Monitors ECC memory controllers for single-bit correctable errors (CE)
/// and multi-bit uncorrectable errors (UE).  Provides a registration API
/// for memory controllers and chip-select rows, plus an x86 MSR-based
/// hardware poll that inspects MCG_STATUS / MCi_STATUS machine-check
/// registers via the `rdmsr` instruction.
///
/// All rules strictly observed:
///   - No heap: no Vec, Box, String, alloc::*
///   - No panics: no unwrap(), expect(), panic!()
///   - No float casts: no as f64, as f32
///   - Saturating arithmetic for counters
///   - Wrapping arithmetic for sequence numbers
///   - Structs in static Mutex are Copy with const fn empty()
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of EDAC memory controllers
pub const MAX_EDAC_MCS: usize = 4;

/// Maximum number of chip-select rows per memory controller
pub const MAX_EDAC_CSROWS: usize = 8;

/// No ECC capability
pub const EDAC_NONE: u8 = 0;
/// Reserved (implementation-defined)
pub const EDAC_RESERVED: u8 = 1;
/// Parity checking only (detect but cannot correct)
pub const EDAC_PARITY: u8 = 2;
/// Error Correction only (EC; single-bit correct, no detection of multi-bit)
pub const EDAC_EC: u8 = 3;
/// Single Error Correct / Double Error Detect (most common ECC DRAM type)
pub const EDAC_SECDED: u8 = 4;

/// MSR address for MCG_STATUS (Machine Check Global Status)
const MCG_STATUS_MSR: u32 = 0x17A;

/// MCi_STATUS MSR base address (bank 0 = 0x401, bank 1 = 0x405, …)
/// We poll bank 0 only in the stub.
const MC0_STATUS_MSR: u32 = 0x401;

/// MCG_STATUS bit 0: RIPV — restart IP valid (set when any MCA bank fired)
const MCG_RIPV_BIT: u64 = 0x1;

/// MCi_STATUS bit 63: VAL — valid error logged
const MCI_STATUS_VAL: u64 = 1u64 << 63;

/// MCi_STATUS bit 61: UC — uncorrected error
const MCI_STATUS_UC: u64 = 1u64 << 61;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A chip-select row within a memory controller.
///
/// Each csrow corresponds to a group of DRAM chips sharing a common CS line.
#[derive(Copy, Clone)]
pub struct EdacCsrow {
    /// Channel number this row belongs to (0-based)
    pub channel: u8,
    /// Size of this rank in megabytes
    pub size_mb: u32,
    /// ECC type: EDAC_NONE / EDAC_PARITY / EDAC_EC / EDAC_SECDED / …
    pub edac_type: u8,
    /// Accumulated count of correctable (single-bit) errors
    pub ce_count: u64,
    /// Accumulated count of uncorrectable (multi-bit) errors
    pub ue_count: u64,
    /// True when this slot is occupied
    pub active: bool,
}

impl EdacCsrow {
    pub const fn empty() -> Self {
        EdacCsrow {
            channel: 0,
            size_mb: 0,
            edac_type: EDAC_NONE,
            ce_count: 0,
            ue_count: 0,
            active: false,
        }
    }
}

/// A registered EDAC memory controller.
#[derive(Copy, Clone)]
pub struct EdacMc {
    /// Numeric ID assigned at registration
    pub id: u32,
    /// Short device/driver name (null-padded, up to 15 printable bytes)
    pub dev_name: [u8; 16],
    /// Chip-select row table
    pub csrows: [EdacCsrow; MAX_EDAC_CSROWS],
    /// Number of active csrows
    pub ncsrows: u8,
    /// Sum of CE counts across all csrows
    pub total_ce: u64,
    /// Sum of UE counts across all csrows
    pub total_ue: u64,
    /// True when this slot is occupied
    pub active: bool,
}

impl EdacMc {
    pub const fn empty() -> Self {
        EdacMc {
            id: 0,
            dev_name: [0u8; 16],
            csrows: [EdacCsrow::empty(); MAX_EDAC_CSROWS],
            ncsrows: 0,
            total_ce: 0,
            total_ue: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static EDAC_MCS: Mutex<[EdacMc; MAX_EDAC_MCS]> = Mutex::new([EdacMc::empty(); MAX_EDAC_MCS]);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Copy up to 15 bytes from `src` into the 16-byte fixed buffer `dst`,
/// NUL-terminating at byte index 15.
fn copy_dev_name(dst: &mut [u8; 16], src: &[u8]) {
    let len = if src.len() < 15 { src.len() } else { 15 };
    let mut i: usize = 0;
    while i < len {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
    // NUL-terminate the rest
    while i < 16 {
        dst[i] = 0;
        i = i.saturating_add(1);
    }
}

/// Find a memory controller index by id.  Returns `None` if not found.
fn find_mc_idx(mcs: &[EdacMc; MAX_EDAC_MCS], mc_id: u32) -> Option<usize> {
    let mut i: usize = 0;
    while i < MAX_EDAC_MCS {
        if mcs[i].active && mcs[i].id == mc_id {
            return Some(i);
        }
        i = i.saturating_add(1);
    }
    None
}

/// Execute the x86 `rdmsr` instruction and return the 64-bit MSR value.
///
/// # Safety
///
/// Requires Ring 0 (CPL 0).  Reads a Model-Specific Register.  If the MSR
/// address is invalid the CPU will raise a #GP fault — this is acceptable
/// at boot time in a bare-metal kernel where a fault handler is expected to
/// be installed.
#[inline]
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack, preserves_flags),
    );
    ((hi as u64) << 32) | (lo as u64)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Register a new memory controller.
///
/// `dev_name` — a short label such as `b"i7-imc"` or `b"amd-k8"`.
///
/// Returns the assigned mc id on success, or `None` if the table is full.
pub fn edac_register_mc(dev_name: &[u8]) -> Option<u32> {
    let mut mcs = EDAC_MCS.lock();
    let mut i: usize = 0;
    while i < MAX_EDAC_MCS {
        if !mcs[i].active {
            let id = i as u32;
            let mut mc = EdacMc {
                id,
                dev_name: [0u8; 16],
                csrows: [EdacCsrow::empty(); MAX_EDAC_CSROWS],
                ncsrows: 0,
                total_ce: 0,
                total_ue: 0,
                active: true,
            };
            copy_dev_name(&mut mc.dev_name, dev_name);
            mcs[i] = mc;
            return Some(id);
        }
        i = i.saturating_add(1);
    }
    None
}

/// Add a chip-select row to an existing memory controller.
///
/// `mc_id`     — controller id returned by `edac_register_mc`.
/// `channel`   — channel number this row belongs to.
/// `size_mb`   — capacity of this rank in megabytes.
/// `edac_type` — ECC type constant (`EDAC_NONE`, `EDAC_SECDED`, etc.).
///
/// Returns `true` on success, `false` if the controller is not found or its
/// csrow table is already full.
pub fn edac_add_csrow(mc_id: u32, channel: u8, size_mb: u32, edac_type: u8) -> bool {
    let mut mcs = EDAC_MCS.lock();
    if let Some(mc_idx) = find_mc_idx(&*mcs, mc_id) {
        let ncs = mcs[mc_idx].ncsrows as usize;
        if ncs >= MAX_EDAC_CSROWS {
            return false;
        }
        mcs[mc_idx].csrows[ncs] = EdacCsrow {
            channel,
            size_mb,
            edac_type,
            ce_count: 0,
            ue_count: 0,
            active: true,
        };
        mcs[mc_idx].ncsrows = mcs[mc_idx].ncsrows.saturating_add(1);
        return true;
    }
    false
}

/// Handle a correctable (single-bit) error event.
///
/// Increments the CE counter for `csrow` inside `mc_id`, and the
/// controller-level `total_ce` counter.  Prints a serial log message.
///
/// `syndrome` — the ECC syndrome bits reported by the hardware.
pub fn edac_handle_ce(mc_id: u32, csrow: u8, syndrome: u32) {
    let mut mcs = EDAC_MCS.lock();
    if let Some(mc_idx) = find_mc_idx(&*mcs, mc_id) {
        let row = csrow as usize;
        if row < MAX_EDAC_CSROWS && mcs[mc_idx].csrows[row].active {
            mcs[mc_idx].csrows[row].ce_count = mcs[mc_idx].csrows[row].ce_count.saturating_add(1);
            mcs[mc_idx].total_ce = mcs[mc_idx].total_ce.saturating_add(1);
            serial_println!(
                "[edac] MC{} csrow{}: correctable error (syndrome=0x{:08x})",
                mc_id,
                csrow,
                syndrome
            );
        }
    }
}

/// Handle an uncorrectable (multi-bit) error event.
///
/// Increments the UE counter for `csrow` inside `mc_id`, and the
/// controller-level `total_ue` counter.  Prints a serial log message marked
/// UNCORRECTABLE ERROR.
///
/// `syndrome` — the ECC syndrome bits reported by the hardware.
pub fn edac_handle_ue(mc_id: u32, csrow: u8, syndrome: u32) {
    let mut mcs = EDAC_MCS.lock();
    if let Some(mc_idx) = find_mc_idx(&*mcs, mc_id) {
        let row = csrow as usize;
        if row < MAX_EDAC_CSROWS && mcs[mc_idx].csrows[row].active {
            mcs[mc_idx].csrows[row].ue_count = mcs[mc_idx].csrows[row].ue_count.saturating_add(1);
            mcs[mc_idx].total_ue = mcs[mc_idx].total_ue.saturating_add(1);
            serial_println!(
                "[edac] MC{} csrow{}: UNCORRECTABLE ERROR (syndrome=0x{:08x})",
                mc_id,
                csrow,
                syndrome
            );
        }
    }
}

/// Return aggregate error counters for a memory controller.
///
/// Returns `Some((total_ce, total_ue))` on success, or `None` if `mc_id` is
/// not found.
pub fn edac_get_mc_stats(mc_id: u32) -> Option<(u64, u64)> {
    let mcs = EDAC_MCS.lock();
    if let Some(mc_idx) = find_mc_idx(&*mcs, mc_id) {
        return Some((mcs[mc_idx].total_ce, mcs[mc_idx].total_ue));
    }
    None
}

/// Poll x86 Machine Check Architecture registers for hardware error events.
///
/// Algorithm:
///   1. Read MCG_STATUS (MSR 0x17A).  If bit 0 (RIPV) is clear no machine
///      check has fired — return immediately.
///   2. Read MC0_STATUS (MSR 0x401, bank 0).
///   3. If both VAL (bit 63) and UC (bit 61) are set, report an
///      uncorrectable error to mc_id=0 / csrow=0 with the lower 32 bits of
///      MC0_STATUS as the syndrome.
///
/// On bare-metal x86 this function must be called from Ring 0 only.
/// In QEMU/simulation the MSRs return 0, so the function returns without
/// reporting any errors.
pub fn edac_poll_hardware() {
    // Safety: Ring-0 bare-metal kernel. #GP on invalid MSR is acceptable.
    let mcg_status = unsafe { rdmsr(MCG_STATUS_MSR) };

    // Bit 0 (RIPV) — if clear, no valid machine check in any bank
    if mcg_status & MCG_RIPV_BIT == 0 {
        return;
    }

    let mc0_status = unsafe { rdmsr(MC0_STATUS_MSR) };

    // Check VAL (bit 63) and UC (bit 61) together
    if (mc0_status & MCI_STATUS_VAL != 0) && (mc0_status & MCI_STATUS_UC != 0) {
        // Lower 32 bits carry the ECC syndrome / error code
        let syndrome = (mc0_status & 0xFFFF_FFFF) as u32;
        edac_handle_ue(0, 0, syndrome);
    }
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the EDAC framework.
///
/// Registers a default memory controller named `b"genesis-mc"` with one
/// SECDED chip-select row of 256 MB on channel 0, then prints a boot message.
pub fn init() {
    match edac_register_mc(b"genesis-mc") {
        Some(mc_id) => {
            edac_add_csrow(mc_id, 0, 256, EDAC_SECDED);
            serial_println!("[edac] EDAC framework initialized");
        }
        None => {
            serial_println!("[edac] EDAC framework initialized (no MC slots available)");
        }
    }
}
