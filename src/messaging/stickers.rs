/// Sticker and emoji system for Genesis messaging
///
/// Provides sticker packs, emoji search, recently-used tracking,
/// custom sticker creation, and animated sticker support.
/// All identifiers are u64 hashes. No floating point.

use alloc::vec::Vec;
use alloc::vec;
use crate::sync::Mutex;
use crate::{serial_print, serial_println};

// ── Constants ──────────────────────────────────────────────────────────

/// Maximum number of sticker packs
const MAX_PACKS: usize = 128;

/// Maximum stickers per pack
const MAX_STICKERS_PER_PACK: usize = 64;

/// Maximum recently-used stickers tracked
const MAX_RECENT: usize = 50;

/// Maximum custom stickers per user
const MAX_CUSTOM_PER_USER: usize = 100;

/// Maximum search results returned
const MAX_SEARCH_RESULTS: usize = 40;

/// Maximum number of animation frames for animated stickers
const MAX_ANIMATION_FRAMES: usize = 60;

/// Animation frame duration in milliseconds (default)
const DEFAULT_FRAME_DURATION_MS: u32 = 50;

// ── Types ──────────────────────────────────────────────────────────────

/// Category for organizing sticker packs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StickerCategory {
    /// Smileys and people
    Smileys,
    /// Animals and nature
    Animals,
    /// Food and drink
    Food,
    /// Activities and sports
    Activities,
    /// Travel and places
    Travel,
    /// Objects
    Objects,
    /// Symbols
    Symbols,
    /// Flags
    Flags,
    /// User-created custom category
    Custom,
}

/// Whether a sticker is static or animated
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StickerType {
    /// Static image sticker
    Static,
    /// Animated sticker with multiple frames
    Animated,
    /// Emoji (text-based)
    Emoji,
}

/// A single sticker within a pack
#[derive(Clone, Debug)]
pub struct Sticker {
    /// Unique sticker identifier
    pub id: u64,
    /// Hash of the sticker name/label
    pub name_hash: u64,
    /// Sticker type (static/animated/emoji)
    pub sticker_type: StickerType,
    /// Category tag
    pub category: StickerCategory,
    /// Width in pixels (for rendering)
    pub width: u16,
    /// Height in pixels (for rendering)
    pub height: u16,
    /// Number of animation frames (1 for static)
    pub frame_count: u16,
    /// Frame duration in milliseconds (for animated)
    pub frame_duration_ms: u32,
    /// Hash of the image data blob
    pub data_hash: u64,
    /// Creator user hash (0 for built-in)
    pub creator_hash: u64,
    /// Usage count (for popularity ranking)
    pub use_count: u32,
    /// Tags for search (hashes of keywords)
    pub tags: Vec<u64>,
}

/// A sticker pack containing multiple stickers
#[derive(Clone, Debug)]
pub struct StickerPack {
    /// Unique pack identifier
    pub id: u64,
    /// Hash of the pack name
    pub name_hash: u64,
    /// Category of the pack
    pub category: StickerCategory,
    /// Stickers in this pack
    pub stickers: Vec<Sticker>,
    /// Creator user hash (0 for built-in)
    pub creator_hash: u64,
    /// Whether this pack is installed/enabled
    pub installed: bool,
    /// Creation timestamp
    pub created_at: u64,
}

/// Recently-used sticker entry
#[derive(Clone, Copy, Debug)]
pub struct RecentEntry {
    /// Sticker ID
    pub sticker_id: u64,
    /// Pack ID
    pub pack_id: u64,
    /// Last used timestamp
    pub last_used: u64,
    /// Number of times used
    pub use_count: u32,
}

/// Sticker manager holding all packs and state
pub struct StickerManager {
    packs: Vec<StickerPack>,
    recent: Vec<RecentEntry>,
    next_pack_id: u64,
    next_sticker_id: u64,
}

// ── Global State ───────────────────────────────────────────────────────

static STICKER_MANAGER: Mutex<Option<StickerManager>> = Mutex::new(None);

// ── Helper Functions ───────────────────────────────────────────────────

/// Simple FNV-1a hash for combining tag searches
fn fnv_hash(data: u64, seed: u64) -> u64 {
    let mut h: u64 = seed ^ 0xCBF29CE484222325;
    let bytes = data.to_le_bytes();
    for &b in &bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001B3);
    }
    h
}

// ── StickerManager Implementation ──────────────────────────────────────

impl StickerManager {
    pub fn new() -> Self {
        Self {
            packs: Vec::new(),
            recent: Vec::new(),
            next_pack_id: 1,
            next_sticker_id: 1,
        }
    }

    /// Create a new sticker pack
    pub fn create_pack(&mut self, name_hash: u64, category: StickerCategory,
                       creator_hash: u64, timestamp: u64) -> u64 {
        if self.packs.len() >= MAX_PACKS {
            serial_println!("[stickers] Pack limit reached ({})", MAX_PACKS);
            return 0;
        }
        let id = self.next_pack_id;
        self.next_pack_id = self.next_pack_id.saturating_add(1);
        self.packs.push(StickerPack {
            id,
            name_hash,
            category,
            stickers: Vec::new(),
            creator_hash,
            installed: true,
            created_at: timestamp,
        });
        serial_println!("[stickers] Created pack id={}", id);
        id
    }

