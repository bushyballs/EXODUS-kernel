use crate::sync::Mutex;
/// Dynamic linker stubs for Genesis
///
/// Handles ELF INTERP (PT_INTERP) segment processing, GOT/PLT relocation,
/// symbol resolution, and shared library loading.
///
/// In a mature OS this would be /lib/ld-genesis.so.1 running in userspace.
/// For now, these are kernel-side stubs that demonstrate the architecture.
///
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

// ── ELF constants ────────────────────────────────────────────────────────────

/// ELF magic bytes
const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];

/// ELF class
const ELFCLASS64: u8 = 2;

/// Program header types
const PT_NULL: u32 = 0;
const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const PT_NOTE: u32 = 4;
const PT_PHDR: u32 = 6;
const PT_GNU_EH_FRAME: u32 = 0x6474_E550;
const PT_GNU_STACK: u32 = 0x6474_E551;
const PT_GNU_RELRO: u32 = 0x6474_E552;

/// Dynamic section tags
const DT_NULL: u64 = 0;
const DT_NEEDED: u64 = 1;
const DT_PLTRELSZ: u64 = 2;
const DT_PLTGOT: u64 = 3;
const DT_HASH: u64 = 4;
const DT_STRTAB: u64 = 5;
const DT_SYMTAB: u64 = 6;
const DT_RELA: u64 = 7;
const DT_RELASZ: u64 = 8;
const DT_RELAENT: u64 = 9;
const DT_STRSZ: u64 = 10;
const DT_SYMENT: u64 = 11;
const DT_INIT: u64 = 12;
const DT_FINI: u64 = 13;
const DT_SONAME: u64 = 14;
const DT_RPATH: u64 = 15;
const DT_SYMBOLIC: u64 = 16;
const DT_REL: u64 = 17;
const DT_RELSZ: u64 = 18;
const DT_RELENT: u64 = 19;
const DT_PLTREL: u64 = 20;
const DT_DEBUG: u64 = 21;
const DT_JMPREL: u64 = 23;

/// Relocation types (x86_64)
const R_X86_64_NONE: u32 = 0;
const R_X86_64_64: u32 = 1;
const R_X86_64_GLOB_DAT: u32 = 6;
const R_X86_64_JUMP_SLOT: u32 = 7;
const R_X86_64_RELATIVE: u32 = 8;

// ── Shared library tracking ──────────────────────────────────────────────────

/// A loaded shared library
#[derive(Debug, Clone)]
pub struct SharedLibrary {
    /// Library name (e.g. "libc.so.1")
    pub name: String,
    /// Base virtual address where loaded
    pub base_addr: usize,
    /// Size in memory
    pub mem_size: usize,
    /// Symbol table: name -> virtual address
    pub symbols: BTreeMap<String, usize>,
    /// Reference count (how many binaries depend on this)
    pub ref_count: u32,
    /// Initializer function address (DT_INIT), if any
    pub init_func: Option<usize>,
    /// Finalizer function address (DT_FINI), if any
    pub fini_func: Option<usize>,
}

/// The global shared library table
struct LibraryTable {
    /// Loaded libraries by name
    libraries: BTreeMap<String, SharedLibrary>,
    /// Next base address for loading a new library
    next_load_addr: usize,
    /// Search paths for libraries
    search_paths: Vec<String>,
}

impl LibraryTable {
    const fn new() -> Self {
        LibraryTable {
            libraries: BTreeMap::new(),
            next_load_addr: 0x4000_0000, // 1GB mark for shared libs
            search_paths: Vec::new(),
        }
    }
}

static LIB_TABLE: Mutex<LibraryTable> = Mutex::new(LibraryTable::new());

// ── ELF header parsing helpers ───────────────────────────────────────────────

fn read_u16_le(data: &[u8], offset: usize) -> u16 {
    if offset + 2 > data.len() {
        return 0;
    }
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    if offset + 4 > data.len() {
        return 0;
    }
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn read_u64_le(data: &[u8], offset: usize) -> u64 {
    if offset + 8 > data.len() {
        return 0;
    }
    u64::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
        data[offset + 4],
        data[offset + 5],
        data[offset + 6],
        data[offset + 7],
    ])
}

/// Validate ELF header
pub fn validate_elf(data: &[u8]) -> Result<(), &'static str> {
    if data.len() < 64 {
        return Err("ELF too small");
    }
    if data[0..4] != ELF_MAGIC {
        return Err("invalid ELF magic");
    }
    if data[4] != ELFCLASS64 {
        return Err("not ELF64");
    }
    Ok(())
}

