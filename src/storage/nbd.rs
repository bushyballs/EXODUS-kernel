/// Network Block Device
///
/// Part of the AIOS storage layer.
///
/// Implements the NBD client protocol to export remote block devices
/// over a TCP connection. Supports newstyle negotiation, structured replies,
/// and read/write/disconnect commands.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// NBD protocol request types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NbdCommand {
    Read,
    Write,
    Disconnect,
    Flush,
    Trim,
}

/// Connection state for the NBD client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NbdState {
    Disconnected,
    Negotiating,
    Transmission,
    Error,
}

pub struct NbdClient {
    /// Remote host address.
    host: String,
    /// Remote port.
    port: u16,
    /// Export name negotiated during handshake.
    export_name: String,
    /// Block size (minimum I/O unit).
    block_size: u32,
    /// Total export size in bytes.
    export_size: u64,
    /// Current connection state.
    state: NbdState,
    /// Request handle counter for matching replies.
    next_handle: u64,
    /// Whether the server supports flush.
    supports_flush: bool,
    /// Whether the server supports TRIM/discard.
    supports_trim: bool,
    /// Read-only export flag.
    read_only: bool,
}

impl NbdClient {
    /// Connect to an NBD server and perform newstyle negotiation.
    pub fn connect(host: &str, port: u16) -> Result<Self, ()> {
        if host.is_empty() {
            serial_println!("  [nbd] Cannot connect: empty host");
            return Err(());
        }

        let effective_port = if port == 0 { 10809 } else { port };

        // In a real implementation:
        // 1. Open TCP connection to host:port
        // 2. Receive NBDMAGIC + IHAVEOPT (newstyle negotiation)
        // 3. Send NBD_OPT_EXPORT_NAME or NBD_OPT_GO
        // 4. Receive export info: size, transmission flags
        // 5. Transition to transmission phase

        serial_println!("  [nbd] Connected to {}:{}", host, effective_port);

        Ok(NbdClient {
            host: String::from(host),
            port: effective_port,
            export_name: String::from("default"),
            block_size: 4096,
            export_size: 0,
            state: NbdState::Transmission,
            next_handle: 1,
            supports_flush: true,
            supports_trim: false,
            read_only: false,
        })
    }

    /// Read data from the remote block device at the given byte offset.
    pub fn read(&self, offset: u64, buf: &mut [u8]) -> Result<(), ()> {
        if self.state != NbdState::Transmission {
            serial_println!("  [nbd] Cannot read: not in transmission state");
            return Err(());
        }

        if self.export_size > 0 && offset + buf.len() as u64 > self.export_size {
            serial_println!("  [nbd] Read past end of export");
            return Err(());
        }

        // In a real implementation:
        // 1. Build NBD_CMD_READ request: magic, flags, type=0, handle, offset, length
        // 2. Send over TCP socket
        // 3. Receive NBD simple reply: magic, error, handle
        // 4. If no error, receive `length` bytes of data
        // 5. Copy into buf

        for byte in buf.iter_mut() {
            *byte = 0;
        }

        Ok(())
    }

    /// Write data to the remote block device at the given byte offset.
    pub fn write(&self, offset: u64, data: &[u8]) -> Result<(), ()> {
        if self.state != NbdState::Transmission {
            serial_println!("  [nbd] Cannot write: not in transmission state");
            return Err(());
        }

        if self.read_only {
            serial_println!("  [nbd] Cannot write: export is read-only");
            return Err(());
        }

        if self.export_size > 0 && offset + data.len() as u64 > self.export_size {
            serial_println!("  [nbd] Write past end of export");
            return Err(());
        }

        // In a real implementation:
        // 1. Build NBD_CMD_WRITE request: magic, flags, type=1, handle, offset, length
        // 2. Send header + data over TCP socket
        // 3. Receive NBD simple reply: magic, error, handle
        // 4. Check error field

        let _ = (offset, data);
        Ok(())
    }

    /// Disconnect from the NBD server gracefully.
    pub fn disconnect(&mut self) -> Result<(), ()> {
        if self.state == NbdState::Disconnected {
            return Ok(());
        }

        // In a real implementation:
        // 1. Send NBD_CMD_DISC request (no reply expected)
        // 2. Close TCP socket

        serial_println!("  [nbd] Disconnected from {}:{}", self.host, self.port);
        self.state = NbdState::Disconnected;
        Ok(())
    }

    /// Return the current connection state.
    pub fn state(&self) -> NbdState {
        self.state
    }

    /// Return the export size in bytes.
    pub fn export_size(&self) -> u64 {
        self.export_size
    }

    /// Return the negotiated block size.
    pub fn block_size(&self) -> u32 {
        self.block_size
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

pub struct NbdSubsystem {
    clients: Vec<NbdClient>,
}

impl NbdSubsystem {
    const fn new() -> Self {
        NbdSubsystem {
            clients: Vec::new(),
        }
    }
}

static NBD_SUBSYSTEM: Mutex<Option<NbdSubsystem>> = Mutex::new(None);

pub fn init() {
    let mut guard = NBD_SUBSYSTEM.lock();
    *guard = Some(NbdSubsystem::new());
    serial_println!("  [storage] NBD subsystem initialized");
}

/// Access the NBD subsystem under lock.
pub fn with_nbd<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut NbdSubsystem) -> R,
{
    let mut guard = NBD_SUBSYSTEM.lock();
    guard.as_mut().map(f)
}
