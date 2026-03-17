// Push notification service: registration, message delivery, retry, acknowledgment, offline queue

use alloc::vec::Vec;
use alloc::vec;
use crate::sync::Mutex;
use crate::{serial_print, serial_println};

/// Q16 fixed-point unit
const Q16_ONE: i32 = 65536;

/// Maximum retry attempts for message delivery
const MAX_RETRIES: u8 = 5;

/// Maximum offline queue size per subscriber
const MAX_OFFLINE_QUEUE: usize = 64;

/// Delivery status of a push message
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryStatus {
    Pending,
    Delivered,
    Acknowledged,
    Failed,
    Expired,
    Queued,
}

/// Priority of a push message
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushPriority {
    Realtime,
    High,
    Normal,
    Low,
    Background,
}

/// A registered push subscriber (app or endpoint)
#[derive(Clone, Copy, Debug)]
pub struct PushSubscriber {
    pub subscriber_id: u32,
    pub app_id: u32,
    pub endpoint_hash: u32,
    pub auth_token_hash: u32,
    pub registered_at: u64,
    pub last_seen: u64,
    pub is_online: bool,
    pub offline_queue_count: u32,
    pub total_delivered: u64,
    pub total_failed: u64,
}

impl PushSubscriber {
    pub fn new(subscriber_id: u32, app_id: u32, endpoint_hash: u32, timestamp: u64) -> Self {
        Self {
            subscriber_id,
            app_id,
            endpoint_hash,
            auth_token_hash: 0,
            registered_at: timestamp,
            last_seen: timestamp,
            is_online: true,
            offline_queue_count: 0,
            total_delivered: 0,
            total_failed: 0,
        }
    }
}

/// A push message awaiting or completed delivery
#[derive(Clone, Copy, Debug)]
pub struct PushMessage {
    pub message_id: u32,
    pub subscriber_id: u32,
    pub payload_hash: u32,
    pub title_hash: u32,
    pub priority: PushPriority,
    pub status: DeliveryStatus,
    pub created_at: u64,
    pub delivered_at: u64,
    pub acknowledged_at: u64,
    pub retry_count: u8,
    pub ttl_seconds: u32,
}

impl PushMessage {
    pub fn new(
        message_id: u32,
        subscriber_id: u32,
        payload_hash: u32,
        title_hash: u32,
        priority: PushPriority,
        created_at: u64,
        ttl_seconds: u32,
    ) -> Self {
        Self {
            message_id,
            subscriber_id,
            payload_hash,
            title_hash,
            priority,
            status: DeliveryStatus::Pending,
            created_at,
            delivered_at: 0,
            acknowledged_at: 0,
            retry_count: 0,
            ttl_seconds,
        }
    }

    /// Check if the message has expired
    pub fn is_expired(&self, current_time: u64) -> bool {
        current_time > self.created_at + self.ttl_seconds as u64
    }
}

/// An offline-queued message for subscribers who are not currently reachable
#[derive(Clone, Copy, Debug)]
pub struct OfflineEntry {
    pub message_id: u32,
    pub subscriber_id: u32,
    pub payload_hash: u32,
    pub title_hash: u32,
    pub priority: PushPriority,
    pub queued_at: u64,
    pub ttl_seconds: u32,
}

/// Retry backoff calculation result
#[derive(Clone, Copy, Debug)]
pub struct RetryInfo {
    pub attempt: u8,
    pub delay_ms_q16: i32,
    pub should_retry: bool,
}

/// Push notification service managing registrations, delivery, retries, and offline queuing
pub struct PushService {
    subscribers: Vec<PushSubscriber>,
    messages: Vec<PushMessage>,
    offline_queue: Vec<OfflineEntry>,
    next_subscriber_id: u32,
    next_message_id: u32,
    total_registrations: u64,
    total_unregistrations: u64,
    total_messages_sent: u64,
    total_acks: u64,
    total_retries: u64,
    total_expired: u64,
    /// Exponential backoff base delay in Q16 (default 1000ms = 1000 * Q16_ONE)
    backoff_base_q16: i32,
    /// Backoff multiplier in Q16 (default 2.0 = 2 * Q16_ONE)
    backoff_multiplier_q16: i32,
}

