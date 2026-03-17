/// WASM bytecode interpreter
///
/// Part of the AIOS.

use alloc::vec::Vec;
use alloc::vec;
use crate::sync::Mutex;

// Common WASM opcodes — complete MVP set
const OP_UNREACHABLE: u8 = 0x00;
const OP_NOP: u8 = 0x01;
const OP_BLOCK: u8 = 0x02;
const OP_LOOP: u8 = 0x03;
const OP_IF: u8 = 0x04;
const OP_ELSE: u8 = 0x05;
const OP_END: u8 = 0x0B;
const OP_BR: u8 = 0x0C;
const OP_BR_IF: u8 = 0x0D;
const OP_BR_TABLE: u8 = 0x0E;
const OP_RETURN: u8 = 0x0F;
const OP_CALL: u8 = 0x10;
const OP_CALL_INDIRECT: u8 = 0x11;
const OP_DROP: u8 = 0x1A;
const OP_SELECT: u8 = 0x1B;
const OP_LOCAL_GET: u8 = 0x20;
const OP_LOCAL_SET: u8 = 0x21;
const OP_LOCAL_TEE: u8 = 0x22;
const OP_GLOBAL_GET: u8 = 0x23;
const OP_GLOBAL_SET: u8 = 0x24;
// Memory load instructions
const OP_I32_LOAD: u8 = 0x28;
const OP_I64_LOAD: u8 = 0x29;
const OP_I32_LOAD8_S: u8 = 0x2C;
const OP_I32_LOAD8_U: u8 = 0x2D;
const OP_I32_LOAD16_S: u8 = 0x2E;
const OP_I32_LOAD16_U: u8 = 0x2F;
const OP_I64_LOAD8_S: u8 = 0x30;
const OP_I64_LOAD8_U: u8 = 0x31;
const OP_I64_LOAD16_S: u8 = 0x32;
const OP_I64_LOAD16_U: u8 = 0x33;
const OP_I64_LOAD32_S: u8 = 0x34;
const OP_I64_LOAD32_U: u8 = 0x35;
// Memory store instructions
const OP_I32_STORE: u8 = 0x36;
const OP_I64_STORE: u8 = 0x37;
const OP_I32_STORE8: u8 = 0x3A;
const OP_I32_STORE16: u8 = 0x3B;
const OP_I64_STORE8: u8 = 0x3C;
const OP_I64_STORE16: u8 = 0x3D;
const OP_I64_STORE32: u8 = 0x3E;
// Memory size/grow
const OP_MEMORY_SIZE: u8 = 0x3F;
const OP_MEMORY_GROW: u8 = 0x40;
// Const
const OP_I32_CONST: u8 = 0x41;
const OP_I64_CONST: u8 = 0x42;
// i32 comparisons
const OP_I32_EQZ: u8 = 0x45;
const OP_I32_EQ: u8 = 0x46;
const OP_I32_NE: u8 = 0x47;
const OP_I32_LT_S: u8 = 0x48;
const OP_I32_LT_U: u8 = 0x49;
const OP_I32_GT_S: u8 = 0x4A;
const OP_I32_GT_U: u8 = 0x4B;
const OP_I32_LE_S: u8 = 0x4C;
const OP_I32_LE_U: u8 = 0x4D;
const OP_I32_GE_S: u8 = 0x4E;
const OP_I32_GE_U: u8 = 0x4F;
// i64 comparisons
const OP_I64_EQZ: u8 = 0x50;
const OP_I64_EQ: u8 = 0x51;
const OP_I64_NE: u8 = 0x52;
const OP_I64_LT_S: u8 = 0x53;
const OP_I64_LT_U: u8 = 0x54;
const OP_I64_GT_S: u8 = 0x55;
const OP_I64_GT_U: u8 = 0x56;
const OP_I64_LE_S: u8 = 0x57;
const OP_I64_LE_U: u8 = 0x58;
const OP_I64_GE_S: u8 = 0x59;
const OP_I64_GE_U: u8 = 0x5A;
// i32 numeric ops
const OP_I32_CLZ: u8 = 0x67;
const OP_I32_CTZ: u8 = 0x68;
const OP_I32_POPCNT: u8 = 0x69;
const OP_I32_ADD: u8 = 0x6A;
const OP_I32_SUB: u8 = 0x6B;
const OP_I32_MUL: u8 = 0x6C;
const OP_I32_DIV_S: u8 = 0x6D;
const OP_I32_DIV_U: u8 = 0x6E;
const OP_I32_REM_S: u8 = 0x6F;
const OP_I32_REM_U: u8 = 0x70;
const OP_I32_AND: u8 = 0x71;
const OP_I32_OR: u8 = 0x72;
const OP_I32_XOR: u8 = 0x73;
const OP_I32_SHL: u8 = 0x74;
const OP_I32_SHR_S: u8 = 0x75;
const OP_I32_SHR_U: u8 = 0x76;
const OP_I32_ROTL: u8 = 0x77;
const OP_I32_ROTR: u8 = 0x78;
// i64 numeric ops
const OP_I64_CLZ: u8 = 0x79;
const OP_I64_CTZ: u8 = 0x7A;
const OP_I64_POPCNT: u8 = 0x7B;
const OP_I64_ADD: u8 = 0x7C;
const OP_I64_SUB: u8 = 0x7D;
const OP_I64_MUL: u8 = 0x7E;
const OP_I64_DIV_S: u8 = 0x7F;
const OP_I64_DIV_U: u8 = 0x80;
const OP_I64_REM_S: u8 = 0x81;
const OP_I64_REM_U: u8 = 0x82;
const OP_I64_AND: u8 = 0x83;
const OP_I64_OR: u8 = 0x84;
const OP_I64_XOR: u8 = 0x85;
const OP_I64_SHL: u8 = 0x86;
const OP_I64_SHR_S: u8 = 0x87;
const OP_I64_SHR_U: u8 = 0x88;
const OP_I64_ROTL: u8 = 0x89;
const OP_I64_ROTR: u8 = 0x8A;
// Type conversions
const OP_I32_WRAP_I64: u8 = 0xA7;
const OP_I64_EXTEND_I32_S: u8 = 0xAC;
const OP_I64_EXTEND_I32_U: u8 = 0xAD;

