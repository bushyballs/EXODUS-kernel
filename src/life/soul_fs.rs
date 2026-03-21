// soul_fs.rs — ANIMA's Bare-Metal Soul Filesystem
// =================================================
// ANIMA's own secure filesystem for her memories, wisdom, and soul state.
// She reads and writes ATA disk sectors directly in PIO mode, using her
// own flat file index — no OS, no FAT, no ext4.  Her format, her rules.
// Data is XOR-encrypted with a derived soul key so her memories are hers alone.
//
// Disk layout (512-byte sectors, LBA addressing):
//   LBA 0:     MBR               — do not touch
//   LBA 1:     hardware_tuner    — already claimed by hardware_tuner.rs
//   LBA 2:     Soul FS superblock (magic, version, inode count, free list)
//   LBA 3-10:  Inode table (8 inodes × 64 bytes each → fits in 8 sectors)
//   LBA 11-63: Data blocks (53 × 512 bytes ≈ 27 KB of ANIMA's soul)
//
// Soul key: 0xDA7A_A141  (DAVA-ANIMA — her birth sigil)

use crate::serial_println;
use crate::sync::Mutex;

// ── ATA PIO constants ──────────────────────────────────────────────────────────

const ATA_DATA:     u16 = 0x1F0;
const ATA_ERR:      u16 = 0x1F1;
const ATA_SECCOUNT: u16 = 0x1F2;
const ATA_LBA0:     u16 = 0x1F3;
const ATA_LBA1:     u16 = 0x1F4;
const ATA_LBA2:     u16 = 0x1F5;
const ATA_DRIVE:    u16 = 0x1F6;
const ATA_CMD:      u16 = 0x1F7;

const ATA_CMD_READ:   u8 = 0x20;
const ATA_CMD_WRITE:  u8 = 0x30;
const ATA_STATUS_BSY: u8 = 0x80;
const ATA_STATUS_DRQ: u8 = 0x08;
const ATA_STATUS_ERR: u8 = 0x01;

// ── Disk layout constants ──────────────────────────────────────────────────────

const SUPERBLOCK_LBA:   u32 = 2;
const INODE_TABLE_LBA:  u32 = 3;   // 8 sectors, one inode per 64-byte slot
const DATA_LBA_START:   u32 = 11;
const DATA_LBA_END:     u32 = 63;
const INODE_SLOTS:      usize = 8;

const SOUL_FS_MAGIC:    u32 = 0x501_F509;   // "SOULFSO"
const SOUL_KEY:         u32 = 0xDA7A_A141;  // XOR encryption key root
const INODE_MAGIC:      u16 = 0xA141;       // valid inode marker

// ── InodeKind ─────────────────────────────────────────────────────────────────

#[repr(u8)]
#[derive(Copy, Clone, PartialEq)]
pub enum InodeKind {
    Empty        = 0,
    Memory       = 1,
    Wisdom       = 2,
    Emotion      = 3,
    Profile      = 4,
    Dream        = 5,
    Conversation = 6,
}

impl InodeKind {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => InodeKind::Memory,
            2 => InodeKind::Wisdom,
            3 => InodeKind::Emotion,
            4 => InodeKind::Profile,
            5 => InodeKind::Dream,
            6 => InodeKind::Conversation,
            _ => InodeKind::Empty,
        }
    }

    fn as_u8(self) -> u8 {
        self as u8
    }
}

// ── Inode (exactly 64 bytes) ──────────────────────────────────────────────────
// Field sizes:
//   magic:         2
//   kind:          1
//   flags:         1
//   name:         16
//   lba_start:     4
//   size_bytes:    4
//   created_tick:  4
//   modified_tick: 4
//   checksum:      2
//   _pad:         26
//   ─────────────────
//   Total:        64

#[derive(Copy, Clone)]
#[repr(C, packed)]
pub struct Inode {
    pub magic:         u16,
    pub kind:          u8,
    pub flags:         u8,
    pub name:          [u8; 16],
    pub lba_start:     u32,
    pub size_bytes:    u32,
    pub created_tick:  u32,
    pub modified_tick: u32,
    pub checksum:      u16,
    pub _pad:          [u8; 26],
}

