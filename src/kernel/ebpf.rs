/// eBPF — Extended Berkeley Packet Filter VM for Genesis
///
/// A lightweight in-kernel virtual machine that safely executes user-provided
/// bytecode programs. Used for: packet filtering, tracing, security policies,
/// performance monitoring, and programmable kernel extensions.
///
/// The eBPF VM has 11 registers (R0-R10), 512-byte stack, and a RISC-like ISA.
/// Programs are verified before execution (bounds checking, no loops guarantee termination).
///
/// Inspired by: Linux eBPF (kernel/bpf/). All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Instruction encoding
// ---------------------------------------------------------------------------

/// eBPF instruction (8 bytes each)
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct BpfInsn {
    /// Opcode
    pub op: u8,
    /// Source/destination register
    pub regs: u8, // dst:4 | src:4
    /// Offset
    pub off: i16,
    /// Immediate value
    pub imm: i32,
}

impl BpfInsn {
    pub fn dst(&self) -> usize {
        (self.regs & 0x0F) as usize
    }
    pub fn src(&self) -> usize {
        ((self.regs >> 4) & 0x0F) as usize
    }

    /// Construct an instruction
    pub fn new(op: u8, dst: u8, src: u8, off: i16, imm: i32) -> Self {
        BpfInsn {
            op,
            regs: (dst & 0x0F) | ((src & 0x0F) << 4),
            off,
            imm,
        }
    }
}

// ---------------------------------------------------------------------------
// Opcode constants — full eBPF instruction set
// ---------------------------------------------------------------------------

pub mod op {
    // ---- ALU 64-bit (class 0x07/0x0F) ----
    pub const ADD_IMM: u8 = 0x07;
    pub const ADD_REG: u8 = 0x0F;
    pub const SUB_IMM: u8 = 0x17;
    pub const SUB_REG: u8 = 0x1F;
    pub const MUL_IMM: u8 = 0x27;
    pub const MUL_REG: u8 = 0x2F;
    pub const DIV_IMM: u8 = 0x37;
    pub const DIV_REG: u8 = 0x3F;
    pub const OR_IMM: u8 = 0x47;
    pub const OR_REG: u8 = 0x4F;
    pub const AND_IMM: u8 = 0x57;
    pub const AND_REG: u8 = 0x5F;
    pub const LSH_IMM: u8 = 0x67;
    pub const LSH_REG: u8 = 0x6F;
    pub const RSH_IMM: u8 = 0x77;
    pub const RSH_REG: u8 = 0x7F;
    pub const NEG: u8 = 0x87;
    pub const MOD_IMM: u8 = 0x97;
    pub const MOD_REG: u8 = 0x9F;
    pub const XOR_IMM: u8 = 0xA7;
    pub const XOR_REG: u8 = 0xAF;
    pub const MOV_IMM: u8 = 0xB7;
    pub const MOV_REG: u8 = 0xBF;
    pub const ARSH_IMM: u8 = 0xC7;
    pub const ARSH_REG: u8 = 0xCF;

    // ---- ALU 32-bit (class 0x04/0x0C) ----
    pub const ADD32_IMM: u8 = 0x04;
    pub const ADD32_REG: u8 = 0x0C;
    pub const SUB32_IMM: u8 = 0x14;
    pub const SUB32_REG: u8 = 0x1C;
    pub const MUL32_IMM: u8 = 0x24;
    pub const MUL32_REG: u8 = 0x2C;
    pub const DIV32_IMM: u8 = 0x34;
    pub const DIV32_REG: u8 = 0x3C;
    pub const OR32_IMM: u8 = 0x44;
    pub const OR32_REG: u8 = 0x4C;
    pub const AND32_IMM: u8 = 0x54;
    pub const AND32_REG: u8 = 0x5C;
    pub const LSH32_IMM: u8 = 0x64;
    pub const LSH32_REG: u8 = 0x6C;
    pub const RSH32_IMM: u8 = 0x74;
    pub const RSH32_REG: u8 = 0x7C;
    pub const NEG32: u8 = 0x84;
    pub const MOD32_IMM: u8 = 0x94;
    pub const MOD32_REG: u8 = 0x9C;
    pub const XOR32_IMM: u8 = 0xA4;
    pub const XOR32_REG: u8 = 0xAC;
    pub const MOV32_IMM: u8 = 0xB4;
    pub const MOV32_REG: u8 = 0xBC;
    pub const ARSH32_IMM: u8 = 0xC4;
    pub const ARSH32_REG: u8 = 0xCC;

    // ---- Memory ----
    pub const LD_DW: u8 = 0x18; // 64-bit immediate load (2 insns)
    pub const LDX_B: u8 = 0x71; // load byte
    pub const LDX_H: u8 = 0x69; // load half
    pub const LDX_W: u8 = 0x61; // load word
    pub const LDX_DW: u8 = 0x79; // load double word
    pub const STX_B: u8 = 0x73; // store byte
    pub const STX_H: u8 = 0x6B; // store half
    pub const STX_W: u8 = 0x63; // store word
    pub const STX_DW: u8 = 0x7B; // store double word
    pub const ST_B: u8 = 0x72; // store byte imm
    pub const ST_H: u8 = 0x6A; // store half imm
    pub const ST_W: u8 = 0x62; // store word imm
    pub const ST_DW: u8 = 0x7A; // store dword imm

    // ---- Atomic memory ops ----
    pub const ATOMIC_W: u8 = 0xDB; // atomic 32-bit
    pub const ATOMIC_DW: u8 = 0xDF; // atomic 64-bit

    // ---- Branch / JMP ----
    pub const JA: u8 = 0x05; // unconditional jump
    pub const JEQ_IMM: u8 = 0x15; // ==
    pub const JEQ_REG: u8 = 0x1D;
    pub const JGT_IMM: u8 = 0x25; // > (unsigned)
    pub const JGT_REG: u8 = 0x2D;
    pub const JGE_IMM: u8 = 0x35; // >= (unsigned)
    pub const JGE_REG: u8 = 0x3D;
    pub const JSET_IMM: u8 = 0x45; // & != 0
    pub const JSET_REG: u8 = 0x4D;
    pub const JNE_IMM: u8 = 0x55; // !=
    pub const JNE_REG: u8 = 0x5D;
    pub const JSGT_IMM: u8 = 0x65; // > (signed)
    pub const JSGT_REG: u8 = 0x6D;
    pub const JSGE_IMM: u8 = 0x75; // >= (signed)
    pub const JSGE_REG: u8 = 0x7D;
    pub const JLT_IMM: u8 = 0xA5; // < (unsigned)
    pub const JLT_REG: u8 = 0xAD;
    pub const JLE_IMM: u8 = 0xB5; // <= (unsigned)
    pub const JLE_REG: u8 = 0xBD;
    pub const JSLT_IMM: u8 = 0xC5; // < (signed)
    pub const JSLT_REG: u8 = 0xCD;
    pub const JSLE_IMM: u8 = 0xD5; // <= (signed)
    pub const JSLE_REG: u8 = 0xDD;
    pub const CALL: u8 = 0x85; // call helper function
    pub const EXIT: u8 = 0x95; // return R0

    // ---- Byte swap (endian) ----
    pub const LE: u8 = 0xD4; // host to little-endian
    pub const BE: u8 = 0xDC; // host to big-endian
}

// ---------------------------------------------------------------------------
// Program types
// ---------------------------------------------------------------------------

/// eBPF program types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BpfProgType {
    SocketFilter,
    Kprobe,
    Tracepoint,
    XdpAction,
    PerfEvent,
    CgroupSkb,
    Sched,
    SecurityPolicy,
}

/// XDP action codes returned by XDP programs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XdpAction {
    /// Drop the packet
    Drop = 1,
    /// Pass the packet up the stack
    Pass = 2,
    /// Send the packet back out the same interface
    Tx = 3,
    /// Redirect the packet to another interface or CPU
    Redirect = 4,
    /// Abort (error)
    Aborted = 0,
}

impl XdpAction {
    pub fn from_u64(v: u64) -> Self {
        match v {
            1 => XdpAction::Drop,
            2 => XdpAction::Pass,
            3 => XdpAction::Tx,
            4 => XdpAction::Redirect,
            _ => XdpAction::Aborted,
        }
    }
}

// ---------------------------------------------------------------------------
// BPF maps
// ---------------------------------------------------------------------------

/// BPF map types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BpfMapType {
    HashMap,
    Array,
    RingBuffer,
    /// Per-CPU array — each CPU gets its own copy of the array
    PerCpuArray,
    /// Per-CPU hash map — each CPU gets its own hash map
    PerCpuHash,
    /// LRU hash map — evicts least-recently-used entries when full
    LruHash,
    /// Stack trace map — stores kernel stack traces indexed by stack_id
    StackTrace,
    /// Perf event array — per-CPU perf event output buffers
    PerfEventArray,
}

/// A BPF map — shared key/value storage between BPF programs and userspace
pub struct BpfMap {
    pub id: u32,
    pub name: String,
    pub map_type: BpfMapType,
    pub key_size: u32,
    pub value_size: u32,
    pub max_entries: u32,
    /// Underlying storage — HashMap / Array use this flat store.
    /// Keys and values are stored as byte blobs.
    entries: BTreeMap<Vec<u8>, Vec<u8>>,
    /// Array backing (index -> value) for Array maps
    array: Vec<Vec<u8>>,
    /// RingBuffer backing
    ring: Vec<Vec<u8>>,
    ring_head: usize,
    ring_capacity: usize,
    /// Per-CPU storage: per_cpu[cpu_idx] = BTreeMap or Vec
    per_cpu_entries: Vec<BTreeMap<Vec<u8>, Vec<u8>>>,
    per_cpu_arrays: Vec<Vec<Vec<u8>>>,
    /// LRU access order tracking: key -> last access timestamp
    lru_order: Vec<Vec<u8>>,
    /// Stack trace storage: stack_id -> Vec<u64> (instruction pointers)
    stack_traces: BTreeMap<u32, Vec<u64>>,
    next_stack_id: u32,
    /// Perf event per-CPU ring buffers
    perf_rings: Vec<Vec<Vec<u8>>>,
}

impl BpfMap {
    fn new(
        id: u32,
        name: &str,
        map_type: BpfMapType,
        key_size: u32,
        value_size: u32,
        max_entries: u32,
    ) -> Self {
        let ncpus = crate::smp::num_cpus().max(1) as usize;
        let mut m = BpfMap {
            id,
            name: String::from(name),
            map_type,
            key_size,
            value_size,
            max_entries,
            entries: BTreeMap::new(),
            array: Vec::new(),
            ring: Vec::new(),
            ring_head: 0,
            ring_capacity: max_entries as usize,
            per_cpu_entries: Vec::new(),
            per_cpu_arrays: Vec::new(),
            lru_order: Vec::new(),
            stack_traces: BTreeMap::new(),
            next_stack_id: 1,
            perf_rings: Vec::new(),
        };
        match map_type {
            BpfMapType::Array => {
                // Pre-allocate array slots filled with zeroes
                let zero_val = vec![0u8; value_size as usize];
                for _ in 0..max_entries {
                    m.array.push(zero_val.clone());
                }
            }
            BpfMapType::PerCpuArray => {
                let zero_val = vec![0u8; value_size as usize];
                for _ in 0..ncpus {
                    let mut cpu_array = Vec::new();
                    for _ in 0..max_entries {
                        cpu_array.push(zero_val.clone());
                    }
                    m.per_cpu_arrays.push(cpu_array);
                }
            }
            BpfMapType::PerCpuHash => {
                for _ in 0..ncpus {
                    m.per_cpu_entries.push(BTreeMap::new());
                }
            }
            BpfMapType::PerfEventArray => {
                for _ in 0..ncpus {
                    m.perf_rings.push(Vec::new());
                }
            }
            _ => {}
        }
        m
    }

    /// Lookup a value by key. Returns None if not found.
    pub fn lookup(&self, key: &[u8]) -> Option<&[u8]> {
        match self.map_type {
            BpfMapType::HashMap | BpfMapType::LruHash => {
                self.entries.get(key).map(|v| v.as_slice())
            }
            BpfMapType::Array => {
                if key.len() < 4 {
                    return None;
                }
                let idx = u32::from_ne_bytes([key[0], key[1], key[2], key[3]]) as usize;
                self.array.get(idx).map(|v| v.as_slice())
            }
            BpfMapType::PerCpuArray => {
                if key.len() < 4 {
                    return None;
                }
                let idx = u32::from_ne_bytes([key[0], key[1], key[2], key[3]]) as usize;
                let cpu = crate::smp::current_cpu() as usize;
                self.per_cpu_arrays
                    .get(cpu)
                    .and_then(|arr| arr.get(idx))
                    .map(|v| v.as_slice())
            }
            BpfMapType::PerCpuHash => {
                let cpu = crate::smp::current_cpu() as usize;
                self.per_cpu_entries
                    .get(cpu)
                    .and_then(|m| m.get(key))
                    .map(|v| v.as_slice())
            }
            BpfMapType::RingBuffer | BpfMapType::PerfEventArray => None,
            BpfMapType::StackTrace => {
                // Key is a stack_id (u32), return the raw instruction pointers as bytes
                if key.len() < 4 {
                    return None;
                }
                let stack_id = u32::from_ne_bytes([key[0], key[1], key[2], key[3]]);
                // Stack traces are returned via a different API; return None here
                let _ = stack_id;
                None
            }
        }
    }

