//! NVMe Command Structures
//!
//! Defines submission and completion queue entry formats for NVMe commands.

use core::mem;

/// NVMe Command Opcode (Admin)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AdminOpcode {
    DeleteIoSubmissionQueue = 0x00,
    CreateIoSubmissionQueue = 0x01,
    GetLogPage = 0x02,
    DeleteIoCompletionQueue = 0x04,
    CreateIoCompletionQueue = 0x05,
    Identify = 0x06,
    Abort = 0x08,
    SetFeatures = 0x09,
    GetFeatures = 0x0A,
    AsyncEventRequest = 0x0C,
    NamespaceManagement = 0x0D,
    FirmwareCommit = 0x10,
    FirmwareImageDownload = 0x11,
    NamespaceAttachment = 0x15,
}

/// NVMe Command Opcode (I/O)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IoOpcode {
    Flush = 0x00,
    Write = 0x01,
    Read = 0x02,
    WriteUncorrectable = 0x04,
    Compare = 0x05,
    WriteZeroes = 0x08,
    DatasetManagement = 0x09,
    Verify = 0x0C,
    ReservationRegister = 0x0D,
    ReservationReport = 0x0E,
    ReservationAcquire = 0x11,
    ReservationRelease = 0x15,
}

/// NVMe Submission Queue Entry (64 bytes)
#[repr(C, align(64))]
#[derive(Clone, Copy)]
pub struct SubmissionQueueEntry {
    pub cdw0: u32,   // Command Dword 0 (Opcode + Flags + Command ID)
    pub nsid: u32,   // Namespace ID
    pub cdw2: u32,   // Reserved
    pub cdw3: u32,   // Reserved
    pub mptr: u64,   // Metadata Pointer
    pub dptr: [u64; 2], // Data Pointer (PRP or SGL)
    pub cdw10: u32,  // Command-specific
    pub cdw11: u32,  // Command-specific
    pub cdw12: u32,  // Command-specific
    pub cdw13: u32,  // Command-specific
    pub cdw14: u32,  // Command-specific
    pub cdw15: u32,  // Command-specific
}

impl SubmissionQueueEntry {
    pub fn new() -> Self {
        unsafe { mem::zeroed() }
    }

    /// Set opcode and command ID
    pub fn set_opcode(&mut self, opcode: u8, command_id: u16) {
        self.cdw0 = (opcode as u32) | ((command_id as u32) << 16);
    }

    /// Set namespace ID
    pub fn set_namespace(&mut self, nsid: u32) {
        self.nsid = nsid;
    }

    /// Set PRP (Physical Region Page) entries for data transfer
    pub fn set_prp(&mut self, prp1: u64, prp2: u64) {
        self.dptr[0] = prp1;
        self.dptr[1] = prp2;
    }

    /// Create Identify command
    pub fn identify(command_id: u16, cns: u8, buffer_addr: u64) -> Self {
        let mut cmd = Self::new();
        cmd.set_opcode(AdminOpcode::Identify as u8, command_id);
        cmd.cdw10 = cns as u32; // Controller/Namespace/List
        cmd.set_prp(buffer_addr, 0);
        cmd
    }

    /// Create I/O Completion Queue creation command
    pub fn create_io_cq(
        command_id: u16,
        queue_id: u16,
        queue_size: u16,
        buffer_addr: u64,
        interrupt_vector: u16,
    ) -> Self {
        let mut cmd = Self::new();
        cmd.set_opcode(AdminOpcode::CreateIoCompletionQueue as u8, command_id);
        cmd.set_prp(buffer_addr, 0);
        cmd.cdw10 = ((queue_size as u32) << 16) | (queue_id as u32);
        cmd.cdw11 = (1 << 0) | ((interrupt_vector as u32) << 16); // Physically contiguous + IV
        cmd
    }

