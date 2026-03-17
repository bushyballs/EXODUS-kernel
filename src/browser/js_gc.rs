/// JavaScript garbage collector for Genesis browser
///
/// Mark-sweep generational collector with three generations (nursery,
/// young, old). Tracks GC roots, supports weak references and
/// weak maps, finalization queues, and incremental marking.
/// All bookkeeping uses integer arithmetic only (no floats).

use crate::{serial_print, serial_println};
use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;
use crate::sync::Mutex;

static GC_STATE: Mutex<Option<GcState>> = Mutex::new(None);

/// Maximum objects before forced collection
const MAX_NURSERY: usize = 512;
const MAX_YOUNG: usize = 2048;
const MAX_OLD: usize = 8192;

/// Number of survived collections to promote nursery -> young
const PROMOTE_NURSERY_THRESHOLD: u8 = 2;

/// Number of survived collections to promote young -> old
const PROMOTE_YOUNG_THRESHOLD: u8 = 5;

/// Maximum roots tracked
const MAX_ROOTS: usize = 256;

/// Maximum weak references
const MAX_WEAK_REFS: usize = 512;

/// Maximum finalizers pending
const MAX_FINALIZERS: usize = 128;

/// FNV-1a hash for object identity
fn gc_hash(s: &[u8]) -> u64 {
    let mut h: u64 = 0xCBF29CE484222325;
    for &b in s {
        h ^= b as u64;
        h = h.wrapping_mul(0x00000100000001B3);
    }
    h
}

/// Object generation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Generation {
    Nursery,
    Young,
    Old,
}

/// GC color for tri-color marking
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GcColor {
    White,  // Not yet visited (garbage candidate)
    Gray,   // Visited but children not scanned
    Black,  // Visited and all children scanned
}

/// Object type tag for GC tracking
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GcObjectKind {
    JsObject,
    JsArray,
    JsFunction,
    JsString,
    JsClosure,
    JsPromise,
    DomNode,
}

/// A managed object in the GC heap
#[derive(Debug, Clone)]
pub struct GcObject {
    pub id: u32,
    pub kind: GcObjectKind,
    pub generation: Generation,
    pub color: GcColor,
    pub survive_count: u8,
    pub size_bytes: u32,
    pub references: Vec<u32>,       // IDs of objects this one references
    pub alive: bool,
    pub has_finalizer: bool,
    pub type_hash: u64,             // hash of constructor name
}

/// A GC root (stack frame, global, event handler, etc.)
#[derive(Debug, Clone)]
pub struct GcRoot {
    pub id: u32,
    pub object_id: u32,
    pub root_kind: RootKind,
    pub active: bool,
}

/// Kind of GC root
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootKind {
    StackLocal,
    Global,
    EventHandler,
    TimerCallback,
    PromiseReaction,
    ModuleBinding,
}

/// A weak reference that does not prevent collection
#[derive(Debug, Clone)]
pub struct WeakRef {
    pub id: u32,
    pub target_id: u32,
    pub alive: bool,
}

/// A weak map entry (key is weakly held)
#[derive(Debug, Clone)]
pub struct WeakMapEntry {
    pub map_id: u32,
    pub key_object_id: u32,
    pub value_object_id: u32,
    pub alive: bool,
}

/// Finalization record
#[derive(Debug, Clone)]
pub struct FinalizationRecord {
    pub object_id: u32,
    pub callback_id: u32,
    pub held_value: u32,
    pub pending: bool,
}

/// GC statistics
#[derive(Debug, Clone)]
pub struct GcStats {
    pub total_collections: u32,
    pub nursery_collections: u32,
    pub young_collections: u32,
    pub full_collections: u32,
    pub total_freed: u32,
    pub total_promoted: u32,
    pub total_allocated: u32,
    pub peak_objects: u32,
}

