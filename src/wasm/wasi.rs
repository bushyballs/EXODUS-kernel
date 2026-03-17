/// WASI system interface implementation
///
/// Part of the AIOS.

use alloc::vec::Vec;
use alloc::vec;
use crate::sync::Mutex;

// WASI syscall numbers (from wasi_snapshot_preview1)
const WASI_ARGS_GET: u32 = 0;
const WASI_ARGS_SIZES_GET: u32 = 1;
const WASI_ENVIRON_GET: u32 = 2;
const WASI_ENVIRON_SIZES_GET: u32 = 3;
const WASI_CLOCK_TIME_GET: u32 = 4;
const WASI_FD_CLOSE: u32 = 6;
const WASI_FD_READ: u32 = 7;
const WASI_FD_WRITE: u32 = 8;
const WASI_FD_SEEK: u32 = 10;
const WASI_FD_PRESTAT_GET: u32 = 23;
const WASI_FD_PRESTAT_DIR_NAME: u32 = 24;
const WASI_PATH_OPEN: u32 = 26;
const WASI_PROC_EXIT: u32 = 60;

// WASI error codes
const WASI_ESUCCESS: i32 = 0;
const WASI_EBADF: i32 = 8;
const WASI_EINVAL: i32 = 28;
const WASI_ENOSYS: i32 = 52;

/// WASI context providing OS-level access to WASM modules.
pub struct WasiCtx {
    preopened_dirs: Vec<u32>,
    args: Vec<Vec<u8>>,
    env: Vec<Vec<u8>>,
    exit_code: Option<i32>,
}

impl WasiCtx {
    pub fn new() -> Self {
        WasiCtx {
            preopened_dirs: Vec::new(),
            args: Vec::new(),
            env: Vec::new(),
            exit_code: None,
        }
    }

    /// Handle a WASI syscall by number.
    pub fn handle_syscall(&mut self, nr: u32, args: &[u64]) -> i32 {
        match nr {
            WASI_ARGS_GET => {
                // args_get(argv_ptr, argv_buf_ptr) -> errno
                crate::serial_println!("[wasi] args_get: {} args", self.args.len());
                WASI_ESUCCESS
            }
            WASI_ARGS_SIZES_GET => {
                // args_sizes_get(argc_ptr, argv_buf_size_ptr) -> errno
                crate::serial_println!("[wasi] args_sizes_get: {} args", self.args.len());
                WASI_ESUCCESS
            }
            WASI_ENVIRON_GET => {
                crate::serial_println!("[wasi] environ_get: {} vars", self.env.len());
                WASI_ESUCCESS
            }
            WASI_ENVIRON_SIZES_GET => {
                crate::serial_println!("[wasi] environ_sizes_get: {} vars", self.env.len());
                WASI_ESUCCESS
            }
            WASI_CLOCK_TIME_GET => {
                // clock_time_get(clock_id, precision, time_ptr) -> errno
                let _clock_id = args.first().copied().unwrap_or(0);
                crate::serial_println!("[wasi] clock_time_get(clock={})", _clock_id);
                WASI_ESUCCESS
            }
            WASI_FD_CLOSE => {
                let fd = args.first().copied().unwrap_or(0) as u32;
                crate::serial_println!("[wasi] fd_close({})", fd);
                WASI_ESUCCESS
            }
            WASI_FD_READ => {
                let fd = args.first().copied().unwrap_or(0) as u32;
                crate::serial_println!("[wasi] fd_read(fd={})", fd);
                WASI_ESUCCESS
            }
            WASI_FD_WRITE => {
                let fd = args.first().copied().unwrap_or(0) as u32;
                crate::serial_println!("[wasi] fd_write(fd={})", fd);
                WASI_ESUCCESS
            }
            WASI_FD_SEEK => {
                let fd = args.first().copied().unwrap_or(0) as u32;
                crate::serial_println!("[wasi] fd_seek(fd={})", fd);
                WASI_ESUCCESS
            }
            WASI_FD_PRESTAT_GET => {
                let fd = args.first().copied().unwrap_or(0) as u32;
                if self.preopened_dirs.contains(&fd) {
                    WASI_ESUCCESS
                } else {
                    WASI_EBADF
                }
            }
            WASI_FD_PRESTAT_DIR_NAME => {
                let fd = args.first().copied().unwrap_or(0) as u32;
                if self.preopened_dirs.contains(&fd) {
                    WASI_ESUCCESS
                } else {
                    WASI_EBADF
                }
            }
            WASI_PATH_OPEN => {
                crate::serial_println!("[wasi] path_open");
                WASI_ESUCCESS
            }
            WASI_PROC_EXIT => {
                let code = args.first().copied().unwrap_or(0) as i32;
                crate::serial_println!("[wasi] proc_exit({})", code);
                self.exit_code = Some(code);
                WASI_ESUCCESS
            }
            _ => {
                crate::serial_println!("[wasi] unhandled syscall nr={}", nr);
                WASI_ENOSYS
            }
        }
    }

    /// Add a preopened directory to the context.
    pub fn preopen_dir(&mut self, fd: u32) {
        if !self.preopened_dirs.contains(&fd) {
            self.preopened_dirs.push(fd);
        }
    }

    /// Add a command-line argument.
    pub fn push_arg(&mut self, arg: &[u8]) {
        self.args.push(arg.to_vec());
    }

    /// Add an environment variable (KEY=VALUE).
    pub fn push_env(&mut self, var: &[u8]) {
        self.env.push(var.to_vec());
    }

    /// Check if the module has called proc_exit.
    pub fn has_exited(&self) -> bool {
        self.exit_code.is_some()
    }

    /// Get exit code if proc_exit was called.
    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }
}

static WASI_CTX: Mutex<Option<WasiCtx>> = Mutex::new(None);

pub fn init() {
    let mut ctx = WasiCtx::new();
    // Preopen fd 3 as the root directory by default
    ctx.preopen_dir(3);
    *WASI_CTX.lock() = Some(ctx);
    crate::serial_println!("[wasm] WASI context ready");
}

/// Handle a WASI syscall from a running module.
pub fn handle_syscall(nr: u32, args: &[u64]) -> i32 {
    match WASI_CTX.lock().as_mut() {
        Some(ctx) => ctx.handle_syscall(nr, args),
        None => WASI_ENOSYS,
    }
}