/// Extract the PT_INTERP path from an ELF binary
pub fn get_interp(data: &[u8]) -> Option<String> {
    if validate_elf(data).is_err() {
        return None;
    }

    let e_phoff = read_u64_le(data, 32) as usize;
    let e_phentsize = read_u16_le(data, 54) as usize;
    let e_phnum = read_u16_le(data, 56) as usize;

    for i in 0..e_phnum {
        let ph_offset = e_phoff + i * e_phentsize;
        let p_type = read_u32_le(data, ph_offset);

        if p_type == PT_INTERP {
            let p_offset = read_u64_le(data, ph_offset + 8) as usize;
            let p_filesz = read_u64_le(data, ph_offset + 32) as usize;

            if p_offset + p_filesz <= data.len() {
                let interp_bytes = &data[p_offset..p_offset + p_filesz];
                // Trim trailing null
                let len = interp_bytes
                    .iter()
                    .position(|&b| b == 0)
                    .unwrap_or(interp_bytes.len());
                return core::str::from_utf8(&interp_bytes[..len])
                    .ok()
                    .map(String::from);
            }
        }
    }

    None
}

/// Extract DT_NEEDED library names from the DYNAMIC segment
pub fn get_needed_libs(data: &[u8]) -> Vec<String> {
    let mut needed = Vec::new();

    if validate_elf(data).is_err() {
        return needed;
    }

    let e_phoff = read_u64_le(data, 32) as usize;
    let e_phentsize = read_u16_le(data, 54) as usize;
    let e_phnum = read_u16_le(data, 56) as usize;

    let mut dynamic_offset = 0usize;
    let mut dynamic_size = 0usize;
    let mut strtab_offset = 0usize;

    // First pass: find PT_DYNAMIC and string table
    for i in 0..e_phnum {
        let ph_offset = e_phoff + i * e_phentsize;
        let p_type = read_u32_le(data, ph_offset);

        if p_type == PT_DYNAMIC {
            dynamic_offset = read_u64_le(data, ph_offset + 8) as usize;
            dynamic_size = read_u64_le(data, ph_offset + 32) as usize;
        }
    }

    if dynamic_offset == 0 {
        return needed;
    }

    // Parse dynamic entries to find DT_STRTAB first
    let mut pos = dynamic_offset;
    while pos + 16 <= data.len() && pos < dynamic_offset + dynamic_size {
        let tag = read_u64_le(data, pos);
        let val = read_u64_le(data, pos + 8);
        if tag == DT_NULL {
            break;
        }
        if tag == DT_STRTAB {
            // val is a virtual address; for simplicity treat as file offset
            // In a real linker we'd translate VA to file offset
            strtab_offset = val as usize;
        }
        pos += 16;
    }

    // Second pass: collect DT_NEEDED entries
    pos = dynamic_offset;
    while pos + 16 <= data.len() && pos < dynamic_offset + dynamic_size {
        let tag = read_u64_le(data, pos);
        let val = read_u64_le(data, pos + 8);
        if tag == DT_NULL {
            break;
        }
        if tag == DT_NEEDED {
            let name_offset = strtab_offset + val as usize;
            if name_offset < data.len() {
                let name_bytes = &data[name_offset..];
                let len = name_bytes.iter().position(|&b| b == 0).unwrap_or(0);
                if let Ok(name) = core::str::from_utf8(&name_bytes[..len]) {
                    needed.push(String::from(name));
                }
            }
        }
        pos += 16;
    }

    needed
}

/// Relocation entry (Elf64_Rela)
#[derive(Debug, Clone, Copy)]
pub struct Rela {
    pub offset: u64,
    pub info: u64,
    pub addend: i64,
}

impl Rela {
    pub fn sym_index(&self) -> u32 {
        (self.info >> 32) as u32
    }
    pub fn rel_type(&self) -> u32 {
        (self.info & 0xFFFF_FFFF) as u32
    }
}

/// Process relocations for a loaded binary
///
/// `base_addr`: base virtual address where the binary is mapped
/// `rela_entries`: relocation table entries
/// `got_addr`: GOT base address
///
/// Returns number of relocations processed.
pub fn process_relocations(base_addr: usize, rela_entries: &[Rela], _got_addr: usize) -> usize {
    let mut processed = 0;

    for rela in rela_entries {
        let target = base_addr + rela.offset as usize;
        match rela.rel_type() {
            R_X86_64_RELATIVE => {
                // *target = base + addend
                let value = (base_addr as i64 + rela.addend) as u64;
                unsafe {
                    *(target as *mut u64) = value;
                }
                processed += 1;
            }
            R_X86_64_64 => {
                // *target = S + A  (S = symbol value, A = addend)
                // For now, stub: use addend as-is relative to base
                let value = (base_addr as i64 + rela.addend) as u64;
                unsafe {
                    *(target as *mut u64) = value;
                }
                processed += 1;
            }
            R_X86_64_GLOB_DAT | R_X86_64_JUMP_SLOT => {
                // These need symbol resolution
                // For now, point to a stub that traps
                let sym_idx = rela.sym_index();
                let resolved = resolve_symbol_by_index(sym_idx);
                unsafe {
                    *(target as *mut u64) = resolved as u64;
                }
                processed += 1;
            }
            R_X86_64_NONE => {
                // Skip
            }
            _ => {
                serial_println!(
                    "  [dynlink] unsupported relocation type {}",
                    rela.rel_type()
                );
            }
        }
    }

    processed
}

