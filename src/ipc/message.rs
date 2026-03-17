use crate::sync::Mutex;
/// Message queues — typed message passing between processes
///
/// Processes can create named message queues and send/receive
/// structured messages. Messages are copied (no shared memory).
/// Queue depth is bounded to prevent memory exhaustion.
use crate::{serial_print, serial_println};
use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;

static MSG_QUEUES: Mutex<Option<BTreeMap<String, MessageQueue>>> = Mutex::new(None);

const MAX_QUEUE_DEPTH: usize = 64;
const MAX_MSG_SIZE: usize = 4096;

#[derive(Debug, Clone)]
pub struct Message {
    pub sender: u32,
    pub msg_type: u32,
    pub data: Vec<u8>,
    pub timestamp: u64,
}

pub struct MessageQueue {
    pub name: String,
    pub owner: u32,
    pub messages: VecDeque<Message>,
    pub max_depth: usize,
    pub msg_count: u64,
}

impl MessageQueue {
    pub fn new(name: &str, owner: u32) -> Self {
        MessageQueue {
            name: String::from(name),
            owner,
            messages: VecDeque::new(),
            max_depth: MAX_QUEUE_DEPTH,
            msg_count: 0,
        }
    }

    pub fn send(&mut self, sender: u32, msg_type: u32, data: Vec<u8>) -> Result<(), &'static str> {
        if data.len() > MAX_MSG_SIZE {
            return Err("message too large");
        }
        if self.messages.len() >= self.max_depth {
            return Err("queue full");
        }
        self.messages.push_back(Message {
            sender,
            msg_type,
            data,
            timestamp: self.msg_count,
        });
        self.msg_count = self.msg_count.saturating_add(1);
        Ok(())
    }

    pub fn recv(&mut self) -> Option<Message> {
        self.messages.pop_front()
    }

    pub fn peek(&self) -> Option<&Message> {
        self.messages.front()
    }

    pub fn pending(&self) -> usize {
        self.messages.len()
    }
}

pub fn init() {
    *MSG_QUEUES.lock() = Some(BTreeMap::new());
    serial_println!("    [msg] Message queue subsystem ready");
}

pub fn create_queue(name: &str, owner: u32) -> Result<(), &'static str> {
    let mut guard = MSG_QUEUES.lock();
    let queues = guard.as_mut().ok_or("not initialized")?;
    if queues.contains_key(name) {
        return Err("queue exists");
    }
    queues.insert(String::from(name), MessageQueue::new(name, owner));
    Ok(())
}

pub fn send(
    queue_name: &str,
    sender: u32,
    msg_type: u32,
    data: Vec<u8>,
) -> Result<(), &'static str> {
    let mut guard = MSG_QUEUES.lock();
    let queues = guard.as_mut().ok_or("not initialized")?;
    let q = queues.get_mut(queue_name).ok_or("queue not found")?;
    q.send(sender, msg_type, data)
}

pub fn recv(queue_name: &str) -> Result<Option<Message>, &'static str> {
    let mut guard = MSG_QUEUES.lock();
    let queues = guard.as_mut().ok_or("not initialized")?;
    let q = queues.get_mut(queue_name).ok_or("queue not found")?;
    Ok(q.recv())
}

/// Peek at the next pending message without removing it.
/// Returns a clone of the front message if any exist.
pub fn peek(queue_name: &str) -> Result<Option<Message>, &'static str> {
    let guard = MSG_QUEUES.lock();
    let queues = guard.as_ref().ok_or("not initialized")?;
    let q = queues.get(queue_name).ok_or("queue not found")?;
    Ok(q.peek().cloned())
}

/// Destroy a message queue, discarding all pending messages.
/// Only the owner or a privileged caller should be allowed to destroy;
/// this implementation does not enforce ownership — callers are responsible.
pub fn destroy_queue(queue_name: &str) -> Result<(), &'static str> {
    let mut guard = MSG_QUEUES.lock();
    let queues = guard.as_mut().ok_or("not initialized")?;
    if queues.remove(queue_name).is_none() {
        return Err("queue not found");
    }
    Ok(())
}

/// Return the number of pending messages in the named queue.
pub fn pending_count(queue_name: &str) -> Result<usize, &'static str> {
    let guard = MSG_QUEUES.lock();
    let queues = guard.as_ref().ok_or("not initialized")?;
    let q = queues.get(queue_name).ok_or("queue not found")?;
    Ok(q.pending())
}

/// Return the total number of messages ever enqueued (monotonic counter).
pub fn total_enqueued(queue_name: &str) -> Result<u64, &'static str> {
    let guard = MSG_QUEUES.lock();
    let queues = guard.as_ref().ok_or("not initialized")?;
    let q = queues.get(queue_name).ok_or("queue not found")?;
    Ok(q.msg_count)
}

/// Return the number of active (non-empty or otherwise existing) queues.
pub fn queue_count() -> usize {
    MSG_QUEUES.lock().as_ref().map(|m| m.len()).unwrap_or(0)
}

/// Check whether a queue with the given name exists.
pub fn queue_exists(queue_name: &str) -> bool {
    MSG_QUEUES
        .lock()
        .as_ref()
        .map(|m| m.contains_key(queue_name))
        .unwrap_or(false)
}

/// Drain all messages from a queue, returning them as a Vec.
/// The queue itself is NOT removed — it remains open for new messages.
pub fn drain(queue_name: &str) -> Result<Vec<Message>, &'static str> {
    let mut guard = MSG_QUEUES.lock();
    let queues = guard.as_mut().ok_or("not initialized")?;
    let q = queues.get_mut(queue_name).ok_or("queue not found")?;
    let mut out = Vec::new();
    while let Some(msg) = q.recv() {
        out.push(msg);
    }
    Ok(out)
}

/// Send a message using a pre-allocated Vec<u8>.  Convenience wrapper that
/// avoids an extra clone when the caller already owns the buffer.
pub fn send_owned(
    queue_name: &str,
    sender: u32,
    msg_type: u32,
    data: Vec<u8>,
) -> Result<(), &'static str> {
    let mut guard = MSG_QUEUES.lock();
    let queues = guard.as_mut().ok_or("not initialized")?;
    let q = queues.get_mut(queue_name).ok_or("queue not found")?;
    q.send(sender, msg_type, data)
}
