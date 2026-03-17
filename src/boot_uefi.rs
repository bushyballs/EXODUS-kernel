/// Genesis AIOS — UEFI Bootloader Infrastructure
///
/// This module implements the UEFI-path boot sequence that runs before the
/// kernel proper takes over.  It is compiled only when the `uefi-boot` feature
/// is enabled (or when building the dedicated UEFI application crate).
///
/// ## Boot sequence
///
/// 1. Locate the UEFI Graphics Output Protocol (GOP) and switch to the
///    preferred 1920×1080 (or closest available) graphics mode.
///    Read the linear framebuffer base address and geometry.
///
/// 2. Scan for the ACPI RSDP signature ("RSD PTR ") in the three canonical
///    locations:
///      a. UEFI EFI_CONFIGURATION_TABLE (preferred — firmware hands it to us)
///      b. EBDA (Extended BIOS Data Area)  0x9FC00 – 0x9FFFF
///      c. BIOS ROM region                 0xE0000 – 0xFFFFF
///
/// 3. Retrieve the UEFI memory map via GetMemoryMap(), classify each
///    descriptor into our MemoryKind, and store the list in a static
///    MemoryRegion array that the kernel will consume.
///
/// 4. Load the kernel ELF: parse the ELF64 header, iterate PT_LOAD segments,
///    copy each segment to its physical load address, zero any .bss, then
///    jump to the kernel entry point (e_entry) passing a pointer to the
///    populated BootInfo structure.
///
/// ## `no_std` + `no_alloc` design
///
/// Everything here uses fixed-size static storage so this module can run
/// before the kernel heap is initialised.  The only dynamic resource is the
/// UEFI memory map buffer, which is allocated via UEFI boot services and
/// freed before ExitBootServices().
///
/// All code is original — Hoags Inc. (c) 2026.

#[allow(dead_code)]
use crate::boot_protocol::{
    BootInfo, FramebufferInfo, MemoryKind, MemoryMapInfo, MemoryRegion, BOOT_INFO_MAGIC,
};
use crate::serial_println;

// ============================================================================
// Static storage — no heap required
// ============================================================================

/// Maximum memory map entries we can track
const MAX_MEMORY_REGIONS: usize = 512;

/// Statically allocated memory region array (filled during boot, read by kernel)
static mut MEMORY_REGIONS: [MemoryRegion; MAX_MEMORY_REGIONS] = [MemoryRegion {
    base: 0,
    length: 0,
    kind: MemoryKind::Reserved,
}; MAX_MEMORY_REGIONS];

static mut MEMORY_REGION_COUNT: usize = 0;

/// Statically allocated BootInfo passed to the kernel
static mut BOOT_INFO: BootInfo = BootInfo {
    magic: BOOT_INFO_MAGIC,
    memory_map: MemoryMapInfo {
        entries: core::ptr::null(),
        count: 0,
    },
    framebuffer: FramebufferInfo {
        address: 0,
        width: 0,
        height: 0,
        stride: 0,
        bpp: 32,
    },
    rsdp_address: 0,
    kernel_physical_start: 0,
    kernel_physical_end: 0,
    boot_volume: [0u8; 64],
};

// ============================================================================
// TASK 1A: GOP Framebuffer setup
// ============================================================================

/// Preferred display resolution (1920 × 1080).
/// If the firmware does not support this mode we fall back to the mode
/// with the highest pixel count that fits within these dimensions.
const PREFERRED_WIDTH: u32 = 1920;
const PREFERRED_HEIGHT: u32 = 1080;

/// UEFI GOP pixel formats (subset we care about)
#[repr(u32)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GopPixelFormat {
    Rgbx32 = 0,  // EfixelFormatRedGreenBlueReserved8BitPerColor
    Bgrx32 = 1,  // EfiPixelFormatBlueGreenRedReserved8BitPerColor
    BitMask = 2, // EfiPixelBitMask
    BltOnly = 3, // No linear framebuffer
}

