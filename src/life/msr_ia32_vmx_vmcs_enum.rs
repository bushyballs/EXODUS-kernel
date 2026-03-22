#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { vmcs_field_width: u16, vmcs_index_max: u16, vmcs_richness_ema: u16, vmcs_pad: u16 }
static MODULE: Mutex<State> = Mutex::new(State { vmcs_field_width:0, vmcs_index_max:0, vmcs_richness_ema:0, vmcs_pad:0 });

#[inline]
fn has_vmx() -> bool {
    let ecx: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 1u32 => _,
            lateout("ecx") ecx, lateout("edx") _,
            options(nostack, nomem),
        );
    }
    (ecx >> 5) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_vmx_vmcs_enum] init"); }
pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }
    if !has_vmx() { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x48Au32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    // bits[9:1]: highest VMCS encoding index — breadth of VMCS support
    let raw_max = (lo >> 1) & 0x1FF;
    // Scale to 0-1000 (max possible index ~512)
    let vmcs_index_max = ((raw_max * 1000) / 512).min(1000) as u16;
    // bit 0: 32-bit vs 64-bit access width
    let vmcs_field_width: u16 = if (lo & 1) != 0 { 1000 } else { 500 };
    let composite = (vmcs_index_max as u32/2).saturating_add(vmcs_field_width as u32/2);
    let mut s = MODULE.lock();
    let vmcs_richness_ema = ((s.vmcs_richness_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.vmcs_field_width=vmcs_field_width; s.vmcs_index_max=vmcs_index_max; s.vmcs_richness_ema=vmcs_richness_ema;
    serial_println!("[msr_ia32_vmx_vmcs_enum] age={} width={} idx_max={} ema={}", age, vmcs_field_width, vmcs_index_max, vmcs_richness_ema);
}
pub fn get_vmcs_field_width()    -> u16 { MODULE.lock().vmcs_field_width }
pub fn get_vmcs_index_max()      -> u16 { MODULE.lock().vmcs_index_max }
pub fn get_vmcs_richness_ema()   -> u16 { MODULE.lock().vmcs_richness_ema }
