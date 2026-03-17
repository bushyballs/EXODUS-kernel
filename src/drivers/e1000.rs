/// Intel e1000 (82540EM) Ethernet driver for Genesis — built from scratch
///
/// Implements the Intel 8254x family Ethernet controller driver.
/// Supports QEMU's default NIC (PCI 8086:100e).
///
/// Architecture:
///   - MMIO register access via PCI BAR0
///   - TX/RX descriptor rings in kernel memory
///   - Interrupt-driven receive, synchronous transmit
///   - Full init sequence: reset, EEPROM MAC, setup rings
///   - Link status detection, multicast table, statistics
///
/// Reference: Intel 8254x Developer's Manual (PCI/PCI-X Family)
/// No external crates. All code is original.
use crate::{serial_print, serial_println};
// I/O port access not needed for MMIO-based e1000, but kept for potential future use
use crate::drivers::pci;
use crate::io::{inl, outl};
use crate::memory::frame_allocator::FRAME_SIZE;
use crate::memory::{frame_allocator, paging};
use crate::net::{MacAddr, NetError, NetworkDriver};
use crate::sync::Mutex;
use alloc::vec::Vec;

/// e1000 register offsets
const REG_CTRL: u32 = 0x0000; // Device Control
const REG_STATUS: u32 = 0x0008; // Device Status
const REG_EECD: u32 = 0x0010; // EEPROM/Flash Control
const REG_EERD: u32 = 0x0014; // EEPROM Read
const REG_CTRL_EXT: u32 = 0x0018; // Extended Device Control
const REG_FCAL: u32 = 0x0028; // Flow Control Address Low
const REG_FCAH: u32 = 0x002C; // Flow Control Address High
const REG_FCT: u32 = 0x0030; // Flow Control Type
const REG_FCTTV: u32 = 0x0170; // Flow Control Transmit Timer Value
const REG_ICR: u32 = 0x00C0; // Interrupt Cause Read
const REG_ITR: u32 = 0x00C4; // Interrupt Throttling Rate
const REG_ICS: u32 = 0x00C8; // Interrupt Cause Set
const REG_IMS: u32 = 0x00D0; // Interrupt Mask Set
const REG_IMC: u32 = 0x00D8; // Interrupt Mask Clear
const REG_RCTL: u32 = 0x0100; // Receive Control
const REG_FCRTL: u32 = 0x2160; // Flow Control Receive Threshold Low
const REG_FCRTH: u32 = 0x2168; // Flow Control Receive Threshold High
const REG_RDBAL: u32 = 0x2800; // RX Descriptor Base Low
const REG_RDBAH: u32 = 0x2804; // RX Descriptor Base High
const REG_RDLEN: u32 = 0x2808; // RX Descriptor Length
const REG_RDH: u32 = 0x2810; // RX Descriptor Head
const REG_RDT: u32 = 0x2818; // RX Descriptor Tail
const REG_RDTR: u32 = 0x2820; // RX Delay Timer
const REG_RADV: u32 = 0x282C; // RX Absolute Delay Timer
const REG_RSRPD: u32 = 0x2C00; // RX Small Packet Detect
const REG_TCTL: u32 = 0x0400; // Transmit Control
const REG_TIPG: u32 = 0x0410; // Transmit Inter Packet Gap
const REG_TDBAL: u32 = 0x3800; // TX Descriptor Base Low
const REG_TDBAH: u32 = 0x3804; // TX Descriptor Base High
const REG_TDLEN: u32 = 0x3808; // TX Descriptor Length
const REG_TDH: u32 = 0x3810; // TX Descriptor Head
const REG_TDT: u32 = 0x3818; // TX Descriptor Tail
const REG_TIDV: u32 = 0x3820; // TX Interrupt Delay Value
const REG_TADV: u32 = 0x382C; // TX Absolute Interrupt Delay Value
const REG_RAL: u32 = 0x5400; // Receive Address Low
const REG_RAH: u32 = 0x5404; // Receive Address High
const REG_MTA: u32 = 0x5200; // Multicast Table Array (128 entries)

/// Statistics register offsets
const REG_CRCERRS: u32 = 0x4000; // CRC Errors
const REG_ALGNERRC: u32 = 0x4004; // Alignment Errors
const REG_RXERRC: u32 = 0x400C; // RX Errors
const REG_MPC: u32 = 0x4010; // Missed Packets
const REG_COLC: u32 = 0x4028; // Collision Count
const REG_GPRC: u32 = 0x4074; // Good Packets Received
const REG_BPRC: u32 = 0x4078; // Broadcast Packets Received
const REG_MPRC: u32 = 0x407C; // Multicast Packets Received
const REG_GPTC: u32 = 0x4080; // Good Packets Transmitted
const REG_GORCL: u32 = 0x4088; // Good Octets Received Count Low
const REG_GORCH: u32 = 0x408C; // Good Octets Received Count High
const REG_GOTCL: u32 = 0x4090; // Good Octets Transmitted Count Low
const REG_GOTCH: u32 = 0x4094; // Good Octets Transmitted Count High
const REG_RNBC: u32 = 0x40A0; // Receive No Buffers Count
const REG_RUC: u32 = 0x40A4; // Receive Undersize Count
const REG_ROC: u32 = 0x40AC; // Receive Oversize Count
const REG_TORL: u32 = 0x40C0; // Total Octets Received Low
const REG_TORH: u32 = 0x40C4; // Total Octets Received High
const REG_TOTL: u32 = 0x40C8; // Total Octets Transmitted Low
const REG_TOTH: u32 = 0x40CC; // Total Octets Transmitted High
const REG_TPR: u32 = 0x40D0; // Total Packets Received
const REG_TPT: u32 = 0x40D4; // Total Packets Transmitted
const REG_MPTC: u32 = 0x40F0; // Multicast Packets Transmitted

/// Control register bits
const CTRL_FD: u32 = 1 << 0; // Full-Duplex
const CTRL_ASDE: u32 = 1 << 5; // Auto-Speed Detection Enable
const CTRL_SLU: u32 = 1 << 6; // Set Link Up
const CTRL_FRCSPD: u32 = 1 << 11; // Force Speed
const CTRL_FRCDPLX: u32 = 1 << 12; // Force Duplex
const CTRL_RST: u32 = 1 << 26; // Reset
const CTRL_VME: u32 = 1 << 30; // VLAN Mode Enable
const CTRL_PHY_RST: u32 = 1 << 31; // PHY Reset

/// Receive control bits
const RCTL_EN: u32 = 1 << 1; // Enable
const RCTL_SBP: u32 = 1 << 2; // Store Bad Packets
const RCTL_UPE: u32 = 1 << 3; // Unicast Promiscuous
const RCTL_MPE: u32 = 1 << 4; // Multicast Promiscuous
const RCTL_LPE: u32 = 1 << 5; // Long Packet Enable
const RCTL_LBM_NONE: u32 = 0; // No Loopback
const RCTL_MO_36: u32 = 0; // Multicast offset bits 47:36
const RCTL_BAM: u32 = 1 << 15; // Broadcast Accept Mode
const RCTL_BSIZE_2048: u32 = 0; // Buffer size 2048 bytes
const RCTL_BSIZE_4096: u32 = 3 << 16; // Buffer size 4096
const RCTL_BSEX: u32 = 1 << 25; // Buffer size extension
const RCTL_SECRC: u32 = 1 << 26; // Strip Ethernet CRC

