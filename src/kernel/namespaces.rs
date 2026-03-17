/// Namespaces — process isolation for Genesis
///
/// Isolates kernel resources so different sets of processes see different
/// views of the system. Foundation for containers.
///
/// Types: PID, Mount, Net, User, IPC, UTS, Cgroup
///
/// Implementation: all storage is in fixed-size static tables. No heap.
/// Public API returns primitive types / writes into caller-supplied buffers.
///
/// Inspired by: Linux namespaces (kernel/nsproxy.c). All code is original.
/// Rules: no_std, no heap, no floats, no panics, saturating counters.
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Limits
// ---------------------------------------------------------------------------

const MAX_NS: usize = 64; // total namespace instances (all types)
const MAX_PIDS: usize = 256; // processes tracked
const MAX_MNT_NS: usize = 8; // mount namespace data slots
const MAX_NET_NS: usize = 8; // network namespace data slots
const MAX_PID_NS: usize = 8; // PID namespace data slots
const MAX_MOUNTS: usize = 16; // mounts per mount namespace
const MAX_NET_IFACE: usize = 8; // interfaces per net namespace
const MAX_NET_ROUTE: usize = 16; // routes per net namespace
const MAX_PID_MAP: usize = 32; // PID mappings per PID namespace
const MAX_NS_PIDS: usize = 16; // PIDs per namespace membership list

// ---------------------------------------------------------------------------
// NsType
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NsType {
    Pid = 0,
    Mount = 1,
    Net = 2,
    User = 3,
    Ipc = 4,
    Uts = 5,
    Cgroup = 6,
}

impl NsType {
    pub fn clone_flag(self) -> u32 {
        match self {
            NsType::Pid => 0x2000_0000,
            NsType::Mount => 0x0002_0000,
            NsType::Net => 0x4000_0000,
            NsType::User => 0x1000_0000,
            NsType::Ipc => 0x0800_0000,
            NsType::Uts => 0x0400_0000,
            NsType::Cgroup => 0x0200_0000,
        }
    }
}

const ALL_NS_TYPES: [NsType; 7] = [
    NsType::Pid,
    NsType::Mount,
    NsType::Net,
    NsType::User,
    NsType::Ipc,
    NsType::Uts,
    NsType::Cgroup,
];

// ---------------------------------------------------------------------------
// Per-process namespace proxy
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct NsProxy {
    pub pid_ns: u32,
    pub mnt_ns: u32,
    pub net_ns: u32,
    pub user_ns: u32,
    pub ipc_ns: u32,
    pub uts_ns: u32,
    pub cgroup_ns: u32,
}

impl NsProxy {
    pub const fn init_ns() -> Self {
        NsProxy {
            pid_ns: 0,
            mnt_ns: 0,
            net_ns: 0,
            user_ns: 0,
            ipc_ns: 0,
            uts_ns: 0,
            cgroup_ns: 0,
        }
    }
    pub fn get(&self, t: NsType) -> u32 {
        match t {
            NsType::Pid => self.pid_ns,
            NsType::Mount => self.mnt_ns,
            NsType::Net => self.net_ns,
            NsType::User => self.user_ns,
            NsType::Ipc => self.ipc_ns,
            NsType::Uts => self.uts_ns,
            NsType::Cgroup => self.cgroup_ns,
        }
    }
    pub fn set(&mut self, t: NsType, id: u32) {
        match t {
            NsType::Pid => self.pid_ns = id,
            NsType::Mount => self.mnt_ns = id,
            NsType::Net => self.net_ns = id,
            NsType::User => self.user_ns = id,
            NsType::Ipc => self.ipc_ns = id,
            NsType::Uts => self.uts_ns = id,
            NsType::Cgroup => self.cgroup_ns = id,
        }
    }
}

// ---------------------------------------------------------------------------
// Core namespace entry
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct Namespace {
    pub id: u32,
    pub ns_type: NsType,
    pub parent: u32,
    pub refcount: u32,
    pub active: bool,
    pub pid_count: u8,
    pub pids: [u32; MAX_NS_PIDS],
    // UTS fields (used when ns_type == Uts)
    pub hostname: [u8; 64],
    pub hostname_len: u8,
    pub domain: [u8; 64],
    pub domain_len: u8,
    // Data table index (for Pid/Mnt/Net namespaces)
    pub data_idx: u8, // u8::MAX = not allocated
    // User namespace UID/GID maps (up to 4 entries each)
    pub uid_map: [(u32, u32, u32); 4], // (ns_uid, host_uid, count)
    pub uid_map_count: u8,
    pub gid_map: [(u32, u32, u32); 4],
    pub gid_map_count: u8,
    // PID namespace: next_ns_pid (used when ns_type == Pid without separate table)
    pub next_ns_pid: u32,
}

