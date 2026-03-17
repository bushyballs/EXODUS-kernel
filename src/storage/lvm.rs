/// Logical Volume Manager for Genesis
///
/// Physical volumes, volume groups, logical volumes with create/extend/reduce,
/// snapshot support, thin provisioning, and metadata management.
///
/// Inspired by: Linux LVM2, FreeBSD ZFS volumes. All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Physical extent size (default 4 MiB).
const DEFAULT_PE_SIZE: u64 = 4 * 1024 * 1024;

/// State of a physical volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PvState {
    Active,
    Missing,
    Disabled,
}

/// Physical Volume — wraps a raw block device.
pub struct PhysicalVolume {
    pub pv_id: u32,
    pub device_path: String,
    pub state: PvState,
    pub total_bytes: u64,
    pub pe_size: u64,
    pub total_extents: u32,
    pub free_extents: u32,
    pub vg_id: Option<u32>,
    pub metadata_offset: u64,
}

/// Volume group state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VgState {
    Active,
    Exported,
    Partial,
    Inactive,
}

/// Volume Group — aggregates one or more PVs.
pub struct VolumeGroup {
    pub vg_id: u32,
    pub name: String,
    pub state: VgState,
    pub pv_ids: Vec<u32>,
    pub pe_size: u64,
    pub total_extents: u32,
    pub free_extents: u32,
    pub lv_ids: Vec<u32>,
    pub created_at: u64,
    pub max_lv: u32,
    pub max_pv: u32,
}

/// Logical volume type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LvType {
    Linear,
    Striped,
    Mirror,
    ThinPool,
    ThinVolume,
    Snapshot,
}

/// Logical volume state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LvState {
    Active,
    Inactive,
    Suspended,
    SnapshotInvalid,
}

/// Extent mapping — which PV and starting PE a range of LEs maps to.
pub struct ExtentMap {
    pub le_start: u32,
    pub le_count: u32,
    pub pv_id: u32,
    pub pe_start: u32,
}

/// Snapshot metadata.
pub struct SnapshotMeta {
    pub origin_lv_id: u32,
    pub cow_used_extents: u32,
    pub cow_total_extents: u32,
    pub created_at: u64,
    /// Usage percent in Q16 fixed-point (0 .. 100<<16).
    pub usage_q16: i32,
}

/// Thin provisioning pool metadata.
pub struct ThinPoolMeta {
    pub data_extents: u32,
    pub data_used: u32,
    pub metadata_extents: u32,
    pub metadata_used: u32,
    pub thin_ids: Vec<u32>,
    /// Overcommit ratio in Q16 (e.g. 2<<16 = 2x).
    pub overcommit_q16: i32,
}

/// Logical Volume.
pub struct LogicalVolume {
    pub lv_id: u32,
    pub name: String,
    pub vg_id: u32,
    pub lv_type: LvType,
    pub state: LvState,
    pub extent_count: u32,
    pub size_bytes: u64,
    pub mappings: Vec<ExtentMap>,
    pub snapshot: Option<SnapshotMeta>,
    pub thin_pool: Option<ThinPoolMeta>,
    pub read_only: bool,
    pub created_at: u64,
}

// ---------------------------------------------------------------------------
// LVM Manager
// ---------------------------------------------------------------------------

pub struct LvmManager {
    pub pvs: Vec<PhysicalVolume>,
    pub vgs: Vec<VolumeGroup>,
    pub lvs: Vec<LogicalVolume>,
    pub next_pv_id: u32,
    pub next_vg_id: u32,
    pub next_lv_id: u32,
    pub metadata_version: u32,
    pub journal: Vec<String>,
}

impl LvmManager {
    const fn new() -> Self {
        LvmManager {
            pvs: Vec::new(),
            vgs: Vec::new(),
            lvs: Vec::new(),
            next_pv_id: 1,
            next_vg_id: 1,
            next_lv_id: 1,
            metadata_version: 1,
            journal: Vec::new(),
        }
    }

