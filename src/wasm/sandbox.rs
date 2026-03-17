/// WASM sandbox enforcement
///
/// Part of the AIOS.

use crate::sync::Mutex;

/// Default sandbox limits.
const DEFAULT_MAX_MEMORY_PAGES: u32 = 256;  // 16 MiB
const DEFAULT_MAX_TABLE_SIZE: u32 = 10000;
const DEFAULT_MAX_FUEL: u64 = 10_000_000;   // ~10M instructions

/// Resource limits and policy for sandboxed WASM execution.
pub struct SandboxPolicy {
    pub max_memory_pages: u32,
    pub max_table_size: u32,
    pub max_fuel: u64,
}

impl SandboxPolicy {
    pub fn new() -> Self {
        SandboxPolicy {
            max_memory_pages: DEFAULT_MAX_MEMORY_PAGES,
            max_table_size: DEFAULT_MAX_TABLE_SIZE,
            max_fuel: DEFAULT_MAX_FUEL,
        }
    }

    /// Create a restrictive policy for untrusted modules.
    pub fn restrictive() -> Self {
        SandboxPolicy {
            max_memory_pages: 16,       // 1 MiB
            max_table_size: 1000,
            max_fuel: 1_000_000,
        }
    }

    /// Create a permissive policy for trusted system modules.
    pub fn permissive() -> Self {
        SandboxPolicy {
            max_memory_pages: 1024,     // 64 MiB
            max_table_size: 100_000,
            max_fuel: u64::MAX,
        }
    }

    /// Check if fuel is exhausted.
    ///
    /// Returns true if the remaining fuel is within budget (execution can continue).
    /// Returns false if fuel is exhausted (execution must stop).
    pub fn check_fuel(&self, remaining: u64) -> bool {
        remaining > 0
    }

    /// Validate a module against this sandbox policy.
    ///
    /// Checks that the module's declared memory and table limits do not
    /// exceed the sandbox policy limits.
    pub fn validate(&self, module: &super::parser::WasmModule) -> bool {
        // Check memory limits
        if let Some(mem_sec) = module.get_section(super::parser::SECTION_MEMORY) {
            if !mem_sec.data.is_empty() {
                let (count, mut offset) = super::parser::decode_leb128_u32(&mem_sec.data);
                for _ in 0..count {
                    if offset >= mem_sec.data.len() { break; }
                    let has_max = mem_sec.data[offset];
                    offset += 1;
                    let (initial, consumed) = super::parser::decode_leb128_u32(&mem_sec.data[offset..]);
                    offset += consumed;

                    if initial > self.max_memory_pages {
                        crate::serial_println!(
                            "[wasm/sandbox] REJECT: memory initial {} > max {}",
                            initial, self.max_memory_pages
                        );
                        return false;
                    }

                    if has_max == 1 {
                        let (max_pages, consumed) = super::parser::decode_leb128_u32(&mem_sec.data[offset..]);
                        offset += consumed;
                        if max_pages > self.max_memory_pages {
                            crate::serial_println!(
                                "[wasm/sandbox] REJECT: memory max {} > policy max {}",
                                max_pages, self.max_memory_pages
                            );
                            return false;
                        }
                    }
                }
            }
        }

        // Check table limits
        if let Some(table_sec) = module.get_section(super::parser::SECTION_TABLE) {
            if !table_sec.data.is_empty() {
                let (count, mut offset) = super::parser::decode_leb128_u32(&table_sec.data);
                for _ in 0..count {
                    if offset >= table_sec.data.len() { break; }
                    // Skip element type byte
                    offset += 1;
                    let has_max = table_sec.data.get(offset).copied().unwrap_or(0);
                    offset += 1;
                    let (initial, consumed) = super::parser::decode_leb128_u32(&table_sec.data[offset..]);
                    offset += consumed;

                    if initial > self.max_table_size {
                        crate::serial_println!(
                            "[wasm/sandbox] REJECT: table initial {} > max {}",
                            initial, self.max_table_size
                        );
                        return false;
                    }

                    if has_max == 1 {
                        let (max_size, consumed) = super::parser::decode_leb128_u32(&table_sec.data[offset..]);
                        offset += consumed;
                        if max_size > self.max_table_size {
                            return false;
                        }
                    }
                }
            }
        }

        crate::serial_println!("[wasm/sandbox] module passed policy validation");
        true
    }
}

pub fn init() {
    crate::serial_println!("[wasm] sandbox policy engine ready");
}
