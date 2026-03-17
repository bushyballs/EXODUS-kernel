use crate::sync::Mutex;
/// xHCI (eXtensible Host Controller Interface) driver
///
/// USB 3.x host controller. Communicates via memory-mapped registers
/// and ring buffers (Command Ring, Event Ring, Transfer Rings).
///
/// Features:
///   - PCI discovery (class 0x0C, subclass 0x03, prog-if 0x30)
///   - MMIO register access via BAR0
///   - Controller reset and initialization sequence
///   - Port enumeration and speed detection
///   - Command Ring, Event Ring, Transfer Ring data structures
///   - Slot enable/disable, device address assignment
///   - Device Context Base Address Array (DCBAA)
///   - Scratchpad buffer allocation
///
/// Reference: xHCI specification 1.2
/// No external crates. All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

static XHCI_STATE: Mutex<Option<XhciController>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// xHCI capability register offsets
// ---------------------------------------------------------------------------

const CAPLENGTH: u32 = 0x00;
const HCIVERSION: u32 = 0x02;
const HCSPARAMS1: u32 = 0x04;
const HCSPARAMS2: u32 = 0x08;
const HCSPARAMS3: u32 = 0x0C;
const HCCPARAMS1: u32 = 0x10;
const DBOFF: u32 = 0x14;
const RTSOFF: u32 = 0x18;
const HCCPARAMS2: u32 = 0x1C;

// ---------------------------------------------------------------------------
// xHCI operational register offsets (from cap_length)
// ---------------------------------------------------------------------------

const USBCMD: u32 = 0x00;
const USBSTS: u32 = 0x04;
const PAGESIZE: u32 = 0x08;
const DNCTRL: u32 = 0x14;
const CRCR: u32 = 0x18;
const DCBAAP: u32 = 0x30;
const CONFIG: u32 = 0x38;

// ---------------------------------------------------------------------------
// USBCMD bits
// ---------------------------------------------------------------------------

const USBCMD_RS: u32 = 1 << 0; // Run/Stop
const USBCMD_HCRST: u32 = 1 << 1; // Host Controller Reset
const USBCMD_INTE: u32 = 1 << 2; // Interrupter Enable
const USBCMD_HSEE: u32 = 1 << 3; // Host System Error Enable

// ---------------------------------------------------------------------------
// USBSTS bits
// ---------------------------------------------------------------------------

const USBSTS_HCH: u32 = 1 << 0; // HC Halted
const USBSTS_HSE: u32 = 1 << 2; // Host System Error
const USBSTS_EINT: u32 = 1 << 3; // Event Interrupt
const USBSTS_PCD: u32 = 1 << 4; // Port Change Detect
const USBSTS_CNR: u32 = 1 << 11; // Controller Not Ready

// ---------------------------------------------------------------------------
// Port register offsets (relative to operational base + 0x400)
// ---------------------------------------------------------------------------

const PORTSC_OFFSET: u32 = 0x400;
const PORT_STRIDE: u32 = 0x10;

// PORTSC bits
const PORTSC_CCS: u32 = 1 << 0; // Current Connect Status
const PORTSC_PED: u32 = 1 << 1; // Port Enabled/Disabled
const PORTSC_OCA: u32 = 1 << 3; // Over-current Active
const PORTSC_PR: u32 = 1 << 4; // Port Reset
const PORTSC_PP: u32 = 1 << 9; // Port Power
const PORTSC_CSC: u32 = 1 << 17; // Connect Status Change
const PORTSC_PEC: u32 = 1 << 18; // Port Enabled/Disabled Change
const PORTSC_PRC: u32 = 1 << 21; // Port Reset Change
const PORTSC_WRC: u32 = 1 << 19; // Warm Port Reset Change

// ---------------------------------------------------------------------------
// USB speed
// ---------------------------------------------------------------------------

/// USB speed
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbSpeed {
    Full,      // 12 Mbps (USB 1.1)
    Low,       // 1.5 Mbps (USB 1.0)
    High,      // 480 Mbps (USB 2.0)
    Super,     // 5 Gbps (USB 3.0)
    SuperPlus, // 10 Gbps (USB 3.1)
}

impl UsbSpeed {
    pub fn name(&self) -> &'static str {
        match self {
            UsbSpeed::Full => "Full-Speed (12 Mbps)",
            UsbSpeed::Low => "Low-Speed (1.5 Mbps)",
            UsbSpeed::High => "High-Speed (480 Mbps)",
            UsbSpeed::Super => "SuperSpeed (5 Gbps)",
            UsbSpeed::SuperPlus => "SuperSpeed+ (10 Gbps)",
        }
    }

    pub fn max_packet_size_default(&self) -> u16 {
        match self {
            UsbSpeed::Low => 8,
            UsbSpeed::Full => 64,
            UsbSpeed::High => 64,
            UsbSpeed::Super | UsbSpeed::SuperPlus => 512,
        }
    }
}

// ---------------------------------------------------------------------------
// USB device info
// ---------------------------------------------------------------------------

/// USB device info
#[derive(Debug, Clone)]
pub struct UsbDevice {
    pub slot_id: u8,
    pub port: u8,
    pub speed: UsbSpeed,
    pub vendor_id: u16,
    pub product_id: u16,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub manufacturer: String,
    pub product: String,
    pub max_packet_size: u16,
    pub num_configurations: u8,
}

impl UsbDevice {
    pub fn class_name(&self) -> &'static str {
        match self.class {
            0x00 => "Composite",
            0x01 => "Audio",
            0x02 => "CDC",
            0x03 => "HID",
            0x05 => "Physical",
            0x06 => "Image",
            0x07 => "Printer",
            0x08 => "Mass Storage",
            0x09 => "Hub",
            0x0A => "CDC-Data",
            0x0E => "Video",
            0x0F => "Personal Healthcare",
            0xE0 => "Wireless",
            0xFE => "Application Specific",
            0xFF => "Vendor Specific",
            _ => "Unknown",
        }
    }
}

// ---------------------------------------------------------------------------
// TRB (Transfer Request Block)
// ---------------------------------------------------------------------------

/// TRB (Transfer Request Block) -- the fundamental xHCI command unit
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Trb {
    pub param_lo: u32,
    pub param_hi: u32,
    pub status: u32,
    pub control: u32,
}

impl Trb {
    pub const fn empty() -> Self {
        Trb {
            param_lo: 0,
            param_hi: 0,
            status: 0,
            control: 0,
        }
    }

    pub fn trb_type(&self) -> u8 {
        ((self.control >> 10) & 0x3F) as u8
    }

    pub fn set_type(&mut self, trb_type: u8) {
        self.control = (self.control & !(0x3F << 10)) | ((trb_type as u32 & 0x3F) << 10);
    }

    pub fn set_cycle(&mut self, cycle: bool) {
        if cycle {
            self.control |= 1;
        } else {
            self.control &= !1;
        }
    }

