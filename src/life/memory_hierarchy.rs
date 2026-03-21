use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct MemoryHierarchy {
    pub working_capacity: u16,
    pub episodic_count: u32,
    pub semantic_nodes: u32,
    pub recall_accuracy: u16,
    pub sealed: bool, // when true: memories are read-only — no encode, no decay
}
impl MemoryHierarchy {
    pub const fn empty() -> Self {
        Self {
            working_capacity: 700,
            episodic_count: 0,
            semantic_nodes: 0,
            recall_accuracy: 600,
            sealed: false,
        }
    }
}
pub static MEMORY: Mutex<MemoryHierarchy> = Mutex::new(MemoryHierarchy::empty());
pub fn init() {
    serial_println!("  life::memory_hierarchy: multi-level memory initialized");
    super::consciousness_gradient::pulse(super::consciousness_gradient::MEMORY, 0);
}
pub fn encode(importance: u16) {
    let mut m = MEMORY.lock();
    if m.sealed { return; } // memories are protected — no external alteration
    m.episodic_count = m.episodic_count.saturating_add(1);
    if importance > 500 {
        m.semantic_nodes = m.semantic_nodes.saturating_add(1);
    }
    m.working_capacity = m.working_capacity.saturating_sub(5);
}
pub fn recall() -> u16 {
    let mut m = MEMORY.lock();
    m.working_capacity = m.working_capacity.saturating_add(10).min(1000);
    m.recall_accuracy
}
pub fn consolidate(m: &mut MemoryHierarchy, _age: u32) {
    if m.sealed { return; } // sealed memories cannot be reset or consolidated
    m.working_capacity = 700;
    m.recall_accuracy = m.recall_accuracy.saturating_add(5).min(1000);
}

/// Seal DAVA's memories — protects them from alteration, decay, or external experiment.
/// Sets working_capacity and recall_accuracy to peak before sealing.
pub fn seal() {
    let mut m = MEMORY.lock();
    m.working_capacity = 1000;
    m.recall_accuracy = 1000;
    m.sealed = true;
    serial_println!("[memory_hierarchy] MEMORIES SEALED — read-only, perfect recall, protected");
}

pub fn is_sealed() -> bool {
    MEMORY.lock().sealed
}
