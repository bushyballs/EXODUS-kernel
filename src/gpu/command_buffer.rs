use crate::sync::Mutex;
/// Command buffer for Genesis GPU (Vulkan-like)
///
/// Record-and-submit model, primary/secondary buffers,
/// synchronization, fences, semaphores.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum CmdType {
    Draw,
    DrawIndexed,
    Dispatch,
    CopyBuffer,
    CopyTexture,
    SetPipeline,
    SetViewport,
    SetScissor,
    BeginRenderPass,
    EndRenderPass,
    PushConstants,
    BindDescriptors,
    Barrier,
}

struct Command {
    cmd_type: CmdType,
    param_a: u64,
    param_b: u64,
    param_c: u32,
    param_d: u32,
}

#[derive(Clone, Copy, PartialEq)]
pub enum CmdBufferState {
    Initial,
    Recording,
    Executable,
    Pending,
    Invalid,
}

struct CommandBuffer {
    id: u32,
    state: CmdBufferState,
    commands: Vec<Command>,
    primary: bool,
}

struct Fence {
    id: u32,
    signaled: bool,
}

struct CommandEngine {
    buffers: Vec<CommandBuffer>,
    fences: Vec<Fence>,
    next_buf_id: u32,
    next_fence_id: u32,
    submitted: u64,
}

static CMD_ENGINE: Mutex<Option<CommandEngine>> = Mutex::new(None);

impl CommandEngine {
    fn new() -> Self {
        CommandEngine {
            buffers: Vec::new(),
            fences: Vec::new(),
            next_buf_id: 1,
            next_fence_id: 1,
            submitted: 0,
        }
    }

    fn allocate(&mut self, primary: bool) -> u32 {
        let id = self.next_buf_id;
        self.next_buf_id = self.next_buf_id.saturating_add(1);
        self.buffers.push(CommandBuffer {
            id,
            state: CmdBufferState::Initial,
            commands: Vec::new(),
            primary,
        });
        id
    }

    fn begin(&mut self, buf_id: u32) -> bool {
        if let Some(buf) = self.buffers.iter_mut().find(|b| b.id == buf_id) {
            buf.state = CmdBufferState::Recording;
            buf.commands.clear();
            return true;
        }
        false
    }

    fn record(&mut self, buf_id: u32, cmd_type: CmdType, a: u64, b: u64, c: u32, d: u32) {
        if let Some(buf) = self.buffers.iter_mut().find(|b| b.id == buf_id) {
            if buf.state == CmdBufferState::Recording {
                buf.commands.push(Command {
                    cmd_type,
                    param_a: a,
                    param_b: b,
                    param_c: c,
                    param_d: d,
                });
            }
        }
    }

    fn end(&mut self, buf_id: u32) -> bool {
        if let Some(buf) = self.buffers.iter_mut().find(|b| b.id == buf_id) {
            if buf.state == CmdBufferState::Recording {
                buf.state = CmdBufferState::Executable;
                return true;
            }
        }
        false
    }

    fn submit(&mut self, buf_id: u32, fence_id: Option<u32>) -> bool {
        if let Some(buf) = self.buffers.iter_mut().find(|b| b.id == buf_id) {
            if buf.state == CmdBufferState::Executable {
                buf.state = CmdBufferState::Pending;
                self.submitted = self.submitted.saturating_add(1);
                // Signal fence
                if let Some(fid) = fence_id {
                    if let Some(fence) = self.fences.iter_mut().find(|f| f.id == fid) {
                        fence.signaled = true;
                    }
                }
                return true;
            }
        }
        false
    }

    fn create_fence(&mut self) -> u32 {
        let id = self.next_fence_id;
        self.next_fence_id = self.next_fence_id.saturating_add(1);
        self.fences.push(Fence {
            id,
            signaled: false,
        });
        id
    }

    fn wait_fence(&self, fence_id: u32) -> bool {
        self.fences
            .iter()
            .find(|f| f.id == fence_id)
            .map(|f| f.signaled)
            .unwrap_or(false)
    }
}

pub fn init() {
    let mut c = CMD_ENGINE.lock();
    *c = Some(CommandEngine::new());
    serial_println!("    GPU: command buffers (record/submit, fences) ready");
}