    /// Create I/O Submission Queue creation command
    pub fn create_io_sq(
        command_id: u16,
        queue_id: u16,
        queue_size: u16,
        buffer_addr: u64,
        cq_id: u16,
    ) -> Self {
        let mut cmd = Self::new();
        cmd.set_opcode(AdminOpcode::CreateIoSubmissionQueue as u8, command_id);
        cmd.set_prp(buffer_addr, 0);
        cmd.cdw10 = ((queue_size as u32) << 16) | (queue_id as u32);
        cmd.cdw11 = (1 << 0) | ((cq_id as u32) << 16); // Physically contiguous + CQID
        cmd
    }

    /// Create Read command
    pub fn read(
        command_id: u16,
        namespace_id: u32,
        start_lba: u64,
        block_count: u16,
        buffer_addr: u64,
    ) -> Self {
        let mut cmd = Self::new();
        cmd.set_opcode(IoOpcode::Read as u8, command_id);
        cmd.set_namespace(namespace_id);
        cmd.set_prp(buffer_addr, 0);
        cmd.cdw10 = (start_lba & 0xFFFFFFFF) as u32;
        cmd.cdw11 = (start_lba >> 32) as u32;
        cmd.cdw12 = block_count as u32; // Number of logical blocks (0-based)
        cmd
    }

    /// Create Write command
    pub fn write(
        command_id: u16,
        namespace_id: u32,
        start_lba: u64,
        block_count: u16,
        buffer_addr: u64,
    ) -> Self {
        let mut cmd = Self::new();
        cmd.set_opcode(IoOpcode::Write as u8, command_id);
        cmd.set_namespace(namespace_id);
        cmd.set_prp(buffer_addr, 0);
        cmd.cdw10 = (start_lba & 0xFFFFFFFF) as u32;
        cmd.cdw11 = (start_lba >> 32) as u32;
        cmd.cdw12 = block_count as u32;
        cmd
    }
}

/// NVMe Completion Queue Entry (16 bytes)
#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub struct CompletionQueueEntry {
    pub dw0: u32,    // Command-specific result
    pub dw1: u32,    // Reserved
    pub sqhd: u16,   // Submission Queue Head Pointer
    pub sqid: u16,   // Submission Queue Identifier
    pub cid: u16,    // Command Identifier
    pub status: u16, // Status Field (Phase + Status Code)
}

impl CompletionQueueEntry {
    pub fn new() -> Self {
        unsafe { mem::zeroed() }
    }

    /// Get command ID
    pub fn command_id(&self) -> u16 {
        self.cid
    }

    /// Get phase bit
    pub fn phase(&self) -> bool {
        (self.status & (1 << 0)) != 0
    }

    /// Get status code type
    pub fn status_code_type(&self) -> u8 {
        ((self.status >> 9) & 0x7) as u8
    }

    /// Get status code
    pub fn status_code(&self) -> u8 {
        ((self.status >> 1) & 0xFF) as u8
    }

    /// Check if command succeeded
    pub fn success(&self) -> bool {
        self.status_code_type() == 0 && self.status_code() == 0
    }

    /// Get command-specific result
    pub fn result(&self) -> u32 {
        self.dw0
    }
}

