/// Virtio device emulation for guest VMs.
///
/// Part of the AIOS hypervisor subsystem.
///
/// Presents virtio devices to the guest over PCI or MMIO transport.
/// Each device has a type, configuration space, feature bits, and
/// one or more virtqueues for data transfer.

use alloc::vec::Vec;
use crate::{serial_print, serial_println};
use crate::sync::Mutex;

/// Global virtio device registry.
static VIRTIO_REGISTRY: Mutex<Option<VirtioRegistry>> = Mutex::new(None);

/// Maximum number of virtio devices across all guests.
const MAX_VIRTIO_DEVICES: usize = 64;

/// Maximum virtqueues per device.
const MAX_QUEUES_PER_DEVICE: usize = 4;

/// Configuration space size per device (bytes).
const CONFIG_SPACE_SIZE: usize = 64;

/// Virtio device status bits (virtio spec 2.1).
const VIRTIO_STATUS_ACKNOWLEDGE: u32 = 1;
const VIRTIO_STATUS_DRIVER: u32 = 2;
const VIRTIO_STATUS_DRIVER_OK: u32 = 4;
const VIRTIO_STATUS_FEATURES_OK: u32 = 8;
const VIRTIO_STATUS_DEVICE_NEEDS_RESET: u32 = 64;
const VIRTIO_STATUS_FAILED: u32 = 128;

/// Supported virtio device types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtioDeviceType {
    Net       = 1,
    Block     = 2,
    Console   = 3,
    Rng       = 4,
    Balloon   = 5,
    Scsi      = 8,
    Gpu       = 16,
    Input     = 18,
    Crypto    = 20,
    Socket    = 19,
    Fs        = 26,
}

impl VirtioDeviceType {
    /// Get the virtio device ID for PCI subsystem identification.
    fn device_id(self) -> u32 {
        self as u32
    }

    /// Get the default number of virtqueues for this device type.
    fn default_queue_count(self) -> usize {
        match self {
            VirtioDeviceType::Net => 3,       // rx, tx, ctrl
            VirtioDeviceType::Block => 1,     // request queue
            VirtioDeviceType::Console => 2,   // rx, tx
            VirtioDeviceType::Rng => 1,       // request queue
            VirtioDeviceType::Balloon => 2,   // inflate, deflate
            VirtioDeviceType::Scsi => 3,      // ctrl, event, request
            VirtioDeviceType::Gpu => 2,       // ctrl, cursor
            VirtioDeviceType::Input => 2,     // event, status
            VirtioDeviceType::Crypto => 2,    // data, control
            VirtioDeviceType::Socket => 2,    // rx, tx
            VirtioDeviceType::Fs => 1,        // request queue
        }
    }

    /// Get the default feature bits offered to the guest.
    fn default_features(self) -> u64 {
        // Common virtio feature flags (bits 24-37).
        let common: u64 = (1 << 32) // VIRTIO_F_VERSION_1
            | (1 << 33); // VIRTIO_F_ACCESS_PLATFORM

        match self {
            VirtioDeviceType::Net => {
                common
                    | (1 << 0)   // VIRTIO_NET_F_CSUM
                    | (1 << 1)   // VIRTIO_NET_F_GUEST_CSUM
                    | (1 << 5)   // VIRTIO_NET_F_MAC
                    | (1 << 16)  // VIRTIO_NET_F_STATUS
            }
            VirtioDeviceType::Block => {
                common
                    | (1 << 1)   // VIRTIO_BLK_F_SIZE_MAX
                    | (1 << 2)   // VIRTIO_BLK_F_SEG_MAX
                    | (1 << 6)   // VIRTIO_BLK_F_BLK_SIZE
            }
            VirtioDeviceType::Rng => common,
            _ => common,
        }
    }
}

/// Registry tracking all virtio devices.
struct VirtioRegistry {
    device_count: usize,
    next_device_id: u64,
}

/// Per-queue state inside a virtio device.
struct DeviceQueue {
    /// Queue size (number of descriptors, must be power of 2).
    size: u16,
    /// Whether the queue has been activated by the guest driver.
    ready: bool,
    /// MMIO/PCI bar offset for queue notification.
    notify_offset: u32,
}

impl DeviceQueue {
    fn new(size: u16) -> Self {
        DeviceQueue {
            size,
            ready: false,
            notify_offset: 0,
        }
    }
}

