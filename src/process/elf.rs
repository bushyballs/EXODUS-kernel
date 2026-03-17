use crate::memory::frame_allocator::FRAME_SIZE;
/// ELF64 binary loader for Genesis
///
/// Parses ELF64 headers and loads segments into process memory.
/// Supports statically-linked executables and basic relocations.
///
/// Features:
///   - Full ELF64 header validation (magic, class, endianness, arch)
///   - Program header parsing (PT_LOAD, PT_INTERP, PT_DYNAMIC, PT_NOTE, PT_PHDR)
///   - Section header parsing (symbol tables, string tables, relocation sections)
///   - Segment loading with proper R/W/X permissions
///   - Relocations (R_X86_64_RELATIVE, R_X86_64_64, R_X86_64_GLOB_DAT, R_X86_64_JUMP_SLOT)
///   - Symbol table lookup by name
///   - User stack setup (argv, envp, auxv)
///   - Auxiliary vector construction (AT_ENTRY, AT_PHDR, AT_PHNUM, AT_PAGESZ, AT_BASE)
///
/// ELF format reference: System V ABI, AMD64 supplement.
/// Inspired by: Linux load_elf_binary(), Fuchsia ELF loader. All code is original.
use crate::memory::{frame_allocator, paging};
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// ELF constants
// ---------------------------------------------------------------------------

/// ELF magic bytes: 0x7F 'E' 'L' 'F'
const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];

/// ELF class: 64-bit
const ELFCLASS64: u8 = 2;

/// ELF data encoding: little-endian
const ELFDATA2LSB: u8 = 1;

/// ELF version: current
const EV_CURRENT: u8 = 1;

/// ELF types
const ET_EXEC: u16 = 2; // Executable
const ET_DYN: u16 = 3; // Shared object (PIE executables)

/// ELF machine: AMD64
const EM_X86_64: u16 = 62;

/// Program header types
const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const PT_NOTE: u32 = 4;
const PT_PHDR: u32 = 6;
const PT_TLS: u32 = 7;
const PT_GNU_STACK: u32 = 0x6474e551;

/// Program header flags
const PF_X: u32 = 1; // Execute
const PF_W: u32 = 2; // Write
const PF_R: u32 = 4; // Read

/// Section header types
const SHT_SYMTAB: u32 = 2;
const SHT_STRTAB: u32 = 3;
const SHT_RELA: u32 = 4;
const SHT_DYNSYM: u32 = 11;

/// Relocation types (x86_64)
const R_X86_64_NONE: u32 = 0;
const R_X86_64_64: u32 = 1;
const R_X86_64_PC32: u32 = 2;
const R_X86_64_GLOB_DAT: u32 = 6;
const R_X86_64_JUMP_SLOT: u32 = 7;
const R_X86_64_RELATIVE: u32 = 8;

/// Auxiliary vector types (for the user stack)
pub const AT_NULL: u64 = 0;
pub const AT_PHDR: u64 = 3;
pub const AT_PHENT: u64 = 4;
pub const AT_PHNUM: u64 = 5;
pub const AT_PAGESZ: u64 = 6;
pub const AT_BASE: u64 = 7;
pub const AT_FLAGS: u64 = 8;
pub const AT_ENTRY: u64 = 9;
pub const AT_UID: u64 = 11;
pub const AT_EUID: u64 = 12;
pub const AT_GID: u64 = 13;
pub const AT_EGID: u64 = 14;
pub const AT_CLKTCK: u64 = 17;
pub const AT_SECURE: u64 = 23;

// ---------------------------------------------------------------------------
// ELF64 structures
// ---------------------------------------------------------------------------

/// ELF64 file header (64 bytes)
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Elf64Header {
    pub e_ident: [u8; 16], // Magic, class, encoding, version, OS/ABI, padding
    pub e_type: u16,       // Object file type
    pub e_machine: u16,    // Architecture
    pub e_version: u32,    // Object file version
    pub e_entry: u64,      // Entry point virtual address
    pub e_phoff: u64,      // Program header table offset
    pub e_shoff: u64,      // Section header table offset
    pub e_flags: u32,      // Processor-specific flags
    pub e_ehsize: u16,     // ELF header size
    pub e_phentsize: u16,  // Program header entry size
    pub e_phnum: u16,      // Number of program headers
    pub e_shentsize: u16,  // Section header entry size
    pub e_shnum: u16,      // Number of section headers
    pub e_shstrndx: u16,   // Section name string table index
}

