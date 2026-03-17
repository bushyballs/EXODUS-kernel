use crate::security::audit;
/// Linux Security Module (LSM) hook framework — Genesis implementation
///
/// Provides a typed hook enum (`LsmHook`) and a policy engine (`LsmPolicy`)
/// that dispatches each hook invocation to the Genesis MAC layer
/// (`genesis_mac::genesis_mac_check`).
///
/// Two operational modes:
///   - Enforcing (default): denials are returned to callers.
///   - Permissive: denials are logged but the operation is allowed
///     (useful during policy development and boot).
///
/// If the policy has not been initialized yet the engine fails open
/// (allows everything) to avoid blocking early-boot kernel operations.
///
/// No heap: no Vec, Box, String, alloc::* anywhere.
/// No panics: no unwrap(), expect(), panic!().
/// All counters: saturating_add / saturating_sub.
/// No float casts.
use crate::serial_println;
use crate::sync::Mutex;

// ── Hook return values ────────────────────────────────────────────────────────

/// Operation allowed.
pub const LSM_ALLOW: i32 = 0;
/// Operation denied (maps to EPERM).
pub const LSM_DENY: i32 = -1;

// ── LsmHook — typed hook variants ────────────────────────────────────────────

/// Every kernel operation subject to LSM policy is represented as one
/// `LsmHook` variant.  The enum is `Copy + Clone` so it can be passed by
/// value without heap allocation.
#[derive(Clone, Copy)]
pub enum LsmHook {
    // ── Process hooks ─────────────────────────────────────────────────────
    /// A new process/thread is being created (fork/clone).
    TaskCreate,
    /// setuid: `u32` is the target uid.
    TaskSetuid(u32),
    /// Signal delivery: `(killer_pid, target_pid)`.
    TaskKill(u32, u32),
    /// ptrace attach: `u32` is the target pid.
    TaskPtrace(u32),

    // ── File hooks ────────────────────────────────────────────────────────
    /// File open: `(inode, mode_flags)`.
    FileOpen(u32, u8),
    /// File read: `inode`.
    FileRead(u32),
    /// File write: `inode`.
    FileWrite(u32),
    /// File exec: `inode`.
    FileExec(u32),
    /// chmod: `(inode, new_mode)`.
    FileChmod(u32, u32),
    /// chown: `(inode, new_uid)`.
    FileChown(u32, u32),

    // ── Socket hooks ──────────────────────────────────────────────────────
    /// Socket creation: `(domain, type)`.
    SocketCreate(u16, u16),
    /// Outgoing connection: `port`.
    SocketConnect(u16),
    /// Bind to local port: `port`.
    SocketBind(u16),
    /// Incoming connection accepted.
    SocketAccept,
    /// Send message on socket.
    SocketSendmsg,
    /// Receive message on socket.
    SocketRecvmsg,

    // ── IPC hooks ─────────────────────────────────────────────────────────
    /// IPC object permission check: `ipc_object_id`.
    IpcPermission(u32),
    /// Shared memory get: `shm_id`.
    ShmGet(u32),
    /// POSIX message queue open.
    MqOpen,

    // ── System hooks ──────────────────────────────────────────────────────
    /// mount(2) system call.
    SysMount,
    /// reboot(2) system call.
    SysReboot,
    /// chroot(2) system call.
    SysChroot,
    /// kexec_load / kexec_file_load.
    SysKexec,
    /// Capability check: `cap_bitmask`.
    CapAble(u64),
}

// ── LsmPolicy ─────────────────────────────────────────────────────────────────

/// Global LSM policy state.
///
/// `Copy` is required for storage in a static `Mutex`.
#[derive(Clone, Copy)]
pub struct LsmPolicy {
    /// In permissive mode denials are logged but operations are allowed.
    pub permissive: bool,
    /// When true, log every denial to the audit ring and serial.
    pub audit: bool,
    /// Set to true once `init()` completes.
    pub initialized: bool,
}

impl LsmPolicy {
    pub const fn empty() -> Self {
        LsmPolicy {
            permissive: false,
            audit: true,
            initialized: false,
        }
    }
}

static LSM_POLICY: Mutex<LsmPolicy> = Mutex::new(LsmPolicy::empty());

// ── Public LSM check API ──────────────────────────────────────────────────────

/// Evaluate the LSM policy for `hook` invoked by (`caller_uid`, `caller_pid`).
///
/// Behaviour:
///   - Not initialized → `LSM_ALLOW` (fail-open during boot).
///   - Permissive mode → log potential denial but return `LSM_ALLOW`.
///   - Enforcing mode  → delegate to Genesis MAC; return its verdict.
pub fn lsm_check(hook: LsmHook, caller_uid: u32, caller_pid: u32) -> i32 {
    let policy = *LSM_POLICY.lock();

    if !policy.initialized {
        return LSM_ALLOW;
    }

    let verdict = crate::security::genesis_mac::genesis_mac_check(&hook, caller_uid, caller_pid);

    if verdict == LSM_DENY {
        if policy.audit {
            audit::log(
                audit::AuditEvent::MacDenied,
                audit::AuditResult::Deny,
                caller_pid,
                caller_uid,
                "lsm_check: genesis_mac denied",
            );
        }
        if policy.permissive {
            serial_println!(
                "  [lsm] PERMISSIVE: would deny uid={} pid={} (logged only)",
                caller_uid,
                caller_pid
            );
            return LSM_ALLOW;
        }
        return LSM_DENY;
    }

    LSM_ALLOW
}

/// Switch permissive mode on (`true`) or off (`false`).
pub fn lsm_set_permissive(val: bool) {
    LSM_POLICY.lock().permissive = val;
    serial_println!("  [lsm] Permissive mode: {}", val);
}

/// Enable (`true`) or disable (`false`) audit logging of denials.
pub fn lsm_set_audit(val: bool) {
    LSM_POLICY.lock().audit = val;
}

/// Query whether the LSM is currently in permissive mode.
pub fn lsm_is_permissive() -> bool {
    LSM_POLICY.lock().permissive
}

/// Initialize the LSM framework.
///
/// Loads the Genesis MAC default policy and marks the engine as ready.
/// Must be called after `audit::init()` and `genesis_mac::init()`.
pub fn init() {
    {
        let mut policy = LSM_POLICY.lock();
        policy.initialized = true;
        policy.permissive = false;
        policy.audit = true;
    }
    serial_println!("  [lsm] LSM framework initialized (enforcing, audit=true)");
    serial_println!(
        "  [lsm] Hook types: TaskCreate/Kill/Setuid/Ptrace, File*, Socket*, IPC*, Sys*, CapAble"
    );
}
