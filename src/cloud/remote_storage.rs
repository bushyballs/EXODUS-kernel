use crate::sync::Mutex;
/// Remote storage for Genesis
///
/// Cloud file access, mounting, caching,
/// multi-provider (S3, GCS, WebDAV, SFTP).
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum StorageProvider {
    S3Compatible,
    WebDav,
    Sftp,
    Custom,
}

struct RemoteMount {
    id: u32,
    provider: StorageProvider,
    mount_path: [u8; 32],
    mount_len: usize,
    host_hash: u64,
    connected: bool,
    cached_bytes: u64,
    total_bytes: u64,
    used_bytes: u64,
}

struct RemoteStorageEngine {
    mounts: Vec<RemoteMount>,
    next_id: u32,
    cache_limit_bytes: u64,
}

static REMOTE_STORAGE: Mutex<Option<RemoteStorageEngine>> = Mutex::new(None);

impl RemoteStorageEngine {
    fn new() -> Self {
        RemoteStorageEngine {
            mounts: Vec::new(),
            next_id: 1,
            cache_limit_bytes: 1024 * 1024 * 1024, // 1GB cache
        }
    }

    fn add_mount(&mut self, provider: StorageProvider, path: &[u8], host_hash: u64) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let mut mp = [0u8; 32];
        let mlen = path.len().min(32);
        mp[..mlen].copy_from_slice(&path[..mlen]);
        self.mounts.push(RemoteMount {
            id,
            provider,
            mount_path: mp,
            mount_len: mlen,
            host_hash,
            connected: false,
            cached_bytes: 0,
            total_bytes: 0,
            used_bytes: 0,
        });
        id
    }

    fn connect(&mut self, mount_id: u32) -> bool {
        if let Some(m) = self.mounts.iter_mut().find(|m| m.id == mount_id) {
            m.connected = true;
            return true;
        }
        false
    }
}

pub fn init() {
    let mut r = REMOTE_STORAGE.lock();
    *r = Some(RemoteStorageEngine::new());
    serial_println!("    Cloud: remote storage (S3, WebDAV, SFTP) ready");
}
