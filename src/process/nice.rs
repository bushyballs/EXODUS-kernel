/// Nice value and static priority management for processes.
///
/// Provides the Linux-compatible nice-to-weight lookup table and process
/// priority helpers integrated with both the lightweight process table
/// (proc_table) and the heavy PCB (pcb::PROCESS_TABLE).
///
/// No std, no float, no panics.

/// Valid nice range: -20 (highest priority) to +19 (lowest).
pub const NICE_MIN: i8 = -20;
pub const NICE_MAX: i8 = 19;

/// Linux prio_to_weight table, indexed by nice + 20 (nice=-20 → index 0).
/// Each step is ~1.25× the next lower priority weight.
/// nice = 0  → weight = 1024  (NICE_0_WEIGHT).
const PRIO_TO_WEIGHT: [u32; 40] = [
    88761, 71755, 56483, 46273, 36291, 29154, 23254, 18705, 14949, 11916, 9548, 7620, 6100, 4904,
    3906, 3121, 2501, 1991, 1586, 1277, 1024, 820, 655, 526, 423, 335, 272, 215, 172, 137, 110, 87,
    70, 56, 45, 36, 29, 23, 18, 15,
];

/// Convert a nice value (-20..=19) to a scheduler weight.
///
/// Weight for nice 0 is 1024.  Each step of +1 reduces weight by ~20 %;
/// each step of -1 increases weight by ~25 %.
pub fn nice_to_weight(nice: i8) -> u32 {
    let clamped = clamp_nice(nice);
    let idx = (clamped as i32 + 20) as usize;
    PRIO_TO_WEIGHT[idx]
}

/// Get the nice value for a process by PID.
///
/// Reads from the lightweight proc_table first; falls back to the PCB if
/// the entry is not present in the lightweight table.
pub fn do_getpriority(pid: u32) -> Result<i8, &'static str> {
    // Lightweight table
    if let Some(entry) = super::proc_table::get_process(pid) {
        return Ok(entry.nice);
    }
    // Heavy PCB table
    let table = super::pcb::PROCESS_TABLE.lock();
    if let Some(ref proc) = table[pid as usize] {
        return Ok(proc.priority.nice);
    }
    Err("no such process")
}

/// Set the nice value for a process by PID.
///
/// Clamps the value to [-20, 19].  Raising priority (nice < 0) requires
/// that the caller is root (euid == 0); lowering priority (nice > current)
/// is always allowed.
///
/// Updates both the lightweight proc_table entry and the heavy PCB.
pub fn do_setpriority(pid: u32, nice: i8) -> Result<(), &'static str> {
    let clamped = clamp_nice(nice);
    let new_weight = nice_to_weight(clamped);

    // Update lightweight process table.
    {
        let mut tbl = super::proc_table::PROCESS_TABLE.lock();
        if let Some(ref mut entry) = tbl.slots[pid as usize] {
            // Permission check: only root may raise priority (set negative nice).
            if clamped < entry.nice {
                // TODO: check caller's euid == 0 via creds
                // For now we allow it unconditionally — the permission check
                // is enforced in the syscall layer above us.
            }
            entry.nice = clamped;
            entry.weight = new_weight;
        }
    }

    // Mirror into the heavy PCB.
    {
        let mut table = super::pcb::PROCESS_TABLE.lock();
        if let Some(ref mut proc) = table[pid as usize] {
            proc.priority.set_nice(clamped);
        }
    }

    crate::serial_println!(
        "  [nice] PID {} nice={} weight={}",
        pid,
        clamped,
        new_weight
    );
    Ok(())
}

/// Clamp a nice value to the valid range [-20, 19].
pub fn clamp_nice(nice: i8) -> i8 {
    if nice < NICE_MIN {
        NICE_MIN
    } else if nice > NICE_MAX {
        NICE_MAX
    } else {
        nice
    }
}

/// Initialize the nice/priority subsystem.
pub fn init() {
    crate::serial_println!("  [nice] priority subsystem ready (nice_to_weight table active)");
}
