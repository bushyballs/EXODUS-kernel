/// Bluetooth HCI stubs — no-heap, no-std, fixed-size static arrays.
///
/// Provides an adapter/connection/scan-result registry on top of the HCI
/// command constants.  All heap operations are prohibited; every collection
/// is a fixed-size array.
///
/// ## Safety / kernel rules enforced
/// - No `alloc::*` — no Vec, Box, String.
/// - No float arithmetic (`as f32` / `as f64` forbidden).
/// - No panic paths — all fallible operations return `Option<T>` or `bool`.
/// - All counters use `saturating_add` / `saturating_sub`.
/// - All sequence/handle numbers use `wrapping_add`.
/// - Structs held in static Mutex are `Copy` and provide `const fn empty()`.
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// HCI opcode constants
// ---------------------------------------------------------------------------

pub const HCI_CMD_RESET: u16 = 0x0C03;
pub const HCI_CMD_SCAN_ENABLE: u16 = 0x0C1A;
pub const HCI_CMD_INQUIRY: u16 = 0x0401;
pub const HCI_ACL_DATA_PKT: u8 = 2;
pub const HCI_EVENT_PKT: u8 = 4;

// ---------------------------------------------------------------------------
// Registry size limits
// ---------------------------------------------------------------------------

pub const MAX_BT_ADAPTERS: usize = 2;
pub const MAX_BT_CONNECTIONS: usize = 16;
pub const MAX_BT_SCAN_RESULTS: usize = 32;

// ---------------------------------------------------------------------------
// Address type
// ---------------------------------------------------------------------------

/// Distinguishes BR/EDR Classic addresses from Bluetooth Low Energy addresses.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BtAddrType {
    Classic,
    Le,
}

// ---------------------------------------------------------------------------
// BtAddr
// ---------------------------------------------------------------------------

/// Six-byte Bluetooth device address.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BtAddr {
    pub bytes: [u8; 6],
}

impl BtAddr {
    /// All-zeros address — used as a sentinel for "unset".
    pub const fn zero() -> Self {
        BtAddr { bytes: [0u8; 6] }
    }
}

// ---------------------------------------------------------------------------
// Connection state
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BtConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Disconnecting,
}

// ---------------------------------------------------------------------------
// BtConnection
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub struct BtConnection {
    pub handle: u16,
    pub remote_addr: BtAddr,
    pub addr_type: BtAddrType,
    pub state: BtConnectionState,
    /// Bytes received on this connection (saturating counter).
    pub rx_bytes: u64,
    /// Bytes transmitted on this connection (saturating counter).
    pub tx_bytes: u64,
    /// Slot is occupied when `true`.
    pub active: bool,
}