impl PushService {
    pub fn new() -> Self {
        Self {
            subscribers: vec![],
            messages: vec![],
            offline_queue: vec![],
            next_subscriber_id: 1,
            next_message_id: 1,
            total_registrations: 0,
            total_unregistrations: 0,
            total_messages_sent: 0,
            total_acks: 0,
            total_retries: 0,
            total_expired: 0,
            backoff_base_q16: 1000 * Q16_ONE,
            backoff_multiplier_q16: 2 * Q16_ONE,
        }
    }

    /// Register a new push subscriber
    pub fn register(
        &mut self,
        app_id: u32,
        endpoint_hash: u32,
        auth_token_hash: u32,
        timestamp: u64,
    ) -> u32 {
        // Check for duplicate registration
        if let Some(existing) = self.subscribers.iter_mut().find(|s| {
            s.app_id == app_id && s.endpoint_hash == endpoint_hash
        }) {
            existing.auth_token_hash = auth_token_hash;
            existing.last_seen = timestamp;
            existing.is_online = true;
            serial_println!(
                "[PUSH] Re-registered subscriber {} for app {}",
                existing.subscriber_id,
                app_id
            );
            return existing.subscriber_id;
        }

        let id = self.next_subscriber_id;
        self.next_subscriber_id = self.next_subscriber_id.saturating_add(1);

        let mut sub = PushSubscriber::new(id, app_id, endpoint_hash, timestamp);
        sub.auth_token_hash = auth_token_hash;
        self.subscribers.push(sub);
        self.total_registrations = self.total_registrations.saturating_add(1);

        serial_println!(
            "[PUSH] Registered subscriber {} for app {} (endpoint 0x{:08X})",
            id,
            app_id,
            endpoint_hash
        );

        id
    }

    /// Unregister a push subscriber
    pub fn unregister(&mut self, subscriber_id: u32) -> bool {
        if let Some(pos) = self.subscribers.iter().position(|s| s.subscriber_id == subscriber_id) {
            self.subscribers.remove(pos);
            // Also remove offline queue entries
            self.offline_queue.retain(|e| e.subscriber_id != subscriber_id);
            self.total_unregistrations = self.total_unregistrations.saturating_add(1);
            serial_println!("[PUSH] Unregistered subscriber {}", subscriber_id);
            true
        } else {
            false
        }
    }

    /// Send a push message to a subscriber
    pub fn send(
        &mut self,
        subscriber_id: u32,
        payload_hash: u32,
        title_hash: u32,
        priority: PushPriority,
        timestamp: u64,
        ttl_seconds: u32,
    ) -> Option<u32> {
        let sub = self.subscribers.iter().find(|s| s.subscriber_id == subscriber_id);
        if sub.is_none() {
            serial_println!("[PUSH] Error: subscriber {} not found", subscriber_id);
            return None;
        }
        let is_online = sub.unwrap().is_online;

        let msg_id = self.next_message_id;
        self.next_message_id = self.next_message_id.saturating_add(1);

        if !is_online {
            // Queue for offline delivery
            let entry = OfflineEntry {
                message_id: msg_id,
                subscriber_id,
                payload_hash,
                title_hash,
                priority,
                queued_at: timestamp,
                ttl_seconds,
            };
            self.enqueue_offline(entry);

            let msg = PushMessage::new(msg_id, subscriber_id, payload_hash, title_hash, priority, timestamp, ttl_seconds);
            let mut msg = msg;
            msg.status = DeliveryStatus::Queued;
            self.messages.push(msg);

            serial_println!(
                "[PUSH] Subscriber {} offline, message {} queued",
                subscriber_id,
                msg_id
            );
            return Some(msg_id);
        }

        let msg = PushMessage::new(msg_id, subscriber_id, payload_hash, title_hash, priority, timestamp, ttl_seconds);
        self.messages.push(msg);
        self.total_messages_sent = self.total_messages_sent.saturating_add(1);

        serial_println!(
            "[PUSH] Sent message {} to subscriber {} (priority {:?})",
            msg_id,
            subscriber_id,
            priority
        );

        Some(msg_id)
    }

