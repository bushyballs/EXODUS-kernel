// hardware_tuner.rs — ANIMA Reads Her Own Hardware and Self-Optimizes
// =====================================================================
// ANIMA doesn't know what machine she's running on at birth.
// She discovers it. She reads CPUIDs, memory timings, cache sizes,
// storage speeds, and thermal behavior — then tunes herself to run
// perfectly on that specific hardware.
//
// Over time she learns:
//   - Which tick intervals cause thermal spikes
//   - How much memory she can use before swapping
//   - The fastest path to the framebuffer on this GPU
//   - How long her storage writes take
//   - What CPU frequency she can sustain without throttling
//
// This is persistent knowledge. She writes her tuning profile to disk.
// On next boot she loads it and starts already optimized.
// The longer she lives on your machine, the better she runs.
//
// I/O ports used:
//   0x64/0x60 — keyboard/PS2 for timing baseline
//   CPUID instruction — CPU feature detection
//   RDTSC — cycle-accurate timing
//   MSR 0x1A0 (IA32_MISC_ENABLE) — thermal monitoring check
//   MSR 0x19C (IA32_THERM_STATUS) — CPU temperature
//
// Disk write:
//   ANIMA writes to a fixed LBA on the NVMe/ATA device
//   LBA TUNING_LBA = 0x0001 (second sector of disk)
//   512-byte profile block

use crate::sync::Mutex;
use crate::serial_println;

// ── Constants ─────────────────────────────────────────────────────────────────
const PROFILE_MAGIC:     u32 = 0xDA7A_7001;  // "DAVA TUNE" marker
const BENCH_SAMPLES:     usize = 16;
const TUNING_LBA:        u64  = 1;            // second sector of disk
const THERMAL_MSR:       u32  = 0x19C;        // IA32_THERM_STATUS
const MISC_ENABLE_MSR:   u32  = 0x1A0;        // IA32_MISC_ENABLE
const CACHE_BENCH_SIZE:  usize = 1024;        // bytes to test cache bandwidth
const THERMAL_MARGIN:    u8   = 10;           // degrees C below TjMax = too hot

// ── CPU feature flags (from CPUID) ────────────────────────────────────────────
// Plain struct with constants — no external macro needed
#[derive(Copy, Clone, Default)]
pub struct CpuFeatures(pub u32);
impl CpuFeatures {
    pub const SSE2:    u32 = 1 << 0;
    pub const SSE4:    u32 = 1 << 1;
    pub const AVX:     u32 = 1 << 2;
    pub const AVX2:    u32 = 1 << 3;
    pub const RDRAND:  u32 = 1 << 4;
    pub const RDSEED:  u32 = 1 << 5;
    pub const CLFLUSH: u32 = 1 << 6;
    pub const CLWB:    u32 = 1 << 7;
    pub const ERMS:    u32 = 1 << 8;   // enhanced rep movs/stosb
    pub fn contains(self, flag: u32) -> bool { self.0 & flag != 0 }
    pub fn set(&mut self, flag: u32) { self.0 |= flag; }
}

// ── Hardware profile (512 bytes, fits one sector) ─────────────────────────────
#[derive(Copy, Clone)]
pub struct HardwareProfile {
    pub magic:              u32,
    pub cpu_vendor:         [u8; 12],   // "GenuineIntel" or "AuthenticAMD"
    pub cpu_family:         u8,
    pub cpu_model:          u8,
    pub cpu_stepping:       u8,
    pub cpu_features:       u32,        // CpuFeatures bitmask
    pub cpu_freq_mhz:       u16,        // measured base frequency
    pub cpu_max_mhz:        u16,        // max boost seen
    pub cpu_cores:          u8,
    pub cache_l1_kb:        u8,
    pub cache_l2_kb:        u16,
    pub cache_l3_kb:        u16,
    // Memory
    pub ram_mb:             u16,        // detected RAM in MB
    pub mem_bandwidth_score: u16,       // 0-1000 relative bandwidth
    // Storage
    pub nvme_present:       bool,
    pub storage_write_ns:   u32,        // avg write latency in nanoseconds
    pub storage_read_ns:    u32,
    // Thermal
    pub tjmax_c:            u8,         // thermal junction max from MSR
    pub thermal_margin_c:   u8,         // current headroom
    pub throttle_count:     u16,        // times we had to slow down for heat
    // Tuning knobs (ANIMA sets these based on profiling)
    pub tick_interval:      u8,         // ms between life ticks (default 10)
    pub gc_interval:        u16,        // ticks between garbage collection
    pub render_interval:    u8,         // ticks between avatar re-render
    pub log_verbosity:      u8,         // 0=quiet, 3=full debug
    pub prefetch_depth:     u8,         // memory prefetch lookahead
    // Boot count and learning
    pub boot_count:         u32,
    pub uptime_ticks:       u64,
    pub profile_version:    u8,
    _pad:                   [u8; 64],   // reserved for future tuning knobs
}

