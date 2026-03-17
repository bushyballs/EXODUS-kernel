/// WebAssembly runtime
///
/// Part of the AIOS.

pub mod parser;
pub mod validator;
pub mod interpreter;
pub mod compiler;
pub mod jit;
pub mod memory;
pub mod table;
pub mod imports;
pub mod runtime;
pub mod wasi;
pub mod sandbox;
pub mod gc;

pub fn init() {
    parser::init();
    validator::init();
    interpreter::init();
    compiler::init();
    jit::init();
    memory::init();
    table::init();
    imports::init();
    runtime::init();
    wasi::init();
    sandbox::init();
    gc::init();
    crate::serial_println!("[wasm] WebAssembly runtime fully initialized");
}
