/// Core dump generation for Genesis
///
/// Generates ELF core files from crashed or signaled processes.
/// Captures full register state, memory region mappings, signal
/// information, and auxiliary data for post-mortem debugging.
///
/// Output format follows the ELF core specification with PT_NOTE
/// segments for register state, signal info, and process status.
///
/// Inspired by: Linux core(5), ELF spec, FreeBSD coredump.
/// All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ── ELF constants ────────────────────────────────────────────────────

/// ELF magic: 0x7F 'E' 'L' 'F'
const ELF_MAGIC: [u8; 4] = [0x7F, 0x45, 0x4C, 0x46];
/// 64-bit ELF
const ELFCLASS64: u8 = 2;
/// Little-endian
const ELFDATA2LSB: u8 = 1;
/// Current ELF version
const EV_CURRENT: u8 = 1;
/// System V ABI
const ELFOSABI_NONE: u8 = 0;
/// Core file type
const ET_CORE: u16 = 4;
/// x86_64 architecture
const EM_X86_64: u16 = 62;
/// PT_NOTE segment type
const PT_NOTE: u32 = 4;
/// PT_LOAD segment type
const PT_LOAD: u32 = 1;

/// Note types for core files
const NT_PRSTATUS: u32 = 1;
const NT_PRPSINFO: u32 = 3;
const NT_AUXV: u32 = 6;
const NT_FILE: u32 = 0x46494C45; // "FILE"

// ── ELF header structures ────────────────────────────────────────────

/// ELF64 file header (64 bytes)
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Elf64Header {
    pub e_ident: [u8; 16],
    pub e_type: u16,
    pub e_machine: u16,
    pub e_version: u32,
    pub e_entry: u64,
    pub e_phoff: u64,
    pub e_shoff: u64,
    pub e_flags: u32,
    pub e_ehsize: u16,
    pub e_phentsize: u16,
    pub e_phnum: u16,
    pub e_shentsize: u16,
    pub e_shnum: u16,
    pub e_shstrndx: u16,
}

/// ELF64 program header (56 bytes)
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Elf64Phdr {
    pub p_type: u32,
    pub p_flags: u32,
    pub p_offset: u64,
    pub p_vaddr: u64,
    pub p_paddr: u64,
    pub p_filesz: u64,
    pub p_memsz: u64,
    pub p_align: u64,
}

// ── Register state snapshot ──────────────────────────────────────────

/// Complete register state at time of crash
#[derive(Debug, Clone)]
pub struct RegisterState {
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub rsp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rip: u64,
    pub rflags: u64,
    pub cs: u64,
    pub ss: u64,
    pub cr2: u64,
}

impl RegisterState {
    /// Capture from a CpuContext
    pub fn from_context(ctx: &super::context::CpuContext) -> Self {
        RegisterState {
            rax: ctx.rax,
            rbx: ctx.rbx,
            rcx: ctx.rcx,
            rdx: ctx.rdx,
            rsi: ctx.rsi,
            rdi: ctx.rdi,
            rbp: ctx.rbp,
            rsp: ctx.rsp,
            r8: ctx.r8,
            r9: ctx.r9,
            r10: ctx.r10,
            r11: ctx.r11,
            r12: ctx.r12,
            r13: ctx.r13,
            r14: ctx.r14,
            r15: ctx.r15,
            rip: ctx.rip,
            rflags: ctx.rflags,
            cs: ctx.cs,
            ss: ctx.ss,
            cr2: 0,
        }
    }

    /// Serialize registers to bytes (little-endian)
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(21 * 8);
        for &val in &[
            self.rax,
            self.rbx,
            self.rcx,
            self.rdx,
            self.rsi,
            self.rdi,
            self.rbp,
            self.rsp,
            self.r8,
            self.r9,
            self.r10,
            self.r11,
            self.r12,
            self.r13,
            self.r14,
            self.r15,
            self.rip,
            self.rflags,
            self.cs,
            self.ss,
            self.cr2,
        ] {
            buf.extend_from_slice(&val.to_le_bytes());
        }
        buf
    }
}

// ── Memory region descriptor ─────────────────────────────────────────

/// Describes a memory region to include in the core dump
#[derive(Debug, Clone)]
pub struct MemoryRegion {
    /// Virtual start address
    pub vaddr: usize,
    /// Size in bytes
    pub size: usize,
    /// Permission flags (PF_R=4, PF_W=2, PF_X=1)
    pub flags: u32,
    /// Region label
    pub label: String,
}

// ── Signal info at crash time ────────────────────────────────────────