    /// Update (insert or overwrite) a key/value pair.
    pub fn update(&mut self, key: &[u8], value: &[u8]) -> Result<(), BpfError> {
        if key.len() != self.key_size as usize {
            return Err(BpfError::InvalidMapKey);
        }
        if value.len() != self.value_size as usize {
            return Err(BpfError::InvalidMapValue);
        }
        match self.map_type {
            BpfMapType::HashMap => {
                if !self.entries.contains_key(key)
                    && self.entries.len() >= self.max_entries as usize
                {
                    return Err(BpfError::MapFull);
                }
                self.entries.insert(key.to_vec(), value.to_vec());
                Ok(())
            }
            BpfMapType::LruHash => {
                if !self.entries.contains_key(key)
                    && self.entries.len() >= self.max_entries as usize
                {
                    // Evict the least-recently-used entry
                    if let Some(lru_key) = self.lru_order.first().cloned() {
                        self.entries.remove(&lru_key);
                        self.lru_order.retain(|k| *k != lru_key);
                    }
                }
                // Update LRU order: remove key if present, then push to end (most recent)
                self.lru_order.retain(|k| k != key);
                self.lru_order.push(key.to_vec());
                self.entries.insert(key.to_vec(), value.to_vec());
                Ok(())
            }
            BpfMapType::Array => {
                if key.len() < 4 {
                    return Err(BpfError::InvalidMapKey);
                }
                let idx = u32::from_ne_bytes([key[0], key[1], key[2], key[3]]) as usize;
                if idx >= self.max_entries as usize {
                    return Err(BpfError::OutOfBounds);
                }
                self.array[idx] = value.to_vec();
                Ok(())
            }
            BpfMapType::PerCpuArray => {
                if key.len() < 4 {
                    return Err(BpfError::InvalidMapKey);
                }
                let idx = u32::from_ne_bytes([key[0], key[1], key[2], key[3]]) as usize;
                if idx >= self.max_entries as usize {
                    return Err(BpfError::OutOfBounds);
                }
                let cpu = crate::smp::current_cpu() as usize;
                if cpu < self.per_cpu_arrays.len() {
                    self.per_cpu_arrays[cpu][idx] = value.to_vec();
                    Ok(())
                } else {
                    Err(BpfError::OutOfBounds)
                }
            }
            BpfMapType::PerCpuHash => {
                let cpu = crate::smp::current_cpu() as usize;
                if cpu < self.per_cpu_entries.len() {
                    let map = &mut self.per_cpu_entries[cpu];
                    if !map.contains_key(key) && map.len() >= self.max_entries as usize {
                        return Err(BpfError::MapFull);
                    }
                    map.insert(key.to_vec(), value.to_vec());
                    Ok(())
                } else {
                    Err(BpfError::OutOfBounds)
                }
            }
            BpfMapType::RingBuffer => {
                // Append to ring
                if self.ring.len() < self.ring_capacity {
                    self.ring.push(value.to_vec());
                } else {
                    let idx = self.ring_head % self.ring_capacity;
                    self.ring[idx] = value.to_vec();
                    self.ring_head = self.ring_head.saturating_add(1);
                }
                Ok(())
            }
            BpfMapType::PerfEventArray => {
                // Write to per-CPU perf ring
                let cpu = crate::smp::current_cpu() as usize;
                if cpu < self.perf_rings.len() {
                    let ring = &mut self.perf_rings[cpu];
                    if ring.len() < self.max_entries as usize {
                        ring.push(value.to_vec());
                    } else {
                        // Overwrite oldest
                        ring.remove(0);
                        ring.push(value.to_vec());
                    }
                    Ok(())
                } else {
                    Err(BpfError::OutOfBounds)
                }
            }
            BpfMapType::StackTrace => {
                Err(BpfError::InvalidMapOperation) // stack traces are recorded via helper
            }
        }
    }

    /// Delete a key from a map.
    pub fn delete(&mut self, key: &[u8]) -> Result<(), BpfError> {
        match self.map_type {
            BpfMapType::HashMap => {
                if self.entries.remove(key).is_some() {
                    Ok(())
                } else {
                    Err(BpfError::MapKeyNotFound)
                }
            }
            BpfMapType::LruHash => {
                if self.entries.remove(key).is_some() {
                    self.lru_order.retain(|k| k != key);
                    Ok(())
                } else {
                    Err(BpfError::MapKeyNotFound)
                }
            }
            BpfMapType::Array | BpfMapType::PerCpuArray => {
                // Arrays can't delete — zero out instead
                if key.len() < 4 {
                    return Err(BpfError::InvalidMapKey);
                }
                let idx = u32::from_ne_bytes([key[0], key[1], key[2], key[3]]) as usize;
                if idx >= self.max_entries as usize {
                    return Err(BpfError::OutOfBounds);
                }
                if self.map_type == BpfMapType::Array {
                    self.array[idx] = vec![0u8; self.value_size as usize];
                } else {
                    let cpu = crate::smp::current_cpu() as usize;
                    if cpu < self.per_cpu_arrays.len() {
                        self.per_cpu_arrays[cpu][idx] = vec![0u8; self.value_size as usize];
                    }
                }
                Ok(())
            }
            BpfMapType::PerCpuHash => {
                let cpu = crate::smp::current_cpu() as usize;
                if cpu < self.per_cpu_entries.len() {
                    if self.per_cpu_entries[cpu].remove(key).is_some() {
                        Ok(())
                    } else {
                        Err(BpfError::MapKeyNotFound)
                    }
                } else {
                    Err(BpfError::OutOfBounds)
                }
            }
            BpfMapType::StackTrace => {
                if key.len() < 4 {
                    return Err(BpfError::InvalidMapKey);
                }
                let stack_id = u32::from_ne_bytes([key[0], key[1], key[2], key[3]]);
                if self.stack_traces.remove(&stack_id).is_some() {
                    Ok(())
                } else {
                    Err(BpfError::MapKeyNotFound)
                }
            }
            BpfMapType::RingBuffer | BpfMapType::PerfEventArray => {
                Err(BpfError::InvalidMapOperation)
            }
        }
    }

    /// Drain ring buffer entries for consumption.
    pub fn ring_drain(&mut self) -> Vec<Vec<u8>> {
        core::mem::take(&mut self.ring)
    }

    /// Number of entries currently stored.
    pub fn len(&self) -> usize {
        match self.map_type {
            BpfMapType::HashMap | BpfMapType::LruHash => self.entries.len(),
            BpfMapType::Array | BpfMapType::PerCpuArray => self.max_entries as usize,
            BpfMapType::PerCpuHash => {
                let cpu = crate::smp::current_cpu() as usize;
                self.per_cpu_entries.get(cpu).map_or(0, |m| m.len())
            }
            BpfMapType::RingBuffer => self.ring.len(),
            BpfMapType::StackTrace => self.stack_traces.len(),
            BpfMapType::PerfEventArray => {
                let cpu = crate::smp::current_cpu() as usize;
                self.perf_rings.get(cpu).map_or(0, |r| r.len())
            }
        }
    }

    /// Record a stack trace and return a stack_id. Used by BPF helper.
    pub fn record_stack_trace(&mut self, ips: &[u64]) -> u32 {
        if self.map_type != BpfMapType::StackTrace {
            return 0;
        }
        let id = self.next_stack_id;
        self.next_stack_id = self.next_stack_id.saturating_add(1);

        // Respect max_entries
        if self.stack_traces.len() >= self.max_entries as usize {
            // Remove oldest
            if let Some(&oldest_key) = self.stack_traces.keys().next() {
                self.stack_traces.remove(&oldest_key);
            }
        }

        self.stack_traces.insert(id, ips.to_vec());
        id
    }

    /// Get a recorded stack trace by ID
    pub fn get_stack_trace(&self, stack_id: u32) -> Option<&Vec<u64>> {
        self.stack_traces.get(&stack_id)
    }

    /// Drain per-CPU perf event ring
    pub fn perf_drain(&mut self, cpu: usize) -> Vec<Vec<u8>> {
        if cpu < self.perf_rings.len() {
            core::mem::take(&mut self.perf_rings[cpu])
        } else {
            Vec::new()
        }
    }

