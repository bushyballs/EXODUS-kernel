/// Cloud services framework for Genesis
///
/// Cloud backup, sync, remote storage,
/// cloud compute offload, multi-cloud support.
///
/// Original implementation for Hoags OS.
pub mod backup;
pub mod remote_storage;
pub mod sync_service;
pub mod upload_queue;

use crate::{serial_print, serial_println};

pub fn init() {
    backup::init();
    sync_service::init();
    remote_storage::init();
    upload_queue::init();
    serial_println!("  Cloud services initialized (backup, sync, remote storage)");
}
