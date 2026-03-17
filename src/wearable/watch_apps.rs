use crate::sync::Mutex;
/// Watch app framework for Genesis
///
/// Lightweight app runtime for wearables,
/// tile-based UI, quick actions.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

struct WatchApp {
    id: u32,
    name: [u8; 24],
    name_len: usize,
    installed: bool,
    tile_enabled: bool,
    memory_kb: u32,
    last_used: u64,
}

struct WatchAppManager {
    apps: Vec<WatchApp>,
    next_id: u32,
    max_memory_kb: u32,
    used_memory_kb: u32,
}

static WATCH_APPS: Mutex<Option<WatchAppManager>> = Mutex::new(None);

impl WatchAppManager {
    fn new() -> Self {
        WatchAppManager {
            apps: Vec::new(),
            next_id: 1,
            max_memory_kb: 256 * 1024, // 256 MB for watch
            used_memory_kb: 0,
        }
    }

    fn install(&mut self, name: &[u8], memory_kb: u32) -> Option<u32> {
        if self.used_memory_kb + memory_kb > self.max_memory_kb {
            return None;
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let mut n = [0u8; 24];
        let nlen = name.len().min(24);
        n[..nlen].copy_from_slice(&name[..nlen]);
        self.apps.push(WatchApp {
            id,
            name: n,
            name_len: nlen,
            installed: true,
            tile_enabled: false,
            memory_kb,
            last_used: 0,
        });
        self.used_memory_kb += memory_kb;
        Some(id)
    }

    fn uninstall(&mut self, app_id: u32) {
        if let Some(idx) = self.apps.iter().position(|a| a.id == app_id) {
            self.used_memory_kb -= self.apps[idx].memory_kb;
            self.apps.remove(idx);
        }
    }
}

pub fn init() {
    let mut w = WATCH_APPS.lock();
    *w = Some(WatchAppManager::new());
    serial_println!("    Wearable: watch app framework ready");
}

// ── Watch app tile rendering ──────────────────────────────────────────────────

const FB_STRIDE: u32 = 1920;
const BG: u32 = 0xFF111120;
const BORDER: u32 = 0xFF2D2D4E;
const TILE_BG: u32 = 0xFF1E1E3A;
const TILE_FG: u32 = 0xFFF59E0B;
const DISABLED: u32 = 0xFF3D3D5E;

/// Render the watch-app tile grid onto the kernel framebuffer.
///
/// Draws a 2-column grid of app tiles, one per installed app whose
/// `tile_enabled` flag is set.  Each tile is a rounded rectangle (simulated
/// with a 1-pixel border) filled with `TILE_BG`.  Apps without tile_enabled
/// are shown as dimmed placeholders.
///
/// # Arguments
/// * `fb`     — ARGB framebuffer slice for the full display
/// * `x`, `y` — top-left corner of the app grid area (pixels)
/// * `w`, `h` — width and height of the grid area (pixels)
pub fn render(fb: &mut [u32], x: u32, y: u32, w: u32, h: u32) {
    if w == 0 || h == 0 {
        return;
    }

    // Panel background
    for row in 0..h {
        let py = y.saturating_add(row);
        for col in 0..w {
            let px = x.saturating_add(col);
            let idx = py.saturating_mul(FB_STRIDE).saturating_add(px) as usize;
            if idx < fb.len() {
                fb[idx] = BG;
            }
        }
    }

    // Snapshot the installed app list under lock, then release before drawing
    let apps_snapshot = {
        let guard = WATCH_APPS.lock();
        if let Some(mgr) = guard.as_ref() {
            mgr.apps
                .iter()
                .map(|a| (a.tile_enabled, a.installed))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        }
    };

    if apps_snapshot.is_empty() {
        return;
    }

    let cols: u32 = 2;
    let tile_w = w.saturating_sub(6) / cols;
    let tile_h = tile_w; // square tiles

    for (i, &(tile_on, installed)) in apps_snapshot.iter().enumerate() {
        let i = i as u32;
        let col = i % cols;
        let row = i / cols;

        let tx = x
            .saturating_add(3)
            .saturating_add(col.saturating_mul(tile_w.saturating_add(2)));
        let ty = y
            .saturating_add(3)
            .saturating_add(row.saturating_mul(tile_h.saturating_add(2)));

        if ty.saturating_add(tile_h) > y.saturating_add(h) {
            break;
        }

        let fill = if installed && tile_on {
            TILE_BG
        } else {
            DISABLED
        };

        for dr in 0..tile_h {
            let py = ty.saturating_add(dr);
            for dc in 0..tile_w {
                let px = tx.saturating_add(dc);
                let idx = py.saturating_mul(FB_STRIDE).saturating_add(px) as usize;
                if idx < fb.len() {
                    fb[idx] = if dr == 0 || dr == tile_h - 1 || dc == 0 || dc == tile_w - 1 {
                        BORDER
                    } else {
                        fill
                    };
                }
            }
        }

        // Dot indicator in the centre of each tile
        if tile_w >= 6 && tile_h >= 6 {
            let dot_x = tx.saturating_add(tile_w / 2);
            let dot_y = ty.saturating_add(tile_h / 2);
            let dot_color = if tile_on { TILE_FG } else { 0xFF555566 };
            for dr in 0..3u32 {
                for dc in 0..3u32 {
                    let px = dot_x.saturating_add(dc).saturating_sub(1);
                    let py = dot_y.saturating_add(dr).saturating_sub(1);
                    let idx = py.saturating_mul(FB_STRIDE).saturating_add(px) as usize;
                    if idx < fb.len() {
                        fb[idx] = dot_color;
                    }
                }
            }
        }
    }
}
