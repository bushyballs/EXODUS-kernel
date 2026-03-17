use crate::sync::Mutex;
/// Graphics pipeline for Genesis GPU (Vulkan-like)
///
/// Pipeline state objects, render passes, framebuffers,
/// descriptor sets, push constants.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum PrimitiveTopology {
    PointList,
    LineList,
    LineStrip,
    TriangleList,
    TriangleStrip,
    TriangleFan,
}

#[derive(Clone, Copy, PartialEq)]
pub enum CompareOp {
    Never,
    Less,
    Equal,
    LessOrEqual,
    Greater,
    NotEqual,
    GreaterOrEqual,
    Always,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Format {
    R8Unorm,
    Rg8Unorm,
    Rgba8Unorm,
    Rgba8Srgb,
    Bgra8Unorm,
    R16Float,
    Rgba16Float,
    R32Float,
    Rg32Float,
    Rgba32Float,
    Depth32Float,
    Depth24Stencil8,
}

struct RenderPass {
    id: u32,
    color_attachments: Vec<Format>,
    depth_attachment: Option<Format>,
    samples: u8,
}

struct GraphicsPipeline {
    id: u32,
    vertex_shader: u32,
    fragment_shader: u32,
    topology: PrimitiveTopology,
    depth_compare: CompareOp,
    depth_test: bool,
    depth_write: bool,
    render_pass: u32,
    blend_enabled: bool,
}

struct PipelineManager {
    render_passes: Vec<RenderPass>,
    pipelines: Vec<GraphicsPipeline>,
    next_rp_id: u32,
    next_pipe_id: u32,
}

static PIPELINE: Mutex<Option<PipelineManager>> = Mutex::new(None);

impl PipelineManager {
    fn new() -> Self {
        PipelineManager {
            render_passes: Vec::new(),
            pipelines: Vec::new(),
            next_rp_id: 1,
            next_pipe_id: 1,
        }
    }

    fn create_render_pass(
        &mut self,
        color_formats: &[Format],
        depth: Option<Format>,
        samples: u8,
    ) -> u32 {
        let id = self.next_rp_id;
        self.next_rp_id = self.next_rp_id.saturating_add(1);
        let mut colors = Vec::new();
        colors.extend_from_slice(color_formats);
        self.render_passes.push(RenderPass {
            id,
            color_attachments: colors,
            depth_attachment: depth,
            samples,
        });
        id
    }

    fn create_pipeline(
        &mut self,
        vs: u32,
        fs: u32,
        topo: PrimitiveTopology,
        depth: bool,
        render_pass: u32,
    ) -> u32 {
        let id = self.next_pipe_id;
        self.next_pipe_id = self.next_pipe_id.saturating_add(1);
        self.pipelines.push(GraphicsPipeline {
            id,
            vertex_shader: vs,
            fragment_shader: fs,
            topology: topo,
            depth_compare: CompareOp::Less,
            depth_test: depth,
            depth_write: depth,
            render_pass,
            blend_enabled: false,
        });
        id
    }
}

pub fn init() {
    let mut p = PIPELINE.lock();
    *p = Some(PipelineManager::new());
    serial_println!("    GPU: Vulkan-like pipeline manager ready");
}
