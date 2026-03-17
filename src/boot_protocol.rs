//! Shared boot protocol between firmware loader and Genesis kernel.
//!
//! This contract is consumed by both the UEFI bootloader crate and the kernel.

use core::slice;
use core::sync::atomic::{AtomicUsize, Ordering};

pub const BOOT_INFO_MAGIC: u64 = 0x484F4147_53424F4F; // "HOAGSBOO"

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryKind {
    Usable = 1,
    Reserved = 2,
    AcpiReclaimable = 3,
    AcpiNvs = 4,
    Mmio = 5,
    Bad = 6,
    Bootloader = 7,
    Kernel = 8,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MemoryRegion {
    pub base: u64,
    pub length: u64,
    pub kind: MemoryKind,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MemoryMapInfo {
    pub entries: *const MemoryRegion,
    pub count: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FramebufferInfo {
    pub address: u64,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub bpp: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct BootInfo {
    pub magic: u64,
    pub memory_map: MemoryMapInfo,
    pub framebuffer: FramebufferInfo,
    pub rsdp_address: u64,
    pub kernel_physical_start: u64,
    pub kernel_physical_end: u64,
    pub boot_volume: [u8; 64],
}

impl BootInfo {
    pub fn memory_regions(&self) -> &'static [MemoryRegion] {
        if self.memory_map.entries.is_null() || self.memory_map.count == 0 {
            return &[];
        }
        unsafe { slice::from_raw_parts(self.memory_map.entries, self.memory_map.count as usize) }
    }

    pub fn boot_volume_hint(&self) -> &str {
        let len = self
            .boot_volume
            .iter()
            .position(|b| *b == 0)
            .unwrap_or(self.boot_volume.len());
        core::str::from_utf8(&self.boot_volume[..len]).unwrap_or("")
    }
}

static BOOT_INFO_PTR: AtomicUsize = AtomicUsize::new(0);

pub unsafe fn install_boot_info(ptr: *const BootInfo) -> bool {
    if ptr.is_null() {
        BOOT_INFO_PTR.store(0, Ordering::SeqCst);
        return false;
    }

    let magic = core::ptr::read_volatile(core::ptr::addr_of!((*ptr).magic));
    if magic != BOOT_INFO_MAGIC {
        BOOT_INFO_PTR.store(0, Ordering::SeqCst);
        return false;
    }

    BOOT_INFO_PTR.store(ptr as usize, Ordering::SeqCst);
    true
}

pub fn boot_info() -> Option<&'static BootInfo> {
    let ptr = BOOT_INFO_PTR.load(Ordering::SeqCst);
    if ptr == 0 {
        None
    } else {
        Some(unsafe { &*(ptr as *const BootInfo) })
    }
}