/// Transmit control bits
const TCTL_EN: u32 = 1 << 1; // Enable
const TCTL_PSP: u32 = 1 << 3; // Pad Short Packets
const TCTL_CT_IEEE: u32 = 15 << 4; // Collision Threshold
const TCTL_COLD_FD: u32 = 64 << 12; // Collision Distance (Full Duplex)
const TCTL_COLD_HD: u32 = 512 << 12; // Collision Distance (Half Duplex)
const TCTL_RTLC: u32 = 1 << 24; // Re-transmit on Late Collision
const TCTL_MULR: u32 = 1 << 28; // Multiple Request Support

/// Interrupt cause bits
const ICR_TXDW: u32 = 1 << 0; // TX Descriptor Written Back
const ICR_TXQE: u32 = 1 << 1; // TX Queue Empty
const ICR_LSC: u32 = 1 << 2; // Link Status Change
const ICR_RXSEQ: u32 = 1 << 3; // RX Sequence Error
const ICR_RXDMT0: u32 = 1 << 4; // RX Descriptor Min Threshold
const ICR_RXO: u32 = 1 << 6; // RX Overrun
const ICR_RXT0: u32 = 1 << 7; // RX Timer Interrupt

/// Number of TX/RX descriptors (must be multiple of 8, max 256 per ring)
const NUM_RX_DESC: usize = 256;
const NUM_TX_DESC: usize = 256;
/// Receive buffer size
const RX_BUF_SIZE: usize = 2048;
/// Maximum packet size (Ethernet MTU + headers)
const MAX_FRAME_SIZE: usize = 1518;

/// TX Descriptor (legacy format)
#[derive(Clone, Copy)]
#[repr(C, align(16))]
struct TxDesc {
    addr: u64,
    length: u16,
    cso: u8,
    cmd: u8,
    status: u8,
    css: u8,
    special: u16,
}

/// RX Descriptor
#[derive(Clone, Copy)]
#[repr(C, align(16))]
struct RxDesc {
    addr: u64,
    length: u16,
    checksum: u16,
    status: u8,
    errors: u8,
    special: u16,
}

/// TX command bits
const TX_CMD_EOP: u8 = 1 << 0; // End of Packet
const TX_CMD_IFCS: u8 = 1 << 1; // Insert FCS/CRC
const TX_CMD_IC: u8 = 1 << 2; // Insert Checksum
const TX_CMD_RS: u8 = 1 << 3; // Report Status
const TX_CMD_IDE: u8 = 1 << 7; // Interrupt Delay Enable

/// RX status bits
const RX_STATUS_DD: u8 = 1 << 0; // Descriptor Done
const RX_STATUS_EOP: u8 = 1 << 1; // End of Packet
const RX_STATUS_IXSM: u8 = 1 << 2; // Ignore Checksum Indication
const RX_STATUS_VP: u8 = 1 << 3; // VLAN Packet

/// RX error bits
const RX_ERR_CE: u8 = 1 << 0; // CRC Error
const RX_ERR_SEQ: u8 = 1 << 2; // Sequence Error
const RX_ERR_RXE: u8 = 1 << 7; // RX Data Error

/// EEPROM register bits
const EERD_START: u32 = 1 << 0; // Start read
const EERD_DONE: u32 = 1 << 4; // Read done

/// NIC statistics counters
#[derive(Debug, Clone, Default)]
pub struct E1000Stats {
    pub rx_packets: u64,
    pub tx_packets: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_errors: u64,
    pub tx_errors: u64,
    pub rx_dropped: u64,
    pub crc_errors: u64,
    pub alignment_errors: u64,
    pub missed_packets: u64,
    pub collisions: u64,
    pub rx_no_buffer: u64,
    pub rx_broadcast: u64,
    pub rx_multicast: u64,
    pub rx_undersize: u64,
    pub rx_oversize: u64,
}

/// e1000 driver instance
pub struct E1000 {
    mmio_base: usize,
    mac: MacAddr,
    tx_descs: usize,               // physical address of TX descriptor ring
    rx_descs: usize,               // physical address of RX descriptor ring
    rx_bufs: [usize; NUM_RX_DESC], // physical addresses of RX buffers
    tx_bufs: [usize; NUM_TX_DESC], // physical addresses of TX buffers
    tx_cur: core::sync::atomic::AtomicUsize,
    rx_cur: core::sync::atomic::AtomicUsize,
    link_up: bool,
    speed_mbps: u32,
    full_duplex: bool,
    stats: E1000Stats,
    interrupts_enabled: bool,
}

impl E1000 {
    /// Read an e1000 MMIO register.
    // hot path: polled in RX/TX descriptor checks and interrupt handling
    #[inline(always)]
    fn read_reg(&self, reg: u32) -> u32 {
        unsafe { core::ptr::read_volatile((self.mmio_base + reg as usize) as *const u32) }
    }

    /// Write an e1000 MMIO register.
    // hot path: called to ring the TX/RX doorbell on every packet send/receive batch
    #[inline(always)]
    fn write_reg(&self, reg: u32, val: u32) {
        unsafe {
            core::ptr::write_volatile((self.mmio_base + reg as usize) as *mut u32, val);
        }
    }

    /// Read a word from the EEPROM at the given offset.
    /// Returns 0xFFFF on timeout (no hang — bounded 100_000 iterations).
    fn eeprom_read(&self, addr: u8) -> u16 {
        self.write_reg(REG_EERD, EERD_START | ((addr as u32) << 8));
        for _ in 0..100_000 {
            let val = self.read_reg(REG_EERD);
            if val & EERD_DONE != 0 {
                return (val >> 16) as u16;
            }
            core::hint::spin_loop();
        }
        0xFFFF // timeout — return sentinel
    }

