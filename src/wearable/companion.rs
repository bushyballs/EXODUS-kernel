use crate::sync::Mutex;
/// Wearable companion for Genesis
///
/// Phone-to-watch sync, notification mirroring,
/// health data sync, app install, find my watch.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum WearableType {
    Watch,
    Band,
    Ring,
    Glasses,
    Earbuds,
}

#[derive(Clone, Copy, PartialEq)]
pub enum SyncState {
    Disconnected,
    Connecting,
    Connected,
    Syncing,
}

struct PairedWearable {
    id: u32,
    wearable_type: WearableType,
    name: [u8; 24],
    name_len: usize,
    state: SyncState,
    battery_pct: u8,
    firmware_version: u32,
    mirror_notifications: bool,
    sync_health: bool,
    last_sync: u64,
}

struct CompanionEngine {
    paired: Vec<PairedWearable>,
    next_id: u32,
}

static COMPANION: Mutex<Option<CompanionEngine>> = Mutex::new(None);

impl CompanionEngine {
    fn new() -> Self {
        CompanionEngine {
            paired: Vec::new(),
            next_id: 1,
        }
    }

    fn pair(&mut self, wtype: WearableType, name: &[u8]) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let mut n = [0u8; 24];
        let nlen = name.len().min(24);
        n[..nlen].copy_from_slice(&name[..nlen]);
        self.paired.push(PairedWearable {
            id,
            wearable_type: wtype,
            name: n,
            name_len: nlen,
            state: SyncState::Connected,
            battery_pct: 100,
            firmware_version: 1,
            mirror_notifications: true,
            sync_health: true,
            last_sync: 0,
        });
        id
    }

    fn sync(&mut self, device_id: u32, timestamp: u64) {
        if let Some(d) = self.paired.iter_mut().find(|d| d.id == device_id) {
            d.state = SyncState::Syncing;
            d.last_sync = timestamp;
            d.state = SyncState::Connected;
        }
    }
}

pub fn init() {
    let mut c = COMPANION.lock();
    *c = Some(CompanionEngine::new());
    serial_println!("    Wearable: companion sync ready");
}

// ── BLE sync status sensor stub ───────────────────────────────────────────────

use crate::sync::Mutex as BleSync;

/// Battery percentage of the primary paired wearable (0-100; 255 = unknown).
static WEARABLE_BATTERY: BleSync<u8> = BleSync::new(255);

/// Update the wearable's battery level from a BLE sync packet.
pub fn set_wearable_battery(pct: u8) {
    *WEARABLE_BATTERY.lock() = pct.min(100);
}

/// Read the last reported wearable battery level.
pub fn wearable_battery() -> u8 {
    *WEARABLE_BATTERY.lock()
}

// ── Companion status display ──────────────────────────────────────────────────

const FB_STRIDE: u32 = 1920;
const BG: u32 = 0xFF0D0D1A;
const BORDER: u32 = 0xFF2D2D4E;
const CONNECTED: u32 = 0xFF10B981;
const SYNCING: u32 = 0xFF3B82F6;
const DISC: u32 = 0xFF6B7280;

/// Render the companion-device status panel onto the kernel framebuffer.
///
/// Shows a simple status indicator:
///   - green  if any wearable is Connected
///   - blue   if any wearable is Syncing
///   - grey   if all are Disconnected
///
/// A battery bar is drawn along the bottom of the panel using the value
/// reported via `set_wearable_battery()`.
///
/// # Arguments
/// * `fb`     — ARGB framebuffer slice for the full display
/// * `x`, `y` — top-left corner of the status panel (pixels)
/// * `w`, `h` — width and height of the panel (pixels)
pub fn render(fb: &mut [u32], x: u32, y: u32, w: u32, h: u32) {
    if w == 0 || h == 0 {
        return;
    }

    // Determine status colour from the first paired device
    let status_color = {
        let guard = COMPANION.lock();
        if let Some(engine) = guard.as_ref() {
            if engine.paired.iter().any(|d| d.state == SyncState::Syncing) {
                SYNCING
            } else if engine
                .paired
                .iter()
                .any(|d| d.state == SyncState::Connected)
            {
                CONNECTED
            } else {
                DISC
            }
        } else {
            DISC
        }
    };

    // Fill panel
    for row in 0..h {
        let py = y.saturating_add(row);
        for col in 0..w {
            let px = x.saturating_add(col);
            let idx = py.saturating_mul(FB_STRIDE).saturating_add(px) as usize;
            if idx < fb.len() {
                fb[idx] = if row == 0 || row == h - 1 || col == 0 || col == w - 1 {
                    BORDER
                } else if row < 4 {
                    status_color // status header strip
                } else {
                    BG
                };
            }
        }
    }

    // Battery bar along the bottom
    let battery_pct = *WEARABLE_BATTERY.lock();
    if battery_pct <= 100 && h >= 6 && w >= 4 {
        let bar_w = (battery_pct as u32).saturating_mul(w.saturating_sub(4)) / 100;
        let bar_y = y.saturating_add(h).saturating_sub(4);
        let bar_x = x.saturating_add(2);
        let bat_color = if battery_pct > 20 {
            CONNECTED
        } else {
            0xFFEF4444
        };
        for dr in 0..2u32 {
            for dc in 0..bar_w {
                let px = bar_x.saturating_add(dc);
                let py = bar_y.saturating_add(dr);
                let idx = py.saturating_mul(FB_STRIDE).saturating_add(px) as usize;
                if idx < fb.len() {
                    fb[idx] = bat_color;
                }
            }
        }
    }
}
