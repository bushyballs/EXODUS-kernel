pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

/// Kernel code segment selector (offset 0x08 in GDT, ring 0)
pub const KERNEL_CS: u16 = 0x08;
/// Kernel data segment selector (offset 0x10 in GDT, ring 0)
pub const KERNEL_DS: u16 = 0x10;
/// User data segment selector (offset 0x18 in GDT, ring 3)
pub const USER_DS: u16 = 0x1B; // 0x18 | RPL 3
/// User code segment selector (offset 0x20 in GDT, ring 3)
pub const USER_CS: u16 = 0x23; // 0x20 | RPL 3
/// TSS segment selector (offset 0x28 in GDT)
pub const TSS_SEL: u16 = 0x28;

/// Task State Segment — used for interrupt stack switching
#[repr(C, packed)]
struct Tss {
    reserved0: u32,
    /// Privilege stack table (RSP for ring 0, 1, 2)
    rsp: [u64; 3],
    reserved1: u64,
    /// Interrupt Stack Table (7 entries)
    ist: [u64; 7],
    reserved2: u64,
    reserved3: u16,
    /// I/O map base address
    iomap_base: u16,
}

impl Tss {
    const fn new() -> Self {
        Tss {
            reserved0: 0,
            rsp: [0; 3],
            reserved1: 0,
            ist: [0; 7],
            reserved2: 0,
            reserved3: 0,
            iomap_base: core::mem::size_of::<Tss>() as u16,
        }
    }
}

/// Double fault handler stack (20 KB)
const DF_STACK_SIZE: usize = 4096 * 5;
static mut DF_STACK: [u8; DF_STACK_SIZE] = [0; DF_STACK_SIZE];

/// The TSS instance
static mut TSS: Tss = Tss::new();

/// GDT with 7 entries (null + kernel CS/DS + user DS/CS + TSS low/high)
#[repr(C, align(8))]
struct Gdt {
    entries: [u64; 7],
}

/// GDT pointer for LGDT instruction
#[repr(C, packed)]
struct GdtPointer {
    limit: u16,
    base: u64,
}

static mut GDT: Gdt = Gdt { entries: [0; 7] };

/// Build a 64-bit code segment descriptor
const fn code_segment_descriptor() -> u64 {
    let mut d: u64 = 0;
    // Accessed, Readable, Executable, Code/Data=1, DPL=0, Present
    d |= 1 << 40; // Accessed
    d |= 1 << 41; // Readable
    d |= 1 << 43; // Executable
    d |= 1 << 44; // Code/data (not system)
    d |= 1 << 47; // Present
    d |= 1 << 53; // Long mode (64-bit)
    d
}

/// Build a 64-bit data segment descriptor
const fn data_segment_descriptor() -> u64 {
    let mut d: u64 = 0;
    d |= 1 << 40; // Accessed
    d |= 1 << 41; // Writable
    d |= 1 << 44; // Code/data (not system)
    d |= 1 << 47; // Present
    d
}

/// Build a 64-bit user code segment descriptor (DPL 3)
const fn user_code_segment_descriptor() -> u64 {
    let mut d: u64 = 0;
    d |= 1 << 40; // Accessed
    d |= 1 << 41; // Readable
    d |= 1 << 43; // Executable
    d |= 1 << 44; // Code/data (not system)
    d |= 3 << 45; // DPL = 3 (user mode)
    d |= 1 << 47; // Present
    d |= 1 << 53; // Long mode (64-bit)
    d
}

/// Build a 64-bit user data segment descriptor (DPL 3)
const fn user_data_segment_descriptor() -> u64 {
    let mut d: u64 = 0;
    d |= 1 << 40; // Accessed
    d |= 1 << 41; // Writable
    d |= 1 << 44; // Code/data (not system)
    d |= 3 << 45; // DPL = 3 (user mode)
    d |= 1 << 47; // Present
    d
}

/// Build a TSS descriptor (two 64-bit entries for a 16-byte descriptor)
fn tss_descriptor(tss_addr: u64, tss_size: u64) -> (u64, u64) {
    let mut low: u64 = 0;

    // Limit bits 15:0
    low |= (tss_size & 0xFFFF) as u64;
    // Base bits 23:0
    low |= ((tss_addr & 0xFFFF) << 16) as u64;
    low |= (((tss_addr >> 16) & 0xFF) << 32) as u64;
    // Type: 0x9 = 64-bit TSS (Available)
    low |= 0x9u64 << 40;
    // Present
    low |= 1u64 << 47;
    // Limit bits 19:16
    low |= (((tss_size >> 16) & 0xF) << 48) as u64;
    // Base bits 31:24
    low |= (((tss_addr >> 24) & 0xFF) << 56) as u64;

    // High 64 bits: base bits 63:32
    let high = (tss_addr >> 32) & 0xFFFFFFFF;

    (low, high)
}

pub fn init() {
    unsafe {
        // Set up IST entry for double faults
        let stack_end = &raw const DF_STACK as *const _ as u64 + DF_STACK_SIZE as u64;
        TSS.ist[DOUBLE_FAULT_IST_INDEX as usize] = stack_end;

        // Build GDT entries
        GDT.entries[0] = 0; // 0x00: Null
        GDT.entries[1] = code_segment_descriptor(); // 0x08: Kernel CS
        GDT.entries[2] = data_segment_descriptor(); // 0x10: Kernel DS
        GDT.entries[3] = user_data_segment_descriptor(); // 0x18: User DS (DPL 3)
        GDT.entries[4] = user_code_segment_descriptor(); // 0x20: User CS (DPL 3)

        let tss_addr = &raw const TSS as *const Tss as u64;
        let tss_size = (core::mem::size_of::<Tss>() - 1) as u64;
        let (tss_low, tss_high) = tss_descriptor(tss_addr, tss_size);
        GDT.entries[5] = tss_low; // 0x28: TSS low
        GDT.entries[6] = tss_high; // 0x30: TSS high

        // Load GDT
        let gdt_ptr = GdtPointer {
            limit: (core::mem::size_of::<Gdt>() - 1) as u16,
            base: &raw const GDT as *const Gdt as u64,
        };

        core::arch::asm!(
            "lgdt [{}]",
            in(reg) &gdt_ptr as *const GdtPointer,
            options(nostack),
        );

        // Reload code segment via far return
        core::arch::asm!(
            "push {sel}",
            "lea {tmp}, [rip + 2f]",
            "push {tmp}",
            "retfq",
            "2:",
            sel = in(reg) KERNEL_CS as u64,
            tmp = lateout(reg) _,
            options(nostack),
        );

        // Load TSS
        core::arch::asm!(
            "ltr {0:x}",
            in(reg) TSS_SEL,
            options(nostack, nomem),
        );

        // Reload data segments
        core::arch::asm!(
            "mov ds, {0:x}",
            "mov es, {0:x}",
            "mov ss, {0:x}",
            in(reg) KERNEL_DS,
            options(nostack, nomem),
        );
    }
}