impl Namespace {
    pub const fn empty() -> Self {
        Namespace {
            id: 0,
            ns_type: NsType::Uts,
            parent: 0,
            refcount: 0,
            active: false,
            pid_count: 0,
            pids: [0u32; MAX_NS_PIDS],
            hostname: [0u8; 64],
            hostname_len: 0,
            domain: [0u8; 64],
            domain_len: 0,
            data_idx: 255,
            uid_map: [(0, 0, 0); 4],
            uid_map_count: 0,
            gid_map: [(0, 0, 0); 4],
            gid_map_count: 0,
            next_ns_pid: 1,
        }
    }
}

// ---------------------------------------------------------------------------
// Mount namespace data table
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct MountEntry {
    pub source: [u8; 24],
    pub source_len: u8,
    pub target: [u8; 24],
    pub target_len: u8,
    pub fs_type: [u8; 12],
    pub fs_len: u8,
    pub flags: u32,
    pub active: bool,
}

impl MountEntry {
    pub const fn empty() -> Self {
        MountEntry {
            source: [0u8; 24],
            source_len: 0,
            target: [0u8; 24],
            target_len: 0,
            fs_type: [0u8; 12],
            fs_len: 0,
            flags: 0,
            active: false,
        }
    }
}

#[derive(Copy, Clone)]
pub struct MountNs {
    pub entries: [MountEntry; MAX_MOUNTS],
    pub count: u8,
    pub used: bool,
}

impl MountNs {
    pub const fn empty() -> Self {
        const ME: MountEntry = MountEntry::empty();
        MountNs {
            entries: [ME; MAX_MOUNTS],
            count: 0,
            used: false,
        }
    }

    fn mount_entry(&mut self, src: &[u8], tgt: &[u8], fst: &[u8], flags: u32) -> bool {
        if self.count as usize >= MAX_MOUNTS {
            return false;
        }
        let i = self.count as usize;
        copy_bytes(&mut self.entries[i].source, src);
        self.entries[i].source_len = src.len().min(23) as u8;
        copy_bytes(&mut self.entries[i].target, tgt);
        self.entries[i].target_len = tgt.len().min(23) as u8;
        copy_bytes(&mut self.entries[i].fs_type, fst);
        self.entries[i].fs_len = fst.len().min(11) as u8;
        self.entries[i].flags = flags;
        self.entries[i].active = true;
        self.count = self.count.saturating_add(1);
        true
    }

    fn umount(&mut self, tgt: &[u8]) -> bool {
        let tlen = tgt.len().min(24);
        let mut i = 0usize;
        while i < self.count as usize {
            if self.entries[i].active && self.entries[i].target_len as usize == tlen {
                let mut eq = true;
                let mut k = 0usize;
                while k < tlen {
                    if self.entries[i].target[k] != tgt[k] {
                        eq = false;
                        break;
                    }
                    k = k.saturating_add(1);
                }
                if eq {
                    self.entries[i].active = false;
                    return true;
                }
            }
            i = i.saturating_add(1);
        }
        false
    }
}

// ---------------------------------------------------------------------------
// Network namespace data table
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct NetIface {
    pub name: [u8; 12],
    pub name_len: u8,
    pub index: u32,
    pub ipv4: [u8; 4],
    pub prefix: u8,
    pub mac: [u8; 6],
    pub up: bool,
    pub mtu: u32,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub active: bool,
}

impl NetIface {
    pub const fn empty() -> Self {
        NetIface {
            name: [0u8; 12],
            name_len: 0,
            index: 0,
            ipv4: [0u8; 4],
            prefix: 0,
            mac: [0u8; 6],
            up: false,
            mtu: 1500,
            rx_bytes: 0,
            tx_bytes: 0,
            active: false,
        }
    }
}

#[derive(Copy, Clone)]
pub struct NetRoute {
    pub dst: [u8; 4],
    pub dst_pfx: u8,
    pub gateway: [u8; 4],
    pub iface: [u8; 12],
    pub iface_len: u8,
    pub metric: u32,
    pub active: bool,
}

impl NetRoute {
    pub const fn empty() -> Self {
        NetRoute {
            dst: [0u8; 4],
            dst_pfx: 0,
            gateway: [0u8; 4],
            iface: [0u8; 12],
            iface_len: 0,
            metric: 0,
            active: false,
        }
    }
}