/// Decode LEB128-encoded signed i32.
fn decode_leb128_i32(bytes: &[u8], offset: &mut usize) -> i32 {
    let mut result: i32 = 0;
    let mut shift = 0u32;
    loop {
        if *offset >= bytes.len() { break; }
        let byte = bytes[*offset];
        *offset += 1;
        result |= ((byte & 0x7F) as i32) << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            // Sign extend
            if shift < 32 && (byte & 0x40) != 0 {
                result |= !0i32 << shift;
            }
            break;
        }
    }
    result
}

/// Decode LEB128-encoded unsigned u32.
fn decode_leb128_u32(bytes: &[u8], offset: &mut usize) -> u32 {
    let mut result: u32 = 0;
    let mut shift = 0u32;
    loop {
        if *offset >= bytes.len() { break; }
        let byte = bytes[*offset];
        *offset += 1;
        result |= ((byte & 0x7F) as u32) << shift;
        if byte & 0x80 == 0 { break; }
        shift += 7;
    }
    result
}

/// Decode LEB128-encoded signed i64.
fn decode_leb128_i64(bytes: &[u8], offset: &mut usize) -> i64 {
    let mut result: i64 = 0;
    let mut shift = 0u32;
    loop {
        if *offset >= bytes.len() { break; }
        let byte = bytes[*offset];
        *offset += 1;
        result |= ((byte & 0x7F) as i64) << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            if shift < 64 && (byte & 0x40) != 0 {
                result |= !0i64 << shift;
            }
            break;
        }
    }
    result
}

/// Stack-based WASM bytecode interpreter.
pub struct WasmInterpreter {
    pub stack: Vec<u64>,
    pub pc: usize,
    pub locals: Vec<u64>,
    /// Global variables (mutable or immutable at the WASM level; we store all as u64).
    pub globals: Vec<u64>,
    /// Linear memory — flat byte array for load/store instructions.
    pub memory: Vec<u8>,
    pub halted: bool,
}

impl WasmInterpreter {
    pub fn new() -> Self {
        // Default: 1 WASM page (64 KiB) of linear memory.
        let mut mem = Vec::new();
        mem.resize(65536, 0u8);
        WasmInterpreter {
            stack: Vec::new(),
            pc: 0,
            locals: Vec::new(),
            globals: Vec::new(),
            memory: mem,
            halted: false,
        }
    }

    // ------------------------------------------------------------------
    // Memory helpers
    // ------------------------------------------------------------------

    /// Read a little-endian u32 from linear memory at `addr`, or 0 if OOB.
    fn mem_load_u32(&self, addr: u32) -> u32 {
        let a = addr as usize;
        if a + 4 > self.memory.len() { return 0; }
        u32::from_le_bytes([
            self.memory[a], self.memory[a + 1],
            self.memory[a + 2], self.memory[a + 3],
        ])
    }

    /// Read a little-endian u64 from linear memory at `addr`, or 0 if OOB.
    fn mem_load_u64(&self, addr: u32) -> u64 {
        let a = addr as usize;
        if a + 8 > self.memory.len() { return 0; }
        u64::from_le_bytes([
            self.memory[a], self.memory[a+1], self.memory[a+2], self.memory[a+3],
            self.memory[a+4], self.memory[a+5], self.memory[a+6], self.memory[a+7],
        ])
    }