/// ELF64 program header (56 bytes)
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Elf64ProgramHeader {
    pub p_type: u32,   // Segment type
    pub p_flags: u32,  // Segment flags
    pub p_offset: u64, // Offset in file
    pub p_vaddr: u64,  // Virtual address in memory
    pub p_paddr: u64,  // Physical address (unused)
    pub p_filesz: u64, // Size in file
    pub p_memsz: u64,  // Size in memory (>= filesz, excess is zeroed)
    pub p_align: u64,  // Alignment
}

/// ELF64 section header (64 bytes)
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Elf64SectionHeader {
    pub sh_name: u32,      // Section name (index into string table)
    pub sh_type: u32,      // Section type
    pub sh_flags: u64,     // Section flags
    pub sh_addr: u64,      // Virtual address if loaded
    pub sh_offset: u64,    // Offset in file
    pub sh_size: u64,      // Size in file
    pub sh_link: u32,      // Link to another section
    pub sh_info: u32,      // Additional info
    pub sh_addralign: u64, // Alignment
    pub sh_entsize: u64,   // Entry size (if section is a table)
}

/// ELF64 symbol table entry (24 bytes)
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Elf64Symbol {
    pub st_name: u32,  // Symbol name (index into string table)
    pub st_info: u8,   // Symbol type and binding
    pub st_other: u8,  // Symbol visibility
    pub st_shndx: u16, // Section index
    pub st_value: u64, // Symbol value (address)
    pub st_size: u64,  // Symbol size
}

impl Elf64Symbol {
    /// Get the symbol binding (STB_LOCAL, STB_GLOBAL, STB_WEAK)
    pub fn binding(&self) -> u8 {
        self.st_info >> 4
    }

    /// Get the symbol type (STT_NOTYPE, STT_FUNC, STT_OBJECT, etc.)
    pub fn sym_type(&self) -> u8 {
        self.st_info & 0xf
    }
}

/// ELF64 relocation entry with addend (24 bytes)
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Elf64Rela {
    pub r_offset: u64, // Address to apply relocation
    pub r_info: u64,   // Relocation type + symbol index
    pub r_addend: i64, // Addend
}

impl Elf64Rela {
    /// Get the symbol index from r_info
    pub fn symbol(&self) -> u32 {
        (self.r_info >> 32) as u32
    }

    /// Get the relocation type from r_info
    pub fn rel_type(&self) -> u32 {
        (self.r_info & 0xFFFFFFFF) as u32
    }
}

/// ELF64 dynamic section entry (16 bytes)
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Elf64Dyn {
    pub d_tag: i64,
    pub d_val: u64,
}

// ---------------------------------------------------------------------------
// Note segment structures
// ---------------------------------------------------------------------------

/// ELF64 note header
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Elf64Nhdr {
    pub n_namesz: u32,
    pub n_descsz: u32,
    pub n_type: u32,
}

