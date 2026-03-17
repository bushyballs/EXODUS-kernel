/// Process credentials — uid, gid, supplementary groups, capabilities.
///
/// Part of the AIOS kernel.
use alloc::vec::Vec;

/// POSIX capability bits.
pub type CapabilitySet = u64;

/// Full credential set for a process.
pub struct Credentials {
    pub uid: u32,
    pub gid: u32,
    pub euid: u32,
    pub egid: u32,
    pub suid: u32,
    pub sgid: u32,
    pub supplementary_groups: Vec<u32>,
    pub capabilities: CapabilitySet,
}

impl Credentials {
    /// Root credentials (uid/gid 0, all capabilities).
    pub fn root() -> Self {
        Credentials {
            uid: 0,
            gid: 0,
            euid: 0,
            egid: 0,
            suid: 0,
            sgid: 0,
            supplementary_groups: Vec::new(),
            capabilities: u64::MAX,
        }
    }

    /// Unprivileged user credentials (no capabilities).
    pub fn user(uid: u32, gid: u32) -> Self {
        Credentials {
            uid,
            gid,
            euid: uid,
            egid: gid,
            suid: uid,
            sgid: gid,
            supplementary_groups: Vec::new(),
            capabilities: 0,
        }
    }

    pub fn is_root(&self) -> bool {
        self.euid == 0
    }
}

/// Initialize the credential subsystem.
pub fn init() {
    // TODO: Set up capability definitions
}