/// A virtio device exposed to a guest VM.
pub struct VirtioDevice {
    /// Device type.
    dev_type: VirtioDeviceType,
    /// Unique device instance ID.
    device_id: u64,
    /// Virtqueues for this device.
    queues: Vec<DeviceQueue>,
    /// Device configuration space (device-specific).
    config_space: [u8; CONFIG_SPACE_SIZE],
    /// Feature bits offered by the host.
    offered_features: u64,
    /// Feature bits accepted by the guest.
    accepted_features: u64,
    /// Device status register (set by guest driver).
    status: u32,
    /// ISR (Interrupt Status Register) bits.
    isr_status: u32,
    /// Currently selected queue index for MMIO config access.
    queue_select: u16,
    /// Guest ID this device is assigned to.
    guest_id: u64,
}

impl VirtioDevice {
    pub fn new(dev_type: VirtioDeviceType) -> Self {
        let device_id = {
            let mut reg = VIRTIO_REGISTRY.lock();
            if let Some(ref mut r) = *reg {
                let id = r.next_device_id;
                r.next_device_id = r.next_device_id.saturating_add(1);
                r.device_count = r.device_count.saturating_add(1);
                id
            } else {
                0
            }
        };

        let queue_count = dev_type.default_queue_count();
        let mut queues = Vec::new();
        for _ in 0..queue_count {
            queues.push(DeviceQueue::new(256));
        }

        let mut config_space = [0u8; CONFIG_SPACE_SIZE];

        // Initialize device-specific config space defaults.
        match dev_type {
            VirtioDeviceType::Net => {
                // Default MAC address: 52:54:00:12:34:56
                config_space[0] = 0x52;
                config_space[1] = 0x54;
                config_space[2] = 0x00;
                config_space[3] = 0x12;
                config_space[4] = 0x34;
                config_space[5] = 0x56;
                // Status: link up (offset 6, u16).
                config_space[6] = 0x01;
                config_space[7] = 0x00;
            }
            VirtioDeviceType::Block => {
                // Capacity: 1 GiB in 512-byte sectors = 2097152 sectors.
                // Stored as u64 at offset 0.
                let capacity: u64 = 2097152;
                let bytes = capacity.to_le_bytes();
                config_space[0..8].copy_from_slice(&bytes);
                // Block size: 512 at offset 20.
                let blk_size: u32 = 512;
                config_space[20..24].copy_from_slice(&blk_size.to_le_bytes());
            }
            VirtioDeviceType::Console => {
                // cols = 80 at offset 0.
                config_space[0..2].copy_from_slice(&80u16.to_le_bytes());
                // rows = 25 at offset 2.
                config_space[2..4].copy_from_slice(&25u16.to_le_bytes());
            }
            _ => {}
        }

        let offered_features = dev_type.default_features();

        serial_println!(
            "    [virtio_host] Created {:?} device (id={}, queues={})",
            dev_type, device_id, queue_count
        );

        VirtioDevice {
            dev_type,
            device_id,
            queues,
            config_space,
            offered_features,
            accepted_features: 0,
            status: 0,
            isr_status: 0,
            queue_select: 0,
            guest_id: 0,
        }
    }

    /// Assign this device to a guest VM.
    pub fn assign_to_guest(&mut self, guest_id: u64) {
        self.guest_id = guest_id;
        serial_println!(
            "    [virtio_host] {:?} device {} assigned to guest {}",
            self.dev_type, self.device_id, guest_id
        );
    }

    /// Handle a device register read from the guest (MMIO transport).
    pub fn mmio_read(&self, offset: u64) -> u32 {
        match offset {
            0x000 => 0x74726976, // MagicValue: "virt" in little-endian.
            0x004 => 2,           // Version: virtio-mmio v2.
            0x008 => self.dev_type.device_id(),
            0x00C => 0x554D4551, // VendorID: "QEMU" (convention).
            0x010 => self.offered_features as u32,       // DeviceFeatures (low).
            0x014 => (self.offered_features >> 32) as u32, // DeviceFeatures (high), selected via FeaturesSel.
            0x034 => {
                // QueueNumMax: max queue size for the selected queue.
                if (self.queue_select as usize) < self.queues.len() {
                    self.queues[self.queue_select as usize].size as u32
                } else {
                    0
                }
            }
            0x044 => {
                // QueueReady
                if (self.queue_select as usize) < self.queues.len() {
                    if self.queues[self.queue_select as usize].ready { 1 } else { 0 }
                } else {
                    0
                }
            }
            0x060 => self.isr_status,           // InterruptStatus
            0x070 => self.status,               // Status
            0x0FC => 0x2,                        // ConfigGeneration
            _ => {
                // Configuration space reads (offset >= 0x100).
                if offset >= 0x100 && offset < 0x100 + CONFIG_SPACE_SIZE as u64 {
                    let idx = (offset - 0x100) as usize;
                    if idx + 3 < CONFIG_SPACE_SIZE {
                        u32::from_le_bytes([
                            self.config_space[idx],
                            self.config_space[idx + 1],
                            self.config_space[idx + 2],
                            self.config_space[idx + 3],
                        ])
                    } else {
                        0
                    }
                } else {
                    0
                }
            }
        }
    }

