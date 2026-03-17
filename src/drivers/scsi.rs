use crate::sync::Mutex;
/// SCSI mid-layer for Genesis — generic SCSI host/device dispatch (no-heap)
///
/// Provides a host-adapter registration framework and a device table.
/// Dispatches SCSI CDB commands through registered queue functions.
/// Includes CDB builders and a simulated INQUIRY response helper.
///
/// All rules strictly observed:
///   - No heap: no Vec, Box, String, alloc::*
///   - No panics: no unwrap(), expect(), panic!()
///   - No float casts: no as f64, as f32
///   - Saturating arithmetic for counters
///   - Wrapping arithmetic for sequence numbers
///   - Structs in static Mutex are Copy with const fn empty()
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of SCSI host adapters
pub const MAX_SCSI_HOSTS: usize = 8;

/// Maximum number of SCSI devices (across all hosts)
pub const MAX_SCSI_DEVICES: usize = 64;

/// Maximum length of a Command Descriptor Block
pub const SCSI_MAX_CDB_LEN: usize = 16;

/// Size of the data transfer buffer embedded in ScsiCmd
pub const SCSI_BUF_SIZE: usize = 4096;

// SCSI command group codes
pub const SCSI_6_BYTE: u8 = 0;
pub const SCSI_10_BYTE: u8 = 1;
pub const SCSI_12_BYTE: u8 = 2;
pub const SCSI_16_BYTE: u8 = 3;

// SCSI opcode constants
pub const SCSI_TEST_UNIT_READY: u8 = 0x00;
pub const SCSI_INQUIRY: u8 = 0x12;
pub const SCSI_READ_CAPACITY: u8 = 0x25;
pub const SCSI_READ_10: u8 = 0x28;
pub const SCSI_WRITE_10: u8 = 0x2A;
pub const SCSI_MODE_SENSE_6: u8 = 0x1A;

// SCSI status codes
pub const SCSI_GOOD: u8 = 0x00;
pub const SCSI_CHECK_CONDITION: u8 = 0x02;
pub const SCSI_BUSY: u8 = 0x08;

// Sense key values
pub const SCSI_NO_SENSE: u8 = 0x00;
pub const SCSI_NOT_READY: u8 = 0x02;
pub const SCSI_MEDIUM_ERROR: u8 = 0x03;
pub const SCSI_ILLEGAL_REQUEST: u8 = 0x05;

// SCSI peripheral device type codes
pub const SCSI_TYPE_DISK: u8 = 0x00;
pub const SCSI_TYPE_TAPE: u8 = 0x01;
pub const SCSI_TYPE_CDROM: u8 = 0x05;

/// Simulated disk capacity: 10 GiB = 20 971 520 × 512-byte blocks
const SIM_DISK_BLOCKS: u64 = 20_971_520;
/// Simulated disk block size
const SIM_DISK_BLOCK_SIZE: u32 = 512;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A SCSI Command Descriptor Block plus associated data transfer buffer
#[derive(Copy, Clone)]
pub struct ScsiCmd {
    /// The raw CDB bytes (up to SCSI_MAX_CDB_LEN)
    pub cdb: [u8; SCSI_MAX_CDB_LEN],
    /// Number of valid bytes in `cdb`
    pub cdb_len: u8,
    /// Data transfer buffer (driver-owned DMA bounce region)
    pub data_buf: [u8; SCSI_BUF_SIZE],
    /// Number of valid bytes in `data_buf`
    pub data_len: usize,
    /// `true` = data transfer is host←device (read), `false` = host→device (write)
    pub data_in: bool,
    /// Logical unit number
    pub lun: u8,
    /// SCSI status byte set by the host adapter after command completion
    pub status: u8,
}

impl ScsiCmd {
    pub const fn empty() -> Self {
        ScsiCmd {
            cdb: [0u8; SCSI_MAX_CDB_LEN],
            cdb_len: 0,
            data_buf: [0u8; SCSI_BUF_SIZE],
            data_len: 0,
            data_in: false,
            lun: 0,
            status: SCSI_GOOD,
        }
    }
}

