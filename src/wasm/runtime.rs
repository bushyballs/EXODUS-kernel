/// WASM execution engine.
///
/// Part of the AIOS wasm subsystem.

use alloc::vec::Vec;
use alloc::string::String;
use crate::sync::Mutex;

/// A loaded module instance.
struct ModuleInstance {
    handle: u64,
    module: super::parser::WasmModule,
    interpreter: super::interpreter::WasmInterpreter,
    memory: super::memory::LinearMemory,
}

/// Core WASM runtime instance.
pub struct WasmRuntime {
    instances: Vec<ModuleInstance>,
    next_handle: u64,
}

impl WasmRuntime {
    pub fn new() -> Self {
        WasmRuntime {
            instances: Vec::new(),
            next_handle: 1,
        }
    }

    /// Instantiate a WASM module and return an instance handle.
    pub fn instantiate(&mut self, bytecode: &[u8]) -> u64 {
        match super::parser::WasmModule::parse(bytecode) {
            Ok(module) => {
                let handle = self.next_handle;
                self.next_handle = self.next_handle.saturating_add(1);

                let interpreter = super::interpreter::WasmInterpreter::new();
                // Default: 1 page of linear memory, max 256 pages
                let memory = super::memory::LinearMemory::new(1, Some(256));

                self.instances.push(ModuleInstance {
                    handle,
                    module,
                    interpreter,
                    memory,
                });

                crate::serial_println!(
                    "[wasm/runtime] instantiated module handle={} ({} functions)",
                    handle,
                    self.instances.last().map(|i| i.module.function_count()).unwrap_or(0)
                );

                handle
            }
            Err(e) => {
                crate::serial_println!("[wasm/runtime] failed to parse module: {}", e);
                0 // 0 = invalid handle
            }
        }
    }

    /// Invoke an exported function by name.
    ///
    /// This is a simplified dispatch: in a full implementation, the runtime
    /// would resolve the export table, set up the call frame with arguments,
    /// and execute the function body from the code section.
    pub fn call(&mut self, instance: u64, func_name: &str, args: &[u64]) -> Vec<u64> {
        if let Some(inst) = self.instances.iter_mut().find(|i| i.handle == instance) {
            // Look up the code section for the function body
            if let Some(code_section) = inst.module.get_section(super::parser::SECTION_CODE) {
                // Set up arguments as locals
                inst.interpreter.locals = args.to_vec();
                inst.interpreter.stack.clear();
                inst.interpreter.halted = false;
                inst.interpreter.pc = 0;

                // Execute the code section data as bytecode (simplified)
                let result = inst.interpreter.execute(&code_section.data);

                crate::serial_println!(
                    "[wasm/runtime] call {}() on handle={} -> {} results",
                    func_name, instance, result.len()
                );

                result
            } else {
                crate::serial_println!("[wasm/runtime] no code section in module handle={}", instance);
                Vec::new()
            }
        } else {
            crate::serial_println!("[wasm/runtime] invalid instance handle={}", instance);
            Vec::new()
        }
    }

    /// Number of loaded instances.
    pub fn instance_count(&self) -> usize {
        self.instances.len()
    }

    /// Remove an instance.
    pub fn destroy(&mut self, handle: u64) {
        self.instances.retain(|i| i.handle != handle);
    }
}

static RUNTIME: Mutex<Option<WasmRuntime>> = Mutex::new(None);

pub fn init() {
    *RUNTIME.lock() = Some(WasmRuntime::new());
    crate::serial_println!("[wasm] runtime engine ready");
}

/// Instantiate a module from bytecode globally.
pub fn instantiate(bytecode: &[u8]) -> u64 {
    match RUNTIME.lock().as_mut() {
        Some(rt) => rt.instantiate(bytecode),
        None => 0,
    }
}

/// Call an exported function globally.
pub fn call(instance: u64, func_name: &str, args: &[u64]) -> Vec<u64> {
    match RUNTIME.lock().as_mut() {
        Some(rt) => rt.call(instance, func_name, args),
        None => Vec::new(),
    }
}
