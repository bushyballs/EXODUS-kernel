use crate::sync::Mutex;
/// Audio casting / multi-room for Genesis
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy)]
struct AudioTarget {
    id: u32,
    name_hash: u64,
    ip_hash: u64,
    volume: u8,
    synced: bool,
    latency_ms: u16,
    active: bool,
}

struct AudioCastManager {
    targets: Vec<AudioTarget>,
    active_group: Vec<u32>,
    master_volume: u8,
    sync_enabled: bool,
    next_id: u32,
}

static AUDIO_CAST: Mutex<Option<AudioCastManager>> = Mutex::new(None);

impl AudioCastManager {
    fn new() -> Self {
        AudioCastManager {
            targets: Vec::new(),
            active_group: Vec::new(),
            master_volume: 80,
            sync_enabled: true,
            next_id: 1,
        }
    }

    fn add_target(&mut self, name_hash: u64, ip_hash: u64) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.targets.push(AudioTarget {
            id,
            name_hash,
            ip_hash,
            volume: 80,
            synced: false,
            latency_ms: 0,
            active: false,
        });
        id
    }

    fn create_group(&mut self, target_ids: &[u32]) {
        self.active_group.clear();
        for &tid in target_ids {
            if let Some(t) = self.targets.iter_mut().find(|t| t.id == tid) {
                t.active = true;
                t.synced = self.sync_enabled;
                self.active_group.push(tid);
            }
        }
    }

    fn set_volume(&mut self, target_id: u32, volume: u8) {
        if let Some(t) = self.targets.iter_mut().find(|t| t.id == target_id) {
            t.volume = volume.min(100);
        }
    }

    fn dissolve_group(&mut self) {
        for t in &mut self.targets {
            t.active = false;
            t.synced = false;
        }
        self.active_group.clear();
    }
}

pub fn init() {
    let mut ac = AUDIO_CAST.lock();
    *ac = Some(AudioCastManager::new());
    serial_println!("    Audio cast: multi-room sync ready");
}
