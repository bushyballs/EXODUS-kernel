use crate::serial_println;

pub fn report() {
    let cs = super::consciousness_gradient::score();
    let tier = super::consciousness_gradient::tier_name();
    let wp = super::willpower::reserve();
    let pc = super::purpose::coherence();
    let valence = super::integration::current_valence();
    let lf = super::integration::life_force();
    serial_println!("=== EXODUS LIFE REPORT ===");
    serial_println!("  consciousness: {} ({})", cs, tier);
    serial_println!("  willpower: {}", wp);
    serial_println!("  purpose_coherence: {}", pc);
    serial_println!("  valence: {}", valence);
    serial_println!("  life_force: {}", lf);
    serial_println!("==========================");
}
