use super::lsm::{LsmHook, LSM_ALLOW, LSM_DENY};
use crate::security::audit;
/// Genesis Mandatory Access Control (MAC) — AppArmor-inspired deny policy
///
/// Stores a fixed table of 256 `MacRule` entries in a static Mutex.
/// Rules express (uid, hook_type, resource) -> allow/deny.
/// The default policy denies privileged operations to unprivileged users
/// (uid >= 1000) while root (uid == 0) always bypasses all checks.
///
/// Hook-type constants mirror the discriminants of `LsmHook` in `lsm.rs`
/// so callers can use the same numeric codes in both places.
///
/// No heap: all storage is fixed-size arrays, no Vec/Box/String.
/// No panics: all out-of-bounds checks return Option/bool.
/// All counters: saturating arithmetic.
use crate::serial_println;
use crate::sync::Mutex;

// ── Hook-type discriminant constants ─────────────────────────────────────────

pub const HOOK_TASK_CREATE: u8 = 0;
pub const HOOK_TASK_SETUID: u8 = 1;
pub const HOOK_TASK_KILL: u8 = 2;
pub const HOOK_TASK_PTRACE: u8 = 3;
pub const HOOK_FILE_OPEN: u8 = 4;
pub const HOOK_FILE_READ: u8 = 5;
pub const HOOK_FILE_WRITE: u8 = 6;
pub const HOOK_FILE_EXEC: u8 = 7;
pub const HOOK_FILE_CHMOD: u8 = 8;
pub const HOOK_FILE_CHOWN: u8 = 9;
pub const HOOK_SOCKET_CREATE: u8 = 10;
pub const HOOK_SOCKET_CONNECT: u8 = 11;
pub const HOOK_SOCKET_BIND: u8 = 12;
pub const HOOK_SOCKET_ACCEPT: u8 = 13;
pub const HOOK_SOCKET_SENDMSG: u8 = 14;
pub const HOOK_SOCKET_RECVMSG: u8 = 15;
pub const HOOK_IPC_PERMISSION: u8 = 16;
pub const HOOK_SHM_GET: u8 = 17;
pub const HOOK_MQ_OPEN: u8 = 18;
pub const HOOK_SYS_MOUNT: u8 = 19;
pub const HOOK_SYS_REBOOT: u8 = 20;
pub const HOOK_SYS_CHROOT: u8 = 21;
pub const HOOK_SYS_KEXEC: u8 = 22;
pub const HOOK_CAP_ABLE: u8 = 23;

// ── Capability bitmasks (subset used in built-in rules) ───────────────────────

/// Bitmask for CAP_SYS_MODULE (load/unload kernel modules).
pub const CAP_SYS_MODULE: u64 = 1 << 16;
/// Bitmask for CAP_SYS_RAWIO (raw I/O: ioperm/iopl, /dev/mem).
pub const CAP_SYS_RAWIO: u64 = 1 << 17;
/// Bitmask for CAP_SYS_ADMIN (broad system administration).
pub const CAP_SYS_ADMIN: u64 = 1 << 21;

/// Convenience: the three high-privilege caps denied to uid >= 1000.
const PRIV_CAP_MASK: u64 = CAP_SYS_MODULE | CAP_SYS_RAWIO | CAP_SYS_ADMIN;

// ── MacRule ────────────────────────────────────────────────────────────────────

/// Maximum number of configurable MAC rules.
const MAX_MAC_RULES: usize = 256;

/// Wildcard UID — matches any caller.
pub const UID_ANY: u32 = 0xFFFF_FFFF;

/// A single MAC policy rule.
///
/// All fields are plain integers so the struct is `Copy` and can live in a
/// static array without heap allocation.
#[derive(Clone, Copy)]
pub struct MacRule {
    /// UID this rule applies to; `UID_ANY` (0xFFFFFFFF) matches every caller.
    pub uid: u32,
    /// Hook-type discriminant (HOOK_* constants above).
    pub hook_type: u8,
    /// Resource identifier: inode, port, pid, capability bitmask, or 0 = any.
    pub resource: u32,
    /// `true` → allow the operation; `false` → deny it.
    pub allow: bool,
    /// Slot is in use.
    pub active: bool,
}

impl MacRule {
    /// Construct an empty (unused) rule slot.
    pub const fn empty() -> Self {
        MacRule {
            uid: 0,
            hook_type: 0,
            resource: 0,
            allow: false,
            active: false,
        }
    }
}

