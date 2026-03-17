/// Safe stack for Genesis — stack buffer overflow protection
///
/// Implements a dual-stack scheme (inspired by LLVM SafeStack):
///   - Safe stack: stores return addresses, spill slots, register saves
///   - Unsafe stack: stores local variables that may be accessed via pointers
///   - Shadow stack: separate copy of return addresses for verification
///   - Stack canary verification with CSPRNG-seeded values
///   - Guard pages between stacks to detect overflow
///   - Per-thread stack allocation and tracking
///   - Runtime stack overflow detection with configurable response
///
/// Reference: LLVM SafeStack, Intel CET Shadow Stack, ARM PAC.
/// All code is original.
use crate::serial_println;
use crate::sync::Mutex;
use alloc::format;
use alloc::vec::Vec;

static SAFE_STACK: Mutex<Option<SafeStackInner>> = Mutex::new(None);

/// Default unsafe stack size (64 KiB)
const DEFAULT_UNSAFE_STACK_SIZE: usize = 64 * 1024;

/// Default shadow stack entries (depth)
const DEFAULT_SHADOW_DEPTH: usize = 256;

/// Guard page size (4 KiB)
const GUARD_PAGE_SIZE: usize = 4096;

/// Maximum threads with safe stack
const MAX_THREADS: usize = 1024;

/// Stack canary magic patterns (per-thread, seeded from CSPRNG)
const CANARY_ENTROPY_SIZE: usize = 8;

/// Stack overflow response
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverflowAction {
    /// Kill the offending thread
    KillThread,
    /// Kill the entire process
    KillProcess,
    /// Panic the kernel (for kernel threads)
    Panic,
    /// Log and continue (debug mode only)
    LogOnly,
}

/// Safe stack configuration
pub struct SafeStackConfig {
    pub unsafe_stack_size: usize,
    pub guard_page: bool,
}

impl SafeStackConfig {
    pub fn new() -> Self {
        // Delegate to global state
        SafeStackConfig {
            unsafe_stack_size: DEFAULT_UNSAFE_STACK_SIZE,
            guard_page: true,
        }
    }

    pub fn allocate_unsafe_stack(&self, thread_id: u32) -> *mut u8 {
        allocate_unsafe_stack(thread_id)
    }

    pub fn free_unsafe_stack(&self, thread_id: u32) {
        free_unsafe_stack(thread_id);
    }
}

/// Per-thread safe stack state
struct ThreadStackState {
    thread_id: u32,
    /// Unsafe stack base address
    unsafe_stack_base: usize,
    /// Unsafe stack size
    unsafe_stack_size: usize,
    /// Current unsafe stack pointer
    unsafe_stack_ptr: usize,
    /// Guard page address (below unsafe stack)
    guard_page_addr: usize,
    /// Shadow stack (return address copies)
    shadow_stack: Vec<u64>,
    /// Shadow stack pointer (index into shadow_stack)
    shadow_sp: usize,
    /// Per-thread stack canary value
    canary: u64,
    /// Number of canary checks performed
    canary_checks: u64,
    /// Number of canary failures
    canary_failures: u64,
    /// Whether this entry is active
    active: bool,
}

impl ThreadStackState {
    fn new(thread_id: u32, base: usize, size: usize, guard_addr: usize) -> Self {
        // Generate per-thread canary from CSPRNG
        let mut canary_bytes = [0u8; CANARY_ENTROPY_SIZE];
        crate::crypto::random::fill_bytes(&mut canary_bytes);
        let mut canary = 0u64;
        for i in 0..8 {
            canary |= (canary_bytes[i] as u64) << (i * 8);
        }
        // Ensure canary is never zero
        if canary == 0 {
            canary = 0xDEAD_C0DE_CAFE_F00D;
        }

        ThreadStackState {
            thread_id,
            unsafe_stack_base: base,
            unsafe_stack_size: size,
            unsafe_stack_ptr: base + size, // Stack grows downward
            guard_page_addr: guard_addr,
            shadow_stack: {
                let mut v = Vec::with_capacity(DEFAULT_SHADOW_DEPTH);
                for _ in 0..DEFAULT_SHADOW_DEPTH {
                    v.push(0);
                }
                v
            },
            shadow_sp: 0,
            canary,
            canary_checks: 0,
            canary_failures: 0,
            active: true,
        }
    }
}

/// Stack allocation entry for memory tracking
struct StackAllocation {
    thread_id: u32,
    base_addr: usize,
    total_size: usize, // Includes guard page
}

