use crate::drivers::pci;
use crate::io::{inw, outw};
use crate::memory::paging;
/// Bochs/QEMU VBE framebuffer driver for Genesis — built from scratch
///
/// Programs the Bochs VBE display adapter (VGA device 1234:1111) to
/// switch from text mode to a high-resolution linear framebuffer.
///
/// The Bochs VBE interface uses I/O ports:
///   0x01CE — VBE index register
///   0x01CF — VBE data register
///
/// The linear framebuffer is at PCI BAR0 of the VGA device.
///
/// No external crates. All code is original.
use crate::{serial_print, serial_println};

/// VBE dispi index port
const VBE_DISPI_INDEX: u16 = 0x01CE;
/// VBE dispi data port
const VBE_DISPI_DATA: u16 = 0x01CF;

/// VBE register indices
const VBE_DISPI_INDEX_ID: u16 = 0;
const VBE_DISPI_INDEX_XRES: u16 = 1;
const VBE_DISPI_INDEX_YRES: u16 = 2;
const VBE_DISPI_INDEX_BPP: u16 = 3;
const VBE_DISPI_INDEX_ENABLE: u16 = 4;
const VBE_DISPI_INDEX_BANK: u16 = 5;
const VBE_DISPI_INDEX_VIRT_WIDTH: u16 = 6;
const VBE_DISPI_INDEX_VIRT_HEIGHT: u16 = 7;
const VBE_DISPI_INDEX_X_OFFSET: u16 = 8;
const VBE_DISPI_INDEX_Y_OFFSET: u16 = 9;

/// VBE enable flags
const VBE_DISPI_DISABLED: u16 = 0x00;
const VBE_DISPI_ENABLED: u16 = 0x01;
const VBE_DISPI_LFB_ENABLED: u16 = 0x40;

/// Write a VBE register
fn vbe_write(index: u16, value: u16) {
    outw(VBE_DISPI_INDEX, index);
    outw(VBE_DISPI_DATA, value);
}

/// Read a VBE register
fn vbe_read(index: u16) -> u16 {
    outw(VBE_DISPI_INDEX, index);
    inw(VBE_DISPI_DATA)
}

/// Set the VBE display mode
///
/// Switches from VGA text mode to a linear framebuffer at the
/// specified resolution and bit depth.
fn set_mode(width: u16, height: u16, bpp: u16) {
    // Disable display first
    vbe_write(VBE_DISPI_INDEX_ENABLE, VBE_DISPI_DISABLED);

    // Set resolution and depth
    vbe_write(VBE_DISPI_INDEX_XRES, width);
    vbe_write(VBE_DISPI_INDEX_YRES, height);
    vbe_write(VBE_DISPI_INDEX_BPP, bpp);

    // Enable with linear framebuffer
    vbe_write(
        VBE_DISPI_INDEX_ENABLE,
        VBE_DISPI_ENABLED | VBE_DISPI_LFB_ENABLED,
    );
}

/// Find the VGA framebuffer address from PCI
fn find_framebuffer_addr() -> Option<usize> {
    // Look for the Bochs VGA device (1234:1111)
    let devices = pci::find_by_id(0x1234, 0x1111);
    if let Some(dev) = devices.first() {
        let (bar0, is_mmio) = pci::read_bar(dev.bus, dev.device, dev.function, 0);
        if is_mmio && bar0 != 0 {
            serial_println!("  BochsVGA: framebuffer at {:#x} (from PCI BAR0)", bar0);
            return Some(bar0 as usize);
        }
    }

    // Fallback: try standard Bochs VGA framebuffer address
    serial_println!("  BochsVGA: using fallback address 0xFD000000");
    Some(0xFD00_0000)
}

/// Identity-map the framebuffer region so the CPU can access it
fn map_framebuffer(addr: usize, size: usize) {
    let pages = size.saturating_add(0xFFF) / 0x1000;
    for i in 0..pages {
        let page = addr.saturating_add(i.saturating_mul(0x1000));
        // Identity-map: virtual = physical, writable, no-cache for MMIO
        let flags = paging::flags::WRITABLE | paging::flags::NO_CACHE;
        let _ = paging::map_page(page, page, flags);
    }
}

/// Initialize the Bochs VBE display
///
/// Switches to 1024x768x32 graphics mode and sets up the framebuffer.
pub fn init() -> bool {
    // Check if VBE interface is available
    let id = vbe_read(VBE_DISPI_INDEX_ID);
    if id < 0xB0C0 {
        serial_println!("  BochsVGA: VBE interface not found (id={:#x})", id);
        return false;
    }

    serial_println!("  BochsVGA: VBE interface detected (id={:#x})", id);

    // Find framebuffer address
    let fb_addr = match find_framebuffer_addr() {
        Some(addr) => addr,
        None => {
            serial_println!("  BochsVGA: could not find framebuffer address");
            return false;
        }
    };

    let width: u32 = 1024;
    let height: u32 = 768;
    let bpp: u32 = 32;
    let pitch = width * (bpp / 8);
    let fb_size = (pitch * height) as usize;

    // Map the framebuffer into our address space
    map_framebuffer(fb_addr, fb_size);

    // Switch to graphics mode
    set_mode(width as u16, height as u16, bpp as u16);

    // Clear framebuffer to dark Hoags background (volatile writes for MMIO)
    let bg_color: u32 = 0x00121218; // Hoags Dark (Bochs uses 0x00RRGGBB)
    unsafe {
        let fb = fb_addr as *mut u32;
        for i in 0..(width * height) as usize {
            core::ptr::write_volatile(fb.add(i), bg_color);
        }
    }

    // Register with the framebuffer driver
    super::framebuffer::set_graphics_mode(fb_addr, width, height, bpp / 8, pitch);

    serial_println!(
        "  BochsVGA: mode set to {}x{}x{}, fb at {:#x}",
        width,
        height,
        bpp,
        fb_addr
    );
    true
}
