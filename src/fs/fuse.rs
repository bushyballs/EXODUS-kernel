use crate::fs::vfs::FsError;
/// FUSE (Filesystem in Userspace) framework for Genesis
///
/// Provides a framework for implementing filesystems in userspace.
/// The kernel intercepts VFS operations on FUSE mounts and forwards
/// them as messages to a userspace daemon via a /dev/fuse device.
///
/// Architecture:
///   1. Userspace process opens /dev/fuse and calls mount()
///   2. Kernel creates a FUSE connection with a request queue
///   3. VFS operations on the mount generate FUSE requests
///   4. Userspace reads requests from /dev/fuse, processes them,
///      and writes responses back
///   5. Kernel completes the original VFS operation with the response
///
/// Request flow: VFS op -> FUSE request -> /dev/fuse -> userspace -> response -> VFS completion
///
/// Inspired by: Linux FUSE, libfuse, macFUSE. All code is original.
use crate::serial_println;
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// FUSE protocol version
const FUSE_KERNEL_VERSION: u32 = 7;
const FUSE_KERNEL_MINOR_VERSION: u32 = 38;

/// Maximum size of a FUSE request/response
const FUSE_MAX_BUFFER: usize = 131072; // 128 KB

/// FUSE opcodes (request types)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum FuseOpcode {
    Lookup = 1,
    Forget = 2,
    Getattr = 3,
    Setattr = 4,
    Readlink = 5,
    Symlink = 6,
    Mknod = 8,
    Mkdir = 9,
    Unlink = 10,
    Rmdir = 11,
    Rename = 12,
    Link = 13,
    Open = 14,
    Read = 15,
    Write = 16,
    Statfs = 17,
    Release = 18,
    Fsync = 20,
    Setxattr = 21,
    Getxattr = 22,
    Listxattr = 23,
    Removexattr = 24,
    Flush = 25,
    Init = 26,
    Opendir = 27,
    Readdir = 28,
    Releasedir = 29,
    Fsyncdir = 30,
    Access = 34,
    Create = 35,
    Destroy = 38,
}

impl FuseOpcode {
    fn from_u32(val: u32) -> Option<Self> {
        match val {
            1 => Some(FuseOpcode::Lookup),
            2 => Some(FuseOpcode::Forget),
            3 => Some(FuseOpcode::Getattr),
            4 => Some(FuseOpcode::Setattr),
            5 => Some(FuseOpcode::Readlink),
            6 => Some(FuseOpcode::Symlink),
            8 => Some(FuseOpcode::Mknod),
            9 => Some(FuseOpcode::Mkdir),
            10 => Some(FuseOpcode::Unlink),
            11 => Some(FuseOpcode::Rmdir),
            12 => Some(FuseOpcode::Rename),
            13 => Some(FuseOpcode::Link),
            14 => Some(FuseOpcode::Open),
            15 => Some(FuseOpcode::Read),
            16 => Some(FuseOpcode::Write),
            17 => Some(FuseOpcode::Statfs),
            18 => Some(FuseOpcode::Release),
            20 => Some(FuseOpcode::Fsync),
            21 => Some(FuseOpcode::Setxattr),
            22 => Some(FuseOpcode::Getxattr),
            23 => Some(FuseOpcode::Listxattr),
            24 => Some(FuseOpcode::Removexattr),
            25 => Some(FuseOpcode::Flush),
            26 => Some(FuseOpcode::Init),
            27 => Some(FuseOpcode::Opendir),
            28 => Some(FuseOpcode::Readdir),
            29 => Some(FuseOpcode::Releasedir),
            30 => Some(FuseOpcode::Fsyncdir),
            34 => Some(FuseOpcode::Access),
            35 => Some(FuseOpcode::Create),
            38 => Some(FuseOpcode::Destroy),
            _ => None,
        }
    }
}

/// FUSE request header (sent from kernel to userspace)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FuseInHeader {
    pub len: u32,
    pub opcode: u32,
    pub unique: u64,
    pub nodeid: u64,
    pub uid: u32,
    pub gid: u32,
    pub pid: u32,
    pub _padding: u32,
}

/// FUSE response header (sent from userspace to kernel)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FuseOutHeader {
    pub len: u32,
    pub error: i32,
    pub unique: u64,
}

/// FUSE init request data
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FuseInitIn {
    pub major: u32,
    pub minor: u32,
    pub max_readahead: u32,
    pub flags: u32,
}

/// FUSE init response data
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FuseInitOut {
    pub major: u32,
    pub minor: u32,
    pub max_readahead: u32,
    pub flags: u32,
    pub max_background: u16,
    pub congestion_threshold: u16,
    pub max_write: u32,
    pub time_gran: u32,
    pub max_pages: u16,
    pub map_alignment: u16,
    pub _unused: [u32; 8],
}