#[derive(Copy, Clone)]
pub struct NetNs {
    pub ifaces: [NetIface; MAX_NET_IFACE],
    pub iface_count: u8,
    pub routes: [NetRoute; MAX_NET_ROUTE],
    pub route_count: u8,
    pub next_idx: u32,
    pub used: bool,
}

impl NetNs {
    pub const fn empty() -> Self {
        const NI: NetIface = NetIface::empty();
        const NR: NetRoute = NetRoute::empty();
        NetNs {
            ifaces: [NI; MAX_NET_IFACE],
            iface_count: 0,
            routes: [NR; MAX_NET_ROUTE],
            route_count: 0,
            next_idx: 1,
            used: false,
        }
    }

    fn add_loopback(&mut self) {
        self.add_iface(b"lo", [127, 0, 0, 1], 8, [0u8; 6], true, 65536);
        self.add_route([127, 0, 0, 0], 8, [0, 0, 0, 0], b"lo", 0);
    }

    fn add_iface(
        &mut self,
        name: &[u8],
        ipv4: [u8; 4],
        prefix: u8,
        mac: [u8; 6],
        up: bool,
        mtu: u32,
    ) -> bool {
        if self.iface_count as usize >= MAX_NET_IFACE {
            return false;
        }
        let i = self.iface_count as usize;
        copy_bytes(&mut self.ifaces[i].name, name);
        self.ifaces[i].name_len = name.len().min(11) as u8;
        self.ifaces[i].index = self.next_idx;
        self.next_idx = self.next_idx.saturating_add(1);
        self.ifaces[i].ipv4 = ipv4;
        self.ifaces[i].prefix = prefix;
        self.ifaces[i].mac = mac;
        self.ifaces[i].up = up;
        self.ifaces[i].mtu = mtu;
        self.ifaces[i].active = true;
        self.iface_count = self.iface_count.saturating_add(1);
        true
    }

    fn set_iface_up(&mut self, name: &[u8], up: bool) -> bool {
        let nlen = name.len().min(12);
        let mut i = 0usize;
        while i < self.iface_count as usize {
            if self.ifaces[i].active && self.ifaces[i].name_len as usize == nlen {
                let mut eq = true;
                let mut k = 0usize;
                while k < nlen {
                    if self.ifaces[i].name[k] != name[k] {
                        eq = false;
                        break;
                    }
                    k = k.saturating_add(1);
                }
                if eq {
                    self.ifaces[i].up = up;
                    return true;
                }
            }
            i = i.saturating_add(1);
        }
        false
    }

    fn add_route(&mut self, dst: [u8; 4], pfx: u8, gw: [u8; 4], iface: &[u8], metric: u32) -> bool {
        if self.route_count as usize >= MAX_NET_ROUTE {
            return false;
        }
        let i = self.route_count as usize;
        self.routes[i].dst = dst;
        self.routes[i].dst_pfx = pfx;
        self.routes[i].gateway = gw;
        copy_bytes(&mut self.routes[i].iface, iface);
        self.routes[i].iface_len = iface.len().min(11) as u8;
        self.routes[i].metric = metric;
        self.routes[i].active = true;
        self.route_count = self.route_count.saturating_add(1);
        true
    }
}

// ---------------------------------------------------------------------------
// PID namespace data table
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct PidNs {
    pub pid_map: [(u32, u32); MAX_PID_MAP], // (ns_pid, global_pid)
    pub map_count: u8,
    pub next_pid: u32,
    pub used: bool,
}

impl PidNs {
    pub const fn empty() -> Self {
        PidNs {
            pid_map: [(0, 0); MAX_PID_MAP],
            map_count: 0,
            next_pid: 1,
            used: false,
        }
    }

    fn add_mapping(&mut self, global_pid: u32) -> u32 {
        if self.map_count as usize >= MAX_PID_MAP {
            return 0;
        }
        let ns_pid = self.next_pid;
        self.next_pid = self.next_pid.saturating_add(1);
        self.pid_map[self.map_count as usize] = (ns_pid, global_pid);
        self.map_count = self.map_count.saturating_add(1);
        ns_pid
    }

    fn global_to_ns(&self, global: u32) -> Option<u32> {
        let mut i = 0usize;
        while i < self.map_count as usize {
            if self.pid_map[i].1 == global {
                return Some(self.pid_map[i].0);
            }
            i = i.saturating_add(1);
        }
        None
    }

    fn ns_to_global(&self, ns_pid: u32) -> Option<u32> {
        let mut i = 0usize;
        while i < self.map_count as usize {
            if self.pid_map[i].0 == ns_pid {
                return Some(self.pid_map[i].1);
            }
            i = i.saturating_add(1);
        }
        None
    }