/// A SCSI device registered in the device table
#[derive(Copy, Clone)]
pub struct ScsiDevice {
    /// Host adapter this device belongs to
    pub host_id: u32,
    /// SCSI target ID (0-based)
    pub target: u8,
    /// Logical unit number
    pub lun: u8,
    /// SCSI peripheral device type (SCSI_TYPE_*)
    pub dev_type: u8,
    /// Vendor identification (8 ASCII bytes, space-padded)
    pub vendor: [u8; 8],
    /// Product identification (16 ASCII bytes, space-padded)
    pub model: [u8; 16],
    /// Device capacity in blocks
    pub capacity_blocks: u64,
    /// Bytes per block
    pub block_size: u32,
    /// Device has responded to TEST UNIT READY
    pub ready: bool,
    /// Slot is occupied
    pub active: bool,
}

impl ScsiDevice {
    pub const fn empty() -> Self {
        ScsiDevice {
            host_id: 0,
            target: 0,
            lun: 0,
            dev_type: SCSI_TYPE_DISK,
            vendor: [0u8; 8],
            model: [0u8; 16],
            capacity_blocks: 0,
            block_size: 0,
            ready: false,
            active: false,
        }
    }
}

/// Signature of a host-adapter queue function.
///
/// The mid-layer calls this to dispatch a CDB to the hardware.
/// Returns the SCSI status byte (SCSI_GOOD, SCSI_CHECK_CONDITION, …).
pub type ScsiQueueFn = fn(host_id: u32, dev: &ScsiDevice, cmd: &mut ScsiCmd) -> u8;

/// A registered SCSI host adapter
#[derive(Copy, Clone)]
pub struct ScsiHost {
    /// Unique numeric ID assigned at registration
    pub id: u32,
    /// Short driver name (ASCII, null-padded, up to 15 bytes)
    pub name: [u8; 16],
    /// Length of the valid portion of `name`
    pub name_len: u8,
    /// Host-adapter CDB dispatch function (None = stub always returns SCSI_GOOD)
    pub queue_fn: Option<ScsiQueueFn>,
    /// Maximum LUN index supported by this adapter
    pub max_lun: u8,
    /// Slot is occupied
    pub active: bool,
}