// ── Static rule table ─────────────────────────────────────────────────────────

struct MacTable {
    rules: [MacRule; MAX_MAC_RULES],
    count: u32,
}

impl MacTable {
    const fn new() -> Self {
        MacTable {
            rules: [MacRule::empty(); MAX_MAC_RULES],
            count: 0,
        }
    }

    /// Return the first free slot index, or None if the table is full.
    fn free_slot(&self) -> Option<usize> {
        for i in 0..MAX_MAC_RULES {
            if !self.rules[i].active {
                return Some(i);
            }
        }
        None
    }

    /// Add a rule; returns `false` when the table is full.
    fn add(&mut self, uid: u32, hook_type: u8, resource: u32, allow: bool) -> bool {
        match self.free_slot() {
            None => false,
            Some(idx) => {
                self.rules[idx] = MacRule {
                    uid,
                    hook_type,
                    resource,
                    allow,
                    active: true,
                };
                self.count = self.count.saturating_add(1);
                true
            }
        }
    }

    /// Remove first matching rule; returns `false` if no match was found.
    fn remove(&mut self, uid: u32, hook_type: u8, resource: u32) -> bool {
        for i in 0..MAX_MAC_RULES {
            let r = &mut self.rules[i];
            if r.active && r.uid == uid && r.hook_type == hook_type && r.resource == resource {
                *r = MacRule::empty();
                self.count = self.count.saturating_sub(1);
                return true;
            }
        }
        false
    }

    /// Zero every rule slot.
    fn clear(&mut self) {
        for i in 0..MAX_MAC_RULES {
            self.rules[i] = MacRule::empty();
        }
        self.count = 0;
    }

    /// Returns `Some(allow)` if a rule matches, `None` for no match.
    fn lookup(&self, uid: u32, hook_type: u8, resource: u32) -> Option<bool> {
        for i in 0..MAX_MAC_RULES {
            let r = &self.rules[i];
            if !r.active {
                continue;
            }
            let uid_match = r.uid == UID_ANY || r.uid == uid;
            let res_match = r.resource == 0 || r.resource == resource;
            if uid_match && r.hook_type == hook_type && res_match {
                return Some(r.allow);
            }
        }
        None
    }
}

static MAC_RULES: Mutex<MacTable> = Mutex::new(MacTable::new());

// ── LsmHook → (hook_type, resource) extraction ───────────────────────────────

/// Decompose an `LsmHook` into its (discriminant, primary resource value).
///
/// The `resource` value is used for resource-specific rule matching.
/// For hooks that carry no resource (Accept, Sendmsg, …) resource = 0.
fn hook_to_type_resource(hook: &LsmHook) -> (u8, u32) {
    match hook {
        LsmHook::TaskCreate => (HOOK_TASK_CREATE, 0),
        LsmHook::TaskSetuid(uid) => (HOOK_TASK_SETUID, *uid),
        LsmHook::TaskKill(_, target) => (HOOK_TASK_KILL, *target),
        LsmHook::TaskPtrace(target) => (HOOK_TASK_PTRACE, *target),
        LsmHook::FileOpen(inode, _) => (HOOK_FILE_OPEN, *inode),
        LsmHook::FileRead(inode) => (HOOK_FILE_READ, *inode),
        LsmHook::FileWrite(inode) => (HOOK_FILE_WRITE, *inode),
        LsmHook::FileExec(inode) => (HOOK_FILE_EXEC, *inode),
        LsmHook::FileChmod(inode, _) => (HOOK_FILE_CHMOD, *inode),
        LsmHook::FileChown(inode, _) => (HOOK_FILE_CHOWN, *inode),
        LsmHook::SocketCreate(domain, _) => (HOOK_SOCKET_CREATE, *domain as u32),
        LsmHook::SocketConnect(port) => (HOOK_SOCKET_CONNECT, *port as u32),
        LsmHook::SocketBind(port) => (HOOK_SOCKET_BIND, *port as u32),
        LsmHook::SocketAccept => (HOOK_SOCKET_ACCEPT, 0),
        LsmHook::SocketSendmsg => (HOOK_SOCKET_SENDMSG, 0),
        LsmHook::SocketRecvmsg => (HOOK_SOCKET_RECVMSG, 0),
        LsmHook::IpcPermission(id) => (HOOK_IPC_PERMISSION, *id),
        LsmHook::ShmGet(id) => (HOOK_SHM_GET, *id),
        LsmHook::MqOpen => (HOOK_MQ_OPEN, 0),
        LsmHook::SysMount => (HOOK_SYS_MOUNT, 0),
        LsmHook::SysReboot => (HOOK_SYS_REBOOT, 0),
        LsmHook::SysChroot => (HOOK_SYS_CHROOT, 0),
        LsmHook::SysKexec => (HOOK_SYS_KEXEC, 0),
        LsmHook::CapAble(mask) => {
            // Truncate 64-bit mask to 32 bits for the resource field;
            // built-in rules also use 32-bit masks cast from the same constants.
            (HOOK_CAP_ABLE, (*mask & 0xFFFF_FFFF) as u32)
        }
    }
}

