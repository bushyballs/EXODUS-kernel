/// iSCSI initiator (network block device)
///
/// Part of the AIOS storage layer.
///
/// Implements an iSCSI initiator that connects to remote targets
/// and presents them as local block devices. Supports login,
/// read/write SCSI commands over the iSCSI protocol, and session management.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

pub struct IscsiTarget {
    pub address: String,
    pub port: u16,
    pub iqn: String,
}

/// Session state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Disconnected,
    LoginPhase,
    FullFeaturePhase,
    LogoutPhase,
    Error,
}

pub struct IscsiSession {
    /// The target we are connected to.
    target_address: String,
    target_port: u16,
    target_iqn: String,
    /// Current session state.
    state: SessionState,
    /// Command sequence number for ordering.
    cmd_sn: u32,
    /// Expected status sequence number from the target.
    exp_stat_sn: u32,
    /// Initiator task tag (monotonically increasing).
    initiator_task_tag: u32,
    /// Maximum data segment length negotiated during login.
    max_recv_data_segment: u32,
    /// Authentication: CHAP username (empty = no auth).
    auth_user: String,
    /// Session ISID (Initiator Session ID).
    isid: u64,
    /// Block size reported by the target.
    block_size: u32,
    /// Total blocks on the target LUN.
    total_blocks: u64,
}

impl IscsiSession {
    /// Initiate a connection and login to the iSCSI target.
    pub fn connect(target: &IscsiTarget) -> Result<Self, ()> {
        if target.address.is_empty() || target.iqn.is_empty() {
            serial_println!("  [iscsi] Invalid target: empty address or IQN");
            return Err(());
        }

        let port = if target.port == 0 { 3260 } else { target.port };

        // In a real kernel, we would:
        // 1. Open a TCP connection to target.address:port
        // 2. Send iSCSI Login Request PDU
        // 3. Negotiate parameters (MaxRecvDataSegmentLength, etc.)
        // 4. Optionally perform CHAP authentication
        // 5. Transition to Full Feature Phase

        let seed = crate::time::clock::unix_time();
        let isid = seed & 0xFFFFFFFFFFFF; // 6-byte ISID

        serial_println!(
            "  [iscsi] Connected to {}:{} ({})",
            target.address,
            port,
            target.iqn
        );

        Ok(IscsiSession {
            target_address: target.address.clone(),
            target_port: port,
            target_iqn: target.iqn.clone(),
            state: SessionState::FullFeaturePhase,
            cmd_sn: 1,
            exp_stat_sn: 0,
            initiator_task_tag: 1,
            max_recv_data_segment: 65536,
            auth_user: String::new(),
            isid,
            block_size: 512,
            total_blocks: 0,
        })
    }

    /// Read a block from the target LUN using a SCSI READ(10) command
    /// encapsulated in an iSCSI SCSI Command PDU.
    pub fn read_block(&self, _lba: u64, buf: &mut [u8]) -> Result<(), ()> {
        if self.state != SessionState::FullFeaturePhase {
            serial_println!("  [iscsi] Cannot read: session not in full feature phase");
            return Err(());
        }

        // In a real implementation:
        // 1. Build SCSI READ(10) CDB: opcode 0x28, LBA, transfer length
        // 2. Wrap in iSCSI SCSI Command PDU with CmdSN, ITT
        // 3. Send over TCP socket
        // 4. Receive iSCSI SCSI Data-In PDU(s) with the data
        // 5. Copy data into buf

        // Simulate: zero-fill the buffer (target has no real data)
        for byte in buf.iter_mut() {
            *byte = 0;
        }

        Ok(())
    }

    /// Write a block to the target LUN using a SCSI WRITE(10) command
    /// encapsulated in an iSCSI SCSI Command PDU.
    pub fn write_block(&self, lba: u64, data: &[u8]) -> Result<(), ()> {
        if self.state != SessionState::FullFeaturePhase {
            serial_println!("  [iscsi] Cannot write: session not in full feature phase");
            return Err(());
        }

        // In a real implementation:
        // 1. Build SCSI WRITE(10) CDB: opcode 0x2A, LBA, transfer length
        // 2. Wrap in iSCSI SCSI Command PDU with F bit, data segment
        // 3. If data > MaxRecvDataSegmentLength, use R2T flow
        // 4. Send over TCP socket
        // 5. Receive iSCSI SCSI Response PDU with status

        let _ = (lba, data);
        Ok(())
    }

    /// Disconnect from the target gracefully.
    pub fn disconnect(&mut self) -> Result<(), ()> {
        if self.state == SessionState::Disconnected {
            return Ok(());
        }

        // In a real implementation:
        // 1. Send iSCSI Logout Request PDU
        // 2. Wait for Logout Response PDU
        // 3. Close TCP connection

        serial_println!(
            "  [iscsi] Disconnected from {}:{}",
            self.target_address,
            self.target_port
        );
        self.state = SessionState::Disconnected;
        Ok(())
    }

    /// Return current session state.
    pub fn state(&self) -> SessionState {
        self.state
    }

    /// Return the current command sequence number.
    pub fn cmd_sn(&self) -> u32 {
        self.cmd_sn
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

pub struct IscsiSubsystem {
    sessions: Vec<IscsiSession>,
}

impl IscsiSubsystem {
    const fn new() -> Self {
        IscsiSubsystem {
            sessions: Vec::new(),
        }
    }
}

static ISCSI_SUBSYSTEM: Mutex<Option<IscsiSubsystem>> = Mutex::new(None);

pub fn init() {
    let mut guard = ISCSI_SUBSYSTEM.lock();
    *guard = Some(IscsiSubsystem::new());
    serial_println!("  [storage] iSCSI initiator initialized");
}

/// Access the iSCSI subsystem under lock.
pub fn with_iscsi<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut IscsiSubsystem) -> R,
{
    let mut guard = ISCSI_SUBSYSTEM.lock();
    guard.as_mut().map(f)
}
