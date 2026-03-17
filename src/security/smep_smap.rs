/// SMEP / SMAP / UMIP enforcement — Genesis kernel hardening
///
/// Three CPU security features controlled through CR4:
///
///   SMEP (Supervisor Mode Execution Prevention) — CR4 bit 20
///     Raises a #PF if the kernel attempts to execute code at a user-space
///     virtual address (< canonical boundary).  Defeats ret2user attacks.
///
///   SMAP (Supervisor Mode Access Prevention) — CR4 bit 21
///     Raises a #PF if the kernel attempts to read or write a user-space page
///     without first issuing `STAC` (Set AC flag).  Defeats data-only attacks
///     that pivot through kernel reads/writes of user memory.
///     Temporarily disabled with `disable_smap_temporarily()` / re-enabled
///     with `enable_smap_after_access()` around legitimate copy_from/to_user
///     paths.
///
///   UMIP (User-Mode Instruction Prevention) — CR4 bit 11
///     Blocks `SGDT`, `SIDT`, `SLDT`, `SMSW`, `STR` from user-space.
///     Prevents user code from reading descriptor table addresses (which would
///     leak kernel virtual addresses and weaken KASLR).
///
/// NX enforcement:
///   `mark_nx(vaddr)` clears the USER_ACCESSIBLE flag and sets the NO_EXECUTE
///   bit on the PTE for `vaddr`, hardening kernel data pages against
///   executable-data attacks even when SMEP is active.
///
/// copy_from_user / copy_to_user:
///   These wrappers validate that `src`/`dst` are in the canonical user range
///   (< 0x0000_8000_0000_0000) before issuing STAC → memcopy → CLAC.
///
/// Critical rules honoured here:
///   - NO float casts.
///   - Saturating arithmetic on counters.
///   - No heap (`alloc`).
///   - No panics — errors return false / early return.
///
/// All code is original.
use crate::serial_println;

// ── CR4 bit definitions ───────────────────────────────────────────────────────

/// CR4 bit 11: User-Mode Instruction Prevention.
const CR4_UMIP: u64 = 1 << 11;
/// CR4 bit 20: Supervisor Mode Execution Prevention.
const CR4_SMEP: u64 = 1 << 20;
/// CR4 bit 21: Supervisor Mode Access Prevention.
const CR4_SMAP: u64 = 1 << 21;

/// Canonical upper bound of the user address space on x86_64 (bits 47:0 used,
/// bit 47 = 0 for user, bit 47 = 1 for kernel).
const USER_ADDR_MAX: u64 = 0x0000_7FFF_FFFF_FFFF;

// ── CR4 read / write ──────────────────────────────────────────────────────────

#[inline(always)]
fn read_cr4() -> u64 {
    let val: u64;
    unsafe {
        core::arch::asm!("mov {}, cr4", out(reg) val, options(nomem, nostack));
    }
    val
}

#[inline(always)]
unsafe fn write_cr4(val: u64) {
    core::arch::asm!("mov cr4, {}", in(reg) val, options(nomem, nostack));
}

// ── CPUID feature queries ─────────────────────────────────────────────────────

/// Read CPUID leaf 7, sub-leaf 0 into (eax, ebx, ecx, edx).
fn cpuid7() -> (u32, u32, u32, u32) {
    let (eax, ebx, ecx, edx): (u32, u32, u32, u32);
    unsafe {
        core::arch::asm!(
            "push rbx",
            "xor ecx, ecx",   // sub-leaf 0
            "cpuid",
            "mov {ebx_out:e}, ebx",
            "pop rbx",
            in("eax") 7u32,
            ebx_out = out(reg) ebx,
            lateout("eax") eax,
            lateout("ecx") ecx,
            lateout("edx") edx,
            options(nomem, nostack),
        );
    }
    (eax, ebx, ecx, edx)
}

/// Returns `true` if SMEP is supported (CPUID leaf 7, EBX bit 7).
pub fn smep_supported() -> bool {
    let (_, ebx, _, _) = cpuid7();
    ebx & (1 << 7) != 0
}

/// Returns `true` if SMAP is supported (CPUID leaf 7, EBX bit 20).
pub fn smap_supported() -> bool {
    let (_, ebx, _, _) = cpuid7();
    ebx & (1 << 20) != 0
}

/// Returns `true` if UMIP is supported (CPUID leaf 7, ECX bit 2).
pub fn umip_supported() -> bool {
    let (_, _, ecx, _) = cpuid7();
    ecx & (1 << 2) != 0
}

// ── Feature enablement ────────────────────────────────────────────────────────

/// Set CR4.SMEP (bit 20).
/// Has no effect if SMEP is already on; safe to call redundantly.
pub fn enable_smep() {
    let cr4 = read_cr4();
    unsafe {
        write_cr4(cr4 | CR4_SMEP);
    }
}

/// Set CR4.SMAP (bit 21).
pub fn enable_smap() {
    let cr4 = read_cr4();
    unsafe {
        write_cr4(cr4 | CR4_SMAP);
    }
}

/// Set CR4.UMIP (bit 11).
pub fn enable_umip() {
    let cr4 = read_cr4();
    unsafe {
        write_cr4(cr4 | CR4_UMIP);
    }
}

// ── Temporary SMAP relaxation ─────────────────────────────────────────────────

