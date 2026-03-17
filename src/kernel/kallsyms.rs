/// Kernel symbol table for stack traces and address resolution.
///
/// Part of the AIOS kernel.
///
/// ## Linker script requirement
///
/// For `load_symbols()` to find any symbols the linker script (`linker.ld`)
/// must export a packed symbol table in the following format and expose two
/// boundary symbols:
///
/// ```text
/// .kallsyms : {
///     __kernel_sym_start = .;
///     KEEP(*(.kallsyms))
///     __kernel_sym_end   = .;
/// }
/// ```
///
/// Each entry in the `.kallsyms` section is laid out as:
///
/// ```text
/// | 8 bytes: u64 address (little-endian) |
/// | 1 byte:  name_len (N)                |
/// | N bytes: symbol name (UTF-8, no NUL) |
/// ```
///
/// If this section is absent (i.e. `__kernel_sym_start == __kernel_sym_end`)
/// the subsystem initialises with 0 symbols and logs a notice.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// A kernel symbol with its address range.
pub struct KernelSymbol {
    pub name: String,
    pub addr: usize,
    pub size: usize,
}

/// Symbol table holding all kernel symbols for lookup.
pub struct SymbolTable {
    pub symbols: Vec<KernelSymbol>,
}

impl SymbolTable {
    pub fn new() -> Self {
        SymbolTable {
            symbols: Vec::new(),
        }
    }

    /// Look up the symbol name for a given address.
    /// Symbols must be sorted by `addr` ascending for binary search to work.
    pub fn lookup(&self, addr: usize) -> Option<&KernelSymbol> {
        // Binary search for the last symbol whose start address <= addr.
        let idx = self.symbols.partition_point(|s| s.addr <= addr);
        if idx == 0 {
            return None;
        }
        let sym = &self.symbols[idx - 1];
        // Check that addr falls within [sym.addr, sym.addr + sym.size).
        if addr < sym.addr.saturating_add(sym.size) {
            Some(sym)
        } else {
            None
        }
    }