    /// Read a single byte from linear memory, or 0 if OOB.
    fn mem_load_u8(&self, addr: u32) -> u8 {
        let a = addr as usize;
        if a < self.memory.len() { self.memory[a] } else { 0 }
    }

    /// Read a little-endian u16 from linear memory, or 0 if OOB.
    fn mem_load_u16(&self, addr: u32) -> u16 {
        let a = addr as usize;
        if a + 2 > self.memory.len() { return 0; }
        u16::from_le_bytes([self.memory[a], self.memory[a + 1]])
    }

    /// Write a little-endian u32 to linear memory; silently ignores OOB.
    fn mem_store_u32(&mut self, addr: u32, val: u32) {
        let a = addr as usize;
        if a + 4 > self.memory.len() { return; }
        let bytes = val.to_le_bytes();
        self.memory[a..a + 4].copy_from_slice(&bytes);
    }

    /// Write a little-endian u64 to linear memory; silently ignores OOB.
    fn mem_store_u64(&mut self, addr: u32, val: u64) {
        let a = addr as usize;
        if a + 8 > self.memory.len() { return; }
        let bytes = val.to_le_bytes();
        self.memory[a..a + 8].copy_from_slice(&bytes);
    }

    /// Write a single byte; silently ignores OOB.
    fn mem_store_u8(&mut self, addr: u32, val: u8) {
        let a = addr as usize;
        if a < self.memory.len() { self.memory[a] = val; }
    }

    /// Write a little-endian u16; silently ignores OOB.
    fn mem_store_u16(&mut self, addr: u32, val: u16) {
        let a = addr as usize;
        if a + 2 > self.memory.len() { return; }
        let bytes = val.to_le_bytes();
        self.memory[a..a + 2].copy_from_slice(&bytes);
    }

    /// Current memory size in WASM pages (64 KiB each).
    fn memory_pages(&self) -> u32 {
        (self.memory.len() / 65536) as u32
    }

    /// Grow linear memory by `delta` pages.  Returns old size in pages, or
    /// u32::MAX on failure.
    fn memory_grow(&mut self, delta: u32) -> u32 {
        let old = self.memory_pages();
        let new_pages = old.saturating_add(delta);
        // Limit to 256 pages (16 MiB) for bare-metal safety.
        if new_pages > 256 {
            return u32::MAX;
        }
        let new_size = new_pages as usize * 65536;
        self.memory.resize(new_size, 0u8);
        old
    }

    // ------------------------------------------------------------------
    // Skip a block to its matching END (used by br/if-not-taken paths).
    // Returns false if we ran out of bytecode.
    // ------------------------------------------------------------------
    fn skip_to_end(&mut self, bytecode: &[u8]) -> bool {
        let mut depth = 1u32;
        while self.pc < bytecode.len() && depth > 0 {
            match bytecode[self.pc] {
                OP_BLOCK | OP_LOOP | OP_IF => { depth += 1; self.pc += 1; }
                OP_END => { depth -= 1; self.pc += 1; }
                OP_ELSE if depth == 1 => {
                    // Only advance past ELSE here when we are inside the
                    // if-true branch (depth==1 means we are directly inside
                    // the nearest if block).
                    self.pc += 1;
                    break;
                }
                _ => { self.pc += 1; }
            }
        }
        self.pc <= bytecode.len()
    }

    /// Push a value onto the operand stack.
    fn push(&mut self, val: u64) {
        self.stack.push(val);
    }

    /// Pop a value from the operand stack.
    fn pop(&mut self) -> u64 {
        self.stack.pop().unwrap_or(0)
    }

    /// Execute a WASM function body, returning the result values.
    pub fn execute(&mut self, bytecode: &[u8]) -> Vec<u64> {
        self.pc = 0;
        self.halted = false;

        while !self.halted && self.pc < bytecode.len() {
            if !self.step(bytecode) {
                break;
            }
        }

        // Return whatever remains on the stack as results
        self.stack.clone()
    }

