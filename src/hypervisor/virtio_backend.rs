/// Virtio device backend for guests
///
/// Part of the AIOS.
///
/// Implements the host-side backend for virtio devices exposed to guest VMs.
/// The backend processes virtqueue descriptors, performs the actual I/O,
/// and writes completion status back to the used ring.

use alloc::vec::Vec;
use crate::{serial_print, serial_println};
use crate::sync::Mutex;
use core::ptr::{read_volatile, write_volatile};

/// Global virtio backend manager.
static VIRTIO_BACKENDS: Mutex<Option<VirtioBackendManager>> = Mutex::new(None);

/// Maximum number of virtio backend instances.
const MAX_BACKENDS: usize = 32;

/// Maximum queue size (number of descriptors).
const MAX_QUEUE_SIZE: u16 = 256;

/// Manages all active virtio backend instances.
struct VirtioBackendManager {
    backends: Vec<Option<BackendEntry>>,
    next_id: u64,
}

struct BackendEntry {
    id: u64,
    device_type: u32,
}

/// Virtqueue descriptor flags.
const VIRTQ_DESC_F_NEXT: u16 = 1;
const VIRTQ_DESC_F_WRITE: u16 = 2;
const VIRTQ_DESC_F_INDIRECT: u16 = 4;

/// Host-side virtio device backend for guest VM I/O.
pub struct VirtioBackend {
    queues: Vec<VirtQueue>,
    /// Backend instance ID.
    backend_id: u64,
    /// Device type (matches virtio spec device IDs).
    device_type: u32,
    /// Status register bits.
    status: u32,
    /// Feature bits negotiated with the guest.
    features: u64,
    /// Number of processed requests (for statistics).
    processed_count: u64,
}

struct VirtQueue {
    /// Physical address of the descriptor table.
    desc_addr: u64,
    /// Physical address of the available ring.
    avail_addr: u64,
    /// Physical address of the used ring.
    used_addr: u64,
    /// Queue size (number of entries).
    size: u16,
    /// Last index we consumed from the available ring.
    last_avail_idx: u16,
    /// Whether this queue is enabled.
    enabled: bool,
    /// Notification suppression flag.
    notification_suppressed: bool,
}

/// Virtqueue descriptor (16 bytes, matching the virtio spec).
#[repr(C)]
struct VirtqDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

/// Available ring header.
#[repr(C)]
struct VirtqAvail {
    flags: u16,
    idx: u16,
    // ring[size] follows, then used_event.
}

/// Used ring header.
#[repr(C)]
struct VirtqUsed {
    flags: u16,
    idx: u16,
    // ring[size] of VirtqUsedElem follows, then avail_event.
}

/// Used ring element.
#[repr(C)]
struct VirtqUsedElem {
    id: u32,
    len: u32,
}

impl VirtQueue {
    fn new() -> Self {
        VirtQueue {
            desc_addr: 0,
            avail_addr: 0,
            used_addr: 0,
            size: MAX_QUEUE_SIZE,
            last_avail_idx: 0,
            enabled: false,
            notification_suppressed: false,
        }
    }

    /// Configure this queue's addresses and size.
    fn configure(&mut self, desc: u64, avail: u64, used: u64, size: u16) {
        self.desc_addr = desc;
        self.avail_addr = avail;
        self.used_addr = used;
        self.size = if size > MAX_QUEUE_SIZE { MAX_QUEUE_SIZE } else { size };
        self.enabled = true;
        self.last_avail_idx = 0;
    }

    /// Check if there are new available descriptors from the guest.
    fn has_available(&self) -> bool {
        if !self.enabled || self.avail_addr == 0 {
            return false;
        }

        // Read the guest's available ring index.
        let avail_idx = unsafe {
            let avail = self.avail_addr as *const VirtqAvail;
            read_volatile(&(*avail).idx)
        };

        avail_idx != self.last_avail_idx
    }

    /// Consume the next available descriptor chain.
    /// Returns the head descriptor index, or None if nothing is available.
    fn pop_available(&mut self) -> Option<u16> {
        if !self.has_available() {
            return None;
        }

        // Read the ring entry at last_avail_idx % size.
        let ring_offset = (self.last_avail_idx % self.size) as usize;
        let ring_entry_addr = self.avail_addr + 4 + (ring_offset as u64) * 2;

        let desc_idx = unsafe {
            read_volatile(ring_entry_addr as *const u16)
        };

        self.last_avail_idx = self.last_avail_idx.wrapping_add(1);
        Some(desc_idx)
    }

    /// Push a completed descriptor chain onto the used ring.
    fn push_used(&mut self, desc_idx: u16, bytes_written: u32) {
        if self.used_addr == 0 {
            return;
        }

        let used = self.used_addr as *mut VirtqUsed;
        let used_idx = unsafe { read_volatile(&(*used).idx) };

        let ring_offset = (used_idx % self.size) as usize;
        let elem_addr = self.used_addr + 4 + (ring_offset as u64) * 8;

        unsafe {
            let elem = elem_addr as *mut VirtqUsedElem;
            write_volatile(&mut (*elem).id, desc_idx as u32);
            write_volatile(&mut (*elem).len, bytes_written);

            // Increment the used index.
            write_volatile(&mut (*used).idx, used_idx.wrapping_add(1));
        }
    }