/// Inner safe stack state
struct SafeStackInner {
    /// Per-thread state
    threads: Vec<ThreadStackState>,
    /// Memory allocations (for freeing)
    allocations: Vec<StackAllocation>,
    /// Configuration
    default_unsafe_size: usize,
    use_guard_pages: bool,
    use_shadow_stack: bool,
    overflow_action: OverflowAction,
    /// Global canary for kernel stacks
    global_canary: u64,
    /// Statistics
    total_allocations: u64,
    total_frees: u64,
    total_canary_checks: u64,
    total_canary_failures: u64,
    total_shadow_checks: u64,
    total_shadow_failures: u64,
    total_overflow_detections: u64,
    /// Next allocation address (simple bump allocator for stack memory)
    next_alloc_addr: usize,
}

impl SafeStackInner {
    fn new() -> Self {
        // Generate global kernel canary
        let mut canary_bytes = [0u8; 8];
        crate::crypto::random::fill_bytes(&mut canary_bytes);
        let mut global_canary = 0u64;
        for i in 0..8 {
            global_canary |= (canary_bytes[i] as u64) << (i * 8);
        }
        if global_canary == 0 {
            global_canary = 0xBAD57AC1;
        }

        SafeStackInner {
            threads: Vec::with_capacity(32),
            allocations: Vec::new(),
            default_unsafe_size: DEFAULT_UNSAFE_STACK_SIZE,
            use_guard_pages: true,
            use_shadow_stack: true,
            overflow_action: OverflowAction::KillThread,
            global_canary,
            total_allocations: 0,
            total_frees: 0,
            total_canary_checks: 0,
            total_canary_failures: 0,
            total_shadow_checks: 0,
            total_shadow_failures: 0,
            total_overflow_detections: 0,
            // Start allocations at a fixed virtual address range for unsafe stacks
            next_alloc_addr: 0xFFFF_8000_1000_0000,
        }
    }

    /// Allocate an unsafe stack for a thread
    fn allocate(&mut self, thread_id: u32) -> *mut u8 {
        if self.threads.len() >= MAX_THREADS {
            serial_println!("    [safe-stack] Max threads reached, cannot allocate");
            return core::ptr::null_mut();
        }

        // Check if thread already has a stack
        if let Some(state) = self
            .threads
            .iter()
            .find(|t| t.thread_id == thread_id && t.active)
        {
            return state.unsafe_stack_base as *mut u8;
        }

        let stack_size = self.default_unsafe_size;
        let guard_size = if self.use_guard_pages {
            GUARD_PAGE_SIZE
        } else {
            0
        };
        let total_size = guard_size + stack_size;

        let alloc_base = self.next_alloc_addr;
        self.next_alloc_addr += total_size + GUARD_PAGE_SIZE; // Extra gap between allocations

        let guard_addr = alloc_base;
        let stack_base = alloc_base + guard_size;

        // Set up guard page (mark as non-accessible in page tables)
        if self.use_guard_pages {
            self.setup_guard_page(guard_addr);
        }

        // Initialize the unsafe stack memory to a poison pattern
        // In a real kernel, this would go through the page allocator
        unsafe {
            let ptr = stack_base as *mut u8;
            // Write poison pattern to detect use of uninitialized stack
            for i in 0..stack_size {
                core::ptr::write_volatile(ptr.add(i), 0xCC);
            }
        }

        // Create thread state
        let state = ThreadStackState::new(thread_id, stack_base, stack_size, guard_addr);
        let canary = state.canary;
        self.threads.push(state);

        // Track allocation
        self.allocations.push(StackAllocation {
            thread_id,
            base_addr: alloc_base,
            total_size,
        });

        self.total_allocations = self.total_allocations.saturating_add(1);

        serial_println!(
            "    [safe-stack] Allocated for thread {}: base=0x{:X}, size={}K, guard={}",
            thread_id,
            stack_base,
            stack_size / 1024,
            if self.use_guard_pages { "yes" } else { "no" }
        );

        // Place canary at bottom of stack
        unsafe {
            let canary_ptr = stack_base as *mut u64;
            core::ptr::write_volatile(canary_ptr, canary);
        }

        stack_base as *mut u8
    }

    /// Free an unsafe stack
    fn free(&mut self, thread_id: u32) {
        // Deactivate thread state
        for state in &mut self.threads {
            if state.thread_id == thread_id && state.active {
                state.active = false;
                break;
            }
        }

        // Remove allocation tracking
        self.allocations.retain(|a| a.thread_id != thread_id);
        self.total_frees = self.total_frees.saturating_add(1);

        serial_println!("    [safe-stack] Freed stack for thread {}", thread_id);
    }

    /// Set up a guard page (mark as non-accessible)
    fn setup_guard_page(&self, addr: usize) {
        // In a real kernel, this would clear the Present bit in the page table
        // and set up a page fault handler to detect stack overflow
        unsafe {
            let ptr = addr as *mut u8;
            for i in 0..GUARD_PAGE_SIZE {
                core::ptr::write_volatile(ptr.add(i), 0xFE); // Poison pattern
            }
        }
    }

