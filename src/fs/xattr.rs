use crate::serial_println;
use crate::sync::Mutex;
/// Extended file attributes (xattr)
///
/// Part of the AIOS filesystem layer.
///
/// Provides name/value pair storage attached to inodes, with namespace
/// separation (user, system, security, trusted). Xattrs are used for
/// ACLs, SELinux labels, capabilities, and arbitrary user metadata.
///
/// Design:
///   - Each inode can have multiple xattrs stored as (namespace, name, value).
///   - Namespaces restrict access: `user.*` is accessible by the file owner;
///     `system.*` and `security.*` require CAP_SYS_ADMIN; `trusted.*`
///     requires CAP_SYS_ADMIN.
///   - A global store maps inode numbers to their xattr sets.
///   - Maximum value size and total per-inode limits are enforced.
///
/// Inspired by: Linux xattr (fs/xattr.c), ext4 xattr blocks. All code is original.
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum size of a single xattr value in bytes.
const MAX_VALUE_SIZE: usize = 65536;

/// Maximum number of xattrs per inode.
const MAX_XATTRS_PER_INODE: usize = 128;

/// Maximum total xattr storage per inode in bytes.
const MAX_TOTAL_SIZE: usize = 1 << 20; // 1 MB

// ---------------------------------------------------------------------------
// setxattr flags (Linux-compatible)
// ---------------------------------------------------------------------------

/// XATTR_CREATE: fail if the attribute already exists.
pub const XATTR_CREATE: u32 = 1;
/// XATTR_REPLACE: fail if the attribute does not already exist.
pub const XATTR_REPLACE: u32 = 2;

// ---------------------------------------------------------------------------
// Namespace
// ---------------------------------------------------------------------------

/// Xattr namespace prefixes.
#[derive(Clone, Copy, PartialEq)]
pub enum XattrNamespace {
    User,
    System,
    Security,
    Trusted,
}

impl XattrNamespace {
    /// Parse the namespace from a full attribute name (e.g. "user.myattr").
    pub fn from_name(name: &str) -> Option<(Self, &str)> {
        if let Some(rest) = name.strip_prefix("user.") {
            Some((XattrNamespace::User, rest))
        } else if let Some(rest) = name.strip_prefix("system.") {
            Some((XattrNamespace::System, rest))
        } else if let Some(rest) = name.strip_prefix("security.") {
            Some((XattrNamespace::Security, rest))
        } else if let Some(rest) = name.strip_prefix("trusted.") {
            Some((XattrNamespace::Trusted, rest))
        } else {
            None
        }
    }

    /// Return the namespace prefix string.
    pub fn prefix(&self) -> &'static str {
        match self {
            XattrNamespace::User => "user.",
            XattrNamespace::System => "system.",
            XattrNamespace::Security => "security.",
            XattrNamespace::Trusted => "trusted.",
        }
    }

    /// Check whether the given uid/capability set permits access.
    /// `is_admin` indicates CAP_SYS_ADMIN or uid==0.
    pub fn check_access(&self, owner_uid: u32, caller_uid: u32, is_admin: bool) -> bool {
        match self {
            XattrNamespace::User => caller_uid == owner_uid || is_admin,
            XattrNamespace::System => is_admin,
            XattrNamespace::Security => is_admin,
            XattrNamespace::Trusted => is_admin,
        }
    }
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A single extended attribute.
#[derive(Clone)]
struct XattrEntry {
    name: String, // Full name including namespace prefix
    value: Vec<u8>,
}

/// All xattrs for one inode.
#[derive(Clone)]
struct InodeXattrs {
    ino: u64,
    attrs: Vec<XattrEntry>,
    total_bytes: usize,
}

/// Global xattr store.
struct XattrStore {
    inodes: Vec<InodeXattrs>,
}

// ---------------------------------------------------------------------------
// InodeXattrs implementation
// ---------------------------------------------------------------------------

impl InodeXattrs {
    fn new(ino: u64) -> Self {
        InodeXattrs {
            ino,
            attrs: Vec::new(),
            total_bytes: 0,
        }
    }

    fn get(&self, name: &str) -> Option<&[u8]> {
        for attr in self.attrs.iter() {
            if attr.name == name {
                return Some(&attr.value);
            }
        }
        None
    }

