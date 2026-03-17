use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec;
/// End-to-end encrypted direct messaging for Genesis OS
///
/// Provides:
///   - Message send / receive with encrypted payloads
///   - X25519 key-exchange simulation (no external crate)
///   - ChaCha20 encrypt / decrypt stubs
///   - Per-conversation state with read receipts and deletion
///   - Conversation history management
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum messages retained per conversation before oldest are pruned.
const MAX_MESSAGES_PER_CONVERSATION: usize = 4096;

/// Fixed key length (32 bytes = 256 bits) for simulated X25519 / ChaCha20.
const KEY_LEN: usize = 32;

/// Nonce length for ChaCha20 (96 bits / 12 bytes).
const NONCE_LEN: usize = 12;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Delivery status of a single message.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DeliveryStatus {
    Sent,
    Delivered,
    Read,
    Failed,
}

/// A single encrypted chat message.
#[derive(Clone)]
pub struct Message {
    pub id: u64,
    pub sender_hash: u64,
    pub recipient_hash: u64,
    pub content_hash: u64,
    pub encrypted_payload: Vec<u8>,
    pub timestamp: u64,
    pub read: bool,
    pub delivered: bool,
    pub status: DeliveryStatus,
}

/// An ongoing conversation between two participants.
pub struct Conversation {
    pub id: u64,
    pub participants: (u64, u64),
    pub messages: Vec<Message>,
    pub last_activity: u64,
    pub shared_key: Vec<u8>,
}

/// Simulated X25519 keypair (private scalar + public point, both 32 bytes).
pub struct KeyPair {
    pub private_key: Vec<u8>,
    pub public_key: Vec<u8>,
}

/// Top-level manager for all direct conversations.
pub struct ChatManager {
    conversations: Vec<Conversation>,
    next_msg_id: u64,
    next_conv_id: u64,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static CHAT_MANAGER: Mutex<Option<ChatManager>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Simulated cryptography helpers
// ---------------------------------------------------------------------------

/// Generate a deterministic but non-trivial pseudo-random byte from a seed.
/// Uses a simple xorshift-style mixing so we never need f32/f64.
fn prng_byte(seed: u64, index: usize) -> u8 {
    let mut s = seed.wrapping_add(index as u64);
    s ^= s << 13;
    s ^= s >> 7;
    s ^= s << 17;
    (s & 0xFF) as u8
}

/// Generate a pseudo-random key of `len` bytes seeded by `seed`.
fn generate_key(seed: u64, len: usize) -> Vec<u8> {
    let mut key = Vec::with_capacity(len);
    for i in 0..len {
        key.push(prng_byte(seed, i));
    }
    key
}

/// Simulate an X25519 key-exchange.
///
/// Produces a `KeyPair` whose "private key" is derived from the user hash and
/// whose "public key" is a deterministic transform of the private key.
/// This is purely illustrative -- real X25519 would use Curve25519 scalar
/// multiplication.
pub fn x25519_generate_keypair(user_hash: u64) -> KeyPair {
    let private_key = generate_key(user_hash, KEY_LEN);
    // "Public key" = each byte of the private key XORed with 0xAA and rotated
    let mut public_key = Vec::with_capacity(KEY_LEN);
    for i in 0..KEY_LEN {
        let b = private_key[i];
        public_key.push(b ^ 0xAA);
    }
    KeyPair {
        private_key,
        public_key,
    }
}

/// Derive a shared secret from our private key and the peer's public key.
///
/// Real X25519 performs scalar * point on Curve25519.  Here we just XOR
/// corresponding bytes (plus mixing) to produce a deterministic 32-byte key
/// that both sides can independently compute.
pub fn x25519_shared_secret(private_key: &[u8], peer_public: &[u8]) -> Vec<u8> {
    let len = if private_key.len() < peer_public.len() {
        private_key.len()
    } else {
        peer_public.len()
    };
    let mut secret = Vec::with_capacity(len);
    for i in 0..len {
        let mixed = private_key[i].wrapping_add(peer_public[i]) ^ 0x5C;
        secret.push(mixed);
    }
    secret
}

/// ChaCha20 encrypt stub.
///
/// XORs each plaintext byte with a keystream byte derived from the key and a
/// nonce.  This is NOT cryptographically secure -- it is a structural
/// placeholder for a real ChaCha20 implementation.
pub fn encrypt_payload(plaintext: &[u8], key: &[u8], nonce_seed: u64) -> Vec<u8> {
    let nonce = generate_key(nonce_seed, NONCE_LEN);
    let mut ciphertext = Vec::with_capacity(NONCE_LEN + plaintext.len());
    // Prepend nonce so the receiver can extract it.
    for b in &nonce {
        ciphertext.push(*b);
    }
    for (i, &byte) in plaintext.iter().enumerate() {
        let ki = i % key.len();
        let ni = i % NONCE_LEN;
        let keystream = key[ki].wrapping_add(nonce[ni]).wrapping_add(i as u8);
        ciphertext.push(byte ^ keystream);
    }
    ciphertext
}

/// ChaCha20 decrypt stub (symmetric with `encrypt_payload`).
pub fn decrypt_payload(ciphertext: &[u8], key: &[u8]) -> Vec<u8> {
    if ciphertext.len() <= NONCE_LEN {
        return vec![];
    }
    let nonce = &ciphertext[..NONCE_LEN];
    let body = &ciphertext[NONCE_LEN..];
    let mut plaintext = Vec::with_capacity(body.len());
    for (i, &byte) in body.iter().enumerate() {
        let ki = i % key.len();
        let ni = i % NONCE_LEN;
        let keystream = key[ki].wrapping_add(nonce[ni]).wrapping_add(i as u8);
        plaintext.push(byte ^ keystream);
    }
    plaintext
}

/// Compute a simple non-cryptographic hash of a byte slice.
/// Used for content_hash bookkeeping (not security).
fn hash_bytes(data: &[u8]) -> u64 {
    let mut h: u64 = 0xCBF29CE484222325; // FNV offset basis
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001B3); // FNV prime
    }
    h
}

