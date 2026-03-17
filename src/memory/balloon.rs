use crate::sync::Mutex;
use alloc::vec::Vec;
/// Hypervisor memory balloon driver for Genesis AIOS
///
/// Implements VM memory ballooning — the host hypervisor can request the
/// guest to "inflate" (give back) pages or "deflate" (reclaim) pages to
/// dynamically rebalance physical memory across VMs.
///
/// Hypervisor detection uses CPUID leaf 0x40000000 (KVM, VMware, Hyper-V).
/// Page tracking uses a global frame list protected by a Mutex.
/// PCI scanning locates the virtio-balloon device (vendor 0x1AF4, device 0x1002)
/// to notify the hypervisor of inflated page lists.
///
/// All code is #![no_std] compatible.
use core::sync::atomic::{AtomicUsize, Ordering};

// ---------------------------------------------------------------------------
// Virtio-balloon PCI identity
// ---------------------------------------------------------------------------

/// Virtio PCI vendor ID
const VIRTIO_VENDOR_ID: u16 = 0x1AF4;

/// Virtio balloon device ID (legacy)
const VIRTIO_BALLOON_DEVICE_ID: u16 = 0x1002;

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

/// Physical page frames (addresses) currently surrendered to the hypervisor.
static BALLOON_PAGES: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// Current balloon size in pages (lockless fast read).
pub static BALLOON_SIZE_PAGES: AtomicUsize = AtomicUsize::new(0);

// ---------------------------------------------------------------------------
// Hypervisor detection via CPUID leaf 0x40000000
// ---------------------------------------------------------------------------

/// Detect whether the kernel is running inside a supported hypervisor.
///
/// Executes CPUID with EAX=0x40000000. The hypervisor signature is packed
/// into EBX, ECX, EDX as a 12-byte ASCII string:
///   - KVM:      "KVMKVMKVM\0\0\0"
///   - VMware:   "VMwareVMware"
///   - Hyper-V:  "Microsoft Hv"
///
/// Returns `true` if any of those strings is found.
pub fn detect_hypervisor() -> bool {
    let ebx: u32;
    let ecx: u32;
    let edx: u32;

    unsafe {
        // CPUID clobbers EAX/EBX/ECX/EDX.  We save/restore RBX ourselves
        // because it is callee-saved in the System V AMD64 ABI and LLVM's
        // inline-asm constraint handling for "rbx" is tricky on x86_64.
        core::arch::asm!(
            "push rbx",
            "mov eax, 0x40000000",
            "cpuid",
            "mov {0:e}, ebx",
            "mov {1:e}, ecx",
            "mov {2:e}, edx",
            "pop rbx",
            out(reg) ebx,
            out(reg) ecx,
            out(reg) edx,
            out("eax") _,
        );
    }

    // Pack the three registers into a 12-byte array for comparison.
    let sig: [u8; 12] = [
        (ebx & 0xFF) as u8,
        ((ebx >> 8) & 0xFF) as u8,
        ((ebx >> 16) & 0xFF) as u8,
        ((ebx >> 24) & 0xFF) as u8,
        (ecx & 0xFF) as u8,
        ((ecx >> 8) & 0xFF) as u8,
        ((ecx >> 16) & 0xFF) as u8,
        ((ecx >> 24) & 0xFF) as u8,
        (edx & 0xFF) as u8,
        ((edx >> 8) & 0xFF) as u8,
        ((edx >> 16) & 0xFF) as u8,
        ((edx >> 24) & 0xFF) as u8,
    ];

    // "KVMKVMKVM\0\0\0"
    const KVM_SIG: [u8; 12] = *b"KVMKVMKVM\0\0\0";
    // "VMwareVMware"
    const VMWARE_SIG: [u8; 12] = *b"VMwareVMware";
    // "Microsoft Hv"
    const HYPERV_SIG: [u8; 12] = *b"Microsoft Hv";

    sig == KVM_SIG || sig == VMWARE_SIG || sig == HYPERV_SIG
}

// ---------------------------------------------------------------------------
// PCI scan helper — locate virtio-balloon device
// ---------------------------------------------------------------------------

/// Scan the PCI bus for a virtio-balloon device.
///
/// Returns the BAR0 MMIO base address if found, or 0 if the device is absent.
fn find_virtio_balloon_bar0() -> u64 {
    use crate::pci::{pci_read_config, PciAddress};

    for bus in 0u8..=255 {
        for dev in 0u8..32 {
            for func in 0u8..8 {
                let addr = PciAddress::new(bus, dev, func);
                let ids = pci_read_config(addr, 0x00);
                let vendor = (ids & 0xFFFF) as u16;
                if vendor == 0xFFFF {
                    // No device; if function 0 skip remaining functions.
                    if func == 0 {
                        break;
                    }
                    continue;
                }
                let device_id = (ids >> 16) as u16;
                if vendor == VIRTIO_VENDOR_ID && device_id == VIRTIO_BALLOON_DEVICE_ID {
                    // BAR0 is at config offset 0x10.
                    let bar0 = pci_read_config(addr, 0x10) as u64;
                    // Memory BAR: strip the lower flags bits.
                    let base = bar0 & !0xF_u64;
                    crate::serial_println!(
                        "balloon: found virtio-balloon at {:02x}:{:02x}.{} BAR0=0x{:x}",
                        bus,
                        dev,
                        func,
                        base
                    );
                    return base;
                }
                // Multi-function check: if function 0 and not MF, skip remaining.
                if func == 0 {
                    let hdr = (pci_read_config(addr, 0x0C) >> 16) as u8;
                    if hdr & 0x80 == 0 {
                        break;
                    }
                }
            }
        }
    }
    0
}

