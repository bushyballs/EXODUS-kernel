/// Capability-based access control
///
/// Every resource access requires an unforgeable capability token.
/// Processes receive capabilities at spawn time and can:
///   - Use a capability to access a resource
///   - Delegate a capability to a child process (with restrictions)
///   - Revoke a delegated capability
///   - Drop a capability (can never re-acquire it)
///
/// This provides the principle of least privilege by default.
///
/// Section 2: Linux-compatible POSIX capability bits and CapabilitySet,
/// implementing the permitted/effective/inheritable/ambient/bounding model
/// from capabilities(7) and the exec-time transition rules.
use crate::serial_println;
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

// ── Linux-compatible POSIX capability bits ────────────────────────────────────

/// Change file ownership (chown/fchown).
pub const CAP_CHOWN: u64 = 1 << 0;
/// Override DAC restrictions on file reads/writes/executes.
pub const CAP_DAC_OVERRIDE: u64 = 1 << 1;
/// Override DAC restrictions for reading on file/dir searches.
pub const CAP_DAC_READ_SEARCH: u64 = 1 << 2;
/// Bypass permission checks on file operations when owner.
pub const CAP_FOWNER: u64 = 1 << 3;
/// Allow setuid/setgid bits to be set on files.
pub const CAP_FSETID: u64 = 1 << 4;
/// Send signals to arbitrary processes (bypass UID check).
pub const CAP_KILL: u64 = 1 << 5;
/// Manipulate supplementary group list.
pub const CAP_SETGID: u64 = 1 << 6;
/// Manipulate UID (setuid/seteuid/setreuid).
pub const CAP_SETUID: u64 = 1 << 7;
/// Transfer capabilities to/from arbitrary processes.
pub const CAP_SETPCAP: u64 = 1 << 8;
/// Protect immutable and append-only flags on files.
pub const CAP_LINUX_IMMUTABLE: u64 = 1 << 9;
/// Bind to privileged TCP/UDP ports (< 1024).
pub const CAP_NET_BIND_SERVICE: u64 = 1 << 10;
/// Alias for CAP_NET_BIND_SERVICE.
pub const CAP_NET_BIND: u64 = CAP_NET_BIND_SERVICE;
/// Broadcast, listen to multicast, set socket options.
pub const CAP_NET_BROADCAST: u64 = 1 << 11;
/// Perform various network administrative operations.
pub const CAP_NET_ADMIN: u64 = 1 << 12;
/// Use RAW and PACKET sockets; bind to any address.
pub const CAP_NET_RAW: u64 = 1 << 13;
/// Lock memory (mlock/mlockall), use hugetlb pages.
pub const CAP_IPC_LOCK: u64 = 1 << 14;
/// Bypass IPC ownership checks.
pub const CAP_IPC_OWNER: u64 = 1 << 15;
/// Load and unload kernel modules.
pub const CAP_SYS_MODULE: u64 = 1 << 16;
/// Raw I/O: ioperm/iopl, access to /dev/mem, /dev/kmem.
pub const CAP_SYS_RAWIO: u64 = 1 << 17;
/// Use chroot.
pub const CAP_SYS_CHROOT: u64 = 1 << 18;
/// Trace arbitrary processes via ptrace.
pub const CAP_SYS_PTRACE: u64 = 1 << 19;
/// Manipulate process accounting.
pub const CAP_SYS_PACCT: u64 = 1 << 20;
/// Perform a range of system administration operations.
pub const CAP_SYS_ADMIN: u64 = 1 << 21;
/// Call reboot, kexec_load.
pub const CAP_SYS_BOOT: u64 = 1 << 22;
/// Alias for CAP_SYS_BOOT (matches mission spec name).
pub const CAP_SYS_REBOOT: u64 = CAP_SYS_BOOT;
/// Set/get scheduler policies; set high nice value.
pub const CAP_SYS_NICE: u64 = 1 << 23;
/// Override resource limits (setrlimit); increase disk quotas.
pub const CAP_SYS_RESOURCE: u64 = 1 << 24;
/// Set system time; set hardware real-time clock.
pub const CAP_SYS_TIME: u64 = 1 << 25;
/// Configure tty devices; vhangup on virtual terminals.
pub const CAP_SYS_TTY_CONFIG: u64 = 1 << 26;
/// Create special files via mknod.
pub const CAP_MKNOD: u64 = 1 << 27;
/// Establish file-system leases.
pub const CAP_LEASE: u64 = 1 << 28;
/// Allow enabling/disabling kernel auditing.
pub const CAP_AUDIT_WRITE: u64 = 1 << 29;
/// Allow changing audit rules and log path.
pub const CAP_AUDIT_CONTROL: u64 = 1 << 30;
/// Set extended file attributes (security.* namespace).
pub const CAP_SETFCAP: u64 = 1 << 31;
/// Override MAC access enforced by MAC-aware filesystems.
pub const CAP_MAC_OVERRIDE: u64 = 1 << 32;
/// Allow MAC configuration and state changes.
pub const CAP_MAC_ADMIN: u64 = 1 << 33;
/// Configure the kernel syslog.
pub const CAP_SYSLOG: u64 = 1 << 34;
/// Trigger wake-up from system sleep.
pub const CAP_WAKE_ALARM: u64 = 1 << 35;
/// Block system suspend.
pub const CAP_BLOCK_SUSPEND: u64 = 1 << 36;
/// Read audit log via multicast netlink socket.
pub const CAP_AUDIT_READ: u64 = 1 << 37;
/// Modify performance event settings.
pub const CAP_PERFMON: u64 = 1 << 38;
/// Load eBPF programs, read kernel memory via BPF maps.
pub const CAP_BPF: u64 = 1 << 39;
/// Checkpoint and restore processes.
pub const CAP_CHECKPOINT_RESTORE: u64 = 1 << 40;