    pub fn cycle_bit(&self) -> bool {
        self.control & 1 != 0
    }

    /// Completion code from event TRBs
    pub fn completion_code(&self) -> u8 {
        ((self.status >> 24) & 0xFF) as u8
    }

    /// Slot ID from event TRBs
    pub fn slot_id(&self) -> u8 {
        ((self.control >> 24) & 0xFF) as u8
    }
}

// ---------------------------------------------------------------------------
// Endpoint Context (simplified, used to track per-slot transfer rings)
// ---------------------------------------------------------------------------

/// xHCI endpoint context index: converts (endpoint_number, direction_in) → DCI.
/// DCI = endpoint_number * 2 + (if direction_in then 1 else 0).
/// EP0 is special (bidirectional, DCI=1).
pub fn ep_dci(ep_num: u8, direction_in: bool) -> usize {
    if ep_num == 0 {
        1 // EP0 control (bidirectional)
    } else {
        let dir_bit: usize = if direction_in { 1 } else { 0 };
        (ep_num as usize) * 2 + dir_bit
    }
}

/// Maximum device context index (EP0..EP15 IN/OUT = DCI 1..31)
pub const MAX_DCI: usize = 32;

// ---------------------------------------------------------------------------
// TRB types
// ---------------------------------------------------------------------------

pub const TRB_NORMAL: u8 = 1;
pub const TRB_SETUP_STAGE: u8 = 2;
pub const TRB_DATA_STAGE: u8 = 3;
pub const TRB_STATUS_STAGE: u8 = 4;
pub const TRB_ISOCH: u8 = 5;
pub const TRB_LINK: u8 = 6;
pub const TRB_EVENT_DATA: u8 = 7;
pub const TRB_NO_OP: u8 = 8;
pub const TRB_ENABLE_SLOT: u8 = 9;
pub const TRB_DISABLE_SLOT: u8 = 10;
pub const TRB_ADDRESS_DEVICE: u8 = 11;
pub const TRB_CONFIGURE_EP: u8 = 12;
pub const TRB_EVALUATE_CTX: u8 = 13;
pub const TRB_RESET_EP: u8 = 14;
pub const TRB_STOP_EP: u8 = 15;
pub const TRB_SET_TR_DEQUEUE: u8 = 16;
pub const TRB_RESET_DEVICE: u8 = 17;
pub const TRB_NO_OP_CMD: u8 = 23;

// Event TRB types
pub const TRB_TRANSFER_EVENT: u8 = 32;
pub const TRB_COMMAND_COMPLETION: u8 = 33;
pub const TRB_PORT_STATUS_CHANGE: u8 = 34;
pub const TRB_HOST_CONTROLLER: u8 = 37;

// Completion codes
pub const CC_SUCCESS: u8 = 1;
pub const CC_SHORT_PACKET: u8 = 13;
pub const CC_STALL: u8 = 6;

// ---------------------------------------------------------------------------
// Ring buffer
// ---------------------------------------------------------------------------

/// Ring buffer size (number of TRBs per ring)
const RING_SIZE: usize = 256;

/// A generic TRB ring (used for Command, Event, and Transfer rings)
pub struct TrbRing {
    pub trbs: Vec<Trb>,
    pub enqueue_idx: usize,
    pub dequeue_idx: usize,
    pub cycle_state: bool,
    pub phys_addr: u64,
}

impl TrbRing {
    pub fn new(size: usize) -> Self {
        let mut trbs = Vec::with_capacity(size + 1);
        for _ in 0..=size {
            trbs.push(Trb::empty());
        }
        let phys_addr = trbs.as_ptr() as u64;

        // Set up link TRB at the end to wrap around
        let last = trbs.len() - 1;
        trbs[last].set_type(TRB_LINK);
        trbs[last].param_lo = phys_addr as u32;
        trbs[last].param_hi = (phys_addr >> 32) as u32;
        // Toggle cycle bit on link TRB
        trbs[last].control |= 1 << 1; // TC (Toggle Cycle) bit

        TrbRing {
            trbs,
            enqueue_idx: 0,
            dequeue_idx: 0,
            cycle_state: true,
            phys_addr,
        }
    }

    /// Enqueue a TRB onto this ring. Returns the physical address of the enqueued TRB.
    pub fn enqueue(&mut self, mut trb: Trb) -> u64 {
        trb.set_cycle(self.cycle_state);
        let idx = self.enqueue_idx;
        self.trbs[idx] = trb;
        let addr = self.phys_addr + (idx * core::mem::size_of::<Trb>()) as u64;

        self.enqueue_idx = self.enqueue_idx.saturating_add(1);
        // Check for link TRB
        if self.enqueue_idx >= self.trbs.len() - 1 {
            // We hit the link TRB -- update its cycle bit and wrap
            self.trbs[self.enqueue_idx].set_cycle(self.cycle_state);
            self.enqueue_idx = 0;
            self.cycle_state = !self.cycle_state;
        }

        addr
    }

    /// Check if there is a pending event (for event ring dequeue)
    pub fn has_pending(&self) -> bool {
        let trb = &self.trbs[self.dequeue_idx];
        trb.cycle_bit() == self.cycle_state
    }

    /// Dequeue a TRB from the event ring
    pub fn dequeue(&mut self) -> Option<Trb> {
        if !self.has_pending() {
            return None;
        }
        let trb = self.trbs[self.dequeue_idx];
        self.dequeue_idx = self.dequeue_idx.saturating_add(1);
        if self.dequeue_idx >= self.trbs.len() - 1 {
            self.dequeue_idx = 0;
            self.cycle_state = !self.cycle_state;
        }
        Some(trb)
    }
}

// ---------------------------------------------------------------------------
// xHCI Controller
// ---------------------------------------------------------------------------

/// Per-slot transfer ring table.
/// Outer index = slot_id (1-based, 0 unused).
/// Inner index = DCI (1..=31).
pub struct SlotTransferRings {
    pub slot_id: u8,
    pub rings: Vec<Option<TrbRing>>, // indexed by DCI (0 = unused)
}

impl SlotTransferRings {
    pub fn new(slot_id: u8) -> Self {
        let mut rings = Vec::with_capacity(MAX_DCI);
        for _ in 0..MAX_DCI {
            rings.push(None);
        }
        SlotTransferRings { slot_id, rings }
    }

    /// Allocate a transfer ring for the given DCI if not already present.
    pub fn ensure_ring(&mut self, dci: usize) -> Option<&mut TrbRing> {
        if dci == 0 || dci >= MAX_DCI {
            return None;
        }
        if self.rings[dci].is_none() {
            self.rings[dci] = Some(TrbRing::new(RING_SIZE));
        }
        self.rings[dci].as_mut()
    }

    /// Get a transfer ring for the given DCI.
    pub fn get_ring(&mut self, dci: usize) -> Option<&mut TrbRing> {
        if dci == 0 || dci >= MAX_DCI {
            return None;
        }
        self.rings[dci].as_mut()
    }

