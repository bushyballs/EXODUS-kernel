use crate::sync::Mutex;
/// Binder IPC — Android-style transactional inter-process communication
///
/// Provides object-oriented RPC between processes via transactions.
/// Services register with the service manager and clients obtain
/// proxy handles to communicate. Supports oneway (async) transactions,
/// parcel-based data serialization, and death notifications.
///
/// Inspired by: Android Binder (transaction model, parcels, service manager),
/// COM/DCOM (interface-based RPC), D-Bus (service discovery).
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_SERVICES: usize = 256;
const MAX_TRANSACTIONS: usize = 1024;
const MAX_PARCEL_SIZE: usize = 8192;
const MAX_DEATH_OBSERVERS: usize = 64;
const BINDER_HANDLE_SERVICE_MANAGER: u32 = 0;

/// Transaction codes
pub const TXN_FIRST_CALL: u32 = 0x0000_0001;
pub const TXN_LAST_CALL: u32 = 0x00FF_FFFF;
pub const TXN_PING: u32 = 0x5F504E47; // '_PNG'
pub const TXN_DUMP: u32 = 0x5F444D50; // '_DMP'
pub const TXN_INTERFACE: u32 = 0x5F494E54; // '_INT'

/// Transaction flags
pub const FLAG_ONEWAY: u32 = 0x01;
pub const FLAG_CLEAR_BUF: u32 = 0x02;
pub const FLAG_STATUS_CODE: u32 = 0x04;
pub const FLAG_ACCEPT_FDS: u32 = 0x10;

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static BINDER_STATE: Mutex<Option<BinderDriver>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Parcel — serialized data container for transactions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Parcel {
    data: Vec<u8>,
    read_pos: usize,
    objects: Vec<FlatBinderObject>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlatBinderType {
    Binder, // local object
    Handle, // remote reference
    Fd,     // file descriptor
}

#[derive(Debug, Clone, Copy)]
pub struct FlatBinderObject {
    pub obj_type: FlatBinderType,
    pub flags: u32,
    pub handle_or_ptr: u64,
    pub cookie: u64,
}

impl Parcel {
    pub fn new() -> Self {
        Parcel {
            data: Vec::new(),
            read_pos: 0,
            objects: Vec::new(),
        }
    }

    pub fn from_data(data: Vec<u8>) -> Self {
        Parcel {
            data,
            read_pos: 0,
            objects: Vec::new(),
        }
    }

    /// Write a 32-bit integer (little-endian)
    pub fn write_i32(&mut self, val: i32) {
        let bytes = val.to_le_bytes();
        self.data.extend_from_slice(&bytes);
    }

    /// Write a 64-bit integer (little-endian)
    pub fn write_u64(&mut self, val: u64) {
        let bytes = val.to_le_bytes();
        self.data.extend_from_slice(&bytes);
    }

    /// Write a length-prefixed byte slice
    pub fn write_bytes(&mut self, buf: &[u8]) {
        self.write_i32(buf.len() as i32);
        self.data.extend_from_slice(buf);
        // Pad to 4-byte alignment
        let pad = (4 - (buf.len() % 4)) % 4;
        for _ in 0..pad {
            self.data.push(0);
        }
    }

    /// Write a length-prefixed string
    pub fn write_string(&mut self, s: &str) {
        self.write_bytes(s.as_bytes());
    }

    /// Read a 32-bit integer
    pub fn read_i32(&mut self) -> Result<i32, &'static str> {
        if self.read_pos + 4 > self.data.len() {
            return Err("parcel underflow");
        }
        let mut buf = [0u8; 4];
        buf.copy_from_slice(&self.data[self.read_pos..self.read_pos + 4]);
        self.read_pos += 4;
        Ok(i32::from_le_bytes(buf))
    }

    /// Read a 64-bit integer
    pub fn read_u64(&mut self) -> Result<u64, &'static str> {
        if self.read_pos + 8 > self.data.len() {
            return Err("parcel underflow");
        }
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&self.data[self.read_pos..self.read_pos + 8]);
        self.read_pos += 8;
        Ok(u64::from_le_bytes(buf))
    }

    /// Read a length-prefixed byte slice
    pub fn read_bytes(&mut self) -> Result<Vec<u8>, &'static str> {
        let len = self.read_i32()? as usize;
        if self.read_pos + len > self.data.len() {
            return Err("parcel underflow");
        }
        let result = self.data[self.read_pos..self.read_pos + len].to_vec();
        self.read_pos += len;
        // Skip alignment padding
        let pad = (4 - (len % 4)) % 4;
        self.read_pos += pad;
        Ok(result)
    }

    /// Attach a flat binder object (for fd passing or binder references)
    pub fn write_object(&mut self, obj: FlatBinderObject) {
        self.objects.push(obj);
    }

    pub fn data_size(&self) -> usize {
        self.data.len()
    }
    pub fn objects_count(&self) -> usize {
        self.objects.len()
    }
    pub fn reset_read(&mut self) {
        self.read_pos = 0;
    }
}

