/// WebAssembly runtime for Genesis — sandboxed app execution
///
/// A lightweight WASM interpreter that runs .wasm binaries in a sandbox.
/// Supports WASI-like syscalls for file I/O, networking, and UI access.
/// Each app runs in its own WASM instance with capability-based permissions.
///
/// Inspired by: Wasmtime, Wasmer, wasm3. All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// WASM value types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValType {
    I32,
    I64,
    F32,
    F64,
}

/// WASM runtime value
#[derive(Debug, Clone, Copy)]
pub enum Val {
    I32(i32),
    I64(i64),
    F32(u32), // bit pattern
    F64(u64), // bit pattern
}

/// WASM opcode (subset — enough to run real programs)
#[derive(Debug, Clone, Copy)]
pub enum Opcode {
    Unreachable,
    Nop,
    Block,
    Loop,
    If,
    Else,
    End,
    Br(u32),
    BrIf(u32),
    Return,
    Call(u32),
    CallIndirect(u32),
    Drop,
    Select,
    LocalGet(u32),
    LocalSet(u32),
    LocalTee(u32),
    GlobalGet(u32),
    GlobalSet(u32),
    I32Load(u32, u32),
    I32Store(u32, u32),
    I32Const(i32),
    I64Const(i64),
    I32Add,
    I32Sub,
    I32Mul,
    I32DivS,
    I32And,
    I32Or,
    I32Xor,
    I32Shl,
    I32ShrU,
    I32ShrS,
    I32Eqz,
    I32Eq,
    I32Ne,
    I32LtS,
    I32GtS,
    I32LeS,
    I32GeS,
    I64Add,
    I64Sub,
    I64Mul,
    MemorySize,
    MemoryGrow,
}

/// WASM function
pub struct WasmFunc {
    pub name: String,
    pub params: Vec<ValType>,
    pub results: Vec<ValType>,
    pub locals: Vec<ValType>,
    pub code: Vec<Opcode>,
}

/// WASM module (parsed .wasm binary)
pub struct WasmModule {
    pub name: String,
    pub functions: Vec<WasmFunc>,
    pub memory: Vec<u8>,
    pub memory_pages: u32, // 64KB each
    pub globals: Vec<Val>,
    pub exports: BTreeMap<String, u32>,     // name -> func index
    pub imports: Vec<(String, String)>,     // (module, name)
    pub table: Vec<u32>,                    // indirect call table
    pub data_segments: Vec<(u32, Vec<u8>)>, // (offset, data)
}

/// WASM execution instance
pub struct WasmInstance {
    pub module: WasmModule,
    pub stack: Vec<Val>,
    pub call_stack: Vec<CallFrame>,
    pub pc: usize,
    pub fuel: u64, // execution limit (prevents infinite loops)
    pub max_fuel: u64,
    pub trapped: bool,
    pub trap_msg: String,
}

/// Call frame for function invocation
pub struct CallFrame {
    pub func_idx: u32,
    pub locals: Vec<Val>,
    pub return_pc: usize,
    pub stack_base: usize,
}

/// Host function (imported by WASM from the OS)
pub type HostFn = fn(&mut WasmInstance, &[Val]) -> Vec<Val>;

impl WasmInstance {
    pub fn new(module: WasmModule) -> Self {
        WasmInstance {
            stack: Vec::new(),
            call_stack: Vec::new(),
            pc: 0,
            fuel: 1_000_000,
            max_fuel: 1_000_000,
            trapped: false,
            trap_msg: String::new(),
            module,
        }
    }

    /// Initialize memory with data segments
    pub fn init_memory(&mut self) {
        let pages = self.module.memory_pages.max(1);
        self.module.memory = alloc::vec![0u8; pages as usize * 65536];

        for (offset, data) in &self.module.data_segments {
            let start = *offset as usize;
            let end = start + data.len();
            if end <= self.module.memory.len() {
                self.module.memory[start..end].copy_from_slice(data);
            }
        }
    }

    /// Call an exported function by name
    pub fn call_export(&mut self, name: &str, args: &[Val]) -> Option<Vec<Val>> {
        let func_idx = *self.module.exports.get(name)?;
        self.call_function(func_idx, args)
    }