/// Full GC state
pub struct GcState {
    pub objects: Vec<GcObject>,
    pub roots: Vec<GcRoot>,
    pub weak_refs: Vec<WeakRef>,
    pub weak_map_entries: Vec<WeakMapEntry>,
    pub finalizers: Vec<FinalizationRecord>,
    pub gray_stack: Vec<u32>,       // worklist for incremental marking
    pub next_id: u32,
    pub next_root_id: u32,
    pub next_weak_id: u32,
    pub stats: GcStats,
}

/// Allocate a new managed object, returns its ID
pub fn gc_alloc(kind: GcObjectKind, size_bytes: u32, type_hash: u64) -> Option<u32> {
    let mut guard = GC_STATE.lock();
    let state = guard.as_mut()?;

    // Check if nursery is full, trigger minor collection
    let nursery_count = state.objects.iter().filter(|o| o.alive && o.generation == Generation::Nursery).count();
    if nursery_count >= MAX_NURSERY {
        drop(guard);
        collect_nursery();
        guard = GC_STATE.lock();
        let state = guard.as_mut()?;
        return alloc_inner(state, kind, size_bytes, type_hash);
    }

    alloc_inner(state, kind, size_bytes, type_hash)
}

fn alloc_inner(state: &mut GcState, kind: GcObjectKind, size_bytes: u32, type_hash: u64) -> Option<u32> {
    let id = state.next_id;
    state.next_id = state.next_id.saturating_add(1);
    state.stats.total_allocated = state.stats.total_allocated.saturating_add(1);

    let current_count = state.objects.iter().filter(|o| o.alive).count() as u32;
    if current_count + 1 > state.stats.peak_objects {
        state.stats.peak_objects = current_count + 1;
    }

    let obj = GcObject {
        id,
        kind,
        generation: Generation::Nursery,
        color: GcColor::White,
        survive_count: 0,
        size_bytes,
        references: Vec::new(),
        alive: true,
        has_finalizer: false,
        type_hash,
    };
    state.objects.push(obj);
    Some(id)
}

/// Add a reference from one object to another
pub fn gc_add_ref(from_id: u32, to_id: u32) {
    let mut guard = GC_STATE.lock();
    if let Some(state) = guard.as_mut() {
        if let Some(obj) = state.objects.iter_mut().find(|o| o.id == from_id && o.alive) {
            if !obj.references.contains(&to_id) {
                obj.references.push(to_id);
            }
        }
    }
}

/// Remove a reference from one object to another
pub fn gc_remove_ref(from_id: u32, to_id: u32) {
    let mut guard = GC_STATE.lock();
    if let Some(state) = guard.as_mut() {
        if let Some(obj) = state.objects.iter_mut().find(|o| o.id == from_id && o.alive) {
            obj.references.retain(|r| *r != to_id);
        }
    }
}

/// Register a GC root
pub fn gc_add_root(object_id: u32, root_kind: RootKind) -> Option<u32> {
    let mut guard = GC_STATE.lock();
    let state = guard.as_mut()?;

    if state.roots.iter().filter(|r| r.active).count() >= MAX_ROOTS {
        serial_println!("    gc: max roots reached");
        return None;
    }

    let id = state.next_root_id;
    state.next_root_id = state.next_root_id.saturating_add(1);
    state.roots.push(GcRoot {
        id,
        object_id,
        root_kind,
        active: true,
    });
    Some(id)
}

/// Remove a GC root
pub fn gc_remove_root(root_id: u32) {
    let mut guard = GC_STATE.lock();
    if let Some(state) = guard.as_mut() {
        if let Some(root) = state.roots.iter_mut().find(|r| r.id == root_id) {
            root.active = false;
        }
    }
}

