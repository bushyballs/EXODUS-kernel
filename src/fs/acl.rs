use crate::serial_println;
use crate::sync::Mutex;
/// POSIX Access Control Lists (ACLs)
///
/// Part of the AIOS filesystem layer.
///
/// Implements POSIX.1e-style ACLs with user, group, mask, and other
/// entries. ACLs extend the basic Unix permission model by allowing
/// fine-grained per-user and per-group permissions on individual files.
///
/// Design:
///   - An Acl is a Vec of AclEntry values.
///   - Permission checking follows the POSIX.1e algorithm:
///     1. If the process is the file owner, use UserObj permissions.
///     2. If there is a named User entry matching the uid, use it (masked).
///     3. If the process gid or supplementary groups match GroupObj or
///        a named Group entry, use the union (masked).
///     4. Fall through to Other.
///   - A global default ACL can be stored per-directory for inheritance.
///   - ACLs are stored in extended attributes (system.posix_acl_access,
///     system.posix_acl_default) but this module handles the in-memory
///     representation and checking logic.
///
/// Inspired by: Linux POSIX ACLs (fs/posix_acl.c). All code is original.
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Permission bits
// ---------------------------------------------------------------------------

pub const ACL_READ: u16 = 0o4;
pub const ACL_WRITE: u16 = 0o2;
pub const ACL_EXECUTE: u16 = 0o1;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Tag type identifying what the ACL entry applies to.
#[derive(Clone, Copy, PartialEq)]
pub enum AclTag {
    /// Permissions for the file owner.
    UserObj,
    /// Permissions for a specific named user.
    User(u32),
    /// Permissions for the file's owning group.
    GroupObj,
    /// Permissions for a specific named group.
    Group(u32),
    /// Maximum permissions granted by User/Group entries (the mask).
    Mask,
    /// Permissions for everyone else.
    Other,
}

/// A single ACL entry.
#[derive(Clone)]
pub struct AclEntry {
    pub tag: AclTag,
    pub perms: u16,
}

/// A complete ACL for a file or directory.
#[derive(Clone)]
pub struct Acl {
    pub entries: Vec<AclEntry>,
}

/// Per-inode ACL storage (inode_nr -> access ACL, default ACL).
struct AclStore {
    /// (inode_nr, access_acl, optional default_acl for directories)
    items: Vec<(u64, Acl, Option<Acl>)>,
}

// ---------------------------------------------------------------------------
// Acl implementation
// ---------------------------------------------------------------------------

impl Acl {
    /// Create a new empty ACL.
    pub fn new() -> Self {
        Acl {
            entries: Vec::new(),
        }
    }

    /// Create an ACL from traditional Unix mode bits.
    pub fn from_mode(mode: u32) -> Self {
        let mut entries = Vec::new();
        entries.push(AclEntry {
            tag: AclTag::UserObj,
            perms: ((mode >> 6) & 0o7) as u16,
        });
        entries.push(AclEntry {
            tag: AclTag::GroupObj,
            perms: ((mode >> 3) & 0o7) as u16,
        });
        entries.push(AclEntry {
            tag: AclTag::Other,
            perms: (mode & 0o7) as u16,
        });
        Acl { entries }
    }

    /// Add an entry. If a duplicate tag exists (for UserObj/GroupObj/Mask/Other)
    /// it is replaced. Named User/Group entries accumulate.
    pub fn add_entry(&mut self, entry: AclEntry) {
        // For singleton tags, replace if present
        match entry.tag {
            AclTag::UserObj | AclTag::GroupObj | AclTag::Mask | AclTag::Other => {
                for existing in self.entries.iter_mut() {
                    if core::mem::discriminant(&existing.tag) == core::mem::discriminant(&entry.tag)
                        && match (&existing.tag, &entry.tag) {
                            (AclTag::UserObj, AclTag::UserObj) => true,
                            (AclTag::GroupObj, AclTag::GroupObj) => true,
                            (AclTag::Mask, AclTag::Mask) => true,
                            (AclTag::Other, AclTag::Other) => true,
                            _ => false,
                        }
                    {
                        existing.perms = entry.perms;
                        return;
                    }
                }
            }
            AclTag::User(uid) => {
                for existing in self.entries.iter_mut() {
                    if let AclTag::User(eu) = existing.tag {
                        if eu == uid {
                            existing.perms = entry.perms;
                            return;
                        }
                    }
                }
            }
            AclTag::Group(gid) => {
                for existing in self.entries.iter_mut() {
                    if let AclTag::Group(eg) = existing.tag {
                        if eg == gid {
                            existing.perms = entry.perms;
                            return;
                        }
                    }
                }
            }
        }
        self.entries.push(entry);
    }