/// Minimal mirror of EFI_GRAPHICS_OUTPUT_MODE_INFORMATION
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GopModeInfo {
    pub version: u32,
    pub horizontal_resolution: u32,
    pub vertical_resolution: u32,
    pub pixel_format: GopPixelFormat,
    pub pixels_per_scan_line: u32,
}

/// Populated framebuffer descriptor (resolution-independent)
#[derive(Clone, Copy, Debug)]
pub struct FramebufferDesc {
    /// Physical base address of the linear framebuffer
    pub base: u64,
    pub width: u32,
    pub height: u32,
    /// Bytes per row (may be wider than width × bpp/8 due to alignment)
    pub stride_bytes: u32,
    /// Bits per pixel (always 32 for RGBX / BGRX modes)
    pub bpp: u32,
}

/// Locate the best GOP mode and populate a FramebufferDesc.
///
/// In a real UEFI environment this function is called from the UEFI entry
/// point (before ExitBootServices) with a pointer to the GOP protocol.
/// Here we model the logic so it can be tested / linked against a UEFI
/// shim, and the result is stored in BOOT_INFO.
///
/// # Safety
///
/// `mode_info_array` must point to `mode_count` valid `GopModeInfo` entries.
/// `framebuffer_base` is the physical address reported by the firmware for
/// mode 0; callers must adjust if the selected mode is different.
pub unsafe fn gop_select_mode(
    mode_info_array: *const GopModeInfo,
    mode_count: u32,
    framebuffer_base: u64,
) -> Option<FramebufferDesc> {
    if mode_info_array.is_null() || mode_count == 0 {
        serial_println!("  UEFI/GOP: no mode info provided");
        return None;
    }

    let modes = core::slice::from_raw_parts(mode_info_array, mode_count as usize);

    // Pass 1: look for exact 1920×1080
    let mut best_idx: Option<usize> = None;
    let mut best_pixels: u64 = 0;

    for (idx, mode) in modes.iter().enumerate() {
        // Skip BltOnly — no linear framebuffer
        if mode.pixel_format == GopPixelFormat::BltOnly {
            continue;
        }

        // Only consider modes that fit within our preferred bounds
        if mode.horizontal_resolution > PREFERRED_WIDTH
            || mode.vertical_resolution > PREFERRED_HEIGHT
        {
            continue;
        }

        let pixels = mode.horizontal_resolution as u64 * mode.vertical_resolution as u64;

        // Prefer exact match; otherwise highest pixel count
        if mode.horizontal_resolution == PREFERRED_WIDTH
            && mode.vertical_resolution == PREFERRED_HEIGHT
        {
            best_idx = Some(idx);
            break;
        }

        if pixels > best_pixels {
            best_pixels = pixels;
            best_idx = Some(idx);
        }
    }

    let idx = match best_idx {
        Some(i) => i,
        None => {
            serial_println!("  UEFI/GOP: no suitable graphics mode found");
            return None;
        }
    };

    let mode = &modes[idx];
    let stride_bytes = mode.pixels_per_scan_line * 4; // 4 bytes per pixel (32-bit)

    let desc = FramebufferDesc {
        base: framebuffer_base,
        width: mode.horizontal_resolution,
        height: mode.vertical_resolution,
        stride_bytes,
        bpp: 32,
    };

    // Store in global BootInfo
    BOOT_INFO.framebuffer = FramebufferInfo {
        address: desc.base,
        width: desc.width,
        height: desc.height,
        stride: desc.stride_bytes,
        bpp: desc.bpp,
    };

    serial_println!(
        "  UEFI/GOP: mode {} selected ({}x{}, stride={}, base=0x{:x})",
        idx,
        desc.width,
        desc.height,
        desc.stride_bytes,
        desc.base,
    );

    Some(desc)
}

// ============================================================================
// TASK 1B: ACPI RSDP location
// ============================================================================

/// RSDP v1 (ACPI 1.0) signature
const RSDP_SIGNATURE: &[u8; 8] = b"RSD PTR ";

