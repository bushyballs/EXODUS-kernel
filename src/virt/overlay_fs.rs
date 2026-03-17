use crate::sync::Mutex;
use crate::{serial_print, serial_println};
/// OverlayFS implementation for Genesis
///
/// Provides union mount filesystem with lower/upper/merged layer views,
/// copy-up semantics, whiteout entries, and opaque directory support
/// for container image layering.
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_OVERLAYS: usize = 64;
const MAX_LOWER_LAYERS: usize = 32;
const MAX_ENTRIES_PER_LAYER: usize = 512;
const WHITEOUT_PREFIX: u64 = 0x2E776F7574; // ".wout" marker hash
const OPAQUE_MARKER: u64 = 0x2E6F706171; // ".opaq" marker hash

// ---------------------------------------------------------------------------
// File entry model
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EntryType {
    RegularFile,
    Directory,
    Symlink,
    Whiteout,
    OpaqueDir,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LayerRole {
    Lower,
    Upper,
    Merged,
    Work,
}

#[derive(Clone, Copy)]
pub struct FileEntry {
    pub inode: u64,
    pub name_hash: u64,
    pub parent_hash: u64,
    pub entry_type: EntryType,
    pub size: u64,
    pub permissions: u16,
    pub uid: u32,
    pub gid: u32,
    pub modified_time: u64,
    pub created_time: u64,
    pub data_hash: u64,
    pub link_target_hash: u64,
}

#[derive(Clone)]
pub struct OverlayLayer {
    pub id: u32,
    pub role: LayerRole,
    pub digest: u64,
    pub entries: Vec<FileEntry>,
    pub read_only: bool,
    pub total_size: u64,
    pub entry_count: u32,
}

impl OverlayLayer {
    fn new(id: u32, role: LayerRole, digest: u64, read_only: bool) -> Self {
        Self {
            id,
            role,
            digest,
            entries: Vec::new(),
            read_only,
            total_size: 0,
            entry_count: 0,
        }
    }

    fn add_entry(&mut self, entry: FileEntry) -> Result<(), &'static str> {
        if self.entries.len() >= MAX_ENTRIES_PER_LAYER {
            return Err("Layer entry limit reached");
        }
        self.total_size += entry.size;
        self.entry_count = self.entry_count.saturating_add(1);
        self.entries.push(entry);
        Ok(())
    }

    fn find_entry(&self, name_hash: u64, parent_hash: u64) -> Option<&FileEntry> {
        self.entries
            .iter()
            .find(|e| e.name_hash == name_hash && e.parent_hash == parent_hash)
    }

    fn remove_entry(&mut self, name_hash: u64, parent_hash: u64) -> Option<FileEntry> {
        if let Some(idx) = self
            .entries
            .iter()
            .position(|e| e.name_hash == name_hash && e.parent_hash == parent_hash)
        {
            let entry = self.entries.remove(idx);
            self.total_size = self.total_size.saturating_sub(entry.size);
            self.entry_count = self.entry_count.saturating_sub(1);
            Some(entry)
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Overlay mount
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OverlayState {
    Unmounted,
    Mounting,
    Mounted,
    Error,
}

#[derive(Clone)]
pub struct OverlayMount {
    pub id: u32,
    pub state: OverlayState,
    pub lower_layers: Vec<OverlayLayer>,
    pub upper_layer: OverlayLayer,
    pub work_layer: OverlayLayer,
    pub mount_point_hash: u64,
    pub copy_up_count: u32,
    pub whiteout_count: u32,
    pub opaque_count: u32,
    pub next_inode: u64,
}

impl OverlayMount {
    fn new(id: u32, mount_point_hash: u64) -> Self {
        Self {
            id,
            state: OverlayState::Unmounted,
            lower_layers: Vec::new(),
            upper_layer: OverlayLayer::new(0, LayerRole::Upper, 0, false),
            work_layer: OverlayLayer::new(0, LayerRole::Work, 0, false),
            mount_point_hash,
            copy_up_count: 0,
            whiteout_count: 0,
            opaque_count: 0,
            next_inode: 1000,
        }
    }

    fn alloc_inode(&mut self) -> u64 {
        let inode = self.next_inode;
        self.next_inode = self.next_inode.saturating_add(1);
        inode
    }

    fn add_lower_layer(&mut self, digest: u64) -> Result<u32, &'static str> {
        if self.lower_layers.len() >= MAX_LOWER_LAYERS {
            return Err("Lower layer limit reached");
        }
        let layer_id = (self.lower_layers.len() + 1) as u32;
        let layer = OverlayLayer::new(layer_id, LayerRole::Lower, digest, true);
        self.lower_layers.push(layer);
        Ok(layer_id)
    }

    fn add_entry_to_lower(
        &mut self,
        layer_idx: usize,
        entry: FileEntry,
    ) -> Result<(), &'static str> {
        if layer_idx >= self.lower_layers.len() {
            return Err("Invalid lower layer index");
        }
        self.lower_layers[layer_idx].add_entry(entry)
    }

    /// Resolve a file by walking merged view: upper first, then lower layers
    /// top-to-bottom. Whiteout entries block further lookups.
    fn resolve_entry(&self, name_hash: u64, parent_hash: u64) -> Option<&FileEntry> {
        // Check upper layer first
        if let Some(entry) = self.upper_layer.find_entry(name_hash, parent_hash) {
            if entry.entry_type == EntryType::Whiteout {
                return None; // File is deleted
            }
            return Some(entry);
        }

        // Walk lower layers from top (last added) to bottom (first added)
        for layer in self.lower_layers.iter().rev() {
            // Check for opaque directory: if parent is opaque in upper, skip lower layers
            if let Some(parent_entry) = self.upper_layer.find_entry(parent_hash, 0) {
                if parent_entry.entry_type == EntryType::OpaqueDir {
                    return None;
                }
            }

            if let Some(entry) = layer.find_entry(name_hash, parent_hash) {
                if entry.entry_type == EntryType::Whiteout {
                    return None;
                }
                return Some(entry);
            }
        }

        None
    }

    /// Copy-up: copy entry from lower to upper for modification
    fn copy_up(&mut self, name_hash: u64, parent_hash: u64) -> Result<(), &'static str> {
        // Find the entry in lower layers
        let mut found_entry: Option<FileEntry> = None;

        for layer in self.lower_layers.iter().rev() {
            if let Some(entry) = layer.find_entry(name_hash, parent_hash) {
                found_entry = Some(*entry);
                break;
            }
        }

        let entry = found_entry.ok_or("Entry not found in lower layers")?;

        if entry.entry_type == EntryType::Whiteout {
            return Err("Cannot copy up a whiteout entry");
        }

        // Create copy in upper layer with new inode
        let mut upper_entry = entry;
        upper_entry.inode = self.alloc_inode();
        self.upper_layer.add_entry(upper_entry)?;
        self.copy_up_count = self.copy_up_count.saturating_add(1);

        Ok(())
    }

    /// Create a whiteout entry to mask a lower layer file
    fn create_whiteout(&mut self, name_hash: u64, parent_hash: u64) -> Result<(), &'static str> {
        let inode = self.alloc_inode();
        let whiteout = FileEntry {
            inode,
            name_hash,
            parent_hash,
            entry_type: EntryType::Whiteout,
            size: 0,
            permissions: 0,
            uid: 0,
            gid: 0,
            modified_time: 0,
            created_time: 0,
            data_hash: WHITEOUT_PREFIX,
            link_target_hash: 0,
        };

        self.upper_layer.add_entry(whiteout)?;
        self.whiteout_count = self.whiteout_count.saturating_add(1);
        Ok(())
    }

    /// Mark a directory as opaque (hides all lower layer contents)
    fn create_opaque_dir(
        &mut self,
        dir_name_hash: u64,
        parent_hash: u64,
    ) -> Result<(), &'static str> {
        let inode = self.alloc_inode();
        let opaque = FileEntry {
            inode,
            name_hash: dir_name_hash,
            parent_hash,
            entry_type: EntryType::OpaqueDir,
            size: 0,
            permissions: 0o755,
            uid: 0,
            gid: 0,
            modified_time: 0,
            created_time: 0,
            data_hash: OPAQUE_MARKER,
            link_target_hash: 0,
        };

        self.upper_layer.add_entry(opaque)?;
        self.opaque_count = self.opaque_count.saturating_add(1);
        Ok(())
    }

    /// Write a new or modified file to the upper layer
    fn write_file(
        &mut self,
        name_hash: u64,
        parent_hash: u64,
        data_hash: u64,
        size: u64,
        permissions: u16,
        uid: u32,
        gid: u32,
    ) -> Result<u64, &'static str> {
        // Remove existing upper entry if present
        let _ = self.upper_layer.remove_entry(name_hash, parent_hash);

        let inode = self.alloc_inode();
        let entry = FileEntry {
            inode,
            name_hash,
            parent_hash,
            entry_type: EntryType::RegularFile,
            size,
            permissions,
            uid,
            gid,
            modified_time: 0,
            created_time: 0,
            data_hash,
            link_target_hash: 0,
        };

        self.upper_layer.add_entry(entry)?;
        Ok(inode)
    }

    /// Create a directory in the upper layer
    fn create_directory(
        &mut self,
        name_hash: u64,
        parent_hash: u64,
        permissions: u16,
        uid: u32,
        gid: u32,
    ) -> Result<u64, &'static str> {
        let inode = self.alloc_inode();
        let entry = FileEntry {
            inode,
            name_hash,
            parent_hash,
            entry_type: EntryType::Directory,
            size: 0,
            permissions,
            uid,
            gid,
            modified_time: 0,
            created_time: 0,
            data_hash: 0,
            link_target_hash: 0,
        };

        self.upper_layer.add_entry(entry)?;
        Ok(inode)
    }

    /// Delete a file (creates whiteout if exists in lower)
    fn delete_file(&mut self, name_hash: u64, parent_hash: u64) -> Result<(), &'static str> {
        // Remove from upper if present
        let _ = self.upper_layer.remove_entry(name_hash, parent_hash);

        // Check if it exists in lower layers; if so, create whiteout
        let in_lower = self
            .lower_layers
            .iter()
            .any(|l| l.find_entry(name_hash, parent_hash).is_some());

        if in_lower {
            self.create_whiteout(name_hash, parent_hash)?;
        }

        Ok(())
    }

    /// List directory entries in merged view
    fn list_directory(&self, parent_hash: u64) -> Vec<(u64, EntryType, u64)> {
        let mut result: Vec<(u64, EntryType, u64)> = Vec::new();
        let mut seen: Vec<u64> = Vec::new();
        let mut whiteouts: Vec<u64> = Vec::new();

        // Check if directory is opaque in upper
        let is_opaque = self
            .upper_layer
            .entries
            .iter()
            .any(|e| e.parent_hash == parent_hash && e.entry_type == EntryType::OpaqueDir);

        // Collect from upper layer
        for entry in self.upper_layer.entries.iter() {
            if entry.parent_hash == parent_hash {
                if entry.entry_type == EntryType::Whiteout {
                    whiteouts.push(entry.name_hash);
                } else if entry.entry_type != EntryType::OpaqueDir {
                    result.push((entry.name_hash, entry.entry_type, entry.size));
                    seen.push(entry.name_hash);
                }
            }
        }

        // If not opaque, also collect from lower layers
        if !is_opaque {
            for layer in self.lower_layers.iter().rev() {
                for entry in layer.entries.iter() {
                    if entry.parent_hash == parent_hash
                        && !seen.contains(&entry.name_hash)
                        && !whiteouts.contains(&entry.name_hash)
                        && entry.entry_type != EntryType::Whiteout
                    {
                        result.push((entry.name_hash, entry.entry_type, entry.size));
                        seen.push(entry.name_hash);
                    }
                }
            }
        }

        result
    }

    fn total_entries(&self) -> u32 {
        let upper = self.upper_layer.entry_count;
        let lower: u32 = self.lower_layers.iter().map(|l| l.entry_count).sum();
        upper + lower
    }

    fn total_size(&self) -> u64 {
        let upper = self.upper_layer.total_size;
        let lower: u64 = self.lower_layers.iter().map(|l| l.total_size).sum();
        upper + lower
    }
}