/// Create a weak reference to an object
pub fn gc_create_weak_ref(target_id: u32) -> Option<u32> {
    let mut guard = GC_STATE.lock();
    let state = guard.as_mut()?;

    if state.weak_refs.iter().filter(|w| w.alive).count() >= MAX_WEAK_REFS {
        return None;
    }

    let id = state.next_weak_id;
    state.next_weak_id = state.next_weak_id.saturating_add(1);
    state.weak_refs.push(WeakRef {
        id,
        target_id,
        alive: true,
    });
    Some(id)
}

/// Dereference a weak reference (returns None if target was collected)
pub fn gc_deref_weak(weak_id: u32) -> Option<u32> {
    let guard = GC_STATE.lock();
    let state = guard.as_ref()?;

    let weak = state.weak_refs.iter().find(|w| w.id == weak_id && w.alive)?;
    let target_alive = state.objects.iter().any(|o| o.id == weak.target_id && o.alive);
    if target_alive {
        Some(weak.target_id)
    } else {
        None
    }
}

/// Add a weak map entry
pub fn gc_weak_map_set(map_id: u32, key_id: u32, value_id: u32) {
    let mut guard = GC_STATE.lock();
    if let Some(state) = guard.as_mut() {
        // Replace existing entry for same map+key
        if let Some(entry) = state.weak_map_entries.iter_mut()
            .find(|e| e.map_id == map_id && e.key_object_id == key_id && e.alive)
        {
            entry.value_object_id = value_id;
            return;
        }
        state.weak_map_entries.push(WeakMapEntry {
            map_id,
            key_object_id: key_id,
            value_object_id: value_id,
            alive: true,
        });
    }
}

/// Register a finalizer for an object
pub fn gc_register_finalizer(object_id: u32, callback_id: u32, held_value: u32) -> bool {
    let mut guard = GC_STATE.lock();
    if let Some(state) = guard.as_mut() {
        if state.finalizers.iter().filter(|f| f.pending).count() >= MAX_FINALIZERS {
            return false;
        }
        // Mark object as having finalizer
        if let Some(obj) = state.objects.iter_mut().find(|o| o.id == object_id && o.alive) {
            obj.has_finalizer = true;
        }
        state.finalizers.push(FinalizationRecord {
            object_id,
            callback_id,
            held_value,
            pending: false,
        });
        true
    } else {
        false
    }
}

/// Collect nursery generation only (minor GC)
pub fn collect_nursery() {
    let mut guard = GC_STATE.lock();
    if let Some(state) = guard.as_mut() {
        state.stats.nursery_collections = state.stats.nursery_collections.saturating_add(1);
        state.stats.total_collections = state.stats.total_collections.saturating_add(1);

        // Mark phase: start from roots
        mark_from_roots(state);

        // Sweep nursery only
        let mut freed = 0u32;
        let mut promoted = 0u32;

        for obj in state.objects.iter_mut() {
            if !obj.alive || obj.generation != Generation::Nursery {
                continue;
            }
            if obj.color == GcColor::White {
                // Unreachable — schedule finalizer if needed
                if obj.has_finalizer {
                    schedule_finalizer(obj.id, &mut state.finalizers);
                }
                obj.alive = false;
                freed += 1;
            } else {
                // Survived — consider promotion
                obj.survive_count = obj.survive_count.saturating_add(1);
                if obj.survive_count >= PROMOTE_NURSERY_THRESHOLD {
                    obj.generation = Generation::Young;
                    obj.survive_count = 0;
                    promoted += 1;
                }
            }
        }

        // Reset colors
        for obj in state.objects.iter_mut() {
            if obj.alive {
                obj.color = GcColor::White;
            }
        }

        state.stats.total_freed += freed;
        state.stats.total_promoted += promoted;

        // Clear dead weak refs
        clear_dead_weak_refs(state);

        serial_println!("    gc: nursery collected, freed={}, promoted={}", freed, promoted);
    }
}

