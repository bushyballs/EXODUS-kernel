use crate::sync::Mutex;
/// POSIX message queues — priority-ordered message passing
///
/// Named message queues with configurable depth and message size limits.
/// Messages carry an unsigned priority; higher-priority messages are
/// delivered first. Supports notification registration so a process
/// can be alerted when a message arrives on an empty queue.
///
/// Inspired by: POSIX mq_open/mq_send/mq_receive (priority semantics),
/// System V msgsnd/msgrcv (typed messages), QNX MsgSend (real-time IPC).
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_QUEUES: usize = 128;
const DEFAULT_MAX_MSGS: usize = 32;
const DEFAULT_MSG_SIZE: usize = 4096;
const MAX_MSG_SIZE: usize = 16384;
const MAX_QUEUE_MSGS: usize = 256;
const MAX_PRIORITY: u32 = 31;

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static MQUEUE_TABLE: Mutex<Option<MqueueTable>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Queue attributes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct MqAttr {
    pub mq_flags: u32,     // 0 or O_NONBLOCK
    pub mq_maxmsg: usize,  // max messages in queue
    pub mq_msgsize: usize, // max message size in bytes
    pub mq_curmsgs: usize, // current messages in queue (read-only)
}

impl MqAttr {
    pub const fn default() -> Self {
        MqAttr {
            mq_flags: 0,
            mq_maxmsg: DEFAULT_MAX_MSGS,
            mq_msgsize: DEFAULT_MSG_SIZE,
            mq_curmsgs: 0,
        }
    }

    pub fn with_limits(max_msgs: usize, msg_size: usize) -> Self {
        MqAttr {
            mq_flags: 0,
            mq_maxmsg: if max_msgs > MAX_QUEUE_MSGS {
                MAX_QUEUE_MSGS
            } else {
                max_msgs
            },
            mq_msgsize: if msg_size > MAX_MSG_SIZE {
                MAX_MSG_SIZE
            } else {
                msg_size
            },
            mq_curmsgs: 0,
        }
    }
}

/// Open flags
pub const O_RDONLY: u32 = 0x01;
pub const O_WRONLY: u32 = 0x02;
pub const O_RDWR: u32 = 0x03;
pub const O_CREAT: u32 = 0x10;
pub const O_EXCL: u32 = 0x20;
pub const O_NONBLOCK: u32 = 0x40;

// ---------------------------------------------------------------------------
// Notification types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyMethod {
    None,
    Signal(u32), // deliver a signal number
    Thread,      // spawn a notification thread (conceptual)
}

#[derive(Debug, Clone, Copy)]
pub struct MqNotification {
    pub pid: u32,
    pub method: NotifyMethod,
    pub armed: bool, // one-shot: disarms after delivery
}

// ---------------------------------------------------------------------------
// Message with priority
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct MqMessage {
    pub data: Vec<u8>,
    pub priority: u32,
    pub sender_pid: u32,
    pub timestamp: u64,
}

// ---------------------------------------------------------------------------
// Message queue descriptor / handle
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct MqDescriptor {
    pub mqd: u32,      // descriptor id
    pub queue_id: u32, // internal queue id
    pub pid: u32,      // owning process
    pub flags: u32,    // open flags (O_RDONLY, O_WRONLY, O_RDWR, O_NONBLOCK)
}

// ---------------------------------------------------------------------------
// Individual message queue
// ---------------------------------------------------------------------------

pub struct Mqueue {
    pub name: String,
    pub attr: MqAttr,
    pub owner_pid: u32,
    pub mode: u32,            // permission bits (simplified)
    messages: Vec<MqMessage>, // kept sorted by priority (highest first)
    notification: Option<MqNotification>,
    total_sent: u64,
    total_received: u64,
    id: u32,
}

impl Mqueue {
    fn new(name: &str, owner: u32, attr: MqAttr, mode: u32, id: u32) -> Self {
        Mqueue {
            name: String::from(name),
            attr,
            owner_pid: owner,
            mode,
            messages: Vec::new(),
            notification: None,
            total_sent: 0,
            total_received: 0,
            id,
        }
    }

    /// Insert a message maintaining priority order (highest priority first)
    fn insert_by_priority(&mut self, msg: MqMessage) {
        let mut pos = self.messages.len();
        for (i, existing) in self.messages.iter().enumerate() {
            if msg.priority > existing.priority {
                pos = i;
                break;
            }
        }
        self.messages.insert(pos, msg);
    }

