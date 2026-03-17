/// WASM to native JIT compiler.
///
/// Part of the AIOS wasm subsystem.

use alloc::vec::Vec;
use alloc::vec;

/// JIT compiler that translates WASM bytecode to native machine code.
///
/// This is a baseline compiler that generates simple x86_64 machine code
/// for each WASM opcode without optimizations. Functions are compiled
/// one at a time and the result is stored in a code buffer.
pub struct JitCompiler {
    code_buffer: Vec<u8>,
    opt_level: OptLevel,
    compile_count: u64,
}

/// Optimization level for JIT compilation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptLevel {
    None,
    Basic,
    Aggressive,
}

impl JitCompiler {
    pub fn new() -> Self {
        JitCompiler {
            code_buffer: Vec::new(),
            opt_level: OptLevel::None,
            compile_count: 0,
        }
    }

    /// Compile a WASM function to native code, returning executable bytes.
    ///
    /// Generates a minimal x86_64 function prologue/epilogue wrapping the
    /// translated bytecode. In a real JIT, each WASM opcode would be lowered
    /// to native instructions; here we emit a stub that returns immediately.
    pub fn compile_function(&mut self, func_body: &[u8]) -> Vec<u8> {
        self.compile_count = self.compile_count.saturating_add(1);
        self.code_buffer.clear();

        // x86_64 function prologue
        // push rbp
        self.code_buffer.push(0x55);
        // mov rbp, rsp
        self.code_buffer.extend_from_slice(&[0x48, 0x89, 0xE5]);

        // For each WASM opcode, emit a nop (0x90) as a placeholder.
        // A real compiler would lower each opcode to x86_64 instructions.
        for &byte in func_body {
            match byte {
                0x0B => break, // end of function
                0x01 => {
                    // nop -> nop
                    self.code_buffer.push(0x90);
                }
                0x41 => {
                    // i32.const: would push to stack via native push
                    self.code_buffer.push(0x90);
                }
                0x6A => {
                    // i32.add: would pop two, add, push result
                    self.code_buffer.push(0x90);
                }
                0x6B => {
                    // i32.sub
                    self.code_buffer.push(0x90);
                }
                0x6C => {
                    // i32.mul
                    self.code_buffer.push(0x90);
                }
                _ => {
                    // Unknown opcode: emit nop
                    self.code_buffer.push(0x90);
                }
            }
        }

        // x86_64 function epilogue
        // xor eax, eax (return 0)
        self.code_buffer.extend_from_slice(&[0x31, 0xC0]);
        // pop rbp
        self.code_buffer.push(0x5D);
        // ret
        self.code_buffer.push(0xC3);

        crate::serial_println!(
            "[wasm/compiler] compiled function #{}: {} WASM bytes -> {} native bytes (opt={:?})",
            self.compile_count, func_body.len(), self.code_buffer.len(), self.opt_level
        );

        self.code_buffer.clone()
    }

    /// Set the optimization level.
    pub fn set_opt_level(&mut self, level: OptLevel) {
        self.opt_level = level;
    }

    /// Number of functions compiled.
    pub fn compile_count(&self) -> u64 {
        self.compile_count
    }
}

pub fn init() {
    crate::serial_println!("[wasm] JIT compiler ready (baseline)");
}
