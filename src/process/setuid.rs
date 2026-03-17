/// POSIX setuid/setgid privilege management
///
/// Implements the full POSIX-2017 semantics for user and group ID manipulation
/// including real/effective/saved triple (ruid, euid, suid) and the
/// `setresuid(2)` extension supported by Linux and BSD.
///
/// ## POSIX Rules Summary
///
/// ### setuid(uid) — Linux / POSIX
/// - If the caller is root (euid == 0):
///     Sets ruid = euid = suid = uid.  Irrevocably drops root.
/// - If the caller is not root:
///     Only changes euid.  The new uid must equal ruid or suid.
///     (A non-root process cannot change its ruid or suid via setuid.)
///
/// ### seteuid(euid)
/// - If root: set euid = new value (ruid and suid unchanged).
/// - If not root: set euid only if uid ∈ {ruid, suid}.
///
/// ### setresuid(ruid, euid, suid)
/// - If root: may set any of the three to any value.
/// - If not root: each non-`u32::MAX` ("−1") argument must be within the
///   caller's current {ruid, euid, suid} set.
///
/// ### setgid / setegid / setresgid — symmetric with the UID variants.
///
/// ### drop_privileges(new_uid, new_gid)
/// - Irrevocably drops from root to `new_uid`:
///     1. Clears all capabilities.
///     2. Sets ruid = euid = suid = new_uid.
///     3. Sets fsuid = new_uid.
/// - Returns `Err` if the process is not currently root, or if `new_uid == 0`.
///
/// ## No-std
/// All functions operate solely on the PCB process table.  No heap allocation
/// is performed here.
// We use the full pcb::PROCESS_TABLE (not proc_table::PROCESS_TABLE) because
// only the full pcb::Process struct carries the `creds` (Credentials) field.
use crate::process::pcb::PROCESS_TABLE;

// POSIX sentinel meaning "leave this ID unchanged" in setresuid/setresgid.
const UNCHANGED: u32 = u32::MAX;

// ---------------------------------------------------------------------------
// Helper: current PID
// ---------------------------------------------------------------------------

