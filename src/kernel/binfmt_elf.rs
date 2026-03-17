/// binfmt_elf — ELF binary format loader
///
/// Parses and loads ELF64 executables and shared libraries:
///   - ELF header validation (magic, class, endianness, machine)
///   - Program header parsing (LOAD, DYNAMIC, INTERP segments)
///   - Segment mapping (RX text, RW data, BSS zero-fill)
///   - Entry point extraction
///   - Dynamic linker path extraction (PT_INTERP)
///
/// Only supports: ELF64, little-endian, x86-64 (EM_X86_64=62).
/// No mmap — segments are described by load records for the VMM to map.
///
/// Rules: no_std, no heap, no floats, no panics, saturating counters.
use crate::serial_println;

// ---------------------------------------------------------------------------
// ELF64 constants
// ---------------------------------------------------------------------------

pub const ELFMAG: [u8; 4] = [0x7F, b'E', b'L', b'F'];
pub const ELFCLASS64: u8 = 2;
pub const ELFDATA2LSB: u8 = 1; // little-endian
pub const ET_EXEC: u16 = 2; // executable
pub const ET_DYN: u16 = 3; // shared object (PIE)
pub const EM_X86_64: u16 = 62;

// Program header types
pub const PT_NULL: u32 = 0;
pub const PT_LOAD: u32 = 1;
pub const PT_DYNAMIC: u32 = 2;
pub const PT_INTERP: u32 = 3;
pub const PT_NOTE: u32 = 4;
pub const PT_GNU_STACK: u32 = 0x6474E551;
pub const PT_GNU_RELRO: u32 = 0x6474E552;

// Segment flags
pub const PF_X: u32 = 1 << 0; // execute
pub const PF_W: u32 = 1 << 1; // write
pub const PF_R: u32 = 1 << 2; // read

// ---------------------------------------------------------------------------
// ELF64 header (64 bytes)
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct Elf64Hdr {
    pub e_ident: [u8; 16],
    pub e_type: u16,
    pub e_machine: u16,
    pub e_version: u32,
    pub e_entry: u64,
    pub e_phoff: u64, // program header offset
    pub e_shoff: u64, // section header offset
    pub e_flags: u32,
    pub e_ehsize: u16,
    pub e_phentsize: u16,
    pub e_phnum: u16,
    pub e_shentsize: u16,
    pub e_shnum: u16,
    pub e_shstrndx: u16,
}

impl Elf64Hdr {
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() < 64 {
            return None;
        }
        Some(Elf64Hdr {
            e_ident: {
                let mut a = [0u8; 16];
                let mut i = 0;
                while i < 16 {
                    a[i] = b[i];
                    i += 1;
                }
                a
            },
            e_type: u16::from_le_bytes([b[16], b[17]]),
            e_machine: u16::from_le_bytes([b[18], b[19]]),
            e_version: u32::from_le_bytes([b[20], b[21], b[22], b[23]]),
            e_entry: u64::from_le_bytes([b[24], b[25], b[26], b[27], b[28], b[29], b[30], b[31]]),
            e_phoff: u64::from_le_bytes([b[32], b[33], b[34], b[35], b[36], b[37], b[38], b[39]]),
            e_shoff: u64::from_le_bytes([b[40], b[41], b[42], b[43], b[44], b[45], b[46], b[47]]),
            e_flags: u32::from_le_bytes([b[48], b[49], b[50], b[51]]),
            e_ehsize: u16::from_le_bytes([b[52], b[53]]),
            e_phentsize: u16::from_le_bytes([b[54], b[55]]),
            e_phnum: u16::from_le_bytes([b[56], b[57]]),
            e_shentsize: u16::from_le_bytes([b[58], b[59]]),
            e_shnum: u16::from_le_bytes([b[60], b[61]]),
            e_shstrndx: u16::from_le_bytes([b[62], b[63]]),
        })
    }

    pub fn is_valid_elf64_x86_64(&self) -> bool {
        self.e_ident[0..4] == ELFMAG
            && self.e_ident[4] == ELFCLASS64
            && self.e_ident[5] == ELFDATA2LSB
            && self.e_machine == EM_X86_64
            && (self.e_type == ET_EXEC || self.e_type == ET_DYN)
    }
}

// ---------------------------------------------------------------------------
// ELF64 program header (56 bytes)
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct Elf64Phdr {
    pub p_type: u32,
    pub p_flags: u32,
    pub p_offset: u64, // offset in file
    pub p_vaddr: u64,  // virtual address
    pub p_paddr: u64,  // physical address (usually == vaddr)
    pub p_filesz: u64, // size in file
    pub p_memsz: u64,  // size in memory (>= filesz; BSS = memsz - filesz)
    pub p_align: u64,
}

impl Elf64Phdr {
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() < 56 {
            return None;
        }
        Some(Elf64Phdr {
            p_type: u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            p_flags: u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
            p_offset: u64::from_le_bytes([b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]]),
            p_vaddr: u64::from_le_bytes([b[16], b[17], b[18], b[19], b[20], b[21], b[22], b[23]]),
            p_paddr: u64::from_le_bytes([b[24], b[25], b[26], b[27], b[28], b[29], b[30], b[31]]),
            p_filesz: u64::from_le_bytes([b[32], b[33], b[34], b[35], b[36], b[37], b[38], b[39]]),
            p_memsz: u64::from_le_bytes([b[40], b[41], b[42], b[43], b[44], b[45], b[46], b[47]]),
            p_align: u64::from_le_bytes([b[48], b[49], b[50], b[51], b[52], b[53], b[54], b[55]]),
        })
    }
}

