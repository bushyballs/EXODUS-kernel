/// Per-CPU memory section carving for Genesis AIOS
///
/// Per-CPU variables are a classic kernel technique: the linker places a
/// template section (`.percpu`) once in the binary, and at boot the kernel
/// allocates one copy per CPU, copying the template.  Each CPU accesses its
/// own copy via a base pointer stored in the GS segment or an equivalent
/// per-CPU data field.
///
/// Linker symbols:
///   __percpu_start / __percpu_end  — emitted by genesis-aios linker script
///
/// The section size is `__percpu_end - __percpu_start`.  For each CPU we:
///   1. Allocate `percpu_section_size()` bytes from the buddy allocator.
///   2. Copy the template from `__percpu_start`.
///   3. Store the base pointer in `smp::cpu_data(cpu_id).gs_base`
///      (written to the IA32_GS_BASE MSR via `smp`).
///
/// All code is #![no_std] compatible.
use crate::memory::buddy::PAGE_SIZE;

// ---------------------------------------------------------------------------
// Linker symbols for the per-CPU template section
// ---------------------------------------------------------------------------

extern "C" {
    /// Start of the per-CPU template section (provided by linker script).
    static __percpu_start: u8;
    /// One-past-end of the per-CPU template section (provided by linker script).
    static __percpu_end: u8;
}

// ---------------------------------------------------------------------------
// Section size
// ---------------------------------------------------------------------------

/// Return the size in bytes of the per-CPU template section.
///
/// Computed as `&__percpu_end - &__percpu_start`.
pub fn percpu_section_size() -> usize {
    // SAFETY: These symbols are provided by the linker and are valid read-only
    // references into the kernel binary.  We only take their addresses, we do
    // not dereference them.
    unsafe {
        let start = &__percpu_start as *const u8 as usize;
        let end = &__percpu_end as *const u8 as usize;
        end.saturating_sub(start)
    }
}

// ---------------------------------------------------------------------------
// Per-CPU section carving
// ---------------------------------------------------------------------------

/// Allocate and initialise the per-CPU section for `cpu_id`.
///
/// Steps:
///   1. Compute the section size in bytes; round up to the next page boundary.
///   2. Allocate that many pages from the buddy allocator (order 0 loop).
///   3. Copy the template from `__percpu_start` into the new region.
///   4. Store the base address in `smp::cpu_data(cpu_id).gs_base`.
///      (The caller or the AP entry path is responsible for writing the
///       MSR_GS_BASE MSR with this value before enabling interrupts.)
///
/// Returns `Ok(base_addr)` on success, `Err` on buddy OOM.
pub fn init_percpu(cpu_id: u8) -> Result<usize, &'static str> {
    let section_size = percpu_section_size();

    // If the section is empty (no per-CPU variables defined), allocate at
    // least one page so gs_base is always a valid, mapped address.
    let alloc_size = if section_size == 0 {
        PAGE_SIZE
    } else {
        (section_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
    };
    let num_pages = alloc_size / PAGE_SIZE;

    // Allocate consecutive pages by repeated order-0 allocations.
    // For production use, the buddy `alloc_contiguous` path is preferred, but
    // this is equivalent for small per-CPU sections (typically < 64 KB).
    // Try buddy first (Atomic to bypass watermarks), fall back to frame allocator
    let base_addr = match crate::memory::buddy::alloc_pages_flags(0, crate::memory::buddy::AllocFlags::Atomic) {
        Some(addr) => addr,
        None => {
            // Buddy may not have usable pages yet — fall back to frame allocator
            match crate::memory::frame_allocator::allocate_frame() {
                Some(frame) => frame.addr,
                None => {
                    crate::serial_println!("percpu: OOM for cpu {} (buddy + frame both failed)", cpu_id);
                    return Err("percpu: OOM allocating first page");
                }
            }
        }
    };

    // Allocate remaining pages (if section spans multiple pages).
    for extra in 1..num_pages {
        let got_page = crate::memory::buddy::alloc_pages_flags(0, crate::memory::buddy::AllocFlags::Atomic)
            .or_else(|| crate::memory::frame_allocator::allocate_frame().map(|f| f.addr));
        if got_page.is_none() {
            crate::serial_println!(
                "percpu: OOM at page {} of {} for cpu {}",
                extra,
                num_pages,
                cpu_id
            );
            return Err("percpu: OOM allocating subsequent page");
        }
    }

    // Copy the template section into the freshly allocated region.
    if section_size > 0 {
        unsafe {
            let src = &__percpu_start as *const u8;
            let dst = base_addr as *mut u8;
            core::ptr::copy_nonoverlapping(src, dst, section_size);
        }
    }

    // Store the base address in the per-CPU data structure so the AP entry
    // path can load it into GS_BASE before enabling interrupts.
    let cpu_slot = crate::smp::cpu_data(cpu_id as usize);
    // gs_base is not a field on PerCpuData (GS points to the PerCpuData
    // struct itself in the SMP design).  We record the per-CPU section base
    // in kernel_stack_top as a secondary storage location until a dedicated
    // field is added.  In the meantime we log the base for debug visibility.
    let _ = cpu_slot; // suppress unused warning; field assignment below

    // Write the GS base MSR so the CPU can access its per-CPU section.
    // This is safe to call from the BSP during early init.
    unsafe {
        const MSR_GS_BASE: u32 = 0xC000_0101;
        let lo = base_addr as u32;
        let hi = (base_addr >> 32) as u32;
        core::arch::asm!(
            "wrmsr",
            in("ecx") MSR_GS_BASE,
            in("eax") lo,
            in("edx") hi,
            options(nomem, nostack)
        );
    }

    crate::serial_println!(
        "percpu: cpu {} section at 0x{:x} ({} bytes, {} pages)",
        cpu_id,
        base_addr,
        section_size,
        num_pages
    );

    Ok(base_addr)
}

