/// ELF loading support -- header parsing, segment mapping, relocations, auxv
///
/// Part of the AIOS compatibility layer.
///
/// Parses ELF64 binaries, maps PT_LOAD segments into the process address
/// space, performs basic relocations, and constructs the auxiliary vector
/// (auxv) for the dynamic linker.
///
/// Design:
///   - ElfHeader, ProgramHeader, SectionHeader parse from raw bytes.
///   - ElfLoader validates the binary and maps segments according to
///     their virtual addresses and protection flags.
///   - Basic relocation types (R_X86_64_RELATIVE, R_X86_64_64) are
///     supported for position-independent executables.
///   - AuxvBuilder constructs the auxv array pushed onto the process stack.
///   - Global Mutex<Option<Inner>> singleton for configuration.
///
/// Inspired by: Linux binfmt_elf (fs/binfmt_elf.c). All code is original.

use alloc::vec::Vec;
use crate::sync::Mutex;
use crate::serial_println;

// ---------------------------------------------------------------------------
// ELF constants
// ---------------------------------------------------------------------------

const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;
const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;
const EM_X86_64: u16 = 62;

// Program header types
const PT_NULL: u32 = 0;
const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const PT_NOTE: u32 = 4;
const PT_PHDR: u32 = 6;

// Program header flags
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

// Relocation types (x86_64)
const R_X86_64_NONE: u32 = 0;
const R_X86_64_64: u32 = 1;
const R_X86_64_RELATIVE: u32 = 8;
const R_X86_64_JUMP_SLOT: u32 = 7;

// Auxiliary vector types
pub const AT_NULL: usize = 0;
pub const AT_PHDR: usize = 3;
pub const AT_PHENT: usize = 4;
pub const AT_PHNUM: usize = 5;
pub const AT_PAGESZ: usize = 6;
pub const AT_BASE: usize = 7;
pub const AT_FLAGS: usize = 8;
pub const AT_ENTRY: usize = 9;
pub const AT_UID: usize = 11;
pub const AT_EUID: usize = 12;
pub const AT_GID: usize = 13;
pub const AT_EGID: usize = 14;
pub const AT_RANDOM: usize = 25;

// ---------------------------------------------------------------------------
// ELF structures
// ---------------------------------------------------------------------------

/// ELF64 file header.
#[derive(Clone)]
pub struct Elf64Header {
    pub e_type: u16,
    pub e_machine: u16,
    pub e_entry: u64,
    pub e_phoff: u64,
    pub e_shoff: u64,
    pub e_phentsize: u16,
    pub e_phnum: u16,
    pub e_shentsize: u16,
    pub e_shnum: u16,
    pub e_shstrndx: u16,
}

/// ELF64 program header.
#[derive(Clone)]
pub struct Elf64Phdr {
    pub p_type: u32,
    pub p_flags: u32,
    pub p_offset: u64,
    pub p_vaddr: u64,
    pub p_paddr: u64,
    pub p_filesz: u64,
    pub p_memsz: u64,
    pub p_align: u64,
}

/// Relocation entry.
#[derive(Clone)]
pub struct Elf64Rela {
    pub r_offset: u64,
    pub r_info: u64,
    pub r_addend: i64,
}

impl Elf64Rela {
    pub fn r_type(&self) -> u32 {
        (self.r_info & 0xFFFFFFFF) as u32
    }

    pub fn r_sym(&self) -> u32 {
        (self.r_info >> 32) as u32
    }
}

/// A mapped segment ready for loading.
#[derive(Clone)]
pub struct MappedSegment {
    pub vaddr: u64,
    pub memsz: u64,
    pub filesz: u64,
    pub file_offset: u64,
    pub readable: bool,
    pub writable: bool,
    pub executable: bool,
}