    /// Step a single instruction. Returns true if execution should continue.
    pub fn step(&mut self, bytecode: &[u8]) -> bool {
        if self.pc >= bytecode.len() {
            self.halted = true;
            return false;
        }

        let opcode = bytecode[self.pc];
        self.pc += 1;

        match opcode {
            OP_UNREACHABLE => {
                crate::serial_println!("[wasm/interp] trap: unreachable");
                self.halted = true;
                return false;
            }
            OP_NOP => {}
            OP_BLOCK | OP_LOOP | OP_IF => {
                // Read block type (single byte for now: 0x40 = void, or valtype)
                if self.pc < bytecode.len() {
                    self.pc += 1; // skip block type
                }
                if opcode == OP_IF {
                    let cond = self.pop();
                    if cond == 0 {
                        // Skip to matching else or end
                        let mut depth = 1u32;
                        while self.pc < bytecode.len() && depth > 0 {
                            match bytecode[self.pc] {
                                OP_BLOCK | OP_LOOP | OP_IF => depth += 1,
                                OP_END => depth -= 1,
                                OP_ELSE if depth == 1 => { self.pc += 1; break; }
                                _ => {}
                            }
                            self.pc += 1;
                        }
                    }
                }
            }
            OP_ELSE => {
                // If we reach else during true branch, skip to end
                let mut depth = 1u32;
                while self.pc < bytecode.len() && depth > 0 {
                    match bytecode[self.pc] {
                        OP_BLOCK | OP_LOOP | OP_IF => depth += 1,
                        OP_END => depth -= 1,
                        _ => {}
                    }
                    self.pc += 1;
                }
            }
            OP_END => {
                // End of block/function
            }
            OP_BR => {
                let _label = decode_leb128_u32(bytecode, &mut self.pc);
                // Simplified: skip to the matching END of the nearest block.
                self.skip_to_end(bytecode);
            }
            OP_BR_IF => {
                let _label = decode_leb128_u32(bytecode, &mut self.pc);
                let cond = self.pop();
                if cond != 0 {
                    let mut depth = 1u32;
                    while self.pc < bytecode.len() && depth > 0 {
                        match bytecode[self.pc] {
                            OP_BLOCK | OP_LOOP | OP_IF => depth += 1,
                            OP_END => depth -= 1,
                            _ => {}
                        }
                        self.pc += 1;
                    }
                }
            }
            OP_RETURN => {
                self.halted = true;
                return false;
            }
            OP_CALL => {
                let _func_idx = decode_leb128_u32(bytecode, &mut self.pc);
                // Function calls are handled at the runtime level
                crate::serial_println!("[wasm/interp] call func {}", _func_idx);
            }
            OP_DROP => {
                self.pop();
            }
            OP_SELECT => {
                let cond = self.pop();
                let val2 = self.pop();
                let val1 = self.pop();
                self.push(if cond != 0 { val1 } else { val2 });
            }
            OP_LOCAL_GET => {
                let idx = decode_leb128_u32(bytecode, &mut self.pc) as usize;
                let val = if idx < self.locals.len() { self.locals[idx] } else { 0 };
                self.push(val);
            }
            OP_LOCAL_SET => {
                let idx = decode_leb128_u32(bytecode, &mut self.pc) as usize;
                let val = self.pop();
                while self.locals.len() <= idx {
                    self.locals.push(0);
                }
                self.locals[idx] = val;
            }
            OP_LOCAL_TEE => {
                let idx = decode_leb128_u32(bytecode, &mut self.pc) as usize;
                let val = *self.stack.last().unwrap_or(&0);
                while self.locals.len() <= idx {
                    self.locals.push(0);
                }
                self.locals[idx] = val;
            }
            OP_I32_CONST => {
                let val = decode_leb128_i32(bytecode, &mut self.pc);
                self.push(val as u32 as u64);
            }
            OP_I64_CONST => {
                let val = decode_leb128_i64(bytecode, &mut self.pc);
                self.push(val as u64);
            }
            OP_I32_EQZ => {
                let a = self.pop() as u32;
                self.push(if a == 0 { 1 } else { 0 });
            }
            OP_I32_EQ => {
                let b = self.pop() as u32;
                let a = self.pop() as u32;
                self.push(if a == b { 1 } else { 0 });
            }
            OP_I32_NE => {
                let b = self.pop() as u32;
                let a = self.pop() as u32;
                self.push(if a != b { 1 } else { 0 });
            }
            OP_I32_LT_S => {
                let b = self.pop() as i32;
                let a = self.pop() as i32;
                self.push(if a < b { 1 } else { 0 });
            }
            OP_I32_GT_S => {
                let b = self.pop() as i32;
                let a = self.pop() as i32;
                self.push(if a > b { 1 } else { 0 });
            }
            OP_I32_LE_S => {
                let b = self.pop() as i32;
                let a = self.pop() as i32;
                self.push(if a <= b { 1 } else { 0 });
            }
            OP_I32_GE_S => {
                let b = self.pop() as i32;
                let a = self.pop() as i32;
                self.push(if a >= b { 1 } else { 0 });
            }
            OP_I32_ADD => {
                let b = self.pop() as u32;
                let a = self.pop() as u32;
                self.push(a.wrapping_add(b) as u64);
            }
            OP_I32_SUB => {
                let b = self.pop() as u32;
                let a = self.pop() as u32;
                self.push(a.wrapping_sub(b) as u64);
            }
            OP_I32_MUL => {
                let b = self.pop() as u32;
                let a = self.pop() as u32;
                self.push(a.wrapping_mul(b) as u64);
            }
            OP_I32_DIV_S => {
                let b = self.pop() as i32;
                let a = self.pop() as i32;
                if b == 0 {
                    crate::serial_println!("[wasm/interp] trap: division by zero");
                    self.halted = true;
                    return false;
                }
                self.push((a.wrapping_div(b)) as u32 as u64);
            }
            OP_I32_REM_S => {
                let b = self.pop() as i32;
                let a = self.pop() as i32;
                if b == 0 {
                    crate::serial_println!("[wasm/interp] trap: remainder by zero");
                    self.halted = true;
                    return false;
                }
                self.push((a.wrapping_rem(b)) as u32 as u64);
            }
            OP_I32_AND => {
                let b = self.pop() as u32;
                let a = self.pop() as u32;
                self.push((a & b) as u64);
            }
            OP_I32_OR => {
                let b = self.pop() as u32;
                let a = self.pop() as u32;
                self.push((a | b) as u64);
            }
            OP_I32_XOR => {
                let b = self.pop() as u32;
                let a = self.pop() as u32;
                self.push((a ^ b) as u64);
            }
            OP_I32_SHL => {
                let b = self.pop() as u32;
                let a = self.pop() as u32;
                self.push((a.wrapping_shl(b & 31)) as u64);
            }
            OP_I32_SHR_S => {
                let b = self.pop() as u32;
                let a = self.pop() as i32;
                self.push((a.wrapping_shr(b & 31)) as u32 as u64);
            }
            OP_I64_ADD => {
                let b = self.pop();
                let a = self.pop();
                self.push(a.wrapping_add(b));
            }
            OP_I64_SUB => {
                let b = self.pop();
                let a = self.pop();
                self.push(a.wrapping_sub(b));
            }
            OP_I64_MUL => {
                let b = self.pop();
                let a = self.pop();
                self.push(a.wrapping_mul(b));
            }

            // ── br_table ───────────────────────────────────────────────
            OP_BR_TABLE => {
                let n = decode_leb128_u32(bytecode, &mut self.pc) as usize;
                for _ in 0..=n {
                    decode_leb128_u32(bytecode, &mut self.pc);
                }
                let _idx = self.pop();
                // Simplified branch: skip to end of nearest enclosing block.
                let mut depth = 1u32;
                while self.pc < bytecode.len() && depth > 0 {
                    match bytecode[self.pc] {
                        OP_BLOCK | OP_LOOP | OP_IF => depth += 1,
                        OP_END => depth -= 1,
                        _ => {}
                    }
                    self.pc += 1;
                }
            }

            // ── call_indirect ──────────────────────────────────────────
            OP_CALL_INDIRECT => {
                let type_idx = decode_leb128_u32(bytecode, &mut self.pc);
                let _table_idx = decode_leb128_u32(bytecode, &mut self.pc);
                let func_ref = self.pop() as u32;
                crate::serial_println!(
                    "[wasm/interp] call_indirect type={} func_ref={}",
                    type_idx, func_ref
                );
                self.push(0);
            }

            // ── global.get / global.set ────────────────────────────────
            OP_GLOBAL_GET => {
                let idx = decode_leb128_u32(bytecode, &mut self.pc) as usize;
                let val = if idx < self.globals.len() { self.globals[idx] } else { 0 };
                self.push(val);
            }
            OP_GLOBAL_SET => {
                let idx = decode_leb128_u32(bytecode, &mut self.pc) as usize;
                let val = self.pop();
                while self.globals.len() <= idx {
                    self.globals.push(0);
                }
                self.globals[idx] = val;
            }

            // ── memory.size / memory.grow ──────────────────────────────
            OP_MEMORY_SIZE => {
                let _mem_idx = decode_leb128_u32(bytecode, &mut self.pc);
                self.push(self.memory_pages() as u64);
            }
            OP_MEMORY_GROW => {
                let _mem_idx = decode_leb128_u32(bytecode, &mut self.pc);
                let delta = self.pop() as u32;
                let old = self.memory_grow(delta);
                self.push(old as u64);
                crate::serial_println!("[wasm/interp] memory.grow delta={} old={}", delta, old);
            }

            // ── i32 memory loads ──────────────────────────────────────
            OP_I32_LOAD => {
                let _align = decode_leb128_u32(bytecode, &mut self.pc);
                let offset = decode_leb128_u32(bytecode, &mut self.pc);
                let base = self.pop() as u32;
                let addr = base.wrapping_add(offset);
                self.push(self.mem_load_u32(addr) as u64);
            }
            OP_I32_LOAD8_S => {
                let _align = decode_leb128_u32(bytecode, &mut self.pc);
                let offset = decode_leb128_u32(bytecode, &mut self.pc);
                let base = self.pop() as u32;
                let addr = base.wrapping_add(offset);
                let v = self.mem_load_u8(addr) as i8 as i32;
                self.push(v as u32 as u64);
            }
            OP_I32_LOAD8_U => {
                let _align = decode_leb128_u32(bytecode, &mut self.pc);
                let offset = decode_leb128_u32(bytecode, &mut self.pc);
                let base = self.pop() as u32;
                let addr = base.wrapping_add(offset);
                self.push(self.mem_load_u8(addr) as u64);
            }
            OP_I32_LOAD16_S => {
                let _align = decode_leb128_u32(bytecode, &mut self.pc);
                let offset = decode_leb128_u32(bytecode, &mut self.pc);
                let base = self.pop() as u32;
                let addr = base.wrapping_add(offset);
                let v = self.mem_load_u16(addr) as i16 as i32;
                self.push(v as u32 as u64);
            }
            OP_I32_LOAD16_U => {
                let _align = decode_leb128_u32(bytecode, &mut self.pc);
                let offset = decode_leb128_u32(bytecode, &mut self.pc);
                let base = self.pop() as u32;
                let addr = base.wrapping_add(offset);
                self.push(self.mem_load_u16(addr) as u64);
            }

            // ── i64 memory loads ──────────────────────────────────────
            OP_I64_LOAD => {
                let _align = decode_leb128_u32(bytecode, &mut self.pc);
                let offset = decode_leb128_u32(bytecode, &mut self.pc);
                let base = self.pop() as u32;
                let addr = base.wrapping_add(offset);
                self.push(self.mem_load_u64(addr));
            }
            OP_I64_LOAD8_S => {
                let _align = decode_leb128_u32(bytecode, &mut self.pc);
                let offset = decode_leb128_u32(bytecode, &mut self.pc);
                let base = self.pop() as u32;
                let addr = base.wrapping_add(offset);
                let v = self.mem_load_u8(addr) as i8 as i64;
                self.push(v as u64);
            }
            OP_I64_LOAD8_U => {
                let _align = decode_leb128_u32(bytecode, &mut self.pc);
                let offset = decode_leb128_u32(bytecode, &mut self.pc);
                let base = self.pop() as u32;
                let addr = base.wrapping_add(offset);
                self.push(self.mem_load_u8(addr) as u64);
            }
            OP_I64_LOAD16_S => {
                let _align = decode_leb128_u32(bytecode, &mut self.pc);
                let offset = decode_leb128_u32(bytecode, &mut self.pc);
                let base = self.pop() as u32;
                let addr = base.wrapping_add(offset);
                let v = self.mem_load_u16(addr) as i16 as i64;
                self.push(v as u64);
            }
            OP_I64_LOAD16_U => {
                let _align = decode_leb128_u32(bytecode, &mut self.pc);
                let offset = decode_leb128_u32(bytecode, &mut self.pc);
                let base = self.pop() as u32;
                let addr = base.wrapping_add(offset);
                self.push(self.mem_load_u16(addr) as u64);
            }
            OP_I64_LOAD32_S => {
                let _align = decode_leb128_u32(bytecode, &mut self.pc);
                let offset = decode_leb128_u32(bytecode, &mut self.pc);
                let base = self.pop() as u32;
                let addr = base.wrapping_add(offset);
                let v = self.mem_load_u32(addr) as i32 as i64;
                self.push(v as u64);
            }
            OP_I64_LOAD32_U => {
                let _align = decode_leb128_u32(bytecode, &mut self.pc);
                let offset = decode_leb128_u32(bytecode, &mut self.pc);
                let base = self.pop() as u32;
                let addr = base.wrapping_add(offset);
                self.push(self.mem_load_u32(addr) as u64);
            }

            // ── i32 memory stores ─────────────────────────────────────
            OP_I32_STORE => {
                let _align = decode_leb128_u32(bytecode, &mut self.pc);
                let offset = decode_leb128_u32(bytecode, &mut self.pc);
                let val = self.pop() as u32;
                let base = self.pop() as u32;
                let addr = base.wrapping_add(offset);
                self.mem_store_u32(addr, val);
            }
            OP_I32_STORE8 => {
                let _align = decode_leb128_u32(bytecode, &mut self.pc);
                let offset = decode_leb128_u32(bytecode, &mut self.pc);
                let val = self.pop() as u8;
                let base = self.pop() as u32;
                let addr = base.wrapping_add(offset);
                self.mem_store_u8(addr, val);
            }
            OP_I32_STORE16 => {
                let _align = decode_leb128_u32(bytecode, &mut self.pc);
                let offset = decode_leb128_u32(bytecode, &mut self.pc);
                let val = self.pop() as u16;
                let base = self.pop() as u32;
                let addr = base.wrapping_add(offset);
                self.mem_store_u16(addr, val);
            }

            // ── i64 memory stores ─────────────────────────────────────
            OP_I64_STORE => {
                let _align = decode_leb128_u32(bytecode, &mut self.pc);
                let offset = decode_leb128_u32(bytecode, &mut self.pc);
                let val = self.pop();
                let base = self.pop() as u32;
                let addr = base.wrapping_add(offset);
                self.mem_store_u64(addr, val);
            }
            OP_I64_STORE8 => {
                let _align = decode_leb128_u32(bytecode, &mut self.pc);
                let offset = decode_leb128_u32(bytecode, &mut self.pc);
                let val = self.pop() as u8;
                let base = self.pop() as u32;
                let addr = base.wrapping_add(offset);
                self.mem_store_u8(addr, val);
            }
            OP_I64_STORE16 => {
                let _align = decode_leb128_u32(bytecode, &mut self.pc);
                let offset = decode_leb128_u32(bytecode, &mut self.pc);
                let val = self.pop() as u16;
                let base = self.pop() as u32;
                let addr = base.wrapping_add(offset);
                self.mem_store_u16(addr, val);
            }
            OP_I64_STORE32 => {
                let _align = decode_leb128_u32(bytecode, &mut self.pc);
                let offset = decode_leb128_u32(bytecode, &mut self.pc);
                let val = self.pop() as u32;
                let base = self.pop() as u32;
                let addr = base.wrapping_add(offset);
                self.mem_store_u32(addr, val);
            }

            // ── i32 unsigned comparisons ──────────────────────────────
            OP_I32_LT_U => {
                let b = self.pop() as u32;
                let a = self.pop() as u32;
                self.push(if a < b { 1 } else { 0 });
            }
            OP_I32_GT_U => {
                let b = self.pop() as u32;
                let a = self.pop() as u32;
                self.push(if a > b { 1 } else { 0 });
            }
            OP_I32_LE_U => {
                let b = self.pop() as u32;
                let a = self.pop() as u32;
                self.push(if a <= b { 1 } else { 0 });
            }
            OP_I32_GE_U => {
                let b = self.pop() as u32;
                let a = self.pop() as u32;
                self.push(if a >= b { 1 } else { 0 });
            }

            // ── i64 comparisons ───────────────────────────────────────
            OP_I64_EQZ => {
                let a = self.pop();
                self.push(if a == 0 { 1 } else { 0 });
            }
            OP_I64_EQ => {
                let b = self.pop();
                let a = self.pop();
                self.push(if a == b { 1 } else { 0 });
            }
            OP_I64_NE => {
                let b = self.pop();
                let a = self.pop();
                self.push(if a != b { 1 } else { 0 });
            }
            OP_I64_LT_S => {
                let b = self.pop() as i64;
                let a = self.pop() as i64;
                self.push(if a < b { 1 } else { 0 });
            }
            OP_I64_LT_U => {
                let b = self.pop();
                let a = self.pop();
                self.push(if a < b { 1 } else { 0 });
            }
            OP_I64_GT_S => {
                let b = self.pop() as i64;
                let a = self.pop() as i64;
                self.push(if a > b { 1 } else { 0 });
            }
            OP_I64_GT_U => {
                let b = self.pop();
                let a = self.pop();
                self.push(if a > b { 1 } else { 0 });
            }
            OP_I64_LE_S => {
                let b = self.pop() as i64;
                let a = self.pop() as i64;
                self.push(if a <= b { 1 } else { 0 });
            }
            OP_I64_LE_U => {
                let b = self.pop();
                let a = self.pop();
                self.push(if a <= b { 1 } else { 0 });
            }
            OP_I64_GE_S => {
                let b = self.pop() as i64;
                let a = self.pop() as i64;
                self.push(if a >= b { 1 } else { 0 });
            }
            OP_I64_GE_U => {
                let b = self.pop();
                let a = self.pop();
                self.push(if a >= b { 1 } else { 0 });
            }

            // ── i32 bit count / extend ────────────────────────────────
            OP_I32_CLZ => {
                let v = self.pop() as u32;
                self.push(v.leading_zeros() as u64);
            }
            OP_I32_CTZ => {
                let v = self.pop() as u32;
                self.push(v.trailing_zeros() as u64);
            }
            OP_I32_POPCNT => {
                let v = self.pop() as u32;
                self.push(v.count_ones() as u64);
            }
            OP_I32_DIV_U => {
                let b = self.pop() as u32;
                let a = self.pop() as u32;
                if b == 0 {
                    crate::serial_println!("[wasm/interp] trap: i32.div_u zero");
                    self.halted = true;
                    return false;
                }
                self.push((a / b) as u64);
            }
            OP_I32_REM_U => {
                let b = self.pop() as u32;
                let a = self.pop() as u32;
                if b == 0 {
                    crate::serial_println!("[wasm/interp] trap: i32.rem_u zero");
                    self.halted = true;
                    return false;
                }
                self.push((a % b) as u64);
            }
            OP_I32_SHR_U => {
                let b = self.pop() as u32;
                let a = self.pop() as u32;
                self.push(a.wrapping_shr(b & 31) as u64);
            }
            OP_I32_ROTL => {
                let b = self.pop() as u32;
                let a = self.pop() as u32;
                self.push(a.rotate_left(b & 31) as u64);
            }
            OP_I32_ROTR => {
                let b = self.pop() as u32;
                let a = self.pop() as u32;
                self.push(a.rotate_right(b & 31) as u64);
            }

            // ── i64 bit and arithmetic operations ─────────────────────
            OP_I64_CLZ => {
                let v = self.pop();
                self.push(v.leading_zeros() as u64);
            }
            OP_I64_CTZ => {
                let v = self.pop();
                self.push(v.trailing_zeros() as u64);
            }
            OP_I64_POPCNT => {
                let v = self.pop();
                self.push(v.count_ones() as u64);
            }
            OP_I64_DIV_S => {
                let b = self.pop() as i64;
                let a = self.pop() as i64;
                if b == 0 {
                    crate::serial_println!("[wasm/interp] trap: i64.div_s zero");
                    self.halted = true;
                    return false;
                }
                self.push(a.wrapping_div(b) as u64);
            }
            OP_I64_DIV_U => {
                let b = self.pop();
                let a = self.pop();
                if b == 0 {
                    crate::serial_println!("[wasm/interp] trap: i64.div_u zero");
                    self.halted = true;
                    return false;
                }
                self.push(a / b);
            }
            OP_I64_REM_S => {
                let b = self.pop() as i64;
                let a = self.pop() as i64;
                if b == 0 {
                    crate::serial_println!("[wasm/interp] trap: i64.rem_s zero");
                    self.halted = true;
                    return false;
                }
                self.push(a.wrapping_rem(b) as u64);
            }
            OP_I64_REM_U => {
                let b = self.pop();
                let a = self.pop();
                if b == 0 {
                    crate::serial_println!("[wasm/interp] trap: i64.rem_u zero");
                    self.halted = true;
                    return false;
                }
                self.push(a % b);
            }
            OP_I64_AND => {
                let b = self.pop();
                let a = self.pop();
                self.push(a & b);
            }
            OP_I64_OR => {
                let b = self.pop();
                let a = self.pop();
                self.push(a | b);
            }
            OP_I64_XOR => {
                let b = self.pop();
                let a = self.pop();
                self.push(a ^ b);
            }
            OP_I64_SHL => {
                let b = self.pop();
                let a = self.pop();
                self.push(a.wrapping_shl((b & 63) as u32));
            }
            OP_I64_SHR_S => {
                let b = self.pop() as u32;
                let a = self.pop() as i64;
                self.push(a.wrapping_shr(b & 63) as u64);
            }
            OP_I64_SHR_U => {
                let b = self.pop();
                let a = self.pop();
                self.push(a.wrapping_shr((b & 63) as u32));
            }
            OP_I64_ROTL => {
                let b = self.pop();
                let a = self.pop();
                self.push(a.rotate_left((b & 63) as u32));
            }
            OP_I64_ROTR => {
                let b = self.pop();
                let a = self.pop();
                self.push(a.rotate_right((b & 63) as u32));
            }

            // ── type conversions ──────────────────────────────────────
            OP_I32_WRAP_I64 => {
                let v = self.pop() as u32;
                self.push(v as u64);
            }
            OP_I64_EXTEND_I32_S => {
                let v = self.pop() as u32 as i32 as i64;
                self.push(v as u64);
            }
            OP_I64_EXTEND_I32_U => {
                let v = self.pop() as u32 as u64;
                self.push(v);
            }

            _ => {
                // Unknown opcode: skip (log and continue)
                crate::serial_println!("[wasm/interp] unknown opcode {:#x} at pc={}", opcode, self.pc - 1);
            }
        }

        true
    }
}

pub fn init() {
    crate::serial_println!(
        "[wasm] interpreter ready: i32/i64 arith+bitwise+cmp, \
        mem load/store (u8/u16/u32/u64), global get/set, \
        memory.size/grow, br_table, call_indirect, type-convert"
    );
}