    fn log_op(&mut self, msg: String) {
        self.metadata_version = self.metadata_version.saturating_add(1);
        self.journal.push(msg);
    }

    // ---- Physical Volume operations ----

    /// Initialize a device as a physical volume.
    pub fn pv_create(&mut self, device: &str, size_bytes: u64) -> u32 {
        let pe_size = DEFAULT_PE_SIZE;
        let total_extents = (size_bytes / pe_size) as u32;
        let id = self.next_pv_id;
        self.next_pv_id = self.next_pv_id.saturating_add(1);

        self.pvs.push(PhysicalVolume {
            pv_id: id,
            device_path: String::from(device),
            state: PvState::Active,
            total_bytes: size_bytes,
            pe_size,
            total_extents,
            free_extents: total_extents,
            vg_id: None,
            metadata_offset: 4096, // after label
        });

        self.log_op(format!("pvcreate {} ({} extents)", device, total_extents));
        serial_println!("  [lvm] PV created: {} ({} extents)", device, total_extents);
        id
    }

    /// Remove a physical volume (must not belong to a VG).
    pub fn pv_remove(&mut self, pv_id: u32) -> bool {
        if let Some(pv) = self.pvs.iter().find(|p| p.pv_id == pv_id) {
            if pv.vg_id.is_some() {
                serial_println!("  [lvm] Cannot remove PV {} — still in a VG", pv_id);
                return false;
            }
        }
        let before = self.pvs.len();
        self.pvs.retain(|p| p.pv_id != pv_id);
        let removed = self.pvs.len() < before;
        if removed {
            self.log_op(format!("pvremove {}", pv_id));
        }
        removed
    }

    // ---- Volume Group operations ----

    /// Create a volume group from one or more PVs.
    pub fn vg_create(&mut self, name: &str, pv_ids: &[u32]) -> Option<u32> {
        if pv_ids.is_empty() {
            return None;
        }

        let mut total_ext = 0u32;
        for &pid in pv_ids {
            match self.pvs.iter().find(|p| p.pv_id == pid) {
                Some(pv) if pv.vg_id.is_none() => {
                    total_ext += pv.total_extents;
                }
                _ => {
                    serial_println!("  [lvm] PV {} not available for VG", pid);
                    return None;
                }
            }
        }

        let vg_id = self.next_vg_id;
        self.next_vg_id = self.next_vg_id.saturating_add(1);

        // Assign PVs to VG
        for &pid in pv_ids {
            if let Some(pv) = self.pvs.iter_mut().find(|p| p.pv_id == pid) {
                pv.vg_id = Some(vg_id);
            }
        }

        self.vgs.push(VolumeGroup {
            vg_id,
            name: String::from(name),
            state: VgState::Active,
            pv_ids: pv_ids.into(),
            pe_size: DEFAULT_PE_SIZE,
            total_extents: total_ext,
            free_extents: total_ext,
            lv_ids: Vec::new(),
            created_at: crate::time::clock::unix_time(),
            max_lv: 256,
            max_pv: 64,
        });

        self.log_op(format!(
            "vgcreate {} ({} PVs, {} extents)",
            name,
            pv_ids.len(),
            total_ext
        ));
        serial_println!("  [lvm] VG '{}' created ({} extents)", name, total_ext);
        Some(vg_id)
    }

    /// Extend an existing VG by adding more PVs.
    pub fn vg_extend(&mut self, vg_id: u32, pv_ids: &[u32]) -> bool {
        let mut added_ext = 0u32;
        for &pid in pv_ids {
            if let Some(pv) = self
                .pvs
                .iter_mut()
                .find(|p| p.pv_id == pid && p.vg_id.is_none())
            {
                pv.vg_id = Some(vg_id);
                added_ext += pv.total_extents;
            } else {
                return false;
            }
        }

        let vg_idx = self.vgs.iter().position(|v| v.vg_id == vg_id);
        if let Some(i) = vg_idx {
            let vg_name = self.vgs[i].name.clone();
            for &pid in pv_ids {
                self.vgs[i].pv_ids.push(pid);
            }
            self.vgs[i].total_extents += added_ext;
            self.vgs[i].free_extents += added_ext;
            self.log_op(format!("vgextend {} (+{} extents)", vg_name, added_ext));
            true
        } else {
            false
        }
    }