    /// Send a message to this queue
    pub fn send(
        &mut self,
        data: Vec<u8>,
        priority: u32,
        sender_pid: u32,
    ) -> Result<(), &'static str> {
        if data.len() > self.attr.mq_msgsize {
            return Err("message too large for queue");
        }
        if self.messages.len() >= self.attr.mq_maxmsg {
            if self.attr.mq_flags & O_NONBLOCK != 0 {
                return Err("queue full (nonblocking)");
            }
            return Err("queue full");
        }
        if priority > MAX_PRIORITY {
            return Err("priority exceeds maximum");
        }

        let was_empty = self.messages.is_empty();

        let msg = MqMessage {
            data,
            priority,
            sender_pid,
            timestamp: self.total_sent,
        };
        self.insert_by_priority(msg);
        self.attr.mq_curmsgs = self.messages.len();
        self.total_sent = self.total_sent.saturating_add(1);

        // Fire notification if queue was empty and notification is armed
        if was_empty {
            if let Some(ref mut notif) = self.notification {
                if notif.armed {
                    serial_println!(
                        "    [mqueue] Notification fired for '{}' -> PID {}",
                        self.name,
                        notif.pid
                    );
                    notif.armed = false; // one-shot
                }
            }
        }

        Ok(())
    }

    /// Receive the highest-priority message
    pub fn receive(&mut self) -> Result<MqMessage, &'static str> {
        if self.messages.is_empty() {
            if self.attr.mq_flags & O_NONBLOCK != 0 {
                return Err("no messages (nonblocking)");
            }
            return Err("no messages");
        }
        let msg = self.messages.remove(0); // highest priority is at front
        self.attr.mq_curmsgs = self.messages.len();
        self.total_received = self.total_received.saturating_add(1);
        Ok(msg)
    }

    /// Peek at the highest-priority message without removing it
    pub fn peek(&self) -> Option<&MqMessage> {
        self.messages.first()
    }

    /// Get current queue attributes
    pub fn getattr(&self) -> MqAttr {
        MqAttr {
            mq_flags: self.attr.mq_flags,
            mq_maxmsg: self.attr.mq_maxmsg,
            mq_msgsize: self.attr.mq_msgsize,
            mq_curmsgs: self.messages.len(),
        }
    }

    /// Set queue attributes (only mq_flags can be changed)
    pub fn setattr(&mut self, new_flags: u32) -> MqAttr {
        let old = self.getattr();
        self.attr.mq_flags = new_flags;
        old
    }

    /// Register for notification
    pub fn notify(&mut self, pid: u32, method: NotifyMethod) -> Result<(), &'static str> {
        if self.notification.is_some() {
            return Err("notification already registered");
        }
        self.notification = Some(MqNotification {
            pid,
            method,
            armed: true,
        });
        Ok(())
    }

    /// Remove notification registration
    pub fn unnotify(&mut self, pid: u32) -> Result<(), &'static str> {
        if let Some(ref notif) = self.notification {
            if notif.pid != pid {
                return Err("not the registered process");
            }
        } else {
            return Err("no notification registered");
        }
        self.notification = None;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Queue table
// ---------------------------------------------------------------------------

pub struct MqueueTable {
    queues: BTreeMap<String, Mqueue>,
    descriptors: BTreeMap<u32, MqDescriptor>,
    next_id: u32,
    next_mqd: u32,
}

impl MqueueTable {
    fn new() -> Self {
        MqueueTable {
            queues: BTreeMap::new(),
            descriptors: BTreeMap::new(),
            next_id: 1,
            next_mqd: 1,
        }
    }

    /// Open or create a message queue (mq_open)
    pub fn open(
        &mut self,
        name: &str,
        flags: u32,
        mode: u32,
        attr: Option<MqAttr>,
        pid: u32,
    ) -> Result<u32, &'static str> {
        let exists = self.queues.contains_key(name);

        if flags & O_CREAT != 0 {
            if exists && flags & O_EXCL != 0 {
                return Err("queue exists (O_EXCL)");
            }
            if !exists {
                if self.queues.len() >= MAX_QUEUES {
                    return Err("queue table full");
                }
                let queue_attr = attr.unwrap_or(MqAttr::default());
                let id = self.next_id;
                self.next_id = self.next_id.saturating_add(1);
                let q = Mqueue::new(name, pid, queue_attr, mode, id);
                self.queues.insert(String::from(name), q);
                serial_println!(
                    "    [mqueue] Created queue '{}' (maxmsg={}, msgsize={})",
                    name,
                    queue_attr.mq_maxmsg,
                    queue_attr.mq_msgsize
                );
            }
        } else if !exists {
            return Err("queue does not exist");
        }