    /// Read a descriptor from the descriptor table.
    fn read_descriptor(&self, idx: u16) -> VirtqDesc {
        let desc_addr = self.desc_addr + (idx as u64) * 16;
        unsafe {
            let desc_ptr = desc_addr as *const VirtqDesc;
            VirtqDesc {
                addr: read_volatile(&(*desc_ptr).addr),
                len: read_volatile(&(*desc_ptr).len),
                flags: read_volatile(&(*desc_ptr).flags),
                next: read_volatile(&(*desc_ptr).next),
            }
        }
    }
}

impl VirtioBackend {
    pub fn new() -> Self {
        let backend_id = {
            let mut mgr = VIRTIO_BACKENDS.lock();
            if let Some(ref mut m) = *mgr {
                let id = m.next_id;
                m.next_id = m.next_id.saturating_add(1);
                id
            } else {
                0
            }
        };

        let mut queues = Vec::new();
        // Most virtio devices need at least 2 queues (rx + tx or request + completion).
        queues.push(VirtQueue::new());
        queues.push(VirtQueue::new());

        VirtioBackend {
            queues,
            backend_id,
            device_type: 0,
            status: 0,
            features: 0,
            processed_count: 0,
        }
    }

    /// Configure the backend for a specific device type.
    pub fn set_device_type(&mut self, device_type: u32) {
        self.device_type = device_type;
    }

    /// Add a new queue to the backend.
    pub fn add_queue(&mut self) -> usize {
        let idx = self.queues.len();
        self.queues.push(VirtQueue::new());
        idx
    }

    /// Configure a specific queue's memory regions.
    pub fn configure_queue(&mut self, queue_index: usize, desc: u64, avail: u64, used: u64, size: u16) {
        if queue_index < self.queues.len() {
            self.queues[queue_index].configure(desc, avail, used, size);
            serial_println!(
                "    [virtio_backend] Backend {} queue {} configured (size={})",
                self.backend_id, queue_index, size
            );
        }
    }

    /// Process available descriptors from a virtqueue.
    ///
    /// Walks the available ring, processes each descriptor chain,
    /// and writes completion status to the used ring.
    pub fn process_queue(&mut self, queue_index: usize) {
        if queue_index >= self.queues.len() {
            return;
        }

        // Process all available descriptor chains.
        loop {
            let head_idx = match self.queues[queue_index].pop_available() {
                Some(idx) => idx,
                None => break,
            };

            // Walk the descriptor chain.
            let mut total_bytes = 0u32;
            let mut current_idx = head_idx;
            loop {
                let desc = self.queues[queue_index].read_descriptor(current_idx);

                if desc.flags & VIRTQ_DESC_F_WRITE != 0 {
                    // Device-writable descriptor: write response data.
                    // For now, zero-fill the buffer (device-specific handling goes here).
                    if desc.addr != 0 && desc.len > 0 {
                        unsafe {
                            let ptr = desc.addr as *mut u8;
                            for i in 0..desc.len as usize {
                                write_volatile(ptr.add(i), 0);
                            }
                        }
                        total_bytes += desc.len;
                    }
                } else {
                    // Device-readable descriptor: consume input data.
                    // Device-specific processing would read from desc.addr.
                    total_bytes += desc.len;
                }

                // Follow chain if NEXT flag is set.
                if desc.flags & VIRTQ_DESC_F_NEXT != 0 {
                    current_idx = desc.next;
                } else {
                    break;
                }
            }

            // Push completion to the used ring.
            self.queues[queue_index].push_used(head_idx, total_bytes);
            self.processed_count = self.processed_count.saturating_add(1);
        }
    }

    /// Notify the guest that used buffers are available.
    ///
    /// This triggers a virtual interrupt (typically MSI-X) to the guest.
    pub fn notify_guest(&self, queue_index: usize) {
        if queue_index >= self.queues.len() {
            return;
        }

        if self.queues[queue_index].notification_suppressed {
            return;
        }

        // In a full implementation, this would inject an interrupt into the
        // guest via the virtual APIC or posted interrupt descriptor.
        // For now, log the notification.
        serial_println!(
            "    [virtio_backend] Notifying guest for backend {} queue {}",
            self.backend_id, queue_index
        );
    }

    /// Set the device status register.
    pub fn set_status(&mut self, status: u32) {
        self.status = status;
    }

    /// Negotiate feature bits with the guest.
    pub fn negotiate_features(&mut self, guest_features: u64) {
        // Accept the intersection of offered and guest-requested features.
        self.features = self.features & guest_features;
    }
}

pub fn init() {
    let mgr = VirtioBackendManager {
        backends: Vec::new(),
        next_id: 1,
    };

    *VIRTIO_BACKENDS.lock() = Some(mgr);
    serial_println!("    [virtio_backend] Virtio backend subsystem initialized");
}