/// Emit `STAC` — clears AC flag, allowing kernel to access user pages.
///
/// Must be paired with an immediate call to `enable_smap_after_access()`.
/// The window between these two calls must be as short as possible.
#[inline(always)]
pub fn disable_smap_temporarily() {
    unsafe {
        core::arch::asm!("stac", options(nomem, nostack));
    }
}

/// Emit `CLAC` — re-sets AC flag, re-enabling SMAP protection.
#[inline(always)]
pub fn enable_smap_after_access() {
    unsafe {
        core::arch::asm!("clac", options(nomem, nostack));
    }
}

// ── User-space memory copy helpers ───────────────────────────────────────────

/// Validate that an address range lies entirely within user space.
#[inline(always)]
fn is_user_range(addr: u64, len: usize) -> bool {
    if addr > USER_ADDR_MAX {
        return false;
    }
    // Check for overflow: addr + len must not wrap or exceed USER_ADDR_MAX.
    let end = match addr.checked_add(len as u64) {
        Some(e) => e,
        None => return false,
    };
    end <= USER_ADDR_MAX.saturating_add(1)
}

/// Copy `len` bytes from user-space `src` to kernel `dst`.
///
/// Validates that `src` is in the user address range, then temporarily
/// disables SMAP (STAC), copies the bytes with `read_volatile`, and
/// re-enables SMAP (CLAC).
///
/// Returns `true` on success, `false` if the source range is invalid.
pub fn copy_from_user(dst: *mut u8, src: *const u8, len: usize) -> bool {
    if dst.is_null() || src.is_null() || len == 0 {
        return false;
    }
    if !is_user_range(src as u64, len) {
        serial_println!(
            "  [smep-smap] copy_from_user: src {:#x} not in user range",
            src as u64
        );
        return false;
    }

    disable_smap_temporarily();
    for i in 0..len {
        let byte = unsafe { core::ptr::read_volatile(src.add(i)) };
        unsafe {
            core::ptr::write_volatile(dst.add(i), byte);
        }
    }
    enable_smap_after_access();
    true
}

/// Copy `len` bytes from kernel `src` to user-space `dst`.
///
/// Validates that `dst` is in the user address range, then temporarily
/// disables SMAP (STAC), copies the bytes with `write_volatile`, and
/// re-enables SMAP (CLAC).
///
/// Returns `true` on success, `false` if the destination range is invalid.
pub fn copy_to_user(dst: *mut u8, src: *const u8, len: usize) -> bool {
    if dst.is_null() || src.is_null() || len == 0 {
        return false;
    }
    if !is_user_range(dst as u64, len) {
        serial_println!(
            "  [smep-smap] copy_to_user: dst {:#x} not in user range",
            dst as u64
        );
        return false;
    }

    disable_smap_temporarily();
    for i in 0..len {
        let byte = unsafe { core::ptr::read_volatile(src.add(i)) };
        unsafe {
            core::ptr::write_volatile(dst.add(i), byte);
        }
    }
    enable_smap_after_access();
    true
}

// ── NX marking ───────────────────────────────────────────────────────────────

/// Mark a 4 KiB page as non-executable (NX) and non-user-accessible in the
/// current page table.
///
/// Calls into `memory::paging::change_permissions` to set the NO_EXECUTE bit
/// and clear USER_ACCESSIBLE on the PTE for `vaddr`.  Used during init to
/// harden kernel data pages.
///
/// If the page is not currently mapped, the call is silently ignored.
pub fn mark_nx(vaddr: u64) {
    use crate::memory::paging::flags;
    let current = match crate::memory::paging::get_pte(vaddr as usize) {
        Some(pte) => pte,
        None => return, // not mapped — nothing to do
    };

    // Build new flags: preserve all current flags, add NX, remove USER.
    let new_flags = (current & flags::FLAGS_MASK) | flags::NO_EXECUTE & !flags::USER_ACCESSIBLE;

    let _ = crate::memory::paging::change_permissions(vaddr as usize, new_flags);
}

// ── Module init ───────────────────────────────────────────────────────────────

/// Enable SMEP, SMAP, and UMIP if the CPU supports them, then log the result.
///
/// Should be called very early — before any other security subsystem — so that
/// all subsequent code executes under full CPU-enforced protection.
pub fn init() {
    let smep = smep_supported();
    let smap = smap_supported();
    let umip = umip_supported();

    if smep {
        enable_smep();
    }
    if smap {
        enable_smap();
    }
    if umip {
        enable_umip();
    }

    let cr4_after = read_cr4();

    serial_println!("  [smep-smap] CR4 after hardening: 0x{:016X}", cr4_after);
    serial_println!(
        "  [smep-smap] SMEP={} SMAP={} UMIP={} (all enforcement active where supported)",
        if smep { "ON" } else { "NOT SUPPORTED" },
        if smap { "ON" } else { "NOT SUPPORTED" },
        if umip { "ON" } else { "NOT SUPPORTED" },
    );

    // Verify bits landed in CR4 when features were reported as supported.
    if smep && (cr4_after & CR4_SMEP == 0) {
        serial_println!("  [smep-smap] WARNING: SMEP bit did not set in CR4!");
    }
    if smap && (cr4_after & CR4_SMAP == 0) {
        serial_println!("  [smep-smap] WARNING: SMAP bit did not set in CR4!");
    }
    if umip && (cr4_after & CR4_UMIP == 0) {
        serial_println!("  [smep-smap] WARNING: UMIP bit did not set in CR4!");
    }
}