impl ScsiHost {
    pub const fn empty() -> Self {
        ScsiHost {
            id: 0,
            name: [0u8; 16],
            name_len: 0,
            queue_fn: None,
            max_lun: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static SCSI_HOSTS: Mutex<[ScsiHost; MAX_SCSI_HOSTS]> =
    Mutex::new([ScsiHost::empty(); MAX_SCSI_HOSTS]);

static SCSI_DEVICES: Mutex<[ScsiDevice; MAX_SCSI_DEVICES]> =
    Mutex::new([ScsiDevice::empty(); MAX_SCSI_DEVICES]);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Copy up to 15 bytes from `src` into a 16-byte fixed array, NUL-terminating
/// at byte 15.  Returns the number of bytes copied (excluding the terminator).
fn copy_name(dst: &mut [u8; 16], src: &[u8]) -> u8 {
    let len = if src.len() < 15 { src.len() } else { 15 };
    let mut i: usize = 0;
    while i < len {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
    while i < 16 {
        dst[i] = 0;
        i = i.saturating_add(1);
    }
    len as u8
}

/// Copy up to `max` bytes from `src` into `dst`, space-padding the remainder.
fn copy_padded(dst: &mut [u8], src: &[u8], max: usize) {
    let copy_len = if src.len() < max { src.len() } else { max };
    let mut i: usize = 0;
    while i < copy_len {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
    while i < max {
        dst[i] = b' ';
        i = i.saturating_add(1);
    }
}

// ---------------------------------------------------------------------------
// Host adapter API
// ---------------------------------------------------------------------------

/// Register a new SCSI host adapter.
///
/// `name`     — a short ASCII label (e.g. `b"genesis-scsi"`).
/// `queue_fn` — the CDB dispatch function; `None` uses a stub that always
///              returns `SCSI_GOOD`.
///
/// Returns the assigned host `id` on success, or `None` if the table is full.
pub fn scsi_host_register(name: &[u8], queue_fn: Option<ScsiQueueFn>) -> Option<u32> {
    let mut hosts = SCSI_HOSTS.lock();
    let mut i: usize = 0;
    while i < MAX_SCSI_HOSTS {
        if !hosts[i].active {
            let id = i as u32;
            hosts[i].id = id;
            hosts[i].name = [0u8; 16];
            hosts[i].name_len = copy_name(&mut hosts[i].name, name);
            hosts[i].queue_fn = queue_fn;
            hosts[i].max_lun = 7;
            hosts[i].active = true;
            return Some(id);
        }
        i = i.saturating_add(1);
    }
    None
}

/// Unregister a host adapter by `host_id`.
///
/// Returns `true` if the adapter was found and removed, `false` otherwise.
pub fn scsi_host_unregister(host_id: u32) -> bool {
    if host_id as usize >= MAX_SCSI_HOSTS {
        return false;
    }
    let mut hosts = SCSI_HOSTS.lock();
    if hosts[host_id as usize].active {
        hosts[host_id as usize] = ScsiHost::empty();
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// Device management API
// ---------------------------------------------------------------------------

/// Add a SCSI device to the device table.
///
/// `host_id`  — the host adapter the device is connected to.
/// `target`   — SCSI target ID (0-based).
/// `lun`      — logical unit number.
/// `dev_type` — `SCSI_TYPE_DISK`, `SCSI_TYPE_TAPE`, `SCSI_TYPE_CDROM`, etc.
///
/// Returns the assigned device index on success, or `None` if the table is
/// full or `host_id` is invalid.
pub fn scsi_device_add(host_id: u32, target: u8, lun: u8, dev_type: u8) -> Option<u32> {
    if host_id as usize >= MAX_SCSI_HOSTS {
        return None;
    }
    // Verify host is registered
    {
        let hosts = SCSI_HOSTS.lock();
        if !hosts[host_id as usize].active {
            return None;
        }
    }

    let mut devs = SCSI_DEVICES.lock();
    let mut i: usize = 0;
    while i < MAX_SCSI_DEVICES {
        if !devs[i].active {
            let dev_id = i as u32;
            devs[i].host_id = host_id;
            devs[i].target = target;
            devs[i].lun = lun;
            devs[i].dev_type = dev_type;
            devs[i].vendor = [b' '; 8];
            devs[i].model = [b' '; 16];
            devs[i].capacity_blocks = 0;
            devs[i].block_size = 0;
            devs[i].ready = false;
            devs[i].active = true;
            return Some(dev_id);
        }
        i = i.saturating_add(1);
    }
    None
}

/// Remove a SCSI device from the device table by its device index.
///
/// Returns `true` if found and removed, `false` otherwise.
pub fn scsi_device_remove(dev_id: u32) -> bool {
    if dev_id as usize >= MAX_SCSI_DEVICES {
        return false;
    }
    let mut devs = SCSI_DEVICES.lock();
    if devs[dev_id as usize].active {
        devs[dev_id as usize] = ScsiDevice::empty();
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// Command execution
// ---------------------------------------------------------------------------

/// Execute a SCSI command against a device.
///
/// Looks up the device in the device table, finds its host adapter, and
/// invokes the host's `queue_fn`.  If `queue_fn` is `None` the command
/// succeeds immediately (stub behaviour).
///
/// Returns the SCSI status byte.
pub fn scsi_execute(dev_id: u32, cmd: &mut ScsiCmd) -> u8 {
    if dev_id as usize >= MAX_SCSI_DEVICES {
        cmd.status = SCSI_CHECK_CONDITION;
        return SCSI_CHECK_CONDITION;
    }

    // Snapshot device info so we can release the device lock before calling
    // the queue function (which may itself try to acquire a lock).
    let (host_id, dev_snap) = {
        let devs = SCSI_DEVICES.lock();
        let d = &devs[dev_id as usize];
        if !d.active {
            return SCSI_CHECK_CONDITION;
        }
        (d.host_id, *d)
    };

    if host_id as usize >= MAX_SCSI_HOSTS {
        cmd.status = SCSI_CHECK_CONDITION;
        return SCSI_CHECK_CONDITION;
    }

    let queue_fn = {
        let hosts = SCSI_HOSTS.lock();
        let h = &hosts[host_id as usize];
        if !h.active {
            return SCSI_CHECK_CONDITION;
        }
        h.queue_fn
    };

    let status = match queue_fn {
        Some(f) => f(host_id, &dev_snap, cmd),
        None => SCSI_GOOD,
    };
    cmd.status = status;
    status
}

// ---------------------------------------------------------------------------
// CDB builder helpers
// ---------------------------------------------------------------------------

/// Build an INQUIRY (6-byte) CDB into `cmd`.
///
/// `alloc_len` — number of bytes the initiator is prepared to receive.
pub fn scsi_build_inquiry(cmd: &mut ScsiCmd, alloc_len: u8) {
    cmd.cdb = [0u8; SCSI_MAX_CDB_LEN];
    cmd.cdb[0] = SCSI_INQUIRY; // 0x12
    cmd.cdb[1] = 0x00; // EVPD=0
    cmd.cdb[2] = 0x00; // Page code (n/a when EVPD=0)
    cmd.cdb[3] = 0x00; // reserved
    cmd.cdb[4] = alloc_len; // Allocation Length
    cmd.cdb[5] = 0x00; // Control
    cmd.cdb_len = 6;
    cmd.data_in = true;
    cmd.data_len = 0;
}

/// Build a READ(10) CDB into `cmd`.
///
/// `lba`    — starting logical block address.
/// `blocks` — transfer length in logical blocks.
pub fn scsi_build_read10(cmd: &mut ScsiCmd, lba: u32, blocks: u16) {
    cmd.cdb = [0u8; SCSI_MAX_CDB_LEN];
    cmd.cdb[0] = SCSI_READ_10; // 0x28
    cmd.cdb[1] = 0x00; // flags (RDPROTECT, DPO, FUA, …)
    cmd.cdb[2] = ((lba >> 24) & 0xFF) as u8;
    cmd.cdb[3] = ((lba >> 16) & 0xFF) as u8;
    cmd.cdb[4] = ((lba >> 8) & 0xFF) as u8;
    cmd.cdb[5] = (lba & 0xFF) as u8;
    cmd.cdb[6] = 0x00; // Group number
    cmd.cdb[7] = ((blocks >> 8) & 0xFF) as u8;
    cmd.cdb[8] = (blocks & 0xFF) as u8;
    cmd.cdb[9] = 0x00; // Control
    cmd.cdb_len = 10;
    cmd.data_in = true;
    cmd.data_len = 0;
}

// ---------------------------------------------------------------------------
// Response builder helpers
// ---------------------------------------------------------------------------

/// Fill `data_buf` with a standard 36-byte INQUIRY response.
///
/// `dev_type` — peripheral device type code (SCSI_TYPE_*).
/// `vendor`   — up to 8 bytes of vendor ASCII (space-padded to 8).
/// `model`    — up to 16 bytes of product ASCII (space-padded to 16).
///
/// Returns the number of bytes written (36).
pub fn scsi_fill_inquiry(
    data_buf: &mut [u8; SCSI_BUF_SIZE],
    dev_type: u8,
    vendor: &[u8],
    model: &[u8],
) -> usize {
    // Zero out the first 36 bytes
    let mut i: usize = 0;
    while i < 36 {
        data_buf[i] = 0;
        i = i.saturating_add(1);
    }

    data_buf[0] = dev_type & 0x1F; // Peripheral device type (bits 4:0)
    data_buf[1] = 0x00; // RMB=0 (not removable)
    data_buf[2] = 0x05; // Version = SPC-3
    data_buf[3] = 0x12; // Response data format = 2, HiSup=1
    data_buf[4] = 31; // Additional length = 36 - 5

    // Bytes 5–7: flags — all zero for a basic device
    // Bytes 8–15: Vendor identification (T10 VID), space-padded
    copy_padded(&mut data_buf[8..16], vendor, 8);
    // Bytes 16–31: Product identification, space-padded
    copy_padded(&mut data_buf[16..32], model, 16);
    // Bytes 32–35: Product revision level (ASCII "1.00")
    data_buf[32] = b'1';
    data_buf[33] = b'.';
    data_buf[34] = b'0';
    data_buf[35] = b'0';

    36
}

// ---------------------------------------------------------------------------
// Driver initialisation
// ---------------------------------------------------------------------------

/// Initialise the SCSI mid-layer.
///
/// Registers one simulated SCSI host adapter ("genesis-scsi") with a stub
/// queue function, then adds one simulated direct-access disk at target 0,
/// LUN 0 with 10 GiB capacity.
pub fn init() {
    // Register simulated host adapter
    let host_id = scsi_host_register(b"genesis-scsi", None);

    if let Some(hid) = host_id {
        // Add simulated disk: target=0, lun=0, type=DISK
        let dev_id = scsi_device_add(hid, 0, 0, SCSI_TYPE_DISK);

        if let Some(did) = dev_id {
            // Populate vendor/model/capacity on the device slot
            let mut devs = SCSI_DEVICES.lock();
            let d = &mut devs[did as usize];

            copy_padded(&mut d.vendor, b"GENESIS ", 8);
            copy_padded(&mut d.model, b"GENESIS SIM DISK", 16);
            d.capacity_blocks = SIM_DISK_BLOCKS;
            d.block_size = SIM_DISK_BLOCK_SIZE;
            d.ready = true;
        }
    }

    serial_println!("[scsi] SCSI mid-layer initialized");
}