// ---------------------------------------------------------------------------
// Transaction — a single Binder RPC call
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionState {
    Pending,
    InProgress,
    Replied,
    Failed,
    OneWay,
}

#[derive(Debug, Clone)]
pub struct Transaction {
    pub id: u64,
    pub sender_pid: u32,
    pub sender_euid: u32,
    pub target_handle: u32,
    pub code: u32,
    pub flags: u32,
    pub data: Vec<u8>,
    pub offsets: Vec<u64>,
    pub reply_data: Vec<u8>,
    pub state: TransactionState,
    pub timestamp: u64,
}

impl Transaction {
    pub fn is_oneway(&self) -> bool {
        self.flags & FLAG_ONEWAY != 0
    }
}

// ---------------------------------------------------------------------------
// Binder node — represents a local service object
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct BinderNode {
    pub ptr: u64,    // pointer in owning process
    pub cookie: u64, // opaque data for the owner
    pub owner_pid: u32,
    pub strong_refs: u32,
    pub weak_refs: u32,
    pub has_async_txn: bool,
    pub accept_fds: bool,
}

// ---------------------------------------------------------------------------
// Binder reference — a handle to a remote binder node
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct BinderRef {
    pub handle: u32,
    pub node_id: u64,
    pub owner_pid: u32,
    pub strong_refs: u32,
    pub weak_refs: u32,
    pub death_notified: bool,
}

// ---------------------------------------------------------------------------
// Death notification — notifies when a binder node's process dies
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DeathNotification {
    pub observer_pid: u32,
    pub target_handle: u32,
    pub cookie: u64,
    pub delivered: bool,
}

// ---------------------------------------------------------------------------
// Service registration — service manager's registry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ServiceEntry {
    pub name: String,
    pub owner_pid: u32,
    pub handle: u32,
    pub node_ptr: u64,
    pub allow_isolated: bool,
    pub dump_priority: i32,
}

// ---------------------------------------------------------------------------
// Per-process binder state
// ---------------------------------------------------------------------------

struct ProcessBinderState {
    pid: u32,
    nodes: BTreeMap<u64, BinderNode>, // ptr -> node
    refs: BTreeMap<u32, BinderRef>,   // handle -> ref
    pending_txns: Vec<u64>,           // transaction IDs
    reply_queue: Vec<u64>,            // completed transaction IDs
    death_notifications: Vec<DeathNotification>,
    next_handle: u32,
    max_threads: u32,
    active_threads: u32,
}