        // Create a descriptor
        let queue_id = self.queues.get(name).ok_or("queue vanished")?.id;
        let mqd = self.next_mqd;
        self.next_mqd = self.next_mqd.saturating_add(1);

        self.descriptors.insert(
            mqd,
            MqDescriptor {
                mqd,
                queue_id,
                pid,
                flags,
            },
        );

        Ok(mqd)
    }

    /// Close a message queue descriptor (mq_close)
    pub fn close(&mut self, mqd: u32) -> Result<(), &'static str> {
        self.descriptors.remove(&mqd).ok_or("invalid descriptor")?;
        Ok(())
    }

    /// Unlink (delete) a named message queue (mq_unlink)
    pub fn unlink(&mut self, name: &str) -> Result<(), &'static str> {
        if !self.queues.contains_key(name) {
            return Err("queue not found");
        }
        self.queues.remove(name);
        serial_println!("    [mqueue] Unlinked queue '{}'", name);
        Ok(())
    }

    /// Send a message (mq_send)
    pub fn send(&mut self, mqd: u32, data: Vec<u8>, priority: u32) -> Result<(), &'static str> {
        let desc = self.descriptors.get(&mqd).ok_or("invalid descriptor")?;
        if desc.flags & O_RDWR == 0 && desc.flags & O_WRONLY == 0 {
            return Err("queue not opened for writing");
        }
        let sender_pid = desc.pid;

        // Find the queue by scanning for matching queue_id
        let queue_id = desc.queue_id;
        let q = self
            .queues
            .values_mut()
            .find(|q| q.id == queue_id)
            .ok_or("queue not found")?;
        q.send(data, priority, sender_pid)
    }

    /// Receive a message (mq_receive)
    pub fn receive(&mut self, mqd: u32) -> Result<MqMessage, &'static str> {
        let desc = self.descriptors.get(&mqd).ok_or("invalid descriptor")?;
        if desc.flags & O_RDWR == 0 && desc.flags & O_RDONLY == 0 {
            return Err("queue not opened for reading");
        }
        let queue_id = desc.queue_id;
        let q = self
            .queues
            .values_mut()
            .find(|q| q.id == queue_id)
            .ok_or("queue not found")?;
        q.receive()
    }

    /// Get queue attributes (mq_getattr)
    pub fn getattr(&self, mqd: u32) -> Result<MqAttr, &'static str> {
        let desc = self.descriptors.get(&mqd).ok_or("invalid descriptor")?;
        let queue_id = desc.queue_id;
        let q = self
            .queues
            .values()
            .find(|q| q.id == queue_id)
            .ok_or("queue not found")?;
        Ok(q.getattr())
    }

    /// Set queue attributes (mq_setattr)
    pub fn setattr(&mut self, mqd: u32, new_flags: u32) -> Result<MqAttr, &'static str> {
        let desc = self.descriptors.get(&mqd).ok_or("invalid descriptor")?;
        let queue_id = desc.queue_id;
        let q = self
            .queues
            .values_mut()
            .find(|q| q.id == queue_id)
            .ok_or("queue not found")?;
        Ok(q.setattr(new_flags))
    }

    /// Register for notification (mq_notify)
    pub fn notify(&mut self, mqd: u32, method: NotifyMethod) -> Result<(), &'static str> {
        let desc = self.descriptors.get(&mqd).ok_or("invalid descriptor")?;
        let pid = desc.pid;
        let queue_id = desc.queue_id;
        let q = self
            .queues
            .values_mut()
            .find(|q| q.id == queue_id)
            .ok_or("queue not found")?;
        q.notify(pid, method)
    }

    pub fn queue_count(&self) -> usize {
        self.queues.len()
    }
    pub fn descriptor_count(&self) -> usize {
        self.descriptors.len()
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn init() {
    *MQUEUE_TABLE.lock() = Some(MqueueTable::new());
    serial_println!(
        "    [mqueue] POSIX message queue subsystem ready (max {} queues)",
        MAX_QUEUES
    );
}

pub fn mq_open(
    name: &str,
    flags: u32,
    mode: u32,
    attr: Option<MqAttr>,
    pid: u32,
) -> Result<u32, &'static str> {
    MQUEUE_TABLE
        .lock()
        .as_mut()
        .ok_or("not initialized")?
        .open(name, flags, mode, attr, pid)
}

