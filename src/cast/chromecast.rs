/// Chromecast-compatible casting for Genesis
///
/// DIAL-based device discovery, media control, sender/receiver protocol,
/// queue management, and multi-room group casting.

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_CAST_DEVICES: usize = 32;
const MAX_QUEUE_ENTRIES: usize = 512;
const MAX_SENDER_CONNECTIONS: usize = 8;
const DIAL_PORT: u16 = 8008;
const CAST_TLS_PORT: u16 = 8009;

/// Default volume Q16 (0.70 * 65536)
const DEFAULT_VOLUME_Q16: i32 = 45875;

/// Maximum volume Q16 (1.0 * 65536)
const MAX_VOLUME_Q16: i32 = 65536;

/// Seek granularity in ms
const SEEK_STEP_MS: u64 = 10000;

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub enum CastDeviceType {
    Chromecast,
    ChromecastUltra,
    ChromecastAudio,
    AndroidTV,
    SmartDisplay,
    Speaker,
}

#[derive(Clone, Copy, PartialEq)]
pub enum CastSessionState {
    Idle,
    Connecting,
    Connected,
    Launching,
    Playing,
    Paused,
    Buffering,
    Error,
}

#[derive(Clone, Copy, PartialEq)]
pub enum MediaType {
    Video,
    Audio,
    Photo,
    ScreenMirror,
    WebPage,
}

#[derive(Clone, Copy, PartialEq)]
pub enum RepeatMode {
    Off,
    Single,
    All,
    AllAndShuffle,
}

#[derive(Clone, Copy, PartialEq)]
pub enum SenderRole {
    Owner,
    Controller,
    Viewer,
}

#[derive(Clone, Copy, PartialEq)]
pub enum DialAppState {
    Stopped,
    Running,
    Installable,
    NotAvailable,
}

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct CastDevice {
    id: u32,
    name_hash: u64,
    device_type: CastDeviceType,
    ip_addr: [u8; 4],
    dial_port: u16,
    tls_port: u16,
    model_hash: u64,
    firmware_ver: u32,
    supports_4k: bool,
    supports_hdr: bool,
    supports_multizone: bool,
    group_id: u32,
    volume_q16: i32,
    muted: bool,
    reachable: bool,
}

#[derive(Clone, Copy)]
struct SenderConnection {
    sender_id: u32,
    device_id: u32,
    role: SenderRole,
    transport_id_hash: u64,
    connected: bool,
    heartbeat_ms: u64,
}

#[derive(Clone, Copy)]
struct CastSession {
    session_id: u64,
    device_id: u32,
    app_hash: u64,
    state: CastSessionState,
    media_type: MediaType,
    content_hash: u64,
    position_ms: u64,
    duration_ms: u64,
    playback_rate_q16: i32,   // Q16 (1.0 = 65536)
    volume_q16: i32,
    muted: bool,
    bytes_streamed: u64,
    buffer_percent: u8,
}

#[derive(Clone, Copy)]
struct QueueItem {
    item_id: u32,
    content_hash: u64,
    media_type: MediaType,
    duration_ms: u64,
    played: bool,
    loading: bool,
    autoplay: bool,
}

#[derive(Clone, Copy)]
struct DeviceGroup {
    group_id: u32,
    leader_device_id: u32,
    member_ids: [u32; 8],
    member_count: u8,
    synced: bool,
    volume_q16: i32,
}

// ---------------------------------------------------------------------------
// DIAL Application Registry
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct DialApp {
    app_hash: u64,
    state: DialAppState,
    allow_stop: bool,
}

// ---------------------------------------------------------------------------
// Manager
// ---------------------------------------------------------------------------

struct ChromecastManager {
    devices: Vec<CastDevice>,
    senders: Vec<SenderConnection>,
    session: Option<CastSession>,
    queue: Vec<QueueItem>,
    groups: Vec<DeviceGroup>,
    dial_apps: Vec<DialApp>,
    repeat_mode: RepeatMode,
    shuffle: bool,
    next_device_id: u32,
    next_session_id: u64,
    next_sender_id: u32,
    next_item_id: u32,
    next_group_id: u32,
    discovery_active: bool,
    total_sessions: u64,
    total_bytes_cast: u64,
}

static CHROMECAST: Mutex<Option<ChromecastManager>> = Mutex::new(None);