// ---------------------------------------------------------------------------
// Load record: describes one segment to map
// ---------------------------------------------------------------------------

const MAX_LOAD_SEGS: usize = 8;
const INTERP_LEN: usize = 64;

#[derive(Copy, Clone)]
pub struct ElfSegment {
    pub vaddr: u64,
    pub offset: u64, // file offset
    pub filesz: u64,
    pub memsz: u64,
    pub flags: u32, // PF_R | PF_W | PF_X
    pub align: u64,
    pub valid: bool,
}

impl ElfSegment {
    pub const fn empty() -> Self {
        ElfSegment {
            vaddr: 0,
            offset: 0,
            filesz: 0,
            memsz: 0,
            flags: 0,
            align: 0,
            valid: false,
        }
    }
}

/// Result of parsing an ELF binary.
#[derive(Copy, Clone)]
pub struct ElfLoadInfo {
    pub entry: u64,
    pub segments: [ElfSegment; MAX_LOAD_SEGS],
    pub seg_count: u8,
    pub interp: [u8; INTERP_LEN], // dynamic linker path (PT_INTERP)
    pub interp_len: u8,
    pub is_pie: bool,     // ET_DYN
    pub stack_exec: bool, // PT_GNU_STACK has PF_X
}

impl ElfLoadInfo {
    pub const fn empty() -> Self {
        const ES: ElfSegment = ElfSegment::empty();
        ElfLoadInfo {
            entry: 0,
            segments: [ES; MAX_LOAD_SEGS],
            seg_count: 0,
            interp: [0u8; INTERP_LEN],
            interp_len: 0,
            is_pie: false,
            stack_exec: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ElfError {
    TooShort,
    BadMagic,
    NotX86_64,
    UnsupportedType,
    BadPhdrOffset,
    TooManySegments,
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// Parse an ELF binary from a byte slice.
/// `image` should be the full ELF file contents (or at least the headers).
pub fn elf_parse(image: &[u8]) -> Result<ElfLoadInfo, ElfError> {
    if image.len() < 64 {
        return Err(ElfError::TooShort);
    }

    let hdr = Elf64Hdr::from_bytes(image).ok_or(ElfError::TooShort)?;
    if !hdr.is_valid_elf64_x86_64() {
        if hdr.e_ident[0..4] != ELFMAG {
            return Err(ElfError::BadMagic);
        }
        if hdr.e_machine != EM_X86_64 {
            return Err(ElfError::NotX86_64);
        }
        return Err(ElfError::UnsupportedType);
    }

    let phoff = hdr.e_phoff as usize;
    let phentsize = hdr.e_phentsize as usize;
    let phnum = hdr.e_phnum as usize;

    if phoff == 0 || phentsize < 56 {
        return Err(ElfError::BadPhdrOffset);
    }

    let mut info = ElfLoadInfo::empty();
    info.entry = hdr.e_entry;
    info.is_pie = hdr.e_type == ET_DYN;

    let mut i = 0usize;
    while i < phnum {
        let off = phoff.saturating_add(i.saturating_mul(phentsize));
        if off.saturating_add(56) > image.len() {
            break;
        }
        let ph = Elf64Phdr::from_bytes(&image[off..]).ok_or(ElfError::BadPhdrOffset)?;

        match ph.p_type {
            PT_LOAD => {
                if info.seg_count as usize >= MAX_LOAD_SEGS {
                    return Err(ElfError::TooManySegments);
                }
                let idx = info.seg_count as usize;
                info.segments[idx] = ElfSegment {
                    vaddr: ph.p_vaddr,
                    offset: ph.p_offset,
                    filesz: ph.p_filesz,
                    memsz: ph.p_memsz,
                    flags: ph.p_flags,
                    align: ph.p_align,
                    valid: true,
                };
                info.seg_count = info.seg_count.saturating_add(1);
            }
            PT_INTERP => {
                let ioff = ph.p_offset as usize;
                let ilen = (ph.p_filesz as usize).min(INTERP_LEN - 1);
                if ioff.saturating_add(ilen) <= image.len() {
                    let mut k = 0usize;
                    while k < ilen {
                        info.interp[k] = image[ioff + k];
                        k = k.saturating_add(1);
                    }
                    info.interp_len = ilen as u8;
                }
            }
            PT_GNU_STACK => {
                info.stack_exec = ph.p_flags & PF_X != 0;
            }
            _ => {}
        }
        i = i.saturating_add(1);
    }

    Ok(info)
}

/// Quick check: does the first 4 bytes look like an ELF binary?
pub fn is_elf(data: &[u8]) -> bool {
    data.len() >= 4 && data[0..4] == ELFMAG
}

/// Returns the virtual address range covered by LOAD segments.
/// Returns (vaddr_min, vaddr_max) or (0,0) if no LOAD segments.
pub fn elf_load_range(info: &ElfLoadInfo) -> (u64, u64) {
    if info.seg_count == 0 {
        return (0, 0);
    }
    let mut vmin = u64::MAX;
    let mut vmax = 0u64;
    let mut i = 0usize;
    while i < info.seg_count as usize {
        let seg = &info.segments[i];
        if seg.valid && seg.p_type_is_load() {
            vmin = vmin.min(seg.vaddr);
            vmax = vmax.max(seg.vaddr.saturating_add(seg.memsz));
        }
        i = i.saturating_add(1);
    }
    if vmin == u64::MAX {
        (0, 0)
    } else {
        (vmin, vmax)
    }
}

impl ElfSegment {
    fn p_type_is_load(&self) -> bool {
        self.valid
    }
}

pub fn init() {
    serial_println!("[binfmt_elf] ELF64/x86-64 binary format loader initialized");
}
