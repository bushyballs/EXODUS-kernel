use crate::serial_println;
/// BLE phone-bridge sync for Genesis wearable
///
/// Implements the wearable side of a Bluetooth Low Energy phone bridge.
/// Provides notification mirroring, health data upload to the companion app,
/// and OTA firmware update transport.
///
/// ## Architecture
///
/// The phone bridge uses two custom GATT services:
///
///   1. **Notification Mirror Service** (NMS)
///      UUID 0x1810 (reused from Blood Pressure — placeholder; production
///      uses a Hoags-proprietary 128-bit UUID)
///      - Characteristic 0x2A35: notification payload (title + body, UTF-8)
///      - Characteristic 0x2A36: notification action (dismiss / reply token)
///
///   2. **Health Sync Service** (HSS)
///      UUID 0x180D (Heart Rate service standard UUID)
///      - Characteristic 0x2A37: HR measurement (standard format)
///      - Characteristic 0x2A53: Running Speed / steps (repurposed)
///      - Characteristic 0xFF01 (Hoags-private): GPS snapshot
///
/// The BLE hardware driver is abstracted behind the `BleSender` trait so
/// this module compiles on any platform.  A no-op stub is used when
/// `BLE_PRESENT` is false.
///
/// All code is original — Hoags Inc. (c) 2026.

#[allow(dead_code)]
use crate::sync::Mutex;
use alloc::vec::Vec;

// ============================================================================
// Notification payloads
// ============================================================================

/// Maximum byte length of a notification title
const MAX_TITLE_LEN: usize = 64;
/// Maximum byte length of a notification body
const MAX_BODY_LEN: usize = 256;
/// Maximum pending notification queue depth
const MAX_PENDING_NOTIFICATIONS: usize = 16;

/// A notification received from the phone and pending display on the watch.
#[derive(Clone)]
pub struct WearableNotification {
    /// App identifier (e.g., "com.messages")
    pub app_id: [u8; 32],
    pub app_id_len: usize,
    /// Notification title (UTF-8, null-padded)
    pub title: [u8; MAX_TITLE_LEN],
    pub title_len: usize,
    /// Notification body text
    pub body: [u8; MAX_BODY_LEN],
    pub body_len: usize,
    /// Arrival timestamp (kernel uptime ms)
    pub timestamp_ms: u64,
    /// Whether the user has dismissed this notification on the watch
    pub dismissed: bool,
    /// Whether a haptic pulse was emitted
    pub haptic_sent: bool,
}

impl WearableNotification {
    fn new() -> Self {
        WearableNotification {
            app_id: [0u8; 32],
            app_id_len: 0,
            title: [0u8; MAX_TITLE_LEN],
            title_len: 0,
            body: [0u8; MAX_BODY_LEN],
            body_len: 0,
            timestamp_ms: 0,
            dismissed: false,
            haptic_sent: false,
        }
    }
}

// ============================================================================
// Sync state machine
// ============================================================================

/// BLE connection state
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BleState {
    /// BLE hardware not present or disabled
    Disabled,
    /// Advertising — waiting for phone to connect
    Advertising,
    /// Connected to phone
    Connected,
    /// Actively syncing health data
    Syncing,
    /// Performing OTA firmware update
    OtaInProgress {
        bytes_received: u32,
        total_bytes: u32,
    },
}

/// Overall BLE bridge state
pub struct BleBridge {
    pub state: BleState,
    /// Received notifications queue
    pending_notifications: Vec<WearableNotification>,
    /// Total bytes of health data sent to phone in this session
    pub health_bytes_sent: u64,
    /// Total notifications received
    pub notifications_received: u64,
    /// RSSI of current connection in dBm (0 = unknown)
    pub rssi_dbm: i8,
    /// Whether the phone has granted health data write permission
    pub health_write_granted: bool,
}

impl BleBridge {
    fn new() -> Self {
        BleBridge {
            state: BleState::Disabled,
            pending_notifications: Vec::new(),
            health_bytes_sent: 0,
            notifications_received: 0,
            rssi_dbm: 0,
            health_write_granted: false,
        }
    }

    /// Handle an incoming notification from the phone (GATT write to NMS).
    pub fn on_notification_received(
        &mut self,
        app_id: &[u8],
        title: &[u8],
        body: &[u8],
        now_ms: u64,
    ) {
        if self.pending_notifications.len() >= MAX_PENDING_NOTIFICATIONS {
            // Drop oldest
            self.pending_notifications.remove(0);
        }

        let mut notif = WearableNotification::new();

        let id_len = app_id.len().min(32);
        notif.app_id[..id_len].copy_from_slice(&app_id[..id_len]);
        notif.app_id_len = id_len;

        let t_len = title.len().min(MAX_TITLE_LEN);
        notif.title[..t_len].copy_from_slice(&title[..t_len]);
        notif.title_len = t_len;

        let b_len = body.len().min(MAX_BODY_LEN);
        notif.body[..b_len].copy_from_slice(&body[..b_len]);
        notif.body_len = b_len;

        notif.timestamp_ms = now_ms;
        self.pending_notifications.push(notif);
        self.notifications_received += 1;
    }

