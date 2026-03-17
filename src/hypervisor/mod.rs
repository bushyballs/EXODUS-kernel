/// Type-1 hypervisor for running VMs inside AIOS
///
/// Part of the AIOS.

pub mod vmx;
pub mod svm;
pub mod vmcs;
pub mod ept;
pub mod vmexit;
pub mod vmenter;
pub mod io_emul;
pub mod device_passthrough;
pub mod passthrough;
pub mod virtio_backend;
pub mod virtio_host;
pub mod guest;
pub mod migration;

use crate::{serial_print, serial_println};

/// Detected CPU virtualization backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtBackend {
    IntelVtx,
    AmdV,
    None,
}

/// Detect which hardware virtualization technology is available.
fn detect_backend() -> VirtBackend {
    if vmx::VmxState::is_supported() {
        VirtBackend::IntelVtx
    } else if svm::SvmState::is_supported() {
        VirtBackend::AmdV
    } else {
        VirtBackend::None
    }
}

pub fn init() {
    let backend = detect_backend();
    serial_println!("    [hypervisor] Detected virtualization backend: {:?}", backend);

    match backend {
        VirtBackend::IntelVtx => {
            vmx::init();
            vmcs::init();
        }
        VirtBackend::AmdV => {
            svm::init();
        }
        VirtBackend::None => {
            serial_println!("    [hypervisor] No hardware virtualization support detected");
        }
    }

    ept::init();
    vmexit::init();
    vmenter::init();
    io_emul::init();
    device_passthrough::init();
    passthrough::init();
    virtio_backend::init();
    virtio_host::init();
    guest::init();

    serial_println!("    [hypervisor] Hypervisor subsystem initialized");
}