    /// Physical address of the transfer ring dequeue pointer for a DCI.
    pub fn ring_phys_addr(&self, dci: usize) -> Option<u64> {
        if dci == 0 || dci >= MAX_DCI {
            return None;
        }
        self.rings[dci].as_ref().map(|r| r.phys_addr)
    }
}

pub struct XhciController {
    pub bar0: u64,
    pub cap_length: u8,
    pub hci_version: u16,
    pub max_slots: u8,
    pub max_ports: u8,
    pub max_interrupters: u16,
    pub devices: Vec<UsbDevice>,
    pub page_size: u32,
    /// Device Context Base Address Array
    pub dcbaa: Vec<u64>,
    /// Command ring
    pub cmd_ring: Option<TrbRing>,
    /// Event ring
    pub event_ring: Option<TrbRing>,
    /// Doorbell offset from BAR0
    pub doorbell_offset: u32,
    /// Runtime register offset from BAR0
    pub runtime_offset: u32,
    /// Number of scratchpad buffers needed
    pub scratchpad_count: u16,
    /// Whether the controller is running
    pub running: bool,
    /// Extended capabilities pointer
    pub ext_caps_offset: u32,
    /// Context size (32 or 64 bytes)
    pub context_size: u8,
    /// Per-slot transfer ring tables (indexed by slot_id, 1-based)
    pub slot_rings: Vec<SlotTransferRings>,
    /// MMIO base address (alias of bar0 for compatibility)
    pub mmio_base: usize,
}

impl XhciController {
    pub fn new(bar0: u64) -> Self {
        let cap_length = unsafe { core::ptr::read_volatile(bar0 as *const u8) };
        let hci_version =
            unsafe { core::ptr::read_volatile((bar0 + HCIVERSION as u64) as *const u16) };
        let hcsparams1 =
            unsafe { core::ptr::read_volatile((bar0 + HCSPARAMS1 as u64) as *const u32) };
        let hcsparams2 =
            unsafe { core::ptr::read_volatile((bar0 + HCSPARAMS2 as u64) as *const u32) };
        let hccparams1 =
            unsafe { core::ptr::read_volatile((bar0 + HCCPARAMS1 as u64) as *const u32) };
        let dboff = unsafe { core::ptr::read_volatile((bar0 + DBOFF as u64) as *const u32) };
        let rtsoff = unsafe { core::ptr::read_volatile((bar0 + RTSOFF as u64) as *const u32) };

        let max_slots = (hcsparams1 & 0xFF) as u8;
        let max_interrupters = ((hcsparams1 >> 8) & 0x7FF) as u16;
        let max_ports = ((hcsparams1 >> 24) & 0xFF) as u8;

        // Scratchpad buffer count: high 5 bits from hcsparams2[25:21], low 5 bits from [4:0]
        let spc_hi = ((hcsparams2 >> 21) & 0x1F) as u16;
        let spc_lo = (hcsparams2 & 0x1F) as u16;
        let scratchpad_count = (spc_hi << 5) | spc_lo;

        // Context size: bit 2 of HCCPARAMS1 indicates 64-byte contexts
        let context_size = if hccparams1 & (1 << 2) != 0 { 64 } else { 32 };

        // Extended capabilities pointer (bits 16:31 of HCCPARAMS1, in dwords)
        let ext_caps_offset = ((hccparams1 >> 16) & 0xFFFF) * 4;

        XhciController {
            bar0,
            cap_length,
            hci_version,
            max_slots,
            max_ports,
            max_interrupters,
            devices: Vec::new(),
            page_size: 4096,
            dcbaa: Vec::new(),
            cmd_ring: None,
            event_ring: None,
            doorbell_offset: dboff,
            runtime_offset: rtsoff,
            scratchpad_count,
            running: false,
            ext_caps_offset,
            context_size,
            slot_rings: Vec::new(),
            mmio_base: bar0 as usize,
        }
    }

    fn op_base(&self) -> u64 {
        self.bar0 + self.cap_length as u64
    }

    fn read_op(&self, offset: u32) -> u32 {
        unsafe { core::ptr::read_volatile((self.op_base() + offset as u64) as *const u32) }
    }

    fn write_op(&self, offset: u32, val: u32) {
        unsafe {
            core::ptr::write_volatile((self.op_base() + offset as u64) as *mut u32, val);
        }
    }

    /// Ring the doorbell for a given slot (0 = host controller command ring)
    fn ring_doorbell(&self, slot: u8, target: u32) {
        let db_addr = self.bar0 + self.doorbell_offset as u64 + (slot as u64 * 4);
        // Fence: ensure all TRB writes are visible to the xHCI controller
        // before the doorbell store triggers DMA fetch.
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        unsafe {
            core::ptr::write_volatile(db_addr as *mut u32, target);
        }
    }