// ---------------------------------------------------------------------------
// ChatManager implementation
// ---------------------------------------------------------------------------

impl ChatManager {
    pub fn new() -> Self {
        Self {
            conversations: vec![],
            next_msg_id: 1,
            next_conv_id: 1,
        }
    }

    /// Find or create a `Conversation` between two user hashes.
    /// Returns the conversation id.
    pub fn get_or_create_conversation(&mut self, user_a: u64, user_b: u64) -> u64 {
        // Check if conversation already exists (order-independent).
        for conv in &self.conversations {
            let (p0, p1) = conv.participants;
            if (p0 == user_a && p1 == user_b) || (p0 == user_b && p1 == user_a) {
                return conv.id;
            }
        }
        // Derive a shared key via simulated X25519.
        let kp_a = x25519_generate_keypair(user_a);
        let kp_b = x25519_generate_keypair(user_b);
        let shared = x25519_shared_secret(&kp_a.private_key, &kp_b.public_key);

        let id = self.next_conv_id;
        self.next_conv_id = self.next_conv_id.saturating_add(1);

        self.conversations.push(Conversation {
            id,
            participants: (user_a, user_b),
            messages: vec![],
            last_activity: 0,
            shared_key: shared,
        });
        id
    }

    /// Find a conversation by its id.
    fn find_conversation_mut(&mut self, conv_id: u64) -> Option<&mut Conversation> {
        self.conversations.iter_mut().find(|c| c.id == conv_id)
    }

    /// Send an encrypted message within a conversation.
    pub fn send_message(
        &mut self,
        conv_id: u64,
        sender_hash: u64,
        recipient_hash: u64,
        plaintext: &[u8],
        timestamp: u64,
    ) -> Option<u64> {
        // Clone the shared key so we don't hold two borrows at once.
        let shared_key = {
            let conv = self.find_conversation_mut(conv_id)?;
            conv.shared_key.clone()
        };

        let content_hash = hash_bytes(plaintext);
        let nonce_seed = timestamp.wrapping_add(sender_hash);
        let encrypted = encrypt_payload(plaintext, &shared_key, nonce_seed);

        let msg_id = self.next_msg_id;
        self.next_msg_id = self.next_msg_id.saturating_add(1);

        let msg = Message {
            id: msg_id,
            sender_hash,
            recipient_hash,
            content_hash,
            encrypted_payload: encrypted,
            timestamp,
            read: false,
            delivered: false,
            status: DeliveryStatus::Sent,
        };

        let conv = self.find_conversation_mut(conv_id)?;
        conv.messages.push(msg);
        conv.last_activity = timestamp;

        // Prune oldest if over limit.
        if conv.messages.len() > MAX_MESSAGES_PER_CONVERSATION {
            let excess = conv.messages.len() - MAX_MESSAGES_PER_CONVERSATION;
            conv.messages.drain(0..excess);
        }

        serial_println!("[e2e_chat] sent msg {} in conv {}", msg_id, conv_id);
        Some(msg_id)
    }

    /// Simulate receiving a message -- marks it as delivered.
    pub fn receive_message(&mut self, conv_id: u64, msg_id: u64) -> bool {
        if let Some(conv) = self.find_conversation_mut(conv_id) {
            for m in conv.messages.iter_mut() {
                if m.id == msg_id {
                    m.delivered = true;
                    m.status = DeliveryStatus::Delivered;
                    serial_println!("[e2e_chat] msg {} delivered", msg_id);
                    return true;
                }
            }
        }
        false
    }

