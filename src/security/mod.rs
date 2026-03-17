/// Hoags Security — capability-based security + mandatory access control
///
/// Security model:
///   1. Capability-based: processes hold unforgeable tokens for resources
///      PLUS Linux-model POSIX capability bits (caps::CAP_* constants +
///      caps::CapabilitySet with permitted/effective/inheritable/ambient/bounding).
///   2. Mandatory Access Control (MAC): system-wide policy enforcement
///   3. Users & groups: Unix-style UID/GID for file permissions
///   4. Sandboxing: processes can drop privileges, never gain them
///   5. LSM hook layer: composable security module stack (lsm.rs)
///   6. Kernel audit log: rich event ring buffer (audit.rs)
///   7. KASLR: kernel address-space randomization (kaslr.rs)
///   8. Stack canary: TSC+RDRAND-seeded stack protection (stack_protect.rs)
///
/// Inspired by: seL4 (capabilities), SELinux (MAC), Capsicum (sandboxing),
/// Plan 9 (per-process namespaces), Linux security modules. All code is original.
use crate::{serial_print, serial_println};
pub mod ai_security;
pub mod aslr;
pub mod audit;
pub mod caps;
pub mod dm_verity;
pub mod harden;
pub mod ima;
pub mod integrity;
pub mod ipsec;
pub mod keystore;
pub mod landlock;
pub mod lockdown;
pub mod mac;
pub mod safe_stack;
pub mod sandbox;
pub mod seccomp;
pub mod secureboot;
pub mod smack;
pub mod tomoyo;
pub mod tpm;
pub mod users;
pub mod yama;
// New security subsystems
pub mod capabilities;
pub mod genesis_mac;
pub mod kaslr;
pub mod lsm;
pub mod stack_protect;
// Kernel hardening mitigations
pub mod kptr;
pub mod smep_smap;
pub mod stack_canary;

/// Initialize the core security subsystem (capability tables, MAC, audit).
pub fn init() {
    audit::init(); // audit must be first — other modules log into it
    users::init();
    caps::init();
    mac::init();
    genesis_mac::init(); // load default Genesis MAC deny policy
    capabilities::init(); // initialize per-process capability table
    lsm::init(); // mark LSM framework ready (enforcing)
    serial_println!("  Security: capability + MAC + LSM hook layer initialized");
}

/// Initialize all hardening subsystems (called after crypto is ready).
pub fn harden() {
    // SMEP/SMAP/UMIP must be first — enforces CPU memory protection before
    // any other subsystem can accidentally execute/read user memory.
    smep_smap::init();

    // Stack protection: canary before any task stacks are created.
    serial_println!("  [security] entering stack_canary::init");
    stack_canary::init();
    serial_println!("  [security] entering stack_protect::init");
    stack_protect::init(); // additional safe-stack / shadow-stack layer
    serial_println!("  [security] entering kptr::init");

    // Kernel pointer restriction: suppress address leaks from this point on.
    kptr::init();

    // KASLR: seed and log the randomization offset.
    kaslr::init();

    harden::init(harden::HardenLevel::Standard);
    lockdown::init(lockdown::LockdownMode::Integrity);
    aslr::init();
    sandbox::init();
    secureboot::init(secureboot::EnforcementMode::Audit);
    integrity::init(integrity::ViolationPolicy::Alert);
    seccomp::init();
    keystore::init();
    ai_security::init();
    landlock::init();
    serial_println!(
        "  Security: hardening complete (SMEP+SMAP+UMIP + canary + KASLR + kptr + AI + Landlock)"
    );
}
