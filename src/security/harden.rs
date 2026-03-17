/// CPU and kernel hardening for Genesis
///
/// Hardware security features:
///   - NX bit (No-Execute) enforcement on data pages
///   - SMEP (Supervisor Mode Execution Prevention) — kernel can't exec user pages
///   - SMAP (Supervisor Mode Access Prevention) — kernel can't read user pages
///   - W^X enforcement — pages are writable XOR executable, never both
///   - Stack canaries — detect stack buffer overflows
///   - CR0 write-protect — prevent kernel from writing read-only pages
///   - UMIP (User-Mode Instruction Prevention) — block SGDT/SIDT from userspace
///
/// All code is original.
use crate::serial_println;
use crate::sync::Mutex;

/// Hardening status
static HARDEN_STATE: Mutex<HardenState> = Mutex::new(HardenState::new());

/// CPU feature flags relevant to security
#[derive(Debug, Clone, Copy)]
pub struct CpuSecurityFeatures {
    pub nx: bool,        // No-Execute bit (AMD: NX, Intel: XD)
    pub smep: bool,      // Supervisor Mode Execution Prevention
    pub smap: bool,      // Supervisor Mode Access Prevention
    pub umip: bool,      // User-Mode Instruction Prevention
    pub rdrand: bool,    // Hardware RNG
    pub rdseed: bool,    // Hardware RNG seeding
    pub aes_ni: bool,    // AES hardware acceleration
    pub sha_ext: bool,   // SHA hardware acceleration
    pub ibrs: bool,      // Indirect Branch Restricted Speculation (Spectre)
    pub ibpb: bool,      // Indirect Branch Predictor Barrier
    pub stibp: bool,     // Single Thread Indirect Branch Predictors
    pub ssbd: bool,      // Speculative Store Bypass Disable
    pub mds_clear: bool, // MDS buffer clearing support
}

impl CpuSecurityFeatures {
    pub const fn empty() -> Self {
        CpuSecurityFeatures {
            nx: false,
            smep: false,
            smap: false,
            umip: false,
            rdrand: false,
            rdseed: false,
            aes_ni: false,
            sha_ext: false,
            ibrs: false,
            ibpb: false,
            stibp: false,
            ssbd: false,
            mds_clear: false,
        }
    }
}

/// Stack canary value — set during boot from CSPRNG
static STACK_CANARY: Mutex<u64> = Mutex::new(0);

/// Canary magic for detecting corruption (set from CSPRNG at boot)
const CANARY_POISON: u64 = 0xDEAD_BEEF_CAFE_BABE;

/// Hardening state
pub struct HardenState {
    pub cpu_features: CpuSecurityFeatures,
    pub wx_enforced: bool,
    pub smep_enabled: bool,
    pub smap_enabled: bool,
    pub nx_enabled: bool,
    pub umip_enabled: bool,
    pub cr0_wp: bool,
    pub spectre_mitigated: bool,
    pub mds_mitigated: bool,
    pub stack_canary_active: bool,
    pub hardening_level: HardenLevel,
}

/// Hardening levels
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardenLevel {
    /// Minimal — basic NX and W^X
    Minimal,
    /// Standard — NX, W^X, SMEP, SMAP, stack canaries
    Standard,
    /// Maximum — all mitigations including Spectre/MDS
    Maximum,
    /// Paranoid — maximum + kernel lockdown + no debug
    Paranoid,
}

impl HardenState {
    const fn new() -> Self {
        HardenState {
            cpu_features: CpuSecurityFeatures::empty(),
            wx_enforced: false,
            smep_enabled: false,
            smap_enabled: false,
            nx_enabled: false,
            umip_enabled: false,
            cr0_wp: false,
            spectre_mitigated: false,
            mds_mitigated: false,
            stack_canary_active: false,
            hardening_level: HardenLevel::Standard,
        }
    }
}