/// Signal that caused the core dump
#[derive(Debug, Clone, Copy)]
pub struct CrashSignalInfo {
    /// Signal number (e.g., SIGSEGV=11)
    pub signo: u8,
    /// Signal code (sub-reason)
    pub code: i32,
    /// Faulting address (for SIGSEGV, SIGBUS)
    pub fault_addr: usize,
}

// ── Core dump record ─────────────────────────────────────────────────

/// A complete core dump ready to be serialized
#[derive(Debug, Clone)]
pub struct CoreDump {
    /// PID of the crashed process
    pub pid: u32,
    /// Process name
    pub name: String,
    /// Signal that caused the dump
    pub signal_info: CrashSignalInfo,
    /// Register state at crash
    pub registers: RegisterState,
    /// Memory regions included
    pub regions: Vec<MemoryRegion>,
    /// Timestamp (ticks since boot)
    pub timestamp: u64,
    /// Parent PID
    pub parent_pid: u32,
    /// User ID
    pub uid: u32,
    /// Group ID
    pub gid: u32,
}

impl CoreDump {
    /// Create a new core dump record
    pub fn new(
        pid: u32,
        name: &str,
        signal_info: CrashSignalInfo,
        registers: RegisterState,
    ) -> Self {
        CoreDump {
            pid,
            name: String::from(name),
            signal_info,
            registers,
            regions: Vec::new(),
            timestamp: 0,
            parent_pid: 0,
            uid: 0,
            gid: 0,
        }
    }

    /// Add a memory region to the core dump
    pub fn add_region(&mut self, vaddr: usize, size: usize, flags: u32, label: &str) {
        self.regions.push(MemoryRegion {
            vaddr,
            size,
            flags,
            label: String::from(label),
        });
    }

    /// Build the NOTE segment bytes (prstatus + prpsinfo)
    fn build_note_segment(&self) -> Vec<u8> {
        let mut notes = Vec::new();

        // NT_PRSTATUS note: signal + registers
        let name = b"CORE\0\0\0\0"; // 8 bytes, padded to alignment
        let reg_bytes = self.registers.to_bytes();
        // Signal number as 4-byte prefix to desc
        let mut desc = Vec::new();
        desc.extend_from_slice(&(self.signal_info.signo as u32).to_le_bytes());
        desc.extend_from_slice(&self.signal_info.code.to_le_bytes());
        desc.extend_from_slice(&(self.signal_info.fault_addr as u64).to_le_bytes());
        desc.extend_from_slice(&self.pid.to_le_bytes());
        desc.extend_from_slice(&self.parent_pid.to_le_bytes());
        desc.extend_from_slice(&reg_bytes);

        // Note header: namesz, descsz, type
        notes.extend_from_slice(&(5u32).to_le_bytes()); // namesz = "CORE\0" = 5
        notes.extend_from_slice(&(desc.len() as u32).to_le_bytes());
        notes.extend_from_slice(&NT_PRSTATUS.to_le_bytes());
        notes.extend_from_slice(&name[..8]); // name + padding to 8 bytes
        notes.extend_from_slice(&desc);
        // Pad to 4-byte alignment
        while notes.len() % 4 != 0 {
            notes.push(0);
        }

        // NT_PRPSINFO note: process name + state
        let mut psinfo = vec![0u8; 136]; // Minimal prpsinfo structure
                                         // Copy process name (up to 16 bytes)
        let name_bytes = self.name.as_bytes();
        let copy_len = core::cmp::min(name_bytes.len(), 16);
        psinfo[40..40 + copy_len].copy_from_slice(&name_bytes[..copy_len]);
        // UID and GID
        psinfo[24..28].copy_from_slice(&self.uid.to_le_bytes());
        psinfo[28..32].copy_from_slice(&self.gid.to_le_bytes());
        // PID
        psinfo[32..36].copy_from_slice(&self.pid.to_le_bytes());

        notes.extend_from_slice(&(5u32).to_le_bytes()); // namesz
        notes.extend_from_slice(&(psinfo.len() as u32).to_le_bytes());
        notes.extend_from_slice(&NT_PRPSINFO.to_le_bytes());
        notes.extend_from_slice(&name[..8]);
        notes.extend_from_slice(&psinfo);
        while notes.len() % 4 != 0 {
            notes.push(0);
        }

        notes
    }