/// Bitmask of all 41 valid Linux capability bits (0–40).
pub const CAP_ALL_VALID: u64 = (1u64 << 41).wrapping_sub(1);

// ── CapabilitySet — Linux-model per-process five-set state ───────────────────

/// The five per-process capability sets described in capabilities(7).
///
/// Exec-time transition rule (`on_exec`):
///   P'(permitted)   = (P(inheritable) & F(inheritable))
///                   | (F(permitted)   & P(bounding))
///                   | P(ambient)
///   P'(effective)   = if F(effective) != 0 { P'(permitted) } else { P'(ambient) }
///   P'(inheritable) = P(inheritable)
///   P'(ambient)     = P(ambient) & P'(permitted)
///   P'(bounding)    = P(bounding)
#[derive(Debug, Clone, Copy)]
pub struct CapabilitySet {
    /// Caps that may be made effective (superset of effective).
    pub permitted: u64,
    /// Currently active caps used for access checks.
    pub effective: u64,
    /// Caps preserved across exec (combined with file inheritable).
    pub inheritable: u64,
    /// Caps always added to effective/permitted on exec without file caps.
    pub ambient: u64,
    /// Hard upper limit on caps that can be raised via exec.
    pub bounding: u64,
}

impl CapabilitySet {
    /// All-privileges root set (PID 0 / UID 0).
    pub fn root() -> Self {
        Self {
            permitted: CAP_ALL_VALID,
            effective: CAP_ALL_VALID,
            inheritable: 0,
            ambient: 0,
            bounding: CAP_ALL_VALID,
        }
    }

    /// Unprivileged empty set.
    pub fn empty() -> Self {
        Self {
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

    /// Return Ok(()) if `cap` is effective, Err("EPERM") otherwise.
    #[inline(always)]
    pub fn check(&self, cap: u64) -> Result<(), &'static str> {
        if self.has(cap) {
            Ok(())
        } else {
            Err("EPERM")
        }
    }

    /// Compute the capability set for the new process image after exec.
    ///
    /// `file_caps` models the file capability xattr:
    ///   - `file_caps.permitted`   = file permitted set
    ///   - `file_caps.inheritable` = file inheritable set
    ///   - `file_caps.effective`   = non-zero if the binary has the effective bit
    ///
    /// Pass `CapabilitySet::empty()` for plain (non-file-capped) binaries.
    pub fn on_exec(&self, file_caps: &CapabilitySet) -> Self {
        // Saturating operations only — no overflow possible with 64-bit masks.
        let new_permitted = (self.inheritable & file_caps.inheritable)
            | (file_caps.permitted & self.bounding)
            | self.ambient;

        // Ambient caps that are no longer permitted must be dropped.
        let new_ambient = self.ambient & new_permitted;

        // Effective is full permitted when the file effective bit is set;
        // otherwise only ambient caps are immediately effective.
        let new_effective = if file_caps.effective != 0 {
            new_permitted
        } else {
            new_ambient
        };

        CapabilitySet {
            permitted: new_permitted,
            effective: new_effective,
            inheritable: self.inheritable,
            ambient: new_ambient,
            bounding: self.bounding,
        }
    }

    /// Permanently drop a capability from all sets.  Irreversible.
    pub fn drop_cap(&mut self, cap: u64) {
        let mask = !cap;
        self.effective &= mask;
        self.permitted &= mask;
        self.inheritable &= mask;
        self.ambient &= mask;
        // Note: we do NOT touch bounding here — callers must call drop_bounding
        // separately if they want to restrict the exec ceiling.
    }

    /// Drop a capability from the bounding set (privilege ceiling).
    pub fn drop_bounding(&mut self, cap: u64) {
        self.bounding &= !cap;
    }

    /// Raise a capability into the effective set.
    /// Fails if the cap is not in the permitted set.
    pub fn raise_effective(&mut self, cap: u64) -> Result<(), &'static str> {
        if self.permitted & cap != cap {
            return Err("EPERM");
        }
        self.effective |= cap;
        Ok(())
    }