    /// Set an xattr value.
    ///
    /// `flags`:
    ///  - 0             — create or replace unconditionally
    ///  - XATTR_CREATE  — fail with -17 (EEXIST) if attribute already exists
    ///  - XATTR_REPLACE — fail with -61 (ENODATA) if attribute does not exist
    fn set(&mut self, name: &str, value: &[u8], flags: u32) -> Result<(), i32> {
        if value.len() > MAX_VALUE_SIZE {
            return Err(-34); // ERANGE
        }

        // Locate existing entry
        let existing = self.attrs.iter_mut().position(|a| a.name == name);

        match existing {
            Some(idx) => {
                // Attribute exists
                if flags & XATTR_CREATE != 0 {
                    return Err(-17); // EEXIST
                }
                let old_size = self.attrs[idx].value.len();
                let new_total = self.total_bytes.saturating_sub(old_size) + value.len();
                if new_total > MAX_TOTAL_SIZE {
                    return Err(-28); // ENOSPC
                }
                self.attrs[idx].value = Vec::from(value);
                self.total_bytes = new_total;
                Ok(())
            }
            None => {
                // Attribute does not exist
                if flags & XATTR_REPLACE != 0 {
                    return Err(-61); // ENODATA
                }
                if self.attrs.len() >= MAX_XATTRS_PER_INODE {
                    return Err(-28); // ENOSPC
                }
                let new_total = self.total_bytes + value.len() + name.len();
                if new_total > MAX_TOTAL_SIZE {
                    return Err(-28); // ENOSPC
                }
                self.attrs.push(XattrEntry {
                    name: String::from(name),
                    value: Vec::from(value),
                });
                self.total_bytes = new_total;
                Ok(())
            }
        }
    }

    fn remove(&mut self, name: &str) -> Result<(), i32> {
        let before = self.attrs.len();
        let mut removed_size = 0usize;
        self.attrs.retain(|a| {
            if a.name == name {
                removed_size += a.value.len() + a.name.len();
                false
            } else {
                true
            }
        });
        if self.attrs.len() == before {
            Err(-1) // ENODATA
        } else {
            self.total_bytes = self.total_bytes.saturating_sub(removed_size);
            Ok(())
        }
    }

    fn list(&self) -> Vec<String> {
        self.attrs.iter().map(|a| a.name.clone()).collect()
    }