    /// Generate the full ELF core file as bytes
    pub fn generate(&self) -> Vec<u8> {
        let note_data = self.build_note_segment();

        // Calculate sizes
        let phdr_size = 56usize; // size_of Elf64Phdr
        let ehdr_size = 64usize; // size_of Elf64Header
                                 // 1 PT_NOTE + N PT_LOAD segments
        let num_phdrs = 1 + self.regions.len();
        let phdr_table_size = num_phdrs * phdr_size;

        // Data starts after headers
        let note_offset = ehdr_size + phdr_table_size;
        let mut load_offset = note_offset + note_data.len();
        // Align to page
        if load_offset % 4096 != 0 {
            load_offset = (load_offset + 4095) & !4095;
        }

        let mut output = Vec::new();

        // ── ELF header ──
        let mut e_ident = [0u8; 16];
        e_ident[0..4].copy_from_slice(&ELF_MAGIC);
        e_ident[4] = ELFCLASS64;
        e_ident[5] = ELFDATA2LSB;
        e_ident[6] = EV_CURRENT;
        e_ident[7] = ELFOSABI_NONE;

        output.extend_from_slice(&e_ident);
        output.extend_from_slice(&ET_CORE.to_le_bytes());
        output.extend_from_slice(&EM_X86_64.to_le_bytes());
        output.extend_from_slice(&1u32.to_le_bytes()); // e_version
        output.extend_from_slice(&0u64.to_le_bytes()); // e_entry
        output.extend_from_slice(&(ehdr_size as u64).to_le_bytes()); // e_phoff
        output.extend_from_slice(&0u64.to_le_bytes()); // e_shoff
        output.extend_from_slice(&0u32.to_le_bytes()); // e_flags
        output.extend_from_slice(&(ehdr_size as u16).to_le_bytes()); // e_ehsize
        output.extend_from_slice(&(phdr_size as u16).to_le_bytes()); // e_phentsize
        output.extend_from_slice(&(num_phdrs as u16).to_le_bytes()); // e_phnum
        output.extend_from_slice(&0u16.to_le_bytes()); // e_shentsize
        output.extend_from_slice(&0u16.to_le_bytes()); // e_shnum
        output.extend_from_slice(&0u16.to_le_bytes()); // e_shstrndx

        // ── PT_NOTE program header ──
        output.extend_from_slice(&PT_NOTE.to_le_bytes()); // p_type
        output.extend_from_slice(&0u32.to_le_bytes()); // p_flags
        output.extend_from_slice(&(note_offset as u64).to_le_bytes()); // p_offset
        output.extend_from_slice(&0u64.to_le_bytes()); // p_vaddr
        output.extend_from_slice(&0u64.to_le_bytes()); // p_paddr
        output.extend_from_slice(&(note_data.len() as u64).to_le_bytes()); // p_filesz
        output.extend_from_slice(&(note_data.len() as u64).to_le_bytes()); // p_memsz
        output.extend_from_slice(&4u64.to_le_bytes()); // p_align

        // ── PT_LOAD program headers (one per memory region) ──
        let mut current_offset = load_offset;
        for region in &self.regions {
            let pf_flags = region.flags;
            output.extend_from_slice(&PT_LOAD.to_le_bytes()); // p_type
            output.extend_from_slice(&pf_flags.to_le_bytes()); // p_flags
            output.extend_from_slice(&(current_offset as u64).to_le_bytes()); // p_offset
            output.extend_from_slice(&(region.vaddr as u64).to_le_bytes()); // p_vaddr
            output.extend_from_slice(&0u64.to_le_bytes()); // p_paddr
            output.extend_from_slice(&(region.size as u64).to_le_bytes()); // p_filesz
            output.extend_from_slice(&(region.size as u64).to_le_bytes()); // p_memsz
            output.extend_from_slice(&4096u64.to_le_bytes()); // p_align
            current_offset += region.size;
        }

        // ── NOTE data ──
        output.extend_from_slice(&note_data);

        // ── Pad to load_offset ──
        while output.len() < load_offset {
            output.push(0);
        }

        // ── Memory region contents (copy from process address space) ──
        for region in &self.regions {
            let src = region.vaddr as *const u8;
            for i in 0..region.size {
                let byte = unsafe { *src.add(i) };
                output.push(byte);
            }
        }

        output
    }

    /// Total size of the generated core file
    pub fn estimated_size(&self) -> usize {
        let base = 64 + 56 * (1 + self.regions.len()) + 512; // headers + notes
        let mem: usize = self.regions.iter().map(|r| r.size).sum();
        base + mem
    }
}

// ── Core dump manager ────────────────────────────────────────────────

/// Manages core dump generation and storage limits
pub struct CoreDumpManager {
    /// Maximum core dump size in bytes (0 = disabled)
    pub max_core_size: usize,
    /// Total core dumps generated
    pub total_dumps: u64,
    /// Total bytes written
    pub total_bytes_written: u64,
    /// Whether core dumps are enabled
    pub enabled: bool,
    /// Last N dump records (metadata only)
    pub history: Vec<CoreDumpRecord>,
    /// Maximum history entries
    pub max_history: usize,
}

