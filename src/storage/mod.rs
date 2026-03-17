pub mod ai_storage;
pub mod analytics;
pub mod cache_tier;
pub mod compress;
pub mod content_provider;
pub mod dedup;
pub mod dm_crypt;
pub mod iscsi;
pub mod loop_dev;
pub mod lvm;
pub mod md;
pub mod nbd;
pub mod partition;
pub mod raid;
/// Storage framework for Genesis
///
/// Scoped storage, content providers, document access,
/// volume management, and storage analytics.
///
/// Inspired by: Android Storage Access Framework, iOS Files. All code is original.
pub mod scoped;
pub mod smart;
pub mod snapshot;
pub mod trim;
pub mod volumes;
pub mod zfs;

use crate::{serial_print, serial_println};

pub fn init() {
    scoped::init();
    content_provider::init();
    volumes::init();
    analytics::init();
    ai_storage::init();
    dm_crypt::init();
    md::init();
    serial_println!("  Storage framework initialized (AI cleanup, cache optimization, dm-crypt AES-XTS, md RAID)");
}