impl ProcessBinderState {
    fn new(pid: u32) -> Self {
        ProcessBinderState {
            pid,
            nodes: BTreeMap::new(),
            refs: BTreeMap::new(),
            pending_txns: Vec::new(),
            reply_queue: Vec::new(),
            death_notifications: Vec::new(),
            next_handle: 1,
            max_threads: 4,
            active_threads: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Binder driver — the core IPC mechanism
// ---------------------------------------------------------------------------

pub struct BinderDriver {
    processes: BTreeMap<u32, ProcessBinderState>,
    transactions: BTreeMap<u64, Transaction>,
    services: BTreeMap<String, ServiceEntry>,
    next_txn_id: u64,
    next_node_id: u64,
    total_transactions: u64,
    total_failed: u64,
}

impl BinderDriver {
    fn new() -> Self {
        BinderDriver {
            processes: BTreeMap::new(),
            transactions: BTreeMap::new(),
            services: BTreeMap::new(),
            next_txn_id: 1,
            next_node_id: 1,
            total_transactions: 0,
            total_failed: 0,
        }
    }

    /// Register a process with the binder driver
    pub fn open_process(&mut self, pid: u32) {
        self.processes.insert(pid, ProcessBinderState::new(pid));
        serial_println!("    [binder] Process {} opened binder", pid);
    }

    /// Remove a process and clean up all its resources
    pub fn close_process(&mut self, pid: u32) {
        // Collect death notifications to deliver
        let mut death_cookies: Vec<(u32, u64)> = Vec::new();
        if let Some(state) = self.processes.get(&pid) {
            // For each node owned by this process, notify observers
            for (_ptr, node) in state.nodes.iter() {
                for (_other_pid, other_state) in self.processes.iter() {
                    for notif in other_state.death_notifications.iter() {
                        if !notif.delivered {
                            death_cookies.push((notif.observer_pid, notif.cookie));
                        }
                    }
                }
                let _ = node; // suppress warning
            }
        }

        // Deliver death notifications
        for (observer_pid, cookie) in death_cookies {
            if let Some(obs_state) = self.processes.get_mut(&observer_pid) {
                for notif in obs_state.death_notifications.iter_mut() {
                    if notif.cookie == cookie && !notif.delivered {
                        notif.delivered = true;
                        serial_println!(
                            "    [binder] Death notification delivered to PID {} (cookie=0x{:X})",
                            observer_pid,
                            cookie
                        );
                    }
                }
            }
        }

        // Remove services owned by this process
        self.services.retain(|_name, entry| entry.owner_pid != pid);

        self.processes.remove(&pid);
        serial_println!("    [binder] Process {} closed binder", pid);
    }

    /// Create a binder node (local service object) in a process
    pub fn create_node(&mut self, pid: u32, ptr: u64, cookie: u64) -> Result<u64, &'static str> {
        let state = self
            .processes
            .get_mut(&pid)
            .ok_or("process not registered")?;
        let node_id = self.next_node_id;
        self.next_node_id = self.next_node_id.saturating_add(1);

        let node = BinderNode {
            ptr,
            cookie,
            owner_pid: pid,
            strong_refs: 1,
            weak_refs: 0,
            has_async_txn: false,
            accept_fds: true,
        };
        state.nodes.insert(ptr, node);
        Ok(node_id)
    }

    /// Create a reference (handle) to a remote binder node
    pub fn create_ref(
        &mut self,
        pid: u32,
        target_pid: u32,
        node_ptr: u64,
    ) -> Result<u32, &'static str> {
        // Verify the target node exists
        let target_state = self
            .processes
            .get(&target_pid)
            .ok_or("target process not found")?;
        if !target_state.nodes.contains_key(&node_ptr) {
            return Err("node not found in target process");
        }

        let state = self
            .processes
            .get_mut(&pid)
            .ok_or("process not registered")?;
        let handle = state.next_handle;
        state.next_handle = state.next_handle.saturating_add(1);

        let bref = BinderRef {
            handle,
            node_id: node_ptr,
            owner_pid: target_pid,
            strong_refs: 1,
            weak_refs: 0,
            death_notified: false,
        };
        state.refs.insert(handle, bref);
        Ok(handle)
    }

    /// Send a transaction (synchronous or oneway)
    pub fn transact(
        &mut self,
        sender_pid: u32,
        target_handle: u32,
        code: u32,
        flags: u32,
        data: Vec<u8>,
    ) -> Result<u64, &'static str> {
        if data.len() > MAX_PARCEL_SIZE {
            self.total_failed = self.total_failed.saturating_add(1);
            return Err("transaction data too large");
        }
        if self.transactions.len() >= MAX_TRANSACTIONS {
            self.total_failed = self.total_failed.saturating_add(1);
            return Err("transaction table full");
        }

        let txn_id = self.next_txn_id;
        self.next_txn_id = self.next_txn_id.saturating_add(1);

        let is_oneway = flags & FLAG_ONEWAY != 0;
        let txn = Transaction {
            id: txn_id,
            sender_pid,
            sender_euid: sender_pid, // simplified: euid == pid
            target_handle,
            code,
            flags,
            data,
            offsets: Vec::new(),
            reply_data: Vec::new(),
            state: if is_oneway {
                TransactionState::OneWay
            } else {
                TransactionState::Pending
            },
            timestamp: self.total_transactions,
        };

        self.transactions.insert(txn_id, txn);
        self.total_transactions = self.total_transactions.saturating_add(1);

        // Find the target process from the handle
        let target_pid = self.resolve_handle_to_pid(sender_pid, target_handle);
        if let Some(tpid) = target_pid {
            if let Some(target_state) = self.processes.get_mut(&tpid) {
                target_state.pending_txns.push(txn_id);
            }
        }

        // For synchronous calls, queue it on sender's reply queue too
        if !is_oneway {
            if let Some(sender_state) = self.processes.get_mut(&sender_pid) {
                sender_state.reply_queue.push(txn_id);
            }
        }

        Ok(txn_id)
    }