/// Identify Controller Data Structure (4096 bytes)
#[repr(C, align(4096))]
#[derive(Clone, Copy)]
pub struct IdentifyController {
    pub vid: u16,          // PCI Vendor ID
    pub ssvid: u16,        // PCI Subsystem Vendor ID
    pub sn: [u8; 20],      // Serial Number
    pub mn: [u8; 40],      // Model Number
    pub fr: [u8; 8],       // Firmware Revision
    pub rab: u8,           // Recommended Arbitration Burst
    pub ieee: [u8; 3],     // IEEE OUI Identifier
    pub cmic: u8,          // Controller Multi-Path I/O and Namespace Sharing Capabilities
    pub mdts: u8,          // Maximum Data Transfer Size
    pub cntlid: u16,       // Controller ID
    pub ver: u32,          // Version
    pub rtd3r: u32,        // RTD3 Resume Latency
    pub rtd3e: u32,        // RTD3 Entry Latency
    pub oaes: u32,         // Optional Async Events Supported
    pub ctratt: u32,       // Controller Attributes
    pub rrls: u16,         // Read Recovery Levels Supported
    pub _reserved1: [u8; 9],
    pub cntrltype: u8,     // Controller Type
    pub fguid: [u8; 16],   // FRU Globally Unique Identifier
    pub crdt1: u16,        // Command Retry Delay Time 1
    pub crdt2: u16,        // Command Retry Delay Time 2
    pub crdt3: u16,        // Command Retry Delay Time 3
    pub _reserved2: [u8; 106],
    pub _reserved3: [u8; 13],
    pub nvmsr: u8,         // NVM Subsystem Report
    pub vwci: u8,          // VPD Write Cycle Information
    pub mec: u8,           // Management Endpoint Capabilities
    pub oacs: u16,         // Optional Admin Command Support
    pub acl: u8,           // Abort Command Limit
    pub aerl: u8,          // Async Event Request Limit
    pub frmw: u8,          // Firmware Updates
    pub lpa: u8,           // Log Page Attributes
    pub elpe: u8,          // Error Log Page Entries
    pub npss: u8,          // Number of Power States Support
    pub avscc: u8,         // Admin Vendor Specific Command Configuration
    pub apsta: u8,         // Autonomous Power State Transition Attributes
    pub wctemp: u16,       // Warning Composite Temperature Threshold
    pub cctemp: u16,       // Critical Composite Temperature Threshold
    pub mtfa: u16,         // Maximum Time for Firmware Activation
    pub hmpre: u32,        // Host Memory Buffer Preferred Size
    pub hmmin: u32,        // Host Memory Buffer Minimum Size
    pub tnvmcap: [u8; 16], // Total NVM Capacity
    pub unvmcap: [u8; 16], // Unallocated NVM Capacity
    pub rpmbs: u32,        // Replay Protected Memory Block Support
    pub edstt: u16,        // Extended Device Self-test Time
    pub dsto: u8,          // Device Self-test Options
    pub fwug: u8,          // Firmware Update Granularity
    pub kas: u16,          // Keep Alive Support
    pub hctma: u16,        // Host Controlled Thermal Management Attributes
    pub mntmt: u16,        // Minimum Thermal Management Temperature
    pub mxtmt: u16,        // Maximum Thermal Management Temperature
    pub sanicap: u32,      // Sanitize Capabilities
    pub hmminds: u32,      // Host Memory Buffer Minimum Descriptor Entry Size
    pub hmmaxd: u16,       // Host Memory Maximum Descriptors Entries
    pub nsetidmax: u16,    // NVM Set Identifier Maximum
    pub endgidmax: u16,    // Endurance Group Identifier Maximum
    pub anatt: u8,         // ANA Transition Time
    pub anacap: u8,        // Asymmetric Namespace Access Capabilities
    pub anagrpmax: u32,    // ANA Group Identifier Maximum
    pub nanagrpid: u32,    // Number of ANA Group Identifiers
    pub pels: u32,         // Persistent Event Log Size
    pub _reserved4: [u8; 156],
    pub sqes: u8,          // Submission Queue Entry Size
    pub cqes: u8,          // Completion Queue Entry Size
    pub maxcmd: u16,       // Maximum Outstanding Commands
    pub nn: u32,           // Number of Namespaces
    pub oncs: u16,         // Optional NVM Command Support
    pub fuses: u16,        // Fused Operation Support
    pub fna: u8,           // Format NVM Attributes
    pub vwc: u8,           // Volatile Write Cache
    pub awun: u16,         // Atomic Write Unit Normal
    pub awupf: u16,        // Atomic Write Unit Power Fail
    pub nvscc: u8,         // NVM Vendor Specific Command Configuration
    pub nwpc: u8,          // Namespace Write Protection Capabilities
    pub acwu: u16,         // Atomic Compare & Write Unit
    pub _reserved5: [u8; 2],
    pub sgls: u32,         // SGL Support
    pub mnan: u32,         // Maximum Number of Allowed Namespaces
    pub _reserved6: [u8; 224],
    pub subnqn: [u8; 256], // NVM Subsystem Qualified Name
    pub _reserved7: [u8; 768],
    pub _reserved8: [u8; 256],
    pub _padding: [u8; 1024],
}