/// Detect CPU security features via CPUID
pub fn detect_cpu_features() -> CpuSecurityFeatures {
    let mut features = CpuSecurityFeatures::empty();

    // CPUID leaf 1, ECX/EDX
    let cpuid1_ecx: u32;
    let cpuid1_edx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 1",
            "cpuid",
            "pop rbx",
            out("ecx") cpuid1_ecx,
            out("edx") cpuid1_edx,
            lateout("eax") _,
        );
    }

    features.aes_ni = (cpuid1_ecx & (1 << 25)) != 0;
    features.rdrand = (cpuid1_ecx & (1 << 30)) != 0;

    // CPUID leaf 7, EBX/ECX/EDX (structured extended features)
    let cpuid7_ebx: u32;
    let cpuid7_ecx: u32;
    let cpuid7_edx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 7",
            "xor ecx, ecx",
            "cpuid",
            "mov {0:e}, ebx",
            "pop rbx",
            out(reg) cpuid7_ebx,
            out("ecx") cpuid7_ecx,
            out("edx") cpuid7_edx,
            lateout("eax") _,
        );
    }

    features.smep = (cpuid7_ebx & (1 << 7)) != 0;
    features.smap = (cpuid7_ebx & (1 << 20)) != 0;
    features.rdseed = (cpuid7_ebx & (1 << 18)) != 0;
    features.sha_ext = (cpuid7_ebx & (1 << 29)) != 0;
    features.umip = (cpuid7_ecx & (1 << 2)) != 0;
    features.ibrs = (cpuid7_edx & (1 << 26)) != 0;
    features.stibp = (cpuid7_edx & (1 << 27)) != 0;
    features.ssbd = (cpuid7_edx & (1 << 31)) != 0;
    features.mds_clear = (cpuid7_edx & (1 << 10)) != 0;

    // Extended CPUID leaf 0x80000001, EDX for NX bit
    let ext_edx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 0x80000001",
            "cpuid",
            "pop rbx",
            out("edx") ext_edx,
            lateout("eax") _,
            lateout("ecx") _,
        );
    }
    features.nx = (ext_edx & (1 << 20)) != 0;

    features
}

/// Enable NX bit via IA32_EFER MSR
fn enable_nx() {
    unsafe {
        let low: u32;
        let high: u32;
        core::arch::asm!("rdmsr", in("ecx") 0xC0000080u32, out("eax") low, out("edx") high);
        let efer = ((high as u64) << 32) | (low as u64);
        let new_efer = efer | (1 << 11); // Set NXE bit
        let new_low = (new_efer & 0xFFFFFFFF) as u32;
        let new_high = ((new_efer >> 32) & 0xFFFFFFFF) as u32;
        core::arch::asm!("wrmsr", in("ecx") 0xC0000080u32, in("eax") new_low, in("edx") new_high);
    }
}

/// Enable SMEP via CR4
fn enable_smep() {
    unsafe {
        let cr4: u64;
        core::arch::asm!("mov {}, cr4", out(reg) cr4);
        let new_cr4 = cr4 | (1 << 20); // SMEP bit
        core::arch::asm!("mov cr4, {}", in(reg) new_cr4);
    }
}

/// Enable SMAP via CR4
fn enable_smap() {
    unsafe {
        let cr4: u64;
        core::arch::asm!("mov {}, cr4", out(reg) cr4);
        let new_cr4 = cr4 | (1 << 21); // SMAP bit
        core::arch::asm!("mov cr4, {}", in(reg) new_cr4);
    }
}

/// Enable UMIP via CR4
fn enable_umip() {
    unsafe {
        let cr4: u64;
        core::arch::asm!("mov {}, cr4", out(reg) cr4);
        let new_cr4 = cr4 | (1 << 11); // UMIP bit
        core::arch::asm!("mov cr4, {}", in(reg) new_cr4);
    }
}

/// Enable CR0 write-protect (prevent kernel from writing read-only pages)
fn enable_cr0_wp() {
    unsafe {
        let cr0: u64;
        core::arch::asm!("mov {}, cr0", out(reg) cr0);
        let new_cr0 = cr0 | (1 << 16); // WP bit
        core::arch::asm!("mov cr0, {}", in(reg) new_cr0);
    }
}

/// Initialize stack canary from CSPRNG
fn init_stack_canary() {
    let mut canary_bytes = [0u8; 8];
    crate::crypto::random::fill_bytes(&mut canary_bytes);
    let mut canary = 0u64;
    for i in 0..8 {
        canary |= (canary_bytes[i] as u64) << (i * 8);
    }
    // Ensure canary is never zero (zero could mask bugs)
    if canary == 0 {
        canary = CANARY_POISON;
    }
    *STACK_CANARY.lock() = canary;
}

/// Get the current stack canary value
pub fn get_stack_canary() -> u64 {
    *STACK_CANARY.lock()
}

/// Verify a stack canary value (call at function epilogue)
pub fn verify_canary(value: u64) -> bool {
    value == *STACK_CANARY.lock()
}

