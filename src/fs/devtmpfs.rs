/// devtmpfs — kernel device filesystem
///
/// Automatically populates /dev with device nodes as drivers register.
///
/// Rules: no_std, no heap, no floats, no panics, saturating counters.
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, PartialEq)]
pub enum DevNodeType {
    CharDevice,
    BlockDevice,
    Symlink,
}

impl DevNodeType {
    const fn default() -> Self {
        DevNodeType::CharDevice
    }
}

#[derive(Copy, Clone)]
pub struct DevNode {
    pub name: [u8; 64],
    pub name_len: u8,
    pub node_type: DevNodeType,
    pub major: u32,
    pub minor: u32,
    pub mode: u16,
    pub uid: u32,
    pub gid: u32,
    pub active: bool,
}

impl DevNode {
    pub const fn empty() -> Self {
        DevNode {
            name: [0u8; 64],
            name_len: 0,
            node_type: DevNodeType::CharDevice,
            major: 0,
            minor: 0,
            mode: 0o600,
            uid: 0,
            gid: 0,
            active: false,
        }
    }
}

const EMPTY_NODE: DevNode = DevNode::empty();
static DEVTMPFS_NODES: Mutex<[DevNode; 256]> = Mutex::new([EMPTY_NODE; 256]);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn name_matches(a: &[u8; 64], alen: u8, b: &[u8]) -> bool {
    let alen = alen as usize;
    if alen != b.len() {
        return false;
    }
    let mut i = 0usize;
    while i < alen {
        if a[i] != b[i] {
            return false;
        }
        i = i.saturating_add(1);
    }
    true
}

fn copy_name(dst: &mut [u8; 64], src: &[u8]) -> u8 {
    let len = src.len().min(63);
    let mut i = 0usize;
    while i < len {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
    len as u8
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn devtmpfs_mknod(name: &[u8], node_type: DevNodeType, major: u32, minor: u32) -> bool {
    let mut nodes = DEVTMPFS_NODES.lock();
    let mut i = 0usize;
    while i < 256 {
        if !nodes[i].active {
            nodes[i] = DevNode::empty();
            nodes[i].name_len = copy_name(&mut nodes[i].name, name);
            nodes[i].node_type = node_type;
            nodes[i].major = major;
            nodes[i].minor = minor;
            nodes[i].mode = 0o600;
            nodes[i].active = true;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn devtmpfs_unlink(name: &[u8]) -> bool {
    let mut nodes = DEVTMPFS_NODES.lock();
    let mut i = 0usize;
    while i < 256 {
        if nodes[i].active && name_matches(&nodes[i].name, nodes[i].name_len, name) {
            nodes[i].active = false;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn devtmpfs_lookup(name: &[u8]) -> Option<DevNode> {
    let nodes = DEVTMPFS_NODES.lock();
    let mut i = 0usize;
    while i < 256 {
        if nodes[i].active && name_matches(&nodes[i].name, nodes[i].name_len, name) {
            return Some(nodes[i]);
        }
        i = i.saturating_add(1);
    }
    None
}

pub fn devtmpfs_readdir(out: &mut [DevNode; 64]) -> usize {
    let nodes = DEVTMPFS_NODES.lock();
    let mut count = 0usize;
    let mut i = 0usize;
    while i < 256 && count < 64 {
        if nodes[i].active {
            out[count] = nodes[i];
            count = count.saturating_add(1);
        }
        i = i.saturating_add(1);
    }
    count
}

pub fn devtmpfs_chmod(name: &[u8], mode: u16) -> bool {
    let mut nodes = DEVTMPFS_NODES.lock();
    let mut i = 0usize;
    while i < 256 {
        if nodes[i].active && name_matches(&nodes[i].name, nodes[i].name_len, name) {
            nodes[i].mode = mode;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn devtmpfs_chown(name: &[u8], uid: u32, gid: u32) -> bool {
    let mut nodes = DEVTMPFS_NODES.lock();
    let mut i = 0usize;
    while i < 256 {
        if nodes[i].active && name_matches(&nodes[i].name, nodes[i].name_len, name) {
            nodes[i].uid = uid;
            nodes[i].gid = gid;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

// ---------------------------------------------------------------------------
// init
// ---------------------------------------------------------------------------

pub fn init() {
    // Standard /dev nodes: (name, type, major, minor)
    let nodes: &[(&[u8], DevNodeType, u32, u32)] = &[
        (b"console", DevNodeType::CharDevice, 5, 1),
        (b"null", DevNodeType::CharDevice, 1, 3),
        (b"zero", DevNodeType::CharDevice, 1, 5),
        (b"full", DevNodeType::CharDevice, 1, 7),
        (b"random", DevNodeType::CharDevice, 1, 8),
        (b"urandom", DevNodeType::CharDevice, 1, 9),
        (b"tty", DevNodeType::CharDevice, 5, 0),
        (b"ttyS0", DevNodeType::CharDevice, 4, 64),
        (b"sda", DevNodeType::BlockDevice, 8, 0),
        (b"sda1", DevNodeType::BlockDevice, 8, 1),
        (b"vda", DevNodeType::BlockDevice, 252, 0),
    ];
    let mut k = 0usize;
    while k < nodes.len() {
        devtmpfs_mknod(nodes[k].0, nodes[k].1, nodes[k].2, nodes[k].3);
        k = k.saturating_add(1);
    }
    serial_println!("[devtmpfs] devtmpfs initialized with 11 devices");
}
