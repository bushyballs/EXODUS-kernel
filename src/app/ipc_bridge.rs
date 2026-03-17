use crate::sync::Mutex;
/// App-to-app communication bridge
///
/// Part of the Genesis app framework. Enables sandboxed apps
/// to exchange messages through a brokered IPC channel with
/// channel-based routing and access control.
use alloc::string::String;
use alloc::vec::Vec;

/// An IPC message between applications
pub struct IpcMessage {
    pub sender_app: u64,
    pub receiver_app: u64,
    pub channel: String,
    pub payload: Vec<u8>,
}

impl IpcMessage {
    /// Create a new IPC message
    pub fn new(sender: u64, receiver: u64, channel: &str, payload: Vec<u8>) -> Self {
        let mut ch = String::new();
        for c in channel.chars() {
            ch.push(c);
        }
        Self {
            sender_app: sender,
            receiver_app: receiver,
            channel: ch,
            payload,
        }
    }

    /// Get the payload size
    pub fn size(&self) -> usize {
        self.payload.len()
    }
}

/// Channel subscription record
struct ChannelSubscription {
    app_id: u64,
    channel: String,
    filter: Option<String>, // optional message type filter
}

/// Access control entry for IPC
struct AclEntry {
    source_app: u64,
    dest_app: u64,
    channel: String,
    allowed: bool,
}

pub struct IpcBridge {
    pub pending: Vec<IpcMessage>,
    subscriptions: Vec<ChannelSubscription>,
    acl: Vec<AclEntry>,
    max_queue_size: usize,
    max_message_size: usize,
    total_messages_sent: u64,
    total_messages_delivered: u64,
    dropped_messages: u64,
}

impl IpcBridge {
    pub fn new() -> Self {
        crate::serial_println!("[app::ipc] bridge created");
        Self {
            pending: Vec::new(),
            subscriptions: Vec::new(),
            acl: Vec::new(),
            max_queue_size: 1024,
            max_message_size: 64 * 1024, // 64KB max message
            total_messages_sent: 0,
            total_messages_delivered: 0,
            dropped_messages: 0,
        }
    }

    /// Subscribe an app to a channel
    pub fn subscribe(&mut self, app_id: u64, channel: &str) {
        let mut ch = String::new();
        for c in channel.chars() {
            ch.push(c);
        }

        // Check for duplicate subscription
        for sub in &self.subscriptions {
            if sub.app_id == app_id && sub.channel.as_str() == channel {
                return; // already subscribed
            }
        }

        self.subscriptions.push(ChannelSubscription {
            app_id,
            channel: ch,
            filter: None,
        });
        crate::serial_println!(
            "[app::ipc] app {} subscribed to channel '{}'",
            app_id,
            channel
        );
    }

    /// Unsubscribe an app from a channel
    pub fn unsubscribe(&mut self, app_id: u64, channel: &str) {
        let mut i = 0;
        while i < self.subscriptions.len() {
            if self.subscriptions[i].app_id == app_id
                && self.subscriptions[i].channel.as_str() == channel
            {
                self.subscriptions.remove(i);
                crate::serial_println!("[app::ipc] app {} unsubscribed from '{}'", app_id, channel);
            } else {
                i += 1;
            }
        }
    }

    /// Add an ACL rule to allow or deny communication between apps
    pub fn add_acl(&mut self, source: u64, dest: u64, channel: &str, allowed: bool) {
        let mut ch = String::new();
        for c in channel.chars() {
            ch.push(c);
        }
        self.acl.push(AclEntry {
            source_app: source,
            dest_app: dest,
            channel: ch,
            allowed,
        });
    }

    /// Check if a message is allowed by the ACL
    fn check_acl(&self, sender: u64, receiver: u64, channel: &str) -> bool {
        // If no ACL rules exist, allow all
        if self.acl.is_empty() {
            return true;
        }
        // Check for an explicit allow rule
        for rule in &self.acl {
            if rule.source_app == sender
                && rule.dest_app == receiver
                && rule.channel.as_str() == channel
            {
                return rule.allowed;
            }
        }
        // Check for wildcard rules (app 0 = any app)
        for rule in &self.acl {
            if (rule.source_app == 0 || rule.source_app == sender)
                && (rule.dest_app == 0 || rule.dest_app == receiver)
                && (rule.channel.is_empty() || rule.channel.as_str() == channel)
            {
                return rule.allowed;
            }
        }
        // Default deny if ACL exists but no matching rule
        false
    }

