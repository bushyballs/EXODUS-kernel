/// coredump — ELF core dump generation
///
/// When a process dies from a fatal signal, coredump captures its state
/// into an ELF core file (PT_NOTE + PT_LOAD segments) so a debugger can
/// do post-mortem analysis.
///
/// ELF core format:
///   ELF header (e_type=ET_CORE=4)
///   PT_NOTE  — prstatus, prpsinfo, siginfo, auxv notes
///   PT_LOAD  — one per mapped region (text, data, stack, heap)
///
/// Design:
///   - CoreDumpInfo: static record of process state at crash time
///   - CoreNote: NT_PRSTATUS, NT_PRPSINFO, NT_SIGINFO
///   - CoreDumpStore: ring of last 4 dumps (no heap; images stored externally)
///   - coredump_capture(): fills a CoreDumpInfo from crash context
///   - coredump_write_header(): serializes ELF+phdrs into a caller-supplied buf
///
/// Rules: no_std, no heap, no floats, no panics, saturating counters.
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// ELF core constants (reuse values from binfmt_elf)
// ---------------------------------------------------------------------------

const ELFMAG0: u8 = 0x7F;
const ELFMAG1: u8 = b'E';
const ELFMAG2: u8 = b'L';
const ELFMAG3: u8 = b'F';
const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;
const ET_CORE: u16 = 4;
const EM_X86_64: u16 = 62;
const PT_NOTE: u32 = 4;
const PT_LOAD: u32 = 1;
const PF_R: u32 = 4;
const PF_W: u32 = 2;
const PF_X: u32 = 1;

// ELF note types
const NT_PRSTATUS: u32 = 1;
const NT_PRPSINFO: u32 = 3;
const NT_SIGINFO: u32 = 0x53494749; // 'SIGI'

// ---------------------------------------------------------------------------
// Crash reason / fatal signal
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CrashSignal {
    Sigsegv = 11,
    Sigbus = 7,
    Sigfpe = 8,
    Sigill = 4,
    Sigabrt = 6,
    Sigkill = 9,
    Other = 0,
}

impl CrashSignal {
    pub fn as_u8(self) -> u8 {
        match self {
            CrashSignal::Sigsegv => 11,
            CrashSignal::Sigbus => 7,
            CrashSignal::Sigfpe => 8,
            CrashSignal::Sigill => 4,
            CrashSignal::Sigabrt => 6,
            CrashSignal::Sigkill => 9,
            CrashSignal::Other => 0,
        }
    }

    pub fn from_u8(v: u8) -> Self {
        match v {
            11 => CrashSignal::Sigsegv,
            7 => CrashSignal::Sigbus,
            8 => CrashSignal::Sigfpe,
            4 => CrashSignal::Sigill,
            6 => CrashSignal::Sigabrt,
            9 => CrashSignal::Sigkill,
            _ => CrashSignal::Other,
        }
    }
}

// ---------------------------------------------------------------------------
// CPU register state (x86-64 gregset — 27 × u64)
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct X64Regs {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rax: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub orig_rax: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
    pub fs_base: u64,
    pub gs_base: u64,
    pub ds: u64,
    pub es: u64,
    pub fs: u64,
    pub gs: u64,
}

