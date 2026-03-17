/// JIT compilation to native code
///
/// Part of the AIOS.

use alloc::vec::Vec;
use crate::sync::Mutex;

/// JIT compiler that translates WASM bytecode to native machine code.
///
/// This is a secondary JIT compiler module that works with the raw bytecode
/// buffer approach. It generates x86_64 machine code from WASM opcodes.
pub struct JitCompiler {
    code_buffer: Vec<u8>,
    optimization_level: u8,
    compile_count: u64,
    total_native_bytes: usize,
}

impl JitCompiler {
    pub fn new() -> Self {
        JitCompiler {
            code_buffer: Vec::new(),
            optimization_level: 0,
            compile_count: 0,
            total_native_bytes: 0,
        }
    }

    /// Compile a WASM function body to native code.
    ///
    /// Generates x86_64 instructions. The output is a self-contained function
    /// with prologue and epilogue. The compilation is single-pass baseline.
    pub fn compile(&mut self, bytecode: &[u8]) -> Vec<u8> {
        self.compile_count = self.compile_count.saturating_add(1);
        self.code_buffer.clear();

        // Function prologue: push rbp; mov rbp, rsp; sub rsp, 64
        self.emit(&[0x55]);                         // push rbp
        self.emit(&[0x48, 0x89, 0xE5]);             // mov rbp, rsp
        self.emit(&[0x48, 0x83, 0xEC, 0x40]);       // sub rsp, 64

        let mut pc = 0;
        while pc < bytecode.len() {
            let op = bytecode[pc];
            pc += 1;

            match op {
                0x01 => {
                    // nop
                    self.emit(&[0x90]);
                }
                0x0B => {
                    // end - stop compiling this function
                    break;
                }
                0x41 => {
                    // i32.const followed by LEB128 immediate
                    // Read the immediate (simplified: single byte for small values)
                    let mut val: i32 = 0;
                    let mut shift = 0u32;
                    while pc < bytecode.len() {
                        let b = bytecode[pc];
                        pc += 1;
                        val |= ((b & 0x7F) as i32) << shift;
                        if b & 0x80 == 0 {
                            if shift < 32 && (b & 0x40) != 0 {
                                val |= !0i32 << (shift + 7);
                            }
                            break;
                        }
                        shift += 7;
                    }
                    // push immediate: push imm32
                    self.emit(&[0x68]);
                    self.emit(&(val as u32).to_le_bytes());
                }
                0x6A => {
                    // i32.add: pop two, add, push result
                    self.emit(&[0x58]);              // pop rax
                    self.emit(&[0x5B]);              // pop rbx (actually rcx but using simplified)
                    self.emit(&[0x01, 0xD8]);        // add eax, ebx
                    self.emit(&[0x50]);              // push rax
                }
                0x6B => {
                    // i32.sub: pop two, sub, push result
                    self.emit(&[0x5B]);              // pop rbx (subtrahend)
                    self.emit(&[0x58]);              // pop rax (minuend)
                    self.emit(&[0x29, 0xD8]);        // sub eax, ebx
                    self.emit(&[0x50]);              // push rax
                }
                0x6C => {
                    // i32.mul
                    self.emit(&[0x5B]);              // pop rbx
                    self.emit(&[0x58]);              // pop rax
                    self.emit(&[0x0F, 0xAF, 0xC3]); // imul eax, ebx
                    self.emit(&[0x50]);              // push rax
                }
                0x1A => {
                    // drop: pop and discard
                    self.emit(&[0x58]);              // pop rax
                }
                _ => {
                    // Unknown opcode: emit nop
                    self.emit(&[0x90]);
                }
            }
        }

        // Function epilogue: result in rax from stack top
        self.emit(&[0x58]);                         // pop rax (return value)
        self.emit(&[0x48, 0x89, 0xEC]);             // mov rsp, rbp
        self.emit(&[0x5D]);                         // pop rbp
        self.emit(&[0xC3]);                         // ret

        self.total_native_bytes += self.code_buffer.len();

        crate::serial_println!(
            "[wasm/jit] compiled function #{}: {} WASM bytes -> {} native bytes (opt={})",
            self.compile_count, bytecode.len(), self.code_buffer.len(), self.optimization_level
        );

        self.code_buffer.clone()
    }

    /// Emit bytes to the code buffer.
    fn emit(&mut self, bytes: &[u8]) {
        self.code_buffer.extend_from_slice(bytes);
    }

    /// Set the optimization level (0 = none, 3 = aggressive).
    pub fn set_opt_level(&mut self, level: u8) {
        self.optimization_level = level.min(3);
    }

    /// Total number of native bytes generated.
    pub fn total_bytes(&self) -> usize {
        self.total_native_bytes
    }

    /// Number of functions compiled.
    pub fn compile_count(&self) -> u64 {
        self.compile_count
    }
}

pub fn init() {
    crate::serial_println!("[wasm] JIT (secondary) ready");
}
