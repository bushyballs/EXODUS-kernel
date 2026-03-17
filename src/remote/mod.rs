/// Remote Access Subsystem for Genesis
///
/// Provides secure remote desktop, shell, and file access:
///   - rdp:           Remote Desktop Protocol server (session management, display encoding, input)
///   - vnc:           VNC/RFB protocol server (framebuffer encoding, authentication)
///   - ssh:           SSH-2 server (key exchange, encrypted shell, SFTP, port forwarding)
///   - screen_share:  Screen sharing (capture, compression, streaming, multi-viewer)
///   - remote_shell:  Remote shell (PTY allocation, session management, secure tunnel)
///
/// All protocols use Genesis crypto primitives (ChaCha20-Poly1305, X25519, SHA-256).
/// No external crates. All code is original.

pub mod rdp;
pub mod vnc;
pub mod ssh;
pub mod screen_share;
pub mod remote_shell;

use crate::{serial_print, serial_println};

/// Initialize all remote access subsystems
pub fn init() {
    rdp::init();
    vnc::init();
    ssh::init();
    screen_share::init();
    remote_shell::init();
    serial_println!("  Remote access subsystem initialized (RDP, VNC, SSH, screen share, shell)");
}