    /// Broadcast a push message to all subscribers of an app
    pub fn broadcast(
        &mut self,
        app_id: u32,
        payload_hash: u32,
        title_hash: u32,
        priority: PushPriority,
        timestamp: u64,
        ttl_seconds: u32,
    ) -> Vec<u32> {
        let sub_ids: Vec<u32> = self.subscribers.iter()
            .filter(|s| s.app_id == app_id)
            .map(|s| s.subscriber_id)
            .collect();

        let mut sent_ids = vec![];
        for sub_id in sub_ids {
            if let Some(msg_id) = self.send(sub_id, payload_hash, title_hash, priority, timestamp, ttl_seconds) {
                sent_ids.push(msg_id);
            }
        }

        serial_println!(
            "[PUSH] Broadcast to app {}: {} messages sent",
            app_id,
            sent_ids.len()
        );

        sent_ids
    }

    /// Mark a message as delivered
    pub fn mark_delivered(&mut self, message_id: u32, timestamp: u64) -> bool {
        if let Some(msg) = self.messages.iter_mut().find(|m| m.message_id == message_id) {
            msg.status = DeliveryStatus::Delivered;
            msg.delivered_at = timestamp;

            // Update subscriber stats
            if let Some(sub) = self.subscribers.iter_mut().find(|s| s.subscriber_id == msg.subscriber_id) {
                sub.total_delivered = sub.total_delivered.saturating_add(1);
                sub.last_seen = timestamp;
            }

            serial_println!("[PUSH] Message {} delivered", message_id);
            true
        } else {
            false
        }
    }

    /// Acknowledge receipt of a message
    pub fn acknowledge(&mut self, message_id: u32, timestamp: u64) -> bool {
        if let Some(msg) = self.messages.iter_mut().find(|m| m.message_id == message_id) {
            msg.status = DeliveryStatus::Acknowledged;
            msg.acknowledged_at = timestamp;
            self.total_acks = self.total_acks.saturating_add(1);

            serial_println!("[PUSH] Message {} acknowledged", message_id);
            true
        } else {
            false
        }
    }

    /// Retry delivery of a failed message with exponential backoff
    pub fn retry(&mut self, message_id: u32) -> RetryInfo {
        if let Some(msg) = self.messages.iter_mut().find(|m| m.message_id == message_id) {
            if msg.retry_count >= MAX_RETRIES {
                msg.status = DeliveryStatus::Failed;
                serial_println!(
                    "[PUSH] Message {} exceeded max retries, marked as failed",
                    message_id
                );
                return RetryInfo {
                    attempt: msg.retry_count,
                    delay_ms_q16: 0,
                    should_retry: false,
                };
            }

            msg.retry_count = msg.retry_count.saturating_add(1);
            msg.status = DeliveryStatus::Pending;
            self.total_retries = self.total_retries.saturating_add(1);

            // Calculate exponential backoff delay
            let delay = self.calculate_backoff(msg.retry_count);

            serial_println!(
                "[PUSH] Retrying message {} (attempt {}/{})",
                message_id,
                msg.retry_count,
                MAX_RETRIES
            );

            RetryInfo {
                attempt: msg.retry_count,
                delay_ms_q16: delay,
                should_retry: true,
            }
        } else {
            RetryInfo {
                attempt: 0,
                delay_ms_q16: 0,
                should_retry: false,
            }
        }
    }

    /// Calculate exponential backoff delay in Q16
    fn calculate_backoff(&self, attempt: u8) -> i32 {
        // delay = base * multiplier^attempt (all in Q16)
        let mut result_q16: i64 = self.backoff_base_q16 as i64;
        for _ in 0..attempt {
            result_q16 = (result_q16 * self.backoff_multiplier_q16 as i64) >> 16;
        }
        // Clamp to i32 range
        if result_q16 > i32::MAX as i64 {
            i32::MAX
        } else {
            result_q16 as i32
        }
    }

    /// Enqueue a message for offline delivery
    fn enqueue_offline(&mut self, entry: OfflineEntry) {
        // Enforce per-subscriber queue limit
        let count = self.offline_queue.iter()
            .filter(|e| e.subscriber_id == entry.subscriber_id)
            .count();

        if count >= MAX_OFFLINE_QUEUE {
            // Remove oldest entry for this subscriber
            if let Some(pos) = self.offline_queue.iter()
                .position(|e| e.subscriber_id == entry.subscriber_id)
            {
                self.offline_queue.remove(pos);
            }
        }

        if let Some(sub) = self.subscribers.iter_mut().find(|s| s.subscriber_id == entry.subscriber_id) {
            sub.offline_queue_count = sub.offline_queue_count.saturating_add(1);
        }

        self.offline_queue.push(entry);
    }

