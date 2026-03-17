//! NVMe (Non-Volatile Memory Express) Driver
//!
//! High-performance PCIe SSD driver for Genesis OS.
//! Supports NVMe 1.0+ specification with async I/O, multiple queues, and command submission.

pub mod registers;
pub mod commands;
pub mod queue;
pub mod controller;

pub use controller::NvmeController;

use crate::pci::PciDevice;

/// Maximum number of NVMe controllers supported
const MAX_NVME_CONTROLLERS: usize = 4;

/// Global NVMe controller array
static mut NVME_CONTROLLERS: [Option<NvmeController>; MAX_NVME_CONTROLLERS] = [const { None }; MAX_NVME_CONTROLLERS];
static mut NVME_COUNT: usize = 0;

/// Initialize all NVMe controllers found on the PCI bus
pub fn init() -> usize {
    use crate::pci;

    unsafe {
        NVME_COUNT = 0;
    }

    // Find all NVMe controllers
    let nvme_devices = pci::find_nvme_controllers();

    for device_opt in nvme_devices.iter() {
        if let Some(device) = device_opt {
            if unsafe { NVME_COUNT } >= MAX_NVME_CONTROLLERS {
                break;
            }

            // Initialize controller
            match NvmeController::new(*device) {
                Ok(controller) => {
                    unsafe {
                        NVME_CONTROLLERS[NVME_COUNT] = Some(controller);
                        NVME_COUNT = NVME_COUNT.saturating_add(1);
                    }
                }
                Err(e) => {
                    // Log error (would use proper logging in production)
                    continue;
                }
            }
        }
    }

    unsafe { NVME_COUNT }
}

/// Get NVMe controller by index
pub fn get_controller(index: usize) -> Option<&'static mut NvmeController> {
    unsafe {
        if index < NVME_COUNT {
            NVME_CONTROLLERS[index].as_mut()
        } else {
            None
        }
    }
}

/// Get total number of initialized NVMe controllers
pub fn controller_count() -> usize {
    unsafe { NVME_COUNT }
}

/// NVMe Error Types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NvmeError {
    NotFound,
    InvalidBar,
    InitializationFailed,
    Timeout,
    QueueFull,
    CommandFailed,
    InvalidNamespace,
    IoError,
}

pub type Result<T> = core::result::Result<T, NvmeError>;
