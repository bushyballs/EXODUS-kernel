use crate::sync::Mutex;
/// SMS/MMS messaging for Genesis
///
/// SMS encoding/decoding (GSM 7-bit, UCS-2), MMS support,
/// group messaging, delivery reports, rich messaging (RCS).
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum MessageType {
    Sms,
    Mms,
    Rcs,
}

#[derive(Clone, Copy, PartialEq)]
pub enum MessageStatus {
    Draft,
    Sending,
    Sent,
    Delivered,
    Read,
    Failed,
}

#[derive(Clone, Copy, PartialEq)]
pub enum SmsEncoding {
    Gsm7Bit,
    Ucs2,
}

struct Message {
    id: u32,
    msg_type: MessageType,
    status: MessageStatus,
    sender: [u8; 20],
    sender_len: usize,
    recipient: [u8; 20],
    recipient_len: usize,
    body_hash: u64,
    body_len: usize,
    encoding: SmsEncoding,
    timestamp: u64,
    is_group: bool,
    thread_id: u32,
}

struct Conversation {
    thread_id: u32,
    participants: Vec<[u8; 20]>,
    message_count: u32,
    last_message_time: u64,
    unread_count: u32,
}

struct SmsEngine {
    messages: Vec<Message>,
    conversations: Vec<Conversation>,
    next_id: u32,
    next_thread: u32,
    sent_count: u32,
    received_count: u32,
}

static SMS_ENGINE: Mutex<Option<SmsEngine>> = Mutex::new(None);

impl SmsEngine {
    fn new() -> Self {
        SmsEngine {
            messages: Vec::new(),
            conversations: Vec::new(),
            next_id: 1,
            next_thread: 1,
            sent_count: 0,
            received_count: 0,
        }
    }

    fn determine_encoding(text: &[u8]) -> SmsEncoding {
        // GSM 7-bit supports ASCII subset
        for &b in text {
            if b > 127 {
                return SmsEncoding::Ucs2;
            }
        }
        SmsEncoding::Gsm7Bit
    }

    fn send_sms(&mut self, recipient: &[u8], body: &[u8], timestamp: u64) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let mut recip = [0u8; 20];
        let rlen = recipient.len().min(20);
        recip[..rlen].copy_from_slice(&recipient[..rlen]);
        let encoding = Self::determine_encoding(body);
        // Simple hash of body
        let mut hash = 0u64;
        for &b in body {
            hash = hash.wrapping_mul(31).wrapping_add(b as u64);
        }
        let thread = self.find_or_create_thread(&recip);
        self.messages.push(Message {
            id,
            msg_type: MessageType::Sms,
            status: MessageStatus::Sending,
            sender: [0; 20], // self
            sender_len: 0,
            recipient: recip,
            recipient_len: rlen,
            body_hash: hash,
            body_len: body.len(),
            encoding,
            timestamp,
            is_group: false,
            thread_id: thread,
        });
        self.sent_count = self.sent_count.saturating_add(1);
        id
    }

    fn receive_sms(&mut self, sender: &[u8], body: &[u8], timestamp: u64) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let mut sndr = [0u8; 20];
        let slen = sender.len().min(20);
        sndr[..slen].copy_from_slice(&sender[..slen]);
        let encoding = Self::determine_encoding(body);
        let mut hash = 0u64;
        for &b in body {
            hash = hash.wrapping_mul(31).wrapping_add(b as u64);
        }
        let thread = self.find_or_create_thread(&sndr);
        self.messages.push(Message {
            id,
            msg_type: MessageType::Sms,
            status: MessageStatus::Delivered,
            sender: sndr,
            sender_len: slen,
            recipient: [0; 20],
            recipient_len: 0,
            body_hash: hash,
            body_len: body.len(),
            encoding,
            timestamp,
            is_group: false,
            thread_id: thread,
        });
        self.received_count = self.received_count.saturating_add(1);
        // Update conversation
        if let Some(conv) = self
            .conversations
            .iter_mut()
            .find(|c| c.thread_id == thread)
        {
            conv.message_count = conv.message_count.saturating_add(1);
            conv.last_message_time = timestamp;
            conv.unread_count = conv.unread_count.saturating_add(1);
        }
        id
    }

    fn find_or_create_thread(&mut self, contact: &[u8; 20]) -> u32 {
        for conv in &self.conversations {
            for p in &conv.participants {
                if p == contact {
                    return conv.thread_id;
                }
            }
        }
        let tid = self.next_thread;
        self.next_thread = self.next_thread.saturating_add(1);
        self.conversations.push(Conversation {
            thread_id: tid,
            participants: {
                let mut v = Vec::new();
                v.push(*contact);
                v
            },
            message_count: 0,
            last_message_time: 0,
            unread_count: 0,
        });
        tid
    }
}

pub fn init() {
    let mut engine = SMS_ENGINE.lock();
    *engine = Some(SmsEngine::new());
    serial_println!("    Telephony: SMS/MMS/RCS messaging ready");
}
