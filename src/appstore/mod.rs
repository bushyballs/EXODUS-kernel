pub mod ai_store;
pub mod install_manager;
/// App store for Genesis
///
/// Package repository, app signing verification,
/// auto-updates, reviews/ratings, categories,
/// featured apps, install management.
///
/// Original implementation for Hoags OS.
pub mod repository;
pub mod signing;
pub mod store_client;

use crate::{serial_print, serial_println};

pub fn init() {
    repository::init();
    signing::init();
    store_client::init();
    ai_store::init();
    install_manager::init();
    serial_println!("  App store initialized (repo, signing, client, AI recommendations)");
}
