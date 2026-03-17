pub mod ai_enterprise;
pub mod device_policy;
/// Enterprise features for Genesis
///
/// Mobile device management (MDM), work profiles,
/// managed VPN, device policy, and remote wipe.
///
/// Inspired by: Android Enterprise, iOS MDM. All code is original.
pub mod mdm;
pub mod remote_admin;
pub mod work_profile;

use crate::{serial_print, serial_println};

pub fn init() {
    mdm::init();
    work_profile::init();
    device_policy::init();
    remote_admin::init();
    ai_enterprise::init();
    serial_println!("  Enterprise initialized (AI compliance, data classification)");
}