// ---------------------------------------------------------------------------
// OverlayFS manager
// ---------------------------------------------------------------------------

pub struct OverlayFsManager {
    overlays: Vec<OverlayMount>,
    next_id: u32,
    total_mounts: u32,
}

impl OverlayFsManager {
    fn new() -> Self {
        Self {
            overlays: Vec::new(),
            next_id: 1,
            total_mounts: 0,
        }
    }

    pub fn create_overlay(&mut self, mount_point_hash: u64) -> Result<u32, &'static str> {
        if self.overlays.len() >= MAX_OVERLAYS {
            return Err("Overlay limit reached");
        }

        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.overlays.push(OverlayMount::new(id, mount_point_hash));
        Ok(id)
    }

    pub fn add_lower_layer(&mut self, overlay_id: u32, digest: u64) -> Result<u32, &'static str> {
        let overlay = self
            .overlays
            .iter_mut()
            .find(|o| o.id == overlay_id)
            .ok_or("Overlay not found")?;

        if overlay.state != OverlayState::Unmounted {
            return Err("Cannot add layer to mounted overlay");
        }

        overlay.add_lower_layer(digest)
    }

    pub fn add_entry_to_lower(
        &mut self,
        overlay_id: u32,
        layer_idx: usize,
        entry: FileEntry,
    ) -> Result<(), &'static str> {
        let overlay = self
            .overlays
            .iter_mut()
            .find(|o| o.id == overlay_id)
            .ok_or("Overlay not found")?;
        overlay.add_entry_to_lower(layer_idx, entry)
    }

    pub fn mount(&mut self, overlay_id: u32) -> Result<(), &'static str> {
        let overlay = self
            .overlays
            .iter_mut()
            .find(|o| o.id == overlay_id)
            .ok_or("Overlay not found")?;

        if overlay.state != OverlayState::Unmounted {
            return Err("Overlay already mounted");
        }

        if overlay.lower_layers.is_empty() {
            return Err("No lower layers configured");
        }

        overlay.state = OverlayState::Mounted;
        self.total_mounts = self.total_mounts.saturating_add(1);
        Ok(())
    }

    pub fn unmount(&mut self, overlay_id: u32) -> Result<(), &'static str> {
        let overlay = self
            .overlays
            .iter_mut()
            .find(|o| o.id == overlay_id)
            .ok_or("Overlay not found")?;

        if overlay.state != OverlayState::Mounted {
            return Err("Overlay not mounted");
        }

        overlay.state = OverlayState::Unmounted;
        Ok(())
    }

    pub fn resolve_file(
        &self,
        overlay_id: u32,
        name_hash: u64,
        parent_hash: u64,
    ) -> Result<Option<FileEntry>, &'static str> {
        let overlay = self
            .overlays
            .iter()
            .find(|o| o.id == overlay_id)
            .ok_or("Overlay not found")?;

        if overlay.state != OverlayState::Mounted {
            return Err("Overlay not mounted");
        }

        Ok(overlay.resolve_entry(name_hash, parent_hash).copied())
    }

    pub fn write_file(
        &mut self,
        overlay_id: u32,
        name_hash: u64,
        parent_hash: u64,
        data_hash: u64,
        size: u64,
        permissions: u16,
        uid: u32,
        gid: u32,
    ) -> Result<u64, &'static str> {
        let overlay = self
            .overlays
            .iter_mut()
            .find(|o| o.id == overlay_id)
            .ok_or("Overlay not found")?;

        if overlay.state != OverlayState::Mounted {
            return Err("Overlay not mounted");
        }

        overlay.write_file(
            name_hash,
            parent_hash,
            data_hash,
            size,
            permissions,
            uid,
            gid,
        )
    }

    pub fn create_directory(
        &mut self,
        overlay_id: u32,
        name_hash: u64,
        parent_hash: u64,
        permissions: u16,
        uid: u32,
        gid: u32,
    ) -> Result<u64, &'static str> {
        let overlay = self
            .overlays
            .iter_mut()
            .find(|o| o.id == overlay_id)
            .ok_or("Overlay not found")?;

        if overlay.state != OverlayState::Mounted {
            return Err("Overlay not mounted");
        }

        overlay.create_directory(name_hash, parent_hash, permissions, uid, gid)
    }

    pub fn delete_file(
        &mut self,
        overlay_id: u32,
        name_hash: u64,
        parent_hash: u64,
    ) -> Result<(), &'static str> {
        let overlay = self
            .overlays
            .iter_mut()
            .find(|o| o.id == overlay_id)
            .ok_or("Overlay not found")?;

        if overlay.state != OverlayState::Mounted {
            return Err("Overlay not mounted");
        }

        overlay.delete_file(name_hash, parent_hash)
    }

    pub fn copy_up(
        &mut self,
        overlay_id: u32,
        name_hash: u64,
        parent_hash: u64,
    ) -> Result<(), &'static str> {
        let overlay = self
            .overlays
            .iter_mut()
            .find(|o| o.id == overlay_id)
            .ok_or("Overlay not found")?;

        if overlay.state != OverlayState::Mounted {
            return Err("Overlay not mounted");
        }

        overlay.copy_up(name_hash, parent_hash)
    }

    pub fn create_opaque_dir(
        &mut self,
        overlay_id: u32,
        dir_name_hash: u64,
        parent_hash: u64,
    ) -> Result<(), &'static str> {
        let overlay = self
            .overlays
            .iter_mut()
            .find(|o| o.id == overlay_id)
            .ok_or("Overlay not found")?;

        if overlay.state != OverlayState::Mounted {
            return Err("Overlay not mounted");
        }

        overlay.create_opaque_dir(dir_name_hash, parent_hash)
    }

    pub fn list_directory(
        &self,
        overlay_id: u32,
        parent_hash: u64,
    ) -> Result<Vec<(u64, EntryType, u64)>, &'static str> {
        let overlay = self
            .overlays
            .iter()
            .find(|o| o.id == overlay_id)
            .ok_or("Overlay not found")?;

        if overlay.state != OverlayState::Mounted {
            return Err("Overlay not mounted");
        }

        Ok(overlay.list_directory(parent_hash))
    }

    pub fn get_stats(&self, overlay_id: u32) -> Result<(u32, u64, u32, u32, u32), &'static str> {
        let overlay = self
            .overlays
            .iter()
            .find(|o| o.id == overlay_id)
            .ok_or("Overlay not found")?;
        Ok((
            overlay.total_entries(),
            overlay.total_size(),
            overlay.copy_up_count,
            overlay.whiteout_count,
            overlay.opaque_count,
        ))
    }

    pub fn overlay_count(&self) -> usize {
        self.overlays.len()
    }

    pub fn mounted_count(&self) -> usize {
        self.overlays
            .iter()
            .filter(|o| o.state == OverlayState::Mounted)
            .count()
    }

    pub fn remove_overlay(&mut self, overlay_id: u32) -> Result<(), &'static str> {
        let idx = self
            .overlays
            .iter()
            .position(|o| o.id == overlay_id)
            .ok_or("Overlay not found")?;

        if self.overlays[idx].state == OverlayState::Mounted {
            return Err("Cannot remove mounted overlay");
        }

        self.overlays.remove(idx);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Global singleton