/// Parsed note entry
#[derive(Debug, Clone)]
pub struct ElfNote {
    pub name: String,
    pub note_type: u32,
    pub desc: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Load result and error types
// ---------------------------------------------------------------------------

/// Result of loading an ELF binary
#[derive(Debug)]
pub struct ElfLoadResult {
    /// Entry point address
    pub entry: usize,
    /// Highest virtual address mapped (for setting up brk/heap)
    pub brk: usize,
    /// Virtual address where program headers were loaded (for AT_PHDR)
    pub phdr_addr: usize,
    /// Number of program headers (for AT_PHNUM)
    pub phnum: usize,
    /// Size of each program header entry (for AT_PHENT)
    pub phent: usize,
    /// Base address for PIE/shared objects (0 for ET_EXEC)
    pub base_addr: usize,
    /// Interpreter path (if PT_INTERP present)
    pub interp: Option<String>,
    /// Whether the ELF requests an executable stack
    pub executable_stack: bool,
}

/// Errors that can occur during ELF loading
#[derive(Debug)]
pub enum ElfError {
    InvalidMagic,
    NotElf64,
    NotLittleEndian,
    NotExecutable,
    WrongArchitecture,
    NoSegments,
    MemoryAllocationFailed,
    MappingFailed,
    InvalidVersion,
    TooSmall,
    InvalidProgramHeader,
    InvalidSectionHeader,
    SymbolNotFound,
    InvalidRelocation,
    InvalidStringTable,
}

// ---------------------------------------------------------------------------
// Parsed ELF information
// ---------------------------------------------------------------------------

/// Fully parsed ELF file information
pub struct ElfInfo {
    /// ELF type (ET_EXEC, ET_DYN)
    pub elf_type: u16,
    /// Entry point
    pub entry: u64,
    /// Program headers
    pub program_headers: Vec<ProgramHeaderInfo>,
    /// Section headers
    pub section_headers: Vec<SectionHeaderInfo>,
    /// Interpreter path (from PT_INTERP)
    pub interp: Option<String>,
    /// Notes (from PT_NOTE)
    pub notes: Vec<ElfNote>,
    /// Whether GNU_STACK says the stack is executable
    pub executable_stack: bool,
}

/// Parsed program header
#[derive(Debug, Clone)]
pub struct ProgramHeaderInfo {
    pub p_type: u32,
    pub flags: u32,
    pub offset: u64,
    pub vaddr: u64,
    pub paddr: u64,
    pub filesz: u64,
    pub memsz: u64,
    pub align: u64,
}

/// Parsed section header
#[derive(Debug, Clone)]
pub struct SectionHeaderInfo {
    pub name: String,
    pub sh_type: u32,
    pub flags: u64,
    pub addr: u64,
    pub offset: u64,
    pub size: u64,
    pub link: u32,
    pub info: u32,
    pub addralign: u64,
    pub entsize: u64,
}

/// A resolved symbol
#[derive(Debug, Clone)]
pub struct ResolvedSymbol {
    pub name: String,
    pub value: u64,
    pub size: u64,
    pub binding: u8,
    pub sym_type: u8,
    pub section_index: u16,
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Validate an ELF64 header thoroughly
fn validate_header(elf_data: &[u8]) -> Result<&Elf64Header, ElfError> {
    if elf_data.len() < core::mem::size_of::<Elf64Header>() {
        return Err(ElfError::TooSmall);
    }

    let header = unsafe { &*(elf_data.as_ptr() as *const Elf64Header) };

    if header.e_ident[0..4] != ELF_MAGIC {
        return Err(ElfError::InvalidMagic);
    }

    if header.e_ident[4] != ELFCLASS64 {
        return Err(ElfError::NotElf64);
    }

    if header.e_ident[5] != ELFDATA2LSB {
        return Err(ElfError::NotLittleEndian);
    }

    if header.e_ident[6] != EV_CURRENT {
        return Err(ElfError::InvalidVersion);
    }

    if header.e_type != ET_EXEC && header.e_type != ET_DYN {
        return Err(ElfError::NotExecutable);
    }

    if header.e_machine != EM_X86_64 {
        return Err(ElfError::WrongArchitecture);
    }

    if header.e_version != EV_CURRENT as u32 {
        return Err(ElfError::InvalidVersion);
    }

    Ok(header)
}

// ---------------------------------------------------------------------------
// Program header parsing
// ---------------------------------------------------------------------------

/// Parse all program headers from an ELF file
fn parse_program_headers(
    elf_data: &[u8],
    header: &Elf64Header,
) -> Result<Vec<ProgramHeaderInfo>, ElfError> {
    let phoff = header.e_phoff as usize;
    let phentsize = header.e_phentsize as usize;
    let phnum = header.e_phnum as usize;

    if phnum == 0 {
        return Ok(Vec::new());
    }

    let mut headers = Vec::new();

    for i in 0..phnum {
        let offset = phoff + i * phentsize;
        if offset + phentsize > elf_data.len() {
            return Err(ElfError::InvalidProgramHeader);
        }

        let phdr = unsafe { &*(elf_data.as_ptr().add(offset) as *const Elf64ProgramHeader) };

        headers.push(ProgramHeaderInfo {
            p_type: phdr.p_type,
            flags: phdr.p_flags,
            offset: phdr.p_offset,
            vaddr: phdr.p_vaddr,
            paddr: phdr.p_paddr,
            filesz: phdr.p_filesz,
            memsz: phdr.p_memsz,
            align: phdr.p_align,
        });
    }

    Ok(headers)
}

/// Extract the interpreter path from a PT_INTERP segment
fn parse_interp(elf_data: &[u8], phdr: &ProgramHeaderInfo) -> Option<String> {
    let offset = phdr.offset as usize;
    let size = phdr.filesz as usize;
    if offset + size > elf_data.len() || size == 0 {
        return None;
    }

    let bytes = &elf_data[offset..offset + size];
    let len = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let s = core::str::from_utf8(&bytes[..len]).ok()?;
    Some(String::from(s))
}

/// Parse notes from a PT_NOTE segment
fn parse_notes(elf_data: &[u8], phdr: &ProgramHeaderInfo) -> Vec<ElfNote> {
    let mut notes = Vec::new();
    let offset = phdr.offset as usize;
    let size = phdr.filesz as usize;

    if offset + size > elf_data.len() {
        return notes;
    }

    let note_data = &elf_data[offset..offset + size];
    let mut pos = 0;

    while pos + 12 <= note_data.len() {
        let nhdr = unsafe { &*(note_data.as_ptr().add(pos) as *const Elf64Nhdr) };
        pos += 12;

        let namesz = nhdr.n_namesz as usize;
        let descsz = nhdr.n_descsz as usize;

        let name_aligned = (namesz + 3) & !3;
        let desc_aligned = (descsz + 3) & !3;

        if pos + name_aligned + desc_aligned > note_data.len() {
            break;
        }

        let name_bytes = &note_data[pos..pos + namesz];
        let name_len = name_bytes.iter().position(|&b| b == 0).unwrap_or(namesz);
        let name = core::str::from_utf8(&name_bytes[..name_len]).unwrap_or("?");
        pos += name_aligned;

        let desc = note_data[pos..pos + descsz].to_vec();
        pos += desc_aligned;

        notes.push(ElfNote {
            name: String::from(name),
            note_type: nhdr.n_type,
            desc,
        });
    }

    notes
}

// ---------------------------------------------------------------------------
// Section header parsing
// ---------------------------------------------------------------------------

/// Get a section header by index
fn get_section_header<'a>(
    elf_data: &'a [u8],
    header: &Elf64Header,
    index: u16,
) -> Option<&'a Elf64SectionHeader> {
    let shoff = header.e_shoff as usize;
    let shentsize = header.e_shentsize as usize;

