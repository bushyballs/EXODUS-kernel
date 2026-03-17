//! NVMe Controller Register Definitions
//!
//! Memory-mapped register interface for NVMe controllers (NVMe 1.4 specification).

use core::ptr::{read_volatile, write_volatile};

/// NVMe Controller Registers (MMIO)
#[repr(C)]
pub struct NvmeRegisters {
    base_addr: usize,
}

impl NvmeRegisters {
    /// Create new register interface from BAR address
    pub fn new(base_addr: u64) -> Self {
        NvmeRegisters {
            base_addr: base_addr as usize,
        }
    }

    /// Read 32-bit register
    #[inline]
    fn read32(&self, offset: usize) -> u32 {
        unsafe { read_volatile((self.base_addr + offset) as *const u32) }
    }

    /// Write 32-bit register
    #[inline]
    fn write32(&self, offset: usize, value: u32) {
        unsafe { write_volatile((self.base_addr + offset) as *mut u32, value) }
    }

    /// Read 64-bit register
    #[inline]
    fn read64(&self, offset: usize) -> u64 {
        unsafe { read_volatile((self.base_addr + offset) as *const u64) }
    }

    /// Write 64-bit register
    #[inline]
    fn write64(&self, offset: usize, value: u64) {
        unsafe { write_volatile((self.base_addr + offset) as *mut u64, value) }
    }

    // ========================================================================
    // Controller Capabilities (CAP) - Offset 0x00 (Read-Only)
    // ========================================================================

    pub fn cap(&self) -> u64 {
        self.read64(0x00)
    }

    pub fn max_queue_entries(&self) -> u16 {
        ((self.cap() & 0xFFFF) + 1) as u16
    }

    pub fn contiguous_queues_required(&self) -> bool {
        (self.cap() & (1 << 16)) != 0
    }

    pub fn arbitration_mechanism(&self) -> u8 {
        ((self.cap() >> 17) & 0x3) as u8
    }

    pub fn timeout(&self) -> u8 {
        ((self.cap() >> 24) & 0xFF) as u8
    }

    pub fn doorbell_stride(&self) -> u8 {
        ((self.cap() >> 32) & 0xF) as u8
    }

    pub fn supports_nvm_command_set(&self) -> bool {
        (self.cap() & (1 << 37)) != 0
    }

    pub fn memory_page_size_min(&self) -> usize {
        let mpsmin = ((self.cap() >> 48) & 0xF) as usize;
        4096 << mpsmin
    }

    pub fn memory_page_size_max(&self) -> usize {
        let mpsmax = ((self.cap() >> 52) & 0xF) as usize;
        4096 << mpsmax
    }

    // ========================================================================
    // Version (VS) - Offset 0x08 (Read-Only)
    // ========================================================================

    pub fn version(&self) -> u32 {
        self.read32(0x08)
    }

    pub fn version_major(&self) -> u16 {
        (self.version() >> 16) as u16
    }

    pub fn version_minor(&self) -> u8 {
        ((self.version() >> 8) & 0xFF) as u8
    }

    // ========================================================================
    // Interrupt Mask Set/Clear (INTMS/INTMC) - Offset 0x0C, 0x10
    // ========================================================================

    pub fn intms(&self) -> u32 {
        self.read32(0x0C)
    }

    pub fn set_intms(&self, value: u32) {
        self.write32(0x0C, value)
    }

    pub fn intmc(&self) -> u32 {
        self.read32(0x10)
    }

    pub fn set_intmc(&self, value: u32) {
        self.write32(0x10, value)
    }

    // ========================================================================
    // Controller Configuration (CC) - Offset 0x14
    // ========================================================================

    pub fn cc(&self) -> u32 {
        self.read32(0x14)
    }

    pub fn set_cc(&self, value: u32) {
        self.write32(0x14, value)
    }

    pub fn enable(&self) {
        let mut cc = self.cc();
        cc |= 1; // Set EN bit
        self.set_cc(cc);
    }

    pub fn disable(&self) {
        let mut cc = self.cc();
        cc &= !1; // Clear EN bit
        self.set_cc(cc);
    }

    pub fn set_io_command_set(&self, css: u8) {
        let mut cc = self.cc();
        cc &= !(0x7 << 4); // Clear CSS bits
        cc |= (css as u32 & 0x7) << 4;
        self.set_cc(cc);
    }