    // ---- Logical Volume operations ----

    /// Create a linear logical volume.
    pub fn lv_create(&mut self, vg_id: u32, name: &str, extent_count: u32) -> Option<u32> {
        let vg = self.vgs.iter_mut().find(|v| v.vg_id == vg_id)?;
        if extent_count > vg.free_extents {
            serial_println!("  [lvm] Not enough free extents in VG '{}'", vg.name);
            return None;
        }

        let lv_id = self.next_lv_id;
        self.next_lv_id = self.next_lv_id.saturating_add(1);

        // Simple allocation: first-fit across PVs
        let mut remaining = extent_count;
        let mut mappings = Vec::new();
        let pv_ids_copy: Vec<u32> = vg.pv_ids.clone();

        for &pid in &pv_ids_copy {
            if remaining == 0 {
                break;
            }
            if let Some(pv) = self.pvs.iter_mut().find(|p| p.pv_id == pid) {
                let take = remaining.min(pv.free_extents);
                if take > 0 {
                    let pe_start = pv.total_extents - pv.free_extents;
                    mappings.push(ExtentMap {
                        le_start: extent_count - remaining,
                        le_count: take,
                        pv_id: pid,
                        pe_start,
                    });
                    pv.free_extents -= take;
                    remaining -= take;
                }
            }
        }

        if remaining > 0 {
            // Shouldn't happen but guard
            serial_println!("  [lvm] Allocation incomplete for LV '{}'", name);
            return None;
        }

        let pe_size = self
            .vgs
            .iter()
            .find(|v| v.vg_id == vg_id)
            .map(|v| v.pe_size)
            .unwrap_or(DEFAULT_PE_SIZE);
        let size_bytes = (extent_count as u64) * pe_size;

        let lv = LogicalVolume {
            lv_id,
            name: String::from(name),
            vg_id,
            lv_type: LvType::Linear,
            state: LvState::Active,
            extent_count,
            size_bytes,
            mappings,
            snapshot: None,
            thin_pool: None,
            read_only: false,
            created_at: crate::time::clock::unix_time(),
        };

        if let Some(vg) = self.vgs.iter_mut().find(|v| v.vg_id == vg_id) {
            vg.free_extents -= extent_count;
            vg.lv_ids.push(lv_id);
        }

        self.log_op(format!("lvcreate {} ({} extents)", name, extent_count));
        serial_println!("  [lvm] LV '{}' created ({} bytes)", name, size_bytes);
        self.lvs.push(lv);
        Some(lv_id)
    }

    /// Extend a logical volume by additional extents.
    pub fn lv_extend(&mut self, lv_id: u32, extra_extents: u32) -> bool {
        let vg_id = match self.lvs.iter().find(|l| l.lv_id == lv_id) {
            Some(lv) => lv.vg_id,
            None => return false,
        };
        let vg = match self.vgs.iter().find(|v| v.vg_id == vg_id) {
            Some(v) => v,
            None => return false,
        };
        if extra_extents > vg.free_extents {
            return false;
        }

        let pe_size = vg.pe_size;
        let pv_ids_copy: Vec<u32> = vg.pv_ids.clone();
        let mut remaining = extra_extents;
        let mut new_maps = Vec::new();

        let lv = match self.lvs.iter().find(|l| l.lv_id == lv_id) {
            Some(l) => l,
            None => return false,
        };
        let current_le = lv.extent_count;

        for &pid in &pv_ids_copy {
            if remaining == 0 {
                break;
            }
            if let Some(pv) = self.pvs.iter_mut().find(|p| p.pv_id == pid) {
                let take = remaining.min(pv.free_extents);
                if take > 0 {
                    let pe_start = pv.total_extents - pv.free_extents;
                    new_maps.push(ExtentMap {
                        le_start: current_le + (extra_extents - remaining),
                        le_count: take,
                        pv_id: pid,
                        pe_start,
                    });
                    pv.free_extents -= take;
                    remaining -= take;
                }
            }
        }

        if remaining > 0 {
            return false;
        }

        if let Some(lv) = self.lvs.iter_mut().find(|l| l.lv_id == lv_id) {
            lv.extent_count += extra_extents;
            lv.size_bytes += (extra_extents as u64) * pe_size;
            lv.mappings.extend(new_maps);
        }
        if let Some(vg) = self.vgs.iter_mut().find(|v| v.vg_id == vg_id) {
            vg.free_extents -= extra_extents;
        }

        self.log_op(format!("lvextend {} (+{} extents)", lv_id, extra_extents));
        true
    }