    /// Add a sticker to an existing pack
    pub fn add_sticker(&mut self, pack_id: u64, name_hash: u64,
                       sticker_type: StickerType, category: StickerCategory,
                       width: u16, height: u16, data_hash: u64,
                       creator_hash: u64, tags: Vec<u64>) -> u64 {
        let pack = match self.packs.iter_mut().find(|p| p.id == pack_id) {
            Some(p) => p,
            None => return 0,
        };
        if pack.stickers.len() >= MAX_STICKERS_PER_PACK {
            return 0;
        }
        let id = self.next_sticker_id;
        self.next_sticker_id = self.next_sticker_id.saturating_add(1);

        let (frame_count, frame_duration_ms) = match sticker_type {
            StickerType::Animated => (MAX_ANIMATION_FRAMES as u16, DEFAULT_FRAME_DURATION_MS),
            _ => (1, 0),
        };

        pack.stickers.push(Sticker {
            id,
            name_hash,
            sticker_type,
            category,
            width,
            height,
            frame_count,
            frame_duration_ms,
            data_hash,
            creator_hash,
            use_count: 0,
            tags,
        });
        serial_println!("[stickers] Added sticker id={} to pack={}", id, pack_id);
        id
    }

    /// Search stickers by tag hash across all installed packs
    pub fn search(&self, tag_hash: u64) -> Vec<(u64, u64)> {
        let mut results: Vec<(u64, u64)> = Vec::new();
        for pack in &self.packs {
            if !pack.installed { continue; }
            for sticker in &pack.stickers {
                if sticker.name_hash == tag_hash
                    || sticker.tags.contains(&tag_hash)
                    || sticker.category as u64 == tag_hash
                {
                    results.push((pack.id, sticker.id));
                    if results.len() >= MAX_SEARCH_RESULTS {
                        return results;
                    }
                }
            }
        }
        results
    }

    /// Record sticker usage and update recent list
    pub fn record_use(&mut self, pack_id: u64, sticker_id: u64, timestamp: u64) {
        // Update use count on the sticker
        if let Some(pack) = self.packs.iter_mut().find(|p| p.id == pack_id) {
            if let Some(sticker) = pack.stickers.iter_mut().find(|s| s.id == sticker_id) {
                sticker.use_count = sticker.use_count.saturating_add(1);
            }
        }
        // Update recent list
        if let Some(entry) = self.recent.iter_mut().find(|r| r.sticker_id == sticker_id) {
            entry.last_used = timestamp;
            entry.use_count = entry.use_count.saturating_add(1);
        } else {
            if self.recent.len() >= MAX_RECENT {
                // Evict oldest
                let mut oldest_idx = 0;
                let mut oldest_ts = u64::MAX;
                for (i, e) in self.recent.iter().enumerate() {
                    if e.last_used < oldest_ts {
                        oldest_ts = e.last_used;
                        oldest_idx = i;
                    }
                }
                self.recent.remove(oldest_idx);
            }
            self.recent.push(RecentEntry {
                sticker_id,
                pack_id,
                last_used: timestamp,
                use_count: 1,
            });
        }
    }

    /// Get recently used stickers sorted by last used (most recent first)
    pub fn get_recent(&self) -> Vec<RecentEntry> {
        let mut sorted = self.recent.clone();
        // Insertion sort by last_used descending
        let len = sorted.len();
        for i in 1..len {
            let mut j = i;
            while j > 0 && sorted[j - 1].last_used < sorted[j].last_used {
                sorted.swap(j - 1, j);
                j -= 1;
            }
        }
        sorted
    }

    /// Install or uninstall a pack
    pub fn set_installed(&mut self, pack_id: u64, installed: bool) -> bool {
        if let Some(pack) = self.packs.iter_mut().find(|p| p.id == pack_id) {
            pack.installed = installed;
            serial_println!("[stickers] Pack {} installed={}", pack_id, installed);
            return true;
        }
        false
    }

    /// Delete a sticker pack
    pub fn delete_pack(&mut self, pack_id: u64) -> bool {
        let before = self.packs.len();
        self.packs.retain(|p| p.id != pack_id);
        if self.packs.len() < before {
            self.recent.retain(|r| r.pack_id != pack_id);
            serial_println!("[stickers] Deleted pack {}", pack_id);
            return true;
        }
        false
    }

    /// Get total sticker count across all packs
    pub fn total_sticker_count(&self) -> usize {
        let mut count: usize = 0;
        for pack in &self.packs {
            count += pack.stickers.len();
        }
        count
    }

    /// Get pack count
    pub fn pack_count(&self) -> usize {
        self.packs.len()
    }
}

// ── Public API ─────────────────────────────────────────────────────────

pub fn create_pack(name_hash: u64, category: StickerCategory,
                   creator_hash: u64, timestamp: u64) -> u64 {
    let mut guard = STICKER_MANAGER.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.create_pack(name_hash, category, creator_hash, timestamp)
    } else { 0 }
}

pub fn search(tag_hash: u64) -> Vec<(u64, u64)> {
    let guard = STICKER_MANAGER.lock();
    if let Some(mgr) = guard.as_ref() {
        mgr.search(tag_hash)
    } else { Vec::new() }
}

pub fn record_use(pack_id: u64, sticker_id: u64, timestamp: u64) {
    let mut guard = STICKER_MANAGER.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.record_use(pack_id, sticker_id, timestamp);
    }
}

pub fn get_recent() -> Vec<RecentEntry> {
    let guard = STICKER_MANAGER.lock();
    if let Some(mgr) = guard.as_ref() {
        mgr.get_recent()
    } else { Vec::new() }
}

// ── Init ───────────────────────────────────────────────────────────────

pub fn init() {
    let mut guard = STICKER_MANAGER.lock();
    *guard = Some(StickerManager::new());
    serial_println!("[stickers] Sticker system initialized (max_packs={}, max_per_pack={})",
                    MAX_PACKS, MAX_STICKERS_PER_PACK);
}
