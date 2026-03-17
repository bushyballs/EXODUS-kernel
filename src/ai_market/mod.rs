pub mod community;
pub mod model_download;
/// AI Model Marketplace for Genesis
///
/// A decentralized, on-device model marketplace enabling users to discover,
/// download, version, and share AI models without relying on cloud services.
///
/// Subsystems:
///   1. Model Store — browse, search, rate, and discover AI models
///   2. Model Download — bandwidth-managed download with verification
///   3. Versioning — track model versions, rollback, update policies
///   4. Community — reviews, forks, fine-tune sharing, leaderboards
///
/// All model metadata is stored locally. Downloads are verified via checksums.
/// No data leaves the device unless the user explicitly initiates a transfer.
///
/// Original implementation for Hoags OS.
pub mod model_store;
pub mod versioning;

use crate::{serial_print, serial_println};

pub fn init() {
    model_store::init();
    model_download::init();
    versioning::init();
    community::init();
    serial_println!("  AI Market: store, downloads, versioning, community");
}