/// Enable Spectre v2 mitigation (IBRS)
fn mitigate_spectre() {
    // Set IBRS via IA32_SPEC_CTRL MSR (0x48)
    unsafe {
        core::arch::asm!("wrmsr", in("ecx") 0x48u32, in("eax") 1u32, in("edx") 0u32);
    }
}

/// Flush MDS buffers (VERW-based mitigation)
pub fn mds_clear() {
    unsafe {
        // VERW on a valid data segment selector flushes MDS buffers
        let ds: u16 = 0;
        core::arch::asm!("verw {0:x}", in(reg) ds);
    }
}

/// W^X policy check — verify a page is not both writable and executable
pub fn check_wx(flags: u64) -> bool {
    let writable = (flags & (1 << 1)) != 0; // Page table WRITE bit
    let executable = (flags & (1 << 63)) == 0; // NX bit NOT set = executable
                                               // W^X: must not be both writable AND executable
    !(writable && executable)
}

/// Full hardening initialization
pub fn init(level: HardenLevel) {
    let features = detect_cpu_features();
    let mut state = HARDEN_STATE.lock();
    state.cpu_features = features;
    state.hardening_level = level;

    serial_println!("  [harden] CPU security features detected:");
    serial_println!(
        "    NX={} SMEP={} SMAP={} UMIP={} RDRAND={} RDSEED={}",
        features.nx,
        features.smep,
        features.smap,
        features.umip,
        features.rdrand,
        features.rdseed
    );
    serial_println!(
        "    AES-NI={} SHA={} IBRS={} STIBP={} SSBD={} MDS={}",
        features.aes_ni,
        features.sha_ext,
        features.ibrs,
        features.stibp,
        features.ssbd,
        features.mds_clear
    );

    // Enable NX bit
    if features.nx {
        enable_nx();
        state.nx_enabled = true;
        serial_println!("  [harden] NX (No-Execute) enabled");
    }

    // Enable CR0 write-protect
    enable_cr0_wp();
    state.cr0_wp = true;
    serial_println!("  [harden] CR0.WP (write-protect) enabled");

    // W^X enforcement
    state.wx_enforced = features.nx;
    if state.wx_enforced {
        serial_println!("  [harden] W^X policy enforced (writable XOR executable)");
    }

    // SMEP
    if features.smep {
        enable_smep();
        state.smep_enabled = true;
        serial_println!("  [harden] SMEP enabled (kernel can't exec user pages)");
    }

    // SMAP
    if features.smap
        && matches!(
            level,
            HardenLevel::Standard | HardenLevel::Maximum | HardenLevel::Paranoid
        )
    {
        enable_smap();
        state.smap_enabled = true;
        serial_println!("  [harden] SMAP enabled (kernel can't access user pages)");
    }

    // UMIP
    if features.umip && matches!(level, HardenLevel::Maximum | HardenLevel::Paranoid) {
        enable_umip();
        state.umip_enabled = true;
        serial_println!("  [harden] UMIP enabled (SGDT/SIDT blocked from userspace)");
    }

    // Stack canary
    init_stack_canary();
    state.stack_canary_active = true;
    serial_println!("  [harden] Stack canary initialized from CSPRNG");

    // Spectre mitigations
    if features.ibrs && matches!(level, HardenLevel::Maximum | HardenLevel::Paranoid) {
        mitigate_spectre();
        state.spectre_mitigated = true;
        serial_println!("  [harden] Spectre v2 mitigated (IBRS)");
    }

    // MDS mitigation
    if features.mds_clear && matches!(level, HardenLevel::Maximum | HardenLevel::Paranoid) {
        state.mds_mitigated = true;
        serial_println!("  [harden] MDS mitigation active (VERW clearing)");
    }

    serial_println!("  [harden] Hardening level: {:?}", level);
}

/// Get current hardening status
pub fn status() -> HardenLevel {
    HARDEN_STATE.lock().hardening_level
}

/// Check if a specific feature is enabled
pub fn is_enabled(feature: &str) -> bool {
    let state = HARDEN_STATE.lock();
    match feature {
        "nx" => state.nx_enabled,
        "smep" => state.smep_enabled,
        "smap" => state.smap_enabled,
        "umip" => state.umip_enabled,
        "wx" => state.wx_enforced,
        "canary" => state.stack_canary_active,
        "spectre" => state.spectre_mitigated,
        "mds" => state.mds_mitigated,
        _ => false,
    }
}