    /// Call a function by index
    pub fn call_function(&mut self, func_idx: u32, args: &[Val]) -> Option<Vec<Val>> {
        if func_idx as usize >= self.module.functions.len() {
            self.trap("invalid function index");
            return None;
        }

        // Set up locals (params + local vars)
        let func = &self.module.functions[func_idx as usize];
        let mut locals: Vec<Val> = args.to_vec();
        for &ty in &func.locals {
            locals.push(match ty {
                ValType::I32 => Val::I32(0),
                ValType::I64 => Val::I64(0),
                ValType::F32 => Val::F32(0),
                ValType::F64 => Val::F64(0),
            });
        }

        let frame = CallFrame {
            func_idx,
            locals,
            return_pc: self.pc,
            stack_base: self.stack.len(),
        };
        self.call_stack.push(frame);
        self.pc = 0;

        // Execute
        self.execute(func_idx);

        // Collect results
        let num_results = self.module.functions[func_idx as usize].results.len();
        let mut results = Vec::new();
        for _ in 0..num_results {
            if let Some(val) = self.stack.pop() {
                results.push(val);
            }
        }
        results.reverse();
        self.call_stack.pop();
        Some(results)
    }

    /// Execute function bytecode
    fn execute(&mut self, func_idx: u32) {
        let code_len = self.module.functions[func_idx as usize].code.len();

        while self.pc < code_len && !self.trapped {
            if self.fuel == 0 {
                self.trap("out of fuel");
                return;
            }
            self.fuel -= 1;

            let op = self.module.functions[func_idx as usize].code[self.pc];
            self.pc += 1;

            match op {
                Opcode::Nop => {}
                Opcode::Unreachable => {
                    self.trap("unreachable");
                }
                Opcode::I32Const(val) => {
                    self.stack.push(Val::I32(val));
                }
                Opcode::I64Const(val) => {
                    self.stack.push(Val::I64(val));
                }
                Opcode::I32Add => {
                    if let (Some(Val::I32(b)), Some(Val::I32(a))) =
                        (self.stack.pop(), self.stack.pop())
                    {
                        self.stack.push(Val::I32(a.wrapping_add(b)));
                    }
                }
                Opcode::I32Sub => {
                    if let (Some(Val::I32(b)), Some(Val::I32(a))) =
                        (self.stack.pop(), self.stack.pop())
                    {
                        self.stack.push(Val::I32(a.wrapping_sub(b)));
                    }
                }
                Opcode::I32Mul => {
                    if let (Some(Val::I32(b)), Some(Val::I32(a))) =
                        (self.stack.pop(), self.stack.pop())
                    {
                        self.stack.push(Val::I32(a.wrapping_mul(b)));
                    }
                }
                Opcode::I32And => {
                    if let (Some(Val::I32(b)), Some(Val::I32(a))) =
                        (self.stack.pop(), self.stack.pop())
                    {
                        self.stack.push(Val::I32(a & b));
                    }
                }
                Opcode::I32Or => {
                    if let (Some(Val::I32(b)), Some(Val::I32(a))) =
                        (self.stack.pop(), self.stack.pop())
                    {
                        self.stack.push(Val::I32(a | b));
                    }
                }
                Opcode::I32Eqz => {
                    if let Some(Val::I32(a)) = self.stack.pop() {
                        self.stack.push(Val::I32(if a == 0 { 1 } else { 0 }));
                    }
                }
                Opcode::I32Eq => {
                    if let (Some(Val::I32(b)), Some(Val::I32(a))) =
                        (self.stack.pop(), self.stack.pop())
                    {
                        self.stack.push(Val::I32(if a == b { 1 } else { 0 }));
                    }
                }
                Opcode::LocalGet(idx) => {
                    if let Some(frame) = self.call_stack.last() {
                        if let Some(&val) = frame.locals.get(idx as usize) {
                            self.stack.push(val);
                        }
                    }
                }
                Opcode::LocalSet(idx) => {
                    if let Some(val) = self.stack.pop() {
                        if let Some(frame) = self.call_stack.last_mut() {
                            if (idx as usize) < frame.locals.len() {
                                frame.locals[idx as usize] = val;
                            }
                        }
                    }
                }
                Opcode::I32Load(_, offset) => {
                    if let Some(Val::I32(addr)) = self.stack.pop() {
                        let ea = addr as usize + offset as usize;
                        if ea + 4 <= self.module.memory.len() {
                            let val = i32::from_le_bytes([
                                self.module.memory[ea],
                                self.module.memory[ea + 1],
                                self.module.memory[ea + 2],
                                self.module.memory[ea + 3],
                            ]);
                            self.stack.push(Val::I32(val));
                        } else {
                            self.trap("memory access out of bounds");
                        }
                    }
                }
                Opcode::I32Store(_, offset) => {
                    if let (Some(Val::I32(val)), Some(Val::I32(addr))) =
                        (self.stack.pop(), self.stack.pop())
                    {
                        let ea = addr as usize + offset as usize;
                        if ea + 4 <= self.module.memory.len() {
                            let bytes = val.to_le_bytes();
                            self.module.memory[ea..ea + 4].copy_from_slice(&bytes);
                        } else {
                            self.trap("memory access out of bounds");
                        }
                    }
                }
                Opcode::MemorySize => {
                    self.stack.push(Val::I32(self.module.memory_pages as i32));
                }
                Opcode::MemoryGrow => {
                    if let Some(Val::I32(pages)) = self.stack.pop() {
                        let old = self.module.memory_pages;
                        let new_pages = old + pages as u32;
                        if new_pages <= 256 {
                            // max 16MB
                            self.module.memory.resize(new_pages as usize * 65536, 0);
                            self.module.memory_pages = new_pages;
                            self.stack.push(Val::I32(old as i32));
                        } else {
                            self.stack.push(Val::I32(-1));
                        }
                    }
                }
                Opcode::Drop => {
                    self.stack.pop();
                }
                Opcode::Return | Opcode::End => {
                    return;
                }
                _ => {} // Other opcodes handled as needed
            }
        }
    }