    /// Send a message to another app via the bridge
    pub fn send(&mut self, msg: IpcMessage) -> Result<(), ()> {
        // Validate message size
        if msg.payload.len() > self.max_message_size {
            crate::serial_println!(
                "[app::ipc] message too large: {} > {} bytes",
                msg.payload.len(),
                self.max_message_size
            );
            return Err(());
        }

        // Check ACL
        if !self.check_acl(msg.sender_app, msg.receiver_app, &msg.channel) {
            crate::serial_println!(
                "[app::ipc] ACL denied: app {} -> app {} on '{}'",
                msg.sender_app,
                msg.receiver_app,
                msg.channel
            );
            return Err(());
        }

        // Check queue capacity
        if self.pending.len() >= self.max_queue_size {
            self.dropped_messages = self.dropped_messages.saturating_add(1);
            crate::serial_println!(
                "[app::ipc] queue full, dropping message (total dropped: {})",
                self.dropped_messages
            );
            return Err(());
        }

        crate::serial_println!(
            "[app::ipc] queued: app {} -> app {} on '{}' ({} bytes)",
            msg.sender_app,
            msg.receiver_app,
            msg.channel,
            msg.payload.len()
        );

        self.pending.push(msg);
        self.total_messages_sent = self.total_messages_sent.saturating_add(1);
        Ok(())
    }

    /// Broadcast a message to all subscribers of a channel
    pub fn broadcast(&mut self, sender: u64, channel: &str, payload: Vec<u8>) -> usize {
        let mut sent = 0;
        let subscribers: Vec<u64> = self
            .subscriptions
            .iter()
            .filter(|s| s.channel.as_str() == channel && s.app_id != sender)
            .map(|s| s.app_id)
            .collect();

        for receiver in subscribers {
            let mut p = Vec::with_capacity(payload.len());
            for b in &payload {
                p.push(*b);
            }
            let msg = IpcMessage::new(sender, receiver, channel, p);
            if self.send(msg).is_ok() {
                sent += 1;
            }
        }
        crate::serial_println!("[app::ipc] broadcast on '{}': {} recipients", channel, sent);
        sent
    }

    /// Receive pending messages for a given app
    pub fn receive(&mut self, app_id: u64) -> Vec<IpcMessage> {
        let mut delivered = Vec::new();
        let mut remaining = Vec::new();

        // Drain the pending queue, separating messages for this app
        while let Some(msg) = self.pending.pop() {
            if msg.receiver_app == app_id {
                delivered.push(msg);
            } else {
                remaining.push(msg);
            }
        }

        // Put remaining messages back (in original order)
        while let Some(msg) = remaining.pop() {
            self.pending.push(msg);
        }

        self.total_messages_delivered += delivered.len() as u64;

        if !delivered.is_empty() {
            crate::serial_println!(
                "[app::ipc] delivered {} messages to app {}",
                delivered.len(),
                app_id
            );
        }

        delivered
    }

    /// Get the number of pending messages for an app
    pub fn pending_count(&self, app_id: u64) -> usize {
        let mut count = 0;
        for msg in &self.pending {
            if msg.receiver_app == app_id {
                count += 1;
            }
        }
        count
    }

    /// Get total messages sent since init
    pub fn stats(&self) -> (u64, u64, u64) {
        (
            self.total_messages_sent,
            self.total_messages_delivered,
            self.dropped_messages,
        )
    }

    /// Purge all pending messages for a given app (e.g., when app exits)
    pub fn purge_app(&mut self, app_id: u64) {
        let before = self.pending.len();
        let mut i = 0;
        while i < self.pending.len() {
            if self.pending[i].sender_app == app_id || self.pending[i].receiver_app == app_id {
                self.pending.remove(i);
            } else {
                i += 1;
            }
        }
        let purged = before - self.pending.len();
        if purged > 0 {
            crate::serial_println!("[app::ipc] purged {} messages for app {}", purged, app_id);
        }

        // Remove subscriptions
        let mut i = 0;
        while i < self.subscriptions.len() {
            if self.subscriptions[i].app_id == app_id {
                self.subscriptions.remove(i);
            } else {
                i += 1;
            }
        }
    }
}

static BRIDGE: Mutex<Option<IpcBridge>> = Mutex::new(None);

pub fn init() {
    let bridge = IpcBridge::new();
    let mut b = BRIDGE.lock();
    *b = Some(bridge);
    crate::serial_println!("[app::ipc] IPC bridge subsystem initialized");
}