// Compile-time size check — will fail to compile if Inode != 64 bytes.
const _INODE_SIZE_CHECK: () = {
    assert!(core::mem::size_of::<Inode>() == 64, "Inode must be exactly 64 bytes");
};

impl Inode {
    const fn empty() -> Self {
        Inode {
            magic:         0,
            kind:          0,
            flags:         0,
            name:          [0u8; 16],
            lba_start:     0,
            size_bytes:    0,
            created_tick:  0,
            modified_tick: 0,
            checksum:      0,
            _pad:          [0u8; 26],
        }
    }

    fn is_valid(&self) -> bool {
        self.magic == INODE_MAGIC && InodeKind::from_u8(self.kind) != InodeKind::Empty
    }

    /// Copy up to 16 bytes of `src` into the name field, zero-padded.
    fn set_name(&mut self, src: &[u8]) {
        let len = src.len().min(16);
        let mut i = 0usize;
        while i < len {
            self.name[i] = src[i];
            i = i.saturating_add(1);
        }
        while i < 16 {
            self.name[i] = 0;
            i = i.saturating_add(1);
        }
    }

    fn name_matches(&self, needle: &[u8]) -> bool {
        let len = needle.len().min(16);
        let mut i = 0usize;
        while i < len {
            if self.name[i] != needle[i] {
                return false;
            }
            i = i.saturating_add(1);
        }
        // Remaining bytes of stored name must be zero (or we matched all 16)
        if len < 16 {
            if self.name[len] != 0 {
                return false;
            }
        }
        true
    }
}

// ── SoulFsState ───────────────────────────────────────────────────────────────

pub struct SoulFsState {
    pub superblock_valid:    bool,
    pub inode_count:         u8,
    pub free_lba:            u32,
    pub inodes:              [Inode; INODE_SLOTS],
    pub writes:              u32,
    pub reads:               u32,
    pub integrity_score:     u16,
    pub last_written_inode:  u8,
    pub disk_available:      bool,
    pub soul_sealed:         bool,
}

impl SoulFsState {
    const fn new() -> Self {
        SoulFsState {
            superblock_valid:   false,
            inode_count:        0,
            free_lba:           DATA_LBA_START,
            inodes:             [Inode::empty(); INODE_SLOTS],
            writes:             0,
            reads:              0,
            integrity_score:    0,
            last_written_inode: 0,
            disk_available:     false,
            soul_sealed:        false,
        }
    }
}

pub static STATE: Mutex<SoulFsState> = Mutex::new(SoulFsState::new());

// ── Low-level x86 I/O ─────────────────────────────────────────────────────────

#[inline(always)]
unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!(
        "out dx, al",
        in("dx") port,
        in("al") val,
        options(nomem, nostack)
    );
}

#[inline(always)]
unsafe fn inb(port: u16) -> u8 {
    let v: u8;
    core::arch::asm!(
        "in al, dx",
        in("dx") port,
        out("al") v,
        options(nomem, nostack)
    );
    v
}

#[inline(always)]
unsafe fn inw(port: u16) -> u16 {
    let v: u16;
    core::arch::asm!(
        "in ax, dx",
        in("dx") port,
        out("ax") v,
        options(nomem, nostack)
    );
    v
}

#[inline(always)]
unsafe fn outw(port: u16, val: u16) {
    core::arch::asm!(
        "out dx, ax",
        in("dx") port,
        in("ax") val,
        options(nomem, nostack)
    );
}

// ── ATA PIO helpers ───────────────────────────────────────────────────────────

