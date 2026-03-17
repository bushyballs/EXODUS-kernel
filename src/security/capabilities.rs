/// POSIX capabilities — per-process capability sets for Genesis
///
/// Provides the five-set Linux capability model (permitted, effective,
/// inheritable, ambient, bounding) for every process tracked by the kernel.
///
/// Storage: a fixed-size array of 256 `CapSet` entries indexed by
/// `pid % 256`.  This is intentionally a simple modular hash — collision
/// handling is the responsibility of higher-level process management.
///
/// Critical rules:
///   - No heap: `[CapSet; 256]` is a plain static array.
///   - No panics: every array access is bounds-checked; functions return
///     `Option` or `bool` where errors are possible.
///   - No float casts.
///   - All counters use saturating arithmetic.
///   - The Mutex inner type is `Copy` and has a `const fn empty()`.
use crate::serial_println;
use crate::sync::Mutex;

// ── Capability bit positions (matching Linux capabilities(7)) ─────────────────

/// Change file ownership (chown/fchown).
pub const CAP_CHOWN: u64 = 1 << 0;
/// Override DAC restrictions on file reads/writes/executes.
pub const CAP_DAC_OVERRIDE: u64 = 1 << 1;
/// Allow setuid/setgid bits to be set on files.
pub const CAP_FSETID: u64 = 1 << 4;
/// Send signals to arbitrary processes (bypass UID check).
pub const CAP_KILL: u64 = 1 << 5;
/// Manipulate supplementary group list.
pub const CAP_SETGID: u64 = 1 << 6;
/// Manipulate UID (setuid/seteuid/setreuid).
pub const CAP_SETUID: u64 = 1 << 7;
/// Bind to privileged TCP/UDP ports (< 1024).
pub const CAP_NET_BIND: u64 = 1 << 10;
/// Perform various network administrative operations.
pub const CAP_NET_ADMIN: u64 = 1 << 12;
/// Use RAW and PACKET sockets; bind to any address.
pub const CAP_NET_RAW: u64 = 1 << 13;
/// Load and unload kernel modules.
pub const CAP_SYS_MODULE: u64 = 1 << 16;
/// Raw I/O: ioperm/iopl, access to /dev/mem, /dev/kmem.
pub const CAP_SYS_RAWIO: u64 = 1 << 17;
/// Use chroot.
pub const CAP_SYS_CHROOT: u64 = 1 << 18;
/// Trace arbitrary processes via ptrace.
pub const CAP_SYS_PTRACE: u64 = 1 << 19;
/// Perform a range of system administration operations.
pub const CAP_SYS_ADMIN: u64 = 1 << 21;
/// Call reboot, kexec_load.
pub const CAP_SYS_BOOT: u64 = 1 << 22;

/// Bitmask of all capability bits defined above (convenience constant).
pub const CAP_ALL_VALID: u64 = (1u64 << 41).wrapping_sub(1);

// ── Per-process capability set ────────────────────────────────────────────────

/// The five POSIX capability sets for a single process.
///
/// All fields are `u64` bitmasks — no heap, no pointers.  Derives `Copy` and
/// `Clone` so the struct can live inside a static `Mutex`.
#[derive(Clone, Copy)]
pub struct CapSet {
    /// Caps that may be made effective.
    pub permitted: u64,
    /// Caps currently active for access checks.
    pub effective: u64,
    /// Caps preserved across exec (combined with file inheritable).
    pub inheritable: u64,
    /// Caps always added to effective/permitted on exec.
    pub ambient: u64,
    /// Hard upper limit on caps that can be raised via exec.
    pub bounding: u64,
}

impl CapSet {
    /// All-privileges root set (uid == 0).
    pub const fn root() -> Self {
        CapSet {
            permitted: CAP_ALL_VALID,
            effective: CAP_ALL_VALID,
            inheritable: 0,
            ambient: 0,
            bounding: CAP_ALL_VALID,
        }
    }

    /// Empty unprivileged set.
    pub const fn empty() -> Self {
        CapSet {
            permitted: 0,
            effective: 0,
            inheritable: 0,
            ambient: 0,
            bounding: CAP_ALL_VALID,
        }
    }

    /// Test whether `cap` is in the effective set.
    #[inline(always)]
    pub fn has(&self, cap: u64) -> bool {
        self.effective & cap != 0
    }

    /// Permanently drop `cap` from effective and permitted sets (irreversible).
    pub fn drop(&mut self, cap: u64) {
        let mask = !cap;
        self.effective &= mask;
        self.permitted &= mask;
    }
}

// ── Per-process capability table ──────────────────────────────────────────────

/// Number of PID slots in the capability table.
const CAP_TABLE_SIZE: usize = 256;

/// Fixed-size array of capability sets, indexed by `pid % CAP_TABLE_SIZE`.
///
/// All slots start as `CapSet::root()` — new processes inherit full caps
/// and the process manager is expected to call `cap_set` / `cap_setuid_drop`
/// to restrict them appropriately.
struct ProcessCapTable {
    slots: [CapSet; CAP_TABLE_SIZE],
}

impl ProcessCapTable {
    const fn new() -> Self {
        ProcessCapTable {
            // All processes start with root capabilities.
            slots: [CapSet::root(); CAP_TABLE_SIZE],
        }
    }

    #[inline(always)]
    fn idx(pid: u32) -> usize {
        (pid as usize) % CAP_TABLE_SIZE
    }