    /// Reply to a pending transaction
    pub fn reply(
        &mut self,
        pid: u32,
        txn_id: u64,
        reply_data: Vec<u8>,
    ) -> Result<(), &'static str> {
        let txn = self
            .transactions
            .get_mut(&txn_id)
            .ok_or("transaction not found")?;
        if txn.state != TransactionState::Pending && txn.state != TransactionState::InProgress {
            return Err("transaction not awaiting reply");
        }
        txn.reply_data = reply_data;
        txn.state = TransactionState::Replied;

        // Remove from target's pending
        if let Some(state) = self.processes.get_mut(&pid) {
            state.pending_txns.retain(|&id| id != txn_id);
        }

        Ok(())
    }

    /// Receive the next pending transaction for a process
    pub fn receive(&mut self, pid: u32) -> Option<Transaction> {
        let state = self.processes.get_mut(&pid)?;
        if state.pending_txns.is_empty() {
            return None;
        }
        let txn_id = state.pending_txns.remove(0);
        let txn = self.transactions.get_mut(&txn_id)?;
        if txn.state == TransactionState::Pending {
            txn.state = TransactionState::InProgress;
        }
        Some(txn.clone())
    }

    /// Check for a completed reply
    pub fn check_reply(&mut self, pid: u32, txn_id: u64) -> Option<Vec<u8>> {
        if let Some(txn) = self.transactions.get(&txn_id) {
            if txn.state == TransactionState::Replied {
                let data = txn.reply_data.clone();
                // Clean up
                self.transactions.remove(&txn_id);
                if let Some(state) = self.processes.get_mut(&pid) {
                    state.reply_queue.retain(|&id| id != txn_id);
                }
                return Some(data);
            }
        }
        None
    }

    /// Resolve a handle back to the owning PID
    fn resolve_handle_to_pid(&self, caller_pid: u32, handle: u32) -> Option<u32> {
        if handle == BINDER_HANDLE_SERVICE_MANAGER {
            // Service manager is always PID 1 (init)
            return Some(1);
        }
        let state = self.processes.get(&caller_pid)?;
        let bref = state.refs.get(&handle)?;
        Some(bref.owner_pid)
    }

    // -----------------------------------------------------------------------
    // Service manager operations
    // -----------------------------------------------------------------------

    /// Register a service with the service manager
    pub fn add_service(
        &mut self,
        name: &str,
        owner_pid: u32,
        node_ptr: u64,
        allow_isolated: bool,
    ) -> Result<u32, &'static str> {
        if self.services.len() >= MAX_SERVICES {
            return Err("service registry full");
        }
        if self.services.contains_key(name) {
            return Err("service already registered");
        }

        let handle = self.services.len() as u32 + 1;
        let entry = ServiceEntry {
            name: String::from(name),
            owner_pid,
            handle,
            node_ptr,
            allow_isolated,
            dump_priority: 0,
        };
        self.services.insert(String::from(name), entry);
        serial_println!(
            "    [binder] Service '{}' registered by PID {} (handle={})",
            name,
            owner_pid,
            handle
        );
        Ok(handle)
    }

    /// Look up a service by name
    pub fn get_service(&self, name: &str) -> Option<&ServiceEntry> {
        self.services.get(name)
    }

    /// List all registered services
    pub fn list_services(&self) -> Vec<&str> {
        self.services.keys().map(|k| k.as_str()).collect()
    }

    /// Check if a service is alive
    pub fn check_service(&self, name: &str) -> bool {
        if let Some(entry) = self.services.get(name) {
            self.processes.contains_key(&entry.owner_pid)
        } else {
            false
        }
    }

    // -----------------------------------------------------------------------
    // Death notifications
    // -----------------------------------------------------------------------

    /// Request a death notification when a binder node's owner dies
    pub fn request_death_notification(
        &mut self,
        observer_pid: u32,
        target_handle: u32,
        cookie: u64,
    ) -> Result<(), &'static str> {
        let state = self
            .processes
            .get_mut(&observer_pid)
            .ok_or("process not registered")?;
        if state.death_notifications.len() >= MAX_DEATH_OBSERVERS {
            return Err("too many death observers");
        }
        state.death_notifications.push(DeathNotification {
            observer_pid,
            target_handle,
            cookie,
            delivered: false,
        });
        Ok(())
    }

    /// Clear a previously registered death notification
    pub fn clear_death_notification(
        &mut self,
        observer_pid: u32,
        cookie: u64,
    ) -> Result<(), &'static str> {
        let state = self
            .processes
            .get_mut(&observer_pid)
            .ok_or("process not registered")?;
        state.death_notifications.retain(|n| n.cookie != cookie);
        Ok(())
    }

    /// Get pending death notifications for a process
    pub fn get_death_notifications(&self, pid: u32) -> Vec<&DeathNotification> {
        if let Some(state) = self.processes.get(&pid) {
            state
                .death_notifications
                .iter()
                .filter(|n| n.delivered)
                .collect()
        } else {
            Vec::new()
        }
    }

    // -----------------------------------------------------------------------
    // Reference counting
    // -----------------------------------------------------------------------

    /// Increment strong reference count on a handle
    pub fn acquire_ref(&mut self, pid: u32, handle: u32) -> Result<(), &'static str> {
        let state = self
            .processes
            .get_mut(&pid)
            .ok_or("process not registered")?;
        let bref = state.refs.get_mut(&handle).ok_or("handle not found")?;
        bref.strong_refs = bref.strong_refs.saturating_add(1);
        Ok(())
    }

    /// Decrement strong reference count on a handle
    pub fn release_ref(&mut self, pid: u32, handle: u32) -> Result<(), &'static str> {
        let state = self
            .processes
            .get_mut(&pid)
            .ok_or("process not registered")?;
        let bref = state.refs.get_mut(&handle).ok_or("handle not found")?;
        if bref.strong_refs == 0 {
            return Err("reference count underflow");
        }
        bref.strong_refs -= 1;
        if bref.strong_refs == 0 && bref.weak_refs == 0 {
            state.refs.remove(&handle);
        }
        Ok(())
    }

    /// Increment weak reference count
    pub fn inc_weak_ref(&mut self, pid: u32, handle: u32) -> Result<(), &'static str> {
        let state = self
            .processes
            .get_mut(&pid)
            .ok_or("process not registered")?;
        let bref = state.refs.get_mut(&handle).ok_or("handle not found")?;
        bref.weak_refs = bref.weak_refs.saturating_add(1);
        Ok(())
    }

    /// Decrement weak reference count
    pub fn dec_weak_ref(&mut self, pid: u32, handle: u32) -> Result<(), &'static str> {
        let state = self
            .processes
            .get_mut(&pid)
            .ok_or("process not registered")?;
        let bref = state.refs.get_mut(&handle).ok_or("handle not found")?;
        if bref.weak_refs == 0 {
            return Err("weak reference count underflow");
        }
        bref.weak_refs -= 1;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Statistics
    // -----------------------------------------------------------------------

    pub fn stats(&self) -> BinderStats {
        let mut total_nodes = 0;
        let mut total_refs = 0;
        for state in self.processes.values() {
            total_nodes += state.nodes.len();
            total_refs += state.refs.len();
        }
        BinderStats {
            processes: self.processes.len() as u32,
            services: self.services.len() as u32,
            total_nodes: total_nodes as u32,
            total_refs: total_refs as u32,
            active_transactions: self.transactions.len() as u32,
            total_transactions: self.total_transactions,
            total_failed: self.total_failed,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BinderStats {
    pub processes: u32,
    pub services: u32,
    pub total_nodes: u32,
    pub total_refs: u32,
    pub active_transactions: u32,
    pub total_transactions: u64,
    pub total_failed: u64,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn init() {
    let mut driver = BinderDriver::new();
    // Register init process (PID 1) as service manager host
    driver.open_process(1);
    *BINDER_STATE.lock() = Some(driver);
    serial_println!(
        "    [binder] Binder IPC driver initialized (max {} services, {} txns)",
        MAX_SERVICES,
        MAX_TRANSACTIONS
    );
}

pub fn open(pid: u32) {
    if let Some(ref mut driver) = *BINDER_STATE.lock() {
        driver.open_process(pid);
    }
}

pub fn close(pid: u32) {
    if let Some(ref mut driver) = *BINDER_STATE.lock() {
        driver.close_process(pid);
    }
}

pub fn transact(
    sender: u32,
    target_handle: u32,
    code: u32,
    flags: u32,
    data: Vec<u8>,
) -> Result<u64, &'static str> {
    BINDER_STATE
        .lock()
        .as_mut()
        .ok_or("not initialized")?
        .transact(sender, target_handle, code, flags, data)
}

pub fn reply(pid: u32, txn_id: u64, data: Vec<u8>) -> Result<(), &'static str> {
    BINDER_STATE
        .lock()
        .as_mut()
        .ok_or("not initialized")?
        .reply(pid, txn_id, data)
}

pub fn add_service(name: &str, owner: u32, node_ptr: u64) -> Result<u32, &'static str> {
    BINDER_STATE
        .lock()
        .as_mut()
        .ok_or("not initialized")?
        .add_service(name, owner, node_ptr, false)
}

pub fn get_service(name: &str) -> Result<u32, &'static str> {
    let guard = BINDER_STATE.lock();
    let driver = guard.as_ref().ok_or("not initialized")?;
    let entry = driver.get_service(name).ok_or("service not found")?;
    Ok(entry.handle)
}

pub fn request_death(observer: u32, handle: u32, cookie: u64) -> Result<(), &'static str> {
    BINDER_STATE
        .lock()
        .as_mut()
        .ok_or("not initialized")?
        .request_death_notification(observer, handle, cookie)
}