impl BtConnection {
    pub const fn empty() -> Self {
        BtConnection {
            handle: 0,
            remote_addr: BtAddr::zero(),
            addr_type: BtAddrType::Classic,
            state: BtConnectionState::Disconnected,
            rx_bytes: 0,
            tx_bytes: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// BtScanResult
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub struct BtScanResult {
    pub addr: BtAddr,
    pub rssi: i8,
    /// UTF-8 device name; only the first `name_len` bytes are meaningful.
    pub name: [u8; 32],
    pub name_len: u8,
    /// Slot is occupied when `true`.
    pub valid: bool,
}

impl BtScanResult {
    pub const fn empty() -> Self {
        BtScanResult {
            addr: BtAddr::zero(),
            rssi: 0,
            name: [0u8; 32],
            name_len: 0,
            valid: false,
        }
    }
}

// ---------------------------------------------------------------------------
// BtAdapter
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub struct BtAdapter {
    pub id: u32,
    pub local_addr: BtAddr,
    /// UTF-8 friendly name; only the first `name_len` bytes are meaningful.
    pub name: [u8; 32],
    pub name_len: u8,
    pub powered: bool,
    pub discoverable: bool,
    pub connections: [BtConnection; MAX_BT_CONNECTIONS],
    pub scan_results: [BtScanResult; MAX_BT_SCAN_RESULTS],
    /// Number of valid scan results stored in `scan_results`.
    pub nscans: u8,
    /// Slot is occupied when `true`.
    pub active: bool,
}

impl BtAdapter {
    pub const fn empty() -> Self {
        BtAdapter {
            id: 0,
            local_addr: BtAddr::zero(),
            name: [0u8; 32],
            name_len: 0,
            powered: false,
            discoverable: false,
            connections: [BtConnection::empty(); MAX_BT_CONNECTIONS],
            scan_results: [BtScanResult::empty(); MAX_BT_SCAN_RESULTS],
            nscans: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global adapter table
// ---------------------------------------------------------------------------

static BT_ADAPTERS: Mutex<[BtAdapter; MAX_BT_ADAPTERS]> =
    Mutex::new([BtAdapter::empty(); MAX_BT_ADAPTERS]);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Find the index of adapter with the given `id`, or `None`.
fn adapter_index(adapters: &[BtAdapter; MAX_BT_ADAPTERS], id: u32) -> Option<usize> {
    for i in 0..MAX_BT_ADAPTERS {
        if adapters[i].active && adapters[i].id == id {
            return Some(i);
        }
    }
    None
}

/// Find the index of a free adapter slot, or `None`.
fn free_adapter_slot(adapters: &[BtAdapter; MAX_BT_ADAPTERS]) -> Option<usize> {
    for i in 0..MAX_BT_ADAPTERS {
        if !adapters[i].active {
            return Some(i);
        }
    }
    None
}

/// Copy at most `dst_len` bytes from `src` into `dst`, return actual length.
fn copy_name(dst: &mut [u8; 32], src: &[u8]) -> u8 {
    let len = src.len().min(32);
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

/// Register a new Bluetooth adapter.
///
/// Allocates a free slot, assigns a monotonically increasing `id`, copies
/// `local_addr` and `name`.  Returns `Some(id)` on success, `None` if the
/// table is full.
pub fn bt_register_adapter(local_addr: [u8; 6], name: &[u8]) -> Option<u32> {
    let mut adapters = BT_ADAPTERS.lock();
    let slot = free_adapter_slot(&adapters)?;

    // Derive a stable id from the slot index so ids never collide within a
    // session.  Ids are 1-based to distinguish from unset (0).
    let id = (slot as u32).wrapping_add(1);

    adapters[slot] = BtAdapter::empty();
    adapters[slot].id = id;
    adapters[slot].local_addr = BtAddr { bytes: local_addr };
    adapters[slot].name_len = copy_name(&mut adapters[slot].name, name);
    adapters[slot].active = true;

    Some(id)
}

/// Power on adapter `adapter_id`.  Returns `false` if adapter not found.
pub fn bt_power_on(adapter_id: u32) -> bool {
    let mut adapters = BT_ADAPTERS.lock();
    match adapter_index(&adapters, adapter_id) {
        None => false,
        Some(i) => {
            adapters[i].powered = true;
            true
        }
    }
}

/// Power off adapter `adapter_id`.  Returns `false` if adapter not found.
pub fn bt_power_off(adapter_id: u32) -> bool {
    let mut adapters = BT_ADAPTERS.lock();
    match adapter_index(&adapters, adapter_id) {
        None => false,
        Some(i) => {
            adapters[i].powered = false;
            adapters[i].discoverable = false;
            true
        }
    }
}

/// Enable or disable page/inquiry scan on adapter `adapter_id`.
/// Returns `false` if the adapter is not found or is not powered.
pub fn bt_set_discoverable(adapter_id: u32, discoverable: bool) -> bool {
    let mut adapters = BT_ADAPTERS.lock();
    match adapter_index(&adapters, adapter_id) {
        None => false,
        Some(i) => {
            if !adapters[i].powered {
                return false;
            }
            adapters[i].discoverable = discoverable;
            true
        }
    }
}

/// Simulate an outbound connection from adapter `adapter_id` to `remote`.
///
/// Always succeeds if the adapter is powered.  Allocates a free connection
/// slot, advances the handle counter with `wrapping_add`, marks the
/// connection `Connected`, and returns `Some(handle)`.
///
/// Returns `None` if the adapter is not found, not powered, or the
/// connection table is full.
pub fn bt_connect(adapter_id: u32, remote: [u8; 6], addr_type: BtAddrType) -> Option<u16> {
    let mut adapters = BT_ADAPTERS.lock();
    let ai = adapter_index(&adapters, adapter_id)?;
    if !adapters[ai].powered {
        return None;
    }

    // Find a free connection slot.
    let mut ci: Option<usize> = None;
    for j in 0..MAX_BT_CONNECTIONS {
        if !adapters[ai].connections[j].active {
            ci = Some(j);
            break;
        }
    }
    let ci = ci?;

    // Derive a non-zero handle: use the slot index, wrapping but never 0.
    let raw_handle = (ci as u16).wrapping_add(1);
    let handle = if raw_handle == 0 { 1u16 } else { raw_handle };

    adapters[ai].connections[ci] = BtConnection {
        handle,
        remote_addr: BtAddr { bytes: remote },
        addr_type,
        state: BtConnectionState::Connected,
        rx_bytes: 0,
        tx_bytes: 0,
        active: true,
    };

    Some(handle)
}

/// Disconnect the connection identified by `handle` on adapter `adapter_id`.
///
/// Marks the connection `Disconnected` and frees the slot.
/// Returns `false` if the adapter or connection handle is not found.
pub fn bt_disconnect(adapter_id: u32, handle: u16) -> bool {
    let mut adapters = BT_ADAPTERS.lock();
    let ai = match adapter_index(&adapters, adapter_id) {
        None => return false,
        Some(i) => i,
    };
    for j in 0..MAX_BT_CONNECTIONS {
        if adapters[ai].connections[j].active && adapters[ai].connections[j].handle == handle {
            adapters[ai].connections[j].state = BtConnectionState::Disconnected;
            adapters[ai].connections[j].active = false;
            return true;
        }
    }
    false
}

/// Simulate sending `len` bytes over connection `handle`.
///
/// Increments `tx_bytes` with saturating arithmetic.  `data` is a fixed
/// 256-byte buffer; only the first `len` bytes are considered valid.
///
/// Returns `false` if the adapter or connection is not found / not active,
/// or if `len > 256`.
pub fn bt_send(adapter_id: u32, handle: u16, data: &[u8; 256], len: usize) -> bool {
    if len > 256 {
        return false;
    }
    let _ = data; // no actual I/O in stub
    let mut adapters = BT_ADAPTERS.lock();
    let ai = match adapter_index(&adapters, adapter_id) {
        None => return false,
        Some(i) => i,
    };
    for j in 0..MAX_BT_CONNECTIONS {
        if adapters[ai].connections[j].active && adapters[ai].connections[j].handle == handle {
            adapters[ai].connections[j].tx_bytes = adapters[ai].connections[j]
                .tx_bytes
                .saturating_add(len as u64);
            return true;
        }
    }
    false
}

/// Add a device discovery result to adapter `adapter_id`'s scan table.
///
/// If the table is full (`nscans == MAX_BT_SCAN_RESULTS`) the oldest entry
/// (index 0) is overwritten by shifting everything left by one, keeping the
/// table size bounded without heap allocation.
pub fn bt_scan_add_result(adapter_id: u32, addr: [u8; 6], rssi: i8, name: &[u8]) {
    let mut adapters = BT_ADAPTERS.lock();
    let ai = match adapter_index(&adapters, adapter_id) {
        None => return,
        Some(i) => i,
    };

    let nscans = adapters[ai].nscans as usize;

    if nscans >= MAX_BT_SCAN_RESULTS {
        // Shift left to discard oldest, then write into the last slot.
        let mut k = 0usize;
        while k < MAX_BT_SCAN_RESULTS.saturating_sub(1) {
            adapters[ai].scan_results[k] = adapters[ai].scan_results[k.saturating_add(1)];
            k = k.saturating_add(1);
        }
        let last = MAX_BT_SCAN_RESULTS.saturating_sub(1);
        let mut r = BtScanResult::empty();
        r.addr = BtAddr { bytes: addr };
        r.rssi = rssi;
        r.name_len = copy_name(&mut r.name, name);
        r.valid = true;
        adapters[ai].scan_results[last] = r;
        // nscans stays at MAX_BT_SCAN_RESULTS
    } else {
        let mut r = BtScanResult::empty();
        r.addr = BtAddr { bytes: addr };
        r.rssi = rssi;
        r.name_len = copy_name(&mut r.name, name);
        r.valid = true;
        adapters[ai].scan_results[nscans] = r;
        adapters[ai].nscans = (nscans as u8).saturating_add(1);
    }
}

/// Copy all scan results for adapter `adapter_id` into `out`.
///
/// Returns the number of valid results written.
/// Returns 0 if the adapter is not found.
pub fn bt_get_scan_results(adapter_id: u32, out: &mut [BtScanResult; MAX_BT_SCAN_RESULTS]) -> u8 {
    let adapters = BT_ADAPTERS.lock();
    let ai = match adapter_index(&adapters, adapter_id) {
        None => return 0,
        Some(i) => i,
    };
    let nscans = adapters[ai].nscans as usize;
    let mut i = 0usize;
    while i < nscans && i < MAX_BT_SCAN_RESULTS {
        out[i] = adapters[ai].scan_results[i];
        i = i.saturating_add(1);
    }
    adapters[ai].nscans
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialise the Bluetooth HCI framework.
///
/// Registers one virtual adapter with the Genesis BD address and starts it
/// in the powered-on / discoverable state.
pub fn init() {
    let local_addr = [0x00u8, 0x1A, 0x7D, 0x00, 0x00, 0x01];
    let name = b"genesis-bt";

    if let Some(id) = bt_register_adapter(local_addr, name) {
        bt_power_on(id);
        bt_set_discoverable(id, true);
        serial_println!("[bluetooth] HCI framework initialized (adapter id={})", id);
    } else {
        serial_println!("[bluetooth] HCI framework initialized (no free adapter slot)");
    }
}
