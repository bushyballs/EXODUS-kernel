/// landlock — Landlock LSM (Linux sandboxing, Linux 5.13+)
///
/// Allows unprivileged processes to restrict their own access to
/// filesystem and network resources using a ruleset-based model:
///
///   1. Create a ruleset with desired access rights
///   2. Add path-based rules to the ruleset
///   3. Restrict the current thread with the ruleset
///
/// Access rights (filesystem):
///   FS_READ_FILE, FS_WRITE_FILE, FS_READ_DIR, FS_MAKE_DIR, etc.
///
/// Implementation: fixed-size static tables, path prefix matching.
/// Inspired by: Linux security/landlock/. All code is original.
/// Rules: no_std, no heap, no floats, no panics, saturating counters.
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Access right bitmasks (match Linux LANDLOCK_ACCESS_FS_*)
// ---------------------------------------------------------------------------

pub const FS_EXECUTE: u64 = 1 << 0;
pub const FS_WRITE_FILE: u64 = 1 << 1;
pub const FS_READ_FILE: u64 = 1 << 2;
pub const FS_READ_DIR: u64 = 1 << 3;
pub const FS_REMOVE_DIR: u64 = 1 << 4;
pub const FS_REMOVE_FILE: u64 = 1 << 5;
pub const FS_MAKE_CHAR: u64 = 1 << 6;
pub const FS_MAKE_DIR: u64 = 1 << 7;
pub const FS_MAKE_REG: u64 = 1 << 8;
pub const FS_MAKE_SOCK: u64 = 1 << 9;
pub const FS_MAKE_FIFO: u64 = 1 << 10;
pub const FS_MAKE_BLOCK: u64 = 1 << 11;
pub const FS_MAKE_SYM: u64 = 1 << 12;
pub const FS_REFER: u64 = 1 << 13;
pub const FS_TRUNCATE: u64 = 1 << 14;
pub const NET_BIND_TCP: u64 = 1 << 32;
pub const NET_CONNECT_TCP: u64 = 1 << 33;
pub const ALL_FS_ACCESS: u64 = (1u64 << 15) - 1;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_RULESETS: usize = 16;
const MAX_RULES: usize = 128;
const MAX_PIDS: usize = 256;
const PATH_LEN: usize = 64;

// ---------------------------------------------------------------------------
// Ruleset
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct LandlockRuleset {
    pub id: u32,
    pub handled_fs: u64,
    pub handled_net: u64,
    pub active: bool,
}