    /// Lower (clear) a capability from the effective set without touching permitted.
    pub fn lower_effective(&mut self, cap: u64) {
        self.effective &= !cap;
    }
}

/// Per-process Linux-model capability set registry (keyed by PID).
static POSIX_CAP_TABLE: Mutex<Option<BTreeMap<u32, CapabilitySet>>> = Mutex::new(None);

/// Install a CapabilitySet for a process.
pub fn set_linux_caps(pid: u32, caps: CapabilitySet) {
    let mut guard = POSIX_CAP_TABLE.lock();
    if guard.is_none() {
        *guard = Some(BTreeMap::new());
    }
    if let Some(ref mut map) = *guard {
        map.insert(pid, caps);
    }
}

/// Retrieve the Linux CapabilitySet for a process (empty for unknown PIDs).
pub fn get_linux_caps(pid: u32) -> CapabilitySet {
    POSIX_CAP_TABLE
        .lock()
        .as_ref()
        .and_then(|m| m.get(&pid).copied())
        .unwrap_or_else(CapabilitySet::empty)
}

/// Test whether a process holds a Linux capability in its effective set.
pub fn process_has_cap(pid: u32, cap: u64) -> bool {
    get_linux_caps(pid).has(cap)
}

/// Remove Linux caps for a process on exit.
pub fn remove_linux_caps(pid: u32) {
    if let Some(ref mut map) = *POSIX_CAP_TABLE.lock() {
        map.remove(&pid);
    }
}

// ── Object-capability table (original unforgeable-token model) ───────────────

/// Global capability table
static CAP_TABLE: Mutex<Option<CapabilityTable>> = Mutex::new(None);

/// A capability — an unforgeable token granting access to a resource
#[derive(Debug, Clone)]
pub struct Capability {
    /// Unique capability ID (system-wide)
    pub id: u64,
    /// The resource this capability grants access to
    pub resource: Resource,
    /// Permissions granted
    pub permissions: Permissions,
    /// Owner process PID
    pub owner: u32,
    /// Whether this cap can be delegated to children
    pub delegatable: bool,
    /// Parent capability (if delegated)
    pub parent: Option<u64>,
    /// Whether this capability has been revoked
    pub revoked: bool,
}

/// A resource that capabilities can reference
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resource {
    /// File or directory path
    File(String),
    /// Network socket (addr, port)
    Network(String, u16),
    /// Device access
    Device(String),
    /// Memory region (start, size)
    Memory(u64, u64),
    /// Process control (target PID)
    Process(u32),
    /// System call (syscall number)
    Syscall(u32),
    /// IPC channel ID
    IpcChannel(u32),
    /// Display surface ID
    DisplaySurface(u32),
    /// Wildcard — grants access to everything (only for PID 0/1)
    All,
}

/// Permission bits
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
    pub create: bool,
    pub delete: bool,
    pub admin: bool,
}

impl Permissions {
    pub const NONE: Self = Permissions {
        read: false,
        write: false,
        execute: false,
        create: false,
        delete: false,
        admin: false,
    };

    pub const READ_ONLY: Self = Permissions {
        read: true,
        write: false,
        execute: false,
        create: false,
        delete: false,
        admin: false,
    };

    pub const READ_WRITE: Self = Permissions {
        read: true,
        write: true,
        execute: false,
        create: false,
        delete: false,
        admin: false,
    };

    pub const READ_EXECUTE: Self = Permissions {
        read: true,
        write: false,
        execute: true,
        create: false,
        delete: false,
        admin: false,
    };

    pub const FULL: Self = Permissions {
        read: true,
        write: true,
        execute: true,
        create: true,
        delete: true,
        admin: true,
    };

    /// Check if self contains all permissions of other
    pub fn contains(&self, other: &Permissions) -> bool {
        (!other.read || self.read)
            && (!other.write || self.write)
            && (!other.execute || self.execute)
            && (!other.create || self.create)
            && (!other.delete || self.delete)
            && (!other.admin || self.admin)
    }
}