    /// Check if EEPROM is present (some e1000 variants don't have one)
    fn eeprom_present(&self) -> bool {
        self.write_reg(REG_EERD, EERD_START);
        for _ in 0..1000 {
            let val = self.read_reg(REG_EERD);
            if val & EERD_DONE != 0 {
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }

    /// Read MAC address from EEPROM
    fn read_mac_from_eeprom(&self) -> MacAddr {
        let mut mac = [0u8; 6];
        for i in 0..3 {
            let word = self.eeprom_read(i as u8);
            mac[i * 2] = (word & 0xFF) as u8;
            mac[i * 2 + 1] = (word >> 8) as u8;
        }
        MacAddr(mac)
    }

    /// Read MAC address from RAL/RAH registers (fallback when no EEPROM)
    fn read_mac_from_ral(&self) -> MacAddr {
        let low = self.read_reg(REG_RAL);
        let high = self.read_reg(REG_RAH);
        MacAddr([
            (low & 0xFF) as u8,
            ((low >> 8) & 0xFF) as u8,
            ((low >> 16) & 0xFF) as u8,
            ((low >> 24) & 0xFF) as u8,
            (high & 0xFF) as u8,
            ((high >> 8) & 0xFF) as u8,
        ])
    }

    /// Write MAC address to RAL/RAH registers
    fn write_mac_to_ral(&self, mac: &MacAddr) {
        let m = mac.0;
        let ral =
            (m[0] as u32) | ((m[1] as u32) << 8) | ((m[2] as u32) << 16) | ((m[3] as u32) << 24);
        let rah = (m[4] as u32) | ((m[5] as u32) << 8) | (1 << 31); // AV (Address Valid)
        self.write_reg(REG_RAL, ral);
        self.write_reg(REG_RAH, rah);
    }

    /// Clear all receive address filters except entry 0 (our MAC)
    fn clear_ra_table(&self) {
        // Entries 1-15 (each is RAL + RAH, 8 bytes apart)
        for i in 1..16u32 {
            self.write_reg(REG_RAL.saturating_add(i.saturating_mul(8)), 0);
            self.write_reg(REG_RAH.saturating_add(i.saturating_mul(8)), 0);
        }
    }

    /// Clear the multicast table array (128 entries of 32 bits = 4096 bits)
    fn clear_mta(&self) {
        for i in 0..128u32 {
            self.write_reg(REG_MTA.saturating_add(i.saturating_mul(4)), 0);
        }
    }

    /// Set a bit in the multicast table for a given MAC address
    pub fn add_multicast(&self, mac: &MacAddr) {
        // Hash function: bits 47:36 of the MAC address
        let m = mac.0;
        // Use bits [35:32] from byte 4 and bits [47:40] from byte 5
        let hash = (((m[5] as u32) << 4) | ((m[4] as u32) >> 4)) & 0xFFF;
        let reg_index = (hash >> 5) & 0x7F;
        let bit_index = hash & 0x1F;
        let mta_reg = REG_MTA.saturating_add(reg_index.saturating_mul(4));
        let old = self.read_reg(mta_reg);
        self.write_reg(mta_reg, old | (1 << bit_index));
    }

    /// Enable promiscuous mode (receive all packets)
    pub fn set_promiscuous(&self, enable: bool) {
        let mut rctl = self.read_reg(REG_RCTL);
        if enable {
            rctl |= RCTL_UPE | RCTL_MPE;
        } else {
            rctl &= !(RCTL_UPE | RCTL_MPE);
        }
        self.write_reg(REG_RCTL, rctl);
    }

    /// Configure the Transmit Inter-Packet Gap register
    fn setup_tipg(&self) {
        // Recommended values for IEEE 802.3 (from Intel datasheet)
        // IPGT = 10, IPGR1 = 8, IPGR2 = 6
        let tipg = 10 | (8 << 10) | (6 << 20);
        self.write_reg(REG_TIPG, tipg);
    }

    /// Initialize TX descriptor ring.
    /// Returns false and logs a message if frame allocation fails.
    fn init_tx(&mut self) -> bool {
        // Allocate enough frames to hold all TX descriptors
        // Each descriptor is 16 bytes, we need NUM_TX_DESC * 16 bytes
        let ring_size = NUM_TX_DESC * 16;
        let ring_frames = (ring_size + FRAME_SIZE - 1) / FRAME_SIZE;

        let first_frame = match frame_allocator::allocate_frame() {
            Some(f) => f,
            None => {
                serial_println!("  e1000: ERROR - failed to alloc TX ring frame");
                return false;
            }
        };
        unsafe {
            core::ptr::write_bytes(first_frame.addr as *mut u8, 0, FRAME_SIZE);
        }
        self.tx_descs = first_frame.addr;

        // Allocate additional frames if needed (must be physically contiguous)
        for _i in 1..ring_frames {
            match frame_allocator::allocate_frame() {
                Some(frame) => {
                    unsafe {
                        core::ptr::write_bytes(frame.addr as *mut u8, 0, FRAME_SIZE);
                    }
                    // These must be contiguous in physical memory (assumption: frame allocator
                    // gives sequential frames). In a real OS we'd use a contiguous allocator.
                }
                None => {
                    serial_println!("  e1000: ERROR - failed to alloc TX ring page");
                    return false;
                }
            }
        }

        // Allocate TX buffers (one frame per descriptor for simplicity)
        for i in 0..NUM_TX_DESC {
            match frame_allocator::allocate_frame() {
                Some(buf) => {
                    unsafe {
                        core::ptr::write_bytes(buf.addr as *mut u8, 0, FRAME_SIZE);
                    }
                    self.tx_bufs[i] = buf.addr;
                }
                None => {
                    serial_println!("  e1000: ERROR - failed to alloc TX buf[{}]", i);
                    return false;
                }
            }
        }

        // Initialize each descriptor
        for i in 0..NUM_TX_DESC {
            let desc = unsafe {
                &mut *((self.tx_descs.saturating_add(i.saturating_mul(16))) as *mut TxDesc)
            };
            desc.addr = self.tx_bufs[i] as u64;
            desc.cmd = 0;
            desc.status = 1; // DD bit set (descriptor done = available)
            desc.length = 0;
            desc.cso = 0;
            desc.css = 0;
            desc.special = 0;
        }

        self.write_reg(REG_TDBAL, self.tx_descs as u32);
        self.write_reg(REG_TDBAH, (self.tx_descs as u64 >> 32) as u32);
        self.write_reg(REG_TDLEN, (NUM_TX_DESC * 16) as u32);
        self.write_reg(REG_TDH, 0);
        self.write_reg(REG_TDT, 0);

        // Set Transmit Inter-Packet Gap
        self.setup_tipg();

        // Enable transmit
        self.write_reg(
            REG_TCTL,
            TCTL_EN | TCTL_PSP | TCTL_CT_IEEE | TCTL_COLD_FD | TCTL_RTLC,
        );

        // Set transmit interrupt delay
        self.write_reg(REG_TIDV, 0); // immediate
        self.write_reg(REG_TADV, 0);

        self.tx_cur.store(0, core::sync::atomic::Ordering::Relaxed);
        serial_println!("  e1000: TX ring initialized ({} descriptors)", NUM_TX_DESC);
        true
    }

    /// Initialize RX descriptor ring.
    /// Returns false and logs a message if frame allocation fails.
    fn init_rx(&mut self) -> bool {
        let ring_size = NUM_RX_DESC * 16;
        let ring_frames = (ring_size + FRAME_SIZE - 1) / FRAME_SIZE;

        let first_frame = match frame_allocator::allocate_frame() {
            Some(f) => f,
            None => {
                serial_println!("  e1000: ERROR - failed to alloc RX ring frame");
                return false;
            }
        };
        unsafe {
            core::ptr::write_bytes(first_frame.addr as *mut u8, 0, FRAME_SIZE);
        }
        self.rx_descs = first_frame.addr;

        for _i in 1..ring_frames {
            match frame_allocator::allocate_frame() {
                Some(frame) => unsafe {
                    core::ptr::write_bytes(frame.addr as *mut u8, 0, FRAME_SIZE);
                },
                None => {
                    serial_println!("  e1000: ERROR - failed to alloc RX ring page");
                    return false;
                }
            }
        }

        // Allocate RX buffers and initialize descriptors
        for i in 0..NUM_RX_DESC {
            let buf_frame = match frame_allocator::allocate_frame() {
                Some(f) => f,
                None => {
                    serial_println!("  e1000: ERROR - failed to alloc RX buf[{}]", i);
                    return false;
                }
            };
            unsafe {
                core::ptr::write_bytes(buf_frame.addr as *mut u8, 0, FRAME_SIZE);
            }
            self.rx_bufs[i] = buf_frame.addr;

            let desc = unsafe {
                &mut *((self.rx_descs.saturating_add(i.saturating_mul(16))) as *mut RxDesc)
            };
            desc.addr = buf_frame.addr as u64;
            desc.length = 0;
            desc.checksum = 0;
            desc.status = 0;
            desc.errors = 0;
            desc.special = 0;
        }

        self.write_reg(REG_RDBAL, self.rx_descs as u32);
        self.write_reg(REG_RDBAH, (self.rx_descs as u64 >> 32) as u32);
        self.write_reg(REG_RDLEN, (NUM_RX_DESC * 16) as u32);
        self.write_reg(REG_RDH, 0);
        self.write_reg(REG_RDT, (NUM_RX_DESC - 1) as u32);

        // Set receive delay timers (microseconds)
        self.write_reg(REG_RDTR, 0); // immediate
        self.write_reg(REG_RADV, 0);

        // Enable receive with appropriate settings
        self.write_reg(
            REG_RCTL,
            RCTL_EN | RCTL_BAM | RCTL_SECRC | RCTL_BSIZE_2048 | RCTL_LBM_NONE | RCTL_MO_36,
        );

        self.rx_cur.store(0, core::sync::atomic::Ordering::Relaxed);
        serial_println!(
            "  e1000: RX ring initialized ({} descriptors, {}B buffers)",
            NUM_RX_DESC,
            RX_BUF_SIZE
        );
        true
    }

    /// Enable interrupts
    fn enable_interrupts(&mut self) {
        // Clear pending interrupts
        self.read_reg(REG_ICR);

        // Enable relevant interrupt causes
        self.write_reg(
            REG_IMS,
            ICR_TXDW | ICR_LSC | ICR_RXO | ICR_RXT0 | ICR_RXDMT0 | ICR_RXSEQ,
        );

        // Set interrupt throttling rate (ITR) to limit interrupt frequency
        // Value is in 256ns units. 5000 = ~1.28ms between interrupts
        self.write_reg(REG_ITR, 5000);

        self.interrupts_enabled = true;
        serial_println!("  e1000: interrupts enabled");
    }

    /// Disable interrupts
    fn disable_interrupts(&mut self) {
        self.write_reg(REG_IMC, 0xFFFFFFFF);
        self.read_reg(REG_ICR); // clear pending
        self.interrupts_enabled = false;
    }

    /// Handle an interrupt from the e1000
    pub fn handle_interrupt(&mut self) {
        let cause = self.read_reg(REG_ICR);

        if cause == 0 {
            return; // spurious
        }

        if cause & ICR_LSC != 0 {
            // Link status change
            let status = self.read_reg(REG_STATUS);
            let new_link = status & (1 << 1) != 0;
            if new_link != self.link_up {
                self.link_up = new_link;
                self.update_link_info();
                serial_println!("  e1000: link {}", if self.link_up { "UP" } else { "DOWN" });
            }
        }

        if cause & ICR_RXT0 != 0 {
            // Receive timer expired — packets are ready in the RX ring.
            //
            // NAPI note: the caller (interrupt dispatch) should invoke
            // `e1000::poll_rx(callback)` after returning from this method
            // to batch-drain up to NAPI_BUDGET packets.  Doing it here
            // would require a callback pointer stored in the driver struct,
            // which adds coupling.  The clean pattern is:
            //
            //   let n = crate::drivers::e1000::poll_rx(net_rx_handler);
            //   if n == NAPI_BUDGET { schedule_softirq(SOFTIRQ_NET_RX); }
            //
            // This ensures we drain up to 64 packets per interrupt and
            // schedule another pass if the ring is still full.
            // hot path: fires at ~100K+/s under load — keep this branch lean
        }

        if cause & ICR_TXDW != 0 {
            // TX descriptor written back — transmission complete
            // Could wake up any threads waiting on TX completion
        }

        if cause & ICR_RXO != 0 {
            // RX overrun — we're not draining fast enough
            self.stats.rx_dropped = self.stats.rx_dropped.saturating_add(1);
            serial_println!("  e1000: RX overrun!");
        }

        if cause & ICR_RXDMT0 != 0 {
            // RX descriptor minimum threshold reached
            // Should replenish descriptors
        }
    }

    /// Read link speed and duplex from STATUS register
    fn update_link_info(&mut self) {
        let status = self.read_reg(REG_STATUS);
        self.full_duplex = (status & (1 << 0)) != 0;

        // Speed bits (6:7)
        let speed_bits = (status >> 6) & 0x3;
        self.speed_mbps = match speed_bits {
            0 => 10,
            1 => 100,
            2 | 3 => 1000,
            _ => 0,
        };
    }

    /// Get current link status
    pub fn link_status(&self) -> bool {
        self.read_reg(REG_STATUS) & (1 << 1) != 0
    }

    /// Read hardware statistics registers and accumulate into our counters
    pub fn read_stats(&mut self) -> &E1000Stats {
        // These registers auto-clear on read, so we accumulate
        self.stats.rx_packets = self
            .stats
            .rx_packets
            .saturating_add(self.read_reg(REG_GPRC) as u64);
        self.stats.tx_packets = self
            .stats
            .tx_packets
            .saturating_add(self.read_reg(REG_GPTC) as u64);

        let rx_lo = self.read_reg(REG_GORCL) as u64;
        let rx_hi = self.read_reg(REG_GORCH) as u64;
        self.stats.rx_bytes = self.stats.rx_bytes.saturating_add((rx_hi << 32) | rx_lo);

        let tx_lo = self.read_reg(REG_GOTCL) as u64;
        let tx_hi = self.read_reg(REG_GOTCH) as u64;
        self.stats.tx_bytes = self.stats.tx_bytes.saturating_add((tx_hi << 32) | tx_lo);

        self.stats.crc_errors = self
            .stats
            .crc_errors
            .saturating_add(self.read_reg(REG_CRCERRS) as u64);
        self.stats.alignment_errors = self
            .stats
            .alignment_errors
            .saturating_add(self.read_reg(REG_ALGNERRC) as u64);
        self.stats.rx_errors = self
            .stats
            .rx_errors
            .saturating_add(self.read_reg(REG_RXERRC) as u64);
        self.stats.missed_packets = self
            .stats
            .missed_packets
            .saturating_add(self.read_reg(REG_MPC) as u64);
        self.stats.collisions = self
            .stats
            .collisions
            .saturating_add(self.read_reg(REG_COLC) as u64);
        self.stats.rx_no_buffer = self
            .stats
            .rx_no_buffer
            .saturating_add(self.read_reg(REG_RNBC) as u64);
        self.stats.rx_broadcast = self
            .stats
            .rx_broadcast
            .saturating_add(self.read_reg(REG_BPRC) as u64);
        self.stats.rx_multicast = self
            .stats
            .rx_multicast
            .saturating_add(self.read_reg(REG_MPRC) as u64);
        self.stats.rx_undersize = self
            .stats
            .rx_undersize
            .saturating_add(self.read_reg(REG_RUC) as u64);
        self.stats.rx_oversize = self
            .stats
            .rx_oversize
            .saturating_add(self.read_reg(REG_ROC) as u64);

        &self.stats
    }

    /// Get speed as string
    pub fn speed_string(&self) -> &'static str {
        match self.speed_mbps {
            10 => "10 Mbps",
            100 => "100 Mbps",
            1000 => "1000 Mbps",
            _ => "Unknown",
        }
    }

    /// Software reset the device
    fn reset(&self) {
        // Disable interrupts first
        self.write_reg(REG_IMC, 0xFFFFFFFF);

        // Disable RX and TX
        self.write_reg(REG_RCTL, 0);
        self.write_reg(REG_TCTL, 0);

        // Issue device reset
        let ctrl = self.read_reg(REG_CTRL);
        self.write_reg(REG_CTRL, ctrl | CTRL_RST);

        // Wait for reset to complete (RST bit self-clears)
        for _ in 0..100_000 {
            if self.read_reg(REG_CTRL) & CTRL_RST == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // Post-reset delay
        for _ in 0..10000 {
            core::hint::spin_loop();
        }

        // Clear interrupt mask again after reset
        self.write_reg(REG_IMC, 0xFFFFFFFF);
        // Read ICR to clear any pending interrupts
        self.read_reg(REG_ICR);
    }

    /// Check how many RX descriptors have data ready
    pub fn rx_pending(&self) -> usize {
        let mut count = 0;
        let cur = self.rx_cur.load(core::sync::atomic::Ordering::Relaxed);
        for i in 0..NUM_RX_DESC {
            let idx = (cur + i) % NUM_RX_DESC;
            let desc = unsafe {
                &*((self.rx_descs.saturating_add(idx.saturating_mul(16))) as *const RxDesc)
            };
            if desc.status & RX_STATUS_DD != 0 {
                count += 1;
            } else {
                break;
            }
        }
        count
    }

    /// Check how many TX descriptors are available for sending
    pub fn tx_available(&self) -> usize {
        let mut count = 0;
        let cur = self.tx_cur.load(core::sync::atomic::Ordering::Relaxed);
        for i in 0..NUM_TX_DESC {
            let idx = (cur + i) % NUM_TX_DESC;
            let desc = unsafe {
                &*((self.tx_descs.saturating_add(idx.saturating_mul(16))) as *const TxDesc)
            };
            if desc.status & 1 != 0 {
                // DD bit
                count += 1;
            } else {
                break;
            }
        }
        count
    }

    /// Disable flow control (simplifies operation)
    fn disable_flow_control(&self) {
        self.write_reg(REG_FCAL, 0);
        self.write_reg(REG_FCAH, 0);
        self.write_reg(REG_FCT, 0);
        self.write_reg(REG_FCTTV, 0);
        self.write_reg(REG_FCRTL, 0);
        self.write_reg(REG_FCRTH, 0);
    }

    /// Receive multiple packets in a batch (drains up to `max` ready descriptors).
    /// Returns a Vec of (packet data, length) pairs. This is more efficient than
    /// calling recv() in a loop because we only bump the tail pointer once at the end.
    pub fn recv_batch(&self, buf_pool: &mut [&mut [u8]], max: usize) -> usize {
        let mut count = 0;
        let mut idx = self.rx_cur.load(core::sync::atomic::Ordering::Relaxed);
        let mut last_returned = idx;

        while count < max && count < buf_pool.len() {
            let desc = unsafe {
                &*((self.rx_descs.saturating_add(idx.saturating_mul(16))) as *const RxDesc)
            };

            if desc.status & RX_STATUS_DD == 0 {
                break; // no more ready descriptors
            }

            // Skip error packets
            if desc.errors & (RX_ERR_CE | RX_ERR_SEQ | RX_ERR_RXE) != 0 {
                let desc_mut = unsafe {
                    &mut *((self.rx_descs.saturating_add(idx.saturating_mul(16))) as *mut RxDesc)
                };
                desc_mut.status = 0;
                desc_mut.errors = 0;
                last_returned = idx;
                idx = (idx + 1) % NUM_RX_DESC;
                continue;
            }

            // Skip non-EOP (multi-descriptor) frames
            if desc.status & RX_STATUS_EOP == 0 {
                let desc_mut = unsafe {
                    &mut *((self.rx_descs.saturating_add(idx.saturating_mul(16))) as *mut RxDesc)
                };
                desc_mut.status = 0;
                last_returned = idx;
                idx = (idx + 1) % NUM_RX_DESC;
                continue;
            }

            let len = desc.length as usize;
            if buf_pool[count].len() >= len {
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        self.rx_bufs[idx] as *const u8,
                        buf_pool[count].as_mut_ptr(),
                        len,
                    );
                }
                // Store actual length in first 2 bytes is not ideal; caller tracks via return
                count += 1;
            }

            // Reset descriptor
            let desc_mut = unsafe {
                &mut *((self.rx_descs.saturating_add(idx.saturating_mul(16))) as *mut RxDesc)
            };
            desc_mut.status = 0;
            desc_mut.errors = 0;
            desc_mut.length = 0;
            last_returned = idx;
            idx = (idx + 1) % NUM_RX_DESC;
        }

        if count > 0 {
            self.rx_cur
                .store(idx, core::sync::atomic::Ordering::Relaxed);
            // Return all consumed descriptors to hardware by bumping tail
            self.write_reg(REG_RDT, last_returned as u32);
        }

        count
    }