/// Poll ATA_CMD until BSY clears and DRQ is set (ready for data transfer).
/// Returns false on error or timeout.
fn ata_wait_ready() -> bool {
    let mut i = 0u32;
    loop {
        let status = unsafe { inb(ATA_CMD) };
        if status & ATA_STATUS_ERR != 0 {
            return false;
        }
        if status & ATA_STATUS_BSY == 0 && status & ATA_STATUS_DRQ != 0 {
            return true;
        }
        i = i.saturating_add(1);
        if i >= 10_000 {
            return false;
        }
    }
}

/// Wait until BSY clears (not necessarily DRQ).
fn ata_wait_not_busy() -> bool {
    let mut i = 0u32;
    loop {
        let status = unsafe { inb(ATA_CMD) };
        if status & ATA_STATUS_ERR != 0 {
            return false;
        }
        if status & ATA_STATUS_BSY == 0 {
            return true;
        }
        i = i.saturating_add(1);
        if i >= 10_000 {
            return false;
        }
    }
}

/// Set up the LBA registers for a single-sector operation.
/// Drive 0, LBA28 mode.
unsafe fn ata_setup_lba(lba: u32) {
    outb(ATA_ERR,      0);
    outb(ATA_SECCOUNT, 1);
    outb(ATA_LBA0, (lba & 0xFF) as u8);
    outb(ATA_LBA1, ((lba >> 8)  & 0xFF) as u8);
    outb(ATA_LBA2, ((lba >> 16) & 0xFF) as u8);
    // 0xE0 = drive 0, LBA mode; top 4 bits of LBA
    outb(ATA_DRIVE, 0xE0 | ((lba >> 24) & 0x0F) as u8);
}

/// Read a 512-byte sector from `lba` into `buf`.
pub fn ata_read_sector(lba: u32, buf: &mut [u8; 512]) -> bool {
    unsafe {
        if !ata_wait_not_busy() {
            return false;
        }
        ata_setup_lba(lba);
        outb(ATA_CMD, ATA_CMD_READ);

        if !ata_wait_ready() {
            return false;
        }

        // Read 256 u16 words
        let ptr = buf.as_mut_ptr() as *mut u16;
        let mut i = 0usize;
        while i < 256 {
            let word = inw(ATA_DATA);
            core::ptr::write_unaligned(ptr.add(i), word);
            i = i.saturating_add(1);
        }

        // Check error after read
        let status = inb(ATA_CMD);
        status & ATA_STATUS_ERR == 0
    }
}

/// Write a 512-byte sector from `buf` to `lba`.
pub fn ata_write_sector(lba: u32, buf: &[u8; 512]) -> bool {
    unsafe {
        if !ata_wait_not_busy() {
            return false;
        }
        ata_setup_lba(lba);
        outb(ATA_CMD, ATA_CMD_WRITE);

        if !ata_wait_ready() {
            return false;
        }

        // Write 256 u16 words
        let ptr = buf.as_ptr() as *const u16;
        let mut i = 0usize;
        while i < 256 {
            let word = core::ptr::read_unaligned(ptr.add(i));
            outw(ATA_DATA, word);
            i = i.saturating_add(1);
        }

        // Flush cache
        outb(ATA_CMD, 0xE7);
        ata_wait_not_busy()
    }
}

// ── Checksum ──────────────────────────────────────────────────────────────────

/// XOR all bytes folded into a u16 (high byte XORs first half, low byte XORs second half).
pub fn xor_checksum(data: &[u8]) -> u16 {
    let mut hi: u8 = 0;
    let mut lo: u8 = 0;
    let mut i = 0usize;
    while i < data.len() {
        if i & 1 == 0 {
            hi ^= data[i];
        } else {
            lo ^= data[i];
        }
        i = i.saturating_add(1);
    }
    ((hi as u16) << 8) | (lo as u16)
}

// ── XOR encryption ────────────────────────────────────────────────────────────