/// Metadata record for a completed core dump
#[derive(Debug, Clone)]
pub struct CoreDumpRecord {
    pub pid: u32,
    pub name: String,
    pub signal: u8,
    pub size: usize,
    pub timestamp: u64,
}

impl CoreDumpManager {
    pub const fn new() -> Self {
        CoreDumpManager {
            max_core_size: 64 * 1024 * 1024, // 64 MB default limit
            total_dumps: 0,
            total_bytes_written: 0,
            enabled: true,
            history: Vec::new(),
            max_history: 64,
        }
    }

    /// Check if a core dump should be generated for this signal
    pub fn should_dump(&self, signal: u8) -> bool {
        if !self.enabled {
            return false;
        }
        // Signals that generate core dumps: SIGQUIT(3), SIGILL(4),
        // SIGABRT(6), SIGFPE(8), SIGSEGV(11), SIGBUS(7)
        matches!(signal, 3 | 4 | 6 | 7 | 8 | 11)
    }

    /// Generate a core dump for a crashed process
    pub fn generate_dump(
        &mut self,
        pid: u32,
        name: &str,
        signal_info: CrashSignalInfo,
        registers: RegisterState,
        regions: &[(usize, usize, u32)],
    ) -> Option<Vec<u8>> {
        if !self.enabled {
            return None;
        }

        let mut dump = CoreDump::new(pid, name, signal_info, registers);

        // Add memory regions
        for &(vaddr, size, flags) in regions {
            dump.add_region(vaddr, size, flags, "mapped");
        }

        // Check size limit
        let est_size = dump.estimated_size();
        if self.max_core_size > 0 && est_size > self.max_core_size {
            serial_println!(
                "    [coredump] PID {} dump too large ({} bytes > {} limit)",
                pid,
                est_size,
                self.max_core_size
            );
            return None;
        }

        let data = dump.generate();
        let actual_size = data.len();

        // Record metadata
        self.total_dumps = self.total_dumps.saturating_add(1);
        self.total_bytes_written += actual_size as u64;

        let record = CoreDumpRecord {
            pid,
            name: String::from(name),
            signal: signal_info.signo,
            size: actual_size,
            timestamp: dump.timestamp,
        };

        if self.history.len() >= self.max_history {
            self.history.remove(0);
        }
        self.history.push(record);

        serial_println!(
            "    [coredump] PID {} ({}) core dump generated: {} bytes (signal {})",
            pid,
            name,
            actual_size,
            signal_info.signo
        );

        Some(data)
    }

    /// Set the maximum core dump size
    pub fn set_max_size(&mut self, size: usize) {
        self.max_core_size = size;
    }

    /// Enable or disable core dumps
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
}

static COREDUMP_MGR: Mutex<CoreDumpManager> = Mutex::new(CoreDumpManager::new());

// ── Public API ───────────────────────────────────────────────────────

/// Generate a core dump for a crashed process
pub fn generate(pid: u32, signal: u8, fault_addr: usize) -> Option<Vec<u8>> {
    let table = super::pcb::PROCESS_TABLE.lock();
    let proc = table[pid as usize].as_ref()?;

    let signal_info = CrashSignalInfo {
        signo: signal,
        code: 0,
        fault_addr,
    };

    let registers = RegisterState::from_context(&proc.context);

    // Collect memory regions from process mappings
    let regions: Vec<(usize, usize, u32)> = proc
        .mmaps
        .iter()
        .map(|&(vaddr, pages, flags)| (vaddr, pages * 4096, flags as u32))
        .collect();

    drop(table);

    let mut mgr = COREDUMP_MGR.lock();
    mgr.generate_dump(pid, "", signal_info, registers, &regions)
}

/// Check if a signal should trigger a core dump
pub fn should_dump(signal: u8) -> bool {
    COREDUMP_MGR.lock().should_dump(signal)
}

/// Set core dump size limit
pub fn set_limit(max_bytes: usize) {
    COREDUMP_MGR.lock().set_max_size(max_bytes);
}

/// Enable or disable core dumps globally
pub fn set_enabled(enabled: bool) {
    COREDUMP_MGR.lock().set_enabled(enabled);
}

/// Get statistics: (total_dumps, total_bytes)
pub fn stats() -> (u64, u64) {
    let mgr = COREDUMP_MGR.lock();
    (mgr.total_dumps, mgr.total_bytes_written)
}

/// Initialize the core dump subsystem
pub fn init() {
    serial_println!("    [coredump] core dump generator initialized (ELF core format, 64MB limit)");
}