    /// Get the mask value (if present). Returns 0o7 if no mask entry.
    fn mask_perms(&self) -> u16 {
        for e in self.entries.iter() {
            if let AclTag::Mask = e.tag {
                return e.perms;
            }
        }
        0o7 // No mask = all bits
    }

    /// Find UserObj permissions.
    fn user_obj_perms(&self) -> u16 {
        for e in self.entries.iter() {
            if let AclTag::UserObj = e.tag {
                return e.perms;
            }
        }
        0
    }

    /// Find Other permissions.
    fn other_perms(&self) -> u16 {
        for e in self.entries.iter() {
            if let AclTag::Other = e.tag {
                return e.perms;
            }
        }
        0
    }

    /// Check access for a process with the given uid, gid, and supplementary
    /// groups. `requested` is a bitmask of ACL_READ | ACL_WRITE | ACL_EXECUTE.
    /// Returns true if access is granted.
    pub fn check_access(
        &self,
        uid: u32,
        gid: u32,
        supplementary_groups: &[u32],
        owner_uid: u32,
        owner_gid: u32,
        requested: u16,
    ) -> bool {
        // Step 1: If the process is root (uid 0), allow everything.
        if uid == 0 {
            return true;
        }

        // Step 2: If the process is the file owner, check UserObj.
        if uid == owner_uid {
            return (self.user_obj_perms() & requested) == requested;
        }

        let mask = self.mask_perms();

        // Step 3: Check named User entries.
        for e in self.entries.iter() {
            if let AclTag::User(u) = e.tag {
                if u == uid {
                    return (e.perms & mask & requested) == requested;
                }
            }
        }

        // Step 4: Check GroupObj and named Group entries.
        let mut group_matched = false;
        let mut group_perms: u16 = 0;

        // Check owning group
        if gid == owner_gid || supplementary_groups.contains(&owner_gid) {
            for e in self.entries.iter() {
                if let AclTag::GroupObj = e.tag {
                    group_perms |= e.perms;
                    group_matched = true;
                }
            }
        }

        // Check named groups
        for e in self.entries.iter() {
            if let AclTag::Group(g) = e.tag {
                if gid == g || supplementary_groups.contains(&g) {
                    group_perms |= e.perms;
                    group_matched = true;
                }
            }
        }

        if group_matched {
            return (group_perms & mask & requested) == requested;
        }

        // Step 5: Fall through to Other.
        (self.other_perms() & requested) == requested
    }

    /// Convert this ACL back to a Unix mode (approximate: uses UserObj,
    /// GroupObj/Mask, Other).
    pub fn to_mode(&self) -> u32 {
        let u = self.user_obj_perms() as u32;
        let g = self.mask_perms() as u32; // mask represents group in ls output
        let o = self.other_perms() as u32;
        (u << 6) | (g << 3) | o
    }

