/// System API bindings for Genesis app developers
///
/// Provides a typed, versioned registry of system APIs that app code
/// can call. Each API endpoint belongs to a category, declares its
/// argument and return types, specifies the required permission, and
/// carries a stability flag. The registry ships with 50+ built-in
/// system APIs covering filesystem, network, display, audio, input,
/// sensors, crypto, AI, database, and IPC.
///
/// Original implementation for Hoags OS. No external crates.
use crate::sync::Mutex;
use alloc::vec;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

use super::app_sdk::Permission;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of registered API endpoints
const MAX_API_ENDPOINTS: usize = 512;

/// Argument type: none / void
const ARG_VOID: u8 = 0x00;
/// Argument type: 32-bit integer
const ARG_I32: u8 = 0x01;
/// Argument type: 64-bit integer
const ARG_I64: u8 = 0x02;
/// Argument type: byte buffer pointer + length
const ARG_BUFFER: u8 = 0x03;
/// Argument type: boolean
const ARG_BOOL: u8 = 0x04;
/// Argument type: hash (u64)
const ARG_HASH: u8 = 0x05;
/// Argument type: Q16 fixed-point
const ARG_Q16: u8 = 0x06;
/// Argument type: string hash
const ARG_STR_HASH: u8 = 0x07;

/// Return type: void (no return)
const RET_VOID: u8 = 0x00;
/// Return type: i32 status code
const RET_I32: u8 = 0x01;
/// Return type: i64 value
const RET_I64: u8 = 0x02;
/// Return type: bool success flag
const RET_BOOL: u8 = 0x03;
/// Return type: buffer (length returned as i32)
const RET_BUFFER: u8 = 0x04;
/// Return type: Q16 fixed-point
const RET_Q16: u8 = 0x06;

/// Status: API call succeeded
const STATUS_OK: i32 = 0;
/// Status: API not found
const STATUS_NOT_FOUND: i32 = -1;
/// Status: permission denied
const STATUS_PERMISSION_DENIED: i32 = -2;
/// Status: invalid arguments
const STATUS_INVALID_ARGS: i32 = -3;
/// Status: API deprecated
const STATUS_DEPRECATED: i32 = -4;
/// Status: internal error
const STATUS_INTERNAL_ERROR: i32 = -5;

// ---------------------------------------------------------------------------
// ApiCategory
// ---------------------------------------------------------------------------

/// Categories of system APIs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiCategory {
    /// File and directory operations
    Filesystem,
    /// Network and socket operations
    Network,
    /// Display and rendering operations
    Display,
    /// Audio playback and recording
    Audio,
    /// Keyboard, mouse, touch, stylus input
    Input,
    /// Hardware sensors (accelerometer, gyro, etc.)
    Sensors,
    /// Cryptographic operations
    Crypto,
    /// AI and machine learning inference
    AI,
    /// Local database operations
    Database,
    /// Inter-process communication
    IPC,
}

impl ApiCategory {
    /// Numeric id for the category
    pub fn id(&self) -> u8 {
        match self {
            ApiCategory::Filesystem => 0,
            ApiCategory::Network => 1,
            ApiCategory::Display => 2,
            ApiCategory::Audio => 3,
            ApiCategory::Input => 4,
            ApiCategory::Sensors => 5,
            ApiCategory::Crypto => 6,
            ApiCategory::AI => 7,
            ApiCategory::Database => 8,
            ApiCategory::IPC => 9,
        }
    }