/// FUSE attr (file attributes in FUSE protocol)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FuseAttr {
    pub ino: u64,
    pub size: u64,
    pub blocks: u64,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
    pub atimensec: u32,
    pub mtimensec: u32,
    pub ctimensec: u32,
    pub mode: u32,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub rdev: u32,
    pub blksize: u32,
    pub _padding: u32,
}

/// FUSE getattr response
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FuseAttrOut {
    pub attr_valid: u64,
    pub attr_valid_nsec: u32,
    pub _dummy: u32,
    pub attr: FuseAttr,
}

/// FUSE entry response (for lookup)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FuseEntryOut {
    pub nodeid: u64,
    pub generation: u64,
    pub entry_valid: u64,
    pub attr_valid: u64,
    pub entry_valid_nsec: u32,
    pub attr_valid_nsec: u32,
    pub attr: FuseAttr,
}

/// FUSE open request
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FuseOpenIn {
    pub flags: u32,
    pub _unused: u32,
}

/// FUSE open response
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FuseOpenOut {
    pub fh: u64,
    pub open_flags: u32,
    pub _padding: u32,
}

/// FUSE read request
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FuseReadIn {
    pub fh: u64,
    pub offset: u64,
    pub size: u32,
    pub read_flags: u32,
    pub lock_owner: u64,
    pub flags: u32,
    pub _padding: u32,
}

/// FUSE write request
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FuseWriteIn {
    pub fh: u64,
    pub offset: u64,
    pub size: u32,
    pub write_flags: u32,
    pub lock_owner: u64,
    pub flags: u32,
    pub _padding: u32,
}

/// FUSE write response
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FuseWriteOut {
    pub size: u32,
    pub _padding: u32,
}

/// Connection state for a FUSE mount
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FuseConnectionState {
    /// Waiting for FUSE_INIT handshake
    Initializing,
    /// Connection is active and processing requests
    Active,
    /// Connection is shutting down
    Destroying,
    /// Connection has been closed
    Closed,
}

/// A pending FUSE request waiting for a userspace response
pub struct FusePendingRequest {
    pub unique: u64,
    pub opcode: FuseOpcode,
    pub nodeid: u64,
    pub uid: u32,
    pub gid: u32,
    pub pid: u32,
    pub data: Vec<u8>,
}

/// A FUSE connection represents a single mounted FUSE filesystem
pub struct FuseConnection {
    /// Connection ID
    pub id: u64,
    /// Mount point path
    pub mount_point: String,
    /// PID of the userspace daemon
    pub daemon_pid: u32,
    /// Connection state
    pub state: FuseConnectionState,
    /// Protocol version negotiated
    pub proto_major: u32,
    pub proto_minor: u32,
    /// Maximum read-ahead
    pub max_readahead: u32,
    /// Maximum write size
    pub max_write: u32,
    /// Request queue (kernel -> userspace)
    pub request_queue: Vec<FusePendingRequest>,
    /// Next unique request ID
    pub next_unique: u64,
    /// Inode-to-lookup-count map (for FORGET)
    pub inode_lookup_count: BTreeMap<u64, u64>,
}

impl FuseConnection {
    pub fn new(id: u64, mount_point: &str, daemon_pid: u32) -> Self {
        FuseConnection {
            id,
            mount_point: String::from(mount_point),
            daemon_pid,
            state: FuseConnectionState::Initializing,
            proto_major: FUSE_KERNEL_VERSION,
            proto_minor: FUSE_KERNEL_MINOR_VERSION,
            max_readahead: 65536,
            max_write: 65536,
            request_queue: Vec::new(),
            next_unique: 1,
            inode_lookup_count: BTreeMap::new(),
        }
    }

    /// Generate the FUSE_INIT request to send to userspace
    pub fn generate_init_request(&mut self) -> Vec<u8> {
        let unique = self.next_unique;
        self.next_unique = self.next_unique.saturating_add(1);

        let header = FuseInHeader {
            len: (core::mem::size_of::<FuseInHeader>() + core::mem::size_of::<FuseInitIn>()) as u32,
            opcode: FuseOpcode::Init as u32,
            unique,
            nodeid: 0,
            uid: 0,
            gid: 0,
            pid: 0,
            _padding: 0,
        };

        let init = FuseInitIn {
            major: FUSE_KERNEL_VERSION,
            minor: FUSE_KERNEL_MINOR_VERSION,
            max_readahead: self.max_readahead,
            flags: 0,
        };

        let mut buf = alloc::vec![0u8; header.len as usize];
        unsafe {
            core::ptr::write_unaligned(buf.as_mut_ptr() as *mut FuseInHeader, header);
            let init_ptr = buf.as_mut_ptr().add(core::mem::size_of::<FuseInHeader>());
            core::ptr::write_unaligned(init_ptr as *mut FuseInitIn, init);
        }

        buf
    }