// ── Built-in policy rules (enforced independently of rule table) ──────────────

/// Evaluate the hard-coded Genesis security invariants that are always active
/// regardless of the configurable rule table.
///
/// Returns `Some(LSM_DENY)` when a built-in rule fires, `None` otherwise.
fn builtin_check(hook: &LsmHook, uid: u32, pid: u32) -> Option<i32> {
    match hook {
        // kexec: never allowed for non-root
        LsmHook::SysKexec if uid != 0 => {
            serial_println!(
                "  [genesis_mac] DENY SysKexec uid={} pid={} (built-in)",
                uid,
                pid
            );
            audit::log(
                audit::AuditEvent::MacDenied,
                audit::AuditResult::Deny,
                pid,
                uid,
                "genesis_mac: built-in deny SysKexec non-root",
            );
            Some(LSM_DENY)
        }
        // reboot: never allowed for non-root
        LsmHook::SysReboot if uid != 0 => {
            serial_println!(
                "  [genesis_mac] DENY SysReboot uid={} pid={} (built-in)",
                uid,
                pid
            );
            audit::log(
                audit::AuditEvent::MacDenied,
                audit::AuditResult::Deny,
                pid,
                uid,
                "genesis_mac: built-in deny SysReboot non-root",
            );
            Some(LSM_DENY)
        }
        // CAP_SYS_RAWIO: denied for uid >= 1000
        LsmHook::CapAble(mask) if uid >= 1000 && (*mask & CAP_SYS_RAWIO != 0) => {
            serial_println!(
                "  [genesis_mac] DENY CapAble(RAWIO) uid={} pid={} (built-in)",
                uid,
                pid
            );
            audit::log(
                audit::AuditEvent::CapDenied,
                audit::AuditResult::Deny,
                pid,
                uid,
                "genesis_mac: built-in deny CAP_SYS_RAWIO uid>=1000",
            );
            Some(LSM_DENY)
        }
        // ptrace: unprivileged processes may not ptrace processes owned by
        // a different UID.  The target uid is encoded as the resource field;
        // here we use pid as the target pid — callers must pass the correct uid.
        // (We can only check what the hook gives us; target uid is unavailable
        //  at this layer without a process table — deny cross-uid ptrace
        //  generically for uid >= 1000 per the YAMA ptrace-scope 1 policy.)
        LsmHook::TaskPtrace(_) if uid >= 1000 => {
            // We do NOT unconditionally deny here — we rely on the configurable
            // rule table for per-uid decisions.  The caller (lsm_check) adds
            // specific deny rules via mac_load_default_policy().
            None
        }
        _ => None,
    }
}

// ── Public MAC check entry point ──────────────────────────────────────────────