// ---------------------------------------------------------------------------
// Inflate: give pages back to the hypervisor
// ---------------------------------------------------------------------------

/// Allocate `num_pages` physical frames from the buddy allocator and hand
/// them to the hypervisor (balloon inflation).
///
/// Each allocated frame address is stored in `BALLOON_PAGES` and the list is
/// written to the virtio-balloon PCI device's page-list MMIO region (if the
/// device is present).  `BALLOON_SIZE_PAGES` is updated atomically.
pub fn balloon_inflate(num_pages: usize) {
    if num_pages == 0 {
        return;
    }

    let bar0 = find_virtio_balloon_bar0();
    let mut pages = BALLOON_PAGES.lock();

    let mut inflated = 0usize;
    for _ in 0..num_pages {
        match crate::memory::buddy::alloc_page() {
            Some(addr) => {
                let frame = addr as u64;
                pages.push(frame);

                // Notify the virtio-balloon device by writing the guest
                // physical frame number (4KB granularity) to MMIO offset 0.
                // The virtio-balloon spec uses a queue-based protocol; this
                // direct MMIO write is a simplified notification compatible
                // with the legacy MMIO transport (virtio-mmio).
                if bar0 != 0 {
                    let pfn = (frame / 4096) as u32;
                    unsafe {
                        core::ptr::write_volatile(bar0 as *mut u32, pfn);
                    }
                }

                inflated = inflated.saturating_add(1);
            }
            None => {
                crate::serial_println!(
                    "balloon: inflate stalled after {} pages — buddy OOM",
                    inflated
                );
                break;
            }
        }
    }

    BALLOON_SIZE_PAGES.fetch_add(inflated, Ordering::Relaxed);
    crate::serial_println!(
        "balloon: inflated {} pages (total balloon = {})",
        inflated,
        BALLOON_SIZE_PAGES.load(Ordering::Relaxed)
    );
}

// ---------------------------------------------------------------------------
// Deflate: reclaim pages from the hypervisor
// ---------------------------------------------------------------------------

/// Return up to `num_pages` previously-inflated frames to the buddy
/// allocator (balloon deflation).
///
/// Frames are popped from `BALLOON_PAGES` and freed via
/// `buddy::free_page()`.  `BALLOON_SIZE_PAGES` is decremented accordingly.
pub fn balloon_deflate(num_pages: usize) {
    if num_pages == 0 {
        return;
    }

    let mut pages = BALLOON_PAGES.lock();
    let to_free = num_pages.min(pages.len());

    for _ in 0..to_free {
        if let Some(frame) = pages.pop() {
            crate::memory::buddy::free_page(frame as usize);
        }
    }

    BALLOON_SIZE_PAGES.fetch_sub(to_free, Ordering::Relaxed);
    crate::serial_println!(
        "balloon: deflated {} pages (total balloon = {})",
        to_free,
        BALLOON_SIZE_PAGES.load(Ordering::Relaxed)
    );
}

// ---------------------------------------------------------------------------
// Current balloon size (lockless)
// ---------------------------------------------------------------------------

/// Return the current balloon size in pages.
#[inline]
pub fn balloon_size_pages() -> usize {
    BALLOON_SIZE_PAGES.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// BalloonDriver facade (preserves existing struct API)
// ---------------------------------------------------------------------------

/// Current balloon state tracking inflated/deflated pages.
pub struct BalloonDriver {
    /// Number of pages currently given back to the hypervisor.
    pub inflated_pages: usize,
    /// Target balloon size requested by the hypervisor.
    pub target_pages: usize,
}

impl BalloonDriver {
    pub fn new() -> Self {
        BalloonDriver {
            inflated_pages: 0,
            target_pages: 0,
        }
    }

    /// Inflate the balloon (return pages to host).
    pub fn inflate(&mut self, num_pages: usize) -> Result<(), &'static str> {
        crate::serial_println!(
            "balloon: inflating {} pages (total will be {})",
            num_pages,
            self.inflated_pages.saturating_add(num_pages)
        );
        balloon_inflate(num_pages);
        self.inflated_pages = self.inflated_pages.saturating_add(num_pages);
        Ok(())
    }

    /// Deflate the balloon (reclaim pages from host).
    pub fn deflate(&mut self, num_pages: usize) -> Result<(), &'static str> {
        crate::serial_println!(
            "balloon: deflating {} pages (current inflated: {})",
            num_pages,
            self.inflated_pages
        );
        balloon_deflate(num_pages);
        self.inflated_pages = self.inflated_pages.saturating_sub(num_pages);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Module init
// ---------------------------------------------------------------------------

/// Initialize the balloon driver.
///
/// Detects the hypervisor via CPUID and logs whether ballooning is available.
pub fn init() {
    if detect_hypervisor() {
        crate::serial_println!("balloon: hypervisor detected — balloon driver active");
    } else {
        crate::serial_println!("balloon: no hypervisor detected — balloon driver inactive");
    }
    // BALLOON_SIZE_PAGES is already 0 from static initialisation.
}