/// Resolve a symbol by index (stub -- returns 0 for unknown)
fn resolve_symbol_by_index(_sym_idx: u32) -> usize {
    // In a real linker, we'd look up the symbol in loaded libraries
    // For now, return 0 (will cause a page fault if called)
    0
}

/// Look up a symbol across all loaded shared libraries
pub fn resolve_symbol(name: &str) -> Option<usize> {
    let table = LIB_TABLE.lock();
    for (_lib_name, lib) in table.libraries.iter() {
        if let Some(&addr) = lib.symbols.get(name) {
            return Some(addr);
        }
    }
    None
}

/// Register a shared library in the global table
pub fn register_library(lib: SharedLibrary) {
    let mut table = LIB_TABLE.lock();
    serial_println!(
        "  [dynlink] registered library '{}' at {:#x} ({} symbols)",
        lib.name,
        lib.base_addr,
        lib.symbols.len()
    );
    table.libraries.insert(lib.name.clone(), lib);
}

/// Unload a shared library by name
pub fn unload_library(name: &str) -> Result<(), &'static str> {
    let mut table = LIB_TABLE.lock();
    let lib = table.libraries.get_mut(name).ok_or("library not found")?;
    lib.ref_count = lib.ref_count.saturating_sub(1);
    if lib.ref_count == 0 {
        serial_println!("  [dynlink] unloading library '{}'", name);
        table.libraries.remove(name);
    }
    Ok(())
}

/// Add a library search path
pub fn add_search_path(path: &str) {
    let mut table = LIB_TABLE.lock();
    table.search_paths.push(String::from(path));
}

/// List all loaded libraries
pub fn list_loaded() -> Vec<(String, usize, usize, u32)> {
    let table = LIB_TABLE.lock();
    table
        .libraries
        .values()
        .map(|lib| (lib.name.clone(), lib.base_addr, lib.mem_size, lib.ref_count))
        .collect()
}

/// Get the next available load address and advance it
pub fn alloc_load_region(size: usize) -> usize {
    let mut table = LIB_TABLE.lock();
    let addr = table.next_load_addr;
    // Page-align the size
    let aligned = (size + 0xFFF) & !0xFFF;
    table.next_load_addr += aligned;
    addr
}

/// Check if a library is already loaded
pub fn is_loaded(name: &str) -> bool {
    LIB_TABLE.lock().libraries.contains_key(name)
}

/// Increment reference count for a library
pub fn add_ref(name: &str) {
    let mut table = LIB_TABLE.lock();
    if let Some(lib) = table.libraries.get_mut(name) {
        lib.ref_count = lib.ref_count.saturating_add(1);
    }
}

/// Statistics
pub fn stats() -> String {
    let table = LIB_TABLE.lock();
    let total_syms: usize = table.libraries.values().map(|l| l.symbols.len()).sum();
    alloc::format!(
        "Dynamic linker: {} libraries loaded, {} total symbols\n\
         Search paths: {}\n\
         Next load address: {:#x}\n",
        table.libraries.len(),
        total_syms,
        table.search_paths.join(":"),
        table.next_load_addr,
    )
}

/// Initialize the dynamic linker subsystem
pub fn init() {
    let mut table = LIB_TABLE.lock();
    // Set default library search paths
    table.search_paths.push(String::from("/lib"));
    table.search_paths.push(String::from("/usr/lib"));
    table.search_paths.push(String::from("/usr/local/lib"));
    drop(table);

    // Register the kernel-provided "vDSO" as a pseudo-library
    let mut vdso_syms = BTreeMap::new();
    vdso_syms.insert(String::from("__genesis_clock_gettime"), 0usize);
    vdso_syms.insert(String::from("__genesis_gettimeofday"), 0usize);
    vdso_syms.insert(String::from("__genesis_getcpu"), 0usize);

    register_library(SharedLibrary {
        name: String::from("linux-vdso.so.1"),
        base_addr: 0xFFFF_F000_0000,
        mem_size: 0x1000,
        symbols: vdso_syms,
        ref_count: 1,
        init_func: None,
        fini_func: None,
    });

    serial_println!("  dynlink: dynamic linker ready (ELF INTERP, GOT/PLT relocation)");
}