    /// Reset the controller
    pub fn reset(&self) {
        // Stop the controller first
        let cmd = self.read_op(USBCMD);
        self.write_op(USBCMD, cmd & !USBCMD_RS);
        // Wait for HCHalted
        for _ in 0..100_000 {
            if self.read_op(USBSTS) & USBSTS_HCH != 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // Set HCRST bit
        self.write_op(USBCMD, self.read_op(USBCMD) | USBCMD_HCRST);
        // Wait for reset to complete
        for _ in 0..100_000 {
            if self.read_op(USBCMD) & USBCMD_HCRST == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        // Wait for CNR (Controller Not Ready) to clear
        for _ in 0..100_000 {
            if self.read_op(USBSTS) & USBSTS_CNR == 0 {
                break;
            }
            core::hint::spin_loop();
        }
    }

    /// Initialize DCBAA, command ring, event ring
    pub fn setup_data_structures(&mut self) {
        // Read page size
        let ps = self.read_op(PAGESIZE);
        self.page_size = (ps & 0xFFFF) << 12; // page size in bytes
        if self.page_size == 0 {
            self.page_size = 4096;
        }

        // Set max device slots
        self.write_op(CONFIG, self.max_slots as u32);

        // Allocate DCBAA (one u64 per slot + slot 0 for scratchpad)
        let _dcbaa_size = (self.max_slots as usize + 1) * 8;
        self.dcbaa = alloc::vec![0u64; self.max_slots as usize + 1];

        // Set DCBAA pointer
        let dcbaa_phys = self.dcbaa.as_ptr() as u64;
        self.write_op(DCBAAP, dcbaa_phys as u32);
        self.write_op(DCBAAP + 4, (dcbaa_phys >> 32) as u32);

        // Create command ring
        let cmd_ring = TrbRing::new(RING_SIZE);
        let crcr_val = cmd_ring.phys_addr | 1; // cycle state = 1
        self.write_op(CRCR, crcr_val as u32);
        self.write_op(CRCR + 4, (crcr_val >> 32) as u32);
        self.cmd_ring = Some(cmd_ring);

        // Create event ring
        self.event_ring = Some(TrbRing::new(RING_SIZE));

        serial_println!(
            "    [xhci] Data structures initialized (DCBAA={:#x}, {} slots)",
            dcbaa_phys,
            self.max_slots
        );
    }

    /// Start the controller
    pub fn start(&mut self) {
        let cmd = self.read_op(USBCMD);
        self.write_op(USBCMD, cmd | USBCMD_RS | USBCMD_INTE);

        // Wait for not halted
        for _ in 0..100_000 {
            if self.read_op(USBSTS) & USBSTS_HCH == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        self.running = true;
        serial_println!("    [xhci] Controller started (RS=1)");
    }

    /// Stop the controller
    pub fn stop(&mut self) {
        let cmd = self.read_op(USBCMD);
        self.write_op(USBCMD, cmd & !USBCMD_RS); // Run/Stop = 0
        for _ in 0..100_000 {
            if self.read_op(USBSTS) & USBSTS_HCH != 0 {
                break;
            }
            core::hint::spin_loop();
        }
        self.running = false;
    }

    /// Read port status/control register
    pub fn port_status(&self, port: u8) -> u32 {
        let port_offset = PORTSC_OFFSET + (port as u32 - 1) * PORT_STRIDE;
        self.read_op(port_offset)
    }

    /// Write port status/control register (preserving RW1C bits)
    pub fn write_port_status(&self, port: u8, val: u32) {
        let port_offset = PORTSC_OFFSET + (port as u32 - 1) * PORT_STRIDE;
        self.write_op(port_offset, val);
    }

    /// Check if a device is connected to a port
    pub fn port_connected(&self, port: u8) -> bool {
        self.port_status(port) & PORTSC_CCS != 0
    }

    /// Check if a port is enabled
    pub fn port_enabled(&self, port: u8) -> bool {
        self.port_status(port) & PORTSC_PED != 0
    }

    /// Check if a port is powered
    pub fn port_powered(&self, port: u8) -> bool {
        self.port_status(port) & PORTSC_PP != 0
    }

    /// Get port speed
    pub fn port_speed(&self, port: u8) -> UsbSpeed {
        match (self.port_status(port) >> 10) & 0xF {
            1 => UsbSpeed::Full,
            2 => UsbSpeed::Low,
            3 => UsbSpeed::High,
            4 => UsbSpeed::Super,
            5 => UsbSpeed::SuperPlus,
            _ => UsbSpeed::Full,
        }
    }

    /// Reset a port
    pub fn reset_port(&self, port: u8) {
        let ps = self.port_status(port);
        // Set PR (Port Reset) while preserving other bits (but clearing RW1C bits)
        let val = (ps & !0x00FE0000) | PORTSC_PR;
        self.write_port_status(port, val);

        // Wait for reset to complete (PRC bit set)
        for _ in 0..100_000 {
            let status = self.port_status(port);
            if status & PORTSC_PRC != 0 {
                // Clear PRC by writing 1 to it
                self.write_port_status(port, status | PORTSC_PRC);
                break;
            }
            core::hint::spin_loop();
        }
    }

    /// Enumerate all ports and detect connected devices
    pub fn enumerate_ports(&mut self) {
        serial_println!("    [xhci] Enumerating {} ports...", self.max_ports);
        for port in 1..=self.max_ports {
            let status = self.port_status(port);
            let connected = status & PORTSC_CCS != 0;
            let enabled = status & PORTSC_PED != 0;
            let _powered = status & PORTSC_PP != 0;

            if connected {
                let speed = self.port_speed(port);
                serial_println!(
                    "    [xhci] Port {}: connected, {}, {}",
                    port,
                    speed.name(),
                    if enabled { "enabled" } else { "disabled" }
                );
            }
        }
    }

    /// Submit a command TRB and ring the command doorbell
    pub fn submit_command(&mut self, trb: Trb) {
        if let Some(ref mut ring) = self.cmd_ring {
            ring.enqueue(trb);
            // Ring doorbell 0 (host controller) with target 0 (command ring)
            self.ring_doorbell(0, 0);
        }
    }

    /// Send Enable Slot command
    pub fn enable_slot(&mut self) {
        let mut trb = Trb::empty();
        trb.set_type(TRB_ENABLE_SLOT);
        self.submit_command(trb);
        serial_println!("    [xhci] Enable Slot command submitted");
    }

    /// Send Disable Slot command
    pub fn disable_slot(&mut self, slot_id: u8) {
        let mut trb = Trb::empty();
        trb.set_type(TRB_DISABLE_SLOT);
        trb.control |= (slot_id as u32) << 24;
        self.submit_command(trb);
    }

    /// Send No-Op command (for testing command ring)
    pub fn no_op(&mut self) {
        let mut trb = Trb::empty();
        trb.set_type(TRB_NO_OP_CMD);
        self.submit_command(trb);
    }

    /// Handle interrupt from xHCI controller
    pub fn handle_interrupt(&mut self) {
        let sts = self.read_op(USBSTS);

        if sts & USBSTS_EINT != 0 {
            // Clear event interrupt by writing 1 to EINT in USBSTS (RW1C)
            self.write_op(USBSTS, USBSTS_EINT);

            // Process event ring — collect events first to avoid borrow conflict
            let mut events = Vec::new();
            if let Some(ref mut ring) = self.event_ring {
                while let Some(trb) = ring.dequeue() {
                    events.push(trb);
                }
            }
            for trb in events {
                self.process_event(trb);
            }

            // Update ERDP so the xHCI controller knows we consumed the events.
            // This must be done AFTER we have finished reading the event ring.
            self.update_erdp();
        }

        if sts & USBSTS_PCD != 0 {
            // Port Change Detect
            self.write_op(USBSTS, USBSTS_PCD);
            serial_println!("    [xhci] Port change detected");
        }

        if sts & USBSTS_HSE != 0 {
            self.write_op(USBSTS, USBSTS_HSE);
            serial_println!("    [xhci] Host system error!");
        }
    }

    /// Process a single event TRB
    fn process_event(&mut self, trb: Trb) {
        let trb_type = trb.trb_type();
        let cc = trb.completion_code();

        match trb_type {
            TRB_COMMAND_COMPLETION => {
                let slot = trb.slot_id();
                if cc == CC_SUCCESS {
                    serial_println!("    [xhci] Command completed: slot {}", slot);
                } else {
                    serial_println!("    [xhci] Command failed: cc={}", cc);
                }
            }
            TRB_PORT_STATUS_CHANGE => {
                let port = ((trb.param_lo >> 24) & 0xFF) as u8;
                serial_println!("    [xhci] Port {} status change", port);
            }
            TRB_TRANSFER_EVENT => {
                let slot = trb.slot_id();
                if cc != CC_SUCCESS && cc != CC_SHORT_PACKET {
                    serial_println!("    [xhci] Transfer error: slot={}, cc={}", slot, cc);
                }
            }
            _ => {}
        }
    }

    /// Get controller info string
    pub fn info(&self) -> String {
        alloc::format!(
            "xHCI v{}.{} -- {} ports, {} slots, ctx={}B, {}",
            self.hci_version >> 8,
            self.hci_version & 0xFF,
            self.max_ports,
            self.max_slots,
            self.context_size,
            if self.running { "running" } else { "stopped" },
        )
    }

    // -----------------------------------------------------------------------
    // Runtime register access (interrupter 0)
    // -----------------------------------------------------------------------

    fn rt_base(&self) -> u64 {
        self.bar0 + self.runtime_offset as u64
    }

    fn read_rt(&self, offset: u32) -> u32 {
        unsafe { core::ptr::read_volatile((self.rt_base() + offset as u64) as *const u32) }
    }

    fn write_rt(&self, offset: u32, val: u32) {
        unsafe {
            core::ptr::write_volatile((self.rt_base() + offset as u64) as *mut u32, val);
        }
    }

    /// Update the Event Ring Dequeue Pointer (ERDP) for interrupter 0.
    /// Must be written after processing events to signal to the HC that we
    /// have consumed up to this address.
    fn update_erdp(&self) {
        // Interrupter 0 ERDP register is at runtime_base + 0x38 (IMAN/IMOD/ERSTSZ/ERSTBA/ERDP)
        // Offset layout within interrupter register set (each 32 bytes):
        //   0x00 IMAN, 0x04 IMOD, 0x08 ERSTSZ, 0x0C reserved,
        //   0x10 ERSTBA_LO, 0x14 ERSTBA_HI, 0x18 ERDP_LO, 0x1C ERDP_HI
        // Interrupter 0 starts at runtime_base + 0x20
        const IR0_ERDP_LO: u32 = 0x20 + 0x18;
        const IR0_ERDP_HI: u32 = 0x20 + 0x1C;

        if let Some(ref ring) = self.event_ring {
            let deq_idx = ring.dequeue_idx;
            let deq_addr =
                ring.phys_addr + (deq_idx.saturating_mul(core::mem::size_of::<Trb>())) as u64;
            // EHB (Event Handler Busy) bit = bit 3 of ERDP_LO must be written 1 to clear
            let lo = (deq_addr as u32) | (1 << 3);
            let hi = (deq_addr >> 32) as u32;
            // Fence before ERDP write: all event reads must complete first
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            self.write_rt(IR0_ERDP_LO, lo);
            self.write_rt(IR0_ERDP_HI, hi);
        }
    }

    // -----------------------------------------------------------------------
    // Per-slot transfer ring management
    // -----------------------------------------------------------------------

    /// Ensure a transfer ring exists for (slot_id, dci) and return its
    /// physical base address, or 0 on error.
    pub fn ensure_transfer_ring(&mut self, slot_id: u8, dci: usize) -> u64 {
        // Grow slot_rings if needed (indexed 0..max_slots, slot 0 unused)
        let slot_idx = slot_id as usize;
        while self.slot_rings.len() <= slot_idx {
            let next_slot = self.slot_rings.len() as u8;
            self.slot_rings.push(SlotTransferRings::new(next_slot));
        }
        self.slot_rings[slot_idx]
            .ensure_ring(dci)
            .map(|r| r.phys_addr)
            .unwrap_or(0)
    }

    /// Ring the endpoint doorbell for a specific slot+DCI.
    /// `dci` is the Doorbell Target field (matches endpoint DCI).
    pub fn ring_ep_doorbell(&self, slot_id: u8, dci: u8) {
        // Per xHCI spec: Doorbell[slot_id] with Target = dci
        self.ring_doorbell(slot_id, dci as u32);
    }

    // -----------------------------------------------------------------------
    // Control transfer TRB chain
    // -----------------------------------------------------------------------

    /// Build and enqueue a 3-TRB control transfer on EP0 (DCI 1) for a slot.
    /// `setup`  — 8-byte USB setup packet (stored little-endian in param_lo/hi)
    /// `dir_in` — true if the data stage transfers data from device to host
    /// `data_len` — transfer length (0 for no-data control transfers)
    /// Returns the physical address of the Setup Stage TRB.
    pub fn enqueue_control_transfer(
        &mut self,
        slot_id: u8,
        setup: &[u8; 8],
        dir_in: bool,
        data_len: u16,
    ) -> u64 {
        let dci = ep_dci(0, true); // EP0 = DCI 1

        // Ensure the ring exists
        self.ensure_transfer_ring(slot_id, dci);

        let slot_idx = slot_id as usize;
        if slot_idx >= self.slot_rings.len() {
            return 0;
        }

        // -- Setup Stage TRB (type 2) --
        // param_lo/hi hold the 8-byte setup packet.
        // status[16:0] = Transfer Length (always 8 for setup).
        // control[16] = IDT (Immediate Data).
        // control[5:4] = TRT (Transfer Type): 0=no-data, 2=OUT, 3=IN
        let trt: u32 = if data_len == 0 {
            0
        } else if dir_in {
            3
        } else {
            2
        };
        let mut setup_trb = Trb::empty();
        setup_trb.set_type(TRB_SETUP_STAGE);
        // Pack 8 setup bytes into param_lo/hi (little-endian)
        setup_trb.param_lo = (setup[0] as u32)
            | ((setup[1] as u32) << 8)
            | ((setup[2] as u32) << 16)
            | ((setup[3] as u32) << 24);
        setup_trb.param_hi = (setup[4] as u32)
            | ((setup[5] as u32) << 8)
            | ((setup[6] as u32) << 16)
            | ((setup[7] as u32) << 24);
        setup_trb.status = 8; // Transfer Length: always 8 bytes for setup packet
                              // Per xHCI spec 6.4.1.2.1:
                              //   bits[15:10] = TRB Type (6)
                              //   bit[6]      = IDT (Immediate Data Transfer: setup data in param_lo/hi)
                              //   bits[17:16] = TRT (Transfer Type: 0=no-data, 2=OUT-data, 3=IN-data)
        setup_trb.control = ((TRB_SETUP_STAGE as u32) << 10)
            | (1 << 6)     // IDT
            | (trt << 16); // TRT

        let setup_addr = if let Some(ring) = self.slot_rings[slot_idx].get_ring(dci) {
            ring.enqueue(setup_trb)
        } else {
            return 0;
        };

        // -- Data Stage TRB (type 3) — only if data_len > 0 --
        if data_len > 0 {
            let mut data_trb = Trb::empty();
            data_trb.set_type(TRB_DATA_STAGE);
            // param_lo/hi = physical data buffer address (caller fills later;
            // we leave at 0 here since we don't have a DMA buffer at this layer)
            data_trb.status = data_len as u32; // Transfer Length
                                               // DIR bit (bit 16): 1 = IN (device to host)
            data_trb.control = ((TRB_DATA_STAGE as u32) << 10)
                | (1 << 5)  // ISP (Interrupt on Short Packet)
                | (1 << 4)  // CH (Chain Bit: links to Status stage)
                | (if dir_in { 1 << 16 } else { 0 }); // DIR
            if let Some(ring) = self.slot_rings[slot_idx].get_ring(dci) {
                ring.enqueue(data_trb);
            }
        }

        // -- Status Stage TRB (type 4) --
        let mut status_trb = Trb::empty();
        status_trb.set_type(TRB_STATUS_STAGE);
        // For a no-data or OUT-data transfer, Status stage direction is IN; vice versa.
        let status_dir_in: bool = data_len == 0 || !dir_in;
        status_trb.control = ((TRB_STATUS_STAGE as u32) << 10)
            | (1 << 5)  // IOC (Interrupt on Completion)
            | (if status_dir_in { 1 << 16 } else { 0 }); // DIR
        if let Some(ring) = self.slot_rings[slot_idx].get_ring(dci) {
            ring.enqueue(status_trb);
        }

        // Ring EP0 doorbell
        self.ring_ep_doorbell(slot_id, dci as u8);

        setup_addr
    }

    // -----------------------------------------------------------------------
    // Bulk transfer TRB
    // -----------------------------------------------------------------------

    /// Enqueue a Normal TRB for a bulk or interrupt transfer.
    /// `ep_num`     — endpoint number (1-15)
    /// `direction_in` — true for IN (device→host), false for OUT (host→device)
    /// `buf_phys`   — physical address of the data buffer
    /// `len`        — transfer length in bytes
    /// `ioc`        — true to generate an interrupt on completion
    /// Returns the physical address of the enqueued TRB, or 0 on error.
    pub fn enqueue_bulk_transfer(
        &mut self,
        slot_id: u8,
        ep_num: u8,
        direction_in: bool,
        buf_phys: u64,
        len: u32,
        ioc: bool,
    ) -> u64 {
        let dci = ep_dci(ep_num, direction_in);
        self.ensure_transfer_ring(slot_id, dci);

        let slot_idx = slot_id as usize;
        if slot_idx >= self.slot_rings.len() {
            return 0;
        }

        let mut trb = Trb::empty();
        trb.set_type(TRB_NORMAL);
        // Buffer pointer (64-bit physical address split across param_lo/hi)
        trb.param_lo = buf_phys as u32;
        trb.param_hi = (buf_phys >> 32) as u32;
        // Transfer length in bits[16:0]; TD Size (number of remaining TRBs) in bits[21:17]
        trb.status = len & 0x1_FFFF; // Transfer length (max 131071 bytes)
                                     // Control: IOC, ISP (interrupt on short packet)
        trb.control = ((TRB_NORMAL as u32) << 10)
            | (if ioc { 1 << 5 } else { 0 })   // IOC
            | (1 << 2); // ISP

        let addr = if let Some(ring) = self.slot_rings[slot_idx].get_ring(dci) {
            ring.enqueue(trb)
        } else {
            return 0;
        };

        // Ring the endpoint doorbell
        self.ring_ep_doorbell(slot_id, dci as u8);
        addr
    }

    // -----------------------------------------------------------------------
    // Interrupt transfer TRB (same structure as bulk, different endpoint type)
    // -----------------------------------------------------------------------

    /// Enqueue a Normal TRB for an interrupt IN transfer (e.g., HID polling).
    pub fn enqueue_interrupt_transfer(
        &mut self,
        slot_id: u8,
        ep_num: u8,
        buf_phys: u64,
        len: u32,
    ) -> u64 {
        // Interrupt endpoints are always IN in HID boot protocol; direction_in = true.
        self.enqueue_bulk_transfer(slot_id, ep_num, true, buf_phys, len, true)
    }

    // -----------------------------------------------------------------------
    // Address Device command
    // -----------------------------------------------------------------------

    /// Send Address Device command for a slot.
    /// `input_ctx_phys` — physical address of the Input Context structure.
    /// `bsr`            — Block Set Address Request (set true for the first
    ///                    step per xHCI, then false for actual addressing).
    pub fn address_device(&mut self, slot_id: u8, input_ctx_phys: u64, bsr: bool) {
        let mut trb = Trb::empty();
        trb.set_type(TRB_ADDRESS_DEVICE);
        trb.param_lo = input_ctx_phys as u32;
        trb.param_hi = (input_ctx_phys >> 32) as u32;
        trb.control |= (slot_id as u32) << 24;
        if bsr {
            trb.control |= 1 << 9;
        } // BSR bit
        self.submit_command(trb);
        serial_println!("    [xhci] Address Device: slot={} bsr={}", slot_id, bsr);
    }

    /// Send Configure Endpoint command for a slot.
    pub fn configure_endpoint(&mut self, slot_id: u8, input_ctx_phys: u64) {
        let mut trb = Trb::empty();
        trb.set_type(TRB_CONFIGURE_EP);
        trb.param_lo = input_ctx_phys as u32;
        trb.param_hi = (input_ctx_phys >> 32) as u32;
        trb.control |= (slot_id as u32) << 24;
        self.submit_command(trb);
        serial_println!("    [xhci] Configure Endpoint: slot={}", slot_id);
    }

    /// Send Reset Endpoint command (clears a halted endpoint).
    pub fn reset_endpoint(&mut self, slot_id: u8, dci: u8) {
        let mut trb = Trb::empty();
        trb.set_type(TRB_RESET_EP);
        trb.control |= (slot_id as u32) << 24;
        trb.control |= (dci as u32) << 16;
        self.submit_command(trb);
        serial_println!("    [xhci] Reset Endpoint: slot={} dci={}", slot_id, dci);
    }

    /// Send Reset Device command.
    pub fn reset_device(&mut self, slot_id: u8) {
        let mut trb = Trb::empty();
        trb.set_type(TRB_RESET_DEVICE);
        trb.control |= (slot_id as u32) << 24;
        self.submit_command(trb);
        serial_println!("    [xhci] Reset Device: slot={}", slot_id);
    }

    // -----------------------------------------------------------------------
    // Standard enumeration control requests
    // -----------------------------------------------------------------------

    /// Queue a GET_DESCRIPTOR (Device, 18 bytes) control transfer on EP0.
    pub fn get_device_descriptor(&mut self, slot_id: u8) -> u64 {
        // bmRequestType=0x80, bRequest=0x06, wValue=0x0100, wIndex=0, wLength=18
        let setup: [u8; 8] = [0x80, 0x06, 0x00, 0x01, 0x00, 0x00, 0x12, 0x00];
        self.enqueue_control_transfer(slot_id, &setup, true, 18)
    }

    /// Queue a GET_DESCRIPTOR (Configuration, first 9 bytes) control transfer.
    pub fn get_config_descriptor_header(&mut self, slot_id: u8) -> u64 {
        let setup: [u8; 8] = [0x80, 0x06, 0x00, 0x02, 0x00, 0x00, 0x09, 0x00];
        self.enqueue_control_transfer(slot_id, &setup, true, 9)
    }

    /// Queue a GET_DESCRIPTOR (Configuration, full length) control transfer.
    pub fn get_config_descriptor_full(&mut self, slot_id: u8, total_len: u16) -> u64 {
        let len_bytes = total_len.to_le_bytes();
        let setup: [u8; 8] = [
            0x80,
            0x06,
            0x00,
            0x02,
            0x00,
            0x00,
            len_bytes[0],
            len_bytes[1],
        ];
        self.enqueue_control_transfer(slot_id, &setup, true, total_len)
    }

    /// Queue a SET_ADDRESS control transfer (no data stage, slot addressed).
    pub fn set_address_request(&mut self, slot_id: u8, address: u8) -> u64 {
        let setup: [u8; 8] = [0x00, 0x05, address, 0x00, 0x00, 0x00, 0x00, 0x00];
        self.enqueue_control_transfer(slot_id, &setup, false, 0)
    }

    /// Queue a SET_CONFIGURATION control transfer.
    pub fn set_configuration_request(&mut self, slot_id: u8, config_value: u8) -> u64 {
        let setup: [u8; 8] = [0x00, 0x09, config_value, 0x00, 0x00, 0x00, 0x00, 0x00];
        self.enqueue_control_transfer(slot_id, &setup, false, 0)
    }

    /// Queue a HID SET_IDLE request (class request on interface).
    pub fn hid_set_idle(&mut self, slot_id: u8, interface: u8, duration: u8, report_id: u8) -> u64 {
        // bmRequestType=0x21, bRequest=0x0A (SET_IDLE)
        // wValue = (duration << 8) | report_id, wIndex = interface, wLength = 0
        let setup: [u8; 8] = [0x21, 0x0A, report_id, duration, interface, 0x00, 0x00, 0x00];
        self.enqueue_control_transfer(slot_id, &setup, false, 0)
    }

    /// Queue a HID SET_PROTOCOL request.
    pub fn hid_set_protocol(&mut self, slot_id: u8, interface: u8, protocol: u8) -> u64 {
        // bmRequestType=0x21, bRequest=0x0B (SET_PROTOCOL)
        // wValue = protocol (0=boot, 1=report), wIndex = interface, wLength = 0
        let setup: [u8; 8] = [0x21, 0x0B, protocol, 0x00, interface, 0x00, 0x00, 0x00];
        self.enqueue_control_transfer(slot_id, &setup, false, 0)
    }

    /// Queue a Mass Storage GET_MAX_LUN request.
    pub fn msc_get_max_lun(&mut self, slot_id: u8, interface: u8) -> u64 {
        // bmRequestType=0xA1, bRequest=0xFE, wValue=0, wIndex=interface, wLength=1
        let setup: [u8; 8] = [0xA1, 0xFE, 0x00, 0x00, interface, 0x00, 0x01, 0x00];
        self.enqueue_control_transfer(slot_id, &setup, true, 1)
    }

    /// Queue a Mass Storage Bulk-Only Reset request.
    pub fn msc_bulk_reset(&mut self, slot_id: u8, interface: u8) -> u64 {
        // bmRequestType=0x21, bRequest=0xFF, wValue=0, wIndex=interface, wLength=0
        let setup: [u8; 8] = [0x21, 0xFF, 0x00, 0x00, interface, 0x00, 0x00, 0x00];
        self.enqueue_control_transfer(slot_id, &setup, false, 0)
    }

    /// Queue a CDC SET_LINE_CODING request (7-byte OUT data stage).
    pub fn cdc_set_line_coding(&mut self, slot_id: u8, interface: u8) -> u64 {
        // bmRequestType=0x21, bRequest=0x20, wValue=0, wIndex=interface, wLength=7
        let setup: [u8; 8] = [0x21, 0x20, 0x00, 0x00, interface, 0x00, 0x07, 0x00];
        self.enqueue_control_transfer(slot_id, &setup, false, 7)
    }

    /// Queue a CDC SET_CONTROL_LINE_STATE request (no data).
    pub fn cdc_set_control_line_state(
        &mut self,
        slot_id: u8,
        interface: u8,
        dtr: bool,
        rts: bool,
    ) -> u64 {
        let wvalue: u8 = (if dtr { 1 } else { 0 }) | (if rts { 2 } else { 0 });
        let setup: [u8; 8] = [0x21, 0x22, wvalue, 0x00, interface, 0x00, 0x00, 0x00];
        self.enqueue_control_transfer(slot_id, &setup, false, 0)
    }

    // -----------------------------------------------------------------------
    // Port detection helpers
    // -----------------------------------------------------------------------

    /// Determine whether a port is USB 3.x (SuperSpeed) based on PORTSC speed field.
    pub fn port_is_superspeed(&self, port: u8) -> bool {
        matches!(self.port_speed(port), UsbSpeed::Super | UsbSpeed::SuperPlus)
    }

    /// Wait for a port to leave reset state and return whether it succeeded.
    pub fn wait_port_reset_done(&self, port: u8) -> bool {
        for _ in 0..200_000 {
            let status = self.port_status(port);
            if status & PORTSC_PR == 0 {
                return true; // Reset bit cleared = reset complete
            }
            core::hint::spin_loop();
        }
        false // Timed out
    }

    /// Count connected USB devices
    pub fn connected_device_count(&self) -> u8 {
        let mut count: u8 = 0;
        for port in 1..=self.max_ports {
            if self.port_connected(port) {
                count = count.saturating_add(1);
            }
        }
        count
    }
}

// ---------------------------------------------------------------------------
// Module-level API
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("    [xhci] Scanning for xHCI USB controllers...");

    // Look for xHCI controller on PCI bus
    // Class: 0x0C (Serial Bus), Subclass: 0x03 (USB), Prog-IF: 0x30 (xHCI)
    let controllers = crate::drivers::pci::find_by_class_full(0x0C, 0x03, 0x30);
    if let Some(dev) = controllers.first() {
        let (bar0_raw, is_mmio) =
            crate::drivers::pci::read_bar(dev.bus, dev.device, dev.function, 0);
        if !is_mmio || bar0_raw == 0 {
            serial_println!("    [xhci] xHCI controller found but BAR0 is not valid MMIO");
            return;
        }
        let bar0 = bar0_raw;

        serial_println!(
            "    [xhci] xHCI controller at {:02x}:{:02x}.{}, BAR0={:#x}",
            dev.bus,
            dev.device,
            dev.function,
            bar0
        );

        // Identity-map the MMIO region (64 KB is enough for xHCI registers)
        let map_size: usize = 0x10000;
        let pages = map_size / 0x1000;
        for i in 0..pages {
            let page = bar0 as usize + i * 0x1000;
            let flags =
                crate::memory::paging::flags::WRITABLE | crate::memory::paging::flags::NO_CACHE;
            let _ = crate::memory::paging::map_page(page, page, flags);
        }

        let mut ctrl = XhciController::new(bar0 as u64);
        serial_println!(
            "    [xhci] Version: {}.{}, {} ports, {} max slots",
            ctrl.hci_version >> 8,
            ctrl.hci_version & 0xFF,
            ctrl.max_ports,
            ctrl.max_slots
        );

        // Reset the controller
        ctrl.reset();
        serial_println!("    [xhci] Controller reset complete");

        // Set up data structures
        ctrl.setup_data_structures();

        // Start the controller
        ctrl.start();

        // Enumerate ports
        ctrl.enumerate_ports();

        let connected = ctrl.connected_device_count();
        serial_println!("    [xhci] {} device(s) connected", connected);

        *XHCI_STATE.lock() = Some(ctrl);

        crate::drivers::register("xhci-usb", crate::drivers::DeviceType::Usb);
        return;
    }

    serial_println!("    [xhci] No xHCI controller found, driver loaded (waiting for hardware)");
}

/// Handle xHCI interrupt (called from interrupt handler)
pub fn handle_interrupt() {
    if let Some(ref mut ctrl) = *XHCI_STATE.lock() {
        ctrl.handle_interrupt();
    }
}

/// Get number of connected USB devices
pub fn connected_devices() -> u8 {
    XHCI_STATE
        .lock()
        .as_ref()
        .map_or(0, |c| c.connected_device_count())
}

/// Get controller info string
pub fn info() -> Option<String> {
    XHCI_STATE.lock().as_ref().map(|c| c.info())
}

/// Check if xHCI is initialized and running
pub fn is_running() -> bool {
    XHCI_STATE.lock().as_ref().map_or(false, |c| c.running)
}

/// Suspend all USB ports by issuing the SUSPEND port feature command.
///
/// Iterates every port on the xHCI controller and sets the PORT_LINK_STATE
/// field in PORTSC to U3 (Suspended) by writing the correct LTS/PLS bits.
/// On EHCI the equivalent is writing the SUSPEND bit in PORTSC; xHCI uses
/// the Link Training State machine instead.
///
/// This is safe to call when no controller is present (no-op).
pub fn suspend_all() {
    if let Some(ref mut ctrl) = *XHCI_STATE.lock() {
        // Walk all ports and suspend them via the operational register set.
        // PORTSC offset = op_base + 0x400 + port_index * 0x10
        // Bit field: [5:2] PLS (Port Link State).  U3 = 0b0011.
        // Writing PLC=1 and PLS=U3 with LWS=1 issues the suspend request.
        let op_base = ctrl.mmio_base as u64 + ctrl.cap_length as u64;
        for port in 0..ctrl.max_ports as u64 {
            let portsc_addr = (op_base + 0x400 + port * 0x10) as *mut u32;
            unsafe {
                let portsc = core::ptr::read_volatile(portsc_addr);
                // LWS (bit 16) = 1 to allow PLS writes; PLS (bits 5:2) = U3 (0b0011)
                let new_portsc = (portsc & !0x003C) | (3u32 << 2) | (1u32 << 16);
                core::ptr::write_volatile(portsc_addr, new_portsc);
            }
        }
        serial_println!("  [xhci] {} port(s) suspended (U3)", ctrl.max_ports);
    } else {
        serial_println!("  [xhci] suspend_all: no controller, skipping");
    }
}

/// Enqueue a USB control transfer on the given slot's EP0.
///
/// Public module-level wrapper around `XhciController::enqueue_control_transfer`
/// so that other USB subsystem modules (e.g. `usb::hub`) can issue control
/// transfers without holding a direct reference to `XHCI_STATE`.
///
/// Parameters:
///   `slot_id`  — xHCI device slot (1-based).
///   `setup`    — 8-byte USB setup packet (little-endian).
///   `dir_in`   — `true` if data stage flows device → host.
///   `data_len` — transfer length in bytes (0 for no-data transfers).
///
/// Returns the physical address of the Setup Stage TRB, or 0 if the
/// controller is not initialised.
pub fn enqueue_control_transfer(slot_id: u8, setup: &[u8; 8], dir_in: bool, data_len: u16) -> u64 {
    if let Some(ref mut ctrl) = *XHCI_STATE.lock() {
        ctrl.enqueue_control_transfer(slot_id, setup, dir_in, data_len)
    } else {
        0
    }
}

/// Resume all USB ports from U3 (Suspended) to U0 (Active).
///
/// Writes PLS=U0 (0b0000) with LWS=1 to each PORTSC register.
/// This is safe to call when no controller is present (no-op).
pub fn resume_all() {
    if let Some(ref mut ctrl) = *XHCI_STATE.lock() {
        let op_base = ctrl.mmio_base as u64 + ctrl.cap_length as u64;
        for port in 0..ctrl.max_ports as u64 {
            let portsc_addr = (op_base + 0x400 + port * 0x10) as *mut u32;
            unsafe {
                let portsc = core::ptr::read_volatile(portsc_addr);
                // LWS (bit 16) = 1; PLS (bits 5:2) = U0 (0b0000)
                let new_portsc = (portsc & !0x003C) | (1u32 << 16);
                core::ptr::write_volatile(portsc_addr, new_portsc);
            }
        }
        serial_println!("  [xhci] {} port(s) resumed (U0)", ctrl.max_ports);
    } else {
        serial_println!("  [xhci] resume_all: no controller, skipping");
    }
}

/// Enqueue a bulk transfer TRB for `slot_id` on endpoint `ep_num`.
///
/// Module-level wrapper over `XhciController::enqueue_bulk_transfer` so that
/// other USB class drivers (e.g. `usb::mass_storage`) can submit bulk
/// transfers without holding a direct reference to `XHCI_STATE`.
///
/// Parameters:
///   `slot_id`    — xHCI device slot (1-based).
///   `ep_num`     — endpoint number (1-15).
///   `direction_in` — `true` = device→host (IN), `false` = host→device (OUT).
///   `buf_phys`   — physical address of the DMA buffer.
///   `len`        — transfer length in bytes.
///
/// Returns the physical address of the enqueued Normal TRB, or 0 if the
/// controller is not initialised.
pub fn enqueue_bulk_transfer(
    slot_id: u8,
    ep_num: u8,
    direction_in: bool,
    buf_phys: u64,
    len: u32,
) -> u64 {
    if let Some(ref mut ctrl) = *XHCI_STATE.lock() {
        ctrl.enqueue_bulk_transfer(slot_id, ep_num, direction_in, buf_phys, len, true)
    } else {
        0
    }
}

/// Poll an interrupt-IN endpoint for received HID data.
/// Returns number of bytes placed in `buf`, or 0 on no data/error.
pub fn interrupt_in_poll(dev_addr: u8, endpoint: u8, buf: &mut [u8]) -> usize {
    let _ = dev_addr;
    let _ = endpoint;
    // Stub: poll the event ring and copy data if available
    // Full implementation requires mapping dev_addr → slot_id
    let _ = buf;
    0
}