    /// Process a FUSE_INIT response from userspace
    pub fn handle_init_response(&mut self, data: &[u8]) -> Result<(), FsError> {
        if data.len() < core::mem::size_of::<FuseOutHeader>() + core::mem::size_of::<FuseInitOut>()
        {
            return Err(FsError::InvalidArgument);
        }

        let out_header: FuseOutHeader =
            unsafe { core::ptr::read_unaligned(data.as_ptr() as *const FuseOutHeader) };

        if out_header.error != 0 {
            serial_println!("  FUSE: init failed with error {}", out_header.error);
            self.state = FuseConnectionState::Closed;
            return Err(FsError::IoError);
        }

        let init_out: FuseInitOut = unsafe {
            core::ptr::read_unaligned(
                data[core::mem::size_of::<FuseOutHeader>()..].as_ptr() as *const FuseInitOut
            )
        };

        self.proto_major = init_out.major;
        self.proto_minor = init_out.minor;
        self.max_readahead = init_out.max_readahead;
        self.max_write = init_out.max_write;
        self.state = FuseConnectionState::Active;

        serial_println!(
            "  FUSE: connection {} initialized (v{}.{})",
            self.id,
            init_out.major,
            init_out.minor
        );

        Ok(())
    }

    /// Enqueue a request for the userspace daemon
    fn enqueue_request(
        &mut self,
        opcode: FuseOpcode,
        nodeid: u64,
        uid: u32,
        gid: u32,
        pid: u32,
        data: Vec<u8>,
    ) -> u64 {
        let unique = self.next_unique;
        self.next_unique = self.next_unique.saturating_add(1);

        self.request_queue.push(FusePendingRequest {
            unique,
            opcode,
            nodeid,
            uid,
            gid,
            pid,
            data,
        });

        unique
    }

    /// Create a LOOKUP request
    pub fn request_lookup(&mut self, parent: u64, name: &str, uid: u32, gid: u32, pid: u32) -> u64 {
        let mut data = name.as_bytes().to_vec();
        data.push(0); // null terminator
        self.enqueue_request(FuseOpcode::Lookup, parent, uid, gid, pid, data)
    }

    /// Create a GETATTR request
    pub fn request_getattr(&mut self, nodeid: u64, uid: u32, gid: u32, pid: u32) -> u64 {
        self.enqueue_request(FuseOpcode::Getattr, nodeid, uid, gid, pid, Vec::new())
    }

    /// Create an OPEN request
    pub fn request_open(&mut self, nodeid: u64, flags: u32, uid: u32, gid: u32, pid: u32) -> u64 {
        let open_in = FuseOpenIn { flags, _unused: 0 };
        let mut data = alloc::vec![0u8; core::mem::size_of::<FuseOpenIn>()];
        unsafe {
            core::ptr::write_unaligned(data.as_mut_ptr() as *mut FuseOpenIn, open_in);
        }
        self.enqueue_request(FuseOpcode::Open, nodeid, uid, gid, pid, data)
    }

    /// Create a READ request
    pub fn request_read(
        &mut self,
        nodeid: u64,
        fh: u64,
        offset: u64,
        size: u32,
        uid: u32,
        gid: u32,
        pid: u32,
    ) -> u64 {
        let read_in = FuseReadIn {
            fh,
            offset,
            size,
            read_flags: 0,
            lock_owner: 0,
            flags: 0,
            _padding: 0,
        };
        let mut data = alloc::vec![0u8; core::mem::size_of::<FuseReadIn>()];
        unsafe {
            core::ptr::write_unaligned(data.as_mut_ptr() as *mut FuseReadIn, read_in);
        }
        self.enqueue_request(FuseOpcode::Read, nodeid, uid, gid, pid, data)
    }

    /// Create a READDIR request
    pub fn request_readdir(
        &mut self,
        nodeid: u64,
        fh: u64,
        offset: u64,
        size: u32,
        uid: u32,
        gid: u32,
        pid: u32,
    ) -> u64 {
        let read_in = FuseReadIn {
            fh,
            offset,
            size,
            read_flags: 0,
            lock_owner: 0,
            flags: 0,
            _padding: 0,
        };
        let mut data = alloc::vec![0u8; core::mem::size_of::<FuseReadIn>()];
        unsafe {
            core::ptr::write_unaligned(data.as_mut_ptr() as *mut FuseReadIn, read_in);
        }
        self.enqueue_request(FuseOpcode::Readdir, nodeid, uid, gid, pid, data)
    }

