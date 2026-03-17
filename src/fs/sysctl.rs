/// sysctl — kernel parameter interface (/proc/sys/)
///
/// Exposes kernel tuning parameters as a key-value store.
///
/// Rules: no_std, no heap, no floats, no panics, saturating counters.
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, PartialEq)]
pub enum SysctlType {
    Integer,
    String64,
    Boolean,
}

#[derive(Copy, Clone)]
pub struct SysctlEntry {
    pub key: [u8; 64],
    pub key_len: u8,
    pub val_type: SysctlType,
    pub int_val: i64,
    pub str_val: [u8; 64],
    pub str_len: u8,
    pub bool_val: bool,
    pub readonly: bool,
    pub active: bool,
}

impl SysctlEntry {
    pub const fn empty() -> Self {
        SysctlEntry {
            key: [0u8; 64],
            key_len: 0,
            val_type: SysctlType::Integer,
            int_val: 0,
            str_val: [0u8; 64],
            str_len: 0,
            bool_val: false,
            readonly: false,
            active: false,
        }
    }
}

const EMPTY_ENTRY: SysctlEntry = SysctlEntry::empty();
static SYSCTL_TABLE: Mutex<[SysctlEntry; 128]> = Mutex::new([EMPTY_ENTRY; 128]);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn key_matches(a: &[u8; 64], alen: u8, b: &[u8]) -> bool {
    let alen = alen as usize;
    if alen != b.len() {
        return false;
    }
    let mut i = 0usize;
    while i < alen {
        if a[i] != b[i] {
            return false;
        }
        i = i.saturating_add(1);
    }
    true
}

fn copy_bytes(dst: &mut [u8; 64], src: &[u8]) -> u8 {
    let len = src.len().min(63);
    let mut i = 0usize;
    while i < len {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
    len as u8
}

fn find_slot(table: &[SysctlEntry; 128]) -> Option<usize> {
    let mut i = 0usize;
    while i < 128 {
        if !table[i].active {
            return Some(i);
        }
        i = i.saturating_add(1);
    }
    None
}

fn find_key(table: &[SysctlEntry; 128], key: &[u8]) -> Option<usize> {
    let mut i = 0usize;
    while i < 128 {
        if table[i].active && key_matches(&table[i].key, table[i].key_len, key) {
            return Some(i);
        }
        i = i.saturating_add(1);
    }
    None
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn sysctl_register_int(key: &[u8], default_val: i64, readonly: bool) -> bool {
    let mut t = SYSCTL_TABLE.lock();
    if find_key(&t, key).is_some() {
        return false;
    } // already exists
    if let Some(slot) = find_slot(&t) {
        t[slot] = SysctlEntry::empty();
        t[slot].key_len = copy_bytes(&mut t[slot].key, key);
        t[slot].val_type = SysctlType::Integer;
        t[slot].int_val = default_val;
        t[slot].readonly = readonly;
        t[slot].active = true;
        true
    } else {
        false
    }
}

pub fn sysctl_register_bool(key: &[u8], default_val: bool, readonly: bool) -> bool {
    let mut t = SYSCTL_TABLE.lock();
    if find_key(&t, key).is_some() {
        return false;
    }
    if let Some(slot) = find_slot(&t) {
        t[slot] = SysctlEntry::empty();
        t[slot].key_len = copy_bytes(&mut t[slot].key, key);
        t[slot].val_type = SysctlType::Boolean;
        t[slot].bool_val = default_val;
        t[slot].readonly = readonly;
        t[slot].active = true;
        true
    } else {
        false
    }
}

pub fn sysctl_register_str(key: &[u8], default_str: &[u8], readonly: bool) -> bool {
    let mut t = SYSCTL_TABLE.lock();
    if find_key(&t, key).is_some() {
        return false;
    }
    if let Some(slot) = find_slot(&t) {
        t[slot] = SysctlEntry::empty();
        t[slot].key_len = copy_bytes(&mut t[slot].key, key);
        t[slot].val_type = SysctlType::String64;
        t[slot].str_len = copy_bytes(&mut t[slot].str_val, default_str);
        t[slot].readonly = readonly;
        t[slot].active = true;
        true
    } else {
        false
    }
}

pub fn sysctl_get_int(key: &[u8]) -> Option<i64> {
    let t = SYSCTL_TABLE.lock();
    let idx = find_key(&t, key)?;
    if t[idx].val_type == SysctlType::Integer {
        Some(t[idx].int_val)
    } else {
        None
    }
}

pub fn sysctl_get_bool(key: &[u8]) -> Option<bool> {
    let t = SYSCTL_TABLE.lock();
    let idx = find_key(&t, key)?;
    if t[idx].val_type == SysctlType::Boolean {
        Some(t[idx].bool_val)
    } else {
        None
    }
}

pub fn sysctl_set_int(key: &[u8], val: i64) -> bool {
    let mut t = SYSCTL_TABLE.lock();
    let idx = match find_key(&t, key) {
        Some(i) => i,
        None => return false,
    };
    if t[idx].readonly {
        return false;
    }
    t[idx].int_val = val;
    true
}

pub fn sysctl_set_bool(key: &[u8], val: bool) -> bool {
    let mut t = SYSCTL_TABLE.lock();
    let idx = match find_key(&t, key) {
        Some(i) => i,
        None => return false,
    };
    if t[idx].readonly {
        return false;
    }
    t[idx].bool_val = val;
    true
}

// ---------------------------------------------------------------------------
// init
// ---------------------------------------------------------------------------

pub fn init() {
    sysctl_register_str(b"kernel.hostname", b"genesis-aios", false);
    sysctl_register_int(b"kernel.panic", 0, false);
    sysctl_register_int(b"kernel.pid_max", 32768, false);
    sysctl_register_int(b"kernel.printk", 7, false);
    sysctl_register_int(b"vm.swappiness", 60, false);
    sysctl_register_int(b"net.ipv4.ip_forward", 0, false);
    sysctl_register_int(b"net.ipv4.tcp_syncookies", 1, false);
    sysctl_register_int(b"fs.file-max", 65536, true);
    serial_println!("[sysctl] kernel parameter interface initialized");
}