/// Attempt to locate the ACPI RSDP.
///
/// Search order:
///   1. `uefi_rsdp_hint` — physical address passed by UEFI firmware via
///      EFI_CONFIGURATION_TABLE (ACPI_20_TABLE_GUID or ACPI_TABLE_GUID).
///      Pass 0 if unavailable.
///   2. EBDA: first 1 KB starting at the segment stored in BDA[0x40E].
///   3. BIOS ROM: 0xE0000 – 0xFFFFF.
///   4. Extended search: 0x100000 – 0x200000 (some OVMF/virtual firmware).
///
/// Returns the physical address of the RSDP or 0 if not found.
///
/// # Safety
///
/// Scans raw physical memory in BIOS regions.  Must be called before
/// ExitBootServices (while identity mapping is active) or from within
/// the kernel after those regions are identity-mapped.
pub unsafe fn locate_rsdp(uefi_rsdp_hint: u64) -> u64 {
    // --- 1. UEFI configuration table hint (most reliable) ---
    if uefi_rsdp_hint != 0 {
        let ptr = uefi_rsdp_hint as *const u8;
        if validate_rsdp_signature(ptr) {
            serial_println!(
                "  UEFI/ACPI: RSDP at 0x{:x} (from firmware config table)",
                uefi_rsdp_hint
            );
            BOOT_INFO.rsdp_address = uefi_rsdp_hint;
            return uefi_rsdp_hint;
        }
    }

    // --- 2. EBDA (Extended BIOS Data Area) ---
    // BDA offset 0x40E holds the EBDA segment (shift left 4 for physical addr)
    let ebda_segment = core::ptr::read_volatile(0x40E as *const u16) as u64;
    let ebda_base = ebda_segment << 4;
    if ebda_base >= 0x80000 && ebda_base < 0xA0000 {
        if let Some(addr) = scan_rsdp(ebda_base, 1024) {
            serial_println!("  UEFI/ACPI: RSDP at 0x{:x} (EBDA)", addr);
            BOOT_INFO.rsdp_address = addr;
            return addr;
        }
    }

    // --- 3. BIOS ROM area: 0xE0000 – 0xFFFFF ---
    if let Some(addr) = scan_rsdp(0xE0000, 0x20000) {
        serial_println!("  UEFI/ACPI: RSDP at 0x{:x} (BIOS ROM)", addr);
        BOOT_INFO.rsdp_address = addr;
        return addr;
    }

    // --- 4. Extended scan: 0x100000 – 0x200000 (OVMF / some QEMU configs) ---
    if let Some(addr) = scan_rsdp(0x100000, 0x100000) {
        serial_println!("  UEFI/ACPI: RSDP at 0x{:x} (extended scan)", addr);
        BOOT_INFO.rsdp_address = addr;
        return addr;
    }

    serial_println!("  UEFI/ACPI: RSDP not found");
    0
}

/// Scan `size` bytes starting at `base` (16-byte aligned) for the RSDP.
unsafe fn scan_rsdp(base: u64, size: u64) -> Option<u64> {
    let end = base.saturating_add(size);
    let mut addr = base;

    while addr < end {
        // Bounds: ensure the 20-byte RSDP fits before the region end
        if addr.saturating_add(20) > end {
            break;
        }
        let ptr = addr as *const u8;
        if validate_rsdp_signature(ptr) && validate_rsdp_checksum(ptr) {
            return Some(addr);
        }
        addr = addr.saturating_add(16); // RSDP is always 16-byte aligned
    }
    None
}

/// Check for "RSD PTR " at `ptr`
#[inline]
unsafe fn validate_rsdp_signature(ptr: *const u8) -> bool {
    if ptr.is_null() {
        return false;
    }
    for (i, &b) in RSDP_SIGNATURE.iter().enumerate() {
        if core::ptr::read_volatile(ptr.add(i)) != b {
            return false;
        }
    }
    true
}