    /// Handle a device register write from the guest (MMIO transport).
    pub fn mmio_write(&mut self, offset: u64, value: u32) {
        match offset {
            0x020 => {
                // DeviceFeaturesSel: select feature bits page.
                // (guest writes which 32-bit page of features to read)
            }
            0x024 => {
                // DriverFeatures (low 32 bits).
                self.accepted_features = (self.accepted_features & 0xFFFF_FFFF_0000_0000)
                    | (value as u64);
            }
            0x028 => {
                // DriverFeatures (high 32 bits).
                self.accepted_features = (self.accepted_features & 0x0000_0000_FFFF_FFFF)
                    | ((value as u64) << 32);
            }
            0x030 => {
                // QueueSel: select which queue subsequent queue operations target.
                self.queue_select = value as u16;
            }
            0x038 => {
                // QueueNum: set the size of the selected queue.
                if (self.queue_select as usize) < self.queues.len() {
                    self.queues[self.queue_select as usize].size = value as u16;
                }
            }
            0x044 => {
                // QueueReady.
                if (self.queue_select as usize) < self.queues.len() {
                    self.queues[self.queue_select as usize].ready = value != 0;
                }
            }
            0x050 => {
                // QueueNotify: guest is notifying a specific queue.
                self.notify(value as u16);
            }
            0x064 => {
                // InterruptACK: guest acknowledges interrupt bits.
                self.isr_status &= !value;
            }
            0x070 => {
                // Status register.
                if value == 0 {
                    // Device reset.
                    self.reset();
                } else {
                    self.status = value;
                }
            }
            _ => {
                // Configuration space writes (offset >= 0x100).
                if offset >= 0x100 && offset < 0x100 + CONFIG_SPACE_SIZE as u64 {
                    let idx = (offset - 0x100) as usize;
                    let bytes = value.to_le_bytes();
                    for (i, &b) in bytes.iter().enumerate() {
                        if idx + i < CONFIG_SPACE_SIZE {
                            self.config_space[idx + i] = b;
                        }
                    }
                }
            }
        }
    }

    /// Process a virtqueue notification from the guest.
    pub fn notify(&mut self, queue_index: u16) {
        let qi = queue_index as usize;
        if qi >= self.queues.len() {
            serial_println!(
                "    [virtio_host] Invalid queue notification {} for device {}",
                queue_index, self.device_id
            );
            return;
        }

        if !self.queues[qi].ready {
            serial_println!(
                "    [virtio_host] Notification for non-ready queue {} on device {}",
                queue_index, self.device_id
            );
            return;
        }

        // Device-type-specific processing.
        match self.dev_type {
            VirtioDeviceType::Net => self.process_net_queue(qi),
            VirtioDeviceType::Block => self.process_block_queue(qi),
            VirtioDeviceType::Console => self.process_console_queue(qi),
            VirtioDeviceType::Rng => self.process_rng_queue(qi),
            _ => {
                serial_println!(
                    "    [virtio_host] Queue notification for unhandled device type {:?}",
                    self.dev_type
                );
            }
        }

        // Set ISR bit to signal completion.
        self.isr_status |= 1;
    }

    /// Reset the device to initial state.
    fn reset(&mut self) {
        self.status = 0;
        self.accepted_features = 0;
        self.isr_status = 0;
        self.queue_select = 0;
        for q in &mut self.queues {
            q.ready = false;
        }
        serial_println!("    [virtio_host] Device {} reset", self.device_id);
    }

    // --- Device-specific queue processors ---

    fn process_net_queue(&self, queue_index: usize) {
        match queue_index {
            0 => { /* RX queue — host would place received packets here */ }
            1 => { /* TX queue — guest is sending packets */ }
            2 => { /* Control queue */ }
            _ => {}
        }
    }

    fn process_block_queue(&self, _queue_index: usize) {
        // Process block I/O request: read header, perform I/O, write status.
    }

    fn process_console_queue(&self, queue_index: usize) {
        match queue_index {
            0 => { /* RX: host sends characters to guest */ }
            1 => { /* TX: guest sends characters to host */ }
            _ => {}
        }
    }

    fn process_rng_queue(&self, _queue_index: usize) {
        // Fill guest buffers with random data from the host RNG.
        // In a real implementation, use RDRAND or the kernel's entropy pool.
    }
}

pub fn init() {
    let registry = VirtioRegistry {
        device_count: 0,
        next_device_id: 1,
    };

    *VIRTIO_REGISTRY.lock() = Some(registry);
    serial_println!("    [virtio_host] Virtio device registry initialized");
}