impl LandlockRuleset {
    pub const fn empty() -> Self {
        LandlockRuleset {
            id: 0,
            handled_fs: 0,
            handled_net: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Rule
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct LandlockRule {
    pub ruleset_id: u32,
    pub path: [u8; PATH_LEN],
    pub path_len: u8,
    pub allowed_fs: u64,
    pub is_dir: bool,
    pub active: bool,
}

impl LandlockRule {
    pub const fn empty() -> Self {
        LandlockRule {
            ruleset_id: 0,
            path: [0u8; PATH_LEN],
            path_len: 0,
            allowed_fs: 0,
            is_dir: false,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Per-PID restriction
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
struct PidRestrict {
    pid: u32,
    ruleset_id: u32,
    active: bool,
}

impl PidRestrict {
    const fn empty() -> Self {
        PidRestrict {
            pid: 0,
            ruleset_id: 0,
            active: false,
        }
    }
}

const EMPTY_RS: LandlockRuleset = LandlockRuleset::empty();
const EMPTY_RL: LandlockRule = LandlockRule::empty();
const EMPTY_PR: PidRestrict = PidRestrict::empty();

static RULESETS: Mutex<[LandlockRuleset; MAX_RULESETS]> = Mutex::new([EMPTY_RS; MAX_RULESETS]);
static RULES: Mutex<[LandlockRule; MAX_RULES]> = Mutex::new([EMPTY_RL; MAX_RULES]);
static RESTRICTS: Mutex<[PidRestrict; MAX_PIDS]> = Mutex::new([EMPTY_PR; MAX_PIDS]);
static RS_NEXT_ID: AtomicU32 = AtomicU32::new(1);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn copy_path(dst: &mut [u8; PATH_LEN], src: &[u8]) -> u8 {
    let len = src.len().min(PATH_LEN - 1);
    let mut i = 0usize;
    while i < len {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
    len as u8
}

fn path_is_under(rule: &[u8], rlen: usize, check: &[u8]) -> bool {
    if check.len() < rlen {
        return false;
    }
    let mut i = 0usize;
    while i < rlen {
        if rule[i] != check[i] {
            return false;
        }
        i = i.saturating_add(1);
    }
    if check.len() == rlen {
        return true;
    }
    rule[rlen.saturating_sub(1)] == b'/' || check[rlen] == b'/'
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn landlock_create_ruleset(handled_fs: u64, handled_net: u64) -> u32 {
    let id = RS_NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let mut rs = RULESETS.lock();
    let mut i = 0usize;
    while i < MAX_RULESETS {
        if !rs[i].active {
            rs[i] = LandlockRuleset {
                id,
                handled_fs,
                handled_net,
                active: true,
            };
            return id;
        }
        i = i.saturating_add(1);
    }
    0
}

pub fn landlock_add_rule(ruleset_id: u32, path: &[u8], allowed_fs: u64, is_dir: bool) -> bool {
    // Verify ruleset exists
    let rs_ok = {
        let rs = RULESETS.lock();
        let mut found = false;
        let mut i = 0usize;
        while i < MAX_RULESETS {
            if rs[i].active && rs[i].id == ruleset_id {
                found = true;
                break;
            }
            i = i.saturating_add(1);
        }
        found
    };
    if !rs_ok {
        return false;
    }
    let mut rules = RULES.lock();
    let mut i = 0usize;
    while i < MAX_RULES {
        if !rules[i].active {
            rules[i] = LandlockRule::empty();
            rules[i].ruleset_id = ruleset_id;
            rules[i].path_len = copy_path(&mut rules[i].path, path);
            rules[i].allowed_fs = allowed_fs;
            rules[i].is_dir = is_dir;
            rules[i].active = true;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn landlock_restrict_self(pid: u32, ruleset_id: u32) -> bool {
    if pid as usize >= MAX_PIDS {
        return false;
    }
    let rs_ok = {
        let rs = RULESETS.lock();
        let mut found = false;
        let mut i = 0usize;
        while i < MAX_RULESETS {
            if rs[i].active && rs[i].id == ruleset_id {
                found = true;
                break;
            }
            i = i.saturating_add(1);
        }
        found
    };
    if !rs_ok {
        return false;
    }
    let mut rts = RESTRICTS.lock();
    let mut i = 0usize;
    while i < MAX_PIDS {
        if rts[i].active && rts[i].pid == pid {
            rts[i].ruleset_id = ruleset_id;
            return true;
        }
        i = i.saturating_add(1);
    }
    i = 0;
    while i < MAX_PIDS {
        if !rts[i].active {
            rts[i] = PidRestrict {
                pid,
                ruleset_id,
                active: true,
            };
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Returns true if the access is permitted (or process is unrestricted).
pub fn landlock_check(pid: u32, path: &[u8], access: u64) -> bool {
    if pid as usize >= MAX_PIDS {
        return true;
    }
    let rts = RESTRICTS.lock();
    let mut ruleset_id = 0u32;
    let mut restricted = false;
    let mut j = 0usize;
    while j < MAX_PIDS {
        if rts[j].active && rts[j].pid == pid {
            ruleset_id = rts[j].ruleset_id;
            restricted = true;
            break;
        }
        j = j.saturating_add(1);
    }
    drop(rts);
    if !restricted {
        return true;
    }
    let rules = RULES.lock();
    let mut i = 0usize;
    while i < MAX_RULES {
        if rules[i].active && rules[i].ruleset_id == ruleset_id {
            let rlen = rules[i].path_len as usize;
            if path_is_under(&rules[i].path, rlen, path) {
                if rules[i].allowed_fs & access == access {
                    return true;
                }
            }
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn landlock_cleanup_pid(pid: u32) {
    if pid as usize >= MAX_PIDS {
        return;
    }
    let mut rts = RESTRICTS.lock();
    let mut i = 0usize;
    while i < MAX_PIDS {
        if rts[i].active && rts[i].pid == pid {
            rts[i].active = false;
            return;
        }
        i = i.saturating_add(1);
    }
}

pub fn landlock_free_ruleset(id: u32) -> bool {
    {
        let mut rules = RULES.lock();
        let mut i = 0usize;
        while i < MAX_RULES {
            if rules[i].active && rules[i].ruleset_id == id {
                rules[i].active = false;
            }
            i = i.saturating_add(1);
        }
    }
    let mut rs = RULESETS.lock();
    let mut i = 0usize;
    while i < MAX_RULESETS {
        if rs[i].active && rs[i].id == id {
            rs[i].active = false;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn init() {
    serial_println!(
        "[landlock] Landlock LSM initialized (max {} rulesets, {} rules, {} pids)",
        MAX_RULESETS,
        MAX_RULES,
        MAX_PIDS
    );
}