    /// Mark a message as read.
    pub fn mark_read(&mut self, conv_id: u64, msg_id: u64) -> bool {
        if let Some(conv) = self.find_conversation_mut(conv_id) {
            for m in conv.messages.iter_mut() {
                if m.id == msg_id {
                    m.read = true;
                    m.status = DeliveryStatus::Read;
                    serial_println!("[e2e_chat] msg {} marked read", msg_id);
                    return true;
                }
            }
        }
        false
    }

    /// Delete a message from a conversation.
    pub fn delete_message(&mut self, conv_id: u64, msg_id: u64) -> bool {
        if let Some(conv) = self.find_conversation_mut(conv_id) {
            let before = conv.messages.len();
            conv.messages.retain(|m| m.id != msg_id);
            if conv.messages.len() < before {
                serial_println!("[e2e_chat] msg {} deleted from conv {}", msg_id, conv_id);
                return true;
            }
        }
        false
    }

    /// Decrypt a message's payload using the conversation's shared key.
    pub fn decrypt_message(&self, conv_id: u64, msg_id: u64) -> Option<Vec<u8>> {
        let conv = self.conversations.iter().find(|c| c.id == conv_id)?;
        let msg = conv.messages.iter().find(|m| m.id == msg_id)?;
        let plaintext = decrypt_payload(&msg.encrypted_payload, &conv.shared_key);
        Some(plaintext)
    }

    /// Get the number of unread messages in a conversation for a given user.
    pub fn unread_count(&self, conv_id: u64, user_hash: u64) -> usize {
        if let Some(conv) = self.conversations.iter().find(|c| c.id == conv_id) {
            conv.messages
                .iter()
                .filter(|m| m.recipient_hash == user_hash && !m.read)
                .count()
        } else {
            0
        }
    }

    /// List conversation ids for a given user.
    pub fn conversations_for_user(&self, user_hash: u64) -> Vec<u64> {
        let mut result = vec![];
        for conv in &self.conversations {
            let (a, b) = conv.participants;
            if a == user_hash || b == user_hash {
                result.push(conv.id);
            }
        }
        result
    }

    /// Return total message count across all conversations.
    pub fn total_message_count(&self) -> usize {
        let mut total: usize = 0;
        for conv in &self.conversations {
            total += conv.messages.len();
        }
        total
    }

    /// Mark all messages in a conversation as read for a given recipient.
    pub fn mark_all_read(&mut self, conv_id: u64, user_hash: u64) -> usize {
        let mut count: usize = 0;
        if let Some(conv) = self.find_conversation_mut(conv_id) {
            for m in conv.messages.iter_mut() {
                if m.recipient_hash == user_hash && !m.read {
                    m.read = true;
                    m.status = DeliveryStatus::Read;
                    count += 1;
                }
            }
        }
        count
    }

    /// Delete an entire conversation.
    pub fn delete_conversation(&mut self, conv_id: u64) -> bool {
        let before = self.conversations.len();
        self.conversations.retain(|c| c.id != conv_id);
        self.conversations.len() < before
    }
}

// ---------------------------------------------------------------------------
// Public API (through the global mutex)
// ---------------------------------------------------------------------------

/// Send a message between two users, creating the conversation if needed.
pub fn send_message(
    sender_hash: u64,
    recipient_hash: u64,
    plaintext: &[u8],
    timestamp: u64,
) -> Option<(u64, u64)> {
    let mut guard = CHAT_MANAGER.lock();
    let mgr = guard.as_mut()?;
    let conv_id = mgr.get_or_create_conversation(sender_hash, recipient_hash);
    let msg_id = mgr.send_message(conv_id, sender_hash, recipient_hash, plaintext, timestamp)?;
    Some((conv_id, msg_id))
}

/// Mark a message as delivered.
pub fn receive_message(conv_id: u64, msg_id: u64) -> bool {
    let mut guard = CHAT_MANAGER.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.receive_message(conv_id, msg_id)
    } else {
        false
    }
}

/// Mark a message as read.
pub fn mark_read(conv_id: u64, msg_id: u64) -> bool {
    let mut guard = CHAT_MANAGER.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.mark_read(conv_id, msg_id)
    } else {
        false
    }
}

/// Delete a single message.
pub fn delete_message(conv_id: u64, msg_id: u64) -> bool {
    let mut guard = CHAT_MANAGER.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.delete_message(conv_id, msg_id)
    } else {
        false
    }
}

/// Decrypt a message and return the plaintext bytes.
pub fn decrypt_message(conv_id: u64, msg_id: u64) -> Option<Vec<u8>> {
    let guard = CHAT_MANAGER.lock();
    guard.as_ref()?.decrypt_message(conv_id, msg_id)
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    let mut guard = CHAT_MANAGER.lock();
    *guard = Some(ChatManager::new());
    serial_println!("[e2e_chat] initialised");
}
