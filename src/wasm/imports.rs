/// Host function imports (WASI)
///
/// Part of the AIOS.

use alloc::vec::Vec;
use alloc::vec;
use crate::sync::Mutex;

/// Registry of host functions importable by WASM modules.
pub struct ImportRegistry {
    entries: Vec<ImportEntry>,
}

struct ImportEntry {
    module: &'static str,
    name: &'static str,
    func: fn(&[u64]) -> Vec<u64>,
}

/// Default host function: returns no values (void).
fn host_nop(_args: &[u64]) -> Vec<u64> {
    Vec::new()
}

/// Host function: proc_exit (WASI) - terminates the module.
fn host_proc_exit(args: &[u64]) -> Vec<u64> {
    let code = args.first().copied().unwrap_or(0);
    crate::serial_println!("[wasm/import] proc_exit({})", code);
    Vec::new()
}

/// Host function: fd_write (WASI) - write to a file descriptor.
fn host_fd_write(args: &[u64]) -> Vec<u64> {
    // fd_write(fd, iovs_ptr, iovs_len, nwritten_ptr) -> errno
    let fd = args.first().copied().unwrap_or(0);
    let _iovs_ptr = args.get(1).copied().unwrap_or(0);
    let iovs_len = args.get(2).copied().unwrap_or(0);
    crate::serial_println!("[wasm/import] fd_write(fd={}, iovs_len={})", fd, iovs_len);
    // Return errno 0 (success)
    vec![0]
}

/// Host function: fd_read (WASI) - read from a file descriptor.
fn host_fd_read(args: &[u64]) -> Vec<u64> {
    let fd = args.first().copied().unwrap_or(0);
    crate::serial_println!("[wasm/import] fd_read(fd={})", fd);
    vec![0]
}

/// Host function: clock_time_get (WASI) - get current time.
fn host_clock_time_get(_args: &[u64]) -> Vec<u64> {
    let time_ns = crate::time::clock::uptime_secs() * 1_000_000_000;
    vec![0, time_ns]
}

impl ImportRegistry {
    pub fn new() -> Self {
        ImportRegistry {
            entries: Vec::new(),
        }
    }

    /// Register a host function that WASM modules can import.
    pub fn register(&mut self, module: &'static str, name: &'static str, func: fn(&[u64]) -> Vec<u64>) {
        // Replace existing entry if same module+name
        for entry in self.entries.iter_mut() {
            if entry.module == module && entry.name == name {
                entry.func = func;
                return;
            }
        }
        self.entries.push(ImportEntry { module, name, func });
    }

    /// Resolve an import to its host function.
    pub fn resolve(&self, module: &str, name: &str) -> Option<fn(&[u64]) -> Vec<u64>> {
        self.entries.iter()
            .find(|e| e.module == module && e.name == name)
            .map(|e| e.func)
    }

    /// Number of registered imports.
    pub fn count(&self) -> usize {
        self.entries.len()
    }

    /// Populate standard WASI imports.
    fn populate_wasi_defaults(&mut self) {
        self.register("wasi_snapshot_preview1", "proc_exit", host_proc_exit);
        self.register("wasi_snapshot_preview1", "fd_write", host_fd_write);
        self.register("wasi_snapshot_preview1", "fd_read", host_fd_read);
        self.register("wasi_snapshot_preview1", "clock_time_get", host_clock_time_get);
        self.register("wasi_snapshot_preview1", "args_get", host_nop);
        self.register("wasi_snapshot_preview1", "args_sizes_get", host_nop);
        self.register("wasi_snapshot_preview1", "environ_get", host_nop);
        self.register("wasi_snapshot_preview1", "environ_sizes_get", host_nop);
    }
}

static IMPORT_REGISTRY: Mutex<Option<ImportRegistry>> = Mutex::new(None);

pub fn init() {
    let mut reg = ImportRegistry::new();
    reg.populate_wasi_defaults();
    let count = reg.count();
    *IMPORT_REGISTRY.lock() = Some(reg);
    crate::serial_println!("[wasm] import registry ready ({} host functions)", count);
}

/// Resolve a host import globally.
pub fn resolve(module: &str, name: &str) -> Option<fn(&[u64]) -> Vec<u64>> {
    IMPORT_REGISTRY.lock().as_ref().and_then(|r| r.resolve(module, name))
}