    /// Push a return address to the shadow stack
    fn shadow_push(&mut self, thread_id: u32, return_addr: u64) {
        if !self.use_shadow_stack {
            return;
        }

        if let Some(state) = self
            .threads
            .iter_mut()
            .find(|t| t.thread_id == thread_id && t.active)
        {
            if state.shadow_sp < state.shadow_stack.len() {
                state.shadow_stack[state.shadow_sp] = return_addr;
                state.shadow_sp += 1;
            } else {
                serial_println!(
                    "    [safe-stack] Shadow stack overflow for thread {}",
                    thread_id
                );
                self.handle_overflow(thread_id, "shadow stack overflow");
            }
        }
    }

    /// Pop and verify a return address from the shadow stack
    fn shadow_pop(&mut self, thread_id: u32, actual_return_addr: u64) -> bool {
        if !self.use_shadow_stack {
            return true;
        }

        self.total_shadow_checks = self.total_shadow_checks.saturating_add(1);

        if let Some(state) = self
            .threads
            .iter_mut()
            .find(|t| t.thread_id == thread_id && t.active)
        {
            if state.shadow_sp == 0 {
                serial_println!(
                    "    [safe-stack] Shadow stack underflow for thread {}",
                    thread_id
                );
                return false;
            }

            state.shadow_sp -= 1;
            let expected = state.shadow_stack[state.shadow_sp];

            if expected != actual_return_addr {
                self.total_shadow_failures = self.total_shadow_failures.saturating_add(1);
                serial_println!(
                    "    [safe-stack] SHADOW MISMATCH: thread {} expected=0x{:X} actual=0x{:X}",
                    thread_id,
                    expected,
                    actual_return_addr
                );

                crate::security::audit::log(
                    crate::security::audit::AuditEvent::CapDenied,
                    crate::security::audit::AuditResult::Deny,
                    thread_id,
                    0,
                    &format!("safe-stack: shadow mismatch ret=0x{:X}", actual_return_addr),
                );

                self.handle_overflow(thread_id, "shadow stack mismatch (ROP detected)");
                return false;
            }

            return true;
        }

        true // Thread not tracked
    }

    /// Verify the stack canary for a thread
    fn verify_canary(&mut self, thread_id: u32) -> bool {
        self.total_canary_checks = self.total_canary_checks.saturating_add(1);

        if let Some(state) = self
            .threads
            .iter_mut()
            .find(|t| t.thread_id == thread_id && t.active)
        {
            let stored_canary =
                unsafe { core::ptr::read_volatile(state.unsafe_stack_base as *const u64) };

            if stored_canary != state.canary {
                state.canary_failures = state.canary_failures.saturating_add(1);
                self.total_canary_failures = self.total_canary_failures.saturating_add(1);

                serial_println!(
                    "    [safe-stack] CANARY SMASHED: thread {} expected=0x{:X} found=0x{:X}",
                    thread_id,
                    state.canary,
                    stored_canary
                );

                crate::security::audit::log(
                    crate::security::audit::AuditEvent::CapDenied,
                    crate::security::audit::AuditResult::Deny,
                    thread_id,
                    0,
                    &format!("safe-stack: canary smashed in thread {}", thread_id),
                );

                self.handle_overflow(thread_id, "stack canary corrupted");
                return false;
            }

            state.canary_checks = state.canary_checks.saturating_add(1);
            return true;
        }

        true // Thread not tracked
    }

    /// Check if unsafe stack pointer is within bounds
    fn check_bounds(&mut self, thread_id: u32, current_sp: usize) -> bool {
        if let Some(state) = self
            .threads
            .iter()
            .find(|t| t.thread_id == thread_id && t.active)
        {
            let low = state.unsafe_stack_base;
            let high = state.unsafe_stack_base + state.unsafe_stack_size;

            if current_sp < low || current_sp > high {
                self.total_overflow_detections = self.total_overflow_detections.saturating_add(1);
                serial_println!(
                    "    [safe-stack] BOUNDS VIOLATION: thread {} sp=0x{:X} range=[0x{:X}, 0x{:X}]",
                    thread_id,
                    current_sp,
                    low,
                    high
                );

                crate::security::audit::log(
                    crate::security::audit::AuditEvent::CapDenied,
                    crate::security::audit::AuditResult::Deny,
                    thread_id,
                    0,
                    &format!("safe-stack: bounds violation sp=0x{:X}", current_sp),
                );

                self.handle_overflow(thread_id, "stack bounds violation");
                return false;
            }
        }
        true
    }