    fn get(&self, pid: u32) -> CapSet {
        self.slots[Self::idx(pid)]
    }

    fn set(&mut self, pid: u32, caps: CapSet) {
        self.slots[Self::idx(pid)] = caps;
    }

    fn drop_cap(&mut self, pid: u32, cap: u64) {
        self.slots[Self::idx(pid)].drop(cap);
    }
}

static PROCESS_CAPS: Mutex<ProcessCapTable> = Mutex::new(ProcessCapTable::new());

// ── Public capability API ─────────────────────────────────────────────────────

/// Return the capability set for `pid`.
///
/// If the PID was never explicitly set the slot holds `CapSet::root()`
/// (the default for all slots — safe because the kernel starts with a single
/// privileged process and explicitly restricts user processes via
/// `cap_setuid_drop`).
pub fn cap_get(pid: u32) -> CapSet {
    PROCESS_CAPS.lock().get(pid)
}

/// Replace the capability set for `pid`.
///
/// Always succeeds (returns `true`); the `bool` return type is kept for
/// interface symmetry with other subsystems that may need fallible variants.
pub fn cap_set(pid: u32, caps: CapSet) -> bool {
    PROCESS_CAPS.lock().set(pid, caps);
    true
}

/// Test whether `pid` holds `cap` in its effective set.
pub fn cap_has(pid: u32, cap: u64) -> bool {
    PROCESS_CAPS.lock().get(pid).has(cap)
}

/// Drop `cap` from `pid`'s effective and permitted sets (irreversible).
pub fn cap_drop(pid: u32, cap: u64) {
    PROCESS_CAPS.lock().drop_cap(pid, cap);
}

/// Called on setuid: if the new UID is non-zero, clear all capabilities.
///
/// This mirrors the Linux exec-time capability drop for non-root processes:
/// when a process transitions away from uid == 0 it loses all privileges
/// unless it explicitly holds `CAP_SETUID` and re-acquires them.
///
/// Simplified Genesis rule: new_uid != 0 → empty cap set.
pub fn cap_setuid_drop(pid: u32, new_uid: u32) {
    if new_uid != 0 {
        PROCESS_CAPS.lock().set(pid, CapSet::empty());
        serial_println!(
            "  [capabilities] pid={} transitioned to uid={}: capabilities cleared",
            pid,
            new_uid
        );
    }
    // uid == 0: retain existing caps (setuid(0) from a privileged helper).
}

/// Initialize capabilities for a process: root gets full, non-root gets none.
pub fn caps_init_process(pid: u32, is_root: bool) -> bool {
    let caps = if is_root {
        CapSet::root()
    } else {
        CapSet::empty()
    };
    PROCESS_CAPS.lock().set(pid, caps);
    true
}

/// Drop a specific capability from effective+permitted sets (irreversible).
pub fn caps_drop(pid: u32, cap: u64) -> bool {
    PROCESS_CAPS.lock().drop_cap(pid, cap);
    true
}

/// Check if a process has a capability in its effective set.
pub fn caps_has(pid: u32, cap: u64) -> bool {
    PROCESS_CAPS.lock().get(pid).has(cap)
}

/// Set the effective capability set to (mask & permitted).
pub fn caps_set_effective(pid: u32, cap_mask: u64) -> bool {
    let mut table = PROCESS_CAPS.lock();
    let mut caps = table.get(pid);
    // Effective can only be a subset of permitted
    caps.effective = cap_mask & caps.permitted;
    table.set(pid, caps);
    true
}

/// Add a capability to the ambient set (if in permitted & inheritable).
pub fn caps_add_ambient(pid: u32, cap: u64) -> bool {
    let mut table = PROCESS_CAPS.lock();
    let mut caps = table.get(pid);
    // Ambient can only contain caps in both permitted and inheritable
    if (caps.permitted & cap) != 0 && (caps.inheritable & cap) != 0 {
        caps.ambient |= cap;
        table.set(pid, caps);
        true
    } else {
        false
    }
}

/// Compute new capabilities after exec:
/// new_permitted = (inheritable & file_inheritable) | (file_permitted & ambient)
/// new_effective = new_permitted
pub fn caps_exec_transition(pid: u32, file_permitted: u64, file_inheritable: u64) {
    let mut table = PROCESS_CAPS.lock();
    let mut caps = table.get(pid);

    let new_permitted = (caps.inheritable & file_inheritable) | (file_permitted & caps.ambient);
    caps.permitted = new_permitted;
    caps.effective = new_permitted;
    // inheritable stays the same across exec

    table.set(pid, caps);
}

/// Return the capability set for a process.
pub fn caps_get(pid: u32) -> Option<CapSet> {
    Some(PROCESS_CAPS.lock().get(pid))
}

/// Remove a process from the capability table.
pub fn caps_remove_process(pid: u32) -> bool {
    let mut table = PROCESS_CAPS.lock();
    table.set(pid, CapSet::empty());
    true
}

/// Initialize the capabilities subsystem.
///
/// Initializes pid 0 (kernel, root) and pid 1 (init, root) with full capabilities.
/// All other slots start with `CapSet::root()` (the default).
pub fn init() {
    let mut table = PROCESS_CAPS.lock();

    // PID 0: kernel (root) — full capabilities
    table.set(0, CapSet::root());

    // PID 1: init (root) — full capabilities
    table.set(1, CapSet::root());

    serial_println!("  [capabilities] Linux capabilities initialized (pid 0 and 1 set to root)");
}
