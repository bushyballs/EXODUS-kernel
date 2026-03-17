/// Build script for Hoags Kernel Genesis
///
/// Tracks linker script changes so Cargo relinks when it's modified.
/// Boot assembly is compiled via global_asm!() in src/boot.rs — no
/// external assembler needed.
fn main() {
    println!("cargo:rerun-if-changed=linker.ld");
}