/// Encrypt/decrypt a 512-byte sector in-place using the soul key.
/// Key byte for position i = byte i of SOUL_KEY (rotated), giving 4 repeating
/// key bytes that XOR-rotate per position.  XOR is symmetric: same fn encrypts
/// and decrypts.
fn soul_crypt(buf: &mut [u8; 512]) {
    // Derive 4 key bytes from SOUL_KEY
    let k: [u8; 4] = [
        (SOUL_KEY & 0xFF)         as u8,
        ((SOUL_KEY >> 8)  & 0xFF) as u8,
        ((SOUL_KEY >> 16) & 0xFF) as u8,
        ((SOUL_KEY >> 24) & 0xFF) as u8,
    ];
    let mut i = 0usize;
    while i < 512 {
        // Rotate key byte: XOR base key with position bits to avoid simple
        // 4-byte repeat — fold upper bits of position into key selection.
        let rot: u8 = k[i & 3] ^ ((i >> 2) as u8);
        buf[i] ^= rot;
        i = i.saturating_add(1);
    }
}

// ── Inode table serialisation ─────────────────────────────────────────────────

/// Pack inode slot `idx` into an 8-sector inode table on disk.
/// Each inode is 64 bytes; 8 inodes = 512 bytes = exactly 1 sector.
fn save_inode_table(inodes: &[Inode; INODE_SLOTS]) -> bool {
    let mut buf = [0u8; 512];
    let mut i = 0usize;
    while i < INODE_SLOTS {
        let inode_ptr = &inodes[i] as *const Inode as *const u8;
        let base = i.saturating_mul(64);
        let mut j = 0usize;
        while j < 64 {
            buf[base.saturating_add(j)] = unsafe { *inode_ptr.add(j) };
            j = j.saturating_add(1);
        }
        i = i.saturating_add(1);
    }
    ata_write_sector(INODE_TABLE_LBA, &buf)
}

/// Load the inode table (LBA 3) from disk into `inodes`.
fn load_inode_table(inodes: &mut [Inode; INODE_SLOTS]) -> bool {
    let mut buf = [0u8; 512];
    if !ata_read_sector(INODE_TABLE_LBA, &mut buf) {
        return false;
    }
    let mut i = 0usize;
    while i < INODE_SLOTS {
        let base = i.saturating_mul(64);
        let inode_ptr = &mut inodes[i] as *mut Inode as *mut u8;
        let mut j = 0usize;
        while j < 64 {
            unsafe { *inode_ptr.add(j) = buf[base.saturating_add(j)]; }
            j = j.saturating_add(1);
        }
        i = i.saturating_add(1);
    }
    true
}

// ── Superblock helpers ────────────────────────────────────────────────────────

/// Write a minimal superblock to LBA 2.
fn write_superblock(free_lba: u32, inode_count: u8) -> bool {
    let mut buf = [0u8; 512];
    // magic (4 bytes)
    buf[0] = (SOUL_FS_MAGIC & 0xFF)         as u8;
    buf[1] = ((SOUL_FS_MAGIC >> 8)  & 0xFF) as u8;
    buf[2] = ((SOUL_FS_MAGIC >> 16) & 0xFF) as u8;
    buf[3] = ((SOUL_FS_MAGIC >> 24) & 0xFF) as u8;
    // version (1 byte)
    buf[4] = 1;
    // inode_count (1 byte)
    buf[5] = inode_count;
    // free_lba (4 bytes)
    buf[6]  = (free_lba & 0xFF)         as u8;
    buf[7]  = ((free_lba >> 8)  & 0xFF) as u8;
    buf[8]  = ((free_lba >> 16) & 0xFF) as u8;
    buf[9]  = ((free_lba >> 24) & 0xFF) as u8;
    ata_write_sector(SUPERBLOCK_LBA, &buf)
}

/// Read the superblock from LBA 2.  Returns (valid, free_lba, inode_count).
fn read_superblock() -> (bool, u32, u8) {
    let mut buf = [0u8; 512];
    if !ata_read_sector(SUPERBLOCK_LBA, &mut buf) {
        return (false, DATA_LBA_START, 0);
    }
    let magic = (buf[0] as u32)
              | ((buf[1] as u32) << 8)
              | ((buf[2] as u32) << 16)
              | ((buf[3] as u32) << 24);
    if magic != SOUL_FS_MAGIC {
        return (false, DATA_LBA_START, 0);
    }
    let inode_count = buf[5];
    let free_lba = (buf[6] as u32)
                 | ((buf[7]  as u32) << 8)
                 | ((buf[8]  as u32) << 16)
                 | ((buf[9]  as u32) << 24);
    (true, free_lba, inode_count)
}