// ---------------------------------------------------------------------------

static OVERLAY_FS: Mutex<Option<OverlayFsManager>> = Mutex::new(None);

pub fn init() {
    let mut mgr = OVERLAY_FS.lock();
    *mgr = Some(OverlayFsManager::new());
    serial_println!(
        "[OVERLAYFS] Overlay filesystem initialized (max: {} overlays, {} layers each)",
        MAX_OVERLAYS,
        MAX_LOWER_LAYERS
    );
}

// -- Public API wrappers ----------------------------------------------------

pub fn create_overlay(mount_point_hash: u64) -> Result<u32, &'static str> {
    let mut mgr = OVERLAY_FS.lock();
    mgr.as_mut()
        .ok_or("OverlayFS not initialized")?
        .create_overlay(mount_point_hash)
}

pub fn add_lower_layer(overlay_id: u32, digest: u64) -> Result<u32, &'static str> {
    let mut mgr = OVERLAY_FS.lock();
    mgr.as_mut()
        .ok_or("OverlayFS not initialized")?
        .add_lower_layer(overlay_id, digest)
}

pub fn mount_overlay(overlay_id: u32) -> Result<(), &'static str> {
    let mut mgr = OVERLAY_FS.lock();
    mgr.as_mut()
        .ok_or("OverlayFS not initialized")?
        .mount(overlay_id)
}