    /// Read a PHY register via the MDI (Management Data Interface).
    /// The e1000 provides MDIC register at offset 0x0020 for PHY access.
    pub fn phy_read(&self, phy_addr: u8, reg_addr: u8) -> Option<u16> {
        const REG_MDIC: u32 = 0x0020;
        const MDIC_OP_READ: u32 = 0x0800_0000; // bits 27:26 = 10 (read)
        const MDIC_READY: u32 = 0x1000_0000; // bit 28
        const MDIC_ERROR: u32 = 0x4000_0000; // bit 30

        let mdic = ((reg_addr as u32) << 16) | ((phy_addr as u32) << 21) | MDIC_OP_READ;
        self.write_reg(REG_MDIC, mdic);

        // Poll for completion
        for _ in 0..100_000 {
            let val = self.read_reg(REG_MDIC);
            if val & MDIC_READY != 0 {
                if val & MDIC_ERROR != 0 {
                    return None;
                }
                return Some((val & 0xFFFF) as u16);
            }
            core::hint::spin_loop();
        }
        None
    }

    /// Write a PHY register via the MDI interface.
    pub fn phy_write(&self, phy_addr: u8, reg_addr: u8, data: u16) -> bool {
        const REG_MDIC: u32 = 0x0020;
        const MDIC_OP_WRITE: u32 = 0x0400_0000; // bits 27:26 = 01 (write)
        const MDIC_READY: u32 = 0x1000_0000;

        let mdic =
            (data as u32) | ((reg_addr as u32) << 16) | ((phy_addr as u32) << 21) | MDIC_OP_WRITE;
        self.write_reg(REG_MDIC, mdic);

        for _ in 0..100_000 {
            let val = self.read_reg(REG_MDIC);
            if val & MDIC_READY != 0 {
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }

    /// Enable or disable VLAN tag stripping on receive
    pub fn set_vlan_strip(&self, enable: bool) {
        let mut ctrl = self.read_reg(REG_CTRL);
        if enable {
            ctrl |= CTRL_VME;
        } else {
            ctrl &= !CTRL_VME;
        }
        self.write_reg(REG_CTRL, ctrl);
    }

    /// Enable or disable long packet reception (jumbo frames up to 16KB)
    pub fn set_long_packet_enable(&self, enable: bool) {
        let mut rctl = self.read_reg(REG_RCTL);
        if enable {
            rctl |= RCTL_LPE;
        } else {
            rctl &= !RCTL_LPE;
        }
        self.write_reg(REG_RCTL, rctl);
    }

    /// Control the e1000 LED registers for link/activity indication.
    /// LED0 (link), LED1 (activity), LED2/LED3 (configurable).
    /// Each LED has a 4-bit mode field in the LED Control register (0x0E00).
    pub fn set_led_mode(&self, led_index: u8, mode: u8) {
        const REG_LEDCTL: u32 = 0x0E00;
        let shift = (led_index & 0x03) * 8;
        let mask = !(0xFFu32 << shift);
        let old = self.read_reg(REG_LEDCTL);
        let new = (old & mask) | ((mode as u32 & 0xFF) << shift);
        self.write_reg(REG_LEDCTL, new);
    }

    /// Blink all LEDs for identification (useful for multi-NIC setups)
    pub fn blink_leds(&self) {
        // Mode 0x0E = blink for each LED slot
        const REG_LEDCTL: u32 = 0x0E00;
        let old = self.read_reg(REG_LEDCTL);
        self.write_reg(REG_LEDCTL, 0x0E0E_0E0E);
        // Spin-wait roughly 2 seconds (2M iterations at ~1us each)
        for _ in 0..2_000_000 {
            core::hint::spin_loop();
        }
        self.write_reg(REG_LEDCTL, old);
    }

    /// Get detailed link diagnostics from PHY registers
    pub fn link_diagnostics(&self) -> (bool, u32, bool, bool) {
        // PHY register 0: Control, register 1: Status
        let phy_status = self.phy_read(1, 1).unwrap_or(0);
        let link = phy_status & (1 << 2) != 0;
        let autoneg_complete = phy_status & (1 << 5) != 0;

        // PHY register 10: Gigabit status (1000BASE-T)
        let gig_status = self.phy_read(1, 10).unwrap_or(0);
        let _partner_1000 = gig_status & (1 << 11) != 0;

        let status = self.read_reg(REG_STATUS);
        let full_duplex = status & 1 != 0;
        let speed_bits = (status >> 6) & 3;
        let speed = match speed_bits {
            0 => 10,
            1 => 100,
            2 | 3 => 1000,
            _ => 0,
        };

        (link, speed, full_duplex, autoneg_complete)
    }

    /// Reset the hardware statistics counters by reading them all (auto-clear on read)
    pub fn reset_stats(&mut self) {
        let _ = self.read_reg(REG_GPRC);
        let _ = self.read_reg(REG_GPTC);
        let _ = self.read_reg(REG_GORCL);
        let _ = self.read_reg(REG_GORCH);
        let _ = self.read_reg(REG_GOTCL);
        let _ = self.read_reg(REG_GOTCH);
        let _ = self.read_reg(REG_CRCERRS);
        let _ = self.read_reg(REG_ALGNERRC);
        let _ = self.read_reg(REG_RXERRC);
        let _ = self.read_reg(REG_MPC);
        let _ = self.read_reg(REG_COLC);
        let _ = self.read_reg(REG_RNBC);
        let _ = self.read_reg(REG_BPRC);
        let _ = self.read_reg(REG_MPRC);
        let _ = self.read_reg(REG_RUC);
        let _ = self.read_reg(REG_ROC);
        let _ = self.read_reg(REG_TPR);
        let _ = self.read_reg(REG_TPT);
        self.stats = E1000Stats::default();
    }

    /// Configure Receive Side Coalescing (interrupt delay timers).
    /// `rx_delay`: RX interrupt delay in microseconds (0 = immediate).
    /// `rx_abs_delay`: Absolute maximum delay before forcing an interrupt.
    pub fn set_rx_coalescing(&self, rx_delay_us: u32, rx_abs_delay_us: u32) {
        self.write_reg(REG_RDTR, rx_delay_us);
        self.write_reg(REG_RADV, rx_abs_delay_us);
    }

    /// Configure TX interrupt coalescing.
    pub fn set_tx_coalescing(&self, tx_delay_us: u32, tx_abs_delay_us: u32) {
        self.write_reg(REG_TIDV, tx_delay_us);
        self.write_reg(REG_TADV, tx_abs_delay_us);
    }

    /// Set the interrupt throttle rate (ITR). Value is in 256-nanosecond units.
    /// Lower values = more interrupts = lower latency but higher CPU usage.
    /// 0 = disable throttling, 5000 = ~1.28ms between interrupts (default).
    pub fn set_interrupt_throttle(&self, itr_value: u32) {
        self.write_reg(REG_ITR, itr_value);
    }
}

impl NetworkDriver for E1000 {
    fn send(&self, frame: &[u8]) -> Result<(), NetError> {
        if frame.len() > MAX_FRAME_SIZE {
            return Err(NetError::InvalidPacket);
        }

        let idx = self.tx_cur.load(core::sync::atomic::Ordering::Relaxed);
        let desc = unsafe {
            &mut *((self.tx_descs.saturating_add(idx.saturating_mul(16))) as *mut TxDesc)
        };

        // Wait for descriptor to be available (DD bit set)
        let mut timeout = 100_000u32;
        while desc.status & 1 == 0 {
            timeout = timeout.saturating_sub(1);
            if timeout == 0 {
                return Err(NetError::IoError);
            }
            core::hint::spin_loop();
        }

        // Copy frame data into the pre-allocated TX buffer
        let buf_addr = self.tx_bufs[idx];
        let len = frame.len();
        unsafe {
            core::ptr::copy_nonoverlapping(frame.as_ptr(), buf_addr as *mut u8, len);
        }

        // Set up descriptor
        desc.addr = buf_addr as u64;
        desc.length = len as u16;
        desc.cmd = TX_CMD_EOP | TX_CMD_IFCS | TX_CMD_RS;
        desc.status = 0;
        desc.cso = 0;
        desc.css = 0;
        desc.special = 0;

        // Advance and bump tail pointer
        let next = (idx + 1) % NUM_TX_DESC;
        self.tx_cur
            .store(next, core::sync::atomic::Ordering::Relaxed);
        // Fence: ensure descriptor write is visible to hardware before ringing doorbell
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.write_reg(REG_TDT, next as u32);

        Ok(())
    }

    fn recv(&self, buf: &mut [u8]) -> Result<usize, NetError> {
        let idx = self.rx_cur.load(core::sync::atomic::Ordering::Relaxed);
        let desc =
            unsafe { &*((self.rx_descs.saturating_add(idx.saturating_mul(16))) as *const RxDesc) };

        // Check if descriptor has data
        if desc.status & RX_STATUS_DD == 0 {
            return Err(NetError::Timeout);
        }

        // Check for errors
        if desc.errors & (RX_ERR_CE | RX_ERR_SEQ | RX_ERR_RXE) != 0 {
            // Reset this descriptor and advance
            let desc_mut = unsafe {
                &mut *((self.rx_descs.saturating_add(idx.saturating_mul(16))) as *mut RxDesc)
            };
            desc_mut.status = 0;
            desc_mut.errors = 0;
            let next = (idx + 1) % NUM_RX_DESC;
            self.rx_cur
                .store(next, core::sync::atomic::Ordering::Relaxed);
            self.write_reg(REG_RDT, idx as u32);
            return Err(NetError::InvalidPacket);
        }

        // Verify this is a complete packet (EOP set)
        if desc.status & RX_STATUS_EOP == 0 {
            // Multi-descriptor packet not supported yet — drop it
            let desc_mut = unsafe {
                &mut *((self.rx_descs.saturating_add(idx.saturating_mul(16))) as *mut RxDesc)
            };
            desc_mut.status = 0;
            let next = (idx + 1) % NUM_RX_DESC;
            self.rx_cur
                .store(next, core::sync::atomic::Ordering::Relaxed);
            self.write_reg(REG_RDT, idx as u32);
            return Err(NetError::InvalidPacket);
        }

        let len = desc.length as usize;
        if buf.len() < len {
            return Err(NetError::BufferTooSmall);
        }

        // Copy data from RX buffer
        unsafe {
            core::ptr::copy_nonoverlapping(self.rx_bufs[idx] as *const u8, buf.as_mut_ptr(), len);
        }

        // Reset descriptor and advance
        let desc_mut = unsafe {
            &mut *((self.rx_descs.saturating_add(idx.saturating_mul(16))) as *mut RxDesc)
        };
        desc_mut.status = 0;
        desc_mut.errors = 0;
        desc_mut.length = 0;

        let next = (idx + 1) % NUM_RX_DESC;
        self.rx_cur
            .store(next, core::sync::atomic::Ordering::Relaxed);
        self.write_reg(REG_RDT, idx as u32);

        Ok(len)
    }

    fn mac_addr(&self) -> MacAddr {
        self.mac
    }
}

/// Global e1000 driver instance
static E1000_DRIVER: Mutex<Option<E1000>> = Mutex::new(None);

/// Get a reference to the e1000 driver
pub fn driver() -> &'static Mutex<Option<E1000>> {
    &E1000_DRIVER
}

/// Handle e1000 interrupt (called from interrupt handler)
pub fn handle_interrupt() {
    if let Some(ref mut nic) = *E1000_DRIVER.lock() {
        nic.handle_interrupt();
    }
}

/// Read current statistics
pub fn get_stats() -> Option<E1000Stats> {
    E1000_DRIVER
        .lock()
        .as_mut()
        .map(|nic| nic.read_stats().clone())
}

/// Check link status
pub fn is_link_up() -> bool {
    E1000_DRIVER
        .lock()
        .as_ref()
        .map(|nic| nic.link_status())
        .unwrap_or(false)
}

/// Get MAC address
pub fn get_mac() -> Option<MacAddr> {
    E1000_DRIVER.lock().as_ref().map(|nic| nic.mac)
}

/// Get the number of pending received packets
pub fn rx_pending() -> usize {
    E1000_DRIVER
        .lock()
        .as_ref()
        .map(|nic| nic.rx_pending())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// NAPI-style batch RX poll
// ---------------------------------------------------------------------------

/// Maximum packets processed per polling call.
/// Processing 64 packets per interrupt avoids re-arming the IRQ for every
/// single frame at high packet rates (~GigE can deliver >1 M pkt/s).
const NAPI_BUDGET: usize = 64;

/// Packet receive callback type.
/// Called once per packet during `poll_rx`.  The slice contains the raw
/// Ethernet frame (no FCS).  The callback should return quickly and
/// not call back into the driver.
pub type RxCallback = fn(frame: &[u8]);

/// NAPI-style polling receive — drain up to NAPI_BUDGET packets from the
/// hardware RX ring in one shot and deliver each to `callback`.
///
/// Call this:
///   1. From the `ICR_RXT0` branch of the e1000 interrupt handler, or
///   2. From a dedicated network softirq/workqueue, or
///   3. From the idle loop for polling-mode operation.
///
/// Returns the number of packets processed.  If `returned == NAPI_BUDGET`
/// the ring may have more packets; schedule another call.
///
/// Compared to calling `recv()` in a loop this avoids writing `REG_RDT`
/// for every single descriptor — instead the tail register is written once
/// per batch, which cuts MMIO traffic by up to 64x.
// hot path: called from e1000 IRQ handler — ~thousands of times/second at GigE
pub fn poll_rx(callback: RxCallback) -> usize {
    const MAX_BUF: usize = 2048; // RX_BUF_SIZE from driver init
    let guard = E1000_DRIVER.lock();
    let nic = match guard.as_ref() {
        Some(n) => n,
        None => return 0,
    };

    let mut processed = 0usize;
    let mut idx = nic.rx_cur.load(core::sync::atomic::Ordering::Relaxed);
    let mut last_idx = idx;

    while processed < NAPI_BUDGET {
        let desc_ptr = nic.rx_descs.saturating_add(idx.saturating_mul(16)) as *const RxDesc;
        let desc = unsafe { &*desc_ptr };

        // No more ready descriptors.
        if desc.status & RX_STATUS_DD == 0 {
            break;
        }

        // Error or non-EOP frame: reset and skip.
        if desc.errors & (RX_ERR_CE | RX_ERR_SEQ | RX_ERR_RXE) != 0
            || desc.status & RX_STATUS_EOP == 0
        {
            let desc_mut = unsafe {
                &mut *(nic.rx_descs.saturating_add(idx.saturating_mul(16)) as *mut RxDesc)
            };
            desc_mut.status = 0;
            desc_mut.errors = 0;
            last_idx = idx;
            idx = (idx + 1) % NUM_RX_DESC;
            continue;
        }

        let len = desc.length as usize;
        if len > 0 && len <= MAX_BUF {
            // Build a slice over the DMA buffer and invoke the callback
            // without copying — zero-copy hot path.
            let frame = unsafe { core::slice::from_raw_parts(nic.rx_bufs[idx] as *const u8, len) };
            callback(frame);
        }

        // Reset descriptor for reuse.
        let desc_mut =
            unsafe { &mut *(nic.rx_descs.saturating_add(idx.saturating_mul(16)) as *mut RxDesc) };
        desc_mut.status = 0;
        desc_mut.errors = 0;
        desc_mut.length = 0;

        last_idx = idx;
        idx = (idx + 1) % NUM_RX_DESC;
        processed += 1;
    }

    if processed > 0 {
        // Commit: advance the software head and write REG_RDT once for the
        // entire batch.  Single MMIO write vs. one-per-packet in the old path.
        nic.rx_cur.store(idx, core::sync::atomic::Ordering::Relaxed);
        nic.write_reg(REG_RDT, last_idx as u32);
    }

    processed
}

/// Initialize the e1000 driver
pub fn init() -> bool {
    // Find the e1000 PCI device (try multiple device IDs)
    let e1000_ids: [(u16, u16); 4] = [
        (0x8086, 0x100E), // 82540EM (QEMU default)
        (0x8086, 0x100F), // 82545EM
        (0x8086, 0x10D3), // 82574L
        (0x8086, 0x153A), // I217-LM
    ];

    let mut found_dev = None;
    for (vendor, device) in &e1000_ids {
        let devices = pci::find_by_id(*vendor, *device);
        if let Some(d) = devices.first() {
            found_dev = Some(d.clone());
            break;
        }
    }

    let dev = match found_dev {
        Some(d) => d,
        None => {
            serial_println!("  e1000: device not found");
            return false;
        }
    };

    serial_println!(
        "  e1000: found {:04x}:{:04x} at PCI {}",
        dev.vendor_id,
        dev.device_id,
        dev.bdf_string()
    );

    // Enable bus mastering and memory space (required for DMA)
    pci::enable_bus_master(dev.bus, dev.device, dev.function);
    pci::enable_memory_space(dev.bus, dev.device, dev.function);

    // Prefer MSI over legacy pin-based interrupts.
    // e1000 vector 0x31 (IDT entry 49) — adjust to match your IDT allocation.
    const E1000_IRQ_VECTOR: u8 = 0x31;
    if !crate::drivers::pci_msi::try_upgrade_to_msi(
        dev.bus,
        dev.device,
        dev.function,
        0, // apic_id 0 = bootstrap CPU
        E1000_IRQ_VECTOR,
        "e1000",
    ) {
        serial_println!(
            "  e1000: MSI not available, using legacy IRQ {}",
            dev.interrupt_line
        );
    }

    // Read BAR0 (MMIO base address)
    let (bar0, is_mmio) = pci::read_bar(dev.bus, dev.device, dev.function, 0);
    if !is_mmio || bar0 == 0 {
        serial_println!("  e1000: invalid BAR0");
        return false;
    }

    let mmio_base = bar0 as usize;
    serial_println!("  e1000: MMIO base at {:#x}", mmio_base);

    // Map MMIO region (128KB is typical for e1000)
    let mmio_size = 128 * 1024;
    let pages = mmio_size / 0x1000;
    for i in 0usize..pages {
        let page = mmio_base.saturating_add(i.saturating_mul(0x1000));
        let flags = paging::flags::WRITABLE | paging::flags::NO_CACHE;
        let _ = paging::map_page(page, page, flags);
    }

    let mut nic = E1000 {
        mmio_base,
        mac: MacAddr::ZERO,
        tx_descs: 0,
        rx_descs: 0,
        rx_bufs: [0; NUM_RX_DESC],
        tx_bufs: [0; NUM_TX_DESC],
        tx_cur: core::sync::atomic::AtomicUsize::new(0),
        rx_cur: core::sync::atomic::AtomicUsize::new(0),
        link_up: false,
        speed_mbps: 0,
        full_duplex: false,
        stats: E1000Stats::default(),
        interrupts_enabled: false,
    };

    // Reset the device
    nic.reset();

    // Set link up, auto-speed detection, full-duplex
    let ctrl = nic.read_reg(REG_CTRL);
    nic.write_reg(REG_CTRL, ctrl | CTRL_SLU | CTRL_ASDE | CTRL_FD);

    // Disable flow control
    nic.disable_flow_control();

    // Clear multicast table
    nic.clear_mta();

    // Clear receive address table
    nic.clear_ra_table();

    // Read MAC address
    if nic.eeprom_present() {
        nic.mac = nic.read_mac_from_eeprom();
        serial_println!("  e1000: MAC from EEPROM: {}", nic.mac);
    } else {
        nic.mac = nic.read_mac_from_ral();
        serial_println!("  e1000: MAC from RAL/RAH: {}", nic.mac);
    }

    if nic.mac == MacAddr::ZERO {
        serial_println!("  e1000: WARNING - MAC address is all zeros");
    }

    // Write MAC to receive address register 0
    nic.write_mac_to_ral(&nic.mac);

    // Initialize TX and RX rings
    if !nic.init_tx() {
        serial_println!("  e1000: TX ring init failed, aborting");
        return false;
    }
    if !nic.init_rx() {
        serial_println!("  e1000: RX ring init failed, aborting");
        return false;
    }

    // Enable interrupts
    nic.enable_interrupts();

    // Check link status
    nic.link_up = nic.link_status();
    nic.update_link_info();
    serial_println!(
        "  e1000: link {} ({}, {})",
        if nic.link_up { "UP" } else { "DOWN" },
        nic.speed_string(),
        if nic.full_duplex {
            "full-duplex"
        } else {
            "half-duplex"
        }
    );

    // Register with network stack
    let mac_addr = nic.mac;
    *E1000_DRIVER.lock() = Some(nic);

    // Configure the network interface
    crate::net::configure_interface(
        "eth0",
        mac_addr,
        crate::net::Ipv4Addr::new(10, 0, 2, 15), // QEMU default
        crate::net::Ipv4Addr::new(255, 255, 255, 0),
        crate::net::Ipv4Addr::new(10, 0, 2, 2), // QEMU default gateway
    );

    true
}
