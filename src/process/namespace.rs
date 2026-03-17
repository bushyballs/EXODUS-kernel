/// Types of namespaces supported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NsType {
    Pid,
    Net,
    Mnt,
    Uts,
    Ipc,
    User,
    Cgroup,
}

/// Reference to a specific namespace instance.
pub struct NamespaceRef {
    pub ns_type: NsType,
    pub ns_id: u64,
}

/// Set of namespaces attached to a process.
pub struct NsSet {
    pub pid_ns: u64,
    pub net_ns: u64,
    pub mnt_ns: u64,
    pub uts_ns: u64,
    pub ipc_ns: u64,
    pub user_ns: u64,
}

/// ID used for the global init namespaces.
const INIT_NS_ID: u64 = 1;

/// Monotonically incrementing counter for fresh namespace IDs.
/// Starts at 2 so INIT_NS_ID (1) is never re-issued.
static NS_ID_COUNTER: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(2);

fn alloc_ns_id() -> u64 {
    NS_ID_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
}

impl NsSet {
    /// Default (init) namespace set — all namespaces share the init instance.
    pub fn init_ns() -> Self {
        NsSet {
            pid_ns: INIT_NS_ID,
            net_ns: INIT_NS_ID,
            mnt_ns: INIT_NS_ID,
            uts_ns: INIT_NS_ID,
            ipc_ns: INIT_NS_ID,
            user_ns: INIT_NS_ID,
        }
    }

    /// Clone this set, replacing the specified namespace type with a fresh one.
    pub fn unshare(&self, ns_type: NsType) -> Self {
        let new_id = alloc_ns_id();
        let mut ns = NsSet {
            pid_ns: self.pid_ns,
            net_ns: self.net_ns,
            mnt_ns: self.mnt_ns,
            uts_ns: self.uts_ns,
            ipc_ns: self.ipc_ns,
            user_ns: self.user_ns,
        };
        match ns_type {
            NsType::Pid => ns.pid_ns = new_id,
            NsType::Net => ns.net_ns = new_id,
            NsType::Mnt => ns.mnt_ns = new_id,
            NsType::Uts => ns.uts_ns = new_id,
            NsType::Ipc => ns.ipc_ns = new_id,
            NsType::User => ns.user_ns = new_id,
            NsType::Cgroup => { /* Cgroup NS not tracked in NsSet fields yet */ }
        }
        ns
    }
}

/// Initialize the namespace subsystem.
pub fn init() {
    // TODO: Create init namespaces
}