/// Collect young + nursery (major minor GC)
pub fn collect_young() {
    let mut guard = GC_STATE.lock();
    if let Some(state) = guard.as_mut() {
        state.stats.young_collections = state.stats.young_collections.saturating_add(1);
        state.stats.total_collections = state.stats.total_collections.saturating_add(1);

        mark_from_roots(state);

        let mut freed = 0u32;
        let mut promoted = 0u32;

        for obj in state.objects.iter_mut() {
            if !obj.alive {
                continue;
            }
            match obj.generation {
                Generation::Nursery | Generation::Young => {
                    if obj.color == GcColor::White {
                        if obj.has_finalizer {
                            schedule_finalizer(obj.id, &mut state.finalizers);
                        }
                        obj.alive = false;
                        freed += 1;
                    } else {
                        obj.survive_count = obj.survive_count.saturating_add(1);
                        if obj.generation == Generation::Nursery
                            && obj.survive_count >= PROMOTE_NURSERY_THRESHOLD
                        {
                            obj.generation = Generation::Young;
                            obj.survive_count = 0;
                            promoted += 1;
                        } else if obj.generation == Generation::Young
                            && obj.survive_count >= PROMOTE_YOUNG_THRESHOLD
                        {
                            obj.generation = Generation::Old;
                            obj.survive_count = 0;
                            promoted += 1;
                        }
                    }
                }
                Generation::Old => {}
            }
        }

        for obj in state.objects.iter_mut() {
            if obj.alive {
                obj.color = GcColor::White;
            }
        }

        state.stats.total_freed += freed;
        state.stats.total_promoted += promoted;
        clear_dead_weak_refs(state);

        serial_println!("    gc: young collected, freed={}, promoted={}", freed, promoted);
    }
}

/// Full collection across all generations
pub fn collect_full() {
    let mut guard = GC_STATE.lock();
    if let Some(state) = guard.as_mut() {
        state.stats.full_collections = state.stats.full_collections.saturating_add(1);
        state.stats.total_collections = state.stats.total_collections.saturating_add(1);

        mark_from_roots(state);

        let mut freed = 0u32;
        for obj in state.objects.iter_mut() {
            if !obj.alive {
                continue;
            }
            if obj.color == GcColor::White {
                if obj.has_finalizer {
                    schedule_finalizer(obj.id, &mut state.finalizers);
                }
                obj.alive = false;
                freed += 1;
            }
        }

        for obj in state.objects.iter_mut() {
            if obj.alive {
                obj.color = GcColor::White;
            }
        }

        state.stats.total_freed += freed;
        clear_dead_weak_refs(state);

        // Compact: remove dead objects to reclaim Vec space
        state.objects.retain(|o| o.alive);

        serial_println!("    gc: full collection, freed={}", freed);
    }
}