pub fn unmount_overlay(overlay_id: u32) -> Result<(), &'static str> {
    let mut mgr = OVERLAY_FS.lock();
    mgr.as_mut()
        .ok_or("OverlayFS not initialized")?
        .unmount(overlay_id)
}

pub fn resolve_file(
    overlay_id: u32,
    name_hash: u64,
    parent_hash: u64,
) -> Result<Option<FileEntry>, &'static str> {
    let mgr = OVERLAY_FS.lock();
    mgr.as_ref()
        .ok_or("OverlayFS not initialized")?
        .resolve_file(overlay_id, name_hash, parent_hash)
}

pub fn write_file(
    overlay_id: u32,
    name_hash: u64,
    parent_hash: u64,
    data_hash: u64,
    size: u64,
    permissions: u16,
    uid: u32,
    gid: u32,
) -> Result<u64, &'static str> {
    let mut mgr = OVERLAY_FS.lock();
    mgr.as_mut().ok_or("OverlayFS not initialized")?.write_file(
        overlay_id,
        name_hash,
        parent_hash,
        data_hash,
        size,
        permissions,
        uid,
        gid,
    )
}

pub fn delete_file(overlay_id: u32, name_hash: u64, parent_hash: u64) -> Result<(), &'static str> {
    let mut mgr = OVERLAY_FS.lock();
    mgr.as_mut()
        .ok_or("OverlayFS not initialized")?
        .delete_file(overlay_id, name_hash, parent_hash)
}