    /// Reduce a logical volume (from end). Data must be moved first.
    pub fn lv_reduce(&mut self, lv_id: u32, remove_extents: u32) -> bool {
        let lv = match self.lvs.iter_mut().find(|l| l.lv_id == lv_id) {
            Some(l) => l,
            None => return false,
        };
        if remove_extents >= lv.extent_count {
            serial_println!("  [lvm] Cannot reduce LV below 1 extent");
            return false;
        }
        let new_count = lv.extent_count - remove_extents;
        let pe_size = self
            .vgs
            .iter()
            .find(|v| v.vg_id == lv.vg_id)
            .map(|v| v.pe_size)
            .unwrap_or(DEFAULT_PE_SIZE);
        let vg_id = lv.vg_id;

        lv.extent_count = new_count;
        lv.size_bytes = (new_count as u64) * pe_size;

        // Trim mappings from the end
        let mut allocated = 0u32;
        lv.mappings.retain(|m| {
            if allocated >= new_count {
                return false;
            }
            allocated += m.le_count;
            true
        });

        if let Some(vg) = self.vgs.iter_mut().find(|v| v.vg_id == vg_id) {
            vg.free_extents += remove_extents;
        }

        self.log_op(format!("lvreduce {} (-{} extents)", lv_id, remove_extents));
        true
    }

    /// Create a snapshot of an existing LV.
    pub fn lv_snapshot(
        &mut self,
        origin_lv_id: u32,
        snap_name: &str,
        cow_extents: u32,
    ) -> Option<u32> {
        let origin = self.lvs.iter().find(|l| l.lv_id == origin_lv_id)?;
        let vg_id = origin.vg_id;

        let lv_id = self.lv_create(vg_id, snap_name, cow_extents)?;

        if let Some(snap) = self.lvs.iter_mut().find(|l| l.lv_id == lv_id) {
            snap.lv_type = LvType::Snapshot;
            snap.read_only = true;
            snap.snapshot = Some(SnapshotMeta {
                origin_lv_id,
                cow_used_extents: 0,
                cow_total_extents: cow_extents,
                created_at: crate::time::clock::unix_time(),
                usage_q16: 0,
            });
        }

        self.log_op(format!("lvsnapshot {} -> {}", origin_lv_id, snap_name));
        serial_println!(
            "  [lvm] Snapshot '{}' of LV {} created",
            snap_name,
            origin_lv_id
        );
        Some(lv_id)
    }