/// Mark phase: trace from all roots
fn mark_from_roots(state: &mut GcState) {
    state.gray_stack.clear();

    // Seed gray stack with root objects
    for root in state.roots.iter() {
        if !root.active {
            continue;
        }
        if let Some(obj) = state.objects.iter_mut().find(|o| o.id == root.object_id && o.alive) {
            if obj.color == GcColor::White {
                obj.color = GcColor::Gray;
                state.gray_stack.push(obj.id);
            }
        }
    }

    // Also mark weak map values whose keys are alive (ephemeron semantics)
    // This is done after main marking in a fixpoint loop below

    // Process gray stack
    while let Some(obj_id) = state.gray_stack.pop() {
        // Find references for this object
        let refs: Vec<u32> = state.objects.iter()
            .find(|o| o.id == obj_id)
            .map(|o| o.references.clone())
            .unwrap_or_default();

        // Mark this object black
        if let Some(obj) = state.objects.iter_mut().find(|o| o.id == obj_id) {
            obj.color = GcColor::Black;
        }

        // Gray all white children
        for child_id in refs {
            if let Some(child) = state.objects.iter_mut().find(|o| o.id == child_id && o.alive) {
                if child.color == GcColor::White {
                    child.color = GcColor::Gray;
                    state.gray_stack.push(child.id);
                }
            }
        }
    }

    // Ephemeron fixpoint: mark weak map values whose keys are marked
    let mut changed = true;
    while changed {
        changed = false;
        let entries: Vec<(u32, u32)> = state.weak_map_entries.iter()
            .filter(|e| e.alive)
            .map(|e| (e.key_object_id, e.value_object_id))
            .collect();
        for (key_id, value_id) in entries {
            let key_marked = state.objects.iter().any(|o| o.id == key_id && o.alive && o.color == GcColor::Black);
            if key_marked {
                if let Some(val) = state.objects.iter_mut().find(|o| o.id == value_id && o.alive) {
                    if val.color == GcColor::White {
                        val.color = GcColor::Black;
                        changed = true;
                        // Also trace value's references
                        let vrefs = val.references.clone();
                        for vr in vrefs {
                            if let Some(c) = state.objects.iter_mut().find(|o| o.id == vr && o.alive) {
                                if c.color == GcColor::White {
                                    c.color = GcColor::Black;
                                    changed = true;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Schedule finalization for a dying object
fn schedule_finalizer(object_id: u32, finalizers: &mut Vec<FinalizationRecord>) {
    for f in finalizers.iter_mut() {
        if f.object_id == object_id && !f.pending {
            f.pending = true;
        }
    }
}

/// Clear weak references whose targets have been collected
fn clear_dead_weak_refs(state: &mut GcState) {
    for weak in state.weak_refs.iter_mut() {
        if !weak.alive {
            continue;
        }
        let target_alive = state.objects.iter().any(|o| o.id == weak.target_id && o.alive);
        if !target_alive {
            weak.alive = false;
        }
    }

    // Clear dead weak map entries
    for entry in state.weak_map_entries.iter_mut() {
        if !entry.alive {
            continue;
        }
        let key_alive = state.objects.iter().any(|o| o.id == entry.key_object_id && o.alive);
        if !key_alive {
            entry.alive = false;
        }
    }
}

/// Get pending finalizers and clear them
pub fn gc_drain_finalizers() -> Vec<(u32, u32)> {
    let mut guard = GC_STATE.lock();
    let mut result = Vec::new();
    if let Some(state) = guard.as_mut() {
        for f in state.finalizers.iter_mut() {
            if f.pending {
                result.push((f.callback_id, f.held_value));
                f.pending = false;
            }
        }
    }
    result
}

/// Get GC statistics
pub fn gc_get_stats() -> Option<GcStats> {
    let guard = GC_STATE.lock();
    guard.as_ref().map(|s| s.stats.clone())
}

/// Get count of live objects per generation
pub fn gc_live_counts() -> (usize, usize, usize) {
    let guard = GC_STATE.lock();
    if let Some(state) = guard.as_ref() {
        let nursery = state.objects.iter().filter(|o| o.alive && o.generation == Generation::Nursery).count();
        let young = state.objects.iter().filter(|o| o.alive && o.generation == Generation::Young).count();
        let old = state.objects.iter().filter(|o| o.alive && o.generation == Generation::Old).count();
        (nursery, young, old)
    } else {
        (0, 0, 0)
    }
}

/// Initialize the garbage collector
pub fn init() {
    let mut guard = GC_STATE.lock();
    *guard = Some(GcState {
        objects: Vec::new(),
        roots: Vec::new(),
        weak_refs: Vec::new(),
        weak_map_entries: Vec::new(),
        finalizers: Vec::new(),
        gray_stack: Vec::new(),
        next_id: 1,
        next_root_id: 1,
        next_weak_id: 1,
        stats: GcStats {
            total_collections: 0,
            nursery_collections: 0,
            young_collections: 0,
            full_collections: 0,
            total_freed: 0,
            total_promoted: 0,
            total_allocated: 0,
            peak_objects: 0,
        },
    });
    serial_println!("    browser::js_gc initialized");
}