/// Validate the 20-byte RSDP v1 checksum (all bytes must sum to 0 mod 256)
#[inline]
unsafe fn validate_rsdp_checksum(ptr: *const u8) -> bool {
    let mut sum: u8 = 0;
    for i in 0..20usize {
        sum = sum.wrapping_add(core::ptr::read_volatile(ptr.add(i)));
    }
    sum == 0
}

// ============================================================================
// TASK 1C: Memory map
// ============================================================================

/// UEFI memory type codes (EFI_MEMORY_TYPE)
#[repr(u32)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UefiMemoryType {
    ReservedMemoryType = 0,
    LoaderCode = 1,
    LoaderData = 2,
    BootServicesCode = 3,
    BootServicesData = 4,
    RuntimeServicesCode = 5,
    RuntimeServicesData = 6,
    ConventionalMemory = 7,
    UnusableMemory = 8,
    AcpiReclaimMemory = 9,
    AcpiMemoryNvs = 10,
    MemoryMappedIo = 11,
    MemoryMappedIoPortSpace = 12,
    PalCode = 13,
    PersistentMemory = 14,
}

impl UefiMemoryType {
    fn from_u32(v: u32) -> Self {
        match v {
            1 => UefiMemoryType::LoaderCode,
            2 => UefiMemoryType::LoaderData,
            3 => UefiMemoryType::BootServicesCode,
            4 => UefiMemoryType::BootServicesData,
            5 => UefiMemoryType::RuntimeServicesCode,
            6 => UefiMemoryType::RuntimeServicesData,
            7 => UefiMemoryType::ConventionalMemory,
            8 => UefiMemoryType::UnusableMemory,
            9 => UefiMemoryType::AcpiReclaimMemory,
            10 => UefiMemoryType::AcpiMemoryNvs,
            11 => UefiMemoryType::MemoryMappedIo,
            12 => UefiMemoryType::MemoryMappedIoPortSpace,
            _ => UefiMemoryType::ReservedMemoryType,
        }
    }
}

/// Raw UEFI memory descriptor (EFI_MEMORY_DESCRIPTOR, 48 bytes)
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct UefiMemoryDescriptor {
    pub memory_type: u32,
    _pad: u32,
    pub physical_start: u64,
    pub virtual_start: u64,
    pub number_of_pages: u64,
    pub attribute: u64,
}

/// Convert a UEFI memory type into our MemoryKind.
fn classify_uefi_memory(mem_type: UefiMemoryType) -> MemoryKind {
    match mem_type {
        UefiMemoryType::ConventionalMemory
        | UefiMemoryType::LoaderCode
        | UefiMemoryType::LoaderData
        | UefiMemoryType::BootServicesCode
        | UefiMemoryType::BootServicesData => MemoryKind::Usable,

        UefiMemoryType::AcpiReclaimMemory => MemoryKind::AcpiReclaimable,
        UefiMemoryType::AcpiMemoryNvs => MemoryKind::AcpiNvs,

        UefiMemoryType::MemoryMappedIo | UefiMemoryType::MemoryMappedIoPortSpace => {
            MemoryKind::Mmio
        }

        UefiMemoryType::UnusableMemory => MemoryKind::Bad,

        // RuntimeServices, PalCode, PersistentMemory, Reserved → Reserved
        _ => MemoryKind::Reserved,
    }
}