    /// Validate the ACL: must have exactly one UserObj, GroupObj, and Other.
    pub fn is_valid(&self) -> bool {
        let mut has_user_obj = false;
        let mut has_group_obj = false;
        let mut has_other = false;
        let mut has_named = false;
        let mut has_mask = false;

        for e in self.entries.iter() {
            match e.tag {
                AclTag::UserObj => {
                    if has_user_obj {
                        return false;
                    }
                    has_user_obj = true;
                }
                AclTag::GroupObj => {
                    if has_group_obj {
                        return false;
                    }
                    has_group_obj = true;
                }
                AclTag::Other => {
                    if has_other {
                        return false;
                    }
                    has_other = true;
                }
                AclTag::Mask => {
                    if has_mask {
                        return false;
                    }
                    has_mask = true;
                }
                AclTag::User(_) | AclTag::Group(_) => {
                    has_named = true;
                }
            }
        }

        // Must have the three base entries
        if !has_user_obj || !has_group_obj || !has_other {
            return false;
        }
        // If named entries exist, mask is required
        if has_named && !has_mask {
            return false;
        }
        true
    }
}

// ---------------------------------------------------------------------------
// AclStore implementation
// ---------------------------------------------------------------------------

impl AclStore {
    fn new() -> Self {
        AclStore { items: Vec::new() }
    }

    fn get_access(&self, ino: u64) -> Option<&Acl> {
        self.items
            .iter()
            .find(|(i, _, _)| *i == ino)
            .map(|(_, acl, _)| acl)
    }

    fn get_default(&self, ino: u64) -> Option<&Acl> {
        self.items
            .iter()
            .find(|(i, _, _)| *i == ino)
            .and_then(|(_, _, d)| d.as_ref())
    }

    fn set_access(&mut self, ino: u64, acl: Acl) {
        for item in self.items.iter_mut() {
            if item.0 == ino {
                item.1 = acl;
                return;
            }
        }
        self.items.push((ino, acl, None));
    }

    fn set_default(&mut self, ino: u64, acl: Acl) {
        for item in self.items.iter_mut() {
            if item.0 == ino {
                item.2 = Some(acl);
                return;
            }
        }
        // Create with a minimal access ACL if not present
        self.items.push((ino, Acl::from_mode(0o755), Some(acl)));
    }

    fn remove(&mut self, ino: u64) {
        self.items.retain(|(i, _, _)| *i != ino);
    }
}

// ---------------------------------------------------------------------------
// Global singleton
// ---------------------------------------------------------------------------

static ACL_STORE: Mutex<Option<AclStore>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Get the access ACL for an inode (clone).
pub fn get_access_acl(ino: u64) -> Option<Acl> {
    let guard = ACL_STORE.lock();
    guard
        .as_ref()
        .and_then(|store| store.get_access(ino).cloned())
}

/// Get the default ACL for a directory inode (clone).
pub fn get_default_acl(ino: u64) -> Option<Acl> {
    let guard = ACL_STORE.lock();
    guard
        .as_ref()
        .and_then(|store| store.get_default(ino).cloned())
}

/// Set the access ACL for an inode.
pub fn set_access_acl(ino: u64, acl: Acl) {
    let mut guard = ACL_STORE.lock();
    if let Some(store) = guard.as_mut() {
        store.set_access(ino, acl);
    }
}

/// Set the default ACL for a directory inode (inherited by new children).
pub fn set_default_acl(ino: u64, acl: Acl) {
    let mut guard = ACL_STORE.lock();
    if let Some(store) = guard.as_mut() {
        store.set_default(ino, acl);
    }
}

/// Remove all ACLs for an inode.
pub fn remove_acl(ino: u64) {
    let mut guard = ACL_STORE.lock();
    if let Some(store) = guard.as_mut() {
        store.remove(ino);
    }
}

/// Check access using the stored ACL for an inode.
pub fn check_access(
    ino: u64,
    uid: u32,
    gid: u32,
    supplementary_groups: &[u32],
    owner_uid: u32,
    owner_gid: u32,
    requested: u16,
) -> bool {
    let guard = ACL_STORE.lock();
    match guard.as_ref().and_then(|store| store.get_access(ino)) {
        Some(acl) => acl.check_access(
            uid,
            gid,
            supplementary_groups,
            owner_uid,
            owner_gid,
            requested,
        ),
        None => false,
    }
}

/// Initialize the ACL subsystem.
pub fn init() {
    let mut guard = ACL_STORE.lock();
    *guard = Some(AclStore::new());
    serial_println!("    acl: initialized (POSIX.1e access control lists)");
}