    /// Resolve an instruction pointer to "symbol+offset" string.
    pub fn symbolize(&self, addr: usize) -> String {
        match self.lookup(addr) {
            Some(sym) => {
                let offset = addr.saturating_sub(sym.addr);
                let mut s = String::from(sym.name.as_str());
                s.push_str("+0x");
                // Format offset as hex without std::fmt heap alloc tricks —
                // build the hex digits directly.
                let mut buf = [0u8; 16];
                let mut n = offset;
                let mut len = 0usize;
                if n == 0 {
                    buf[0] = b'0';
                    len = 1;
                } else {
                    while n > 0 {
                        let digit = (n & 0xf) as u8;
                        buf[len] = if digit < 10 {
                            b'0' + digit
                        } else {
                            b'a' + digit - 10
                        };
                        n >>= 4;
                        len += 1;
                    }
                    buf[..len].reverse();
                }
                s.push_str(core::str::from_utf8(&buf[..len]).unwrap_or("?"));
                s
            }
            None => {
                // Return raw address as "0x<hex>".
                let mut s = String::from("0x");
                let mut buf = [0u8; 16];
                let mut n = addr;
                let mut len = 0usize;
                if n == 0 {
                    buf[0] = b'0';
                    len = 1;
                } else {
                    while n > 0 {
                        let digit = (n & 0xf) as u8;
                        buf[len] = if digit < 10 {
                            b'0' + digit
                        } else {
                            b'a' + digit - 10
                        };
                        n >>= 4;
                        len += 1;
                    }
                    buf[..len].reverse();
                }
                s.push_str(core::str::from_utf8(&buf[..len]).unwrap_or("?"));
                s
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Linker-exported symbol table boundary markers.
//
// These are provided by the linker script.  If the `.kallsyms` section is
// absent both symbols will point to the same address and `load_symbols()`
// will record 0 symbols.
// ---------------------------------------------------------------------------

extern "C" {
    /// First byte of the packed kallsyms table (linker-provided).
    static __kernel_sym_start: u8;
    /// One-past-the-last byte of the packed kallsyms table (linker-provided).
    static __kernel_sym_end: u8;
}

// ---------------------------------------------------------------------------
// Maximum number of symbols we store in the static array.
// Entries beyond this limit are silently ignored.
// ---------------------------------------------------------------------------
const MAX_SYMBOLS: usize = 512;

/// A static symbol record that does not require heap allocation.
#[derive(Clone, Copy)]
struct StaticSymbol {
    addr: u64,
    /// Byte length of `name` (0 = slot unused).
    name_len: u8,
    /// Fixed-size name storage — names longer than 63 bytes are truncated.
    name: [u8; 63],
}

impl StaticSymbol {
    const fn zeroed() -> Self {
        StaticSymbol {
            addr: 0,
            name_len: 0,
            name: [0u8; 63],
        }
    }
}

/// Inner state protected by the global Mutex.
struct KallsymsState {
    syms: [StaticSymbol; MAX_SYMBOLS],
    count: usize,
}

impl KallsymsState {
    const fn new() -> Self {
        KallsymsState {
            syms: [StaticSymbol::zeroed(); MAX_SYMBOLS],
            count: 0,
        }
    }
}

static KALLSYMS: Mutex<KallsymsState> = Mutex::new(KallsymsState::new());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse the linker-generated symbol table and populate the static registry.
///
/// Entry format (packed, no padding):
///   [u64 address (8 bytes LE)] [u8 name_len] [name_len bytes of UTF-8 name]
///
/// Entries are stored in address order as they appear in the section;
/// the caller is responsible for ensuring the linker script emits them sorted
/// if `SymbolTable::lookup()` binary-search semantics are needed.
pub fn load_symbols() {
    let mut state = KALLSYMS.lock();
    state.count = 0;

    // Safety: the linker guarantees these symbols are valid read-only pointers
    // inside the kernel image.  We only read, never write.
    let (start_ptr, end_ptr) = unsafe {
        (
            &__kernel_sym_start as *const u8,
            &__kernel_sym_end as *const u8,
        )
    };

    let table_bytes = end_ptr as usize - start_ptr as usize;

    if table_bytes == 0 {
        // No kallsyms section present in the linker script yet.
        // This is expected during early kernel development.
        // To enable symbol loading, add the following to your linker script:
        //
        //   .kallsyms : {
        //       __kernel_sym_start = .;
        //       KEEP(*(.kallsyms))
        //       __kernel_sym_end   = .;
        //   }
        //
        // and emit symbol entries (addr u64 LE + name_len u8 + name bytes)
        // from your build tooling into the `.kallsyms` input section.
        crate::serial_println!(
            "[kallsyms] no symbol table found (linker section absent) — 0 symbols loaded"
        );
        return;
    }

    // Iterate the packed table.
    let mut offset: usize = 0;
    let min_entry = 8 + 1; // 8-byte addr + 1-byte name_len

    while offset + min_entry <= table_bytes && state.count < MAX_SYMBOLS {
        // Safety: offset is always within [0, table_bytes).
        let entry_ptr = unsafe { start_ptr.add(offset) };

        // Read 8-byte little-endian address.
        let addr = unsafe {
            let b = core::slice::from_raw_parts(entry_ptr, 8);
            u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
        };
        offset += 8;

        // Read 1-byte name length.
        let name_len = unsafe { *start_ptr.add(offset) };
        offset += 1;

        let name_len_usize = name_len as usize;

        // Bounds check: make sure the name fits within the table.
        if offset + name_len_usize > table_bytes {
            crate::serial_println!(
                "[kallsyms] truncated entry at offset {} — stopping parse",
                offset
            );
            break;
        }

        // Copy name bytes into fixed-size storage (truncate to 63 chars).
        let copy_len = name_len_usize.min(63);
        let mut sym = StaticSymbol::zeroed();
        sym.addr = addr;
        sym.name_len = copy_len as u8;

        unsafe {
            let name_ptr = start_ptr.add(offset);
            for i in 0..copy_len {
                sym.name[i] = *name_ptr.add(i);
            }
        }

        let idx = state.count;
        state.syms[idx] = sym;
        state.count += 1;

        // Advance past the full (possibly longer) name.
        offset += name_len_usize;
    }

    crate::serial_println!(
        "[kallsyms] loaded {} symbol(s) from linker table ({} bytes)",
        state.count,
        table_bytes
    );
}

/// Resolve an address to a symbol name using the static registry.
/// Returns `None` if no matching symbol is found.
pub fn lookup(addr: u64) -> Option<(u64, &'static str)> {
    // We cannot return a reference into the Mutex-protected data easily,
    // so we do a best-effort linear scan and return a static str slice
    // from the name bytes.  Because the static array lives for 'static
    // we can project a &'static str from it after locking.
    //
    // Safety: the KallsymsState is static and its contents are only ever
    // written during `load_symbols()` which runs once at boot before SMP
    // is fully active.  Post-init reads are effectively read-only.
    let state = KALLSYMS.lock();
    for i in 0..state.count {
        let sym = &state.syms[i];
        if sym.addr <= addr {
            // Continue to find the last symbol whose address <= addr.
            let is_last = i + 1 >= state.count || state.syms[i + 1].addr > addr;
            if is_last {
                let name_bytes = &sym.name[..sym.name_len as usize];
                // Safety: we only stored valid UTF-8 bytes from the linker table.
                let name_str = core::str::from_utf8(name_bytes).unwrap_or("<invalid>");
                // We need to return a 'static reference.  The underlying storage
                // is a static, so projecting a pointer is valid for 'static.
                let static_name: &'static str = unsafe {
                    let ptr = name_bytes.as_ptr();
                    let len = name_bytes.len();
                    let slice = core::slice::from_raw_parts(ptr, len);
                    core::str::from_utf8_unchecked(slice)
                };
                let sym_addr = sym.addr;
                drop(state);
                return Some((sym_addr, static_name));
            }
        }
    }
    None
}

/// Return the total number of symbols currently loaded.
pub fn symbol_count() -> usize {
    KALLSYMS.lock().count
}

/// Initialize the kernel symbol table.
pub fn init() {
    load_symbols();
}