/// Parse the UEFI memory map returned by GetMemoryMap().
///
/// `map_buffer`      — pointer to the raw memory descriptor buffer
/// `map_size`        — total size of the buffer in bytes (as returned by UEFI)
/// `descriptor_size` — size of each descriptor in bytes (may be > 48 due to
///                     firmware-specific extensions; always stride by this value)
///
/// Populates the static `MEMORY_REGIONS` array and updates `BOOT_INFO`.
///
/// # Safety
///
/// `map_buffer` must point to `map_size` bytes of valid UEFI memory descriptor
/// data.  `descriptor_size` must be >= `size_of::<UefiMemoryDescriptor>()`.
pub unsafe fn parse_uefi_memory_map(
    map_buffer: *const u8,
    map_size: usize,
    descriptor_size: usize,
) {
    if map_buffer.is_null() || map_size == 0 || descriptor_size == 0 {
        serial_println!("  UEFI/MMap: invalid arguments");
        return;
    }

    let min_desc_size = core::mem::size_of::<UefiMemoryDescriptor>();
    if descriptor_size < min_desc_size {
        serial_println!(
            "  UEFI/MMap: descriptor_size ({}) < expected ({})",
            descriptor_size,
            min_desc_size
        );
        return;
    }

    let entry_count = map_size / descriptor_size;
    let mut region_idx: usize = 0;

    for i in 0..entry_count {
        if region_idx >= MAX_MEMORY_REGIONS {
            serial_println!("  UEFI/MMap: region table full at {} entries", region_idx);
            break;
        }

        let desc_ptr = map_buffer.add(i * descriptor_size) as *const UefiMemoryDescriptor;
        let desc = &*desc_ptr;

        if desc.number_of_pages == 0 {
            continue; // skip zero-length descriptors
        }

        let kind = classify_uefi_memory(UefiMemoryType::from_u32(desc.memory_type));
        let length = desc.number_of_pages * 4096;

        MEMORY_REGIONS[region_idx] = MemoryRegion {
            base: desc.physical_start,
            length,
            kind,
        };
        region_idx += 1;
    }

    MEMORY_REGION_COUNT = region_idx;

    // Wire up the BootInfo pointers
    BOOT_INFO.memory_map = MemoryMapInfo {
        entries: MEMORY_REGIONS.as_ptr(),
        count: region_idx as u64,
    };

    serial_println!(
        "  UEFI/MMap: {} regions parsed from {} UEFI descriptors",
        region_idx,
        entry_count
    );
}

// ============================================================================
// TASK 1D: ELF64 kernel loading
// ============================================================================

/// ELF64 magic bytes
const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];

/// ELF64 header (Ehdr)
#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct Elf64Ehdr {
    e_ident: [u8; 16],
    e_type: u16,
    e_machine: u16,
    e_version: u32,
    e_entry: u64, // Virtual address of the entry point
    e_phoff: u64, // Offset to the first program header
    e_shoff: u64, // Offset to section headers (unused here)
    e_flags: u32,
    e_ehsize: u16,
    e_phentsize: u16, // Size of one program header entry
    e_phnum: u16,     // Number of program header entries
    e_shentsize: u16,
    e_shnum: u16,
    e_shstrndx: u16,
}

/// ELF64 program header (Phdr)
#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct Elf64Phdr {
    p_type: u32,   // PT_NULL=0, PT_LOAD=1, PT_DYNAMIC=2, ...
    p_flags: u32,  // PF_X=1, PF_W=2, PF_R=4
    p_offset: u64, // Offset in the ELF file
    p_vaddr: u64,  // Virtual address in memory
    p_paddr: u64,  // Physical address (used for loading)
    p_filesz: u64, // Bytes in the file image
    p_memsz: u64,  // Bytes in memory (p_memsz >= p_filesz; gap is zero-filled)
    p_align: u64,  // Alignment (must be power of two)
}

/// PT_LOAD program header type
const PT_LOAD: u32 = 1;

/// ELF machine type: x86_64
const EM_X86_64: u16 = 0x3E;

/// Result of a successful ELF load
#[derive(Clone, Copy, Debug)]
pub struct ElfLoadResult {
    /// Physical entry point address
    pub entry_point: u64,
    /// Lowest physical address occupied by any PT_LOAD segment
    pub phys_start: u64,
    /// One byte past the highest physical address occupied
    pub phys_end: u64,
}