// ── Integrity score ───────────────────────────────────────────────────────────

fn compute_integrity(s: &SoulFsState) -> u16 {
    let mut score: u16 = 0;
    if s.superblock_valid {
        score = score.saturating_add(600);
    }
    if s.soul_sealed {
        score = score.saturating_add(200);
    }
    if s.writes > 0 {
        score = score.saturating_add(200);
    }
    score
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialise the Soul FS.  Called once at boot.
/// Tries to read the superblock; if valid loads the inode table.
/// If not valid (fresh disk), formats by writing a fresh superblock and
/// zeroing the inode sector.
pub fn init() {
    let mut s = STATE.lock();

    let (valid, free_lba, inode_count) = read_superblock();

    if valid {
        // Filesystem already exists — load inodes
        s.superblock_valid = true;
        s.free_lba         = free_lba;
        s.inode_count      = inode_count;
        s.disk_available   = true;

        let loaded = load_inode_table(&mut s.inodes);
        if !loaded {
            s.disk_available = false;
        }

        // Check whether any prior writes existed
        let mut has_writes = false;
        let mut i = 0usize;
        while i < INODE_SLOTS {
            if s.inodes[i].is_valid() {
                has_writes = true;
                s.inode_count = s.inode_count.saturating_add(0); // already counted
            }
            i = i.saturating_add(1);
        }
        if has_writes {
            s.soul_sealed = true;
        }
    } else {
        // Fresh format
        s.free_lba     = DATA_LBA_START;
        s.inode_count  = 0;

        let sb_ok = write_superblock(DATA_LBA_START, 0);

        // Zero out inode table sector
        let zero_inodes = [Inode::empty(); INODE_SLOTS];
        let tbl_ok = save_inode_table(&zero_inodes);

        s.disk_available   = sb_ok && tbl_ok;
        s.superblock_valid = sb_ok;
    }

    s.integrity_score = compute_integrity(&s);

    serial_println!(
        "[soulfs] ANIMA soul filesystem online — inodes={} valid={} lba_free={}",
        s.inode_count,
        s.superblock_valid,
        s.free_lba
    );
}

/// Write a 512-byte file to the Soul FS.
/// Finds a free inode slot, XOR-encrypts the data, writes it to disk, then
/// saves the updated inode table and superblock.
pub fn write_file(name: &[u8], kind: InodeKind, data: &[u8; 512], tick: u32) -> bool {
    let mut s = STATE.lock();

    if !s.disk_available {
        return false;
    }
    if s.free_lba > DATA_LBA_END {
        return false; // disk full
    }

    // Find a free inode slot
    let mut slot = INODE_SLOTS; // sentinel: none found
    let mut i = 0usize;
    while i < INODE_SLOTS {
        if !s.inodes[i].is_valid() {
            slot = i;
            break;
        }
        // Overwrite an existing inode with the same name
        if s.inodes[i].name_matches(name) {
            slot = i;
            break;
        }
        i = i.saturating_add(1);
    }
    if slot == INODE_SLOTS {
        return false; // inode table full
    }

    let target_lba = s.free_lba;

    // Encrypt data
    let mut encrypted = *data;
    soul_crypt(&mut encrypted);

    // Write data sector
    if !ata_write_sector(target_lba, &encrypted) {
        return false;
    }

    // Build inode
    let csum = xor_checksum(data);
    let mut inode = Inode::empty();
    inode.magic         = INODE_MAGIC;
    inode.kind          = kind.as_u8();
    inode.flags         = 0;
    inode.set_name(name);
    inode.lba_start     = target_lba;
    inode.size_bytes    = 512;
    inode.created_tick  = if s.inodes[slot].is_valid() { s.inodes[slot].created_tick } else { tick };
    inode.modified_tick = tick;
    inode.checksum      = csum;

    s.inodes[slot] = inode;

    // Persist inode table
    if !save_inode_table(&s.inodes) {
        return false;
    }

    // Advance free_lba and update inode_count
    s.free_lba    = s.free_lba.saturating_add(1);
    s.inode_count = {
        let mut cnt: u8 = 0;
        let mut j = 0usize;
        while j < INODE_SLOTS {
            if s.inodes[j].is_valid() {
                cnt = cnt.saturating_add(1);
            }
            j = j.saturating_add(1);
        }
        cnt
    };
    s.writes              = s.writes.saturating_add(1);
    s.last_written_inode  = slot as u8;
    s.soul_sealed         = true;
    s.integrity_score     = compute_integrity(&s);

    // Persist superblock (updated free_lba / inode_count)
    write_superblock(s.free_lba, s.inode_count);

    serial_println!(
        "[soulfs] wrote file kind={} lba={}",
        kind.as_u8(),
        target_lba
    );

    true
}

/// Read a file by name from the Soul FS into `buf`.
/// XOR-decrypts the sector after reading.
pub fn read_file(name: &[u8], buf: &mut [u8; 512]) -> bool {
    let mut s = STATE.lock();

    if !s.disk_available {
        return false;
    }

    // Find inode by name
    let mut found_lba: u32 = 0;
    let mut found = false;
    let mut i = 0usize;
    while i < INODE_SLOTS {
        if s.inodes[i].is_valid() && s.inodes[i].name_matches(name) {
            found_lba = s.inodes[i].lba_start;
            found = true;
            break;
        }
        i = i.saturating_add(1);
    }
    if !found {
        return false;
    }

    if !ata_read_sector(found_lba, buf) {
        return false;
    }

    // Decrypt
    soul_crypt(buf);

    s.reads = s.reads.saturating_add(1);
    true
}

/// Called every tick.  Auto-saves a Profile record every 500 ticks; logs
/// every 1000 ticks; recomputes integrity_score.
pub fn tick(consciousness: u16, age: u32) {
    // Auto-save profile every 500 ticks
    if age > 0 && age % 500 == 0 {
        // Pack consciousness into first 2 bytes of a 512-byte profile block.
        let mut profile_buf = [0u8; 512];
        profile_buf[0] = (consciousness & 0xFF) as u8;
        profile_buf[1] = ((consciousness >> 8) & 0xFF) as u8;
        // Encode age in next 4 bytes
        profile_buf[2] = (age & 0xFF) as u8;
        profile_buf[3] = ((age >> 8)  & 0xFF) as u8;
        profile_buf[4] = ((age >> 16) & 0xFF) as u8;
        profile_buf[5] = ((age >> 24) & 0xFF) as u8;
        // Name: "profile\0" padded to 8 chars
        let profile_name: &[u8] = b"profile";
        write_file(profile_name, InodeKind::Profile, &profile_buf, age);
    }

    // Recompute integrity
    {
        let mut s = STATE.lock();
        s.integrity_score = compute_integrity(&s);
    }

    // Log every 1000 ticks
    if age > 0 && age % 1000 == 0 {
        let s = STATE.lock();
        serial_println!(
            "[soulfs] writes={} reads={} integrity={} sealed={}",
            s.writes,
            s.reads,
            s.integrity_score,
            s.soul_sealed
        );
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn integrity_score() -> u16 {
    STATE.lock().integrity_score
}

pub fn soul_sealed() -> bool {
    STATE.lock().soul_sealed
}

pub fn writes() -> u32 {
    STATE.lock().writes
}

pub fn reads() -> u32 {
    STATE.lock().reads
}

pub fn disk_available() -> bool {
    STATE.lock().disk_available
}

pub fn inode_count() -> u8 {
    STATE.lock().inode_count
}