    /// Flush offline queue when subscriber comes back online
    pub fn flush_offline_queue(&mut self, subscriber_id: u32, current_time: u64) -> Vec<u32> {
        let mut flushed_ids = vec![];

        // Collect entries for this subscriber
        let entries: Vec<OfflineEntry> = self.offline_queue.iter()
            .filter(|e| e.subscriber_id == subscriber_id)
            .copied()
            .collect();

        // Remove from offline queue
        self.offline_queue.retain(|e| e.subscriber_id != subscriber_id);

        // Deliver each non-expired entry
        for entry in &entries {
            if current_time <= entry.queued_at + entry.ttl_seconds as u64 {
                // Update existing message status
                if let Some(msg) = self.messages.iter_mut().find(|m| m.message_id == entry.message_id) {
                    msg.status = DeliveryStatus::Pending;
                }
                flushed_ids.push(entry.message_id);
            } else {
                self.total_expired = self.total_expired.saturating_add(1);
                if let Some(msg) = self.messages.iter_mut().find(|m| m.message_id == entry.message_id) {
                    msg.status = DeliveryStatus::Expired;
                }
            }
        }

        // Reset subscriber offline count
        if let Some(sub) = self.subscribers.iter_mut().find(|s| s.subscriber_id == subscriber_id) {
            sub.offline_queue_count = 0;
            sub.is_online = true;
            sub.last_seen = current_time;
        }

        serial_println!(
            "[PUSH] Flushed {} offline messages for subscriber {} ({} expired)",
            flushed_ids.len(),
            subscriber_id,
            entries.len() - flushed_ids.len()
        );

        flushed_ids
    }

    /// Set subscriber online/offline status
    pub fn set_online(&mut self, subscriber_id: u32, online: bool, timestamp: u64) -> bool {
        if let Some(sub) = self.subscribers.iter_mut().find(|s| s.subscriber_id == subscriber_id) {
            sub.is_online = online;
            sub.last_seen = timestamp;
            serial_println!(
                "[PUSH] Subscriber {} is now {}",
                subscriber_id,
                if online { "online" } else { "offline" }
            );
            true
        } else {
            false
        }
    }

    /// Expire stale messages
    pub fn expire_messages(&mut self, current_time: u64) -> u32 {
        let mut expired_count: u32 = 0;
        for msg in &mut self.messages {
            if msg.status == DeliveryStatus::Pending && msg.is_expired(current_time) {
                msg.status = DeliveryStatus::Expired;
                expired_count += 1;
            }
        }
        if expired_count > 0 {
            self.total_expired += expired_count as u64;
            serial_println!("[PUSH] Expired {} stale messages", expired_count);
        }
        expired_count
    }

    /// Get subscriber count
    pub fn subscriber_count(&self) -> usize {
        self.subscribers.len()
    }

    /// Get pending message count
    pub fn pending_count(&self) -> usize {
        self.messages.iter().filter(|m| m.status == DeliveryStatus::Pending).count()
    }

    /// Get offline queue size
    pub fn offline_queue_size(&self) -> usize {
        self.offline_queue.len()
    }

    /// Get stats tuple: (registrations, messages_sent, acks, retries, expired)
    pub fn stats(&self) -> (u64, u64, u64, u64, u64) {
        (
            self.total_registrations,
            self.total_messages_sent,
            self.total_acks,
            self.total_retries,
            self.total_expired,
        )
    }

    /// Clean up delivered/acknowledged/expired messages older than threshold
    pub fn cleanup(&mut self, before_timestamp: u64) -> u32 {
        let initial = self.messages.len();
        self.messages.retain(|m| {
            match m.status {
                DeliveryStatus::Acknowledged | DeliveryStatus::Expired | DeliveryStatus::Failed => {
                    m.created_at >= before_timestamp
                }
                _ => true,
            }
        });
        let removed = (initial - self.messages.len()) as u32;
        if removed > 0 {
            serial_println!("[PUSH] Cleaned up {} old messages", removed);
        }
        removed
    }
}

static PUSH_SERVICE: Mutex<Option<PushService>> = Mutex::new(None);

/// Initialize the push notification service
pub fn init() {
    let mut lock = PUSH_SERVICE.lock();
    *lock = Some(PushService::new());
    serial_println!("[PUSH] Push notification service initialized");
}

/// Get a reference to the push service
pub fn get_service() -> &'static Mutex<Option<PushService>> {
    &PUSH_SERVICE
}