/// Parse a kernel ELF image and copy each PT_LOAD segment to its physical
/// load address.
///
/// `elf_data` — pointer to the ELF binary in memory
/// `elf_size` — size of the ELF binary in bytes
///
/// Returns `Some(ElfLoadResult)` on success, `None` on any validation error.
///
/// # Safety
///
/// * `elf_data` must point to `elf_size` valid readable bytes.
/// * The physical addresses in all PT_LOAD segments must already be identity-
///   mapped and writable (pre-ExitBootServices, or kernel address space).
/// * Overlapping segments produce undefined behaviour; well-formed ELFs don't
///   produce overlapping PT_LOAD segments.
pub unsafe fn load_kernel_elf(elf_data: *const u8, elf_size: usize) -> Option<ElfLoadResult> {
    // --- Validate ELF magic ---
    if elf_size < core::mem::size_of::<Elf64Ehdr>() {
        serial_println!("  ELF: image too small ({} bytes)", elf_size);
        return None;
    }

    let ehdr = &*(elf_data as *const Elf64Ehdr);

    if ehdr.e_ident[0..4] != ELF_MAGIC {
        serial_println!("  ELF: bad magic");
        return None;
    }
    if ehdr.e_ident[4] != 2 {
        // ELFCLASS64
        serial_println!("  ELF: not a 64-bit ELF");
        return None;
    }
    if ehdr.e_ident[5] != 1 {
        // ELFDATA2LSB
        serial_println!("  ELF: not little-endian");
        return None;
    }
    if ehdr.e_machine != EM_X86_64 {
        serial_println!("  ELF: wrong machine type (0x{:x})", ehdr.e_machine);
        return None;
    }
    if ehdr.e_phentsize < core::mem::size_of::<Elf64Phdr>() as u16 {
        serial_println!("  ELF: program header entry too small");
        return None;
    }

    let phoff = ehdr.e_phoff as usize;
    let phnum = ehdr.e_phnum as usize;
    let phentsize = ehdr.e_phentsize as usize;

    // Bounds check: program header table must fit within the ELF image
    let phtab_end = phoff.saturating_add(phnum.saturating_mul(phentsize));
    if phtab_end > elf_size {
        serial_println!("  ELF: program header table extends beyond image");
        return None;
    }

    let mut phys_start = u64::MAX;
    let mut phys_end: u64 = 0;
    let mut loaded_segments: usize = 0;

    for i in 0..phnum {
        let phdr_offset = phoff.saturating_add(i.saturating_mul(phentsize));
        let phdr = &*(elf_data.add(phdr_offset) as *const Elf64Phdr);

        if phdr.p_type != PT_LOAD {
            continue;
        }

        // Validate offsets before memcpy
        let file_offset = phdr.p_offset as usize;
        let file_size = phdr.p_filesz as usize;
        let mem_size = phdr.p_memsz as usize;
        let phys_addr = phdr.p_paddr;

        if file_size > 0 {
            let seg_end = file_offset.saturating_add(file_size);
            if seg_end > elf_size {
                serial_println!(
                    "  ELF: segment {} file data extends beyond image (off={}, filesz={})",
                    i,
                    file_offset,
                    file_size
                );
                return None;
            }

            // Copy the file image to the physical load address
            let src = elf_data.add(file_offset);
            let dst = phys_addr as *mut u8;
            core::ptr::copy_nonoverlapping(src, dst, file_size);
        }

        // Zero the .bss / uninitialized gap (p_memsz - p_filesz)
        if mem_size > file_size {
            let bss_start = (phys_addr as usize).saturating_add(file_size);
            let bss_len = mem_size - file_size;
            core::ptr::write_bytes(bss_start as *mut u8, 0, bss_len);
        }

        // Track physical footprint
        let seg_phys_end = phys_addr.saturating_add(mem_size as u64);
        if phys_addr < phys_start {
            phys_start = phys_addr;
        }
        if seg_phys_end > phys_end {
            phys_end = seg_phys_end;
        }

        loaded_segments += 1;
        serial_println!(
            "  ELF: PT_LOAD[{}] paddr=0x{:x} filesz={} memsz={}",
            i,
            phys_addr,
            file_size,
            mem_size
        );
    }

    if loaded_segments == 0 {
        serial_println!("  ELF: no PT_LOAD segments found");
        return None;
    }

    // Update BootInfo physical extent
    BOOT_INFO.kernel_physical_start = phys_start;
    BOOT_INFO.kernel_physical_end = phys_end;

    serial_println!(
        "  ELF: {} segments loaded, entry=0x{:x}, phys 0x{:x}–0x{:x}",
        loaded_segments,
        ehdr.e_entry,
        phys_start,
        phys_end
    );

    Some(ElfLoadResult {
        entry_point: ehdr.e_entry,
        phys_start,
        phys_end,
    })
}

