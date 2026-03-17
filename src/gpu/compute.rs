use crate::sync::Mutex;
/// GPU compute for Genesis
///
/// General-purpose GPU computing, dispatch,
/// storage buffers, atomic operations.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

struct ComputeDispatch {
    id: u32,
    shader_id: u32,
    workgroup_x: u32,
    workgroup_y: u32,
    workgroup_z: u32,
    buffer_bindings: Vec<BufferBinding>,
}

struct BufferBinding {
    binding: u32,
    buffer_offset: u64,
    buffer_size: u64,
    read_only: bool,
}

struct ComputeEngine {
    dispatches: Vec<ComputeDispatch>,
    next_id: u32,
    total_dispatches: u64,
    total_workgroups: u64,
}

static COMPUTE: Mutex<Option<ComputeEngine>> = Mutex::new(None);

impl ComputeEngine {
    fn new() -> Self {
        ComputeEngine {
            dispatches: Vec::new(),
            next_id: 1,
            total_dispatches: 0,
            total_workgroups: 0,
        }
    }

    fn dispatch(&mut self, shader_id: u32, x: u32, y: u32, z: u32) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.total_dispatches = self.total_dispatches.saturating_add(1);
        self.total_workgroups += (x as u64) * (y as u64) * (z as u64);
        self.dispatches.push(ComputeDispatch {
            id,
            shader_id,
            workgroup_x: x,
            workgroup_y: y,
            workgroup_z: z,
            buffer_bindings: Vec::new(),
        });
        id
    }

    fn bind_buffer(
        &mut self,
        dispatch_id: u32,
        binding: u32,
        offset: u64,
        size: u64,
        read_only: bool,
    ) {
        if let Some(d) = self.dispatches.iter_mut().find(|d| d.id == dispatch_id) {
            d.buffer_bindings.push(BufferBinding {
                binding,
                buffer_offset: offset,
                buffer_size: size,
                read_only,
            });
        }
    }
}

pub fn init() {
    let mut c = COMPUTE.lock();
    *c = Some(ComputeEngine::new());
    serial_println!("    GPU: compute shader dispatch ready");
}
