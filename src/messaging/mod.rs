/// Messaging / chat subsystem for Genesis OS
///
/// Provides end-to-end encrypted direct messaging, group conversations,
/// media attachment sharing, and multi-device message synchronisation.
/// All cryptographic operations are stub simulations suitable for a
/// bare-metal kernel with no access to external crates.
pub mod e2e_chat;
pub mod group_chat;
pub mod inbox;
pub mod media_share;
pub mod msg_sync;

pub fn init() {
    e2e_chat::init();
    group_chat::init();
    media_share::init();
    msg_sync::init();
    inbox::init();
}