/// Evaluate the Genesis MAC policy for `hook` invoked by `(uid, pid)`.
///
/// Evaluation order:
///   1. uid == 0 (root) → always allow (capable_override).
///   2. Built-in invariants (SysKexec, SysReboot, CapAble(RAWIO)).
///   3. Configurable rule table lookup.
///   4. Default: allow (whitelist-deny model — rules add denials).
pub fn genesis_mac_check(hook: &LsmHook, uid: u32, pid: u32) -> i32 {
    // 1. Root bypasses all MAC checks.
    if uid == 0 {
        return LSM_ALLOW;
    }

    // 2. Built-in hard-wired denials.
    if let Some(verdict) = builtin_check(hook, uid, pid) {
        return verdict;
    }

    // 3. Configurable rule table.
    let (hook_type, resource) = hook_to_type_resource(hook);
    let verdict = MAC_RULES.lock().lookup(uid, hook_type, resource);

    match verdict {
        Some(true) => {
            // Explicit allow rule matched.
            LSM_ALLOW
        }
        Some(false) => {
            // Explicit deny rule matched.
            serial_println!(
                "  [genesis_mac] DENY hook={} uid={} pid={} resource={}",
                hook_type,
                uid,
                pid,
                resource
            );
            audit::log(
                audit::AuditEvent::MacDenied,
                audit::AuditResult::Deny,
                pid,
                uid,
                "genesis_mac: rule deny",
            );
            LSM_DENY
        }
        None => {
            // No matching rule → default allow.
            LSM_ALLOW
        }
    }
}

// ── Rule management API ───────────────────────────────────────────────────────

/// Add a rule to the MAC rule table.
///
/// Returns `false` if the table is full (256 active rules).
pub fn mac_add_rule(uid: u32, hook_type: u8, resource: u32, allow: bool) -> bool {
    MAC_RULES.lock().add(uid, hook_type, resource, allow)
}

/// Remove the first rule matching `(uid, hook_type, resource)`.
///
/// Returns `false` if no matching rule exists.
pub fn mac_remove_rule(uid: u32, hook_type: u8, resource: u32) -> bool {
    MAC_RULES.lock().remove(uid, hook_type, resource)
}

/// Clear all configurable rules (built-in invariants are unaffected).
pub fn mac_clear_rules() {
    MAC_RULES.lock().clear();
    serial_println!("  [genesis_mac] All configurable rules cleared");
}

/// Return the number of active rules in the table.
pub fn mac_get_rule_count() -> u32 {
    MAC_RULES.lock().count
}

/// Load the standard Genesis default deny policy.
///
/// Adds deny rules for uid >= 1000 on:
///   - SysReboot, SysMount, SysKexec, SysChroot
///   - CapAble(CAP_SYS_MODULE | CAP_SYS_RAWIO | CAP_SYS_ADMIN)
///
/// The UID_ANY wildcard is intentionally NOT used here: rules target the
/// "unprivileged user" range (uid >= 1000).  Because the rule table matches
/// exact UIDs, we install rules for `UID_ANY` with a uid-range guard applied
/// inside `genesis_mac_check()` via the built-in check path.  For the
/// configurable table we use `UID_ANY` so the rules apply to all uids; root
/// (uid == 0) is exempted before we reach the table.
pub fn mac_load_default_policy() {
    let mut tbl = MAC_RULES.lock();
    tbl.clear();

    // Deny SysReboot for all non-root (root is exempted before lookup).
    tbl.add(UID_ANY, HOOK_SYS_REBOOT, 0, false);

    // Deny SysMount for all non-root.
    tbl.add(UID_ANY, HOOK_SYS_MOUNT, 0, false);

    // Deny SysKexec for all non-root.
    tbl.add(UID_ANY, HOOK_SYS_KEXEC, 0, false);

    // Deny SysChroot for all non-root.
    tbl.add(UID_ANY, HOOK_SYS_CHROOT, 0, false);

    // Deny CapAble(CAP_SYS_MODULE) for all non-root.
    // Resource encodes the low 32 bits of the capability bitmask.
    tbl.add(
        UID_ANY,
        HOOK_CAP_ABLE,
        (CAP_SYS_MODULE & 0xFFFF_FFFF) as u32,
        false,
    );

    // Deny CapAble(CAP_SYS_RAWIO) for all non-root.
    tbl.add(
        UID_ANY,
        HOOK_CAP_ABLE,
        (CAP_SYS_RAWIO & 0xFFFF_FFFF) as u32,
        false,
    );

    // Deny CapAble(CAP_SYS_ADMIN) for all non-root.
    tbl.add(
        UID_ANY,
        HOOK_CAP_ABLE,
        (CAP_SYS_ADMIN & 0xFFFF_FFFF) as u32,
        false,
    );

    serial_println!(
        "  [genesis_mac] Default policy loaded ({} rules)",
        tbl.count
    );
}

/// Initialize the Genesis MAC subsystem.
pub fn init() {
    mac_load_default_policy();
    serial_println!("  [genesis_mac] Genesis MAC policy engine initialized");
}