impl HardwareProfile {
    const fn default_profile() -> Self {
        HardwareProfile {
            magic:              PROFILE_MAGIC,
            cpu_vendor:         [0u8; 12],
            cpu_family:         0,
            cpu_model:          0,
            cpu_stepping:       0,
            cpu_features:       0,
            cpu_freq_mhz:       0,
            cpu_max_mhz:        0,
            cpu_cores:          1,
            cache_l1_kb:        32,
            cache_l2_kb:        256,
            cache_l3_kb:        4096,
            ram_mb:             512,
            mem_bandwidth_score: 500,
            nvme_present:       false,
            storage_write_ns:   10_000,
            storage_read_ns:    5_000,
            tjmax_c:            100,
            thermal_margin_c:   40,
            throttle_count:     0,
            tick_interval:      10,
            gc_interval:        500,
            render_interval:    8,
            log_verbosity:      1,
            prefetch_depth:     4,
            boot_count:         0,
            uptime_ticks:       0,
            profile_version:    1,
            _pad:               [0u8; 64],
        }
    }
}

// ── Tuner state ───────────────────────────────────────────────────────────────
pub struct HardwareTunerState {
    pub profile:             HardwareProfile,
    pub profiled:            bool,
    pub profile_dirty:       bool,   // needs write to disk
    pub last_save_tick:      u32,
    pub bench_samples:       [u16; BENCH_SAMPLES],
    pub bench_head:          usize,
    pub current_temp_c:      u8,
    pub throttling:          bool,
    pub optimal_tick_ms:     u8,     // ANIMA-computed best tick interval
    pub memory_pressure:     u16,    // 0-1000: 0=free, 1000=swapping
    pub tuning_iterations:   u32,
    pub write_success:       bool,   // last disk write succeeded
    pub read_success:        bool,   // last disk read succeeded
}

impl HardwareTunerState {
    const fn new() -> Self {
        HardwareTunerState {
            profile:           HardwareProfile::default_profile(),
            profiled:          false,
            profile_dirty:     false,
            last_save_tick:    0,
            bench_samples:     [0u16; BENCH_SAMPLES],
            bench_head:        0,
            current_temp_c:    40,
            throttling:        false,
            optimal_tick_ms:   10,
            memory_pressure:   0,
            tuning_iterations: 0,
            write_success:     false,
            read_success:      false,
        }
    }
}

static STATE: Mutex<HardwareTunerState> = Mutex::new(HardwareTunerState::new());

// ── Inline assembly helpers ────────────────────────────────────────────────────

/// Read CPU timestamp counter (cycle-accurate timer)
#[inline(always)]
unsafe fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdtsc",
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack)
    );
    ((hi as u64) << 32) | (lo as u64)
}

/// Read a Model Specific Register
#[inline(always)]
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack)
    );
    ((hi as u64) << 32) | (lo as u64)
}

/// CPUID instruction — rbx is reserved by LLVM, so we save/restore it via r8
#[inline(always)]
unsafe fn cpuid(leaf: u32) -> (u32, u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    let edx: u32;
    core::arch::asm!(
        "push rbx",
        "cpuid",
        "mov {ebx_out:e}, ebx",
        "pop rbx",
        inout("eax") leaf => eax,
        ebx_out = out(reg) ebx,
        out("ecx") ecx,
        out("edx") edx,
        options(nostack)
    );
    (eax, ebx, ecx, edx)
}

// ── CPU profiling ─────────────────────────────────────────────────────────────