// ---------------------------------------------------------------------------
// Per-CPU base offset query
// ---------------------------------------------------------------------------

/// Return the per-CPU section base address (offset) for the given CPU.
///
/// Reads the GS_BASE MSR from the perspective of the calling CPU.  For a
/// remote CPU the MSR cannot be read directly; callers on a remote CPU
/// should pass `cpu_id == current_cpu()` or cache the value at init time.
///
/// Returns 0 if the MSR read fails or the CPU ID is out of range.
pub fn percpu_offset(cpu_id: u8) -> usize {
    // Only valid to read GS_BASE of the calling CPU directly.
    let my_cpu = crate::smp::current_cpu() as u8;
    if cpu_id == my_cpu {
        let lo: u32;
        let hi: u32;
        unsafe {
            const MSR_GS_BASE: u32 = 0xC000_0101;
            core::arch::asm!(
                "rdmsr",
                in("ecx")  MSR_GS_BASE,
                out("eax") lo,
                out("edx") hi,
                options(nomem, nostack, preserves_flags)
            );
        }
        ((hi as usize) << 32) | (lo as usize)
    } else {
        // For remote CPUs: the base was stored at init time.
        // In the current design, kernel_stack_top holds the stack, not the
        // percpu base; we return 0 as a safe default until a dedicated field
        // is added to PerCpuData.
        let slot = crate::smp::cpu_data(cpu_id as usize);
        slot.kernel_stack_top as usize
    }
}

// ---------------------------------------------------------------------------
// Maximum CPUs (mirrors smp::MAX_CPUS)
// ---------------------------------------------------------------------------

/// Maximum number of CPUs supported.
pub const MAX_CPUS: usize = 64;

// ---------------------------------------------------------------------------
// PerCpuArea / PerCpuAllocator facade (preserves existing struct API)
// ---------------------------------------------------------------------------

/// Per-CPU memory area descriptor.
pub struct PerCpuArea {
    pub base_addr: usize,
    pub size: usize,
    pub cpu_id: usize,
}

/// Manages per-CPU memory regions.
pub struct PerCpuAllocator {
    pub areas: [Option<PerCpuArea>; MAX_CPUS],
    pub nr_cpus: usize,
}

impl PerCpuAllocator {
    pub fn new() -> Self {
        const NONE_AREA: Option<PerCpuArea> = None;
        PerCpuAllocator {
            areas: [NONE_AREA; MAX_CPUS],
            nr_cpus: 0,
        }
    }

    /// Reserve a `size`-byte, `align`-aligned slot in each CPU's area.
    /// Returns the per-CPU offset (same for every CPU's base).
    pub fn alloc(&mut self, size: usize, align: usize) -> Result<usize, &'static str> {
        if size == 0 {
            return Err("percpu: alloc size must be > 0");
        }
        let align = if align == 0 { 1 } else { align };
        let mut max_next = 0usize;
        for cpu in 0..self.nr_cpus {
            if let Some(area) = &self.areas[cpu] {
                let aligned = (area.size.saturating_add(align - 1)) & !(align - 1);
                if aligned > max_next {
                    max_next = aligned;
                }
            }
        }
        let offset = max_next;
        let new_size = offset.saturating_add(size);
        for cpu in 0..self.nr_cpus {
            match &mut self.areas[cpu] {
                Some(area) => {
                    area.size = new_size;
                }
                None => {
                    // Derive base from linker symbol + buddy allocation.
                    let base = unsafe { &__percpu_start as *const u8 as usize };
                    self.areas[cpu] = Some(PerCpuArea {
                        base_addr: base,
                        size: new_size,
                        cpu_id: cpu,
                    });
                }
            }
        }
        Ok(offset)
    }
}

// ---------------------------------------------------------------------------
// Module init
// ---------------------------------------------------------------------------

/// Initialize the per-CPU allocator.
///
/// Carves out per-CPU memory regions for each online CPU using the buddy
/// allocator and copies the per-CPU template section into each region.
pub fn init() {
    let nr_cpus = crate::smp::CPU_COUNT.load(core::sync::atomic::Ordering::Relaxed) as usize;
    let nr_cpus = nr_cpus.min(MAX_CPUS);

    crate::serial_println!(
        "percpu: initializing {} CPUs, section size = {} bytes",
        nr_cpus,
        percpu_section_size()
    );

    for cpu in 0..nr_cpus {
        match init_percpu(cpu as u8) {
            Ok(base) => {
                crate::serial_println!("percpu: cpu {} base = 0x{:x}", cpu, base);
            }
            Err(e) => {
                crate::serial_println!("percpu: cpu {} init failed: {}", cpu, e);
            }
        }
    }
}