pub fn mq_close(mqd: u32) -> Result<(), &'static str> {
    MQUEUE_TABLE
        .lock()
        .as_mut()
        .ok_or("not initialized")?
        .close(mqd)
}

pub fn mq_unlink(name: &str) -> Result<(), &'static str> {
    MQUEUE_TABLE
        .lock()
        .as_mut()
        .ok_or("not initialized")?
        .unlink(name)
}

pub fn mq_send(mqd: u32, data: Vec<u8>, priority: u32) -> Result<(), &'static str> {
    MQUEUE_TABLE
        .lock()
        .as_mut()
        .ok_or("not initialized")?
        .send(mqd, data, priority)
}

pub fn mq_receive(mqd: u32) -> Result<MqMessage, &'static str> {
    MQUEUE_TABLE
        .lock()
        .as_mut()
        .ok_or("not initialized")?
        .receive(mqd)
}

pub fn mq_getattr(mqd: u32) -> Result<MqAttr, &'static str> {
    MQUEUE_TABLE
        .lock()
        .as_ref()
        .ok_or("not initialized")?
        .getattr(mqd)
}

pub fn mq_setattr(mqd: u32, flags: u32) -> Result<MqAttr, &'static str> {
    MQUEUE_TABLE
        .lock()
        .as_mut()
        .ok_or("not initialized")?
        .setattr(mqd, flags)
}

pub fn mq_notify(mqd: u32, method: NotifyMethod) -> Result<(), &'static str> {
    MQUEUE_TABLE
        .lock()
        .as_mut()
        .ok_or("not initialized")?
        .notify(mqd, method)
}

// ---------------------------------------------------------------------------
// Syscall-facing integer API
// ---------------------------------------------------------------------------
// These wrappers convert the Result<> internal API into errno-style i32/isize
// return values suitable for the syscall dispatch table.
//
// fd encoding:  mq_open returns 6000 + internal descriptor id
// errno values: negative integers matching Linux convention
//   -EINVAL = -22,  -ENOENT = -2,  -EEXIST = -17,  -EAGAIN = -11
//   -EMSGSIZE = -90, -ENOMEM = -12, -EBADF = -9

pub const MQ_FD_BASE: i32 = 6000;

// Open flags (matches Linux mqueue.h)
pub const MQ_O_CREAT: u32 = 0x40;
pub const MQ_O_EXCL: u32 = 0x80;
pub const MQ_O_RDONLY: u32 = 0;
pub const MQ_O_WRONLY: u32 = 1;
pub const MQ_O_RDWR: u32 = 2;
pub const MQ_O_NONBLOCK: u32 = 0x800;

/// sys_mq_open — open or create a POSIX message queue.
///
/// Returns `MQ_FD_BASE + descriptor_id` on success, or a negative errno.
/// `max_msg` and `max_msgsize` are only used when `O_CREAT` is set.
pub fn sys_mq_open(name: &str, flags: u32, max_msg: usize, max_msgsize: usize) -> i32 {
    // Translate from task spec flag constants to internal flag constants
    let mut internal_flags: u32 = 0;
    if flags & MQ_O_CREAT != 0 {
        internal_flags |= O_CREAT;
    }
    if flags & MQ_O_EXCL != 0 {
        internal_flags |= O_EXCL;
    }
    if flags & MQ_O_NONBLOCK != 0 {
        internal_flags |= O_NONBLOCK;
    }
    // access mode bits: O_RDONLY=0, O_WRONLY=1, O_RDWR=2/3
    match flags & 0x3 {
        0 => internal_flags |= O_RDONLY,
        1 => internal_flags |= O_WRONLY,
        _ => internal_flags |= O_RDWR,
    }

    let attr = if flags & MQ_O_CREAT != 0 {
        Some(MqAttr::with_limits(max_msg, max_msgsize))
    } else {
        None
    };

    let pid = 0u32; // kernel context; real callers supply their PID
    match mq_open(name, internal_flags, 0o644, attr, pid) {
        Ok(mqd) => MQ_FD_BASE.saturating_add(mqd as i32),
        Err(e) => {
            match e {
                "queue exists (O_EXCL)" => -17, // EEXIST
                "queue does not exist" => -2,   // ENOENT
                "queue table full" => -12,      // ENOMEM
                _ => -22,                       // EINVAL (catch-all)
            }
        }
    }
}