impl IdentifyController {
    pub fn new() -> Self {
        unsafe { mem::zeroed() }
    }

    /// Get serial number as string
    pub fn serial_number(&self) -> &str {
        core::str::from_utf8(&self.sn).unwrap_or("UNKNOWN").trim()
    }

    /// Get model number as string
    pub fn model_number(&self) -> &str {
        core::str::from_utf8(&self.mn).unwrap_or("UNKNOWN").trim()
    }

    /// Get firmware revision as string
    pub fn firmware_revision(&self) -> &str {
        core::str::from_utf8(&self.fr).unwrap_or("UNKNOWN").trim()
    }
}

/// Identify Namespace Data Structure (4096 bytes)
#[repr(C, align(4096))]
#[derive(Clone, Copy)]
pub struct IdentifyNamespace {
    pub nsze: u64,         // Namespace Size
    pub ncap: u64,         // Namespace Capacity
    pub nuse: u64,         // Namespace Utilization
    pub nsfeat: u8,        // Namespace Features
    pub nlbaf: u8,         // Number of LBA Formats
    pub flbas: u8,         // Formatted LBA Size
    pub mc: u8,            // Metadata Capabilities
    pub dpc: u8,           // End-to-end Data Protection Capabilities
    pub dps: u8,           // End-to-end Data Protection Type Settings
    pub nmic: u8,          // Namespace Multi-path I/O and Namespace Sharing Capabilities
    pub rescap: u8,        // Reservation Capabilities
    pub fpi: u8,           // Format Progress Indicator
    pub dlfeat: u8,        // Deallocate Logical Block Features
    pub nawun: u16,        // Namespace Atomic Write Unit Normal
    pub nawupf: u16,       // Namespace Atomic Write Unit Power Fail
    pub nacwu: u16,        // Namespace Atomic Compare & Write Unit
    pub nabsn: u16,        // Namespace Atomic Boundary Size Normal
    pub nabo: u16,         // Namespace Atomic Boundary Offset
    pub nabspf: u16,       // Namespace Atomic Boundary Size Power Fail
    pub noiob: u16,        // Namespace Optimal IO Boundary
    pub nvmcap: [u8; 16],  // NVM Capacity
    pub npwg: u16,         // Namespace Preferred Write Granularity
    pub npwa: u16,         // Namespace Preferred Write Alignment
    pub npdg: u16,         // Namespace Preferred Deallocate Granularity
    pub npda: u16,         // Namespace Preferred Deallocate Alignment
    pub nows: u16,         // Namespace Optimal Write Size
    pub _reserved1: [u8; 18],
    pub anagrpid: u32,     // ANA Group Identifier
    pub _reserved2: [u8; 3],
    pub nsattr: u8,        // Namespace Attributes
    pub nvmsetid: u16,     // NVM Set Identifier
    pub endgid: u16,       // Endurance Group Identifier
    pub nguid: [u8; 16],   // Namespace Globally Unique Identifier
    pub eui64: [u8; 8],    // IEEE Extended Unique Identifier
    pub lbaf: [LbaFormat; 16], // LBA Format Support
    pub _reserved3: [u8; 192],
    pub vs: [u8; 3712],    // Vendor Specific
}

/// LBA Format Data Structure
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LbaFormat {
    pub ms: u16,    // Metadata Size
    pub lbads: u8,  // LBA Data Size (power of 2)
    pub rp: u8,     // Relative Performance
}

impl IdentifyNamespace {
    pub fn new() -> Self {
        unsafe { mem::zeroed() }
    }

    /// Get block size in bytes
    pub fn block_size(&self) -> usize {
        let format_index = (self.flbas & 0xF) as usize;
        if format_index < 16 {
            1 << self.lbaf[format_index].lbads
        } else {
            512 // Default to 512 bytes
        }
    }

    /// Get total capacity in bytes
    pub fn capacity(&self) -> u64 {
        self.ncap * (self.block_size() as u64)
    }
}