    /// Create a thin provisioning pool.
    pub fn create_thin_pool(
        &mut self,
        vg_id: u32,
        name: &str,
        data_extents: u32,
        meta_extents: u32,
    ) -> Option<u32> {
        let total_needed = data_extents + meta_extents;
        let lv_id = self.lv_create(vg_id, name, total_needed)?;

        if let Some(lv) = self.lvs.iter_mut().find(|l| l.lv_id == lv_id) {
            lv.lv_type = LvType::ThinPool;
            lv.thin_pool = Some(ThinPoolMeta {
                data_extents,
                data_used: 0,
                metadata_extents: meta_extents,
                metadata_used: 0,
                thin_ids: Vec::new(),
                overcommit_q16: 3 << 16, // 3x overcommit default
            });
        }

        self.log_op(format!(
            "thin_pool_create {} ({} data + {} meta)",
            name, data_extents, meta_extents
        ));
        serial_println!("  [lvm] Thin pool '{}' created", name);
        Some(lv_id)
    }

    /// Create a thin volume inside a thin pool.
    pub fn create_thin_volume(
        &mut self,
        pool_lv_id: u32,
        name: &str,
        virtual_extents: u32,
    ) -> Option<u32> {
        // Verify pool exists and has capacity (with overcommit)
        let pool = self.lvs.iter().find(|l| l.lv_id == pool_lv_id)?;
        if pool.lv_type != LvType::ThinPool {
            return None;
        }
        let pool_meta = pool.thin_pool.as_ref()?;
        let vg_id = pool.vg_id;

        let max_virtual =
            ((pool_meta.data_extents as i64) * (pool_meta.overcommit_q16 as i64)) >> 16;
        let current_virtual: u32 = pool_meta
            .thin_ids
            .iter()
            .filter_map(|&tid| self.lvs.iter().find(|l| l.lv_id == tid))
            .map(|l| l.extent_count)
            .sum();

        if (current_virtual + virtual_extents) as i64 > max_virtual {
            serial_println!("  [lvm] Thin pool overcommit limit reached");
            return None;
        }

        let lv_id = self.next_lv_id;
        self.next_lv_id = self.next_lv_id.saturating_add(1);

        let pe_size = self
            .vgs
            .iter()
            .find(|v| v.vg_id == vg_id)
            .map(|v| v.pe_size)
            .unwrap_or(DEFAULT_PE_SIZE);

        let lv = LogicalVolume {
            lv_id,
            name: String::from(name),
            vg_id,
            lv_type: LvType::ThinVolume,
            state: LvState::Active,
            extent_count: virtual_extents,
            size_bytes: (virtual_extents as u64) * pe_size,
            mappings: Vec::new(), // allocated on write
            snapshot: None,
            thin_pool: None,
            read_only: false,
            created_at: crate::time::clock::unix_time(),
        };
        self.lvs.push(lv);

        // Register in pool
        if let Some(pool) = self.lvs.iter_mut().find(|l| l.lv_id == pool_lv_id) {
            if let Some(ref mut meta) = pool.thin_pool {
                meta.thin_ids.push(lv_id);
            }
        }

        self.log_op(format!(
            "thin_create {} ({} virtual extents)",
            name, virtual_extents
        ));
        serial_println!(
            "  [lvm] Thin volume '{}' created ({} virtual extents)",
            name,
            virtual_extents
        );
        Some(lv_id)
    }

    /// Get LV by id.
    pub fn get_lv(&self, lv_id: u32) -> Option<&LogicalVolume> {
        self.lvs.iter().find(|l| l.lv_id == lv_id)
    }

    /// Get VG by id.
    pub fn get_vg(&self, vg_id: u32) -> Option<&VolumeGroup> {
        self.vgs.iter().find(|v| v.vg_id == vg_id)
    }

    /// Summary stats.
    pub fn summary(&self) -> (usize, usize, usize) {
        (self.pvs.len(), self.vgs.len(), self.lvs.len())
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static LVM_MANAGER: Mutex<Option<LvmManager>> = Mutex::new(None);

pub fn init() {
    let mut guard = LVM_MANAGER.lock();
    *guard = Some(LvmManager::new());
    serial_println!("  [storage] Logical volume manager initialized");
}

/// Access the LVM manager under lock.
pub fn with_lvm<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut LvmManager) -> R,
{
    let mut guard = LVM_MANAGER.lock();
    guard.as_mut().map(f)
}