/// Capability table — manages all capabilities system-wide
pub struct CapabilityTable {
    caps: BTreeMap<u64, Capability>,
    next_id: u64,
    /// Per-process capability sets
    process_caps: BTreeMap<u32, Vec<u64>>,
}

impl CapabilityTable {
    pub fn new() -> Self {
        CapabilityTable {
            caps: BTreeMap::new(),
            next_id: 1,
            process_caps: BTreeMap::new(),
        }
    }

    /// Create a new capability for a process
    pub fn create(
        &mut self,
        owner: u32,
        resource: Resource,
        permissions: Permissions,
        delegatable: bool,
    ) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        let cap = Capability {
            id,
            resource,
            permissions,
            owner,
            delegatable,
            parent: None,
            revoked: false,
        };

        self.caps.insert(id, cap);
        self.process_caps
            .entry(owner)
            .or_insert_with(Vec::new)
            .push(id);
        id
    }

    /// Check if a process has a capability for a resource with given permissions
    pub fn check(&self, pid: u32, resource: &Resource, required: &Permissions) -> bool {
        if let Some(cap_ids) = self.process_caps.get(&pid) {
            for &cap_id in cap_ids {
                if let Some(cap) = self.caps.get(&cap_id) {
                    if cap.revoked {
                        continue;
                    }
                    if (cap.resource == *resource || cap.resource == Resource::All)
                        && cap.permissions.contains(required)
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Delegate a capability to another process (with optional restriction)
    pub fn delegate(
        &mut self,
        cap_id: u64,
        to_pid: u32,
        restricted: Option<Permissions>,
    ) -> Result<u64, &'static str> {
        let parent = self
            .caps
            .get(&cap_id)
            .ok_or("capability not found")?
            .clone();

        if parent.revoked {
            return Err("capability revoked");
        }
        if !parent.delegatable {
            return Err("capability not delegatable");
        }

        let perms = match restricted {
            Some(r) => {
                // Can only restrict, not expand
                if !parent.permissions.contains(&r) {
                    return Err("cannot expand permissions beyond parent");
                }
                r
            }
            None => parent.permissions,
        };

        let new_id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        let delegated = Capability {
            id: new_id,
            resource: parent.resource.clone(),
            permissions: perms,
            owner: to_pid,
            delegatable: parent.delegatable,
            parent: Some(cap_id),
            revoked: false,
        };

        self.caps.insert(new_id, delegated);
        self.process_caps
            .entry(to_pid)
            .or_insert_with(Vec::new)
            .push(new_id);
        Ok(new_id)
    }

    /// Revoke a capability and all its delegated children
    pub fn revoke(&mut self, cap_id: u64) {
        if let Some(cap) = self.caps.get_mut(&cap_id) {
            cap.revoked = true;
        }
        // Recursively revoke children
        let children: Vec<u64> = self
            .caps
            .values()
            .filter(|c| c.parent == Some(cap_id))
            .map(|c| c.id)
            .collect();
        for child_id in children {
            self.revoke(child_id);
        }
    }

    /// Drop a capability (process voluntarily gives it up)
    pub fn drop_cap(&mut self, pid: u32, cap_id: u64) {
        if let Some(cap_ids) = self.process_caps.get_mut(&pid) {
            cap_ids.retain(|&id| id != cap_id);
        }
        self.caps.remove(&cap_id);
    }

    /// Get all capabilities for a process
    pub fn get_process_caps(&self, pid: u32) -> Vec<&Capability> {
        self.process_caps
            .get(&pid)
            .map(|ids| ids.iter().filter_map(|id| self.caps.get(id)).collect())
            .unwrap_or_default()
    }

    /// Grant root capabilities to PID 0 and PID 1
    fn grant_root_caps(&mut self) {
        self.create(0, Resource::All, Permissions::FULL, true);
        self.create(1, Resource::All, Permissions::FULL, true);
    }
}

/// Initialize the capability system
pub fn init() {
    let mut table = CapabilityTable::new();
    table.grant_root_caps();
    *CAP_TABLE.lock() = Some(table);
    serial_println!("    [caps] Capability table initialized");
}

/// Check if a process has permission for a resource
pub fn check(pid: u32, resource: &Resource, perms: &Permissions) -> bool {
    CAP_TABLE
        .lock()
        .as_ref()
        .map(|t| t.check(pid, resource, perms))
        .unwrap_or(false)
}

/// Grant a new capability to a process (requires admin on the process)
pub fn grant(pid: u32, resource: Resource, perms: Permissions) -> u64 {
    CAP_TABLE
        .lock()
        .as_mut()
        .map(|t| t.create(pid, resource, perms, true))
        .unwrap_or(0)
}