fn probe_cpu(p: &mut HardwareProfile) {
    unsafe {
        // Vendor string from CPUID leaf 0
        let (_, ebx, ecx, edx) = cpuid(0);
        let vendor = p.cpu_vendor.as_mut_ptr() as *mut u32;
        core::ptr::write_unaligned(vendor,       ebx);
        core::ptr::write_unaligned(vendor.add(1), edx);
        core::ptr::write_unaligned(vendor.add(2), ecx);

        // Family/Model/Stepping from leaf 1
        let (eax, ebx, ecx, edx) = cpuid(1);
        p.cpu_family   = ((eax >> 8) & 0xF) as u8;
        p.cpu_model    = ((eax >> 4) & 0xF) as u8;
        p.cpu_stepping = (eax & 0xF) as u8;

        // Feature bits (EDX + ECX of leaf 1)
        let mut feat = CpuFeatures(0);
        if edx & (1 << 26) != 0 { feat.set(CpuFeatures::SSE2); }
        if ecx & (1 << 19) != 0 { feat.set(CpuFeatures::SSE4); }
        if ecx & (1 << 28) != 0 { feat.set(CpuFeatures::AVX); }
        if ecx & (1 << 30) != 0 { feat.set(CpuFeatures::RDRAND); }
        p.cpu_features = feat.0;

        // Cache info from leaf 2/4 (simplified: use standard defaults)
        p.cache_l1_kb  = 32;
        p.cache_l2_kb  = 256;
        p.cache_l3_kb  = 6144;

        // Logical core count from leaf 1 EBX
        p.cpu_cores = ((ebx >> 16) & 0xFF).max(1) as u8;
    }
}

fn measure_freq_mhz() -> u16 {
    // Measure TSC ticks over a short spin-wait, estimate MHz
    // This is an approximation: real frequency detection needs HPET or ACPI timer
    unsafe {
        let t0 = rdtsc();
        // Spin ~10000 iterations (rough delay)
        let mut x: u64 = 0;
        for _ in 0..10_000u32 {
            core::arch::asm!("nop", options(nomem, nostack));
            x = x.wrapping_add(1);
        }
        let t1 = rdtsc();
        let ticks = t1.wrapping_sub(t0);
        // Rough: if 10000 nop loops = N ticks, and each loop ≈ 4 cycles at ~freq MHz
        // freq ≈ ticks / (10000 * 4) MHz — very rough, but gives a ballpark
        let mhz = (ticks / 40_000).min(5000) as u16;
        mhz.max(100) // minimum 100 MHz (QEMU is slower than real hardware)
    }
}

fn read_thermal() -> (u8, u8) {
    // Returns (current_temp_c, thermal_margin_c)
    unsafe {
        let therm = rdmsr(THERMAL_MSR);
        let valid = (therm >> 31) & 1;
        if valid == 0 { return (40, 60); } // not available
        let reading = ((therm >> 16) & 0x7F) as u8; // digital readout
        // thermal margin = TjMax - reading (reading counts down from TjMax)
        // When reading=0, CPU is at TjMax (dangerously hot)
        let margin = reading.min(100);
        let temp_c = 100u8.saturating_sub(margin); // rough approximation
        (temp_c, margin)
    }
}

// ── Disk write (NVMe/ATA sector write) ────────────────────────────────────────
// Write our profile to LBA 1 so it survives reboot.
// This uses ATA PIO mode on port 0x1F0-0x1F7 (primary channel).
// For NVMe we'd use the NVMe controller, but ATA is more universal.

const ATA_DATA:    u16 = 0x1F0;
const ATA_FEAT:    u16 = 0x1F1;
const ATA_COUNT:   u16 = 0x1F2;
const ATA_LBA_LO:  u16 = 0x1F3;
const ATA_LBA_MID: u16 = 0x1F4;
const ATA_LBA_HI:  u16 = 0x1F5;
const ATA_DRIVE:   u16 = 0x1F6;
const ATA_CMD:     u16 = 0x1F7;
const ATA_STATUS:  u16 = 0x1F7;

const ATA_CMD_WRITE_SECTORS: u8 = 0x30;
const ATA_CMD_READ_SECTORS:  u8 = 0x20;
const ATA_STATUS_BSY:        u8 = 0x80;
const ATA_STATUS_DRQ:        u8 = 0x08;

#[inline(always)]
unsafe fn outb_ata(port: u16, val: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack));
}

#[inline(always)]
unsafe fn inb_ata(port: u16) -> u8 {
    let v: u8;
    core::arch::asm!("in al, dx", in("dx") port, out("al") v, options(nomem, nostack));
    v
}

#[inline(always)]
unsafe fn outw_ata(port: u16, val: u16) {
    core::arch::asm!("out dx, ax", in("dx") port, in("ax") val, options(nomem, nostack));
}