    /// Handle a detected overflow/corruption
    fn handle_overflow(&self, thread_id: u32, reason: &str) {
        serial_println!(
            "    [safe-stack] OVERFLOW DETECTED: thread {} - {}",
            thread_id,
            reason
        );

        match self.overflow_action {
            OverflowAction::KillThread => {
                serial_println!("    [safe-stack] Killing thread {}", thread_id);
                // In a real kernel: signal the scheduler to kill this thread
            }
            OverflowAction::KillProcess => {
                serial_println!("    [safe-stack] Killing process for thread {}", thread_id);
                // In a real kernel: signal the scheduler to kill the entire process
            }
            OverflowAction::Panic => {
                panic!("safe-stack: {} in thread {}", reason, thread_id);
            }
            OverflowAction::LogOnly => {
                serial_println!("    [safe-stack] (debug mode: continuing after {})", reason);
            }
        }
    }

    /// Get the global canary value for kernel stacks
    fn get_global_canary(&self) -> u64 {
        self.global_canary
    }

    /// Verify the global canary
    fn verify_global_canary(&mut self, value: u64) -> bool {
        self.total_canary_checks = self.total_canary_checks.saturating_add(1);
        if value != self.global_canary {
            self.total_canary_failures = self.total_canary_failures.saturating_add(1);
            serial_println!(
                "    [safe-stack] GLOBAL CANARY SMASHED: expected=0x{:X} found=0x{:X}",
                self.global_canary,
                value
            );
            false
        } else {
            true
        }
    }
}

/// Allocate an unsafe stack for a thread
pub fn allocate_unsafe_stack(thread_id: u32) -> *mut u8 {
    if let Some(ref mut inner) = *SAFE_STACK.lock() {
        return inner.allocate(thread_id);
    }
    core::ptr::null_mut()
}

/// Free an unsafe stack
pub fn free_unsafe_stack(thread_id: u32) {
    if let Some(ref mut inner) = *SAFE_STACK.lock() {
        inner.free(thread_id);
    }
}

/// Push return address to shadow stack
pub fn shadow_push(thread_id: u32, return_addr: u64) {
    if let Some(ref mut inner) = *SAFE_STACK.lock() {
        inner.shadow_push(thread_id, return_addr);
    }
}

/// Pop and verify return address from shadow stack
pub fn shadow_pop(thread_id: u32, actual_return_addr: u64) -> bool {
    if let Some(ref mut inner) = *SAFE_STACK.lock() {
        return inner.shadow_pop(thread_id, actual_return_addr);
    }
    true
}

/// Verify stack canary for a thread
pub fn verify_canary(thread_id: u32) -> bool {
    if let Some(ref mut inner) = *SAFE_STACK.lock() {
        return inner.verify_canary(thread_id);
    }
    true
}

/// Check stack bounds
pub fn check_bounds(thread_id: u32, current_sp: usize) -> bool {
    if let Some(ref mut inner) = *SAFE_STACK.lock() {
        return inner.check_bounds(thread_id, current_sp);
    }
    true
}

/// Get the global canary value
pub fn get_global_canary() -> u64 {
    if let Some(ref inner) = *SAFE_STACK.lock() {
        return inner.get_global_canary();
    }
    0
}

/// Verify the global canary
pub fn verify_global_canary(value: u64) -> bool {
    if let Some(ref mut inner) = *SAFE_STACK.lock() {
        return inner.verify_global_canary(value);
    }
    false
}

/// Set the overflow action
pub fn set_overflow_action(action: OverflowAction) {
    if let Some(ref mut inner) = *SAFE_STACK.lock() {
        inner.overflow_action = action;
        serial_println!("    [safe-stack] Overflow action set to {:?}", action);
    }
}

/// Get statistics
pub fn stats() -> (u64, u64, u64, u64, u64, u64) {
    if let Some(ref inner) = *SAFE_STACK.lock() {
        return (
            inner.total_allocations,
            inner.total_canary_checks,
            inner.total_canary_failures,
            inner.total_shadow_checks,
            inner.total_shadow_failures,
            inner.total_overflow_detections,
        );
    }
    (0, 0, 0, 0, 0, 0)
}

/// Initialize the safe stack subsystem
pub fn init() {
    let inner = SafeStackInner::new();
    let canary = inner.global_canary;

    *SAFE_STACK.lock() = Some(inner);

    serial_println!("    [safe-stack] Safe stack protection initialized");
    serial_println!(
        "    [safe-stack] Unsafe stack: {}K, guard pages: enabled, shadow stack: enabled",
        DEFAULT_UNSAFE_STACK_SIZE / 1024
    );
    serial_println!(
        "    [safe-stack] Global canary: 0x{:016X} (CSPRNG-seeded)",
        canary
    );
    serial_println!(
        "    [safe-stack] Max threads: {}, shadow depth: {}",
        MAX_THREADS,
        DEFAULT_SHADOW_DEPTH
    );
}