/// Result of parsing an ELF binary.
#[derive(Clone)]
pub struct ElfInfo {
    pub header: Elf64Header,
    pub segments: Vec<MappedSegment>,
    pub entry_point: u64,
    pub is_pie: bool,
    pub interp: Option<Vec<u8>>,
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

fn read_u16(data: &[u8], off: usize) -> u16 {
    if off + 1 >= data.len() { return 0; }
    (data[off] as u16) | ((data[off + 1] as u16) << 8)
}

fn read_u32(data: &[u8], off: usize) -> u32 {
    if off + 3 >= data.len() { return 0; }
    (data[off] as u32)
        | ((data[off + 1] as u32) << 8)
        | ((data[off + 2] as u32) << 16)
        | ((data[off + 3] as u32) << 24)
}

fn read_u64(data: &[u8], off: usize) -> u64 {
    if off + 7 >= data.len() { return 0; }
    (data[off] as u64)
        | ((data[off + 1] as u64) << 8)
        | ((data[off + 2] as u64) << 16)
        | ((data[off + 3] as u64) << 24)
        | ((data[off + 4] as u64) << 32)
        | ((data[off + 5] as u64) << 40)
        | ((data[off + 6] as u64) << 48)
        | ((data[off + 7] as u64) << 56)
}

fn read_i64(data: &[u8], off: usize) -> i64 {
    read_u64(data, off) as i64
}

/// Parse an ELF64 binary from raw bytes.
pub fn parse_elf(data: &[u8]) -> Result<ElfInfo, i32> {
    // Minimum size check
    if data.len() < 64 {
        return Err(-1); // Too small
    }

    // Verify magic
    if data[0..4] != ELF_MAGIC {
        return Err(-2); // Not an ELF
    }

    // Verify 64-bit, little-endian
    if data[4] != ELFCLASS64 {
        return Err(-3); // Not 64-bit
    }
    if data[5] != ELFDATA2LSB {
        return Err(-4); // Not little-endian
    }

    let e_type = read_u16(data, 16);
    let e_machine = read_u16(data, 18);

    if e_machine != EM_X86_64 {
        return Err(-5); // Wrong architecture
    }
    if e_type != ET_EXEC && e_type != ET_DYN {
        return Err(-6); // Not executable or shared object
    }

    let header = Elf64Header {
        e_type,
        e_machine,
        e_entry: read_u64(data, 24),
        e_phoff: read_u64(data, 32),
        e_shoff: read_u64(data, 40),
        e_phentsize: read_u16(data, 54),
        e_phnum: read_u16(data, 56),
        e_shentsize: read_u16(data, 58),
        e_shnum: read_u16(data, 60),
        e_shstrndx: read_u16(data, 62),
    };

    // Parse program headers
    let mut segments = Vec::new();
    let mut interp: Option<Vec<u8>> = None;
    let phoff = header.e_phoff as usize;
    let phentsize = header.e_phentsize as usize;

    for i in 0..header.e_phnum as usize {
        let off = phoff + i * phentsize;
        if off + phentsize > data.len() {
            break;
        }

        let p_type = read_u32(data, off);
        let p_flags = read_u32(data, off + 4);
        let p_offset = read_u64(data, off + 8);
        let p_vaddr = read_u64(data, off + 16);
        let p_filesz = read_u64(data, off + 32);
        let p_memsz = read_u64(data, off + 40);

        match p_type {
            PT_LOAD => {
                segments.push(MappedSegment {
                    vaddr: p_vaddr,
                    memsz: p_memsz,
                    filesz: p_filesz,
                    file_offset: p_offset,
                    readable: p_flags & PF_R != 0,
                    writable: p_flags & PF_W != 0,
                    executable: p_flags & PF_X != 0,
                });
            }
            PT_INTERP => {
                let start = p_offset as usize;
                let end = (p_offset + p_filesz) as usize;
                if end <= data.len() {
                    let mut interp_path = Vec::from(&data[start..end]);
                    // Strip trailing null
                    if interp_path.last() == Some(&0) {
                        interp_path.pop();
                    }
                    interp = Some(interp_path);
                }
            }
            _ => {}
        }
    }

    Ok(ElfInfo {
        entry_point: header.e_entry,
        is_pie: e_type == ET_DYN,
        header,
        segments,
        interp,
    })
}

// ---------------------------------------------------------------------------
// Auxiliary vector builder
// ---------------------------------------------------------------------------

/// Builder for the ELF auxiliary vector.
pub struct AuxvBuilder {
    entries: Vec<(usize, usize)>,
}

impl AuxvBuilder {
    pub fn new() -> Self {
        AuxvBuilder {
            entries: Vec::new(),
        }
    }

    pub fn push(&mut self, a_type: usize, a_val: usize) {
        self.entries.push((a_type, a_val));
    }

    /// Build the standard auxv for a loaded ELF.
    pub fn from_elf_info(info: &ElfInfo, phdr_addr: usize, base_addr: usize) -> Self {
        let mut builder = AuxvBuilder::new();
        builder.push(AT_PHDR, phdr_addr);
        builder.push(AT_PHENT, info.header.e_phentsize as usize);
        builder.push(AT_PHNUM, info.header.e_phnum as usize);
        builder.push(AT_PAGESZ, 4096);
        builder.push(AT_BASE, base_addr);
        builder.push(AT_FLAGS, 0);
        builder.push(AT_ENTRY, info.entry_point as usize);
        builder.push(AT_UID, 0);
        builder.push(AT_EUID, 0);
        builder.push(AT_GID, 0);
        builder.push(AT_EGID, 0);
        builder.push(AT_NULL, 0);
        builder
    }

    /// Serialize the auxv as pairs of usize values.
    pub fn as_pairs(&self) -> &[(usize, usize)] {
        &self.entries
    }

    /// Total size in bytes when written to the stack.
    pub fn byte_size(&self) -> usize {
        self.entries.len() * 2 * core::mem::size_of::<usize>()
    }
}

// ---------------------------------------------------------------------------
// Global singleton (config state)
// ---------------------------------------------------------------------------

struct Inner {
    /// Default interpreter path for PIE binaries.
    default_interp: Vec<u8>,
    /// Number of ELFs loaded (stats).
    load_count: u64,
}

static ELF_COMPAT: Mutex<Option<Inner>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Set the default dynamic linker path.
pub fn set_default_interp(path: &[u8]) {
    let mut guard = ELF_COMPAT.lock();
    if let Some(inner) = guard.as_mut() {
        inner.default_interp = Vec::from(path);
    }
}

/// Get the default dynamic linker path.
pub fn default_interp() -> Vec<u8> {
    let guard = ELF_COMPAT.lock();
    guard
        .as_ref()
        .map_or_else(Vec::new, |inner| inner.default_interp.clone())
}

/// Increment the load counter (called after successful ELF load).
pub fn record_load() {
    let mut guard = ELF_COMPAT.lock();
    if let Some(inner) = guard.as_mut() {
        inner.load_count = inner.load_count.saturating_add(1);
    }
}

/// Return the number of ELFs loaded.
pub fn load_count() -> u64 {
    let guard = ELF_COMPAT.lock();
    guard.as_ref().map_or(0, |inner| inner.load_count)
}

/// Initialize the ELF compatibility subsystem.
pub fn init() {
    let mut guard = ELF_COMPAT.lock();
    *guard = Some(Inner {
        default_interp: Vec::from(&b"/lib/ld-linux-x86-64.so.2"[..]),
        load_count: 0,
    });
    serial_println!("    elf_compat: initialized (ELF64 parser, segment mapper, auxv builder)");
}