impl ChromecastManager {
    fn new() -> Self {
        ChromecastManager {
            devices: Vec::new(),
            senders: Vec::new(),
            session: None,
            queue: Vec::new(),
            groups: Vec::new(),
            dial_apps: Vec::new(),
            repeat_mode: RepeatMode::Off,
            shuffle: false,
            next_device_id: 1,
            next_session_id: 1,
            next_sender_id: 1,
            next_item_id: 1,
            next_group_id: 1,
            discovery_active: false,
            total_sessions: 0,
            total_bytes_cast: 0,
        }
    }

    // -- DIAL Discovery -----------------------------------------------------

    fn start_dial_discovery(&mut self) {
        self.discovery_active = true;
        serial_println!("    Chromecast: DIAL/SSDP discovery on port {}", DIAL_PORT);
    }

    fn stop_discovery(&mut self) {
        self.discovery_active = false;
    }

    fn register_device(&mut self, name_hash: u64, ip: [u8; 4],
                        device_type: CastDeviceType, model_hash: u64,
                        supports_4k: bool, supports_hdr: bool) -> u32 {
        if self.devices.len() >= MAX_CAST_DEVICES { return 0; }
        let id = self.next_device_id;
        self.next_device_id = self.next_device_id.saturating_add(1);
        self.devices.push(CastDevice {
            id,
            name_hash,
            device_type,
            ip_addr: ip,
            dial_port: DIAL_PORT,
            tls_port: CAST_TLS_PORT,
            model_hash,
            firmware_ver: 0,
            supports_4k,
            supports_hdr,
            supports_multizone: device_type != CastDeviceType::ChromecastAudio,
            group_id: 0,
            volume_q16: DEFAULT_VOLUME_Q16,
            muted: false,
            reachable: true,
        });
        id
    }

    fn remove_device(&mut self, device_id: u32) {
        if let Some(sess) = &self.session {
            if sess.device_id == device_id {
                self.end_session();
            }
        }
        self.senders.retain(|s| s.device_id != device_id);
        self.devices.retain(|d| d.id != device_id);
    }

    fn find_device(&self, device_id: u32) -> Option<&CastDevice> {
        self.devices.iter().find(|d| d.id == device_id)
    }

    fn device_count(&self) -> usize {
        self.devices.len()
    }

    // -- DIAL App Management ------------------------------------------------

    fn register_dial_app(&mut self, app_hash: u64, allow_stop: bool) {
        self.dial_apps.push(DialApp {
            app_hash,
            state: DialAppState::Stopped,
            allow_stop,
        });
    }

    fn launch_app(&mut self, app_hash: u64) -> bool {
        if let Some(app) = self.dial_apps.iter_mut().find(|a| a.app_hash == app_hash) {
            app.state = DialAppState::Running;
            true
        } else {
            false
        }
    }

    fn stop_app(&mut self, app_hash: u64) -> bool {
        if let Some(app) = self.dial_apps.iter_mut().find(|a| a.app_hash == app_hash) {
            if app.allow_stop {
                app.state = DialAppState::Stopped;
                return true;
            }
        }
        false
    }

    // -- Sender / Receiver --------------------------------------------------

    fn connect_sender(&mut self, device_id: u32, role: SenderRole) -> u32 {
        if self.senders.len() >= MAX_SENDER_CONNECTIONS { return 0; }
        if self.find_device(device_id).is_none() { return 0; }
        let sid = self.next_sender_id;
        self.next_sender_id = self.next_sender_id.saturating_add(1);
        self.senders.push(SenderConnection {
            sender_id: sid,
            device_id,
            role,
            transport_id_hash: 0,
            connected: true,
            heartbeat_ms: 0,
        });
        sid
    }

    fn disconnect_sender(&mut self, sender_id: u32) {
        if let Some(s) = self.senders.iter_mut().find(|s| s.sender_id == sender_id) {
            s.connected = false;
        }
    }

    fn update_heartbeat(&mut self, sender_id: u32, timestamp_ms: u64) {
        if let Some(s) = self.senders.iter_mut().find(|s| s.sender_id == sender_id) {
            s.heartbeat_ms = timestamp_ms;
        }
    }

    fn prune_stale_senders(&mut self, cutoff_ms: u64) {
        self.senders.retain(|s| s.heartbeat_ms >= cutoff_ms || !s.connected);
    }

