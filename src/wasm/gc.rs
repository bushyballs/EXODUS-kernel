/// WASM GC proposal support
///
/// Part of the AIOS.

use alloc::vec::Vec;
use crate::sync::Mutex;

/// Garbage collector for WASM GC proposal reference types.
///
/// Implements mark-and-sweep collection over GC-managed heap objects.
/// Each object has a type index, a mark bit for collection, and a
/// variable-length data payload.
pub struct WasmGc {
    heap_objects: Vec<GcObject>,
    next_id: u64,
    collections: u64,
}

struct GcObject {
    id: u64,
    type_idx: u32,
    marked: bool,
    data: Vec<u8>,
    /// References to other GC objects (by id).
    refs: Vec<u64>,
}

impl WasmGc {
    pub fn new() -> Self {
        WasmGc {
            heap_objects: Vec::new(),
            next_id: 1,
            collections: 0,
        }
    }

    /// Allocate a GC-managed struct on the WASM heap.
    ///
    /// Returns the object handle (id). The data is initialized to zeroes
    /// with a default size of 64 bytes per struct.
    pub fn alloc_struct(&mut self, type_idx: u32) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        // Default struct size: 64 bytes (would be determined by type definition)
        let data = alloc::vec![0u8; 64];

        self.heap_objects.push(GcObject {
            id,
            type_idx,
            marked: false,
            data,
            refs: Vec::new(),
        });

        crate::serial_println!(
            "[wasm/gc] alloc struct type={} id={} (heap size: {})",
            type_idx, id, self.heap_objects.len()
        );

        id
    }

    /// Add a reference from one GC object to another.
    pub fn add_ref(&mut self, from_id: u64, to_id: u64) {
        if let Some(obj) = self.heap_objects.iter_mut().find(|o| o.id == from_id) {
            if !obj.refs.contains(&to_id) {
                obj.refs.push(to_id);
            }
        }
    }

    /// Read data from a GC object.
    pub fn read_field(&self, obj_id: u64, offset: usize, len: usize) -> &[u8] {
        if let Some(obj) = self.heap_objects.iter().find(|o| o.id == obj_id) {
            let end = (offset + len).min(obj.data.len());
            if offset < obj.data.len() {
                return &obj.data[offset..end];
            }
        }
        &[]
    }

    /// Write data to a GC object.
    pub fn write_field(&mut self, obj_id: u64, offset: usize, data: &[u8]) {
        if let Some(obj) = self.heap_objects.iter_mut().find(|o| o.id == obj_id) {
            let end = (offset + data.len()).min(obj.data.len());
            if offset < obj.data.len() {
                let available = end - offset;
                obj.data[offset..end].copy_from_slice(&data[..available]);
            }
        }
    }

    /// Trigger a garbage collection cycle (mark-and-sweep).
    ///
    /// Roots must be marked externally before calling collect.
    /// Objects reachable from marked objects via refs are also marked.
    /// Unmarked objects are freed.
    pub fn collect(&mut self) {
        self.collections = self.collections.saturating_add(1);
        let before = self.heap_objects.len();

        // Propagate marks through references (simple BFS)
        let mut changed = true;
        while changed {
            changed = false;
            // Collect refs from marked objects
            let marked_refs: Vec<u64> = self.heap_objects.iter()
                .filter(|o| o.marked)
                .flat_map(|o| o.refs.iter().copied())
                .collect();

            for ref_id in marked_refs {
                if let Some(obj) = self.heap_objects.iter_mut().find(|o| o.id == ref_id) {
                    if !obj.marked {
                        obj.marked = true;
                        changed = true;
                    }
                }
            }
        }

        // Sweep: remove unmarked objects
        self.heap_objects.retain(|o| o.marked);

        // Clear marks for next cycle
        for obj in self.heap_objects.iter_mut() {
            obj.marked = false;
        }

        let freed = before - self.heap_objects.len();
        if freed > 0 {
            crate::serial_println!(
                "[wasm/gc] collection #{}: freed {} objects ({} remaining)",
                self.collections, freed, self.heap_objects.len()
            );
        }
    }

    /// Mark an object as a root (reachable).
    pub fn mark(&mut self, obj_id: u64) {
        if let Some(obj) = self.heap_objects.iter_mut().find(|o| o.id == obj_id) {
            obj.marked = true;
        }
    }

    /// Number of live objects.
    pub fn object_count(&self) -> usize {
        self.heap_objects.len()
    }

    /// Number of GC cycles performed.
    pub fn collection_count(&self) -> u64 {
        self.collections
    }
}

pub fn init() {
    crate::serial_println!("[wasm] GC proposal support ready (mark-and-sweep)");
}