    fn remove_global(&mut self, global: u32) {
        let mut i = 0usize;
        while i < self.map_count as usize {
            if self.pid_map[i].1 == global {
                let last = self.map_count as usize - 1;
                self.pid_map[i] = self.pid_map[last];
                self.pid_map[last] = (0, 0);
                self.map_count = self.map_count.saturating_sub(1);
                return;
            }
            i = i.saturating_add(1);
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn copy_bytes<const N: usize>(dst: &mut [u8; N], src: &[u8]) {
    let len = src.len().min(N);
    let mut i = 0usize;
    while i < len {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
}

// ---------------------------------------------------------------------------
// Global tables
// ---------------------------------------------------------------------------

const EMPTY_NS: Namespace = Namespace::empty();
const EMPTY_MNT_NS: MountNs = MountNs::empty();
const EMPTY_NET_NS: NetNs = NetNs::empty();
const EMPTY_PID_NS: PidNs = PidNs::empty();
const EMPTY_PROXY: NsProxy = NsProxy::init_ns();

static NS_TABLE: Mutex<[Namespace; MAX_NS]> = Mutex::new([EMPTY_NS; MAX_NS]);
static MNT_DATA: Mutex<[MountNs; MAX_MNT_NS]> = Mutex::new([EMPTY_MNT_NS; MAX_MNT_NS]);
static NET_DATA: Mutex<[NetNs; MAX_NET_NS]> = Mutex::new([EMPTY_NET_NS; MAX_NET_NS]);
static PID_DATA: Mutex<[PidNs; MAX_PID_NS]> = Mutex::new([EMPTY_PID_NS; MAX_PID_NS]);
static PROXIES: Mutex<[NsProxy; MAX_PIDS]> = Mutex::new([EMPTY_PROXY; MAX_PIDS]);
static NS_NEXT_ID: AtomicU32 = AtomicU32::new(1);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn alloc_mnt_slot(data: &mut [MountNs; MAX_MNT_NS]) -> Option<u8> {
    for i in 0..MAX_MNT_NS {
        if !data[i].used {
            data[i].used = true;
            return Some(i as u8);
        }
    }
    None
}
fn alloc_net_slot(data: &mut [NetNs; MAX_NET_NS]) -> Option<u8> {
    for i in 0..MAX_NET_NS {
        if !data[i].used {
            data[i].used = true;
            return Some(i as u8);
        }
    }
    None
}
fn alloc_pid_slot(data: &mut [PidNs; MAX_PID_NS]) -> Option<u8> {
    for i in 0..MAX_PID_NS {
        if !data[i].used {
            data[i].used = true;
            return Some(i as u8);
        }
    }
    None
}

fn find_ns<'a>(table: &'a [Namespace; MAX_NS], id: u32, ns_type: NsType) -> Option<usize> {
    let mut i = 0usize;
    while i < MAX_NS {
        if table[i].active && table[i].id == id && table[i].ns_type == ns_type {
            return Some(i);
        }
        i = i.saturating_add(1);
    }
    None
}

fn alloc_ns(table: &mut [Namespace; MAX_NS]) -> Option<usize> {
    let mut i = 0usize;
    while i < MAX_NS {
        if !table[i].active {
            return Some(i);
        }
        i = i.saturating_add(1);
    }
    None
}

fn ns_add_pid(ns: &mut Namespace, pid: u32) {
    if ns.pid_count as usize >= MAX_NS_PIDS {
        return;
    }
    ns.pids[ns.pid_count as usize] = pid;
    ns.pid_count = ns.pid_count.saturating_add(1);
}

fn ns_remove_pid(ns: &mut Namespace, pid: u32) {
    let mut i = 0usize;
    while i < ns.pid_count as usize {
        if ns.pids[i] == pid {
            let last = ns.pid_count as usize - 1;
            ns.pids[i] = ns.pids[last];
            ns.pids[last] = 0;
            ns.pid_count = ns.pid_count.saturating_sub(1);
            return;
        }
        i = i.saturating_add(1);
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    let mut nst = NS_TABLE.lock();
    let mut mnt = MNT_DATA.lock();
    let mut net = NET_DATA.lock();
    let mut pid = PID_DATA.lock();

    // Create init namespaces (id=0) for each type
    let mut slot = 0usize;
    let mut t = 0usize;
    while t < ALL_NS_TYPES.len() && slot < MAX_NS {
        let ns_type = ALL_NS_TYPES[t];
        nst[slot] = Namespace::empty();
        nst[slot].id = 0;
        nst[slot].ns_type = ns_type;
        nst[slot].parent = 0;
        nst[slot].refcount = 1;
        nst[slot].active = true;
        // Set genesis as default hostname for UTS init ns
        if ns_type == NsType::Uts {
            let h = b"genesis";
            copy_bytes(&mut nst[slot].hostname, h);
            nst[slot].hostname_len = h.len() as u8;
        }
        // Allocate type-specific data for init namespaces
        match ns_type {
            NsType::Mount => {
                if let Some(idx) = alloc_mnt_slot(&mut mnt) {
                    nst[slot].data_idx = idx;
                    // Default mounts
                    mnt[idx as usize].mount_entry(b"rootfs", b"/", b"rootfs", 0);
                    mnt[idx as usize].mount_entry(b"proc", b"/proc", b"proc", 0);
                    mnt[idx as usize].mount_entry(b"sysfs", b"/sys", b"sysfs", 0);
                    mnt[idx as usize].mount_entry(b"devtmpfs", b"/dev", b"devtmpfs", 0);
                    mnt[idx as usize].mount_entry(b"tmpfs", b"/tmp", b"tmpfs", 0);
                }
            }
            NsType::Net => {
                if let Some(idx) = alloc_net_slot(&mut net) {
                    nst[slot].data_idx = idx;
                    net[idx as usize].add_loopback();
                    net[idx as usize].add_iface(
                        b"eth0",
                        [10, 0, 0, 1],
                        24,
                        [0x02, 0x00, 0x00, 0x00, 0x00, 0x01],
                        true,
                        1500,
                    );
                    net[idx as usize].add_route([0, 0, 0, 0], 0, [10, 0, 0, 254], b"eth0", 100);
                }
            }
            NsType::Pid => {
                if let Some(idx) = alloc_pid_slot(&mut pid) {
                    nst[slot].data_idx = idx;
                }
            }
            _ => {}
        }
        slot = slot.saturating_add(1);
        t = t.saturating_add(1);
    }

    crate::serial_println!(
        "  [namespaces] process namespaces initialized (PID, Mount, Net, User, IPC, UTS, Cgroup)"
    );
}

// ---------------------------------------------------------------------------
// Internal create
// ---------------------------------------------------------------------------

fn create_ns(ns_type: NsType, parent_id: u32) -> u32 {
    let id = NS_NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let mut nst = NS_TABLE.lock();
    let slot = match alloc_ns(&mut nst) {
        Some(s) => s,
        None => return 0,
    };

    // Increment parent refcount
    if let Some(pi) = find_ns(&nst, parent_id, ns_type) {
        nst[pi].refcount = nst[pi].refcount.saturating_add(1);
    }

    nst[slot] = Namespace::empty();
    nst[slot].id = id;
    nst[slot].ns_type = ns_type;
    nst[slot].parent = parent_id;
    nst[slot].refcount = 1;
    nst[slot].active = true;

    // Inherit UTS hostname from parent
    if ns_type == NsType::Uts {
        if let Some(pi) = find_ns(&nst, parent_id, NsType::Uts) {
            nst[slot].hostname = nst[pi].hostname;
            nst[slot].hostname_len = nst[pi].hostname_len;
            nst[slot].domain = nst[pi].domain;
            nst[slot].domain_len = nst[pi].domain_len;
        }
    }

    // Allocate type-specific data
    drop(nst); // must drop before acquiring other locks

    match ns_type {
        NsType::Mount => {
            let mut mnt = MNT_DATA.lock();
            if let Some(idx) = alloc_mnt_slot(&mut mnt) {
                // Clone parent mount table
                if let Some(pi) = find_ns(&NS_TABLE.lock(), parent_id, NsType::Mount) {
                    let parent_idx = NS_TABLE.lock()[pi].data_idx as usize;
                    if parent_idx < MAX_MNT_NS {
                        mnt[idx as usize].entries = mnt[parent_idx].entries;
                        mnt[idx as usize].count = mnt[parent_idx].count;
                    }
                }
                let mut nst2 = NS_TABLE.lock();
                if let Some(si) = find_ns(&nst2, id, ns_type) {
                    nst2[si].data_idx = idx;
                }
            }
        }
        NsType::Net => {
            let mut net = NET_DATA.lock();
            if let Some(idx) = alloc_net_slot(&mut net) {
                net[idx as usize].add_loopback();
                let mut nst2 = NS_TABLE.lock();
                if let Some(si) = find_ns(&nst2, id, ns_type) {
                    nst2[si].data_idx = idx;
                }
            }
        }
        NsType::Pid => {
            let mut pid = PID_DATA.lock();
            if let Some(idx) = alloc_pid_slot(&mut pid) {
                let mut nst2 = NS_TABLE.lock();
                if let Some(si) = find_ns(&nst2, id, ns_type) {
                    nst2[si].data_idx = idx;
                }
            }
        }
        _ => {}
    }

    id
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new namespace of given type for pid. Returns new namespace id.
pub fn unshare(pid: u32, ns_type: NsType) -> u32 {
    if pid as usize >= MAX_PIDS {
        return 0;
    }
    let parent_id = PROXIES.lock()[pid as usize].get(ns_type);
    let new_id = create_ns(ns_type, parent_id);
    if new_id == 0 {
        return 0;
    }
    PROXIES.lock()[pid as usize].set(ns_type, new_id);
    {
        let mut nst = NS_TABLE.lock();
        if let Some(si) = find_ns(&nst, new_id, ns_type) {
            ns_add_pid(&mut nst[si], pid);
            // For PID ns, create mapping
            if ns_type == NsType::Pid {
                let didx = nst[si].data_idx as usize;
                drop(nst);
                if didx < MAX_PID_NS {
                    PID_DATA.lock()[didx].add_mapping(pid);
                }
            }
        }
    }
    new_id
}

/// Unshare multiple namespace types via clone flags.
pub fn unshare_flags(pid: u32, clone_flags: u32) {
    for &t in &ALL_NS_TYPES {
        if clone_flags & t.clone_flag() != 0 {
            unshare(pid, t);
        }
    }
}

/// Move a process into an existing namespace.
pub fn setns(pid: u32, ns_id: u32, ns_type: NsType) -> bool {
    if pid as usize >= MAX_PIDS {
        return false;
    }
    let mut nst = NS_TABLE.lock();
    if find_ns(&nst, ns_id, ns_type).is_none() {
        return false;
    }
    // Remove from old
    let old_id = PROXIES.lock()[pid as usize].get(ns_type);
    if let Some(oi) = find_ns(&nst, old_id, ns_type) {
        ns_remove_pid(&mut nst[oi], pid);
        nst[oi].refcount = nst[oi].refcount.saturating_sub(1);
    }
    // Add to new
    if let Some(ni) = find_ns(&nst, ns_id, ns_type) {
        ns_add_pid(&mut nst[ni], pid);
        nst[ni].refcount = nst[ni].refcount.saturating_add(1);
    }
    drop(nst);
    PROXIES.lock()[pid as usize].set(ns_type, ns_id);
    // PID mapping
    if ns_type == NsType::Pid {
        let nst2 = NS_TABLE.lock();
        if let Some(ni) = find_ns(&nst2, ns_id, ns_type) {
            let didx = nst2[ni].data_idx as usize;
            drop(nst2);
            if didx < MAX_PID_NS {
                PID_DATA.lock()[didx].add_mapping(pid);
            }
        }
    }
    true
}

/// Copy namespace proxy from parent to child (on fork).
pub fn fork_proxy(parent_pid: u32, child_pid: u32) {
    if parent_pid as usize >= MAX_PIDS || child_pid as usize >= MAX_PIDS {
        return;
    }
    let parent_proxy = PROXIES.lock()[parent_pid as usize];
    PROXIES.lock()[child_pid as usize] = parent_proxy;
    // Bump refcounts and add child to each namespace's pid list
    let mut nst = NS_TABLE.lock();
    for &t in &ALL_NS_TYPES {
        let ns_id = parent_proxy.get(t);
        if let Some(ni) = find_ns(&nst, ns_id, t) {
            nst[ni].refcount = nst[ni].refcount.saturating_add(1);
            ns_add_pid(&mut nst[ni], child_pid);
        }
    }
    // PID mapping for child
    let pid_ns_id = parent_proxy.pid_ns;
    if let Some(ni) = find_ns(&nst, pid_ns_id, NsType::Pid) {
        let didx = nst[ni].data_idx as usize;
        drop(nst);
        if didx < MAX_PID_NS {
            PID_DATA.lock()[didx].add_mapping(child_pid);
        }
    }
}

/// Clean up all namespace references for an exiting process.
pub fn process_exit(pid: u32) {
    if pid as usize >= MAX_PIDS {
        return;
    }
    let proxy = PROXIES.lock()[pid as usize];
    let mut nst = NS_TABLE.lock();
    for &t in &ALL_NS_TYPES {
        let ns_id = proxy.get(t);
        if let Some(ni) = find_ns(&nst, ns_id, t) {
            ns_remove_pid(&mut nst[ni], pid);
            nst[ni].refcount = nst[ni].refcount.saturating_sub(1);
        }
    }
    // Remove PID mapping
    let pid_ns_id = proxy.pid_ns;
    if let Some(ni) = find_ns(&nst, pid_ns_id, NsType::Pid) {
        let didx = nst[ni].data_idx as usize;
        drop(nst);
        if didx < MAX_PID_NS {
            PID_DATA.lock()[didx].remove_global(pid);
        }
    }
    // GC non-init namespaces with zero refcount
    // (done lazily — they stay until another operation touches them)
    PROXIES.lock()[pid as usize] = NsProxy::init_ns();
}

/// Get the namespace id a process belongs to for a given type.
pub fn ns_id_for(pid: u32, ns_type: NsType) -> u32 {
    if pid as usize >= MAX_PIDS {
        return 0;
    }
    PROXIES.lock()[pid as usize].get(ns_type)
}

// --- UTS ---
pub fn set_hostname(pid: u32, name: &[u8]) {
    if pid as usize >= MAX_PIDS {
        return;
    }
    let uts_id = PROXIES.lock()[pid as usize].uts_ns;
    let mut nst = NS_TABLE.lock();
    if let Some(ni) = find_ns(&nst, uts_id, NsType::Uts) {
        copy_bytes(&mut nst[ni].hostname, name);
        nst[ni].hostname_len = name.len().min(63) as u8;
    }
}

pub fn get_hostname(pid: u32, out: &mut [u8; 64]) -> u8 {
    if pid as usize >= MAX_PIDS {
        return 0;
    }
    let uts_id = PROXIES.lock()[pid as usize].uts_ns;
    let nst = NS_TABLE.lock();
    if let Some(ni) = find_ns(&nst, uts_id, NsType::Uts) {
        let len = nst[ni].hostname_len as usize;
        let mut i = 0usize;
        while i < len {
            out[i] = nst[ni].hostname[i];
            i = i.saturating_add(1);
        }
        return len as u8;
    }
    0
}

pub fn set_domainname(pid: u32, name: &[u8]) {
    if pid as usize >= MAX_PIDS {
        return;
    }
    let uts_id = PROXIES.lock()[pid as usize].uts_ns;
    let mut nst = NS_TABLE.lock();
    if let Some(ni) = find_ns(&nst, uts_id, NsType::Uts) {
        copy_bytes(&mut nst[ni].domain, name);
        nst[ni].domain_len = name.len().min(63) as u8;
    }
}

pub fn get_domainname(pid: u32, out: &mut [u8; 64]) -> u8 {
    if pid as usize >= MAX_PIDS {
        return 0;
    }
    let uts_id = PROXIES.lock()[pid as usize].uts_ns;
    let nst = NS_TABLE.lock();
    if let Some(ni) = find_ns(&nst, uts_id, NsType::Uts) {
        let len = nst[ni].domain_len as usize;
        let mut i = 0usize;
        while i < len {
            out[i] = nst[ni].domain[i];
            i = i.saturating_add(1);
        }
        return len as u8;
    }
    0
}

// --- PID ---
pub fn global_to_ns_pid(global_pid: u32, pid_ns_id: u32) -> Option<u32> {
    if pid_ns_id == 0 {
        return Some(global_pid);
    }
    let nst = NS_TABLE.lock();
    let ni = find_ns(&nst, pid_ns_id, NsType::Pid)?;
    let didx = nst[ni].data_idx as usize;
    drop(nst);
    if didx >= MAX_PID_NS {
        return None;
    }
    PID_DATA.lock()[didx].global_to_ns(global_pid)
}

pub fn ns_to_global_pid(ns_pid: u32, pid_ns_id: u32) -> Option<u32> {
    if pid_ns_id == 0 {
        return Some(ns_pid);
    }
    let nst = NS_TABLE.lock();
    let ni = find_ns(&nst, pid_ns_id, NsType::Pid)?;
    let didx = nst[ni].data_idx as usize;
    drop(nst);
    if didx >= MAX_PID_NS {
        return None;
    }
    PID_DATA.lock()[didx].ns_to_global(ns_pid)
}

pub fn pid_ns_init(pid_ns_id: u32) -> Option<u32> {
    ns_to_global_pid(1, pid_ns_id)
}

// --- User ---
pub fn set_uid_map(ns_id: u32, ns_uid: u32, host_uid: u32, count: u32) {
    let mut nst = NS_TABLE.lock();
    if let Some(ni) = find_ns(&nst, ns_id, NsType::User) {
        let c = nst[ni].uid_map_count as usize;
        if c < 4 {
            nst[ni].uid_map[c] = (ns_uid, host_uid, count);
            nst[ni].uid_map_count += 1;
        }
    }
}

pub fn set_gid_map(ns_id: u32, ns_gid: u32, host_gid: u32, count: u32) {
    let mut nst = NS_TABLE.lock();
    if let Some(ni) = find_ns(&nst, ns_id, NsType::User) {
        let c = nst[ni].gid_map_count as usize;
        if c < 4 {
            nst[ni].gid_map[c] = (ns_gid, host_gid, count);
            nst[ni].gid_map_count += 1;
        }
    }
}

pub fn ns_uid_to_host(ns_id: u32, ns_uid: u32) -> Option<u32> {
    let nst = NS_TABLE.lock();
    let ni = find_ns(&nst, ns_id, NsType::User)?;
    let mut i = 0usize;
    while i < nst[ni].uid_map_count as usize {
        let (mu, hu, cnt) = nst[ni].uid_map[i];
        if ns_uid >= mu && ns_uid < mu.saturating_add(cnt) {
            return Some(hu + (ns_uid - mu));
        }
        i = i.saturating_add(1);
    }
    None
}

pub fn host_uid_to_ns(ns_id: u32, host_uid: u32) -> Option<u32> {
    let nst = NS_TABLE.lock();
    let ni = find_ns(&nst, ns_id, NsType::User)?;
    let mut i = 0usize;
    while i < nst[ni].uid_map_count as usize {
        let (mu, hu, cnt) = nst[ni].uid_map[i];
        if host_uid >= hu && host_uid < hu.saturating_add(cnt) {
            return Some(mu + (host_uid - hu));
        }
        i = i.saturating_add(1);
    }
    None
}

// --- Mount ---
pub fn mount(pid: u32, src: &[u8], tgt: &[u8], fst: &[u8], flags: u32) -> bool {
    if pid as usize >= MAX_PIDS {
        return false;
    }
    let mnt_id = PROXIES.lock()[pid as usize].mnt_ns;
    let nst = NS_TABLE.lock();
    if let Some(ni) = find_ns(&nst, mnt_id, NsType::Mount) {
        let didx = nst[ni].data_idx as usize;
        drop(nst);
        if didx < MAX_MNT_NS {
            return MNT_DATA.lock()[didx].mount_entry(src, tgt, fst, flags);
        }
    }
    false
}

pub fn umount(pid: u32, target: &[u8]) -> bool {
    if pid as usize >= MAX_PIDS {
        return false;
    }
    let mnt_id = PROXIES.lock()[pid as usize].mnt_ns;
    let nst = NS_TABLE.lock();
    if let Some(ni) = find_ns(&nst, mnt_id, NsType::Mount) {
        let didx = nst[ni].data_idx as usize;
        drop(nst);
        if didx < MAX_MNT_NS {
            return MNT_DATA.lock()[didx].umount(target);
        }
    }
    false
}

// --- Net ---
pub fn net_add_iface(pid: u32, name: &[u8], ipv4: [u8; 4], prefix: u8, mac: [u8; 6]) -> bool {
    if pid as usize >= MAX_PIDS {
        return false;
    }
    let net_id = PROXIES.lock()[pid as usize].net_ns;
    let nst = NS_TABLE.lock();
    if let Some(ni) = find_ns(&nst, net_id, NsType::Net) {
        let didx = nst[ni].data_idx as usize;
        drop(nst);
        if didx < MAX_NET_NS {
            return NET_DATA.lock()[didx].add_iface(name, ipv4, prefix, mac, false, 1500);
        }
    }
    false
}

pub fn net_set_iface_up(pid: u32, name: &[u8], up: bool) -> bool {
    if pid as usize >= MAX_PIDS {
        return false;
    }
    let net_id = PROXIES.lock()[pid as usize].net_ns;
    let nst = NS_TABLE.lock();
    if let Some(ni) = find_ns(&nst, net_id, NsType::Net) {
        let didx = nst[ni].data_idx as usize;
        drop(nst);
        if didx < MAX_NET_NS {
            return NET_DATA.lock()[didx].set_iface_up(name, up);
        }
    }
    false
}

pub fn net_add_route(
    pid: u32,
    dst: [u8; 4],
    prefix: u8,
    gw: [u8; 4],
    iface: &[u8],
    metric: u32,
) -> bool {
    if pid as usize >= MAX_PIDS {
        return false;
    }
    let net_id = PROXIES.lock()[pid as usize].net_ns;
    let nst = NS_TABLE.lock();
    if let Some(ni) = find_ns(&nst, net_id, NsType::Net) {
        let didx = nst[ni].data_idx as usize;
        drop(nst);
        if didx < MAX_NET_NS {
            return NET_DATA.lock()[didx].add_route(dst, prefix, gw, iface, metric);
        }
    }
    false
}