    pub fn set_memory_page_size(&self, mps: u8) {
        let mut cc = self.cc();
        cc &= !(0xF << 7); // Clear MPS bits
        cc |= (mps as u32 & 0xF) << 7;
        self.set_cc(cc);
    }

    pub fn set_arbitration_mechanism(&self, ams: u8) {
        let mut cc = self.cc();
        cc &= !(0x7 << 11); // Clear AMS bits
        cc |= (ams as u32 & 0x7) << 11;
        self.set_cc(cc);
    }

    pub fn set_shutdown_notification(&self, shn: u8) {
        let mut cc = self.cc();
        cc &= !(0x3 << 14); // Clear SHN bits
        cc |= (shn as u32 & 0x3) << 14;
        self.set_cc(cc);
    }

    pub fn set_io_submission_queue_entry_size(&self, iosqes: u8) {
        let mut cc = self.cc();
        cc &= !(0xF << 16); // Clear IOSQES bits
        cc |= (iosqes as u32 & 0xF) << 16;
        self.set_cc(cc);
    }

    pub fn set_io_completion_queue_entry_size(&self, iocqes: u8) {
        let mut cc = self.cc();
        cc &= !(0xF << 20); // Clear IOCQES bits
        cc |= (iocqes as u32 & 0xF) << 20;
        self.set_cc(cc);
    }

    // ========================================================================
    // Controller Status (CSTS) - Offset 0x1C (Read-Only)
    // ========================================================================

    pub fn csts(&self) -> u32 {
        self.read32(0x1C)
    }

    pub fn is_ready(&self) -> bool {
        (self.csts() & 1) != 0
    }

    pub fn controller_fatal_status(&self) -> bool {
        (self.csts() & (1 << 1)) != 0
    }

    pub fn shutdown_status(&self) -> u8 {
        ((self.csts() >> 2) & 0x3) as u8
    }

    // ========================================================================
    // NVM Subsystem Reset (NSSR) - Offset 0x20
    // ========================================================================

    pub fn nssr(&self) -> u32 {
        self.read32(0x20)
    }

    pub fn set_nssr(&self, value: u32) {
        self.write32(0x20, value)
    }

    // ========================================================================
    // Admin Queue Attributes (AQA) - Offset 0x24
    // ========================================================================

    pub fn aqa(&self) -> u32 {
        self.read32(0x24)
    }

    pub fn set_aqa(&self, acqs: u16, asqs: u16) {
        let value = ((acqs as u32) << 16) | (asqs as u32);
        self.write32(0x24, value);
    }

    // ========================================================================
    // Admin Submission Queue Base Address (ASQ) - Offset 0x28
    // ========================================================================

    pub fn asq(&self) -> u64 {
        self.read64(0x28)
    }

    pub fn set_asq(&self, addr: u64) {
        self.write64(0x28, addr);
    }

    // ========================================================================
    // Admin Completion Queue Base Address (ACQ) - Offset 0x30
    // ========================================================================

    pub fn acq(&self) -> u64 {
        self.read64(0x30)
    }

    pub fn set_acq(&self, addr: u64) {
        self.write64(0x30, addr);
    }

    // ========================================================================
    // Doorbell Registers - Offset 0x1000+
    // ========================================================================

    pub fn doorbell_offset(&self, queue_id: u16, is_completion: bool) -> usize {
        let stride = (4 << self.doorbell_stride()) as usize;
        let base = 0x1000;
        let queue_offset = (2 * queue_id as usize + if is_completion { 1 } else { 0 }) * stride;
        base + queue_offset
    }

    pub fn ring_submission_doorbell(&self, queue_id: u16, tail: u16) {
        let offset = self.doorbell_offset(queue_id, false);
        self.write32(offset, tail as u32);
    }

    pub fn ring_completion_doorbell(&self, queue_id: u16, head: u16) {
        let offset = self.doorbell_offset(queue_id, true);
        self.write32(offset, head as u32);
    }
}

/// Controller Configuration defaults
impl NvmeRegisters {
    /// Configure controller for standard NVM command set with 4K pages
    pub fn configure_standard(&self) {
        self.set_io_command_set(0); // NVM command set
        self.set_memory_page_size(0); // 4K pages (4096 << 0)
        self.set_arbitration_mechanism(0); // Round Robin
        self.set_io_submission_queue_entry_size(6); // 64 bytes (2^6)
        self.set_io_completion_queue_entry_size(4); // 16 bytes (2^4)
    }
}
