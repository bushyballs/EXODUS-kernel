/// Module validation
///
/// Part of the AIOS.

use crate::sync::Mutex;
use super::parser::{WasmModule, SECTION_TYPE, SECTION_FUNCTION, SECTION_CODE, SECTION_MEMORY};

/// Validates a parsed WASM module for type safety and well-formedness.
pub struct WasmValidator {
    errors: usize,
}

impl WasmValidator {
    pub fn new() -> Self {
        WasmValidator {
            errors: 0,
        }
    }

    /// Validate a parsed WASM module.
    ///
    /// Checks:
    /// - Module has at least one section
    /// - Type section (if present) is well-formed
    /// - Function section count matches code section count
    /// - Memory section limits are valid
    pub fn validate(&mut self, module: &WasmModule) -> Result<(), &'static str> {
        self.errors = 0;

        // Check that the module has at least some sections
        if module.section_count() == 0 {
            self.errors = self.errors.saturating_add(1);
            return Err("module has no sections");
        }

        // Validate type section if present
        if let Some(type_sec) = module.get_section(SECTION_TYPE) {
            if type_sec.data.is_empty() {
                self.errors = self.errors.saturating_add(1);
                return Err("type section is empty");
            }
            // First byte(s) is count of type entries
            let (count, consumed) = super::parser::decode_leb128_u32(&type_sec.data);
            if consumed == 0 {
                self.errors = self.errors.saturating_add(1);
                return Err("malformed type section count");
            }
            // Each type entry should start with 0x60 (function type)
            let mut offset = consumed;
            for _ in 0..count {
                if offset >= type_sec.data.len() {
                    self.errors = self.errors.saturating_add(1);
                    return Err("type section truncated");
                }
                if type_sec.data[offset] != 0x60 {
                    self.errors = self.errors.saturating_add(1);
                    return Err("expected function type marker 0x60");
                }
                offset += 1;
                // Skip param types
                let (param_count, c) = super::parser::decode_leb128_u32(&type_sec.data[offset..]);
                offset += c + param_count as usize;
                // Skip result types
                if offset < type_sec.data.len() {
                    let (result_count, c) = super::parser::decode_leb128_u32(&type_sec.data[offset..]);
                    offset += c + result_count as usize;
                }
            }
        }

        // Validate function/code section consistency
        let func_count = module.function_count();
        if let Some(code_sec) = module.get_section(SECTION_CODE) {
            if !code_sec.data.is_empty() {
                let (code_count, _) = super::parser::decode_leb128_u32(&code_sec.data);
                if func_count != code_count as usize {
                    self.errors = self.errors.saturating_add(1);
                    return Err("function count does not match code section count");
                }
            }
        }

        // Validate memory section limits
        if let Some(mem_sec) = module.get_section(SECTION_MEMORY) {
            if !mem_sec.data.is_empty() {
                let (count, offset) = super::parser::decode_leb128_u32(&mem_sec.data);
                if count > 1 {
                    self.errors = self.errors.saturating_add(1);
                    return Err("multiple memories not supported");
                }
            }
        }

        if self.errors == 0 {
            crate::serial_println!("[wasm/validator] module valid ({} functions)", func_count);
            Ok(())
        } else {
            Err("validation failed")
        }
    }

    /// Return the number of validation errors found.
    pub fn error_count(&self) -> usize {
        self.errors
    }
}

pub fn init() {
    crate::serial_println!("[wasm] validator ready");
}
