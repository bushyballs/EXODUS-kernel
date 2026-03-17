/// inbox.rs — static per-user inbox with fixed-size message structs.
///
/// Provides a bare-metal, no-alloc inbox backed by a static array of
/// 64 `Message` slots.  All string fields use fixed-size `[u8; N]`
/// arrays.  Messages are addressed by a simple slot index (0-63).
///
/// Thread model: conversations are identified by the sorted pair
/// (sender, recipient) encoded as a `u64` key.  `get_messages` returns
/// the full inbox slice so callers can filter by `from`/`to` themselves.
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Number of inbox slots.
pub const INBOX_SIZE: usize = 64;

/// Fixed maximum length of the `from` and `to` fields (bytes).
pub const ADDR_LEN: usize = 32;

/// Fixed maximum length of the message body (bytes).
pub const BODY_LEN: usize = 512;

// ---------------------------------------------------------------------------
// Message
// ---------------------------------------------------------------------------

/// A single message in the inbox.
#[derive(Clone, Copy)]
pub struct Message {
    /// Sender address / username, null-padded.
    pub from: [u8; ADDR_LEN],
    pub from_len: usize,

    /// Recipient address / username, null-padded.
    pub to: [u8; ADDR_LEN],
    pub to_len: usize,

    /// Message body, null-padded.
    pub body: [u8; BODY_LEN],
    pub body_len: usize,

    /// Unix-style timestamp (seconds since epoch, or kernel tick).
    pub timestamp: u64,

    /// Whether the recipient has read this message.
    pub read: bool,

    /// Whether this slot is occupied.
    pub valid: bool,
}

impl Message {
    /// Construct an empty, invalid message.
    pub const fn empty() -> Self {
        Self {
            from: [0u8; ADDR_LEN],
            from_len: 0,
            to: [0u8; ADDR_LEN],
            to_len: 0,
            body: [0u8; BODY_LEN],
            body_len: 0,
            timestamp: 0,
            read: false,
            valid: false,
        }
    }

    /// Return `from` as a `&str`.
    pub fn from_str(&self) -> &str {
        core::str::from_utf8(&self.from[..self.from_len]).unwrap_or("")
    }

    /// Return `to` as a `&str`.
    pub fn to_str(&self) -> &str {
        core::str::from_utf8(&self.to[..self.to_len]).unwrap_or("")
    }

    /// Return `body` as a `&str`.
    pub fn body_str(&self) -> &str {
        core::str::from_utf8(&self.body[..self.body_len]).unwrap_or("")
    }

    /// Derive a conversation key for the pair (sender, recipient).
    ///
    /// The key is order-independent: `conv_key(a, b) == conv_key(b, a)`.
    pub fn conv_key(a: &[u8], b: &[u8]) -> u64 {
        let ha = fnv(a);
        let hb = fnv(b);
        // XOR is commutative, add the individual hashes for asymmetry.
        (ha ^ hb).wrapping_add(ha.wrapping_add(hb))
    }

    /// Compute the conversation key for this message.
    pub fn this_conv_key(&self) -> u64 {
        Self::conv_key(&self.from[..self.from_len], &self.to[..self.to_len])
    }
}

// ---------------------------------------------------------------------------
// FNV-1a helper
// ---------------------------------------------------------------------------

fn fnv(data: &[u8]) -> u64 {
    let mut h: u64 = 0xCBF2_9CE4_8422_2325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

// ---------------------------------------------------------------------------
// Static inbox
// ---------------------------------------------------------------------------

static mut INBOX: [Option<Message>; INBOX_SIZE] = [None; INBOX_SIZE];
static mut MSG_COUNT: usize = 0;

// ---------------------------------------------------------------------------
// send_message
// ---------------------------------------------------------------------------

/// Place a message into the inbox.
///
/// `to` is the recipient address, `body` is the message text.
/// The kernel's current-user context is represented by the caller
/// supplying an explicit `from` string.
///
/// Returns `true` on success, `false` if the inbox is full or the
/// inputs exceed their respective maximum lengths.
pub fn send_message(from: &str, to: &str, body: &str, timestamp: u64) -> bool {
    if from.len() > ADDR_LEN || to.len() > ADDR_LEN || body.len() > BODY_LEN {
        serial_println!("[inbox] send_message: field too long");
        return false;
    }

    unsafe {
        for slot in 0..INBOX_SIZE {
            if INBOX[slot].is_none() {
                let mut msg = Message::empty();
                msg.valid = true;
                msg.timestamp = timestamp;

                let fl = from.len();
                msg.from[..fl].copy_from_slice(from.as_bytes());
                msg.from_len = fl;

                let tl = to.len();
                msg.to[..tl].copy_from_slice(to.as_bytes());
                msg.to_len = tl;

                let bl = body.len();
                msg.body[..bl].copy_from_slice(body.as_bytes());
                msg.body_len = bl;

                INBOX[slot] = Some(msg);
                MSG_COUNT = MSG_COUNT.saturating_add(1);
                serial_println!("[inbox] send_message slot={} from={} to={}", slot, from, to);
                return true;
            }
        }
    }
    serial_println!("[inbox] send_message: inbox full");
    false
}

// ---------------------------------------------------------------------------
// get_messages
// ---------------------------------------------------------------------------

/// Return a reference to the entire inbox slice.
///
/// Callers can iterate and filter by `msg.to_str() == user` to obtain
/// messages for a specific recipient.
pub fn get_messages() -> &'static [Option<Message>] {
    unsafe { &INBOX }
}