    if shentsize == 0 || shoff == 0 {
        return None;
    }

    let offset = shoff + (index as usize) * shentsize;
    if offset + shentsize > elf_data.len() {
        return None;
    }

    Some(unsafe { &*(elf_data.as_ptr().add(offset) as *const Elf64SectionHeader) })
}

/// Read a null-terminated string from a string table section
fn read_string_from_strtab(
    elf_data: &[u8],
    strtab_offset: usize,
    strtab_size: usize,
    name_offset: u32,
) -> String {
    let start = strtab_offset + name_offset as usize;
    if start >= elf_data.len() || start >= strtab_offset + strtab_size {
        return String::from("");
    }

    let end = strtab_offset + strtab_size;
    let max_end = core::cmp::min(end, elf_data.len());
    let bytes = &elf_data[start..max_end];
    let len = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    core::str::from_utf8(&bytes[..len])
        .map(String::from)
        .unwrap_or_else(|_| String::from("?"))
}

/// Parse all section headers
fn parse_section_headers(elf_data: &[u8], header: &Elf64Header) -> Vec<SectionHeaderInfo> {
    let shoff = header.e_shoff as usize;
    let shentsize = header.e_shentsize as usize;
    let shnum = header.e_shnum as usize;
    let shstrndx = header.e_shstrndx;

    if shoff == 0 || shentsize == 0 || shnum == 0 {
        return Vec::new();
    }

    let shstrtab = get_section_header(elf_data, header, shstrndx);
    let (strtab_offset, strtab_size) = shstrtab
        .map(|s| (s.sh_offset as usize, s.sh_size as usize))
        .unwrap_or((0, 0));

    let mut sections = Vec::new();

    for i in 0..shnum {
        let offset = shoff + i * shentsize;
        if offset + shentsize > elf_data.len() {
            break;
        }

        let shdr = unsafe { &*(elf_data.as_ptr().add(offset) as *const Elf64SectionHeader) };

        let name = if strtab_offset > 0 {
            read_string_from_strtab(elf_data, strtab_offset, strtab_size, shdr.sh_name)
        } else {
            String::from("")
        };

        sections.push(SectionHeaderInfo {
            name,
            sh_type: shdr.sh_type,
            flags: shdr.sh_flags,
            addr: shdr.sh_addr,
            offset: shdr.sh_offset,
            size: shdr.sh_size,
            link: shdr.sh_link,
            info: shdr.sh_info,
            addralign: shdr.sh_addralign,
            entsize: shdr.sh_entsize,
        });
    }

    sections
}

