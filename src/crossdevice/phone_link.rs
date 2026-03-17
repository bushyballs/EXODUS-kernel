/// Phone link for Genesis
///
/// Connect phone to desktop, mirror notifications,
/// send/receive SMS, transfer photos, and screen mirror.
///
/// Inspired by: Windows Phone Link, KDE Connect. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Link status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkStatus {
    Disconnected,
    Pairing,
    Connected,
    Syncing,
}

/// Phone notification
pub struct PhoneNotification {
    pub id: u32,
    pub app_name: String,
    pub title: String,
    pub body: String,
    pub timestamp: u64,
    pub dismissed: bool,
}

/// SMS message
pub struct SmsMessage {
    pub id: u32,
    pub sender: String,
    pub body: String,
    pub timestamp: u64,
    pub is_outgoing: bool,
    pub read: bool,
}

/// Phone battery info
pub struct PhoneBattery {
    pub level: u8,
    pub charging: bool,
}

/// Phone link manager
pub struct PhoneLink {
    pub status: LinkStatus,
    pub phone_name: String,
    pub phone_model: String,
    pub battery: PhoneBattery,
    pub notifications: Vec<PhoneNotification>,
    pub messages: Vec<SmsMessage>,
    pub next_msg_id: u32,
    pub mirror_notifications: bool,
    pub sync_photos: bool,
    pub screen_mirror: bool,
}

impl PhoneLink {
    const fn new() -> Self {
        PhoneLink {
            status: LinkStatus::Disconnected,
            phone_name: String::new(),
            phone_model: String::new(),
            battery: PhoneBattery {
                level: 0,
                charging: false,
            },
            notifications: Vec::new(),
            messages: Vec::new(),
            next_msg_id: 1,
            mirror_notifications: true,
            sync_photos: false,
            screen_mirror: false,
        }
    }

    pub fn pair(&mut self, name: &str, model: &str) {
        self.status = LinkStatus::Pairing;
        self.phone_name = String::from(name);
        self.phone_model = String::from(model);
        self.status = LinkStatus::Connected;
    }

    pub fn disconnect(&mut self) {
        self.status = LinkStatus::Disconnected;
    }

    pub fn push_notification(&mut self, app: &str, title: &str, body: &str) {
        if !self.mirror_notifications {
            return;
        }
        self.notifications.push(PhoneNotification {
            id: self.notifications.len() as u32 + 1,
            app_name: String::from(app),
            title: String::from(title),
            body: String::from(body),
            timestamp: crate::time::clock::unix_time(),
            dismissed: false,
        });
    }

    pub fn send_sms(&mut self, to: &str, body: &str) -> u32 {
        let id = self.next_msg_id;
        self.next_msg_id = self.next_msg_id.saturating_add(1);
        self.messages.push(SmsMessage {
            id,
            sender: String::from(to),
            body: String::from(body),
            timestamp: crate::time::clock::unix_time(),
            is_outgoing: true,
            read: true,
        });
        id
    }

    pub fn receive_sms(&mut self, from: &str, body: &str) {
        let id = self.next_msg_id;
        self.next_msg_id = self.next_msg_id.saturating_add(1);
        self.messages.push(SmsMessage {
            id,
            sender: String::from(from),
            body: String::from(body),
            timestamp: crate::time::clock::unix_time(),
            is_outgoing: false,
            read: false,
        });
    }

    pub fn unread_sms_count(&self) -> usize {
        self.messages
            .iter()
            .filter(|m| !m.is_outgoing && !m.read)
            .count()
    }

    pub fn update_battery(&mut self, level: u8, charging: bool) {
        self.battery = PhoneBattery { level, charging };
    }

    pub fn is_connected(&self) -> bool {
        self.status == LinkStatus::Connected || self.status == LinkStatus::Syncing
    }
}

static PHONE: Mutex<PhoneLink> = Mutex::new(PhoneLink::new());

pub fn init() {
    crate::serial_println!("  [crossdevice] Phone Link initialized");
}