pub fn copy_up(overlay_id: u32, name_hash: u64, parent_hash: u64) -> Result<(), &'static str> {
    let mut mgr = OVERLAY_FS.lock();
    mgr.as_mut()
        .ok_or("OverlayFS not initialized")?
        .copy_up(overlay_id, name_hash, parent_hash)
}

pub fn list_directory(
    overlay_id: u32,
    parent_hash: u64,
) -> Result<Vec<(u64, EntryType, u64)>, &'static str> {
    let mgr = OVERLAY_FS.lock();
    mgr.as_ref()
        .ok_or("OverlayFS not initialized")?
        .list_directory(overlay_id, parent_hash)
}

pub fn get_overlay_stats(overlay_id: u32) -> Result<(u32, u64, u32, u32, u32), &'static str> {
    let mgr = OVERLAY_FS.lock();
    mgr.as_ref()
        .ok_or("OverlayFS not initialized")?
        .get_stats(overlay_id)
}

pub fn overlay_count() -> usize {
    let mgr = OVERLAY_FS.lock();
    match mgr.as_ref() {
        Some(m) => m.overlay_count(),
        None => 0,
    }
}

pub fn mounted_count() -> usize {
    let mgr = OVERLAY_FS.lock();
    match mgr.as_ref() {
        Some(m) => m.mounted_count(),
        None => 0,
    }
}

pub fn remove_overlay(overlay_id: u32) -> Result<(), &'static str> {
    let mut mgr = OVERLAY_FS.lock();
    mgr.as_mut()
        .ok_or("OverlayFS not initialized")?
        .remove_overlay(overlay_id)
}