    fn list_in_namespace(&self, ns: XattrNamespace) -> Vec<String> {
        let prefix = ns.prefix();
        self.attrs
            .iter()
            .filter(|a| a.name.starts_with(prefix))
            .map(|a| a.name.clone())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// XattrStore implementation
// ---------------------------------------------------------------------------

impl XattrStore {
    fn new() -> Self {
        XattrStore { inodes: Vec::new() }
    }

    fn get_or_create(&mut self, ino: u64) -> &mut InodeXattrs {
        // Find existing
        let pos = self.inodes.iter().position(|ix| ix.ino == ino);
        match pos {
            Some(idx) => &mut self.inodes[idx],
            None => {
                self.inodes.push(InodeXattrs::new(ino));
                let last = self.inodes.len() - 1;
                &mut self.inodes[last]
            }
        }
    }

    fn find(&self, ino: u64) -> Option<&InodeXattrs> {
        self.inodes.iter().find(|ix| ix.ino == ino)
    }

    fn remove_inode(&mut self, ino: u64) {
        self.inodes.retain(|ix| ix.ino != ino);
    }
}

// ---------------------------------------------------------------------------
// Global singleton
// ---------------------------------------------------------------------------

static XATTR_STORE: Mutex<Option<XattrStore>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Get an extended attribute value.
pub fn getxattr(ino: u64, name: &str) -> Result<Vec<u8>, i32> {
    let guard = XATTR_STORE.lock();
    let store = guard.as_ref().ok_or(-1)?;
    let ix = store.find(ino).ok_or(-1)?; // ENODATA
    ix.get(name).map(|v| Vec::from(v)).ok_or(-1)
}

/// Set an extended attribute.
///
/// `flags` controls create/replace semantics:
///   - 0             — create or replace unconditionally
///   - XATTR_CREATE  — fail (EEXIST) if attribute already exists
///   - XATTR_REPLACE — fail (ENODATA) if attribute does not exist
pub fn setxattr(ino: u64, name: &str, value: &[u8], flags: u32) -> Result<(), i32> {
    // Validate namespace
    if XattrNamespace::from_name(name).is_none() {
        return Err(-95); // EOPNOTSUPP
    }
    let mut guard = XATTR_STORE.lock();
    let store = guard.as_mut().ok_or(-1)?;
    let ix = store.get_or_create(ino);
    ix.set(name, value, flags)
}

// ---------------------------------------------------------------------------
// Syscall-boundary wrappers
// These are called from syscall.rs with raw user buffers already validated.
// They copy data out of/into caller-supplied byte slices and translate the
// Rust Result into a Linux-compatible return value (>=0 on success, <0 errno).
// ---------------------------------------------------------------------------

/// sys_getxattr: copy the xattr value into `out_buf`.
///
/// Returns `Ok(n)` where n is the number of bytes written, or `Err(errno)`.
/// If `out_buf` is empty (len==0), returns the size of the attribute value
/// without copying (Linux list-size query convention).
pub fn sys_getxattr(ino: u64, name: &str, out_buf: &mut [u8]) -> Result<usize, i32> {
    let value = getxattr(ino, name)?;
    if out_buf.is_empty() {
        // Query-only: return required size
        return Ok(value.len());
    }
    if out_buf.len() < value.len() {
        return Err(-34); // ERANGE
    }
    out_buf[..value.len()].copy_from_slice(&value);
    Ok(value.len())
}

/// sys_setxattr: set an xattr from a raw byte slice.
///
/// Returns `Ok(())` on success or `Err(errno)`.
pub fn sys_setxattr(ino: u64, name: &str, value_buf: &[u8], flags: u32) -> Result<(), i32> {
    setxattr(ino, name, value_buf, flags)
}

/// sys_listxattr: write null-separated attribute names into `out_buf`.
///
/// Returns the total number of bytes written (including null terminators).
/// If `out_buf` is empty, returns the required buffer size without writing.
pub fn sys_listxattr(ino: u64, out_buf: &mut [u8]) -> usize {
    let names = listxattr(ino);

    // Compute required size: each name + one null byte
    let required: usize = names.iter().map(|n| n.len() + 1).sum();
    if out_buf.is_empty() {
        return required;
    }

    let mut pos = 0usize;
    for name in &names {
        let bytes = name.as_bytes();
        let end = pos + bytes.len() + 1; // +1 for null terminator
        if end > out_buf.len() {
            break;
        }
        out_buf[pos..pos + bytes.len()].copy_from_slice(bytes);
        out_buf[pos + bytes.len()] = 0u8;
        pos = end;
    }
    pos
}

/// sys_removexattr: remove an xattr by name.
///
/// Returns `Ok(())` on success or `Err(errno)`.
pub fn sys_removexattr(ino: u64, name: &str) -> Result<(), i32> {
    removexattr(ino, name)
}

/// Remove an extended attribute.
pub fn removexattr(ino: u64, name: &str) -> Result<(), i32> {
    let mut guard = XATTR_STORE.lock();
    let store = guard.as_mut().ok_or(-1)?;
    let ix = store.get_or_create(ino);
    ix.remove(name)
}

/// List all extended attribute names for an inode.
pub fn listxattr(ino: u64) -> Vec<String> {
    let guard = XATTR_STORE.lock();
    match guard.as_ref().and_then(|store| store.find(ino)) {
        Some(ix) => ix.list(),
        None => Vec::new(),
    }
}

/// List extended attribute names in a specific namespace.
pub fn listxattr_ns(ino: u64, ns: XattrNamespace) -> Vec<String> {
    let guard = XATTR_STORE.lock();
    match guard.as_ref().and_then(|store| store.find(ino)) {
        Some(ix) => ix.list_in_namespace(ns),
        None => Vec::new(),
    }
}

/// Remove all xattrs for an inode (called on inode deletion).
pub fn remove_all(ino: u64) {
    let mut guard = XATTR_STORE.lock();
    if let Some(store) = guard.as_mut() {
        store.remove_inode(ino);
    }
}

/// Initialize the xattr subsystem.
pub fn init() {
    let mut guard = XATTR_STORE.lock();
    *guard = Some(XattrStore::new());
    serial_println!("    xattr: initialized (user, system, security, trusted namespaces)");
}