    // -- Media Control ------------------------------------------------------

    fn start_cast(&mut self, device_id: u32, app_hash: u64, media_type: MediaType,
                   content_hash: u64, duration_ms: u64) -> bool {
        if self.find_device(device_id).is_none() { return false; }
        let sid = self.next_session_id;
        self.next_session_id = self.next_session_id.saturating_add(1);
        self.session = Some(CastSession {
            session_id: sid,
            device_id,
            app_hash,
            state: CastSessionState::Launching,
            media_type,
            content_hash,
            position_ms: 0,
            duration_ms,
            playback_rate_q16: 65536, // 1.0x
            volume_q16: DEFAULT_VOLUME_Q16,
            muted: false,
            bytes_streamed: 0,
            buffer_percent: 0,
        });
        self.total_sessions = self.total_sessions.saturating_add(1);
        true
    }

    fn end_session(&mut self) {
        if let Some(sess) = self.session.take() {
            self.total_bytes_cast += sess.bytes_streamed;
        }
    }

    fn play(&mut self) {
        if let Some(sess) = self.session.as_mut() {
            sess.state = CastSessionState::Playing;
        }
    }

    fn pause(&mut self) {
        if let Some(sess) = self.session.as_mut() {
            sess.state = CastSessionState::Paused;
        }
    }

    fn seek(&mut self, position_ms: u64) {
        if let Some(sess) = self.session.as_mut() {
            if position_ms <= sess.duration_ms {
                sess.position_ms = position_ms;
            }
        }
    }

    fn seek_forward(&mut self) {
        if let Some(sess) = self.session.as_mut() {
            let target = sess.position_ms + SEEK_STEP_MS;
            sess.position_ms = if target > sess.duration_ms { sess.duration_ms } else { target };
        }
    }

    fn seek_backward(&mut self) {
        if let Some(sess) = self.session.as_mut() {
            sess.position_ms = sess.position_ms.saturating_sub(SEEK_STEP_MS);
        }
    }

    fn set_volume(&mut self, device_id: u32, volume_q16: i32) {
        let clamped = if volume_q16 < 0 { 0 }
                      else if volume_q16 > MAX_VOLUME_Q16 { MAX_VOLUME_Q16 }
                      else { volume_q16 };
        if let Some(dev) = self.devices.iter_mut().find(|d| d.id == device_id) {
            dev.volume_q16 = clamped;
        }
        if let Some(sess) = self.session.as_mut() {
            if sess.device_id == device_id {
                sess.volume_q16 = clamped;
            }
        }
    }

    fn set_muted(&mut self, device_id: u32, muted: bool) {
        if let Some(dev) = self.devices.iter_mut().find(|d| d.id == device_id) {
            dev.muted = muted;
        }
        if let Some(sess) = self.session.as_mut() {
            if sess.device_id == device_id {
                sess.muted = muted;
            }
        }
    }

    fn set_playback_rate(&mut self, rate_q16: i32) {
        if let Some(sess) = self.session.as_mut() {
            sess.playback_rate_q16 = rate_q16;
        }
    }

    fn update_buffer(&mut self, percent: u8) {
        if let Some(sess) = self.session.as_mut() {
            sess.buffer_percent = percent.min(100);
            if percent >= 100 && sess.state == CastSessionState::Buffering {
                sess.state = CastSessionState::Playing;
            }
        }
    }

    // -- Queue Management ---------------------------------------------------

    fn enqueue(&mut self, content_hash: u64, media_type: MediaType,
               duration_ms: u64, autoplay: bool) -> u32 {
        if self.queue.len() >= MAX_QUEUE_ENTRIES { return 0; }
        let iid = self.next_item_id;
        self.next_item_id = self.next_item_id.saturating_add(1);
        self.queue.push(QueueItem {
            item_id: iid,
            content_hash,
            media_type,
            duration_ms,
            played: false,
            loading: false,
            autoplay,
        });
        iid
    }

    fn dequeue_next(&mut self) -> Option<QueueItem> {
        let pos = self.queue.iter().position(|q| !q.played)?;
        self.queue[pos].played = true;
        Some(self.queue[pos])
    }

    fn remove_from_queue(&mut self, item_id: u32) {
        self.queue.retain(|q| q.item_id != item_id);
    }