/// Return the PID of the currently executing process.
///
/// Uses the same per-CPU slot as `sched_core::current_pid()` but avoids a
/// circular dependency by reading the underlying SMP atomic directly.
#[inline(always)]
fn caller_pid() -> u32 {
    crate::smp::this_cpu()
        .current_pid
        .load(core::sync::atomic::Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Helper: read current credentials without mut
// ---------------------------------------------------------------------------

/// Read (ruid, euid, suid) for the calling process.
fn current_uids() -> (u32, u32, u32) {
    let pid = caller_pid();
    let tbl = PROCESS_TABLE.lock();
    tbl[pid as usize]
        .as_ref()
        .map(|p| (p.creds.uid, p.creds.euid, p.creds.suid))
        .unwrap_or((u32::MAX, u32::MAX, u32::MAX))
}

/// Read (rgid, egid, sgid) for the calling process.
fn current_gids() -> (u32, u32, u32) {
    let pid = caller_pid();
    let tbl = PROCESS_TABLE.lock();
    tbl[pid as usize]
        .as_ref()
        .map(|p| (p.creds.gid, p.creds.egid, p.creds.sgid))
        .unwrap_or((u32::MAX, u32::MAX, u32::MAX))
}

// ---------------------------------------------------------------------------
// sys_setuid
// ---------------------------------------------------------------------------

/// `setuid(uid)` — change user identity.
///
/// Implements POSIX semantics:
/// - **root** (euid == 0): sets ruid = euid = suid = uid.
/// - **non-root**: sets euid = uid if uid ∈ {ruid, suid}; all other
///   combinations return `Err(-1)` (EPERM).
///
/// Returns `Ok(())` on success, `Err(-1)` (EPERM) on permission failure, or
/// `Err(-3)` (ESRCH) if the process cannot be found.
pub fn sys_setuid(uid: u32) -> Result<(), i32> {
    let (ruid, euid, suid) = current_uids();
    let is_root = euid == 0;

    if !is_root && uid != ruid && uid != suid {
        return Err(-1); // EPERM
    }

    let pid = caller_pid();
    let mut tbl = PROCESS_TABLE.lock();
    let proc = tbl[pid as usize].as_mut().ok_or(-3i32)?; // ESRCH

    if is_root {
        // Root: set all three IDs and drop all capabilities.
        proc.creds.uid = uid;
        proc.creds.euid = uid;
        proc.creds.suid = uid;
        proc.creds.fsuid = uid;
        // Irrevocably drop all capabilities when leaving root uid.
        if uid != 0 {
            proc.creds.capabilities = 0;
        }
    } else {
        // Non-root: only change euid; ruid and suid are unchanged.
        proc.creds.euid = uid;
        proc.creds.fsuid = uid;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// sys_setgid
// ---------------------------------------------------------------------------

/// `setgid(gid)` — change group identity.
///
/// Symmetric with `sys_setuid` but operates on GID triple.
pub fn sys_setgid(gid: u32) -> Result<(), i32> {
    let (rgid, egid, sgid) = current_gids();
    let (_, euid, _) = current_uids();
    let is_root = euid == 0;

    if !is_root && gid != rgid && gid != sgid {
        return Err(-1); // EPERM
    }

    let pid = caller_pid();
    let mut tbl = PROCESS_TABLE.lock();
    let proc = tbl[pid as usize].as_mut().ok_or(-3i32)?;

    let _ = egid; // unused after permission check
    if is_root {
        proc.creds.gid = gid;
        proc.creds.egid = gid;
        proc.creds.sgid = gid;
        proc.creds.fsgid = gid;
    } else {
        proc.creds.egid = gid;
        proc.creds.fsgid = gid;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// sys_seteuid
// ---------------------------------------------------------------------------

/// `seteuid(euid)` — set effective user ID only.
///
/// - **root**: may set euid to any value; ruid and suid unchanged.
/// - **non-root**: euid must be the current ruid or suid.
pub fn sys_seteuid(euid: u32) -> Result<(), i32> {
    let (ruid, cur_euid, suid) = current_uids();
    let is_root = cur_euid == 0;

    if !is_root && euid != ruid && euid != suid {
        return Err(-1); // EPERM
    }

    let pid = caller_pid();
    let mut tbl = PROCESS_TABLE.lock();
    let proc = tbl[pid as usize].as_mut().ok_or(-3i32)?;

    proc.creds.euid = euid;
    proc.creds.fsuid = euid;

    Ok(())
}

// ---------------------------------------------------------------------------
// sys_setegid
// ---------------------------------------------------------------------------

/// `setegid(egid)` — set effective group ID only.
pub fn sys_setegid(egid: u32) -> Result<(), i32> {
    let (rgid, _cur_egid, sgid) = current_gids();
    let (_, euid, _) = current_uids();
    let is_root = euid == 0;

    if !is_root && egid != rgid && egid != sgid {
        return Err(-1); // EPERM
    }

    let pid = caller_pid();
    let mut tbl = PROCESS_TABLE.lock();
    let proc = tbl[pid as usize].as_mut().ok_or(-3i32)?;

    proc.creds.egid = egid;
    proc.creds.fsgid = egid;

    Ok(())
}

// ---------------------------------------------------------------------------
// sys_setresuid
// ---------------------------------------------------------------------------

/// `setresuid(ruid, euid, suid)` — set all three user IDs independently.
///
/// Pass `u32::MAX` (i.e. `(u32)-1` / UNCHANGED) for any component to leave
/// it unchanged.
///
/// POSIX / Linux semantics:
/// - **root** (euid == 0): may set any component to any value.
/// - **non-root**: each non-UNCHANGED argument must belong to the caller's
///   current {ruid, euid, suid} set.
pub fn sys_setresuid(new_ruid: u32, new_euid: u32, new_suid: u32) -> Result<(), i32> {
    let (old_ruid, old_euid, old_suid) = current_uids();
    let is_root = old_euid == 0;

    if !is_root {
        // Every supplied (non-UNCHANGED) value must come from the current set.
        let allowed = |v: u32| v == UNCHANGED || v == old_ruid || v == old_euid || v == old_suid;
        if !allowed(new_ruid) || !allowed(new_euid) || !allowed(new_suid) {
            return Err(-1); // EPERM
        }
    }

    let pid = caller_pid();
    let mut tbl = PROCESS_TABLE.lock();
    let proc = tbl[pid as usize].as_mut().ok_or(-3i32)?;

    if new_ruid != UNCHANGED {
        proc.creds.uid = new_ruid;
    }
    if new_euid != UNCHANGED {
        proc.creds.euid = new_euid;
        proc.creds.fsuid = new_euid;
    }
    if new_suid != UNCHANGED {
        proc.creds.suid = new_suid;
    }

    // If root dropped its effective UID and none of the final UIDs is 0,
    // clear all capabilities (no way back to root).
    let final_euid = if new_euid != UNCHANGED {
        new_euid
    } else {
        old_euid
    };
    let final_ruid = if new_ruid != UNCHANGED {
        new_ruid
    } else {
        old_ruid
    };
    let final_suid = if new_suid != UNCHANGED {
        new_suid
    } else {
        old_suid
    };
    if is_root && final_euid != 0 && final_ruid != 0 && final_suid != 0 {
        proc.creds.capabilities = 0;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// sys_setresgid
// ---------------------------------------------------------------------------

/// `setresgid(rgid, egid, sgid)` — set all three group IDs independently.
///
/// Same semantics as `sys_setresuid` but for GIDs.
pub fn sys_setresgid(new_rgid: u32, new_egid: u32, new_sgid: u32) -> Result<(), i32> {
    let (old_rgid, old_egid, old_sgid) = current_gids();
    let (_, euid, _) = current_uids();
    let is_root = euid == 0;

    if !is_root {
        let allowed = |v: u32| v == UNCHANGED || v == old_rgid || v == old_egid || v == old_sgid;
        if !allowed(new_rgid) || !allowed(new_egid) || !allowed(new_sgid) {
            return Err(-1); // EPERM
        }
    }

    let pid = caller_pid();
    let mut tbl = PROCESS_TABLE.lock();
    let proc = tbl[pid as usize].as_mut().ok_or(-3i32)?;

    if new_rgid != UNCHANGED {
        proc.creds.gid = new_rgid;
    }
    if new_egid != UNCHANGED {
        proc.creds.egid = new_egid;
        proc.creds.fsgid = new_egid;
    }
    if new_sgid != UNCHANGED {
        proc.creds.sgid = new_sgid;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// sys_getuid / sys_geteuid / sys_getgid / sys_getegid
// ---------------------------------------------------------------------------

/// Return the real user ID of the calling process.
pub fn sys_getuid() -> u32 {
    let pid = caller_pid();
    let tbl = PROCESS_TABLE.lock();
    tbl[pid as usize]
        .as_ref()
        .map(|p| p.creds.uid)
        .unwrap_or(u32::MAX)
}

/// Return the effective user ID of the calling process.
pub fn sys_geteuid() -> u32 {
    let pid = caller_pid();
    let tbl = PROCESS_TABLE.lock();
    tbl[pid as usize]
        .as_ref()
        .map(|p| p.creds.euid)
        .unwrap_or(u32::MAX)
}

/// Return the real group ID of the calling process.
pub fn sys_getgid() -> u32 {
    let pid = caller_pid();
    let tbl = PROCESS_TABLE.lock();
    tbl[pid as usize]
        .as_ref()
        .map(|p| p.creds.gid)
        .unwrap_or(u32::MAX)
}

/// Return the effective group ID of the calling process.
pub fn sys_getegid() -> u32 {
    let pid = caller_pid();
    let tbl = PROCESS_TABLE.lock();
    tbl[pid as usize]
        .as_ref()
        .map(|p| p.creds.egid)
        .unwrap_or(u32::MAX)
}

// ---------------------------------------------------------------------------
// drop_privileges
// ---------------------------------------------------------------------------

/// Irrevocably drop root privileges and assume `new_uid` (and optionally
/// `new_gid`).
///
/// Safe-drop sequence (matches Linux `libcap` best practice):
/// 1. Verify the process is currently root (euid == 0).
/// 2. Verify `new_uid != 0` (refusing to "drop" to another root account).
/// 3. Clear all capability bits.
/// 4. Set ruid = euid = suid = fsuid = new_uid.
/// 5. If `new_gid != UNCHANGED`, set rgid = egid = sgid = fsgid = new_gid.
///
/// After a successful call the process has no way to regain root.
///
/// Returns:
/// - `Ok(())` on success.
/// - `Err(-1)` (EPERM) if the process is not root or `new_uid == 0`.
/// - `Err(-3)` (ESRCH) if the process entry does not exist.
pub fn drop_privileges(new_uid: u32, new_gid: u32) -> Result<(), i32> {
    let (_, cur_euid, _) = current_uids();

    // Must be root to call this.
    if cur_euid != 0 {
        return Err(-1); // EPERM
    }
    // Refuse to "drop" to uid 0 — that would be a no-op and likely a bug.
    if new_uid == 0 {
        return Err(-1); // EPERM
    }

    let pid = caller_pid();
    let mut tbl = PROCESS_TABLE.lock();
    let proc = tbl[pid as usize].as_mut().ok_or(-3i32)?;

    // Step 1: clear all capabilities — no way back to elevated privileges.
    proc.creds.capabilities = 0;

    // Step 2: set all UID components.
    proc.creds.uid = new_uid;
    proc.creds.euid = new_uid;
    proc.creds.suid = new_uid;
    proc.creds.fsuid = new_uid;

    // Step 3: optionally update GID.
    if new_gid != UNCHANGED {
        proc.creds.gid = new_gid;
        proc.creds.egid = new_gid;
        proc.creds.sgid = new_gid;
        proc.creds.fsgid = new_gid;
    }

    crate::serial_println!(
        "  [setuid] PID {} dropped privileges to uid={} gid={}",
        pid,
        new_uid,
        new_gid
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// dispatch — called from syscall.rs for the extended setuid syscalls
// ---------------------------------------------------------------------------

/// Syscall numbers for the extended setuid family.
///
/// These mirror Linux x86-64 ABI numbers for compatibility.
pub mod nr {
    /// `geteuid()` — Linux ABI 107
    pub const SYS_GETEUID: u64 = 107;
    /// `getegid()` — Linux ABI 108
    pub const SYS_GETEGID: u64 = 108;
    /// `seteuid()` — non-standard; matches Genesis convention 117
    pub const SYS_SETEUID: u64 = 117;
    /// `setresuid()` — Linux ABI 117; Genesis uses 118
    pub const SYS_SETRESUID: u64 = 118;
    /// `setresgid()` — Linux ABI 119
    pub const SYS_SETRESGID: u64 = 119;
    /// `drop_privileges()` — Genesis extension 502
    pub const SYS_DROP_PRIV: u64 = 502;
}