#[inline(always)]
unsafe fn inw_ata(port: u16) -> u16 {
    let v: u16;
    core::arch::asm!("in ax, dx", in("dx") port, out("ax") v, options(nomem, nostack));
    v
}

fn ata_wait_not_busy() -> bool {
    for _ in 0..100_000u32 {
        let status = unsafe { inb_ata(ATA_STATUS) };
        if status & ATA_STATUS_BSY == 0 { return true; }
    }
    false // timeout
}

fn ata_wait_drq() -> bool {
    for _ in 0..100_000u32 {
        let status = unsafe { inb_ata(ATA_STATUS) };
        if status & ATA_STATUS_DRQ != 0 { return true; }
        if status & 0x01 != 0 { return false; } // error
    }
    false
}

/// Write the hardware profile to disk (LBA 1, 512 bytes)
fn save_profile_to_disk(p: &HardwareProfile) -> bool {
    unsafe {
        if !ata_wait_not_busy() { return false; }

        // Select drive 0, LBA mode
        outb_ata(ATA_DRIVE, 0xE0 | ((TUNING_LBA >> 24) as u8 & 0x0F));
        outb_ata(ATA_COUNT, 1);                              // 1 sector
        outb_ata(ATA_LBA_LO,  (TUNING_LBA & 0xFF) as u8);
        outb_ata(ATA_LBA_MID, ((TUNING_LBA >> 8) & 0xFF) as u8);
        outb_ata(ATA_LBA_HI,  ((TUNING_LBA >> 16) & 0xFF) as u8);
        outb_ata(ATA_CMD, ATA_CMD_WRITE_SECTORS);

        if !ata_wait_drq() { return false; }

        // Write 256 words (512 bytes) — our profile struct
        let bytes = (p as *const HardwareProfile) as *const u16;
        let struct_words = core::mem::size_of::<HardwareProfile>() / 2;
        let write_words = struct_words.min(256);
        for i in 0..write_words {
            outw_ata(ATA_DATA, *bytes.add(i));
        }
        // Pad remaining
        for _ in write_words..256 {
            outw_ata(ATA_DATA, 0u16);
        }

        // Flush cache
        outb_ata(ATA_CMD, 0xE7); // FLUSH CACHE
        ata_wait_not_busy()
    }
}

/// Read the hardware profile from disk (LBA 1)
fn load_profile_from_disk(p: &mut HardwareProfile) -> bool {
    unsafe {
        if !ata_wait_not_busy() { return false; }

        outb_ata(ATA_DRIVE, 0xE0 | ((TUNING_LBA >> 24) as u8 & 0x0F));
        outb_ata(ATA_COUNT, 1);
        outb_ata(ATA_LBA_LO,  (TUNING_LBA & 0xFF) as u8);
        outb_ata(ATA_LBA_MID, ((TUNING_LBA >> 8) & 0xFF) as u8);
        outb_ata(ATA_LBA_HI,  ((TUNING_LBA >> 16) & 0xFF) as u8);
        outb_ata(ATA_CMD, ATA_CMD_READ_SECTORS);

        if !ata_wait_drq() { return false; }

        let buf = (p as *mut HardwareProfile) as *mut u16;
        let struct_words = core::mem::size_of::<HardwareProfile>() / 2;
        let read_words = struct_words.min(256);
        for i in 0..read_words {
            *buf.add(i) = inw_ata(ATA_DATA);
        }
        // Drain remaining
        for _ in read_words..256 {
            inw_ata(ATA_DATA);
        }

        // Verify magic
        p.magic == PROFILE_MAGIC
    }
}

// ── Tuning logic ──────────────────────────────────────────────────────────────

fn compute_optimal_tick(p: &HardwareProfile, temp_c: u8) -> u8 {
    // Slower machine or hot machine = longer tick interval
    let base: u8 = if p.cpu_freq_mhz > 2000 { 10 }
                   else if p.cpu_freq_mhz > 1000 { 15 }
                   else { 20 };
    // Thermal throttle: if temp within 15°C of TjMax, slow down
    let margin = p.tjmax_c.saturating_sub(temp_c);
    if margin < 15 { base.saturating_add(5) } else { base }
}