    /// Dismiss a pending notification by index.
    pub fn dismiss_notification(&mut self, index: usize) {
        if let Some(n) = self.pending_notifications.get_mut(index) {
            n.dismissed = true;
        }
    }

    /// Return the number of un-dismissed notifications.
    pub fn unread_count(&self) -> usize {
        self.pending_notifications
            .iter()
            .filter(|n| !n.dismissed)
            .count()
    }

    /// Return a reference to all pending notifications.
    pub fn notifications(&self) -> &[WearableNotification] {
        &self.pending_notifications
    }

    /// Clear all notifications.
    pub fn clear_notifications(&mut self) {
        self.pending_notifications.clear();
    }

    /// Update RSSI (called periodically by the BLE driver).
    pub fn update_rssi(&mut self, rssi_dbm: i8) {
        self.rssi_dbm = rssi_dbm;
    }

    /// Handle BLE connection event.
    pub fn on_connected(&mut self) {
        self.state = BleState::Connected;
        serial_println!("    BLE: phone connected");
    }

    /// Handle BLE disconnection event.
    pub fn on_disconnected(&mut self) {
        self.state = BleState::Advertising;
        self.rssi_dbm = 0;
        self.health_write_granted = false;
        serial_println!("    BLE: phone disconnected, advertising...");
    }

    /// Record that health bytes were sent upstream.
    pub fn record_health_send(&mut self, bytes: u32) {
        self.health_bytes_sent += bytes as u64;
    }

    /// Begin OTA update process.
    pub fn begin_ota(&mut self, total_bytes: u32) {
        self.state = BleState::OtaInProgress {
            bytes_received: 0,
            total_bytes,
        };
        serial_println!("    BLE: OTA started ({} bytes)", total_bytes);
    }

    /// Feed received OTA bytes.  Returns `true` when the update is complete.
    pub fn ota_feed(&mut self, chunk_len: u32) -> bool {
        if let BleState::OtaInProgress {
            ref mut bytes_received,
            total_bytes,
        } = self.state
        {
            *bytes_received = bytes_received.saturating_add(chunk_len);
            if *bytes_received >= total_bytes {
                serial_println!("    BLE: OTA complete ({} bytes)", total_bytes);
                self.state = BleState::Connected;
                return true;
            }
        }
        false
    }
}

static BLE_BRIDGE: Mutex<BleBridge> = Mutex::new(BleBridge {
    state: BleState::Disabled,
    pending_notifications: Vec::new(),
    health_bytes_sent: 0,
    notifications_received: 0,
    rssi_dbm: 0,
    health_write_granted: false,
});

// ============================================================================
// Public API
// ============================================================================

/// Initialise the BLE bridge.  Call after the BLE hardware driver is ready.
pub fn init() {
    let mut b = BLE_BRIDGE.lock();
    b.state = BleState::Advertising;
    serial_println!("    Wearable/BLE: bridge ready, advertising for phone connection");
}

/// Mark BLE hardware as unavailable (e.g., no BLE chip on this board).
pub fn disable() {
    BLE_BRIDGE.lock().state = BleState::Disabled;
    serial_println!("    Wearable/BLE: disabled (no hardware)");
}

/// Called by the BLE driver on phone connection.
pub fn on_phone_connected() {
    BLE_BRIDGE.lock().on_connected();
}

/// Called by the BLE driver on disconnection.
pub fn on_phone_disconnected() {
    BLE_BRIDGE.lock().on_disconnected();
}

/// Deliver a notification from the phone.
pub fn deliver_notification(app_id: &[u8], title: &[u8], body: &[u8], now_ms: u64) {
    BLE_BRIDGE
        .lock()
        .on_notification_received(app_id, title, body, now_ms);
}

/// Get the number of pending unread notifications.
pub fn unread_notification_count() -> usize {
    BLE_BRIDGE.lock().unread_count()
}

/// Dismiss a notification by its queue position.
pub fn dismiss_notification(index: usize) {
    BLE_BRIDGE.lock().dismiss_notification(index);
}

/// Clear all pending notifications.
pub fn clear_notifications() {
    BLE_BRIDGE.lock().clear_notifications();
}

/// Get current BLE connection state.
pub fn ble_state() -> BleState {
    BLE_BRIDGE.lock().state
}

/// Update RSSI from the BLE driver.
pub fn update_rssi(rssi_dbm: i8) {
    BLE_BRIDGE.lock().update_rssi(rssi_dbm);
}

/// Record health data bytes sent to phone.
pub fn record_health_send(bytes: u32) {
    BLE_BRIDGE.lock().record_health_send(bytes);
}

/// Begin an OTA firmware update (total byte count known from manifest).
pub fn begin_ota(total_bytes: u32) {
    BLE_BRIDGE.lock().begin_ota(total_bytes);
}

/// Feed a received OTA chunk.  Returns `true` if the update is now complete.
pub fn ota_feed(chunk_len: u32) -> bool {
    BLE_BRIDGE.lock().ota_feed(chunk_len)
}