    fn reorder_queue(&mut self, item_id: u32, new_index: usize) {
        if let Some(pos) = self.queue.iter().position(|q| q.item_id == item_id) {
            let item = self.queue.remove(pos);
            let idx = if new_index > self.queue.len() { self.queue.len() } else { new_index };
            self.queue.insert(idx, item);
        }
    }

    fn clear_queue(&mut self) {
        self.queue.clear();
    }

    fn set_repeat(&mut self, mode: RepeatMode) {
        self.repeat_mode = mode;
        self.shuffle = mode == RepeatMode::AllAndShuffle;
    }

    fn queue_length(&self) -> usize {
        self.queue.iter().filter(|q| !q.played).count()
    }

    // -- Group / Multi-room -------------------------------------------------

    fn create_group(&mut self, leader_id: u32, member_ids: &[u32]) -> u32 {
        if self.find_device(leader_id).is_none() { return 0; }
        let gid = self.next_group_id;
        self.next_group_id = self.next_group_id.saturating_add(1);
        let mut members = [0u32; 8];
        let count = member_ids.len().min(8);
        for i in 0..count {
            members[i] = member_ids[i];
        }
        self.groups.push(DeviceGroup {
            group_id: gid,
            leader_device_id: leader_id,
            member_ids: members,
            member_count: count as u8,
            synced: true,
            volume_q16: DEFAULT_VOLUME_Q16,
        });
        // Tag devices with group
        for &mid in &member_ids[..count] {
            if let Some(dev) = self.devices.iter_mut().find(|d| d.id == mid) {
                dev.group_id = gid;
            }
        }
        if let Some(dev) = self.devices.iter_mut().find(|d| d.id == leader_id) {
            dev.group_id = gid;
        }
        gid
    }

    fn dissolve_group(&mut self, group_id: u32) {
        for dev in &mut self.devices {
            if dev.group_id == group_id {
                dev.group_id = 0;
            }
        }
        self.groups.retain(|g| g.group_id != group_id);
    }

    fn set_group_volume(&mut self, group_id: u32, volume_q16: i32) {
        if let Some(grp) = self.groups.iter_mut().find(|g| g.group_id == group_id) {
            let clamped = if volume_q16 < 0 { 0 }
                          else if volume_q16 > MAX_VOLUME_Q16 { MAX_VOLUME_Q16 }
                          else { volume_q16 };
            grp.volume_q16 = clamped;
        }
    }

    // -- Status -------------------------------------------------------------

    fn is_casting(&self) -> bool {
        self.session.as_ref().map_or(false, |s| {
            s.state == CastSessionState::Playing || s.state == CastSessionState::Buffering
        })
    }

    fn session_state(&self) -> CastSessionState {
        self.session.as_ref().map_or(CastSessionState::Idle, |s| s.state)
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn start_discovery() {
    let mut cc = CHROMECAST.lock();
    if let Some(mgr) = cc.as_mut() {
        mgr.start_dial_discovery();
    }
}

pub fn stop_discovery() {
    let mut cc = CHROMECAST.lock();
    if let Some(mgr) = cc.as_mut() {
        mgr.stop_discovery();
    }
}

pub fn device_count() -> usize {
    let cc = CHROMECAST.lock();
    cc.as_ref().map_or(0, |m| m.device_count())
}

pub fn start_cast(device_id: u32, app_hash: u64, media_type: MediaType,
                   content_hash: u64, duration_ms: u64) -> bool {
    let mut cc = CHROMECAST.lock();
    cc.as_mut().map_or(false, |m| m.start_cast(device_id, app_hash, media_type, content_hash, duration_ms))
}

pub fn play() {
    let mut cc = CHROMECAST.lock();
    if let Some(mgr) = cc.as_mut() { mgr.play(); }
}

pub fn pause() {
    let mut cc = CHROMECAST.lock();
    if let Some(mgr) = cc.as_mut() { mgr.pause(); }
}

pub fn stop() {
    let mut cc = CHROMECAST.lock();
    if let Some(mgr) = cc.as_mut() { mgr.end_session(); }
}

pub fn is_casting() -> bool {
    let cc = CHROMECAST.lock();
    cc.as_ref().map_or(false, |m| m.is_casting())
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    let mut cc = CHROMECAST.lock();
    *cc = Some(ChromecastManager::new());
    serial_println!("    Chromecast: DIAL discovery, sender/receiver, queue mgmt ready");
}