    /// Lookup all per-CPU values for a given key (for per-CPU maps)
    pub fn lookup_all_cpus(&self, key: &[u8]) -> Vec<Option<Vec<u8>>> {
        match self.map_type {
            BpfMapType::PerCpuArray => {
                if key.len() < 4 {
                    return Vec::new();
                }
                let idx = u32::from_ne_bytes([key[0], key[1], key[2], key[3]]) as usize;
                self.per_cpu_arrays
                    .iter()
                    .map(|arr| arr.get(idx).cloned())
                    .collect()
            }
            BpfMapType::PerCpuHash => self
                .per_cpu_entries
                .iter()
                .map(|m| m.get(key).cloned())
                .collect(),
            _ => Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// BPF program
// ---------------------------------------------------------------------------

/// eBPF program
pub struct BpfProg {
    pub id: u32,
    pub name: String,
    pub prog_type: BpfProgType,
    pub insns: Vec<BpfInsn>,
    pub verified: bool,
    /// Map IDs this program is allowed to access
    pub map_ids: Vec<u32>,
    /// Expected helper functions (verified at load time)
    pub used_helpers: Vec<u32>,
}

// ---------------------------------------------------------------------------
// Verifier — register liveness, bounded loops, memory access safety
// ---------------------------------------------------------------------------

/// Verifier register state tracking
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegState {
    /// Register has not been written
    Uninitialized,
    /// Register holds a known scalar value
    Scalar,
    /// Register holds a pointer to the context
    CtxPtr,
    /// Register holds a pointer to the stack
    StackPtr,
    /// Register holds a pointer to a map value
    MapValuePtr,
    /// Register holds the frame pointer (R10)
    FramePtr,
}

/// Per-instruction verifier state (all 11 registers)
#[derive(Clone)]
struct VerifierState {
    reg_state: [RegState; 11],
    /// Scalar value bounds: (min, max) — only meaningful when state == Scalar
    reg_min: [i64; 11],
    reg_max: [i64; 11],
}

impl VerifierState {
    fn initial() -> Self {
        let mut s = VerifierState {
            reg_state: [RegState::Uninitialized; 11],
            reg_min: [0i64; 11],
            reg_max: [0i64; 11],
        };
        // R1 = context pointer, R10 = frame pointer
        s.reg_state[1] = RegState::CtxPtr;
        s.reg_state[10] = RegState::FramePtr;
        s
    }
}

/// Full verifier
pub struct BpfVerifier {
    /// States at each instruction (for branch exploration)
    states: Vec<Option<VerifierState>>,
    /// Instructions already visited
    visited: Vec<bool>,
    /// Worklist of (pc, state) for branch exploration
    worklist: Vec<(usize, VerifierState)>,
    /// Maximum backward jump count (bounded loop detection)
    max_back_edges: usize,
    back_edge_count: usize,
    /// Program type (determines allowed helpers and context size)
    prog_type: BpfProgType,
}

impl BpfVerifier {
    fn new(prog_len: usize, prog_type: BpfProgType) -> Self {
        BpfVerifier {
            states: vec![None; prog_len],
            visited: vec![false; prog_len],
            worklist: Vec::new(),
            max_back_edges: 32, // allow bounded loops up to 32 back-edges
            back_edge_count: 0,
            prog_type,
        }
    }

    fn is_alu_op(op: u8) -> bool {
        let class = op & 0x07;
        class == 0x04 || class == 0x07 // ALU32 or ALU64
    }

    fn is_jmp_op(op: u8) -> bool {
        let class = op & 0x07;
        class == 0x05 // JMP class
    }

    fn is_mem_load(op: u8) -> bool {
        matches!(
            op,
            op::LDX_B | op::LDX_H | op::LDX_W | op::LDX_DW | op::LD_DW
        )
    }

    fn is_mem_store(op: u8) -> bool {
        matches!(
            op,
            op::STX_B
                | op::STX_H
                | op::STX_W
                | op::STX_DW
                | op::ST_B
                | op::ST_H
                | op::ST_W
                | op::ST_DW
        )
    }

    fn allowed_helper(&self, helper_id: u32) -> bool {
        // Common helpers allowed for all program types
        let common: &[u32] = &[1, 2, 3, 6, 14, 15, 16, 113];
        if common.contains(&helper_id) {
            return true;
        }

        match self.prog_type {
            BpfProgType::Kprobe | BpfProgType::Tracepoint | BpfProgType::PerfEvent => {
                // trace-related helpers
                matches!(helper_id, 35 | 100..=120)
            }
            BpfProgType::XdpAction | BpfProgType::SocketFilter | BpfProgType::CgroupSkb => {
                // packet-related helpers
                matches!(helper_id, 50..=80)
            }
            _ => false,
        }
    }

    /// Run the verifier on a program. Returns Ok(()) if safe.
    fn verify(&mut self, prog: &BpfProg) -> Result<(), BpfError> {
        let insns = &prog.insns;
        let len = insns.len();

        if len == 0 {
            return Err(BpfError::VerificationFailed(String::from("empty program")));
        }
        if len > 4096 {
            return Err(BpfError::VerificationFailed(String::from(
                "program too large (>4096 insns)",
            )));
        }

        // Last instruction must be EXIT
        if insns.last().map(|i| i.op) != Some(op::EXIT) {
            return Err(BpfError::VerificationFailed(String::from(
                "must end with EXIT",
            )));
        }

        // Pass 1: structural checks (register indices, jump targets)
        for (i, insn) in insns.iter().enumerate() {
            if insn.dst() > 10 || insn.src() > 10 {
                return Err(BpfError::VerificationFailed(alloc::format!(
                    "invalid register at insn {}",
                    i
                )));
            }
            // R10 (frame pointer) is read-only
            if insn.dst() == 10 && Self::is_alu_op(insn.op) {
                return Err(BpfError::VerificationFailed(alloc::format!(
                    "write to R10 (frame pointer) at insn {}",
                    i
                )));
            }
            // Jump target bounds
            if Self::is_jmp_op(insn.op) && insn.op != op::CALL && insn.op != op::EXIT {
                let target = i as i64 + insn.off as i64 + 1;
                if target < 0 || target >= len as i64 {
                    return Err(BpfError::VerificationFailed(alloc::format!(
                        "jump out of bounds at insn {} -> {}",
                        i,
                        target
                    )));
                }
            }
            // LD_DW takes 2 instruction slots
            if insn.op == op::LD_DW && i + 1 >= len {
                return Err(BpfError::VerificationFailed(alloc::format!(
                    "LD_DW at end of program (needs 2 slots) at insn {}",
                    i
                )));
            }
            // Validate helper calls
            if insn.op == op::CALL {
                if !self.allowed_helper(insn.imm as u32) {
                    return Err(BpfError::VerificationFailed(alloc::format!(
                        "disallowed helper {} for prog type {:?} at insn {}",
                        insn.imm,
                        self.prog_type,
                        i
                    )));
                }
            }
        }

        // Pass 2: abstract interpretation (register liveness + bounded loops)
        self.worklist.push((0, VerifierState::initial()));

        while let Some((pc, state)) = self.worklist.pop() {
            if pc >= len {
                return Err(BpfError::VerificationFailed(alloc::format!(
                    "execution fell off end at pc={}",
                    pc
                )));
            }

            // If we already visited this PC, check for back-edge
            if self.visited[pc] {
                self.back_edge_count = self.back_edge_count.saturating_add(1);
                if self.back_edge_count > self.max_back_edges {
                    return Err(BpfError::VerificationFailed(alloc::format!(
                        "too many back-edges ({}) — possible unbounded loop",
                        self.back_edge_count
                    )));
                }
                continue;
            }

            self.visited[pc] = true;
            self.states[pc] = Some(state.clone());

            let insn = insns[pc];
            let mut next_state = state;

            // Simulate effect on register state
            match insn.op {
                // ALU ops: dst becomes Scalar
                _ if Self::is_alu_op(insn.op) => {
                    let dst = insn.dst();
                    if dst < 10 {
                        // Check source register is initialized if using reg operand
                        if (insn.op & 0x08) != 0 {
                            // REG variant
                            let src = insn.src();
                            if next_state.reg_state[src] == RegState::Uninitialized {
                                return Err(BpfError::VerificationFailed(alloc::format!(
                                    "use of uninitialized R{} at insn {}",
                                    src,
                                    pc
                                )));
                            }
                        }
                        next_state.reg_state[dst] = RegState::Scalar;
                    }
                }
                // MOV sets dst
                op::MOV_IMM | op::MOV32_IMM => {
                    next_state.reg_state[insn.dst()] = RegState::Scalar;
                    next_state.reg_min[insn.dst()] = insn.imm as i64;
                    next_state.reg_max[insn.dst()] = insn.imm as i64;
                }
                op::MOV_REG | op::MOV32_REG => {
                    let src = insn.src();
                    if next_state.reg_state[src] == RegState::Uninitialized {
                        return Err(BpfError::VerificationFailed(alloc::format!(
                            "use of uninitialized R{} at insn {}",
                            src,
                            pc
                        )));
                    }
                    next_state.reg_state[insn.dst()] = next_state.reg_state[src];
                }
                // Memory loads: check source pointer, dst becomes appropriate type
                _ if Self::is_mem_load(insn.op) => {
                    let src = insn.src();
                    match next_state.reg_state[src] {
                        RegState::CtxPtr
                        | RegState::StackPtr
                        | RegState::MapValuePtr
                        | RegState::FramePtr => {}
                        _ => {
                            return Err(BpfError::VerificationFailed(alloc::format!(
                                "load from non-pointer R{} (state={:?}) at insn {}",
                                src,
                                next_state.reg_state[src],
                                pc
                            )));
                        }
                    }
                    if insn.op == op::LD_DW {
                        next_state.reg_state[insn.dst()] = RegState::Scalar;
                    } else {
                        next_state.reg_state[insn.dst()] = RegState::Scalar;
                    }
                }
                // Memory stores: check dst is a pointer
                _ if Self::is_mem_store(insn.op) => {
                    let dst = insn.dst();
                    match next_state.reg_state[dst] {
                        RegState::CtxPtr
                        | RegState::StackPtr
                        | RegState::MapValuePtr
                        | RegState::FramePtr => {}
                        _ => {
                            return Err(BpfError::VerificationFailed(alloc::format!(
                                "store to non-pointer R{} (state={:?}) at insn {}",
                                dst,
                                next_state.reg_state[dst],
                                pc
                            )));
                        }
                    }
                }
                // CALL: R0 gets return value, R1-R5 are caller-saved (become uninitialized)
                op::CALL => {
                    let helper_id = insn.imm as u32;
                    // map_lookup returns a MapValuePtr (or NULL=Scalar)
                    next_state.reg_state[0] = if helper_id == 1 {
                        RegState::MapValuePtr
                    } else {
                        RegState::Scalar
                    };
                    // Caller-saved registers: R1-R5 are clobbered
                    for r in 1..=5 {
                        next_state.reg_state[r] = RegState::Uninitialized;
                    }
                }
                // EXIT: R0 must be initialized
                op::EXIT => {
                    if next_state.reg_state[0] == RegState::Uninitialized {
                        return Err(BpfError::VerificationFailed(alloc::format!(
                            "EXIT with uninitialized R0 at insn {}",
                            pc
                        )));
                    }
                    continue; // no successor
                }
                // Conditional branches: explore both paths
                _ if Self::is_jmp_op(insn.op) && insn.op != op::CALL && insn.op != op::EXIT => {
                    if insn.op == op::JA {
                        let target = (pc as i64 + insn.off as i64 + 1) as usize;
                        self.worklist.push((target, next_state));
                        continue;
                    }
                    // Check src register for reg-variant jumps
                    if (insn.op & 0x08) != 0 {
                        let src = insn.src();
                        if next_state.reg_state[src] == RegState::Uninitialized {
                            return Err(BpfError::VerificationFailed(alloc::format!(
                                "branch on uninitialized R{} at insn {}",
                                src,
                                pc
                            )));
                        }
                    }
                    if next_state.reg_state[insn.dst()] == RegState::Uninitialized {
                        return Err(BpfError::VerificationFailed(alloc::format!(
                            "branch on uninitialized R{} at insn {}",
                            insn.dst(),
                            pc
                        )));
                    }
                    let target = (pc as i64 + insn.off as i64 + 1) as usize;
                    self.worklist.push((target, next_state.clone()));
                    // Fall through
                    self.worklist.push((pc + 1, next_state));
                    continue;
                }
                _ => {}
            }

            // Default fall-through to next instruction
            let next_pc = if insn.op == op::LD_DW { pc + 2 } else { pc + 1 };
            if next_pc < len {
                self.worklist.push((next_pc, next_state));
            }
        }

        // Ensure EXIT is reachable
        let exit_reachable = insns
            .iter()
            .enumerate()
            .any(|(i, insn)| insn.op == op::EXIT && self.visited[i]);
        if !exit_reachable {
            return Err(BpfError::VerificationFailed(String::from(
                "no reachable EXIT instruction",
            )));
        }

        Ok(())
    }
}

/// Public verification entry point
pub fn verify(prog: &BpfProg) -> Result<(), BpfError> {
    let mut verifier = BpfVerifier::new(prog.insns.len(), prog.prog_type);
    verifier.verify(prog)
}

// ---------------------------------------------------------------------------
// Helper function dispatch table
// ---------------------------------------------------------------------------

/// BPF helper function IDs
pub mod helpers {
    pub const MAP_LOOKUP_ELEM: u32 = 1;
    pub const MAP_UPDATE_ELEM: u32 = 2;
    pub const MAP_DELETE_ELEM: u32 = 3;
    pub const PROBE_READ: u32 = 4;
    pub const KTIME_GET_NS: u32 = 5;
    pub const TRACE_PRINTK: u32 = 6;
    pub const GET_PRANDOM_U32: u32 = 7;
    pub const GET_SMP_PROCESSOR_ID: u32 = 8;
    pub const TAIL_CALL: u32 = 12;
    pub const GET_CURRENT_PID_TGID: u32 = 14;
    pub const GET_CURRENT_UID_GID: u32 = 15;
    pub const GET_CURRENT_COMM: u32 = 16;
    pub const PERF_EVENT_OUTPUT: u32 = 25;
    pub const REDIRECT: u32 = 51;
    pub const XDP_ADJUST_HEAD: u32 = 44;
    pub const GET_KTIME_NS: u32 = 113;
    pub const GET_STACK: u32 = 67;
    pub const PERF_EVENT_READ: u32 = 22;
    pub const GET_NUMA_NODE_ID: u32 = 42;
    pub const PROBE_READ_KERNEL: u32 = 113;
    pub const RINGBUF_OUTPUT: u32 = 130;
    pub const RINGBUF_RESERVE: u32 = 131;
    pub const RINGBUF_SUBMIT: u32 = 132;
    pub const GET_FUNC_IP: u32 = 173;
}

/// Simple PRNG state for bpf_get_prandom_u32
static PRNG_STATE: AtomicU32 = AtomicU32::new(0x12345678);

fn prng_u32() -> u32 {
    // xorshift32
    let mut s = PRNG_STATE.load(Ordering::Relaxed);
    s ^= s << 13;
    s ^= s >> 17;
    s ^= s << 5;
    PRNG_STATE.store(s, Ordering::Relaxed);
    s
}

/// Call a BPF helper function. `regs` holds R1-R5 arguments, returns R0.
fn call_helper(id: u32, regs: &[u64; 11]) -> u64 {
    match id {
        helpers::MAP_LOOKUP_ELEM => {
            // R1 = map_id, R2 = key pointer
            let map_id = regs[1] as u32;
            let key_ptr = regs[2];
            let maps = MAPS.lock();
            if let Some(map) = maps.iter().find(|m| m.id == map_id) {
                // Read key from the key pointer (simplified: first 4/8 bytes)
                let key_bytes = unsafe {
                    core::slice::from_raw_parts(key_ptr as *const u8, map.key_size as usize)
                };
                if let Some(val) = map.lookup(key_bytes) {
                    val.as_ptr() as u64
                } else {
                    0 // NULL = not found
                }
            } else {
                0
            }
        }
        helpers::MAP_UPDATE_ELEM => {
            // R1 = map_id, R2 = key ptr, R3 = value ptr, R4 = flags
            let map_id = regs[1] as u32;
            let key_ptr = regs[2];
            let val_ptr = regs[3];
            let mut maps = MAPS.lock();
            if let Some(map) = maps.iter_mut().find(|m| m.id == map_id) {
                let key = unsafe {
                    core::slice::from_raw_parts(key_ptr as *const u8, map.key_size as usize)
                };
                let val = unsafe {
                    core::slice::from_raw_parts(val_ptr as *const u8, map.value_size as usize)
                };
                if map.update(key, val).is_ok() {
                    0
                } else {
                    !0u64
                }
            } else {
                !0u64
            }
        }
        helpers::MAP_DELETE_ELEM => {
            let map_id = regs[1] as u32;
            let key_ptr = regs[2];
            let mut maps = MAPS.lock();
            if let Some(map) = maps.iter_mut().find(|m| m.id == map_id) {
                let key = unsafe {
                    core::slice::from_raw_parts(key_ptr as *const u8, map.key_size as usize)
                };
                if map.delete(key).is_ok() {
                    0
                } else {
                    !0u64
                }
            } else {
                !0u64
            }
        }
        helpers::PROBE_READ => {
            // R1 = dst, R2 = size, R3 = src (unsafe kernel read)
            let dst = regs[1] as *mut u8;
            let size = regs[2] as usize;
            let src = regs[3] as *const u8;
            if size <= 256 {
                unsafe {
                    core::ptr::copy_nonoverlapping(src, dst, size);
                }
                0
            } else {
                !0u64
            }
        }
        helpers::KTIME_GET_NS | helpers::GET_KTIME_NS => {
            crate::time::clock::uptime_ms() * 1_000_000
        }
        helpers::TRACE_PRINTK => {
            // R1 = fmt string pointer (simplified: just log that it was called)
            crate::serial_println!("  [ebpf] trace_printk called (R1={:#x})", regs[1]);
            0
        }
        helpers::GET_PRANDOM_U32 => prng_u32() as u64,
        helpers::GET_SMP_PROCESSOR_ID => crate::smp::current_cpu() as u64,
        helpers::GET_CURRENT_PID_TGID => {
            let pid = crate::process::getpid() as u64;
            // Return pid in lower 32 bits, tgid in upper (same for now)
            (pid << 32) | pid
        }
        helpers::GET_CURRENT_UID_GID => 0,
        helpers::GET_CURRENT_COMM => 0,
        helpers::PERF_EVENT_OUTPUT => {
            // R1 = ctx, R2 = map_id, R3 = flags, R4 = data, R5 = size
            0
        }
        helpers::TAIL_CALL => {
            // R1 = ctx, R2 = prog_array_map_id, R3 = index
            // Tail calls replace the current program — not implemented in this VM
            0
        }
        helpers::REDIRECT => {
            // R1 = ifindex, R2 = flags  (XDP redirect)
            regs[1] // return the ifindex as the redirect target
        }
        helpers::XDP_ADJUST_HEAD => {
            // R1 = xdp_md ptr, R2 = delta
            0
        }
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// eBPF VM — fetch-decode-execute engine
// ---------------------------------------------------------------------------

/// eBPF VM state (per-execution)
pub struct BpfVm {
    /// 11 registers (R0-R10, R10 = frame pointer)
    regs: [u64; 11],
    /// 512-byte stack
    stack: [u8; 512],
    /// Program counter
    pc: usize,
    /// Instruction count (for termination guarantee)
    insn_count: u64,
    /// Max instructions per execution
    max_insns: u64,
}

impl BpfVm {
    pub fn new() -> Self {
        BpfVm {
            regs: [0u64; 11],
            stack: [0u8; 512],
            pc: 0,
            insn_count: 0,
            max_insns: 1_000_000,
        }
    }

    /// Set the max instruction limit
    pub fn set_max_insns(&mut self, max: u64) {
        self.max_insns = max;
    }

    /// Read a value from the VM stack at given offset from frame pointer
    fn stack_read(&self, offset: i16, size: u8) -> u64 {
        let base = 512i32 + offset as i32;
        if base < 0 || (base as usize + size as usize) > 512 {
            return 0;
        }
        let b = base as usize;
        match size {
            1 => self.stack[b] as u64,
            2 => u16::from_le_bytes([self.stack[b], self.stack[b + 1]]) as u64,
            4 => u32::from_le_bytes([
                self.stack[b],
                self.stack[b + 1],
                self.stack[b + 2],
                self.stack[b + 3],
            ]) as u64,
            8 => u64::from_le_bytes([
                self.stack[b],
                self.stack[b + 1],
                self.stack[b + 2],
                self.stack[b + 3],
                self.stack[b + 4],
                self.stack[b + 5],
                self.stack[b + 6],
                self.stack[b + 7],
            ]),
            _ => 0,
        }
    }

    /// Write a value to the VM stack at given offset from frame pointer
    fn stack_write(&mut self, offset: i16, size: u8, value: u64) {
        let base = 512i32 + offset as i32;
        if base < 0 || (base as usize + size as usize) > 512 {
            return;
        }
        let b = base as usize;
        match size {
            1 => {
                self.stack[b] = value as u8;
            }
            2 => {
                let bytes = (value as u16).to_le_bytes();
                self.stack[b..b + 2].copy_from_slice(&bytes);
            }
            4 => {
                let bytes = (value as u32).to_le_bytes();
                self.stack[b..b + 4].copy_from_slice(&bytes);
            }
            8 => {
                let bytes = value.to_le_bytes();
                self.stack[b..b + 8].copy_from_slice(&bytes);
            }
            _ => {}
        }
    }

    /// Execute a BPF program. Returns R0 (the return value).
    pub fn execute(&mut self, prog: &BpfProg, ctx: u64) -> Result<u64, BpfError> {
        if !prog.verified {
            return Err(BpfError::NotVerified);
        }

        self.regs = [0u64; 11];
        self.stack = [0u8; 512];
        self.regs[1] = ctx;
        self.regs[10] = &self.stack as *const _ as u64 + 512; // frame pointer
        self.pc = 0;
        self.insn_count = 0;

        let insns = &prog.insns;
        let len = insns.len();

        while self.pc < len {
            self.insn_count = self.insn_count.saturating_add(1);
            if self.insn_count > self.max_insns {
                return Err(BpfError::ExceededInsnLimit);
            }

            let insn = insns[self.pc];
            let dst = insn.dst();
            let src = insn.src();
            let imm = insn.imm;

            match insn.op {
                // ========== ALU 64-bit ==========
                op::ADD_IMM => self.regs[dst] = self.regs[dst].wrapping_add(imm as i64 as u64),
                op::ADD_REG => self.regs[dst] = self.regs[dst].wrapping_add(self.regs[src]),
                op::SUB_IMM => self.regs[dst] = self.regs[dst].wrapping_sub(imm as i64 as u64),
                op::SUB_REG => self.regs[dst] = self.regs[dst].wrapping_sub(self.regs[src]),
                op::MUL_IMM => self.regs[dst] = self.regs[dst].wrapping_mul(imm as i64 as u64),
                op::MUL_REG => self.regs[dst] = self.regs[dst].wrapping_mul(self.regs[src]),
                op::DIV_IMM => {
                    let d = imm as u64;
                    if d == 0 {
                        return Err(BpfError::DivByZero);
                    }
                    self.regs[dst] /= d;
                }
                op::DIV_REG => {
                    if self.regs[src] == 0 {
                        return Err(BpfError::DivByZero);
                    }
                    self.regs[dst] /= self.regs[src];
                }
                op::OR_IMM => self.regs[dst] |= imm as i64 as u64,
                op::OR_REG => self.regs[dst] |= self.regs[src],
                op::AND_IMM => self.regs[dst] &= imm as i64 as u64,
                op::AND_REG => self.regs[dst] &= self.regs[src],
                op::LSH_IMM => self.regs[dst] = self.regs[dst].wrapping_shl(imm as u32),
                op::LSH_REG => self.regs[dst] = self.regs[dst].wrapping_shl(self.regs[src] as u32),
                op::RSH_IMM => self.regs[dst] = self.regs[dst].wrapping_shr(imm as u32),
                op::RSH_REG => self.regs[dst] = self.regs[dst].wrapping_shr(self.regs[src] as u32),
                op::NEG => self.regs[dst] = (-(self.regs[dst] as i64)) as u64,
                op::MOD_IMM => {
                    let d = imm as u64;
                    if d == 0 {
                        return Err(BpfError::DivByZero);
                    }
                    self.regs[dst] %= d;
                }
                op::MOD_REG => {
                    if self.regs[src] == 0 {
                        return Err(BpfError::DivByZero);
                    }
                    self.regs[dst] %= self.regs[src];
                }
                op::XOR_IMM => self.regs[dst] ^= imm as i64 as u64,
                op::XOR_REG => self.regs[dst] ^= self.regs[src],
                op::MOV_IMM => self.regs[dst] = imm as i64 as u64,
                op::MOV_REG => self.regs[dst] = self.regs[src],
                op::ARSH_IMM => {
                    self.regs[dst] = ((self.regs[dst] as i64).wrapping_shr(imm as u32)) as u64
                }
                op::ARSH_REG => {
                    self.regs[dst] =
                        ((self.regs[dst] as i64).wrapping_shr(self.regs[src] as u32)) as u64
                }

                // ========== ALU 32-bit (zero-extend result to 64 bits) ==========
                op::ADD32_IMM => {
                    self.regs[dst] = (self.regs[dst] as u32).wrapping_add(imm as u32) as u64
                }
                op::ADD32_REG => {
                    self.regs[dst] =
                        (self.regs[dst] as u32).wrapping_add(self.regs[src] as u32) as u64
                }
                op::SUB32_IMM => {
                    self.regs[dst] = (self.regs[dst] as u32).wrapping_sub(imm as u32) as u64
                }
                op::SUB32_REG => {
                    self.regs[dst] =
                        (self.regs[dst] as u32).wrapping_sub(self.regs[src] as u32) as u64
                }
                op::MUL32_IMM => {
                    self.regs[dst] = (self.regs[dst] as u32).wrapping_mul(imm as u32) as u64
                }
                op::MUL32_REG => {
                    self.regs[dst] =
                        (self.regs[dst] as u32).wrapping_mul(self.regs[src] as u32) as u64
                }
                op::DIV32_IMM => {
                    let d = imm as u32;
                    if d == 0 {
                        return Err(BpfError::DivByZero);
                    }
                    self.regs[dst] = ((self.regs[dst] as u32) / d) as u64;
                }
                op::DIV32_REG => {
                    let d = self.regs[src] as u32;
                    if d == 0 {
                        return Err(BpfError::DivByZero);
                    }
                    self.regs[dst] = ((self.regs[dst] as u32) / d) as u64;
                }
                op::OR32_IMM => self.regs[dst] = ((self.regs[dst] as u32) | (imm as u32)) as u64,
                op::OR32_REG => {
                    self.regs[dst] = ((self.regs[dst] as u32) | (self.regs[src] as u32)) as u64
                }
                op::AND32_IMM => self.regs[dst] = ((self.regs[dst] as u32) & (imm as u32)) as u64,
                op::AND32_REG => {
                    self.regs[dst] = ((self.regs[dst] as u32) & (self.regs[src] as u32)) as u64
                }
                op::LSH32_IMM => {
                    self.regs[dst] = (self.regs[dst] as u32).wrapping_shl(imm as u32) as u64
                }
                op::LSH32_REG => {
                    self.regs[dst] =
                        (self.regs[dst] as u32).wrapping_shl(self.regs[src] as u32) as u64
                }
                op::RSH32_IMM => {
                    self.regs[dst] = (self.regs[dst] as u32).wrapping_shr(imm as u32) as u64
                }
                op::RSH32_REG => {
                    self.regs[dst] =
                        (self.regs[dst] as u32).wrapping_shr(self.regs[src] as u32) as u64
                }
                op::NEG32 => {
                    self.regs[dst] = (-((self.regs[dst] as i32) as i64)) as u64 & 0xFFFF_FFFF
                }
                op::MOD32_IMM => {
                    let d = imm as u32;
                    if d == 0 {
                        return Err(BpfError::DivByZero);
                    }
                    self.regs[dst] = ((self.regs[dst] as u32) % d) as u64;
                }
                op::MOD32_REG => {
                    let d = self.regs[src] as u32;
                    if d == 0 {
                        return Err(BpfError::DivByZero);
                    }
                    self.regs[dst] = ((self.regs[dst] as u32) % d) as u64;
                }
                op::XOR32_IMM => self.regs[dst] = ((self.regs[dst] as u32) ^ (imm as u32)) as u64,
                op::XOR32_REG => {
                    self.regs[dst] = ((self.regs[dst] as u32) ^ (self.regs[src] as u32)) as u64
                }
                op::MOV32_IMM => self.regs[dst] = imm as u32 as u64,
                op::MOV32_REG => self.regs[dst] = self.regs[src] as u32 as u64,
                op::ARSH32_IMM => {
                    self.regs[dst] =
                        ((self.regs[dst] as i32).wrapping_shr(imm as u32) as u32) as u64
                }
                op::ARSH32_REG => {
                    self.regs[dst] =
                        ((self.regs[dst] as i32).wrapping_shr(self.regs[src] as u32) as u32) as u64
                }

                // ========== Byte swap / endian =========
                op::LE => {
                    // host to little-endian — on LE hosts this is a no-op truncation
                    match imm {
                        16 => self.regs[dst] = (self.regs[dst] as u16).to_le() as u64,
                        32 => self.regs[dst] = (self.regs[dst] as u32).to_le() as u64,
                        64 => self.regs[dst] = self.regs[dst].to_le(),
                        _ => {}
                    }
                }
                op::BE => match imm {
                    16 => self.regs[dst] = (self.regs[dst] as u16).to_be() as u64,
                    32 => self.regs[dst] = (self.regs[dst] as u32).to_be() as u64,
                    64 => self.regs[dst] = self.regs[dst].to_be(),
                    _ => {}
                },

                // ========== Memory: load from register + offset ==========
                op::LD_DW => {
                    // 64-bit immediate load spanning two instructions
                    if self.pc + 1 >= len {
                        return Err(BpfError::OutOfBounds);
                    }
                    let lo = imm as u32 as u64;
                    let hi = insns[self.pc + 1].imm as u32 as u64;
                    self.regs[dst] = lo | (hi << 32);
                    self.pc += 1; // skip the second instruction slot
                }
                op::LDX_B => {
                    let addr = self.regs[src].wrapping_add(insn.off as i64 as u64);
                    self.regs[dst] = unsafe { core::ptr::read_volatile(addr as *const u8) } as u64;
                }
                op::LDX_H => {
                    let addr = self.regs[src].wrapping_add(insn.off as i64 as u64);
                    self.regs[dst] = unsafe { core::ptr::read_volatile(addr as *const u16) } as u64;
                }
                op::LDX_W => {
                    let addr = self.regs[src].wrapping_add(insn.off as i64 as u64);
                    self.regs[dst] = unsafe { core::ptr::read_volatile(addr as *const u32) } as u64;
                }
                op::LDX_DW => {
                    let addr = self.regs[src].wrapping_add(insn.off as i64 as u64);
                    self.regs[dst] = unsafe { core::ptr::read_volatile(addr as *const u64) };
                }

                // ========== Memory: store register to [reg+offset] ==========
                op::STX_B => {
                    let addr = self.regs[dst].wrapping_add(insn.off as i64 as u64);
                    unsafe {
                        core::ptr::write_volatile(addr as *mut u8, self.regs[src] as u8);
                    }
                }
                op::STX_H => {
                    let addr = self.regs[dst].wrapping_add(insn.off as i64 as u64);
                    unsafe {
                        core::ptr::write_volatile(addr as *mut u16, self.regs[src] as u16);
                    }
                }
                op::STX_W => {
                    let addr = self.regs[dst].wrapping_add(insn.off as i64 as u64);
                    unsafe {
                        core::ptr::write_volatile(addr as *mut u32, self.regs[src] as u32);
                    }
                }
                op::STX_DW => {
                    let addr = self.regs[dst].wrapping_add(insn.off as i64 as u64);
                    unsafe {
                        core::ptr::write_volatile(addr as *mut u64, self.regs[src]);
                    }
                }

                // ========== Memory: store immediate to [reg+offset] ==========
                op::ST_B => {
                    let addr = self.regs[dst].wrapping_add(insn.off as i64 as u64);
                    unsafe {
                        core::ptr::write_volatile(addr as *mut u8, imm as u8);
                    }
                }
                op::ST_H => {
                    let addr = self.regs[dst].wrapping_add(insn.off as i64 as u64);
                    unsafe {
                        core::ptr::write_volatile(addr as *mut u16, imm as u16);
                    }
                }
                op::ST_W => {
                    let addr = self.regs[dst].wrapping_add(insn.off as i64 as u64);
                    unsafe {
                        core::ptr::write_volatile(addr as *mut u32, imm as u32);
                    }
                }
                op::ST_DW => {
                    let addr = self.regs[dst].wrapping_add(insn.off as i64 as u64);
                    unsafe {
                        core::ptr::write_volatile(addr as *mut u64, imm as i64 as u64);
                    }
                }

                // ========== Branch / JMP ==========
                op::JA => {
                    self.pc = (self.pc as i64 + insn.off as i64) as usize;
                }
                op::JEQ_IMM => {
                    if self.regs[dst] == imm as i64 as u64 {
                        self.pc = (self.pc as i64 + insn.off as i64) as usize;
                    }
                }
                op::JEQ_REG => {
                    if self.regs[dst] == self.regs[src] {
                        self.pc = (self.pc as i64 + insn.off as i64) as usize;
                    }
                }
                op::JGT_IMM => {
                    if self.regs[dst] > imm as i64 as u64 {
                        self.pc = (self.pc as i64 + insn.off as i64) as usize;
                    }
                }
                op::JGT_REG => {
                    if self.regs[dst] > self.regs[src] {
                        self.pc = (self.pc as i64 + insn.off as i64) as usize;
                    }
                }
                op::JGE_IMM => {
                    if self.regs[dst] >= imm as i64 as u64 {
                        self.pc = (self.pc as i64 + insn.off as i64) as usize;
                    }
                }
                op::JGE_REG => {
                    if self.regs[dst] >= self.regs[src] {
                        self.pc = (self.pc as i64 + insn.off as i64) as usize;
                    }
                }
                op::JSET_IMM => {
                    if (self.regs[dst] & imm as i64 as u64) != 0 {
                        self.pc = (self.pc as i64 + insn.off as i64) as usize;
                    }
                }
                op::JSET_REG => {
                    if (self.regs[dst] & self.regs[src]) != 0 {
                        self.pc = (self.pc as i64 + insn.off as i64) as usize;
                    }
                }
                op::JNE_IMM => {
                    if self.regs[dst] != imm as i64 as u64 {
                        self.pc = (self.pc as i64 + insn.off as i64) as usize;
                    }
                }
                op::JNE_REG => {
                    if self.regs[dst] != self.regs[src] {
                        self.pc = (self.pc as i64 + insn.off as i64) as usize;
                    }
                }
                op::JSGT_IMM => {
                    if (self.regs[dst] as i64) > imm as i64 {
                        self.pc = (self.pc as i64 + insn.off as i64) as usize;
                    }
                }
                op::JSGT_REG => {
                    if (self.regs[dst] as i64) > (self.regs[src] as i64) {
                        self.pc = (self.pc as i64 + insn.off as i64) as usize;
                    }
                }
                op::JSGE_IMM => {
                    if (self.regs[dst] as i64) >= imm as i64 {
                        self.pc = (self.pc as i64 + insn.off as i64) as usize;
                    }
                }
                op::JSGE_REG => {
                    if (self.regs[dst] as i64) >= (self.regs[src] as i64) {
                        self.pc = (self.pc as i64 + insn.off as i64) as usize;
                    }
                }
                op::JLT_IMM => {
                    if self.regs[dst] < imm as i64 as u64 {
                        self.pc = (self.pc as i64 + insn.off as i64) as usize;
                    }
                }
                op::JLT_REG => {
                    if self.regs[dst] < self.regs[src] {
                        self.pc = (self.pc as i64 + insn.off as i64) as usize;
                    }
                }
                op::JLE_IMM => {
                    if self.regs[dst] <= imm as i64 as u64 {
                        self.pc = (self.pc as i64 + insn.off as i64) as usize;
                    }
                }
                op::JLE_REG => {
                    if self.regs[dst] <= self.regs[src] {
                        self.pc = (self.pc as i64 + insn.off as i64) as usize;
                    }
                }
                op::JSLT_IMM => {
                    if (self.regs[dst] as i64) < imm as i64 {
                        self.pc = (self.pc as i64 + insn.off as i64) as usize;
                    }
                }
                op::JSLT_REG => {
                    if (self.regs[dst] as i64) < (self.regs[src] as i64) {
                        self.pc = (self.pc as i64 + insn.off as i64) as usize;
                    }
                }
                op::JSLE_IMM => {
                    if (self.regs[dst] as i64) <= imm as i64 {
                        self.pc = (self.pc as i64 + insn.off as i64) as usize;
                    }
                }
                op::JSLE_REG => {
                    if (self.regs[dst] as i64) <= (self.regs[src] as i64) {
                        self.pc = (self.pc as i64 + insn.off as i64) as usize;
                    }
                }

                op::CALL => {
                    self.regs[0] = call_helper(imm as u32, &self.regs);
                }

                op::EXIT => {
                    return Ok(self.regs[0]);
                }

                _ => return Err(BpfError::InvalidOpcode(insn.op)),
            }

            self.pc += 1;
        }

        Ok(self.regs[0])
    }
}

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum BpfError {
    NotVerified,
    InvalidOpcode(u8),
    DivByZero,
    OutOfBounds,
    ExceededInsnLimit,
    VerificationFailed(String),
    InvalidMapKey,
    InvalidMapValue,
    MapFull,
    MapKeyNotFound,
    InvalidMapOperation,
    MapNotFound,
    ProgNotFound,
}

// ---------------------------------------------------------------------------
// Global registries
// ---------------------------------------------------------------------------

static PROGRAMS: Mutex<Vec<BpfProg>> = Mutex::new(Vec::new());
static MAPS: Mutex<Vec<BpfMap>> = Mutex::new(Vec::new());
static NEXT_PROG_ID: AtomicU32 = AtomicU32::new(1);
static NEXT_MAP_ID: AtomicU32 = AtomicU32::new(1);

// ---------------------------------------------------------------------------
// Map management API
// ---------------------------------------------------------------------------

/// Create a BPF map. Returns the map ID.
pub fn create_map(
    name: &str,
    map_type: BpfMapType,
    key_size: u32,
    value_size: u32,
    max_entries: u32,
) -> u32 {
    let id = NEXT_MAP_ID.fetch_add(1, Ordering::Relaxed);
    let map = BpfMap::new(id, name, map_type, key_size, value_size, max_entries);
    MAPS.lock().push(map);
    id
}

/// Lookup a value in a map (userspace API).
pub fn map_lookup(map_id: u32, key: &[u8]) -> Option<Vec<u8>> {
    let maps = MAPS.lock();
    maps.iter()
        .find(|m| m.id == map_id)
        .and_then(|m| m.lookup(key).map(|v| v.to_vec()))
}

/// Update a value in a map (userspace API).
pub fn map_update(map_id: u32, key: &[u8], value: &[u8]) -> Result<(), BpfError> {
    let mut maps = MAPS.lock();
    let map = maps
        .iter_mut()
        .find(|m| m.id == map_id)
        .ok_or(BpfError::MapNotFound)?;
    map.update(key, value)
}

/// Delete a key from a map.
pub fn map_delete(map_id: u32, key: &[u8]) -> Result<(), BpfError> {
    let mut maps = MAPS.lock();
    let map = maps
        .iter_mut()
        .find(|m| m.id == map_id)
        .ok_or(BpfError::MapNotFound)?;
    map.delete(key)
}

/// List all maps.
pub fn list_maps() -> Vec<(u32, String, BpfMapType, u32)> {
    let maps = MAPS.lock();
    maps.iter()
        .map(|m| (m.id, m.name.clone(), m.map_type, m.max_entries))
        .collect()
}

// ---------------------------------------------------------------------------
// Program management API
// ---------------------------------------------------------------------------

/// Load a BPF program (verify, then register).
pub fn load_prog(name: &str, prog_type: BpfProgType, insns: Vec<BpfInsn>) -> Result<u32, BpfError> {
    let mut prog = BpfProg {
        id: NEXT_PROG_ID.fetch_add(1, Ordering::Relaxed),
        name: String::from(name),
        prog_type,
        insns,
        verified: false,
        map_ids: Vec::new(),
        used_helpers: Vec::new(),
    };

    verify(&prog)?;
    prog.verified = true;

    let id = prog.id;
    PROGRAMS.lock().push(prog);
    crate::serial_println!(
        "  [ebpf] Loaded program '{}' (id={}, type={:?})",
        name,
        id,
        prog_type
    );
    Ok(id)
}

/// Load a BPF program with map associations.
pub fn load_prog_with_maps(
    name: &str,
    prog_type: BpfProgType,
    insns: Vec<BpfInsn>,
    map_ids: Vec<u32>,
) -> Result<u32, BpfError> {
    let mut prog = BpfProg {
        id: NEXT_PROG_ID.fetch_add(1, Ordering::Relaxed),
        name: String::from(name),
        prog_type,
        insns,
        verified: false,
        map_ids,
        used_helpers: Vec::new(),
    };

    verify(&prog)?;
    prog.verified = true;

    let id = prog.id;
    PROGRAMS.lock().push(prog);
    Ok(id)
}

/// Run a loaded BPF program with a context value.
pub fn run_prog(id: u32, ctx: u64) -> Result<u64, BpfError> {
    let progs = PROGRAMS.lock();
    let prog = progs
        .iter()
        .find(|p| p.id == id)
        .ok_or(BpfError::ProgNotFound)?;
    let mut vm = BpfVm::new();
    vm.execute(prog, ctx)
}

/// Unload a BPF program.
pub fn unload_prog(id: u32) -> Result<(), BpfError> {
    let mut progs = PROGRAMS.lock();
    let idx = progs
        .iter()
        .position(|p| p.id == id)
        .ok_or(BpfError::ProgNotFound)?;
    progs.remove(idx);
    Ok(())
}

/// List all loaded programs.
pub fn list_progs() -> Vec<(u32, String, BpfProgType, usize, bool)> {
    let progs = PROGRAMS.lock();
    progs
        .iter()
        .map(|p| (p.id, p.name.clone(), p.prog_type, p.insns.len(), p.verified))
        .collect()
}

/// Run an XDP program on a packet buffer. Returns the XDP action.
pub fn run_xdp(prog_id: u32, packet: &[u8]) -> Result<XdpAction, BpfError> {
    let result = run_prog(prog_id, packet.as_ptr() as u64)?;
    Ok(XdpAction::from_u64(result))
}

/// Lookup all per-CPU values in a map
pub fn map_lookup_all_cpus(map_id: u32, key: &[u8]) -> Vec<Option<Vec<u8>>> {
    let maps = MAPS.lock();
    maps.iter()
        .find(|m| m.id == map_id)
        .map(|m| m.lookup_all_cpus(key))
        .unwrap_or_default()
}

/// Record a stack trace in a stack trace map
pub fn map_record_stack(map_id: u32, ips: &[u64]) -> u32 {
    let mut maps = MAPS.lock();
    if let Some(map) = maps.iter_mut().find(|m| m.id == map_id) {
        map.record_stack_trace(ips)
    } else {
        0
    }
}

/// Get a stack trace from a stack trace map
pub fn map_get_stack(map_id: u32, stack_id: u32) -> Option<Vec<u64>> {
    let maps = MAPS.lock();
    maps.iter()
        .find(|m| m.id == map_id)
        .and_then(|m| m.get_stack_trace(stack_id).cloned())
}

/// Drain perf event output for a CPU
pub fn perf_drain(map_id: u32, cpu: usize) -> Vec<Vec<u8>> {
    let mut maps = MAPS.lock();
    if let Some(map) = maps.iter_mut().find(|m| m.id == map_id) {
        map.perf_drain(cpu)
    } else {
        Vec::new()
    }
}

/// Get map info
pub fn map_info(map_id: u32) -> Option<(String, BpfMapType, u32, u32, u32, usize)> {
    let maps = MAPS.lock();
    maps.iter().find(|m| m.id == map_id).map(|m| {
        (
            m.name.clone(),
            m.map_type,
            m.key_size,
            m.value_size,
            m.max_entries,
            m.len(),
        )
    })
}

/// Get program info
pub fn prog_info(prog_id: u32) -> Option<(String, BpfProgType, usize, bool, Vec<u32>)> {
    let progs = PROGRAMS.lock();
    progs.iter().find(|p| p.id == prog_id).map(|p| {
        (
            p.name.clone(),
            p.prog_type,
            p.insns.len(),
            p.verified,
            p.map_ids.clone(),
        )
    })
}

// ---------------------------------------------------------------------------
// TASK 3 — x86-64 JIT compiler for eBPF
// ---------------------------------------------------------------------------
//
// Translates a verified eBPF program into native x86-64 machine code.
//
// Register mapping (eBPF → x86-64):
//   R0  → rax    (return value)
//   R1  → rdi    (arg 0 / context)
//   R2  → rsi    (arg 1)
//   R3  → rdx    (arg 2)
//   R4  → rcx    (arg 3)
//   R5  → r8     (arg 4)
//   R6  → rbx    (callee-saved)
//   R7  → r13    (callee-saved)
//   R8  → r14    (callee-saved)
//   R9  → r15    (callee-saved)
//   R10 → rbp    (frame pointer — read-only in BPF)
//
// The generated function uses the System V AMD64 ABI:
//   - Prologue: push callee-saved regs, set up stack frame
//   - Epilogue: pop callee-saved regs, ret
//
// Memory layout of JIT output buffer:
//   A contiguous Vec<u8> of machine code bytes.  In a real kernel this would
//   be executable memory obtained from a dedicated executable allocator.
//   Here we store it as data and invoke it via a function pointer cast.
//
// Instruction count limit: JIT output is bounded; if the BPF program is larger
// than MAX_JIT_INSNS it falls back to the interpreter.

/// Maximum BPF instructions the JIT compiler will process per program
const MAX_JIT_INSNS: usize = 4096;

/// Maximum size of the x86 output buffer (generous upper bound: 64 bytes per BPF insn)
const MAX_JIT_BYTES: usize = MAX_JIT_INSNS * 64;

// ---------------------------------------------------------------------------
// x86-64 register encodings (ModRM / REX)
// ---------------------------------------------------------------------------

/// x86-64 register numbers in ModRM/SIB encoding
mod x86 {
    pub const RAX: u8 = 0;
    pub const RCX: u8 = 1;
    pub const RDX: u8 = 2;
    pub const RBX: u8 = 3;
    pub const RSP: u8 = 4;
    pub const RBP: u8 = 5;
    pub const RSI: u8 = 6;
    pub const RDI: u8 = 7;
    pub const R8: u8 = 8;
    pub const R9: u8 = 9;
    // R10-R15 continue
    pub const R13: u8 = 13;
    pub const R14: u8 = 14;
    pub const R15: u8 = 15;
}

/// Map a BPF register index (0-10) to an x86-64 register number.
/// Returns (reg_num, needs_rex_b) where needs_rex_b is true for R8-R15.
#[inline]
fn bpf_to_x86(r: usize) -> (u8, bool) {
    match r {
        0 => (x86::RAX, false),  // R0  → rax
        1 => (x86::RDI, false),  // R1  → rdi
        2 => (x86::RSI, false),  // R2  → rsi
        3 => (x86::RDX, false),  // R3  → rdx
        4 => (x86::RCX, false),  // R4  → rcx
        5 => (x86::R8, true),    // R5  → r8
        6 => (x86::RBX, false),  // R6  → rbx
        7 => (x86::R13, true),   // R7  → r13
        8 => (x86::R14, true),   // R8  → r14
        9 => (x86::R15, true),   // R9  → r15
        10 => (x86::RBP, false), // R10 → rbp
        _ => (x86::RAX, false),  // fallback (should not happen after verify)
    }
}

// ---------------------------------------------------------------------------
// Code emitter helpers
// ---------------------------------------------------------------------------

/// Emitter: appends bytes to a Vec<u8>
struct Emit<'a>(&'a mut Vec<u8>);

impl<'a> Emit<'a> {
    #[inline]
    fn byte(&mut self, b: u8) {
        self.0.push(b);
    }
    #[inline]
    fn word(&mut self, w: u16) {
        self.0.extend_from_slice(&w.to_le_bytes());
    }
    #[inline]
    fn dword(&mut self, d: u32) {
        self.0.extend_from_slice(&d.to_le_bytes());
    }
    #[inline]
    fn qword(&mut self, q: u64) {
        self.0.extend_from_slice(&q.to_le_bytes());
    }

    /// Emit REX.W prefix for 64-bit operations.
    /// `r` is the ModRM.reg field's extension bit (REX.R).
    /// `b` is the ModRM.rm  field's extension bit (REX.B).
    #[inline]
    fn rex_w(&mut self, r: bool, b: bool) {
        let mut rex: u8 = 0x48; // REX.W
        if r {
            rex |= 0x04;
        } // REX.R
        if b {
            rex |= 0x01;
        } // REX.B
        self.byte(rex);
    }

    /// Emit REX prefix (no W bit) for 32-bit ops that use r8-r15.
    #[inline]
    fn rex(&mut self, r: bool, b: bool) {
        let mut rex: u8 = 0x40;
        if r {
            rex |= 0x04;
        }
        if b {
            rex |= 0x01;
        }
        if rex != 0x40 {
            self.byte(rex);
        } // only emit if non-trivial
    }

    /// Emit ModRM byte: mod=0b11 (register direct), reg, rm.
    #[inline]
    fn modrm_rr(&mut self, reg: u8, rm: u8) {
        self.byte(0xC0 | ((reg & 7) << 3) | (rm & 7));
    }

    /// Emit ModRM byte: mod=0b10 (memory [rm + disp32]), reg, rm.
    #[inline]
    fn modrm_mem_disp32(&mut self, reg: u8, rm: u8) {
        self.byte(0x80 | ((reg & 7) << 3) | (rm & 7));
    }

    /// Emit ModRM byte: mod=0b01 (memory [rm + disp8]), reg, rm.
    #[inline]
    fn modrm_mem_disp8(&mut self, reg: u8, rm: u8) {
        self.byte(0x40 | ((reg & 7) << 3) | (rm & 7));
    }

    /// Emit: REX.W + opcode + ModRM(reg, rm) for 64-bit reg-reg op.
    #[inline]
    fn op_rr64(&mut self, opcode: u8, dst: usize, src: usize) {
        let (dr, db) = bpf_to_x86(dst);
        let (sr, sb) = bpf_to_x86(src);
        self.rex_w(sb, db); // REX.R encodes src, REX.B encodes dst
        self.byte(opcode);
        self.modrm_rr(sr & 7, dr & 7);
    }

    /// Emit: REX.W + opcode(imm) + ModRM(/ext, rm) + imm32
    #[inline]
    fn op_ri64(&mut self, opcode: u8, ext: u8, dst: usize, imm: i32) {
        let (dr, db) = bpf_to_x86(dst);
        self.rex_w(false, db);
        self.byte(opcode);
        self.modrm_rr(ext & 7, dr & 7);
        self.dword(imm as u32);
    }

    /// Emit: REX.W + 0xB8+rd (mov reg, imm64)
    #[inline]
    fn mov_ri64(&mut self, dst: usize, imm: u64) {
        let (dr, db) = bpf_to_x86(dst);
        let rex = 0x48u8 | (if db { 0x01 } else { 0 });
        self.byte(rex);
        self.byte(0xB8 | (dr & 7));
        self.qword(imm);
    }

    /// Emit: REX + 0x89 (mov qword [dst+off], src)   — 64-bit store
    #[inline]
    fn mov_mem_src64(&mut self, base: usize, off: i16, src: usize) {
        let (br, bb) = bpf_to_x86(base);
        let (sr, sb) = bpf_to_x86(src);
        self.rex_w(sb, bb);
        self.byte(0x89); // MOV r/m64, r64
        if off == 0 && (br & 7) != x86::RBP {
            self.byte(((sr & 7) << 3) | (br & 7)); // mod=00
        } else if off >= -128 && off <= 127 {
            self.modrm_mem_disp8(sr & 7, br & 7);
            self.byte(off as i8 as u8);
        } else {
            self.modrm_mem_disp32(sr & 7, br & 7);
            self.dword(off as i32 as u32);
        }
    }

    /// Emit: REX + 0x8B (mov dst, qword [src+off])  — 64-bit load
    #[inline]
    fn mov_dst_mem64(&mut self, dst: usize, base: usize, off: i16) {
        let (dr, db) = bpf_to_x86(dst);
        let (br, bb) = bpf_to_x86(base);
        self.rex_w(db, bb);
        self.byte(0x8B); // MOV r64, r/m64
        if off == 0 && (br & 7) != x86::RBP {
            self.byte(((dr & 7) << 3) | (br & 7)); // mod=00
        } else if off >= -128 && off <= 127 {
            self.modrm_mem_disp8(dr & 7, br & 7);
            self.byte(off as i8 as u8);
        } else {
            self.modrm_mem_disp32(dr & 7, br & 7);
            self.dword(off as i32 as u32);
        }
    }

    /// Current offset in the output buffer
    #[inline]
    fn offset(&self) -> usize {
        self.0.len()
    }

    /// Reserve 4 bytes for a 32-bit patch-up (returns the index to patch)
    #[inline]
    fn reserve_i32(&mut self) -> usize {
        let idx = self.0.len();
        self.dword(0);
        idx
    }

    /// Patch a previously reserved i32 at `idx` with the relative offset
    /// from the byte *after* the patch to `target`.
    #[inline]
    fn patch_rel32(&mut self, idx: usize, target: usize) {
        let after = idx + 4;
        let rel = (target as i64 - after as i64) as i32;
        let bytes = rel.to_le_bytes();
        self.0[idx..idx + 4].copy_from_slice(&bytes);
    }
}

// ---------------------------------------------------------------------------
// JIT compilation result
// ---------------------------------------------------------------------------

/// Compiled JIT program — a native x86-64 function in a Vec<u8>
pub struct JitProg {
    /// Raw machine code bytes
    pub code: Vec<u8>,
    /// Source eBPF program ID
    pub prog_id: u32,
}

impl JitProg {
    /// Execute the JIT-compiled program with a context pointer.
    ///
    /// # Safety
    /// The `code` Vec must contain valid x86-64 machine code that conforms
    /// to the System V AMD64 ABI calling convention.  The code must have been
    /// produced by `jit_compile` from a verified eBPF program.
    ///
    /// In a real kernel this memory would be marked executable via mprotect /
    /// a dedicated RWX mapping.  Here we cast a data pointer to a function
    /// pointer, which is architecturally equivalent when the memory region
    /// is executable.
    pub unsafe fn call(&self, ctx: u64) -> u64 {
        if self.code.is_empty() {
            return 0;
        }
        let fn_ptr: extern "C" fn(u64) -> u64 = core::mem::transmute(self.code.as_ptr());
        fn_ptr(ctx)
    }
}

// ---------------------------------------------------------------------------
// TASK 3 — jit_compile: BPF → x86-64 code generation
// ---------------------------------------------------------------------------

/// Compile a verified eBPF program to native x86-64 code.
///
/// Returns `Ok(JitProg)` with the emitted machine code, or an error if the
/// program is too large or contains an unsupported instruction.
///
/// The emitted function has signature: `extern "C" fn(ctx: u64) -> u64`
/// where ctx is passed in rdi (R1 in BPF convention).
pub fn jit_compile(prog: &BpfProg) -> Result<JitProg, BpfError> {
    if !prog.verified {
        return Err(BpfError::NotVerified);
    }
    let insns = &prog.insns;
    let len = insns.len();
    if len > MAX_JIT_INSNS {
        return Err(BpfError::VerificationFailed(alloc::format!(
            "program too large for JIT ({} > {})",
            len,
            MAX_JIT_INSNS
        )));
    }

    let mut code: Vec<u8> = Vec::with_capacity(len * 16);
    let mut e = Emit(&mut code);

    // ---- Prologue ----
    // push rbx; push r13; push r14; push r15; push rbp
    // sub rsp, 512       ; BPF stack (R10 frame pointer = rsp+512)
    // mov rbp, rsp       ; set BPF frame pointer (R10) to top of BPF stack
    // add rbp, 512
    // (R1/rdi already has ctx from caller)

    e.byte(0x53); // push rbx
    e.byte(0x41);
    e.byte(0x55); // push r13
    e.byte(0x41);
    e.byte(0x56); // push r14
    e.byte(0x41);
    e.byte(0x57); // push r15
    e.byte(0x55); // push rbp

    // sub rsp, 512
    e.byte(0x48);
    e.byte(0x81);
    e.byte(0xEC);
    e.dword(512u32);

    // lea rbp, [rsp + 512]  (BPF R10 = frame pointer at top of 512B BPF stack)
    e.byte(0x48);
    e.byte(0x8D);
    e.byte(0xAC);
    e.byte(0x24);
    e.dword(512u32);

    // xor rax, rax  (clear R0)
    e.byte(0x48);
    e.byte(0x31);
    e.byte(0xC0);

    // Map from BPF pc → x86 byte offset for jump patching
    // We make two passes: first to collect offsets, second to patch jumps.
    // For simplicity we do a single pass and collect relocation patches.

    /// A relocation: (patch_idx, target_bpf_pc) — patch the 4-byte field at
    /// `patch_idx` with a relative offset to the x86 byte at offset of
    /// `target_bpf_pc`.
    struct Reloc {
        patch_idx: usize,
        target_bpf_pc: usize,
    }

    let mut pc_offsets: Vec<usize> = Vec::with_capacity(len + 1);
    let mut relocs: Vec<Reloc> = Vec::new();

    // Drop the Emit borrow temporarily to do two-pass
    drop(e);

    // --- Pass 1: emit code and collect relocations ---
    for (pc, insn) in insns.iter().enumerate() {
        pc_offsets.push(code.len());
        let mut e = Emit(&mut code);
        let dst = insn.dst();
        let src = insn.src();
        let imm = insn.imm;
        let off = insn.off;

        match insn.op {
            // ---- ALU64: dst op= imm ----
            op::ADD_IMM => {
                // add dst, imm32
                let (dr, db) = bpf_to_x86(dst);
                e.rex_w(false, db);
                e.byte(0x81);
                e.modrm_rr(0, dr & 7);
                e.dword(imm as u32);
            }
            op::SUB_IMM => {
                // sub dst, imm32
                let (dr, db) = bpf_to_x86(dst);
                e.rex_w(false, db);
                e.byte(0x81);
                e.modrm_rr(5, dr & 7);
                e.dword(imm as u32);
            }
            op::MUL_IMM => {
                // imul dst, dst, imm32
                let (dr, db) = bpf_to_x86(dst);
                e.rex_w(db, db);
                e.byte(0x69);
                e.modrm_rr(dr & 7, dr & 7);
                e.dword(imm as u32);
            }
            op::DIV_IMM => {
                if imm == 0 {
                    return Err(BpfError::DivByZero);
                }
                // mov rcx, imm64; ... (simplified: use imm32 for most values)
                // We load imm into rcx, then div
                // mov rcx, sign_extend(imm)
                let (dr, db) = bpf_to_x86(dst);
                // mov rax, dst
                e.rex_w(false, db);
                e.byte(0x89);
                e.modrm_rr(dr & 7, x86::RAX);
                // mov rcx, imm64
                e.byte(0x48);
                e.byte(0xB9);
                e.qword(imm as i64 as u64);
                // xor rdx, rdx (clear high bits for div)
                e.byte(0x48);
                e.byte(0x31);
                e.byte(0xD2);
                // div rcx  (unsigned 64-bit: rdx:rax / rcx → rax)
                e.byte(0x48);
                e.byte(0xF7);
                e.byte(0xF1);
                // mov dst, rax
                e.rex_w(db, false);
                e.byte(0x89);
                e.modrm_rr(x86::RAX, dr & 7);
            }
            op::OR_IMM => {
                // or dst, imm32
                let (dr, db) = bpf_to_x86(dst);
                e.rex_w(false, db);
                e.byte(0x81);
                e.modrm_rr(1, dr & 7);
                e.dword(imm as u32);
            }
            op::AND_IMM => {
                // and dst, imm32
                let (dr, db) = bpf_to_x86(dst);
                e.rex_w(false, db);
                e.byte(0x81);
                e.modrm_rr(4, dr & 7);
                e.dword(imm as u32);
            }
            op::LSH_IMM => {
                // shl dst, imm8
                let (dr, db) = bpf_to_x86(dst);
                e.rex_w(false, db);
                e.byte(0xC1);
                e.modrm_rr(4, dr & 7);
                e.byte(imm as u8 & 63);
            }
            op::RSH_IMM => {
                // shr dst, imm8 (logical right shift)
                let (dr, db) = bpf_to_x86(dst);
                e.rex_w(false, db);
                e.byte(0xC1);
                e.modrm_rr(5, dr & 7);
                e.byte(imm as u8 & 63);
            }
            op::ARSH_IMM => {
                // sar dst, imm8 (arithmetic right shift)
                let (dr, db) = bpf_to_x86(dst);
                e.rex_w(false, db);
                e.byte(0xC1);
                e.modrm_rr(7, dr & 7);
                e.byte(imm as u8 & 63);
            }
            op::XOR_IMM => {
                // xor dst, imm32
                let (dr, db) = bpf_to_x86(dst);
                e.rex_w(false, db);
                e.byte(0x81);
                e.modrm_rr(6, dr & 7);
                e.dword(imm as u32);
            }
            op::MOV_IMM => {
                // mov dst, imm64  (sign-extend imm32 to 64 bits)
                e.mov_ri64(dst, imm as i64 as u64);
            }
            op::NEG => {
                // neg dst
                let (dr, db) = bpf_to_x86(dst);
                e.rex_w(false, db);
                e.byte(0xF7);
                e.modrm_rr(3, dr & 7);
            }
            op::MOD_IMM => {
                if imm == 0 {
                    return Err(BpfError::DivByZero);
                }
                // mod via div: rdx = rdx:rax mod rcx
                let (dr, db) = bpf_to_x86(dst);
                e.rex_w(false, db);
                e.byte(0x89);
                e.modrm_rr(dr & 7, x86::RAX); // mov rax, dst
                e.byte(0x48);
                e.byte(0xB9);
                e.qword(imm as i64 as u64); // mov rcx, imm
                e.byte(0x48);
                e.byte(0x31);
                e.byte(0xD2); // xor rdx, rdx
                e.byte(0x48);
                e.byte(0xF7);
                e.byte(0xF1); // div rcx
                e.rex_w(db, false);
                e.byte(0x89);
                e.modrm_rr(x86::RDX, dr & 7); // mov dst, rdx (remainder)
            }

            // ---- ALU64: dst op= src ----
            op::ADD_REG => {
                e.op_rr64(0x01, dst, src);
            } // add dst, src
            op::SUB_REG => {
                e.op_rr64(0x29, dst, src);
            } // sub dst, src
            op::MUL_REG => {
                // imul dst, src  (REX.W 0F AF /r)
                let (dr, db) = bpf_to_x86(dst);
                let (sr, sb) = bpf_to_x86(src);
                e.rex_w(db, sb);
                e.byte(0x0F);
                e.byte(0xAF);
                e.modrm_rr(dr & 7, sr & 7);
            }
            op::DIV_REG => {
                // Unsigned 64-bit divide: rax = dst; div src; dst = rax
                let (dr, db) = bpf_to_x86(dst);
                let (sr, sb) = bpf_to_x86(src);
                // Temporarily move dst to rax, divisor to rcx
                e.rex_w(false, db);
                e.byte(0x89);
                e.modrm_rr(dr & 7, x86::RAX);
                e.rex_w(sb, false);
                e.byte(0x89);
                e.modrm_rr(sr & 7, x86::RCX);
                e.byte(0x48);
                e.byte(0x31);
                e.byte(0xD2); // xor rdx, rdx
                e.byte(0x48);
                e.byte(0xF7);
                e.byte(0xF1); // div rcx
                e.rex_w(db, false);
                e.byte(0x89);
                e.modrm_rr(x86::RAX, dr & 7); // mov dst, rax
            }
            op::OR_REG => {
                e.op_rr64(0x09, dst, src);
            } // or dst, src
            op::AND_REG => {
                e.op_rr64(0x21, dst, src);
            } // and dst, src
            op::XOR_REG => {
                e.op_rr64(0x31, dst, src);
            } // xor dst, src
            op::MOV_REG => {
                // mov dst, src  (REX.W 0x8B /r)
                let (dr, db) = bpf_to_x86(dst);
                let (sr, sb) = bpf_to_x86(src);
                e.rex_w(db, sb);
                e.byte(0x89);
                e.modrm_rr(sr & 7, dr & 7);
            }
            op::LSH_REG => {
                // shl dst, cl  — move src to cl first
                let (dr, db) = bpf_to_x86(dst);
                let (sr, sb) = bpf_to_x86(src);
                // mov rcx, src
                e.rex_w(false, sb);
                e.byte(0x89);
                e.modrm_rr(sr & 7, x86::RCX);
                // shl dst, cl
                e.rex_w(false, db);
                e.byte(0xD3);
                e.modrm_rr(4, dr & 7);
            }
            op::RSH_REG => {
                // shr dst, cl
                let (dr, db) = bpf_to_x86(dst);
                let (sr, sb) = bpf_to_x86(src);
                e.rex_w(false, sb);
                e.byte(0x89);
                e.modrm_rr(sr & 7, x86::RCX);
                e.rex_w(false, db);
                e.byte(0xD3);
                e.modrm_rr(5, dr & 7);
            }
            op::ARSH_REG => {
                // sar dst, cl
                let (dr, db) = bpf_to_x86(dst);
                let (sr, sb) = bpf_to_x86(src);
                e.rex_w(false, sb);
                e.byte(0x89);
                e.modrm_rr(sr & 7, x86::RCX);
                e.rex_w(false, db);
                e.byte(0xD3);
                e.modrm_rr(7, dr & 7);
            }
            op::MOD_REG => {
                let (dr, db) = bpf_to_x86(dst);
                let (sr, sb) = bpf_to_x86(src);
                e.rex_w(false, db);
                e.byte(0x89);
                e.modrm_rr(dr & 7, x86::RAX);
                e.rex_w(false, sb);
                e.byte(0x89);
                e.modrm_rr(sr & 7, x86::RCX);
                e.byte(0x48);
                e.byte(0x31);
                e.byte(0xD2);
                e.byte(0x48);
                e.byte(0xF7);
                e.byte(0xF1);
                e.rex_w(db, false);
                e.byte(0x89);
                e.modrm_rr(x86::RDX, dr & 7);
            }

            // ---- ALU32 ops: zero-extend result to 64 bits ----
            op::ADD32_IMM => {
                // add dst32, imm32  (no REX.W → 32-bit, automatically zero-extended)
                let (dr, db) = bpf_to_x86(dst);
                e.rex(false, db);
                e.byte(0x81);
                e.modrm_rr(0, dr & 7);
                e.dword(imm as u32);
                // zero-extend: movzx dst64, dst32 is implicit for 32-bit writes
            }
            op::SUB32_IMM => {
                let (dr, db) = bpf_to_x86(dst);
                e.rex(false, db);
                e.byte(0x81);
                e.modrm_rr(5, dr & 7);
                e.dword(imm as u32);
            }
            op::XOR32_IMM => {
                let (dr, db) = bpf_to_x86(dst);
                e.rex(false, db);
                e.byte(0x81);
                e.modrm_rr(6, dr & 7);
                e.dword(imm as u32);
            }
            op::AND32_IMM => {
                let (dr, db) = bpf_to_x86(dst);
                e.rex(false, db);
                e.byte(0x81);
                e.modrm_rr(4, dr & 7);
                e.dword(imm as u32);
            }
            op::OR32_IMM => {
                let (dr, db) = bpf_to_x86(dst);
                e.rex(false, db);
                e.byte(0x81);
                e.modrm_rr(1, dr & 7);
                e.dword(imm as u32);
            }
            op::MOV32_IMM => {
                // mov dst32, imm32 (zero-extends automatically)
                let (dr, db) = bpf_to_x86(dst);
                e.rex(false, db);
                e.byte(0xB8 | (dr & 7));
                e.dword(imm as u32);
            }
            op::ADD32_REG => {
                // add dst32, src32
                let (dr, db) = bpf_to_x86(dst);
                let (sr, sb) = bpf_to_x86(src);
                e.rex(sb, db);
                e.byte(0x01);
                e.modrm_rr(sr & 7, dr & 7);
            }
            op::SUB32_REG => {
                let (dr, db) = bpf_to_x86(dst);
                let (sr, sb) = bpf_to_x86(src);
                e.rex(sb, db);
                e.byte(0x29);
                e.modrm_rr(sr & 7, dr & 7);
            }
            op::MOV32_REG => {
                // mov dst32, src32
                let (dr, db) = bpf_to_x86(dst);
                let (sr, sb) = bpf_to_x86(src);
                e.rex(db, sb);
                e.byte(0x89);
                e.modrm_rr(sr & 7, dr & 7);
            }
            op::XOR32_REG => {
                let (dr, db) = bpf_to_x86(dst);
                let (sr, sb) = bpf_to_x86(src);
                e.rex(sb, db);
                e.byte(0x31);
                e.modrm_rr(sr & 7, dr & 7);
            }

            // ---- Memory: 64-bit load (LDX_DW) ----
            op::LDX_DW => {
                // mov dst, qword [src + off]
                e.mov_dst_mem64(dst, src, off);
            }
            op::LDX_W => {
                // mov dst32, dword [src + off]  (zero-extends to 64 bits)
                let (dr, db) = bpf_to_x86(dst);
                let (br, bb) = bpf_to_x86(src);
                e.rex(db, bb);
                e.byte(0x8B); // MOV r32, r/m32
                if off == 0 && (br & 7) != x86::RBP {
                    e.byte(((dr & 7) << 3) | (br & 7));
                } else if off >= -128 && off <= 127 {
                    e.modrm_mem_disp8(dr & 7, br & 7);
                    e.byte(off as i8 as u8);
                } else {
                    e.modrm_mem_disp32(dr & 7, br & 7);
                    e.dword(off as i32 as u32);
                }
            }
            op::LDX_H => {
                // movzx dst, word [src + off]
                let (dr, db) = bpf_to_x86(dst);
                let (br, bb) = bpf_to_x86(src);
                e.rex_w(db, bb);
                e.byte(0x0F);
                e.byte(0xB7); // MOVZX r64, r/m16
                if off == 0 && (br & 7) != x86::RBP {
                    e.byte(((dr & 7) << 3) | (br & 7));
                } else if off >= -128 && off <= 127 {
                    e.modrm_mem_disp8(dr & 7, br & 7);
                    e.byte(off as i8 as u8);
                } else {
                    e.modrm_mem_disp32(dr & 7, br & 7);
                    e.dword(off as i32 as u32);
                }
            }
            op::LDX_B => {
                // movzx dst, byte [src + off]
                let (dr, db) = bpf_to_x86(dst);
                let (br, bb) = bpf_to_x86(src);
                e.rex_w(db, bb);
                e.byte(0x0F);
                e.byte(0xB6); // MOVZX r64, r/m8
                if off == 0 && (br & 7) != x86::RBP {
                    e.byte(((dr & 7) << 3) | (br & 7));
                } else if off >= -128 && off <= 127 {
                    e.modrm_mem_disp8(dr & 7, br & 7);
                    e.byte(off as i8 as u8);
                } else {
                    e.modrm_mem_disp32(dr & 7, br & 7);
                    e.dword(off as i32 as u32);
                }
            }

            // ---- Memory: 64-bit store (STX_DW) ----
            op::STX_DW => {
                // mov qword [dst + off], src
                e.mov_mem_src64(dst, off, src);
            }
            op::STX_W => {
                // mov dword [dst + off], src32
                let (dr, db) = bpf_to_x86(dst);
                let (sr, sb) = bpf_to_x86(src);
                e.rex(sb, db);
                e.byte(0x89);
                if off == 0 && (dr & 7) != x86::RBP {
                    e.byte(((sr & 7) << 3) | (dr & 7));
                } else if off >= -128 && off <= 127 {
                    e.modrm_mem_disp8(sr & 7, dr & 7);
                    e.byte(off as i8 as u8);
                } else {
                    e.modrm_mem_disp32(sr & 7, dr & 7);
                    e.dword(off as i32 as u32);
                }
            }
            op::STX_H => {
                // mov word [dst + off], src16
                let (dr, db) = bpf_to_x86(dst);
                let (sr, sb) = bpf_to_x86(src);
                e.byte(0x66);
                e.rex(sb, db);
                e.byte(0x89);
                if off == 0 && (dr & 7) != x86::RBP {
                    e.byte(((sr & 7) << 3) | (dr & 7));
                } else if off >= -128 && off <= 127 {
                    e.modrm_mem_disp8(sr & 7, dr & 7);
                    e.byte(off as i8 as u8);
                } else {
                    e.modrm_mem_disp32(sr & 7, dr & 7);
                    e.dword(off as i32 as u32);
                }
            }
            op::STX_B => {
                // mov byte [dst + off], src8
                let (dr, db) = bpf_to_x86(dst);
                let (sr, sb) = bpf_to_x86(src);
                // Need REX if src or dst is r8-r15 to access low byte
                if sb || db || sr >= 4 || dr >= 4 {
                    let mut rex: u8 = 0x40;
                    if sb {
                        rex |= 0x04;
                    }
                    if db {
                        rex |= 0x01;
                    }
                    e.byte(rex);
                }
                e.byte(0x88);
                if off == 0 && (dr & 7) != x86::RBP {
                    e.byte(((sr & 7) << 3) | (dr & 7));
                } else if off >= -128 && off <= 127 {
                    e.modrm_mem_disp8(sr & 7, dr & 7);
                    e.byte(off as i8 as u8);
                } else {
                    e.modrm_mem_disp32(sr & 7, dr & 7);
                    e.dword(off as i32 as u32);
                }
            }

            // ---- Immediate stores: ST_* ----
            op::ST_DW => {
                // mov qword [dst + off], sign_extend(imm32)
                let (dr, db) = bpf_to_x86(dst);
                e.rex_w(false, db);
                e.byte(0xC7);
                if off == 0 && (dr & 7) != x86::RBP {
                    e.byte(dr & 7);
                } else if off >= -128 && off <= 127 {
                    e.modrm_mem_disp8(0, dr & 7);
                    e.byte(off as i8 as u8);
                } else {
                    e.modrm_mem_disp32(0, dr & 7);
                    e.dword(off as i32 as u32);
                }
                e.dword(imm as u32);
            }
            op::ST_W => {
                let (dr, db) = bpf_to_x86(dst);
                e.rex(false, db);
                e.byte(0xC7);
                if off == 0 && (dr & 7) != x86::RBP {
                    e.byte(dr & 7);
                } else if off >= -128 && off <= 127 {
                    e.modrm_mem_disp8(0, dr & 7);
                    e.byte(off as i8 as u8);
                } else {
                    e.modrm_mem_disp32(0, dr & 7);
                    e.dword(off as i32 as u32);
                }
                e.dword(imm as u32);
            }
            op::ST_H => {
                let (dr, db) = bpf_to_x86(dst);
                e.byte(0x66);
                e.rex(false, db);
                e.byte(0xC7);
                if off == 0 && (dr & 7) != x86::RBP {
                    e.byte(dr & 7);
                } else if off >= -128 && off <= 127 {
                    e.modrm_mem_disp8(0, dr & 7);
                    e.byte(off as i8 as u8);
                } else {
                    e.modrm_mem_disp32(0, dr & 7);
                    e.dword(off as i32 as u32);
                }
                e.word(imm as u16);
            }
            op::ST_B => {
                let (dr, db) = bpf_to_x86(dst);
                e.rex(false, db);
                e.byte(0xC6);
                if off == 0 && (dr & 7) != x86::RBP {
                    e.byte(dr & 7);
                } else if off >= -128 && off <= 127 {
                    e.modrm_mem_disp8(0, dr & 7);
                    e.byte(off as i8 as u8);
                } else {
                    e.modrm_mem_disp32(0, dr & 7);
                    e.dword(off as i32 as u32);
                }
                e.byte(imm as u8);
            }

            // ---- 64-bit immediate load (LD_DW, 2-instruction encoding) ----
            op::LD_DW => {
                if pc + 1 >= len {
                    return Err(BpfError::OutOfBounds);
                }
                let lo = insn.imm as u32 as u64;
                let hi = insns[pc + 1].imm as u32 as u64;
                let val = lo | (hi << 32);
                e.mov_ri64(dst, val);
                // The second instruction slot is a NOP in JIT (we just skip it
                // by advancing pc_offsets ahead — handled after this match)
            }

            // ---- BPF_CALL: indirect call via helper table ----
            op::CALL => {
                // Save caller-saved regs if needed, call helper, restore.
                // We use a simplified approach: call a Rust trampoline that
                // dispatches to call_helper(id, regs).
                //
                // Helper calling convention:
                //   - BPF args in R1-R5 (rdi, rsi, rdx, rcx, r8)
                //   - We push rdi..r8 to an on-stack BpfRegs array, pass pointer
                //   - call_helper_jit_trampoline(helper_id, regs_ptr) → u64 in rax
                //
                // For JIT we inline a simplified version: push R1-R5 to the
                // BPF stack, call the Rust `call_helper` shim, get result in rax.
                //
                // Architecture note: this is a near-call with an absolute address
                // moved into a scratch register (r11).  r11 is caller-saved and
                // not used by BPF registers.
                let helper_id = imm as u32;
                let trampoline_addr = jit_call_helper_shim as *const () as u64;

                // mov r11, trampoline_addr (imm64)
                e.byte(0x49);
                e.byte(0xBB);
                e.qword(trampoline_addr);

                // We need to pass (helper_id, &regs) to the shim.
                // R1-R5 are already in rdi, rsi, rdx, rcx, r8.
                // We push them to the stack as a [u64; 11] array.
                // For simplicity: push r8, rcx, rdx, rsi, rdi, then pad 6 slots.
                //
                // sub rsp, 88 (11 * 8 bytes for BPF reg snapshot)
                e.byte(0x48);
                e.byte(0x83);
                e.byte(0xEC);
                e.byte(88u8);
                // Store BPF registers into [rsp + n*8]:
                // rsp+0=R0(rax), rsp+8=R1(rdi), rsp+16=R2(rsi), ...
                e.byte(0x48);
                e.byte(0x89);
                e.byte(0x04);
                e.byte(0x24); // [rsp+0] = rax
                e.byte(0x48);
                e.byte(0x89);
                e.byte(0x7C);
                e.byte(0x24);
                e.byte(8u8); // [rsp+8]  = rdi
                e.byte(0x48);
                e.byte(0x89);
                e.byte(0x74);
                e.byte(0x24);
                e.byte(16u8); // [rsp+16] = rsi
                e.byte(0x48);
                e.byte(0x89);
                e.byte(0x54);
                e.byte(0x24);
                e.byte(24u8); // [rsp+24] = rdx
                e.byte(0x48);
                e.byte(0x89);
                e.byte(0x4C);
                e.byte(0x24);
                e.byte(32u8); // [rsp+32] = rcx
                e.byte(0x4C);
                e.byte(0x89);
                e.byte(0x44);
                e.byte(0x24);
                e.byte(40u8); // [rsp+40] = r8

                // mov edi, helper_id (first arg to shim = helper_id: u32)
                e.byte(0xBF);
                e.dword(helper_id);
                // lea rsi, [rsp]  (second arg = pointer to reg snapshot)
                e.byte(0x48);
                e.byte(0x8D);
                e.byte(0x34);
                e.byte(0x24);
                // call r11
                e.byte(0x41);
                e.byte(0xFF);
                e.byte(0xD3);
                // Restore rsp
                e.byte(0x48);
                e.byte(0x83);
                e.byte(0xC4);
                e.byte(88u8);
                // Result is in rax (BPF R0)
            }

            // ---- BPF_EXIT: return R0 ----
            op::EXIT => {
                // mov rax, R0 (already rax if dst=0; but R0=rax always)
                // Epilogue
                // add rsp, 512   ; deallocate BPF stack
                e.byte(0x48);
                e.byte(0x81);
                e.byte(0xC4);
                e.dword(512u32);
                // pop rbp; pop r15; pop r14; pop r13; pop rbx
                e.byte(0x5D);
                e.byte(0x41);
                e.byte(0x5F);
                e.byte(0x41);
                e.byte(0x5E);
                e.byte(0x41);
                e.byte(0x5D);
                e.byte(0x5B);
                e.byte(0xC3); // ret
            }

            // ---- Unconditional jump ----
            op::JA => {
                // jmp rel32  (0xE9 + 32-bit relative offset)
                let cur_off = e.offset();
                e.byte(0xE9);
                let patch_idx = e.reserve_i32();
                let target_bpf_pc = (pc as i64 + off as i64 + 1) as usize;
                drop(e); // release borrow before pushing to relocs
                relocs.push(Reloc {
                    patch_idx,
                    target_bpf_pc,
                });
                continue;
            }

            // ---- Conditional branches ----
            op::JEQ_IMM
            | op::JEQ_REG
            | op::JNE_IMM
            | op::JNE_REG
            | op::JGT_IMM
            | op::JGT_REG
            | op::JGE_IMM
            | op::JGE_REG
            | op::JLT_IMM
            | op::JLT_REG
            | op::JLE_IMM
            | op::JLE_REG
            | op::JSGT_IMM
            | op::JSGT_REG
            | op::JSGE_IMM
            | op::JSGE_REG
            | op::JSLT_IMM
            | op::JSLT_REG
            | op::JSLE_IMM
            | op::JSLE_REG
            | op::JSET_IMM
            | op::JSET_REG => {
                let is_imm = (insn.op & 0x08) == 0;
                let (dr, db) = bpf_to_x86(dst);
                let (sr, sb) = bpf_to_x86(src);

                // Emit CMP / TEST
                if is_imm {
                    e.rex_w(false, db);
                    if insn.op == op::JSET_IMM {
                        // test dst, imm32
                        e.byte(0xF7);
                        e.modrm_rr(0, dr & 7);
                        e.dword(imm as u32);
                    } else {
                        // cmp dst, imm32
                        e.byte(0x81);
                        e.modrm_rr(7, dr & 7);
                        e.dword(imm as u32);
                    }
                } else {
                    e.rex_w(sb, db);
                    if insn.op == op::JSET_REG {
                        // test dst, src
                        e.byte(0x85);
                        e.modrm_rr(sr & 7, dr & 7);
                    } else {
                        // cmp dst, src  (opcode 0x3B: cmp r64, r/m64)
                        e.byte(0x3B);
                        e.modrm_rr(dr & 7, sr & 7);
                    }
                }

                // Choose Jcc opcode (near, rel32): 0x0F 0x8x
                let jcc_op: u8 = match insn.op {
                    op::JEQ_IMM | op::JEQ_REG => 0x84,   // JE  / JZ
                    op::JNE_IMM | op::JNE_REG => 0x85,   // JNE / JNZ
                    op::JGT_IMM | op::JGT_REG => 0x87,   // JA  (unsigned >)
                    op::JGE_IMM | op::JGE_REG => 0x83,   // JAE (unsigned >=)
                    op::JLT_IMM | op::JLT_REG => 0x82,   // JB  (unsigned <)
                    op::JLE_IMM | op::JLE_REG => 0x86,   // JBE (unsigned <=)
                    op::JSGT_IMM | op::JSGT_REG => 0x8F, // JG  (signed >)
                    op::JSGE_IMM | op::JSGE_REG => 0x8D, // JGE (signed >=)
                    op::JSLT_IMM | op::JSLT_REG => 0x8C, // JL  (signed <)
                    op::JSLE_IMM | op::JSLE_REG => 0x8E, // JLE (signed <=)
                    op::JSET_IMM | op::JSET_REG => 0x85, // JNZ (bit set → nonzero)
                    _ => 0x85,
                };

                e.byte(0x0F);
                e.byte(jcc_op);
                let patch_idx = e.reserve_i32();
                let target_bpf_pc = (pc as i64 + off as i64 + 1) as usize;
                drop(e);
                relocs.push(Reloc {
                    patch_idx,
                    target_bpf_pc,
                });
                continue;
            }

            // ---- Endian swaps ----
            op::LE => {
                // On a little-endian host, LE swap is a mask/truncate
                let (dr, db) = bpf_to_x86(dst);
                match imm {
                    16 => {
                        // movzx dst, dst16
                        e.rex_w(db, db);
                        e.byte(0x0F);
                        e.byte(0xB7);
                        e.modrm_rr(dr & 7, dr & 7);
                    }
                    32 => {
                        // mov dst32, dst32  (zero-extends)
                        e.rex(db, db);
                        e.byte(0x89);
                        e.modrm_rr(dr & 7, dr & 7);
                    }
                    64 => {} // no-op on LE host
                    _ => {}
                }
            }
            op::BE => {
                // bswap
                let (dr, db) = bpf_to_x86(dst);
                match imm {
                    16 => {
                        // xchg al, ah via rol16 by 8 (easier): ror16 r16, 8
                        e.byte(0x66);
                        e.rex(false, db);
                        e.byte(0xC1);
                        e.modrm_rr(1, dr & 7);
                        e.byte(8u8);
                        // zero-extend to 64: movzx
                        e.rex_w(db, db);
                        e.byte(0x0F);
                        e.byte(0xB7);
                        e.modrm_rr(dr & 7, dr & 7);
                    }
                    32 => {
                        // bswap dst32
                        e.rex(false, db);
                        e.byte(0x0F);
                        e.byte(0xC8 | (dr & 7));
                    }
                    64 => {
                        // bswap dst64
                        e.rex_w(false, db);
                        e.byte(0x0F);
                        e.byte(0xC8 | (dr & 7));
                    }
                    _ => {}
                }
            }

            // All other / unsupported opcodes
            _ => {
                // Emit a trap (ud2) rather than silently skipping
                e.byte(0x0F);
                e.byte(0x0B); // UD2 — causes #UD
            }
        }

        // For LD_DW, skip the second instruction slot in pc_offsets
        if insn.op == op::LD_DW {
            pc_offsets.push(code.len()); // dummy entry for pc+1
                                         // Note: the outer `for` loop will advance pc past this naturally
        }
    }

    // Mark the end of the last instruction
    pc_offsets.push(code.len());

    // --- Pass 2: patch relocations ---
    for reloc in &relocs {
        let target_x86 = if reloc.target_bpf_pc < pc_offsets.len() {
            pc_offsets[reloc.target_bpf_pc]
        } else {
            // Target is past end — patch to just before ret (the EXIT epilogue
            // should already be emitted at pc_offsets.last)
            *pc_offsets.last().unwrap_or(&0)
        };

        let after_patch = reloc.patch_idx + 4;
        let rel = (target_x86 as i64 - after_patch as i64) as i32;
        code[reloc.patch_idx..reloc.patch_idx + 4].copy_from_slice(&rel.to_le_bytes());
    }

    crate::serial_println!(
        "  [ebpf] JIT compiled prog_id={} insns={} → {} x86 bytes",
        prog.id,
        insns.len(),
        code.len()
    );

    Ok(JitProg {
        code,
        prog_id: prog.id,
    })
}

/// JIT helper shim: wraps `call_helper` with a C ABI compatible signature.
///
/// `id`      — BPF helper function ID (passed in edi by the JIT)
/// `regs_ptr`— pointer to a [u64; 11] snapshot of BPF registers on the JIT stack
///
/// Returns the helper result in rax.
///
/// # Safety
/// `regs_ptr` must point to a valid [u64; 11] array with at least 6 valid entries.
extern "C" fn jit_call_helper_shim(id: u32, regs_ptr: *const u64) -> u64 {
    let regs: [u64; 11] = unsafe {
        let slice = core::slice::from_raw_parts(regs_ptr, 11);
        [
            slice[0], slice[1], slice[2], slice[3], slice[4], slice[5], 0, 0, 0, 0, 0,
        ]
    };
    call_helper(id, &regs)
}

/// Public API: compile and run a loaded BPF program using the JIT.
/// Falls back to interpreter if compilation fails.
pub fn run_prog_jit(id: u32, ctx: u64) -> Result<u64, BpfError> {
    let progs = PROGRAMS.lock();
    let prog = progs
        .iter()
        .find(|p| p.id == id)
        .ok_or(BpfError::ProgNotFound)?;

    match jit_compile(prog) {
        Ok(jit_prog) => {
            drop(progs);
            // SAFETY: JIT output is generated from a verified BPF program
            // by our own compiler; it conforms to the System V ABI.
            // In a production kernel this memory would be mapped executable.
            unsafe { Ok(jit_prog.call(ctx)) }
        }
        Err(_) => {
            // Fall back to interpreter
            let mut vm = BpfVm::new();
            vm.execute(prog, ctx)
        }
    }
}

pub fn init() {
    crate::serial_println!("  [ebpf] eBPF virtual machine initialized (ALU64/ALU32, verifier, {} map types, helpers, JIT)",
        8); // HashMap, Array, RingBuffer, PerCpuArray, PerCpuHash, LruHash, StackTrace, PerfEventArray
}
