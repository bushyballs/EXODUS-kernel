pub mod compose;
/// Email client subsystem for Genesis
///
/// Provides a complete email client stack:
///   - IMAP client for receiving and managing email
///   - SMTP client for sending email
///   - Inbox manager for local message storage and organization
///   - Compose module for drafting, replying, forwarding
///
/// All protocol implementations are bare-metal, no external crates.
/// Uses kernel TCP/TLS for network transport.
pub mod imap_client;
pub mod inbox;
pub mod smtp_client;

use crate::{serial_print, serial_println};

/// Initialize the entire email subsystem
pub fn init() {
    serial_println!("[email] Initializing email subsystem");

    imap_client::init();
    serial_println!("[email]   IMAP client ready");

    smtp_client::init();
    serial_println!("[email]   SMTP client ready");

    inbox::init();
    serial_println!("[email]   Inbox manager ready");

    compose::init();
    serial_println!("[email]   Compose engine ready");

    serial_println!("[email] Email subsystem initialized");
}