// ============================================================================
// TASK 1E: High-level UEFI boot entry
// ============================================================================

/// Type alias for the kernel entry function.
///
/// The kernel entry (`_start`) takes a single pointer to BootInfo and
/// never returns.
type KernelEntry = unsafe extern "sysv64" fn(*const BootInfo) -> !;

/// Complete UEFI boot flow.
///
/// Call this from the UEFI application entry point after locating:
///   - GOP mode info array and current framebuffer base
///   - RSDP hint from EFI_CONFIGURATION_TABLE
///   - Raw UEFI memory map buffer
///   - Kernel ELF image in memory
///
/// If all steps succeed this function jumps to the kernel and never returns.
/// On error it returns `false`.
///
/// # Safety
///
/// All pointer parameters must be valid.  The kernel ELF must be self-
/// relocating or position-independent if the physical addresses in its
/// PT_LOAD segments differ from the addresses it was linked for.
pub unsafe fn uefi_boot(
    gop_modes: *const GopModeInfo,
    gop_mode_count: u32,
    gop_framebuffer_base: u64,
    uefi_rsdp_hint: u64,
    uefi_mmap_buffer: *const u8,
    uefi_mmap_size: usize,
    uefi_mmap_descriptor_size: usize,
    kernel_elf_data: *const u8,
    kernel_elf_size: usize,
    boot_volume_label: &[u8],
) -> bool {
    serial_println!("Genesis UEFI boot started");

    // 1. GOP framebuffer
    if gop_select_mode(gop_modes, gop_mode_count, gop_framebuffer_base).is_none() {
        serial_println!("UEFI boot: GOP setup failed — continuing without framebuffer");
    }

    // 2. ACPI RSDP
    locate_rsdp(uefi_rsdp_hint);

    // 3. Memory map
    parse_uefi_memory_map(uefi_mmap_buffer, uefi_mmap_size, uefi_mmap_descriptor_size);

    // 4. Load kernel ELF
    let load_result = match load_kernel_elf(kernel_elf_data, kernel_elf_size) {
        Some(r) => r,
        None => {
            serial_println!("UEFI boot: ELF load failed");
            return false;
        }
    };

    // 5. Store boot volume label
    let label_len = boot_volume_label.len().min(63);
    BOOT_INFO.boot_volume[..label_len].copy_from_slice(&boot_volume_label[..label_len]);
    BOOT_INFO.boot_volume[label_len] = 0;

    serial_println!(
        "UEFI boot: jumping to kernel entry at 0x{:x}",
        load_result.entry_point
    );

    // 6. Jump to kernel (never returns)
    let entry: KernelEntry = core::mem::transmute(load_result.entry_point);
    entry(&BOOT_INFO as *const BootInfo)
}

// ============================================================================
// Unit-testable helpers (callable without UEFI environment)
// ============================================================================

/// Expose the current BOOT_INFO (read-only) for diagnostics / tests.
///
/// # Safety
///
/// Must only be called after at least one of the `parse_*` / `locate_*`
/// functions has been called.
pub unsafe fn boot_info_snapshot() -> BootInfo {
    BOOT_INFO
}

/// Return the number of memory regions that were parsed from the UEFI map.
pub fn parsed_region_count() -> usize {
    unsafe { MEMORY_REGION_COUNT }
}
