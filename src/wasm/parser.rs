/// WASM binary format parser
///
/// Part of the AIOS.

use alloc::vec::Vec;
use crate::sync::Mutex;

// WASM binary magic number and version
const WASM_MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6D]; // \0asm
const WASM_VERSION: [u8; 4] = [0x01, 0x00, 0x00, 0x00]; // version 1

// Section IDs
pub const SECTION_CUSTOM: u8 = 0;
pub const SECTION_TYPE: u8 = 1;
pub const SECTION_IMPORT: u8 = 2;
pub const SECTION_FUNCTION: u8 = 3;
pub const SECTION_TABLE: u8 = 4;
pub const SECTION_MEMORY: u8 = 5;
pub const SECTION_GLOBAL: u8 = 6;
pub const SECTION_EXPORT: u8 = 7;
pub const SECTION_START: u8 = 8;
pub const SECTION_ELEMENT: u8 = 9;
pub const SECTION_CODE: u8 = 10;
pub const SECTION_DATA: u8 = 11;

/// Parsed representation of a WASM binary module.
pub struct WasmModule {
    pub sections: Vec<Section>,
}

pub struct Section {
    pub id: u8,
    pub data: Vec<u8>,
}

/// Decode a LEB128 unsigned integer from the byte stream.
/// Returns (value, bytes_consumed).
pub fn decode_leb128_u32(bytes: &[u8]) -> (u32, usize) {
    let mut result: u32 = 0;
    let mut shift = 0u32;
    let mut idx = 0;
    loop {
        if idx >= bytes.len() {
            break;
        }
        let byte = bytes[idx];
        idx += 1;
        result |= ((byte & 0x7F) as u32) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift >= 35 {
            break; // overflow protection
        }
    }
    (result, idx)
}

impl WasmModule {
    /// Parse a WASM binary from raw bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() < 8 {
            return Err("wasm binary too short");
        }

        // Verify magic number
        if bytes[0..4] != WASM_MAGIC {
            return Err("invalid wasm magic");
        }

        // Verify version
        if bytes[4..8] != WASM_VERSION {
            return Err("unsupported wasm version");
        }

        let mut sections = Vec::new();
        let mut offset = 8;

        // Parse sections
        while offset < bytes.len() {
            if offset >= bytes.len() {
                break;
            }

            // Section ID (1 byte)
            let section_id = bytes[offset];
            offset += 1;

            // Section size (LEB128)
            let (section_size, consumed) = decode_leb128_u32(&bytes[offset..]);
            offset += consumed;

            let section_size = section_size as usize;
            if offset + section_size > bytes.len() {
                return Err("section extends past end of binary");
            }

            // Copy section data
            let data = bytes[offset..offset + section_size].to_vec();
            sections.push(Section {
                id: section_id,
                data,
            });

            offset += section_size;
        }

        crate::serial_println!(
            "[wasm/parser] parsed module: {} sections, {} bytes",
            sections.len(), bytes.len()
        );

        Ok(WasmModule { sections })
    }

    /// Return the number of functions defined in the module.
    ///
    /// This counts entries in the Function section (section 3), which lists
    /// the type indices for each function body in the Code section.
    pub fn function_count(&self) -> usize {
        for section in &self.sections {
            if section.id == SECTION_FUNCTION {
                if section.data.is_empty() {
                    return 0;
                }
                // First byte(s) is the count as LEB128
                let (count, _) = decode_leb128_u32(&section.data);
                return count as usize;
            }
        }
        0
    }

    /// Get a section by ID.
    pub fn get_section(&self, id: u8) -> Option<&Section> {
        self.sections.iter().find(|s| s.id == id)
    }

    /// Get the number of sections.
    pub fn section_count(&self) -> usize {
        self.sections.len()
    }
}

pub fn init() {
    crate::serial_println!("[wasm] parser ready");
}