// ---------------------------------------------------------------------------
// Symbol table operations
// ---------------------------------------------------------------------------

/// Look up a symbol by name in a symbol table section.
pub fn lookup_symbol(
    elf_data: &[u8],
    symtab_offset: usize,
    symtab_size: usize,
    strtab_offset: usize,
    strtab_size: usize,
    name: &str,
) -> Option<ResolvedSymbol> {
    let entry_size = core::mem::size_of::<Elf64Symbol>();
    let num_symbols = symtab_size / entry_size;

    for i in 0..num_symbols {
        let offset = symtab_offset + i * entry_size;
        if offset + entry_size > elf_data.len() {
            break;
        }

        let sym = unsafe { &*(elf_data.as_ptr().add(offset) as *const Elf64Symbol) };

        let sym_name = read_string_from_strtab(elf_data, strtab_offset, strtab_size, sym.st_name);
        if sym_name.as_str() == name {
            return Some(ResolvedSymbol {
                name: sym_name,
                value: sym.st_value,
                size: sym.st_size,
                binding: sym.binding(),
                sym_type: sym.sym_type(),
                section_index: sym.st_shndx,
            });
        }
    }

    None
}

/// List all symbols in a symbol table (for debugging)
pub fn list_symbols(
    elf_data: &[u8],
    symtab_offset: usize,
    symtab_size: usize,
    strtab_offset: usize,
    strtab_size: usize,
) -> Vec<ResolvedSymbol> {
    let entry_size = core::mem::size_of::<Elf64Symbol>();
    let num_symbols = symtab_size / entry_size;
    let mut result = Vec::new();

    for i in 0..num_symbols {
        let offset = symtab_offset + i * entry_size;
        if offset + entry_size > elf_data.len() {
            break;
        }

        let sym = unsafe { &*(elf_data.as_ptr().add(offset) as *const Elf64Symbol) };

        if sym.st_name == 0 && sym.st_value == 0 {
            continue;
        }

        let sym_name = read_string_from_strtab(elf_data, strtab_offset, strtab_size, sym.st_name);

        result.push(ResolvedSymbol {
            name: sym_name,
            value: sym.st_value,
            size: sym.st_size,
            binding: sym.binding(),
            sym_type: sym.sym_type(),
            section_index: sym.st_shndx,
        });
    }

    result
}

/// Find the symbol table and its associated string table in the section headers
pub fn find_symtab(sections: &[SectionHeaderInfo]) -> Option<(usize, usize, usize, usize)> {
    for sec in sections.iter() {
        if sec.sh_type == SHT_SYMTAB {
            let link = sec.link as usize;
            if link < sections.len() {
                let strtab = &sections[link];
                return Some((
                    sec.offset as usize,
                    sec.size as usize,
                    strtab.offset as usize,
                    strtab.size as usize,
                ));
            }
        }
    }
    None
}

