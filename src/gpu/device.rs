use crate::sync::Mutex;
/// GPU device abstraction for Genesis
///
/// GPU enumeration, memory management, queue families,
/// feature detection, driver interface.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum GpuVendor {
    Intel,
    Amd,
    Nvidia,
    Qualcomm,
    Arm,
    Software, // software fallback
    Unknown,
}

#[derive(Clone, Copy, PartialEq)]
pub enum QueueType {
    Graphics,
    Compute,
    Transfer,
    Present,
}

struct GpuQueue {
    queue_type: QueueType,
    family_index: u32,
    count: u32,
    priority: u32, // 0-100
}

struct GpuMemoryHeap {
    size_bytes: u64,
    used_bytes: u64,
    device_local: bool,
    host_visible: bool,
    host_coherent: bool,
}

pub struct GpuDevice {
    id: u32,
    vendor: GpuVendor,
    device_id: u32,
    name: [u8; 48],
    name_len: usize,
    vram_bytes: u64,
    max_texture_size: u32,
    max_compute_workgroup: [u32; 3],
    supports_geometry_shader: bool,
    supports_tessellation: bool,
    supports_raytracing: bool,
    supports_mesh_shaders: bool,
    supports_compute: bool,
    api_version: u32,
    queues: Vec<GpuQueue>,
    heaps: Vec<GpuMemoryHeap>,
}

struct GpuManager {
    devices: Vec<GpuDevice>,
    active_device: Option<u32>,
    software_fallback: bool,
}

static GPU_MGR: Mutex<Option<GpuManager>> = Mutex::new(None);

impl GpuManager {
    fn new() -> Self {
        GpuManager {
            devices: Vec::new(),
            active_device: None,
            software_fallback: true,
        }
    }

    fn enumerate(&mut self) {
        // Check PCI for GPU devices (class 0x03)
        // For now, register software rasterizer
        let mut name = [0u8; 48];
        let n = b"Genesis Software Rasterizer";
        name[..n.len()].copy_from_slice(n);
        let mut queues = Vec::new();
        queues.push(GpuQueue {
            queue_type: QueueType::Graphics,
            family_index: 0,
            count: 1,
            priority: 100,
        });
        queues.push(GpuQueue {
            queue_type: QueueType::Compute,
            family_index: 1,
            count: 1,
            priority: 50,
        });
        queues.push(GpuQueue {
            queue_type: QueueType::Transfer,
            family_index: 2,
            count: 1,
            priority: 50,
        });
        let mut heaps = Vec::new();
        heaps.push(GpuMemoryHeap {
            size_bytes: 256 * 1024 * 1024,
            used_bytes: 0,
            device_local: false,
            host_visible: true,
            host_coherent: true,
        });
        self.devices.push(GpuDevice {
            id: 0,
            vendor: GpuVendor::Software,
            device_id: 0,
            name,
            name_len: n.len(),
            vram_bytes: 256 * 1024 * 1024,
            max_texture_size: 4096,
            max_compute_workgroup: [256, 256, 64],
            supports_geometry_shader: true,
            supports_tessellation: true,
            supports_raytracing: false,
            supports_mesh_shaders: false,
            supports_compute: true,
            api_version: 1,
            queues,
            heaps,
        });
        self.active_device = Some(0);
    }

    fn allocate_memory(&mut self, device_id: u32, bytes: u64, device_local: bool) -> Option<u64> {
        let dev = self.devices.iter_mut().find(|d| d.id == device_id)?;
        let heap = dev
            .heaps
            .iter_mut()
            .find(|h| h.device_local == device_local && h.size_bytes - h.used_bytes >= bytes)?;
        let offset = heap.used_bytes;
        heap.used_bytes = heap.used_bytes.saturating_add(bytes);
        Some(offset)
    }
}

pub fn init() {
    let mut mgr = GPU_MGR.lock();
    let mut m = GpuManager::new();
    m.enumerate();
    *mgr = Some(m);
    serial_println!("    GPU: device enumerated (software rasterizer fallback)");
}