    /// Reconstruct from numeric id
    pub fn from_id(id: u8) -> Option<ApiCategory> {
        match id {
            0 => Some(ApiCategory::Filesystem),
            1 => Some(ApiCategory::Network),
            2 => Some(ApiCategory::Display),
            3 => Some(ApiCategory::Audio),
            4 => Some(ApiCategory::Input),
            5 => Some(ApiCategory::Sensors),
            6 => Some(ApiCategory::Crypto),
            7 => Some(ApiCategory::AI),
            8 => Some(ApiCategory::Database),
            9 => Some(ApiCategory::IPC),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// ApiEndpoint
// ---------------------------------------------------------------------------

/// A single system API endpoint
#[derive(Debug, Clone)]
pub struct ApiEndpoint {
    /// Unique endpoint identifier
    pub id: u32,
    /// Which category this API belongs to
    pub category: ApiCategory,
    /// Hash of the API name (e.g. hash of "fs_open")
    pub name_hash: u64,
    /// Argument types expected by this API
    pub arg_types: Vec<u8>,
    /// Return type
    pub return_type: u8,
    /// Permission required to call this API
    pub permission_required: Permission,
    /// Whether this API is considered stable (vs experimental)
    pub stable: bool,
    /// Whether this API has been deprecated
    pub deprecated: bool,
    /// Version when this API was introduced
    pub since_version: u32,
}

// ---------------------------------------------------------------------------
// ApiCallResult
// ---------------------------------------------------------------------------

/// Result of calling a system API
#[derive(Debug, Clone)]
pub struct ApiCallResult {
    /// Status code (0 = success, negative = error)
    pub status: i32,
    /// Return value (interpretation depends on return_type)
    pub value: i64,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static API_REGISTRY: Mutex<Option<ApiRegistryState>> = Mutex::new(None);

struct ApiRegistryState {
    endpoints: Vec<ApiEndpoint>,
    next_id: u32,
    call_count: u64,
    initialized: bool,
}

impl ApiRegistryState {
    fn new() -> Self {
        Self {
            endpoints: Vec::new(),
            next_id: 1,
            call_count: 0,
            initialized: true,
        }
    }
}

// ---------------------------------------------------------------------------
// ApiBindings — public API
// ---------------------------------------------------------------------------

/// System API bindings interface
pub struct ApiBindings;

impl ApiBindings {
    /// Register a new API endpoint in the registry
    ///
    /// Returns the assigned endpoint id, or 0 on failure.
    pub fn register_api(
        category: ApiCategory,
        name_hash: u64,
        arg_types: Vec<u8>,
        return_type: u8,
        permission_required: Permission,
        stable: bool,
    ) -> u32 {
        let mut guard = API_REGISTRY.lock();
        let state = match guard.as_mut() {
            Some(s) => s,
            None => return 0,
        };

        if state.endpoints.len() >= MAX_API_ENDPOINTS {
            serial_println!("[api] registry full, cannot register");
            return 0;
        }

        // Check for duplicate name hash within the same category
        for ep in &state.endpoints {
            if ep.category == category && ep.name_hash == name_hash {
                serial_println!("[api] duplicate name_hash in category {:?}", category);
                return 0;
            }
        }

        let id = state.next_id;
        state.next_id = state.next_id.saturating_add(1);

        let endpoint = ApiEndpoint {
            id,
            category,
            name_hash,
            arg_types,
            return_type,
            permission_required,
            stable,
            deprecated: false,
            since_version: 0x00010000,
        };

        state.endpoints.push(endpoint);
        id
    }

    /// Call a system API by its id
    ///
    /// Validates arguments, checks permissions (via app_id),
    /// and returns a result with status code and value.
    pub fn call_api(endpoint_id: u32, app_id: u32, args: &[u8]) -> ApiCallResult {
        let mut guard = API_REGISTRY.lock();
        let state = match guard.as_mut() {
            Some(s) => s,
            None => {
                return ApiCallResult {
                    status: STATUS_INTERNAL_ERROR,
                    value: 0,
                };
            }
        };

        state.call_count = state.call_count.saturating_add(1);

        let endpoint = match state.endpoints.iter().find(|e| e.id == endpoint_id) {
            Some(e) => e,
            None => {
                return ApiCallResult {
                    status: STATUS_NOT_FOUND,
                    value: 0,
                };
            }
        };

        if endpoint.deprecated {
            serial_println!("[api] warning: calling deprecated API id={}", endpoint_id);
            return ApiCallResult {
                status: STATUS_DEPRECATED,
                value: 0,
            };
        }

        // Verify the app has the required permission
        if !super::app_sdk::SdkApi::check_permission(app_id, endpoint.permission_required) {
            serial_println!(
                "[api] permission denied: app {} needs {:?} for API {}",
                app_id,
                endpoint.permission_required,
                endpoint_id
            );
            return ApiCallResult {
                status: STATUS_PERMISSION_DENIED,
                value: 0,
            };
        }

        // Validate argument count matches expected types
        if !Self::validate_args_internal(&endpoint.arg_types, args) {
            return ApiCallResult {
                status: STATUS_INVALID_ARGS,
                value: 0,
            };
        }

        // Dispatch — in a real kernel, this would jump into the actual implementation.
        // Here we return a success stub.
        serial_println!(
            "[api] call id={} category={:?} by app={}",
            endpoint_id,
            endpoint.category,
            app_id
        );

        ApiCallResult {
            status: STATUS_OK,
            value: 0,
        }
    }

    /// List all API endpoints, optionally filtered by category
    pub fn list_apis(category_filter: Option<ApiCategory>) -> Vec<ApiEndpoint> {
        let guard = API_REGISTRY.lock();
        let state = match guard.as_ref() {
            Some(s) => s,
            None => return Vec::new(),
        };

        match category_filter {
            Some(cat) => state
                .endpoints
                .iter()
                .filter(|e| e.category == cat)
                .cloned()
                .collect(),
            None => state.endpoints.clone(),
        }
    }

    /// Get a documentation hash for a given API endpoint
    ///
    /// Returns a synthetic hash derived from the endpoint id and name_hash.
    pub fn get_api_docs_hash(endpoint_id: u32) -> u64 {
        let guard = API_REGISTRY.lock();
        let state = match guard.as_ref() {
            Some(s) => s,
            None => return 0,
        };

        match state.endpoints.iter().find(|e| e.id == endpoint_id) {
            Some(ep) => {
                // Combine id and name_hash into a docs hash
                ep.name_hash ^ ((endpoint_id as u64) << 32) ^ 0xD0C5D0C5D0C5D0C5
            }
            None => 0,
        }
    }

    /// Validate that a set of argument bytes matches the expected types
    pub fn validate_args(endpoint_id: u32, args: &[u8]) -> bool {
        let guard = API_REGISTRY.lock();
        let state = match guard.as_ref() {
            Some(s) => s,
            None => return false,
        };

        match state.endpoints.iter().find(|e| e.id == endpoint_id) {
            Some(ep) => Self::validate_args_internal(&ep.arg_types, args),
            None => false,
        }
    }

    /// Internal argument validation
    fn validate_args_internal(arg_types: &[u8], args: &[u8]) -> bool {
        // Calculate expected minimum byte count from arg types
        let mut expected_bytes: usize = 0;
        for atype in arg_types {
            match *atype {
                ARG_VOID => {}
                ARG_I32 | ARG_Q16 => expected_bytes += 4,
                ARG_I64 | ARG_HASH | ARG_STR_HASH => expected_bytes += 8,
                ARG_BUFFER => expected_bytes += 8, // ptr(4) + len(4)
                ARG_BOOL => expected_bytes += 1,
                _ => return false, // unknown arg type
            }
        }

        // Void args with no data is fine
        if arg_types.len() == 1 && arg_types[0] == ARG_VOID && args.is_empty() {
            return true;
        }

        args.len() >= expected_bytes
    }

    /// Mark an API endpoint as deprecated
    pub fn deprecate_api(endpoint_id: u32) -> bool {
        let mut guard = API_REGISTRY.lock();
        let state = match guard.as_mut() {
            Some(s) => s,
            None => return false,
        };

        match state.endpoints.iter_mut().find(|e| e.id == endpoint_id) {
            Some(ep) => {
                ep.deprecated = true;
                ep.stable = false;
                serial_println!("[api] deprecated endpoint id={}", endpoint_id);
                true
            }
            None => false,
        }
    }

    /// Get the total number of registered APIs
    pub fn api_count() -> usize {
        let guard = API_REGISTRY.lock();
        match guard.as_ref() {
            Some(s) => s.endpoints.len(),
            None => 0,
        }
    }

    /// Get the total number of API calls made since boot
    pub fn total_calls() -> u64 {
        let guard = API_REGISTRY.lock();
        match guard.as_ref() {
            Some(s) => s.call_count,
            None => 0,
        }
    }

    /// Find an API endpoint by name hash
    pub fn find_by_name(name_hash: u64) -> Option<ApiEndpoint> {
        let guard = API_REGISTRY.lock();
        let state = match guard.as_ref() {
            Some(s) => s,
            None => return None,
        };
        state
            .endpoints
            .iter()
            .find(|e| e.name_hash == name_hash)
            .cloned()
    }

    /// List only stable, non-deprecated APIs
    pub fn list_stable_apis() -> Vec<ApiEndpoint> {
        let guard = API_REGISTRY.lock();
        let state = match guard.as_ref() {
            Some(s) => s,
            None => return Vec::new(),
        };
        state
            .endpoints
            .iter()
            .filter(|e| e.stable && !e.deprecated)
            .cloned()
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Syscall registry
// ---------------------------------------------------------------------------

/// Metadata about one kernel syscall, for introspection by dev tools
pub struct SyscallInfo {
    /// Syscall number (matches the interrupt call table)
    pub number: u32,
    /// ASCII name, e.g. "sys_read"
    pub name: &'static str,
    /// Number of arguments the syscall accepts
    pub arg_count: u8,
}

/// The static syscall table — all Genesis kernel syscalls.
/// Numbers follow the Genesis ABI (x86-64 in rax, args in rdi/rsi/rdx/rcx/r8/r9).
static SYSCALL_TABLE: &[SyscallInfo] = &[
    SyscallInfo {
        number: 0,
        name: "sys_read",
        arg_count: 3,
    },
    SyscallInfo {
        number: 1,
        name: "sys_write",
        arg_count: 3,
    },
    SyscallInfo {
        number: 2,
        name: "sys_open",
        arg_count: 3,
    },
    SyscallInfo {
        number: 3,
        name: "sys_close",
        arg_count: 1,
    },
    SyscallInfo {
        number: 4,
        name: "sys_stat",
        arg_count: 2,
    },
    SyscallInfo {
        number: 5,
        name: "sys_fstat",
        arg_count: 2,
    },
    SyscallInfo {
        number: 6,
        name: "sys_lstat",
        arg_count: 2,
    },
    SyscallInfo {
        number: 7,
        name: "sys_seek",
        arg_count: 3,
    },
    SyscallInfo {
        number: 8,
        name: "sys_mmap",
        arg_count: 6,
    },
    SyscallInfo {
        number: 9,
        name: "sys_munmap",
        arg_count: 2,
    },
    SyscallInfo {
        number: 10,
        name: "sys_mprotect",
        arg_count: 3,
    },
    SyscallInfo {
        number: 11,
        name: "sys_brk",
        arg_count: 1,
    },
    SyscallInfo {
        number: 12,
        name: "sys_exit",
        arg_count: 1,
    },
    SyscallInfo {
        number: 13,
        name: "sys_fork",
        arg_count: 0,
    },
    SyscallInfo {
        number: 14,
        name: "sys_exec",
        arg_count: 3,
    },
    SyscallInfo {
        number: 15,
        name: "sys_wait",
        arg_count: 3,
    },
    SyscallInfo {
        number: 16,
        name: "sys_getpid",
        arg_count: 0,
    },
    SyscallInfo {
        number: 17,
        name: "sys_getuid",
        arg_count: 0,
    },
    SyscallInfo {
        number: 18,
        name: "sys_kill",
        arg_count: 2,
    },
    SyscallInfo {
        number: 19,
        name: "sys_signal",
        arg_count: 2,
    },
    SyscallInfo {
        number: 20,
        name: "sys_socket",
        arg_count: 3,
    },
    SyscallInfo {
        number: 21,
        name: "sys_bind",
        arg_count: 3,
    },
    SyscallInfo {
        number: 22,
        name: "sys_connect",
        arg_count: 3,
    },
    SyscallInfo {
        number: 23,
        name: "sys_accept",
        arg_count: 3,
    },
    SyscallInfo {
        number: 24,
        name: "sys_send",
        arg_count: 4,
    },
    SyscallInfo {
        number: 25,
        name: "sys_recv",
        arg_count: 4,
    },
    SyscallInfo {
        number: 26,
        name: "sys_gettimeofday",
        arg_count: 2,
    },
    SyscallInfo {
        number: 27,
        name: "sys_settimeofday",
        arg_count: 2,
    },
    SyscallInfo {
        number: 28,
        name: "sys_ioctl",
        arg_count: 3,
    },
    SyscallInfo {
        number: 29,
        name: "sys_fcntl",
        arg_count: 3,
    },
    SyscallInfo {
        number: 30,
        name: "sys_pipe",
        arg_count: 1,
    },
    SyscallInfo {
        number: 31,
        name: "sys_dup",
        arg_count: 1,
    },
    SyscallInfo {
        number: 32,
        name: "sys_dup2",
        arg_count: 2,
    },
    SyscallInfo {
        number: 33,
        name: "sys_mkdir",
        arg_count: 2,
    },
    SyscallInfo {
        number: 34,
        name: "sys_rmdir",
        arg_count: 1,
    },
    SyscallInfo {
        number: 35,
        name: "sys_unlink",
        arg_count: 1,
    },
    SyscallInfo {
        number: 36,
        name: "sys_rename",
        arg_count: 2,
    },
    SyscallInfo {
        number: 37,
        name: "sys_chmod",
        arg_count: 2,
    },
    SyscallInfo {
        number: 38,
        name: "sys_chown",
        arg_count: 3,
    },
    SyscallInfo {
        number: 39,
        name: "sys_mount",
        arg_count: 5,
    },
    SyscallInfo {
        number: 40,
        name: "sys_umount",
        arg_count: 2,
    },
    SyscallInfo {
        number: 41,
        name: "sys_sync",
        arg_count: 0,
    },
    SyscallInfo {
        number: 42,
        name: "sys_reboot",
        arg_count: 1,
    },
    SyscallInfo {
        number: 43,
        name: "sys_shmget",
        arg_count: 3,
    },
    SyscallInfo {
        number: 44,
        name: "sys_shmat",
        arg_count: 3,
    },
    SyscallInfo {
        number: 45,
        name: "sys_shmdt",
        arg_count: 1,
    },
    SyscallInfo {
        number: 46,
        name: "sys_semget",
        arg_count: 3,
    },
    SyscallInfo {
        number: 47,
        name: "sys_semop",
        arg_count: 3,
    },
    SyscallInfo {
        number: 48,
        name: "sys_msgget",
        arg_count: 2,
    },
    SyscallInfo {
        number: 49,
        name: "sys_msgsnd",
        arg_count: 4,
    },
    SyscallInfo {
        number: 50,
        name: "sys_msgrcv",
        arg_count: 5,
    },
    SyscallInfo {
        number: 51,
        name: "sys_ai_infer",
        arg_count: 4,
    },
    SyscallInfo {
        number: 52,
        name: "sys_ai_embed",
        arg_count: 3,
    },
    SyscallInfo {
        number: 53,
        name: "sys_framebuf",
        arg_count: 4,
    },
    SyscallInfo {
        number: 54,
        name: "sys_audio_write",
        arg_count: 3,
    },
    SyscallInfo {
        number: 55,
        name: "sys_input_read",
        arg_count: 2,
    },
    SyscallInfo {
        number: 56,
        name: "sys_kv_get",
        arg_count: 3,
    },
    SyscallInfo {
        number: 57,
        name: "sys_kv_set",
        arg_count: 4,
    },
    SyscallInfo {
        number: 58,
        name: "sys_ipc_send",
        arg_count: 3,
    },
    SyscallInfo {
        number: 59,
        name: "sys_ipc_recv",
        arg_count: 3,
    },
    SyscallInfo {
        number: 60,
        name: "sys_log",
        arg_count: 2,
    },
    SyscallInfo {
        number: 61,
        name: "sys_perf_counter",
        arg_count: 1,
    },
    SyscallInfo {
        number: 62,
        name: "sys_yield",
        arg_count: 0,
    },
    SyscallInfo {
        number: 63,
        name: "sys_sleep_ms",
        arg_count: 1,
    },
];

/// Return a reference to the complete static syscall table.
pub fn list_syscalls() -> &'static [SyscallInfo] {
    SYSCALL_TABLE
}

/// Return the kernel version string.
pub fn get_kernel_version() -> &'static str {
    "Genesis 0.1.0"
}

/// Query whether a hardware or kernel capability is present.
///
/// Recognised capability strings (case-sensitive):
/// - `"smp"`         — symmetric multi-processing (multiple CPU cores)
/// - `"iommu"`       — I/O memory management unit
/// - `"kvm"`         — hardware virtualisation (KVM / VMX / SVM)
/// - `"secure_boot"` — UEFI Secure Boot chain validation active
/// - `"avx2"`        — AVX-2 SIMD instruction set
/// - `"avx512"`      — AVX-512 SIMD instruction set
/// - `"rdrand"`      — hardware random number generator (RDRAND)
/// - `"tpm"`         — Trusted Platform Module
/// - `"uefi"`        — booted via UEFI (vs legacy BIOS)
/// - `"acpi"`        — ACPI power management tables present
///
/// Unknown capability strings return `false`.
pub fn query_capability(cap: &str) -> bool {
    // In a real kernel these would be determined at boot by CPUID / ACPI probing.
    // For the Genesis 0.1.0 baseline we expose a fixed capability set.
    match cap {
        "uefi" => true,
        "acpi" => true,
        "rdrand" => true,
        "smp" => false, // single-core baseline
        "iommu" => false,
        "kvm" => false,
        "secure_boot" => false,
        "avx2" => false,
        "avx512" => false,
        "tpm" => false,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Dynamic driver registry
// ---------------------------------------------------------------------------

/// Maximum number of drivers that can be registered
const MAX_DRIVERS: usize = 64;

/// A registered driver entry
struct DriverEntry {
    /// Hash of the driver name (FNV-1a 64-bit)
    name_hash: u64,
    /// Driver initialisation function (returns true on success)
    init_fn: fn() -> bool,
    /// Assigned driver id (1-based)
    id: u32,
    /// Whether init_fn has been called successfully
    initialized: bool,
}

static DRIVER_REGISTRY: Mutex<Option<DriverRegistry>> = Mutex::new(None);

struct DriverRegistry {
    drivers: Vec<DriverEntry>,
    next_id: u32,
}

impl DriverRegistry {
    fn new() -> Self {
        DriverRegistry {
            drivers: Vec::new(),
            next_id: 1,
        }
    }
}

/// Compute FNV-1a 64-bit hash of a byte string (no_std compatible)
fn fnv1a_64(s: &str) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x00000100000001b3;
    let mut hash = FNV_OFFSET;
    for &byte in s.as_bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Register a driver by name and initialisation function.
///
/// The driver `init_fn` is called immediately. If it returns `true` the
/// driver is marked as initialized. Returns the assigned driver id (≥ 1)
/// on success, or `0` if the registry is full or the name is already taken.
pub fn register_driver(name: &str, init_fn: fn() -> bool) -> u32 {
    let name_hash = fnv1a_64(name);

    let mut guard = DRIVER_REGISTRY.lock();
    let reg = match guard.as_mut() {
        Some(r) => r,
        None => return 0,
    };

    if reg.drivers.len() >= MAX_DRIVERS {
        serial_println!("[api] driver registry full, cannot register '{}'", name);
        return 0;
    }

    // Check for duplicate name
    for d in &reg.drivers {
        if d.name_hash == name_hash {
            serial_println!("[api] driver '{}' already registered (id={})", name, d.id);
            return 0;
        }
    }

    let id = reg.next_id;
    reg.next_id = reg.next_id.saturating_add(1);

    // Call the init function before recording
    let initialized = init_fn();
    if initialized {
        serial_println!(
            "[api] driver '{}' registered and initialized (id={})",
            name,
            id
        );
    } else {
        serial_println!(
            "[api] driver '{}' registered but init FAILED (id={})",
            name,
            id
        );
    }

    reg.drivers.push(DriverEntry {
        name_hash,
        init_fn,
        id,
        initialized,
    });
    id
}

/// Look up a registered driver by name.
///
/// Returns `Some(driver_id)` if a driver with that name was previously
/// registered, `None` otherwise.
pub fn driver_lookup(name: &str) -> Option<u32> {
    let name_hash = fnv1a_64(name);
    let guard = DRIVER_REGISTRY.lock();
    let reg = guard.as_ref()?;
    reg.drivers
        .iter()
        .find(|d| d.name_hash == name_hash)
        .map(|d| d.id)
}

// ---------------------------------------------------------------------------
// Built-in system API registration
// ---------------------------------------------------------------------------

/// Register all built-in system APIs (50+ endpoints)
fn register_builtin_apis() {
    // === Filesystem APIs (8) ===
    ApiBindings::register_api(
        ApiCategory::Filesystem,
        0xF500000000000001,
        vec![ARG_HASH, ARG_I32],
        RET_I32,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Filesystem,
        0xF500000000000002,
        vec![ARG_I32],
        RET_BOOL,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Filesystem,
        0xF500000000000003,
        vec![ARG_I32, ARG_BUFFER],
        RET_I32,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Filesystem,
        0xF500000000000004,
        vec![ARG_I32, ARG_BUFFER],
        RET_I32,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Filesystem,
        0xF500000000000005,
        vec![ARG_HASH],
        RET_BOOL,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Filesystem,
        0xF500000000000006,
        vec![ARG_HASH],
        RET_I64,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Filesystem,
        0xF500000000000007,
        vec![ARG_HASH, ARG_HASH],
        RET_BOOL,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Filesystem,
        0xF500000000000008,
        vec![ARG_HASH],
        RET_BOOL,
        Permission::Storage,
        true,
    );

    // === Network APIs (7) ===
    ApiBindings::register_api(
        ApiCategory::Network,
        0xAE00000000000001,
        vec![ARG_I32, ARG_I32],
        RET_I32,
        Permission::Internet,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Network,
        0xAE00000000000002,
        vec![ARG_I32, ARG_BUFFER],
        RET_BOOL,
        Permission::Internet,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Network,
        0xAE00000000000003,
        vec![ARG_I32, ARG_BUFFER],
        RET_I32,
        Permission::Internet,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Network,
        0xAE00000000000004,
        vec![ARG_I32, ARG_BUFFER, ARG_I32],
        RET_I32,
        Permission::Internet,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Network,
        0xAE00000000000005,
        vec![ARG_I32],
        RET_BOOL,
        Permission::Internet,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Network,
        0xAE00000000000006,
        vec![ARG_HASH],
        RET_I64,
        Permission::Internet,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Network,
        0xAE00000000000007,
        vec![ARG_I32, ARG_I32],
        RET_BOOL,
        Permission::Internet,
        true,
    );

    // === Display APIs (6) ===
    ApiBindings::register_api(
        ApiCategory::Display,
        0xD100000000000001,
        vec![ARG_I32, ARG_I32, ARG_I32, ARG_I32],
        RET_I32,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Display,
        0xD100000000000002,
        vec![ARG_I32, ARG_BUFFER],
        RET_BOOL,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Display,
        0xD100000000000003,
        vec![ARG_I32],
        RET_BOOL,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Display,
        0xD100000000000004,
        vec![ARG_VOID],
        RET_I32,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Display,
        0xD100000000000005,
        vec![ARG_I32, ARG_I32, ARG_I32],
        RET_BOOL,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Display,
        0xD100000000000006,
        vec![ARG_I32, ARG_I32],
        RET_I32,
        Permission::Storage,
        true,
    );

    // === Audio APIs (5) ===
    ApiBindings::register_api(
        ApiCategory::Audio,
        0xAD00000000000001,
        vec![ARG_I32, ARG_I32, ARG_I32],
        RET_I32,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Audio,
        0xAD00000000000002,
        vec![ARG_I32, ARG_BUFFER],
        RET_I32,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Audio,
        0xAD00000000000003,
        vec![ARG_I32],
        RET_BOOL,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Audio,
        0xAD00000000000004,
        vec![ARG_I32, ARG_Q16],
        RET_BOOL,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Audio,
        0xAD00000000000005,
        vec![ARG_I32, ARG_I32],
        RET_I32,
        Permission::Microphone,
        true,
    );

    // === Input APIs (5) ===
    ApiBindings::register_api(
        ApiCategory::Input,
        0x1A00000000000001,
        vec![ARG_VOID],
        RET_I32,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Input,
        0x1A00000000000002,
        vec![ARG_VOID],
        RET_I32,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Input,
        0x1A00000000000003,
        vec![ARG_VOID],
        RET_I32,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Input,
        0x1A00000000000004,
        vec![ARG_I32],
        RET_BOOL,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Input,
        0x1A00000000000005,
        vec![ARG_I32, ARG_I32],
        RET_BOOL,
        Permission::Storage,
        true,
    );

    // === Sensor APIs (5) ===
    ApiBindings::register_api(
        ApiCategory::Sensors,
        0x5E00000000000001,
        vec![ARG_I32],
        RET_BOOL,
        Permission::Location,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Sensors,
        0x5E00000000000002,
        vec![ARG_I32],
        RET_Q16,
        Permission::Location,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Sensors,
        0x5E00000000000003,
        vec![ARG_VOID],
        RET_I32,
        Permission::Location,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Sensors,
        0x5E00000000000004,
        vec![ARG_I32, ARG_I32],
        RET_BOOL,
        Permission::Location,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Sensors,
        0x5E00000000000005,
        vec![ARG_I32],
        RET_BUFFER,
        Permission::Location,
        true,
    );

    // === Crypto APIs (5) ===
    ApiBindings::register_api(
        ApiCategory::Crypto,
        0xC400000000000001,
        vec![ARG_BUFFER],
        RET_BUFFER,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Crypto,
        0xC400000000000002,
        vec![ARG_BUFFER, ARG_BUFFER],
        RET_BUFFER,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Crypto,
        0xC400000000000003,
        vec![ARG_BUFFER, ARG_BUFFER],
        RET_BUFFER,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Crypto,
        0xC400000000000004,
        vec![ARG_I32],
        RET_BUFFER,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Crypto,
        0xC400000000000005,
        vec![ARG_BUFFER, ARG_BUFFER],
        RET_BOOL,
        Permission::Storage,
        true,
    );

    // === AI APIs (4) ===
    ApiBindings::register_api(
        ApiCategory::AI,
        0xA100000000000001,
        vec![ARG_BUFFER],
        RET_BUFFER,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::AI,
        0xA100000000000002,
        vec![ARG_BUFFER],
        RET_BUFFER,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::AI,
        0xA100000000000003,
        vec![ARG_BUFFER, ARG_I32],
        RET_I32,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::AI,
        0xA100000000000004,
        vec![ARG_BUFFER],
        RET_Q16,
        Permission::Storage,
        false,
    );

    // === Database APIs (5) ===
    ApiBindings::register_api(
        ApiCategory::Database,
        0xDB00000000000001,
        vec![ARG_HASH],
        RET_I32,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Database,
        0xDB00000000000002,
        vec![ARG_I32],
        RET_BOOL,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Database,
        0xDB00000000000003,
        vec![ARG_I32, ARG_BUFFER],
        RET_I32,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Database,
        0xDB00000000000004,
        vec![ARG_I32, ARG_BUFFER],
        RET_BUFFER,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::Database,
        0xDB00000000000005,
        vec![ARG_I32, ARG_HASH, ARG_BUFFER],
        RET_BOOL,
        Permission::Storage,
        true,
    );

    // === IPC APIs (5) ===
    ApiBindings::register_api(
        ApiCategory::IPC,
        0x1C00000000000001,
        vec![ARG_HASH],
        RET_I32,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::IPC,
        0x1C00000000000002,
        vec![ARG_I32, ARG_BUFFER],
        RET_BOOL,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::IPC,
        0x1C00000000000003,
        vec![ARG_I32, ARG_BUFFER],
        RET_I32,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::IPC,
        0x1C00000000000004,
        vec![ARG_I32],
        RET_BOOL,
        Permission::Storage,
        true,
    );
    ApiBindings::register_api(
        ApiCategory::IPC,
        0x1C00000000000005,
        vec![ARG_I32, ARG_I32],
        RET_BOOL,
        Permission::Storage,
        true,
    );

    serial_println!(
        "[api] registered {} built-in system APIs",
        ApiBindings::api_count()
    );
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialize the API bindings subsystem and register all built-in APIs
pub fn init() {
    {
        let mut guard = API_REGISTRY.lock();
        *guard = Some(ApiRegistryState::new());
    }
    {
        let mut guard = DRIVER_REGISTRY.lock();
        *guard = Some(DriverRegistry::new());
    }
    register_builtin_apis();
    serial_println!(
        "[api] API bindings initialized ({} endpoints, {} syscalls, driver registry ready)",
        ApiBindings::api_count(),
        SYSCALL_TABLE.len(),
    );
}