/// Find the dynamic symbol table and its string table
pub fn find_dynsym(sections: &[SectionHeaderInfo]) -> Option<(usize, usize, usize, usize)> {
    for sec in sections.iter() {
        if sec.sh_type == SHT_DYNSYM {
            let link = sec.link as usize;
            if link < sections.len() {
                let strtab = &sections[link];
                return Some((
                    sec.offset as usize,
                    sec.size as usize,
                    strtab.offset as usize,
                    strtab.size as usize,
                ));
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Relocations
// ---------------------------------------------------------------------------

/// Apply relocations from a RELA section.
///
/// `base_addr`: load bias (0 for ET_EXEC, base address for ET_DYN)
pub fn apply_relocations(
    elf_data: &[u8],
    base_addr: usize,
    rela_offset: usize,
    rela_size: usize,
    symtab_offset: usize,
    _symtab_size: usize,
) -> Result<usize, ElfError> {
    let entry_size = core::mem::size_of::<Elf64Rela>();
    let num_relas = rela_size / entry_size;
    let sym_entry_size = core::mem::size_of::<Elf64Symbol>();
    let mut applied = 0;

    for i in 0..num_relas {
        let offset = rela_offset + i * entry_size;
        if offset + entry_size > elf_data.len() {
            break;
        }

        let rela = unsafe { &*(elf_data.as_ptr().add(offset) as *const Elf64Rela) };

        let rel_type = rela.rel_type();
        let sym_idx = rela.symbol() as usize;
        let target_addr = base_addr + rela.r_offset as usize;

        let sym_value = if sym_idx > 0 && symtab_offset > 0 {
            let sym_offset = symtab_offset + sym_idx * sym_entry_size;
            if sym_offset + sym_entry_size <= elf_data.len() {
                let sym = unsafe { &*(elf_data.as_ptr().add(sym_offset) as *const Elf64Symbol) };
                sym.st_value as usize + base_addr
            } else {
                0
            }
        } else {
            0
        };

        match rel_type {
            R_X86_64_NONE => {}
            R_X86_64_RELATIVE => {
                let value = base_addr as u64 + rela.r_addend as u64;
                unsafe {
                    *(target_addr as *mut u64) = value;
                }
                applied += 1;
            }
            R_X86_64_64 => {
                let value = sym_value as u64 + rela.r_addend as u64;
                unsafe {
                    *(target_addr as *mut u64) = value;
                }
                applied += 1;
            }
            R_X86_64_GLOB_DAT | R_X86_64_JUMP_SLOT => {
                unsafe {
                    *(target_addr as *mut u64) = sym_value as u64;
                }
                applied += 1;
            }
            R_X86_64_PC32 => {
                let value = (sym_value as i64 + rela.r_addend - target_addr as i64) as u32;
                unsafe {
                    *(target_addr as *mut u32) = value;
                }
                applied += 1;
            }
            _ => {
                crate::serial_println!("  ELF: unknown relocation type {}", rel_type);
            }
        }
    }

    Ok(applied)
}

// ---------------------------------------------------------------------------
// Full ELF parsing
// ---------------------------------------------------------------------------

/// Parse an ELF file completely (headers, sections, notes, interp)
pub fn parse(elf_data: &[u8]) -> Result<ElfInfo, ElfError> {
    let header = validate_header(elf_data)?;
    let program_headers = parse_program_headers(elf_data, header)?;
    let section_headers = parse_section_headers(elf_data, header);

    let mut interp = None;
    let mut notes = Vec::new();
    let mut executable_stack = false;

    for phdr in &program_headers {
        match phdr.p_type {
            PT_INTERP => {
                interp = parse_interp(elf_data, phdr);
            }
            PT_NOTE => {
                let mut n = parse_notes(elf_data, phdr);
                notes.append(&mut n);
            }
            PT_GNU_STACK => {
                executable_stack = (phdr.flags & PF_X) != 0;
            }
            _ => {}
        }
    }

    Ok(ElfInfo {
        elf_type: header.e_type,
        entry: header.e_entry,
        program_headers,
        section_headers,
        interp,
        notes,
        executable_stack,
    })
}

// ---------------------------------------------------------------------------
// Segment loading
// ---------------------------------------------------------------------------

/// Load an ELF64 binary from a byte buffer into the current address space.
///
/// Maps all PT_LOAD segments to their specified virtual addresses,
/// allocating physical frames and creating page table mappings.
///
/// Returns the entry point and break address on success.
pub fn load(elf_data: &[u8]) -> Result<ElfLoadResult, ElfError> {
    let header = validate_header(elf_data)?;

    let phoff = header.e_phoff as usize;
    let phentsize = header.e_phentsize as usize;
    let phnum = header.e_phnum as usize;

    if phnum == 0 {
        return Err(ElfError::NoSegments);
    }

    let mut highest_addr: usize = 0;
    let mut phdr_addr: usize = 0;
    let mut interp: Option<String> = None;
    let mut executable_stack = false;
    let mut has_load_segment = false;

    // First pass: parse metadata segments
    for i in 0..phnum {
        let ph_offset = phoff + i * phentsize;
        if ph_offset + phentsize > elf_data.len() {
            continue;
        }

        let phdr = unsafe { &*(elf_data.as_ptr().add(ph_offset) as *const Elf64ProgramHeader) };

        match phdr.p_type {
            PT_INTERP => {
                let offset = phdr.p_offset as usize;
                let size = phdr.p_filesz as usize;
                if offset + size <= elf_data.len() && size > 0 {
                    let bytes = &elf_data[offset..offset + size];
                    let len = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
                    if let Ok(s) = core::str::from_utf8(&bytes[..len]) {
                        interp = Some(String::from(s));
                    }
                }
            }
            PT_GNU_STACK => {
                executable_stack = (phdr.p_flags & PF_X) != 0;
            }
            PT_PHDR => {
                phdr_addr = phdr.p_vaddr as usize;
            }
            _ => {}
        }
    }

    // Second pass: load PT_LOAD segments
    for i in 0..phnum {
        let ph_offset = phoff + i * phentsize;
        if ph_offset + phentsize > elf_data.len() {
            continue;
        }

        let phdr = unsafe { &*(elf_data.as_ptr().add(ph_offset) as *const Elf64ProgramHeader) };

        if phdr.p_type != PT_LOAD {
            continue;
        }

        has_load_segment = true;
        let vaddr = phdr.p_vaddr as usize;
        let memsz = phdr.p_memsz as usize;
        let filesz = phdr.p_filesz as usize;
        let file_offset = phdr.p_offset as usize;

        // Determine page flags
        let mut page_flags = paging::flags::USER_ACCESSIBLE;
        if phdr.p_flags & PF_W != 0 {
            page_flags |= paging::flags::WRITABLE;
        }
        if phdr.p_flags & PF_X == 0 {
            page_flags |= paging::flags::NO_EXECUTE;
        }

        // Map pages for this segment
        let page_start = vaddr & !0xFFF;
        let page_end = (vaddr + memsz + 0xFFF) & !0xFFF;

        for page_addr in (page_start..page_end).step_by(FRAME_SIZE) {
            let frame =
                frame_allocator::allocate_frame().ok_or(ElfError::MemoryAllocationFailed)?;

            // Zero the frame first
            unsafe {
                core::ptr::write_bytes(frame.addr as *mut u8, 0, FRAME_SIZE);
            }

            paging::map_page(page_addr, frame.addr, page_flags)
                .map_err(|_| ElfError::MappingFailed)?;

            // Copy file data into this page if applicable
            let page_offset_in_segment = if page_addr >= vaddr {
                page_addr - vaddr
            } else {
                0
            };

            if page_offset_in_segment < filesz {
                let src_start = file_offset + page_offset_in_segment;
                let copy_len = core::cmp::min(FRAME_SIZE, filesz - page_offset_in_segment);

                if src_start + copy_len <= elf_data.len() {
                    let dest_offset = if page_addr < vaddr {
                        vaddr - page_addr
                    } else {
                        0
                    };

                    unsafe {
                        let dest = (frame.addr + dest_offset) as *mut u8;
                        let src = elf_data.as_ptr().add(src_start);
                        core::ptr::copy_nonoverlapping(src, dest, copy_len);
                    }
                }
            }
        }

        let segment_end = vaddr + memsz;
        if segment_end > highest_addr {
            highest_addr = segment_end;
        }
    }

    if !has_load_segment {
        return Err(ElfError::NoSegments);
    }

    Ok(ElfLoadResult {
        entry: header.e_entry as usize,
        brk: (highest_addr + 0xFFF) & !0xFFF,
        phdr_addr,
        phnum,
        phent: phentsize,
        base_addr: 0,
        interp,
        executable_stack,
    })
}

// ---------------------------------------------------------------------------
// Stack setup for loaded ELF (argv, envp, auxv)
// ---------------------------------------------------------------------------

/// Auxiliary vector entry (key-value pair pushed onto user stack)
pub struct AuxvEntry {
    pub key: u64,
    pub value: u64,
}

/// Build the auxiliary vector for a loaded ELF binary
pub fn build_auxv(load_result: &ElfLoadResult, uid: u32, gid: u32) -> Vec<AuxvEntry> {
    let mut auxv = Vec::new();

    auxv.push(AuxvEntry {
        key: AT_PHDR,
        value: load_result.phdr_addr as u64,
    });
    auxv.push(AuxvEntry {
        key: AT_PHENT,
        value: load_result.phent as u64,
    });
    auxv.push(AuxvEntry {
        key: AT_PHNUM,
        value: load_result.phnum as u64,
    });
    auxv.push(AuxvEntry {
        key: AT_PAGESZ,
        value: FRAME_SIZE as u64,
    });
    auxv.push(AuxvEntry {
        key: AT_BASE,
        value: load_result.base_addr as u64,
    });
    auxv.push(AuxvEntry {
        key: AT_FLAGS,
        value: 0,
    });
    auxv.push(AuxvEntry {
        key: AT_ENTRY,
        value: load_result.entry as u64,
    });
    auxv.push(AuxvEntry {
        key: AT_UID,
        value: uid as u64,
    });
    auxv.push(AuxvEntry {
        key: AT_EUID,
        value: uid as u64,
    });
    auxv.push(AuxvEntry {
        key: AT_GID,
        value: gid as u64,
    });
    auxv.push(AuxvEntry {
        key: AT_EGID,
        value: gid as u64,
    });
    auxv.push(AuxvEntry {
        key: AT_CLKTCK,
        value: 100,
    });
    auxv.push(AuxvEntry {
        key: AT_SECURE,
        value: 0,
    });
    auxv.push(AuxvEntry {
        key: AT_NULL,
        value: 0,
    });

    auxv
}

/// Set up the user stack for a loaded process.
///
/// The System V x86_64 ABI specifies this stack layout (growing downward):
///
///   [padding for alignment]
///   [environment strings]
///   [argument strings]
///   [auxv: AT_NULL sentinel]
///   [auxv entries...]
///   [NULL sentinel for envp]
///   [envp pointers]
///   [NULL sentinel for argv]
///   [argv pointers]
///   [argc]              <-- RSP points here on entry
///
/// Returns the new stack pointer value.
///
/// SAFETY: `stack_top` must point to valid mapped memory with enough space.
pub unsafe fn setup_user_stack(
    stack_top: usize,
    argv: &[&str],
    envp: &[&str],
    auxv: &[AuxvEntry],
) -> usize {
    let mut sp = stack_top;

    // 1. Write argument and environment strings onto the stack.
    let mut arg_ptrs = Vec::new();
    for &arg in argv.iter().rev() {
        let bytes = arg.as_bytes();
        sp -= bytes.len() + 1;
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), sp as *mut u8, bytes.len());
        *((sp + bytes.len()) as *mut u8) = 0;
        arg_ptrs.push(sp as u64);
    }
    arg_ptrs.reverse();

    let mut env_ptrs = Vec::new();
    for &env in envp.iter().rev() {
        let bytes = env.as_bytes();
        sp -= bytes.len() + 1;
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), sp as *mut u8, bytes.len());
        *((sp + bytes.len()) as *mut u8) = 0;
        env_ptrs.push(sp as u64);
    }
    env_ptrs.reverse();

    // 2. Align stack to 16 bytes
    sp &= !0xF;

    // 3. Calculate total items for alignment
    let total_items = 1 + argv.len() + 1 + envp.len() + 1 + auxv.len() * 2;
    if total_items % 2 != 0 {
        sp -= 8;
        *(sp as *mut u64) = 0;
    }

    // 4. Push auxiliary vector (reverse order, AT_NULL last pushed = first on stack)
    for aux in auxv.iter().rev() {
        sp -= 8;
        *(sp as *mut u64) = aux.value;
        sp -= 8;
        *(sp as *mut u64) = aux.key;
    }

    // 5. Push envp NULL sentinel
    sp -= 8;
    *(sp as *mut u64) = 0;

    // 6. Push envp pointers
    for ptr in env_ptrs.iter().rev() {
        sp -= 8;
        *(sp as *mut u64) = *ptr;
    }

    // 7. Push argv NULL sentinel
    sp -= 8;
    *(sp as *mut u64) = 0;

    // 8. Push argv pointers
    for ptr in arg_ptrs.iter().rev() {
        sp -= 8;
        *(sp as *mut u64) = *ptr;
    }

    // 9. Push argc
    sp -= 8;
    *(sp as *mut u64) = argv.len() as u64;

    sp
}