    /// Dequeue the next pending request (userspace reads from /dev/fuse)
    pub fn dequeue_request(&mut self) -> Option<FusePendingRequest> {
        if self.request_queue.is_empty() {
            None
        } else {
            Some(self.request_queue.remove(0))
        }
    }

    /// Serialize a pending request into the wire format for /dev/fuse read
    pub fn serialize_request(req: &FusePendingRequest) -> Vec<u8> {
        let header_size = core::mem::size_of::<FuseInHeader>();
        let total = header_size + req.data.len();

        let header = FuseInHeader {
            len: total as u32,
            opcode: req.opcode as u32,
            unique: req.unique,
            nodeid: req.nodeid,
            uid: req.uid,
            gid: req.gid,
            pid: req.pid,
            _padding: 0,
        };

        let mut buf = alloc::vec![0u8; total];
        unsafe {
            core::ptr::write_unaligned(buf.as_mut_ptr() as *mut FuseInHeader, header);
        }
        buf[header_size..].copy_from_slice(&req.data);

        buf
    }

    /// Begin unmount: send DESTROY and transition state
    pub fn begin_unmount(&mut self) -> u64 {
        self.state = FuseConnectionState::Destroying;
        self.enqueue_request(FuseOpcode::Destroy, 0, 0, 0, 0, Vec::new())
    }

    /// Complete unmount after DESTROY response
    pub fn complete_unmount(&mut self) {
        self.state = FuseConnectionState::Closed;
        self.request_queue.clear();
        self.inode_lookup_count.clear();
        serial_println!(
            "  FUSE: connection {} unmounted from {}",
            self.id,
            self.mount_point
        );
    }

    /// Track inode lookup count (for FORGET protocol)
    pub fn increment_lookup(&mut self, nodeid: u64) {
        let count = self.inode_lookup_count.entry(nodeid).or_insert(0);
        *count = count.saturating_add(1);
    }

    /// Decrement lookup count (FORGET)
    pub fn forget(&mut self, nodeid: u64, nlookup: u64) {
        if let Some(count) = self.inode_lookup_count.get_mut(&nodeid) {
            if nlookup >= *count {
                self.inode_lookup_count.remove(&nodeid);
            } else {
                *count -= nlookup;
            }
        }
    }
}

/// Global FUSE connection registry
struct FuseRegistry {
    connections: BTreeMap<u64, FuseConnection>,
    next_id: u64,
}

static FUSE_REGISTRY: Mutex<FuseRegistry> = Mutex::new(FuseRegistry {
    connections: BTreeMap::new(),
    next_id: 1,
});

/// Mount a new FUSE filesystem
pub fn mount(mount_point: &str, daemon_pid: u32) -> Result<u64, FsError> {
    let mut registry = FUSE_REGISTRY.lock();

    // Check if mount point is already in use
    for (_id, conn) in registry.connections.iter() {
        if conn.mount_point == mount_point && conn.state != FuseConnectionState::Closed {
            return Err(FsError::AlreadyExists);
        }
    }

    let id = registry.next_id;
    registry.next_id = registry.next_id.saturating_add(1);

    let conn = FuseConnection::new(id, mount_point, daemon_pid);
    registry.connections.insert(id, conn);

    serial_println!(
        "  FUSE: mount {} at {} (daemon pid {})",
        id,
        mount_point,
        daemon_pid
    );
    Ok(id)
}

/// Unmount a FUSE filesystem
pub fn unmount(connection_id: u64) -> Result<(), FsError> {
    let mut registry = FUSE_REGISTRY.lock();

    let conn = registry
        .connections
        .get_mut(&connection_id)
        .ok_or(FsError::NotFound)?;

    if conn.state == FuseConnectionState::Closed {
        return Err(FsError::InvalidArgument);
    }

    conn.begin_unmount();
    Ok(())
}

/// Get the number of active FUSE connections
pub fn active_connections() -> usize {
    let registry = FUSE_REGISTRY.lock();
    registry
        .connections
        .iter()
        .filter(|(_, c)| c.state == FuseConnectionState::Active)
        .count()
}

/// List all FUSE mount points
pub fn list_mounts() -> Vec<(u64, String, FuseConnectionState)> {
    let registry = FUSE_REGISTRY.lock();
    registry
        .connections
        .iter()
        .map(|(id, conn)| (*id, conn.mount_point.clone(), conn.state))
        .collect()
}

/// Initialize the FUSE framework
pub fn init() {
    serial_println!(
        "  FUSE: userspace filesystem framework initialized (v{}.{})",
        FUSE_KERNEL_VERSION,
        FUSE_KERNEL_MINOR_VERSION
    );
}
