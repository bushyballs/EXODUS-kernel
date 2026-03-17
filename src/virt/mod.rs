pub mod container_net;
/// Virtualization for Genesis
///
/// Subsystems:
///   1. Containers: process-level isolation with namespaces/cgroups
///   2. VM Support: VMX/VT-x hardware virtualization (high-level manager)
///   3. Isolation: sandboxing, syscall filters, resource limits
///   4. Multi-user: user profiles, permissions, session isolation
///   5. Docker: Docker-compatible container runtime
///   6. OCI: OCI runtime spec bundles, hooks, namespaces
///   7. OverlayFS: union mount with copy-up semantics
///   8. Container networking: bridges, veth, NAT, CNI
///   9. VMX: Intel VT-x low-level driver (VMXON/VMXOFF/VMREAD/VMWRITE/VMLAUNCH)
///  10. vCPU: Virtual CPU management (VMCS setup, guest state, exit dispatch)
///  11. EPT: Extended Page Tables (guest memory isolation, GPA->HPA mapping)
pub mod containers;
pub mod docker;
pub mod ept;
pub mod isolation;
pub mod multi_user;
pub mod oci;
pub mod overlay_fs;
pub mod vcpu;
pub mod vm_support;
pub mod vmx;

use crate::{serial_print, serial_println};

pub fn init() {
    serial_println!("[VIRT] Initializing virtualization subsystems...");

    // Low-level VMX driver: detect VT-x, execute VMXON.
    vmx::init();

    // EPT subsystem: check EPT support.
    ept::init();

    // High-level VM manager (VMCS abstraction, virtual devices, EPT manager).
    containers::init();
    vm_support::init();
    isolation::init();
    multi_user::init();
    docker::init();
    oci::init();
    overlay_fs::init();
    container_net::init();

    // Log overall VMX availability so other subsystems can gate on it.
    if vmx::vmx_supported() {
        serial_println!("[VIRT] Hardware virtualization (VMX/VT-x) AVAILABLE");
    } else {
        serial_println!(
            "[VIRT] Hardware virtualization (VMX/VT-x) NOT available -- software emulation only"
        );
    }

    serial_println!("[VIRT] Virtualization subsystems ready");
}