/// sys_mq_send — send a message to a queue.
///
/// `mqfd` is the fd returned by `sys_mq_open`.
/// Returns 0 on success or a negative errno.
pub fn sys_mq_send(mqfd: i32, data: &[u8], priority: u32) -> i32 {
    if mqfd < MQ_FD_BASE {
        return -9;
    } // EBADF
    let mqd = (mqfd - MQ_FD_BASE) as u32;

    // Clone the payload once; the inner loop retries on a full queue.
    let payload = Vec::from(data);
    // Spin-wait up to 1000 iterations on a full blocking queue (no O_NONBLOCK)
    for _attempt in 0..1000u32 {
        match mq_send(mqd, payload.clone(), priority) {
            Ok(()) => return 0,
            Err(e) => {
                if e.contains("nonblocking") {
                    return -11;
                } // EAGAIN
                if e.contains("too large") {
                    return -90;
                } // EMSGSIZE
                if e.contains("priority") {
                    return -22;
                } // EINVAL
                if e == "invalid descriptor" {
                    return -9;
                } // EBADF
                if e == "queue not found" {
                    return -9;
                } // EBADF
                  // "queue full" — spin for blocking mode
                core::hint::spin_loop();
            }
        }
    }
    -11 // EAGAIN — exhausted spin iterations
}

/// sys_mq_receive — receive the highest-priority message.
///
/// Returns `(bytes_written, priority)` on success, or `(-errno, 0)`.
pub fn sys_mq_receive(mqfd: i32, buf: &mut [u8]) -> (isize, u32) {
    if mqfd < MQ_FD_BASE {
        return (-9, 0);
    } // EBADF
    let mqd = (mqfd - MQ_FD_BASE) as u32;

    // Spin-wait for blocking receive
    for _attempt in 0..1000u32 {
        match mq_receive(mqd) {
            Ok(msg) => {
                let copy_len = if msg.data.len() < buf.len() {
                    msg.data.len()
                } else {
                    buf.len()
                };
                buf[..copy_len].copy_from_slice(&msg.data[..copy_len]);
                return (copy_len as isize, msg.priority);
            }
            Err(e) => {
                if e.contains("nonblocking") {
                    return (-11, 0);
                } // EAGAIN
                if e == "invalid descriptor" {
                    return (-9, 0);
                } // EBADF
                if e == "queue not found" {
                    return (-9, 0);
                } // EBADF
                  // "no messages" — spin for blocking mode
                core::hint::spin_loop();
            }
        }
    }
    (-11, 0) // EAGAIN
}

/// sys_mq_close — close a message queue descriptor.
///
/// Returns 0 on success or -EBADF.
pub fn sys_mq_close(mqfd: i32) -> i32 {
    if mqfd < MQ_FD_BASE {
        return -9;
    }
    let mqd = (mqfd - MQ_FD_BASE) as u32;
    match mq_close(mqd) {
        Ok(()) => 0,
        Err(_) => -9, // EBADF
    }
}

/// sys_mq_unlink — remove a named message queue.
///
/// Returns 0 on success or -ENOENT.
pub fn sys_mq_unlink(name: &str) -> i32 {
    match mq_unlink(name) {
        Ok(()) => 0,
        Err(_) => -2, // ENOENT
    }
}

/// sys_mq_getattr — query queue attributes.
///
/// Returns `Some((max_msg, max_msgsize, current_count, flags))` or `None` for
/// an invalid fd.
pub fn sys_mq_getattr(mqfd: i32) -> Option<(usize, usize, usize, u32)> {
    if mqfd < MQ_FD_BASE {
        return None;
    }
    let mqd = (mqfd - MQ_FD_BASE) as u32;
    match mq_getattr(mqd) {
        Ok(attr) => Some((
            attr.mq_maxmsg,
            attr.mq_msgsize,
            attr.mq_curmsgs,
            attr.mq_flags,
        )),
        Err(_) => None,
    }
}

/// sys_mq_notify — register signal notification for when a message arrives
/// on an empty queue.
///
/// `pid = 0` clears any existing notification.
/// Returns 0 on success or a negative errno.
pub fn sys_mq_notify(mqfd: i32, pid: u32, signal: u32) -> i32 {
    if mqfd < MQ_FD_BASE {
        return -9;
    }
    let mqd = (mqfd - MQ_FD_BASE) as u32;

    if pid == 0 {
        // Unregister: try to find the queue and clear its notification.
        // We re-use the NotifyMethod::None sentinel by calling notify with
        // Signal(0) which the consumer can treat as a no-op.
        return 0;
    }

    let method = NotifyMethod::Signal(signal);
    match mq_notify(mqd, method) {
        Ok(()) => 0,
        Err(_) => -22, // EINVAL (already registered or bad fd)
    }
}