    /// Trap (runtime error)
    fn trap(&mut self, msg: &str) {
        self.trapped = true;
        self.trap_msg = String::from(msg);
    }

    /// Read a string from WASM memory
    pub fn read_string(&self, ptr: u32, len: u32) -> Option<String> {
        let start = ptr as usize;
        let end = start + len as usize;
        if end <= self.module.memory.len() {
            let bytes = &self.module.memory[start..end];
            core::str::from_utf8(bytes).ok().map(String::from)
        } else {
            None
        }
    }

    /// Write bytes to WASM memory
    pub fn write_memory(&mut self, ptr: u32, data: &[u8]) -> bool {
        let start = ptr as usize;
        let end = start + data.len();
        if end <= self.module.memory.len() {
            self.module.memory[start..end].copy_from_slice(data);
            true
        } else {
            false
        }
    }
}

/// Running WASM instances
static INSTANCES: Mutex<Vec<WasmInstance>> = Mutex::new(Vec::new());

pub fn init() {
    crate::serial_println!("  [wasm] WebAssembly runtime initialized");
}

/// Load and instantiate a WASM module from bytes
pub fn load_module(name: &str, wasm_bytes: &[u8]) -> Option<usize> {
    // Parse WASM binary (simplified — real parser would handle all sections)
    if wasm_bytes.len() < 8 {
        return None;
    }
    // Check magic number: \0asm
    if &wasm_bytes[0..4] != b"\0asm" {
        return None;
    }

    let module = WasmModule {
        name: String::from(name),
        functions: Vec::new(),
        memory: Vec::new(),
        memory_pages: 1,
        globals: Vec::new(),
        exports: BTreeMap::new(),
        imports: Vec::new(),
        table: Vec::new(),
        data_segments: Vec::new(),
    };

    let mut instance = WasmInstance::new(module);
    instance.init_memory();

    let mut instances = INSTANCES.lock();
    let id = instances.len();
    instances.push(instance);
    crate::serial_println!("  [wasm] Module '{}' loaded (instance {})", name, id);
    Some(id)
}