/// Return the number of messages addressed to `user` (read or unread).
pub fn message_count_for(user: &str) -> usize {
    let needle = user.as_bytes();
    let mut count = 0usize;
    unsafe {
        for slot in INBOX.iter().flatten() {
            if &slot.to[..slot.to_len] == needle {
                count += 1;
            }
        }
    }
    count
}

/// Return the number of **unread** messages addressed to `user`.
pub fn unread_count_for(user: &str) -> usize {
    let needle = user.as_bytes();
    let mut count = 0usize;
    unsafe {
        for slot in INBOX.iter().flatten() {
            if &slot.to[..slot.to_len] == needle && !slot.read {
                count += 1;
            }
        }
    }
    count
}

// ---------------------------------------------------------------------------
// mark_read
// ---------------------------------------------------------------------------

/// Mark the message at `id` (slot index 0-63) as read.
///
/// Returns `true` if the slot contained a valid message.
pub fn mark_read(id: usize) -> bool {
    if id >= INBOX_SIZE {
        return false;
    }
    unsafe {
        if let Some(ref mut msg) = INBOX[id] {
            msg.read = true;
            serial_println!("[inbox] mark_read id={}", id);
            return true;
        }
    }
    false
}

/// Mark **all** messages addressed to `user` as read.
///
/// Returns the number of messages updated.
pub fn mark_all_read(user: &str) -> usize {
    let needle = user.as_bytes();
    let mut count = 0usize;
    unsafe {
        for slot in INBOX.iter_mut().flatten() {
            if &slot.to[..slot.to_len] == needle && !slot.read {
                slot.read = true;
                count += 1;
            }
        }
    }
    serial_println!("[inbox] mark_all_read user={} updated={}", user, count);
    count
}

// ---------------------------------------------------------------------------
// Thread (conversation) helpers
// ---------------------------------------------------------------------------

/// Return the slot indices of all messages that belong to the conversation
/// between `user_a` and `user_b` (order-independent), sorted by timestamp.
///
/// Because we work with a static array and no allocator, callers should
/// provide a mutable slice to receive the results.  Returns the number of
/// slots written.
pub fn get_thread(user_a: &str, user_b: &str, out: &mut [usize]) -> usize {
    let key = Message::conv_key(user_a.as_bytes(), user_b.as_bytes());
    let mut written = 0usize;
    // Two-pass: collect indices then insertion-sort by timestamp.
    let mut indices = [0usize; INBOX_SIZE];
    let mut count = 0usize;
    unsafe {
        for (slot, maybe) in INBOX.iter().enumerate() {
            if let Some(ref msg) = maybe {
                if msg.this_conv_key() == key {
                    indices[count] = slot;
                    count += 1;
                }
            }
        }
    }
    // Insertion sort by ascending timestamp.
    for i in 1..count {
        let mut j = i;
        while j > 0 {
            let ts_j = unsafe { INBOX[indices[j]].as_ref().map_or(0, |m| m.timestamp) };
            let ts_jm1 = unsafe { INBOX[indices[j - 1]].as_ref().map_or(0, |m| m.timestamp) };
            if ts_j < ts_jm1 {
                indices.swap(j, j - 1);
                j -= 1;
            } else {
                break;
            }
        }
    }
    // Copy into `out`.
    for i in 0..count.min(out.len()) {
        out[written] = indices[i];
        written += 1;
    }
    written
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("[inbox] static inbox ready (slots={})", INBOX_SIZE);
}
