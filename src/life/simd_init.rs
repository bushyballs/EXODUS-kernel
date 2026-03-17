//! simd_init.rs — Enable SSE + AVX2 from safe Rust
//!
//! Called AFTER paging and interrupts are set up.
//! This is the correct place to enable SIMD — not in boot asm.
//! Once enabled, LLVM can use AVX2 instructions in functions
//! marked with #[target_feature(enable = "avx2")].

use crate::serial_println;
use core::arch::asm;

static mut SIMD_ENABLED: bool = false;

/// Enable SSE + AVX2 on the current CPU.
/// Must be called after paging is fully initialized.
pub fn init() {
    // Check CPUID for SSE and AVX support before enabling
    let has_sse: bool;
    let has_avx: bool;
    let has_xsave: bool;

    unsafe {
        let ecx: u32;
        let edx: u32;
        asm!(
            "push rbx",      // save rbx (LLVM reserved)
            "mov eax, 1",
            "cpuid",
            "pop rbx",       // restore rbx
            out("ecx") ecx,
            out("edx") edx,
            out("eax") _,
            options(nomem, nostack),
        );
        has_sse = (edx & (1 << 25)) != 0; // SSE
        has_xsave = (ecx & (1 << 26)) != 0; // XSAVE
        has_avx = (ecx & (1 << 28)) != 0; // AVX
    }

    if !has_sse {
        serial_println!("[simd] No SSE support — running scalar only");
        return;
    }

    // Enable SSE: CR0.EM=0, CR0.MP=1, CR4.OSFXSR=1, CR4.OSXMMEXCPT=1
    unsafe {
        let mut cr0: u64;
        asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack));
        cr0 &= !(1u64 << 2); // clear EM
        cr0 |= 1u64 << 1; // set MP
        asm!("mov cr0, {}", in(reg) cr0, options(nomem, nostack));

        let mut cr4: u64;
        asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack));
        cr4 |= (1u64 << 9) | (1u64 << 10); // OSFXSR + OSXMMEXCPT
        asm!("mov cr4, {}", in(reg) cr4, options(nomem, nostack));
    }

    serial_println!("[simd] SSE enabled.");

    // Enable AVX if supported
    if has_xsave && has_avx {
        unsafe {
            // Set CR4.OSXSAVE (bit 18)
            let mut cr4: u64;
            asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack));
            cr4 |= 1u64 << 18;
            asm!("mov cr4, {}", in(reg) cr4, options(nomem, nostack));

            // Enable AVX in XCR0
            let xcr0_low: u32;
            let xcr0_high: u32;
            asm!(
                "xgetbv",
                in("ecx") 0u32,
                out("eax") xcr0_low,
                out("edx") xcr0_high,
                options(nomem, nostack),
            );
            let new_xcr0 = xcr0_low | 0x07; // x87 + SSE + AVX
            asm!(
                "xsetbv",
                in("ecx") 0u32,
                in("eax") new_xcr0,
                in("edx") xcr0_high,
                options(nomem, nostack),
            );

            SIMD_ENABLED = true;
        }
        serial_println!("[simd] AVX2 enabled. 256-bit SIMD active. Ready for 1T training.");
    } else {
        serial_println!(
            "[simd] AVX not available (XSAVE={}, AVX={}). Using SSE only.",
            has_xsave,
            has_avx
        );
        unsafe {
            SIMD_ENABLED = false;
        }
    }
}

/// Check if SIMD is enabled
pub fn is_enabled() -> bool {
    unsafe { SIMD_ENABLED }
}