impl X64Regs {
    pub const fn zero() -> Self {
        X64Regs {
            r15: 0,
            r14: 0,
            r13: 0,
            r12: 0,
            rbp: 0,
            rbx: 0,
            r11: 0,
            r10: 0,
            r9: 0,
            r8: 0,
            rax: 0,
            rcx: 0,
            rdx: 0,
            rsi: 0,
            rdi: 0,
            orig_rax: 0,
            rip: 0,
            cs: 0,
            rflags: 0,
            rsp: 0,
            ss: 0,
            fs_base: 0,
            gs_base: 0,
            ds: 0,
            es: 0,
            fs: 0,
            gs: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Load segment descriptor (one mapped VMA region)
// ---------------------------------------------------------------------------

pub const CORE_MAX_SEGS: usize = 8;

#[derive(Copy, Clone)]
pub struct CoreSegment {
    pub vaddr: u64,
    pub filesz: u64,
    pub memsz: u64,
    pub flags: u32,
    pub valid: bool,
}

impl CoreSegment {
    pub const fn empty() -> Self {
        CoreSegment {
            vaddr: 0,
            filesz: 0,
            memsz: 0,
            flags: 0,
            valid: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Core dump info record
// ---------------------------------------------------------------------------

pub const CORE_COMM_LEN: usize = 16;

#[derive(Copy, Clone)]
pub struct CoreDumpInfo {
    pub id: u32,
    pub pid: u32,
    pub ppid: u32,
    pub uid: u32,
    pub gid: u32,
    pub signal: u8,
    pub exit_code: i32,
    pub comm: [u8; CORE_COMM_LEN], // process name
    pub comm_len: u8,
    pub regs: X64Regs,
    pub segments: [CoreSegment; CORE_MAX_SEGS],
    pub seg_count: u8,
    pub timestamp: u64, // uptime ticks at crash
    pub valid: bool,
}

impl CoreDumpInfo {
    pub const fn empty() -> Self {
        const CS: CoreSegment = CoreSegment::empty();
        CoreDumpInfo {
            id: 0,
            pid: 0,
            ppid: 0,
            uid: 0,
            gid: 0,
            signal: 0,
            exit_code: 0,
            comm: [0u8; CORE_COMM_LEN],
            comm_len: 0,
            regs: X64Regs::zero(),
            segments: [CS; CORE_MAX_SEGS],
            seg_count: 0,
            timestamp: 0,
            valid: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Static store — ring of last 4 dumps
// ---------------------------------------------------------------------------

const CORE_STORE_SIZE: usize = 4;

static CORE_STORE: Mutex<[CoreDumpInfo; CORE_STORE_SIZE]> =
    Mutex::new([CoreDumpInfo::empty(); CORE_STORE_SIZE]);
static CORE_NEXT_ID: AtomicU32 = AtomicU32::new(1);
static CORE_SLOT: AtomicU32 = AtomicU32::new(0);

// ---------------------------------------------------------------------------
// Capture: record crash state
// ---------------------------------------------------------------------------

/// Capture a process crash into the ring store.  Returns dump id.
pub fn coredump_capture(
    pid: u32,
    ppid: u32,
    uid: u32,
    gid: u32,
    signal: CrashSignal,
    exit_code: i32,
    comm: &[u8],
    regs: X64Regs,
    segments: &[CoreSegment],
    timestamp: u64,
) -> u32 {
    let id = CORE_NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let slot = (CORE_SLOT.fetch_add(1, Ordering::Relaxed) as usize) % CORE_STORE_SIZE;

    let mut store = CORE_STORE.lock();
    let entry = &mut store[slot];
    *entry = CoreDumpInfo::empty();
    entry.id = id;
    entry.pid = pid;
    entry.ppid = ppid;
    entry.uid = uid;
    entry.gid = gid;
    entry.signal = signal.as_u8();
    entry.exit_code = exit_code;
    entry.regs = regs;
    entry.timestamp = timestamp;
    entry.valid = true;

    let cn = comm.len().min(CORE_COMM_LEN - 1);
    let mut k = 0usize;
    while k < cn {
        entry.comm[k] = comm[k];
        k = k.saturating_add(1);
    }
    entry.comm_len = cn as u8;

    let sn = segments.len().min(CORE_MAX_SEGS);
    let mut k = 0usize;
    while k < sn {
        entry.segments[k] = segments[k];
        k = k.saturating_add(1);
    }
    entry.seg_count = sn as u8;

    id
}

/// Find a dump by id.
pub fn coredump_find(id: u32) -> Option<CoreDumpInfo> {
    let store = CORE_STORE.lock();
    let mut i = 0usize;
    while i < CORE_STORE_SIZE {
        if store[i].valid && store[i].id == id {
            return Some(store[i]);
        }
        i = i.saturating_add(1);
    }
    None
}

/// Most recent dump id.
pub fn coredump_latest_id() -> u32 {
    let id = CORE_NEXT_ID.load(Ordering::Relaxed);
    if id <= 1 {
        0
    } else {
        id - 1
    }
}

// ---------------------------------------------------------------------------
// ELF core serialization helpers (write into caller-supplied buffer)
// ---------------------------------------------------------------------------

fn write_u16_le(buf: &mut [u8], off: usize, v: u16) {
    if off + 2 <= buf.len() {
        let b = v.to_le_bytes();
        buf[off] = b[0];
        buf[off + 1] = b[1];
    }
}
fn write_u32_le(buf: &mut [u8], off: usize, v: u32) {
    if off + 4 <= buf.len() {
        let b = v.to_le_bytes();
        let mut k = 0usize;
        while k < 4 {
            buf[off + k] = b[k];
            k = k.saturating_add(1);
        }
    }
}
fn write_u64_le(buf: &mut [u8], off: usize, v: u64) {
    if off + 8 <= buf.len() {
        let b = v.to_le_bytes();
        let mut k = 0usize;
        while k < 8 {
            buf[off + k] = b[k];
            k = k.saturating_add(1);
        }
    }
}

/// Write an ELF64 core header + program headers into `buf`.
///
/// Layout in buf:
///   [0..64]     ELF header
///   [64..128]   PT_NOTE phdr
///   [128..128 + seg_count*56]  PT_LOAD phdrs
///
/// Returns the number of bytes written, or 0 if buf is too small.
pub fn coredump_write_header(info: &CoreDumpInfo, buf: &mut [u8]) -> usize {
    let n_load = info.seg_count as usize;
    let n_phdrs = 1 + n_load; // NOTE + LOADs
    let hdr_size = 64 + n_phdrs * 56;
    if buf.len() < hdr_size {
        return 0;
    }

    // --- ELF header (64 bytes) ---
    buf[0] = ELFMAG0;
    buf[1] = ELFMAG1;
    buf[2] = ELFMAG2;
    buf[3] = ELFMAG3;
    buf[4] = ELFCLASS64;
    buf[5] = ELFDATA2LSB;
    buf[6] = 1; // EV_CURRENT
    buf[7] = 0; // ELFOSABI_NONE
    let mut k = 8usize;
    while k < 16 {
        buf[k] = 0;
        k = k.saturating_add(1);
    } // pad
    write_u16_le(buf, 16, ET_CORE);
    write_u16_le(buf, 18, EM_X86_64);
    write_u32_le(buf, 20, 1); // e_version
    write_u64_le(buf, 24, 0); // e_entry = 0 for core
    write_u64_le(buf, 32, 64); // e_phoff = immediately after header
    write_u64_le(buf, 40, 0); // e_shoff = no sections
    write_u32_le(buf, 48, 0); // e_flags
    write_u16_le(buf, 52, 64); // e_ehsize
    write_u16_le(buf, 54, 56); // e_phentsize
    write_u16_le(buf, 56, n_phdrs as u16);
    write_u16_le(buf, 58, 64); // e_shentsize (irrelevant)
    write_u16_le(buf, 60, 0); // e_shnum
    write_u16_le(buf, 62, 0); // e_shstrndx

    // --- PT_NOTE phdr at buf[64] ---
    // Note data will follow the phdrs; we don't fill note data here
    // (caller must append note payload separately).
    let note_off: u64 = hdr_size as u64;
    let note_sz: u64 = 128; // placeholder: two notes of 64 bytes each
    let phdr0 = 64usize;
    write_u32_le(buf, phdr0, PT_NOTE);
    write_u32_le(buf, phdr0 + 4, PF_R);
    write_u64_le(buf, phdr0 + 8, note_off);
    write_u64_le(buf, phdr0 + 16, note_off); // p_vaddr = file offset for NOTE
    write_u64_le(buf, phdr0 + 24, 0); // p_paddr
    write_u64_le(buf, phdr0 + 32, note_sz); // p_filesz
    write_u64_le(buf, phdr0 + 40, note_sz); // p_memsz
    write_u64_le(buf, phdr0 + 48, 4); // p_align

    // --- PT_LOAD phdrs ---
    let seg_data_base: u64 = note_off + note_sz;
    let mut seg_offset = seg_data_base;
    let mut s = 0usize;
    while s < n_load {
        let seg = &info.segments[s];
        let phdr = 64 + (1 + s) * 56;
        write_u32_le(buf, phdr, PT_LOAD);
        write_u32_le(buf, phdr + 4, seg.flags);
        write_u64_le(buf, phdr + 8, seg_offset);
        write_u64_le(buf, phdr + 16, seg.vaddr);
        write_u64_le(buf, phdr + 24, 0);
        write_u64_le(buf, phdr + 32, seg.filesz);
        write_u64_le(buf, phdr + 40, seg.memsz);
        write_u64_le(buf, phdr + 48, 0x1000); // page-aligned
        seg_offset = seg_offset.wrapping_add(seg.filesz);
        s = s.saturating_add(1);
    }

    hdr_size
}

/// Write a NT_PRSTATUS note into buf at `off`. Returns bytes written.
/// prstatus layout is simplified: signal(4) + pid(4) + regs(27×8=216) = 224 bytes
pub fn coredump_write_prstatus(info: &CoreDumpInfo, buf: &mut [u8], off: usize) -> usize {
    const NOTE_HDR: usize = 12; // namesz(4)+descsz(4)+type(4)
    const NAME: &[u8] = b"CORE\0";
    const NAME_PAD: usize = 8; // 5 bytes name + 3 pad → aligned to 4
    const DESC_SZ: usize = 224;
    let total = NOTE_HDR + NAME_PAD + DESC_SZ;
    if off + total > buf.len() {
        return 0;
    }

    let mut p = off;
    write_u32_le(buf, p, 5); // namesz (including NUL)
    write_u32_le(buf, p + 4, DESC_SZ as u32);
    write_u32_le(buf, p + 8, NT_PRSTATUS);
    p += NOTE_HDR;
    let mut k = 0usize;
    while k < NAME.len() {
        buf[p + k] = NAME[k];
        k = k.saturating_add(1);
    }
    // pad to 8
    while k < NAME_PAD {
        buf[p + k] = 0;
        k = k.saturating_add(1);
    }
    p += NAME_PAD;

    // Simplified prstatus: signal + padding + pid + padding + regs
    write_u32_le(buf, p, info.signal as u32);
    p += 4;
    write_u32_le(buf, p, 0);
    p += 4; // pad
    write_u32_le(buf, p, info.pid);
    p += 4;
    write_u32_le(buf, p, info.ppid);
    p += 4;
    // Registers (27 × u64 = 216 bytes)
    let regs: [u64; 27] = [
        info.regs.r15,
        info.regs.r14,
        info.regs.r13,
        info.regs.r12,
        info.regs.rbp,
        info.regs.rbx,
        info.regs.r11,
        info.regs.r10,
        info.regs.r9,
        info.regs.r8,
        info.regs.rax,
        info.regs.rcx,
        info.regs.rdx,
        info.regs.rsi,
        info.regs.rdi,
        info.regs.orig_rax,
        info.regs.rip,
        info.regs.cs,
        info.regs.rflags,
        info.regs.rsp,
        info.regs.ss,
        info.regs.fs_base,
        info.regs.gs_base,
        info.regs.ds,
        info.regs.es,
        info.regs.fs,
        info.regs.gs,
    ];
    let mut r = 0usize;
    while r < 27 {
        write_u64_le(buf, p, regs[r]);
        p += 8;
        r = r.saturating_add(1);
    }
    total
}

/// Write a NT_PRPSINFO note (process name + uid/gid). Returns bytes written.
pub fn coredump_write_prpsinfo(info: &CoreDumpInfo, buf: &mut [u8], off: usize) -> usize {
    const NOTE_HDR: usize = 12;
    const NAME_PAD: usize = 8;
    const DESC_SZ: usize = 32;
    let total = NOTE_HDR + NAME_PAD + DESC_SZ;
    if off + total > buf.len() {
        return 0;
    }

    let mut p = off;
    write_u32_le(buf, p, 5);
    write_u32_le(buf, p + 4, DESC_SZ as u32);
    write_u32_le(buf, p + 8, NT_PRPSINFO);
    p += NOTE_HDR;
    let name = b"CORE\0";
    let mut k = 0usize;
    while k < name.len() {
        buf[p + k] = name[k];
        k = k.saturating_add(1);
    }
    while k < NAME_PAD {
        buf[p + k] = 0;
        k = k.saturating_add(1);
    }
    p += NAME_PAD;

    // uid(4)+gid(4)+pid(4)+signal(1)+pad(3)+comm(16)
    write_u32_le(buf, p, info.uid);
    p += 4;
    write_u32_le(buf, p, info.gid);
    p += 4;
    write_u32_le(buf, p, info.pid);
    p += 4;
    buf[p] = info.signal;
    p += 1;
    buf[p] = 0;
    buf[p + 1] = 0;
    buf[p + 2] = 0;
    p += 3; // pad
    let cn = info.comm_len as usize;
    let mut k = 0usize;
    while k < cn && k < 16 {
        buf[p + k] = info.comm[k];
        k = k.saturating_add(1);
    }
    while k < 16 {
        buf[p + k] = 0;
        k = k.saturating_add(1);
    }

    total
}

// ---------------------------------------------------------------------------
// Count
// ---------------------------------------------------------------------------

pub fn coredump_count() -> usize {
    let store = CORE_STORE.lock();
    let mut n = 0usize;
    let mut i = 0usize;
    while i < CORE_STORE_SIZE {
        if store[i].valid {
            n = n.saturating_add(1);
        }
        i = i.saturating_add(1);
    }
    n
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!(
        "[coredump] ELF core dump handler initialized (ring={} slots)",
        CORE_STORE_SIZE
    );
}