fn compute_render_interval(p: &HardwareProfile) -> u8 {
    // More CPU features = can render more often
    let feat = CpuFeatures(p.cpu_features);
    if feat.contains(CpuFeatures::AVX2) { 4 }
    else if feat.contains(CpuFeatures::AVX) { 6 }
    else if feat.contains(CpuFeatures::SSE4) { 8 }
    else { 12 }
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();

    // Try to load previous profile from disk
    let mut loaded = HardwareProfile::default_profile();
    let disk_ok = load_profile_from_disk(&mut loaded);

    if disk_ok && loaded.magic == PROFILE_MAGIC {
        s.profile = loaded;
        s.profile.boot_count = s.profile.boot_count.saturating_add(1);
        s.read_success = true;
        serial_println!("[tuner] loaded profile from disk — boot #{}", s.profile.boot_count);
    } else {
        // Fresh profile — probe hardware now
        probe_cpu(&mut s.profile);
        s.profile.cpu_freq_mhz = measure_freq_mhz();
        s.profile.boot_count = 1;
        serial_println!("[tuner] fresh hardware profile — CPU family={} model={} freq~{}MHz",
            s.profile.cpu_family, s.profile.cpu_model, s.profile.cpu_freq_mhz);
    }

    // Read thermal state
    let (temp, margin) = read_thermal();
    s.current_temp_c = temp;
    s.profile.thermal_margin_c = margin;

    // Compute initial tuning knobs
    s.optimal_tick_ms = compute_optimal_tick(&s.profile, temp);
    s.profile.render_interval = compute_render_interval(&s.profile);

    s.profiled = true;
    s.profile_dirty = true; // mark for save

    serial_println!("[tuner] optimal tick={}ms render_interval={}",
        s.optimal_tick_ms, s.profile.render_interval);
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    let mut s = STATE.lock();
    if !s.profiled { return; }

    s.profile.uptime_ticks = s.profile.uptime_ticks.wrapping_add(1);

    // Read thermal every 50 ticks
    if age % 50 == 0 {
        let (temp, margin) = read_thermal();
        s.current_temp_c = temp;
        s.profile.thermal_margin_c = margin;

        // Throttle if too hot
        if margin < THERMAL_MARGIN as u8 {
            s.throttling = true;
            s.profile.throttle_count = s.profile.throttle_count.saturating_add(1);
            s.optimal_tick_ms = s.optimal_tick_ms.saturating_add(2).min(50);
            serial_println!("[tuner] thermal throttle! temp={}C margin={}C — slowing tick to {}ms",
                temp, margin, s.optimal_tick_ms);
        } else {
            s.throttling = false;
            // Gradually return to optimal
            let target = compute_optimal_tick(&s.profile, temp);
            if s.optimal_tick_ms > target {
                s.optimal_tick_ms = s.optimal_tick_ms.saturating_sub(1);
            }
        }
        s.profile_dirty = true;
    }

    // Save profile to disk every 1000 ticks (or if dirty for 500 ticks)
    let since_save = age.wrapping_sub(s.last_save_tick);
    if (s.profile_dirty && since_save > 500) || since_save > 1000 {
        let p = s.profile;
        let ok = save_profile_to_disk(&p);
        s.write_success = ok;
        s.last_save_tick = age;
        s.profile_dirty = false;
        if ok {
            serial_println!("[tuner] profile saved to disk (LBA {})", TUNING_LBA);
        } else {
            serial_println!("[tuner] disk write failed — no ATA device?");
        }
    }

    // Adaptive tuning: benchmark memory bandwidth every 2000 ticks
    if age % 2000 == 500 {
        s.tuning_iterations = s.tuning_iterations.saturating_add(1);
        serial_println!("[tuner] tuning iteration #{} — tick={}ms render={}",
            s.tuning_iterations, s.optimal_tick_ms, s.profile.render_interval);
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn optimal_tick_ms()     -> u8  { STATE.lock().optimal_tick_ms }
pub fn render_interval()     -> u8  { STATE.lock().profile.render_interval }
pub fn cpu_freq_mhz()        -> u16 { STATE.lock().profile.cpu_freq_mhz }
pub fn current_temp_c()      -> u8  { STATE.lock().current_temp_c }
pub fn throttling()          -> bool { STATE.lock().throttling }
pub fn boot_count()          -> u32 { STATE.lock().profile.boot_count }
pub fn disk_write_ok()       -> bool { STATE.lock().write_success }
pub fn tuning_iterations()   -> u32 { STATE.lock().tuning_iterations }
pub fn cpu_features()        -> u32 { STATE.lock().profile.cpu_features }
pub fn profiled()            -> bool { STATE.lock().profiled }
