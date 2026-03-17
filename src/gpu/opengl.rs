/// OpenGL-like compatibility layer for Genesis GPU
///
/// State machine model, vertex arrays/buffers, textures,
/// framebuffers, shader programs, uniform management,
/// draw calls, blending, depth/stencil, viewport/scissor.
///
/// All values use Q16 fixed-point (i32, 16 fractional bits). No floats.

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

// ── Q16 fixed-point helpers ───────────────────────────────────────────────

pub type Q16 = i32;
const Q16_ONE: Q16 = 65536;

fn q16_from_int(v: i32) -> Q16 {
    v.wrapping_mul(Q16_ONE)
}

fn q16_mul(a: Q16, b: Q16) -> Q16 {
    ((a as i64 * b as i64) >> 16) as Q16
}

fn q16_div(a: Q16, b: Q16) -> Q16 {
    if b == 0 { return 0; }
    (((a as i64) << 16) / (b as i64)) as Q16
}

// ── GL enums ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
pub enum GlError {
    NoError,
    InvalidEnum,
    InvalidValue,
    InvalidOperation,
    OutOfMemory,
    InvalidFramebufferOp,
    StackOverflow,
    StackUnderflow,
}

#[derive(Clone, Copy, PartialEq)]
pub enum GlPrimitive {
    Points,
    Lines,
    LineStrip,
    LineLoop,
    Triangles,
    TriangleStrip,
    TriangleFan,
}

#[derive(Clone, Copy, PartialEq)]
pub enum GlBufferTarget {
    ArrayBuffer,
    ElementArrayBuffer,
    UniformBuffer,
    ShaderStorageBuffer,
    CopyReadBuffer,
    CopyWriteBuffer,
}

#[derive(Clone, Copy, PartialEq)]
pub enum GlBufferUsage {
    StaticDraw,
    DynamicDraw,
    StreamDraw,
    StaticRead,
    DynamicRead,
    StreamRead,
    StaticCopy,
    DynamicCopy,
    StreamCopy,
}

#[derive(Clone, Copy, PartialEq)]
pub enum GlTextureTarget {
    Texture2D,
    Texture3D,
    TextureCubeMap,
    Texture2DArray,
    TextureRectangle,
    TextureBuffer,
}

#[derive(Clone, Copy, PartialEq)]
pub enum GlTexParam {
    MinFilter,
    MagFilter,
    WrapS,
    WrapT,
    WrapR,
    MinLod,
    MaxLod,
    CompareMode,
    CompareFunc,
    MaxAnisotropy,
}

#[derive(Clone, Copy, PartialEq)]
pub enum GlShaderType {
    Vertex,
    Fragment,
    Geometry,
    TessControl,
    TessEvaluation,
    Compute,
}

#[derive(Clone, Copy, PartialEq)]
pub enum GlBlendFactor {
    Zero,
    One,
    SrcColor,
    OneMinusSrcColor,
    DstColor,
    OneMinusDstColor,
    SrcAlpha,
    OneMinusSrcAlpha,
    DstAlpha,
    OneMinusDstAlpha,
    ConstantColor,
    OneMinusConstantColor,
    ConstantAlpha,
    OneMinusConstantAlpha,
    SrcAlphaSaturate,
}

#[derive(Clone, Copy, PartialEq)]
pub enum GlBlendEquation {
    Add,
    Subtract,
    ReverseSubtract,
    Min,
    Max,
}

#[derive(Clone, Copy, PartialEq)]
pub enum GlDepthFunc {
    Never,
    Less,
    Equal,
    LessEqual,
    Greater,
    NotEqual,
    GreaterEqual,
    Always,
}

#[derive(Clone, Copy, PartialEq)]
pub enum GlCullFace {
    Front,
    Back,
    FrontAndBack,
}

#[derive(Clone, Copy, PartialEq)]
pub enum GlFrontFace {
    Clockwise,
    CounterClockwise,
}

#[derive(Clone, Copy, PartialEq)]
pub enum GlCapability {
    DepthTest,
    StencilTest,
    Blend,
    CullFace,
    ScissorTest,
    Multisample,
    FramebufferSrgb,
    PrimitiveRestart,
    DepthClamp,
    RasterizerDiscard,
    ProgramPointSize,
    SeamlessCubeMap,
}

// ── Buffer object ─────────────────────────────────────────────────────────

struct GlBuffer {
    id: u32,
    target: GlBufferTarget,
    usage: GlBufferUsage,
    size_bytes: u64,
    mapped: bool,
    mapped_offset: u64,
    mapped_length: u64,
}

// ── Vertex array object ───────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct GlVertexAttrib {
    enabled: bool,
    buffer_id: u32,
    size: u32,        // 1..4 components
    stride: u32,
    offset: u64,
    normalized: bool,
    divisor: u32,     // 0 = per-vertex, N = per N instances
    integer: bool,
}

struct GlVertexArray {
    id: u32,
    attribs: [GlVertexAttrib; 16],
    element_buffer: u32,
}

impl GlVertexArray {
    fn new(id: u32) -> Self {
        GlVertexArray {
            id,
            attribs: [GlVertexAttrib {
                enabled: false, buffer_id: 0, size: 4,
                stride: 0, offset: 0, normalized: false,
                divisor: 0, integer: false,
            }; 16],
            element_buffer: 0,
        }
    }
}

// ── Texture object ────────────────────────────────────────────────────────

struct GlTexture {
    id: u32,
    target: GlTextureTarget,
    width: u32,
    height: u32,
    depth: u32,
    internal_format: u32,
    mip_levels: u32,
    min_filter: u32,
    mag_filter: u32,
    wrap_s: u32,
    wrap_t: u32,
    wrap_r: u32,
    anisotropy_q16: Q16,
    memory_bytes: u64,
}

// ── Shader / program ──────────────────────────────────────────────────────

struct GlShader {
    id: u32,
    shader_type: GlShaderType,
    compiled: bool,
    source_hash: u64,
    source_len: u32,
}

struct GlUniform {
    location: i32,
    name_hash: u64,
    uniform_type: u32,
    count: u32,
    data: [i32; 16],    // up to mat4 worth of Q16 values
}

struct GlProgram {
    id: u32,
    vertex_shader: u32,
    fragment_shader: u32,
    geometry_shader: Option<u32>,
    tess_ctrl_shader: Option<u32>,
    tess_eval_shader: Option<u32>,
    compute_shader: Option<u32>,
    linked: bool,
    uniforms: Vec<GlUniform>,
    attrib_locations: Vec<(u64, u32)>,  // (name_hash, location)
    next_uniform_loc: i32,
}

// ── Framebuffer object ────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
pub enum GlAttachmentPoint {
    Color0,
    Color1,
    Color2,
    Color3,
    Color4,
    Color5,
    Color6,
    Color7,
    Depth,
    Stencil,
    DepthStencil,
}

struct GlFramebufferAttachment {
    point: GlAttachmentPoint,
    texture_id: u32,
    level: u32,
    layer: u32,
}

struct GlFramebuffer {
    id: u32,
    attachments: Vec<GlFramebufferAttachment>,
    width: u32,
    height: u32,
    complete: bool,
}

// ── Renderbuffer ──────────────────────────────────────────────────────────

struct GlRenderbuffer {
    id: u32,
    internal_format: u32,
    width: u32,
    height: u32,
    samples: u32,
}

// ── OpenGL state machine ──────────────────────────────────────────────────

struct GlState {
    // Current bindings
    bound_vao: u32,
    bound_program: u32,
    bound_framebuffer_draw: u32,
    bound_framebuffer_read: u32,
    bound_renderbuffer: u32,
    bound_buffers: [(GlBufferTarget, u32); 6],
    bound_textures: [u32; 16],     // per texture unit
    active_texture_unit: u32,

    // Capabilities
    depth_test: bool,
    stencil_test: bool,
    blend: bool,
    cull_face_enabled: bool,
    scissor_test: bool,
    multisample: bool,
    depth_clamp: bool,
    rasterizer_discard: bool,

    // Depth
    depth_func: GlDepthFunc,
    depth_write_mask: bool,
    depth_range_near_q16: Q16,
    depth_range_far_q16: Q16,

    // Blend
    blend_src_rgb: GlBlendFactor,
    blend_dst_rgb: GlBlendFactor,
    blend_src_alpha: GlBlendFactor,
    blend_dst_alpha: GlBlendFactor,
    blend_equation_rgb: GlBlendEquation,
    blend_equation_alpha: GlBlendEquation,
    blend_color: [Q16; 4],

    // Cull face
    cull_face: GlCullFace,
    front_face: GlFrontFace,

    // Viewport / Scissor
    viewport: [i32; 4],
    scissor: [i32; 4],

    // Clear values
    clear_color: [Q16; 4],
    clear_depth_q16: Q16,
    clear_stencil: i32,

    // Color mask
    color_mask: [bool; 4],

    // Line / point
    line_width_q16: Q16,
    point_size_q16: Q16,

    // Polygon offset
    polygon_offset_factor_q16: Q16,
    polygon_offset_units_q16: Q16,
    polygon_offset_fill: bool,

    // Error
    last_error: GlError,
}

// ── Main GL context manager ───────────────────────────────────────────────

struct OpenGlContext {
    state: GlState,
    buffers: Vec<GlBuffer>,
    vaos: Vec<GlVertexArray>,
    textures: Vec<GlTexture>,
    shaders: Vec<GlShader>,
    programs: Vec<GlProgram>,
    framebuffers: Vec<GlFramebuffer>,
    renderbuffers: Vec<GlRenderbuffer>,
    next_buffer_id: u32,
    next_vao_id: u32,
    next_texture_id: u32,
    next_shader_id: u32,
    next_program_id: u32,
    next_fbo_id: u32,
    next_rbo_id: u32,
    draw_calls: u64,
    triangles_drawn: u64,
    total_texture_memory: u64,
    max_texture_memory: u64,
}

static GL_CTX: Mutex<Option<OpenGlContext>> = Mutex::new(None);

impl OpenGlContext {
    fn new() -> Self {
        OpenGlContext {
            state: GlState {
                bound_vao: 0,
                bound_program: 0,
                bound_framebuffer_draw: 0,
                bound_framebuffer_read: 0,
                bound_renderbuffer: 0,
                bound_buffers: [
                    (GlBufferTarget::ArrayBuffer, 0),
                    (GlBufferTarget::ElementArrayBuffer, 0),
                    (GlBufferTarget::UniformBuffer, 0),
                    (GlBufferTarget::ShaderStorageBuffer, 0),
                    (GlBufferTarget::CopyReadBuffer, 0),
                    (GlBufferTarget::CopyWriteBuffer, 0),
                ],
                bound_textures: [0; 16],
                active_texture_unit: 0,
                depth_test: false,
                stencil_test: false,
                blend: false,
                cull_face_enabled: false,
                scissor_test: false,
                multisample: false,
                depth_clamp: false,
                rasterizer_discard: false,
                depth_func: GlDepthFunc::Less,
                depth_write_mask: true,
                depth_range_near_q16: 0,
                depth_range_far_q16: Q16_ONE,
                blend_src_rgb: GlBlendFactor::One,
                blend_dst_rgb: GlBlendFactor::Zero,
                blend_src_alpha: GlBlendFactor::One,
                blend_dst_alpha: GlBlendFactor::Zero,
                blend_equation_rgb: GlBlendEquation::Add,
                blend_equation_alpha: GlBlendEquation::Add,
                blend_color: [0; 4],
                cull_face: GlCullFace::Back,
                front_face: GlFrontFace::CounterClockwise,
                viewport: [0, 0, 1920, 1080],
                scissor: [0, 0, 1920, 1080],
                clear_color: [0, 0, 0, Q16_ONE],
                clear_depth_q16: Q16_ONE,
                clear_stencil: 0,
                color_mask: [true, true, true, true],
                line_width_q16: Q16_ONE,
                point_size_q16: Q16_ONE,
                polygon_offset_factor_q16: 0,
                polygon_offset_units_q16: 0,
                polygon_offset_fill: false,
                last_error: GlError::NoError,
            },
            buffers: Vec::new(),
            vaos: Vec::new(),
            textures: Vec::new(),
            shaders: Vec::new(),
            programs: Vec::new(),
            framebuffers: Vec::new(),
            renderbuffers: Vec::new(),
            next_buffer_id: 1,
            next_vao_id: 1,
            next_texture_id: 1,
            next_shader_id: 1,
            next_program_id: 1,
            next_fbo_id: 1,
            next_rbo_id: 1,
            draw_calls: 0,
            triangles_drawn: 0,
            total_texture_memory: 0,
            max_texture_memory: 512 * 1024 * 1024,
        }
    }

    // ── Capability management ─────────────────────────────────────────────

    fn enable(&mut self, cap: GlCapability) {
        match cap {
            GlCapability::DepthTest => self.state.depth_test = true,
            GlCapability::StencilTest => self.state.stencil_test = true,
            GlCapability::Blend => self.state.blend = true,
            GlCapability::CullFace => self.state.cull_face_enabled = true,
            GlCapability::ScissorTest => self.state.scissor_test = true,
            GlCapability::Multisample => self.state.multisample = true,
            GlCapability::DepthClamp => self.state.depth_clamp = true,
            GlCapability::RasterizerDiscard => self.state.rasterizer_discard = true,
            _ => {}
        }
    }

    fn disable(&mut self, cap: GlCapability) {
        match cap {
            GlCapability::DepthTest => self.state.depth_test = false,
            GlCapability::StencilTest => self.state.stencil_test = false,
            GlCapability::Blend => self.state.blend = false,
            GlCapability::CullFace => self.state.cull_face_enabled = false,
            GlCapability::ScissorTest => self.state.scissor_test = false,
            GlCapability::Multisample => self.state.multisample = false,
            GlCapability::DepthClamp => self.state.depth_clamp = false,
            GlCapability::RasterizerDiscard => self.state.rasterizer_discard = false,
            _ => {}
        }
    }

    fn is_enabled(&self, cap: GlCapability) -> bool {
        match cap {
            GlCapability::DepthTest => self.state.depth_test,
            GlCapability::StencilTest => self.state.stencil_test,
            GlCapability::Blend => self.state.blend,
            GlCapability::CullFace => self.state.cull_face_enabled,
            GlCapability::ScissorTest => self.state.scissor_test,
            GlCapability::Multisample => self.state.multisample,
            GlCapability::DepthClamp => self.state.depth_clamp,
            GlCapability::RasterizerDiscard => self.state.rasterizer_discard,
            _ => false,
        }
    }

    // ── Buffer operations ─────────────────────────────────────────────────

    fn gen_buffer(&mut self) -> u32 {
        let id = self.next_buffer_id;
        self.next_buffer_id = self.next_buffer_id.saturating_add(1);
        self.buffers.push(GlBuffer {
            id, target: GlBufferTarget::ArrayBuffer,
            usage: GlBufferUsage::StaticDraw,
            size_bytes: 0, mapped: false,
            mapped_offset: 0, mapped_length: 0,
        });
        id
    }

    fn bind_buffer(&mut self, target: GlBufferTarget, buf_id: u32) {
        for entry in self.state.bound_buffers.iter_mut() {
            if entry.0 == target {
                entry.1 = buf_id;
                break;
            }
        }
        if let Some(buf) = self.buffers.iter_mut().find(|b| b.id == buf_id) {
            buf.target = target;
        }
    }

    fn buffer_data(&mut self, target: GlBufferTarget, size: u64, usage: GlBufferUsage) {
        let buf_id = self.state.bound_buffers.iter()
            .find(|e| e.0 == target).map(|e| e.1).unwrap_or(0);
        if let Some(buf) = self.buffers.iter_mut().find(|b| b.id == buf_id) {
            buf.size_bytes = size;
            buf.usage = usage;
        }
    }

    fn delete_buffer(&mut self, buf_id: u32) {
        // Unbind if currently bound
        for entry in self.state.bound_buffers.iter_mut() {
            if entry.1 == buf_id { entry.1 = 0; }
        }
        if let Some(idx) = self.buffers.iter().position(|b| b.id == buf_id) {
            self.buffers.remove(idx);
        }
    }

    // ── VAO operations ────────────────────────────────────────────────────

    fn gen_vertex_array(&mut self) -> u32 {
        let id = self.next_vao_id;
        self.next_vao_id = self.next_vao_id.saturating_add(1);
        self.vaos.push(GlVertexArray::new(id));
        id
    }

    fn bind_vertex_array(&mut self, vao_id: u32) {
        self.state.bound_vao = vao_id;
    }

    fn vertex_attrib_pointer(&mut self, index: u32, size: u32, stride: u32,
                             offset: u64, normalized: bool) {
        let vao_id = self.state.bound_vao;
        let buf_id = self.state.bound_buffers.iter()
            .find(|e| e.0 == GlBufferTarget::ArrayBuffer)
            .map(|e| e.1).unwrap_or(0);
        if let Some(vao) = self.vaos.iter_mut().find(|v| v.id == vao_id) {
            let idx = index as usize;
            if idx < 16 {
                vao.attribs[idx].buffer_id = buf_id;
                vao.attribs[idx].size = size;
                vao.attribs[idx].stride = stride;
                vao.attribs[idx].offset = offset;
                vao.attribs[idx].normalized = normalized;
            }
        }
    }

    fn enable_vertex_attrib(&mut self, index: u32) {
        let vao_id = self.state.bound_vao;
        if let Some(vao) = self.vaos.iter_mut().find(|v| v.id == vao_id) {
            let idx = index as usize;
            if idx < 16 { vao.attribs[idx].enabled = true; }
        }
    }

    fn disable_vertex_attrib(&mut self, index: u32) {
        let vao_id = self.state.bound_vao;
        if let Some(vao) = self.vaos.iter_mut().find(|v| v.id == vao_id) {
            let idx = index as usize;
            if idx < 16 { vao.attribs[idx].enabled = false; }
        }
    }

    fn vertex_attrib_divisor(&mut self, index: u32, divisor: u32) {
        let vao_id = self.state.bound_vao;
        if let Some(vao) = self.vaos.iter_mut().find(|v| v.id == vao_id) {
            let idx = index as usize;
            if idx < 16 { vao.attribs[idx].divisor = divisor; }
        }
    }

    fn delete_vertex_array(&mut self, vao_id: u32) {
        if self.state.bound_vao == vao_id { self.state.bound_vao = 0; }
        if let Some(idx) = self.vaos.iter().position(|v| v.id == vao_id) {
            self.vaos.remove(idx);
        }
    }

    // ── Texture operations ────────────────────────────────────────────────

    fn gen_texture(&mut self) -> u32 {
        let id = self.next_texture_id;
        self.next_texture_id = self.next_texture_id.saturating_add(1);
        self.textures.push(GlTexture {
            id, target: GlTextureTarget::Texture2D,
            width: 0, height: 0, depth: 0,
            internal_format: 0x8058, // RGBA8
            mip_levels: 1, min_filter: 0x2601, // LINEAR
            mag_filter: 0x2601, wrap_s: 0x2901, // REPEAT
            wrap_t: 0x2901, wrap_r: 0x2901,
            anisotropy_q16: Q16_ONE, memory_bytes: 0,
        });
        id
    }

    fn bind_texture(&mut self, target: GlTextureTarget, tex_id: u32) {
        let unit = self.state.active_texture_unit as usize;
        if unit < 16 {
            self.state.bound_textures[unit] = tex_id;
        }
        if let Some(tex) = self.textures.iter_mut().find(|t| t.id == tex_id) {
            tex.target = target;
        }
    }

    fn active_texture(&mut self, unit: u32) {
        if unit < 16 {
            self.state.active_texture_unit = unit;
        }
    }

    fn tex_image_2d(&mut self, tex_id: u32, level: u32, internal_format: u32,
                    width: u32, height: u32) {
        if let Some(tex) = self.textures.iter_mut().find(|t| t.id == tex_id) {
            let old_mem = tex.memory_bytes;
            tex.width = width;
            tex.height = height;
            tex.depth = 1;
            tex.internal_format = internal_format;
            // Rough bytes per pixel estimate
            let bpp: u64 = match internal_format {
                0x8229 => 1,        // R8
                0x822B => 2,        // RG8
                0x8058 => 4,        // RGBA8
                0x881A => 8,        // RGBA16F
                0x8814 => 16,       // RGBA32F
                _ => 4,
            };
            tex.memory_bytes = width as u64 * height as u64 * bpp;
            if level == 0 {
                tex.mip_levels = 1;
            } else {
                if level >= tex.mip_levels { tex.mip_levels = level + 1; }
            }
            self.total_texture_memory = self.total_texture_memory - old_mem + tex.memory_bytes;
        }
    }

    fn generate_mipmaps(&mut self, tex_id: u32) {
        if let Some(tex) = self.textures.iter_mut().find(|t| t.id == tex_id) {
            let max_dim = tex.width.max(tex.height);
            if max_dim > 0 {
                tex.mip_levels = 32 - max_dim.leading_zeros();
                // Approx 4/3 of base for total mip chain
                let old_mem = tex.memory_bytes;
                tex.memory_bytes = tex.memory_bytes * 4 / 3;
                self.total_texture_memory = self.total_texture_memory - old_mem + tex.memory_bytes;
            }
        }
    }

    fn tex_parameter(&mut self, tex_id: u32, param: GlTexParam, value: i32) {
        if let Some(tex) = self.textures.iter_mut().find(|t| t.id == tex_id) {
            match param {
                GlTexParam::MinFilter => tex.min_filter = value as u32,
                GlTexParam::MagFilter => tex.mag_filter = value as u32,
                GlTexParam::WrapS => tex.wrap_s = value as u32,
                GlTexParam::WrapT => tex.wrap_t = value as u32,
                GlTexParam::WrapR => tex.wrap_r = value as u32,
                GlTexParam::MaxAnisotropy => tex.anisotropy_q16 = value,
                _ => {}
            }
        }
    }

    fn delete_texture(&mut self, tex_id: u32) {
        // Unbind from all units
        for unit in self.state.bound_textures.iter_mut() {
            if *unit == tex_id { *unit = 0; }
        }
        if let Some(idx) = self.textures.iter().position(|t| t.id == tex_id) {
            self.total_texture_memory -= self.textures[idx].memory_bytes;
            self.textures.remove(idx);
        }
    }

    // ── Shader / program operations ───────────────────────────────────────

    fn create_shader(&mut self, shader_type: GlShaderType) -> u32 {
        let id = self.next_shader_id;
        self.next_shader_id = self.next_shader_id.saturating_add(1);
        self.shaders.push(GlShader {
            id, shader_type, compiled: false,
            source_hash: 0, source_len: 0,
        });
        id
    }

    fn shader_source(&mut self, shader_id: u32, hash: u64, len: u32) {
        if let Some(sh) = self.shaders.iter_mut().find(|s| s.id == shader_id) {
            sh.source_hash = hash;
            sh.source_len = len;
        }
    }

    fn compile_shader(&mut self, shader_id: u32) -> bool {
        if let Some(sh) = self.shaders.iter_mut().find(|s| s.id == shader_id) {
            if sh.source_len > 0 {
                sh.compiled = true;
                return true;
            }
        }
        false
    }

    fn create_program(&mut self) -> u32 {
        let id = self.next_program_id;
        self.next_program_id = self.next_program_id.saturating_add(1);
        self.programs.push(GlProgram {
            id,
            vertex_shader: 0,
            fragment_shader: 0,
            geometry_shader: None,
            tess_ctrl_shader: None,
            tess_eval_shader: None,
            compute_shader: None,
            linked: false,
            uniforms: Vec::new(),
            attrib_locations: Vec::new(),
            next_uniform_loc: 0,
        });
        id
    }

    fn attach_shader(&mut self, program_id: u32, shader_id: u32) {
        let shader_type = self.shaders.iter().find(|s| s.id == shader_id)
            .map(|s| s.shader_type);
        if let (Some(prog), Some(stype)) = (
            self.programs.iter_mut().find(|p| p.id == program_id),
            shader_type
        ) {
            match stype {
                GlShaderType::Vertex => prog.vertex_shader = shader_id,
                GlShaderType::Fragment => prog.fragment_shader = shader_id,
                GlShaderType::Geometry => prog.geometry_shader = Some(shader_id),
                GlShaderType::TessControl => prog.tess_ctrl_shader = Some(shader_id),
                GlShaderType::TessEvaluation => prog.tess_eval_shader = Some(shader_id),
                GlShaderType::Compute => prog.compute_shader = Some(shader_id),
            }
        }
    }

    fn link_program(&mut self, program_id: u32) -> bool {
        if let Some(prog) = self.programs.iter_mut().find(|p| p.id == program_id) {
            if prog.vertex_shader != 0 && prog.fragment_shader != 0 {
                prog.linked = true;
                return true;
            }
            if prog.compute_shader.is_some() {
                prog.linked = true;
                return true;
            }
        }
        false
    }

    fn use_program(&mut self, program_id: u32) {
        self.state.bound_program = program_id;
    }

    fn get_uniform_location(&mut self, program_id: u32, name_hash: u64) -> i32 {
        if let Some(prog) = self.programs.iter_mut().find(|p| p.id == program_id) {
            // Check if already registered
            for u in prog.uniforms.iter() {
                if u.name_hash == name_hash { return u.location; }
            }
            // Register new
            let loc = prog.next_uniform_loc;
            prog.next_uniform_loc = prog.next_uniform_loc.saturating_add(1);
            prog.uniforms.push(GlUniform {
                location: loc, name_hash, uniform_type: 0,
                count: 1, data: [0; 16],
            });
            return loc;
        }
        -1
    }

    fn uniform_1i(&mut self, location: i32, value: i32) {
        let prog_id = self.state.bound_program;
        if let Some(prog) = self.programs.iter_mut().find(|p| p.id == prog_id) {
            if let Some(u) = prog.uniforms.iter_mut().find(|u| u.location == location) {
                u.data[0] = value;
                u.uniform_type = 1;
            }
        }
    }

    fn uniform_4q16(&mut self, location: i32, v: [Q16; 4]) {
        let prog_id = self.state.bound_program;
        if let Some(prog) = self.programs.iter_mut().find(|p| p.id == prog_id) {
            if let Some(u) = prog.uniforms.iter_mut().find(|u| u.location == location) {
                u.data[0] = v[0]; u.data[1] = v[1];
                u.data[2] = v[2]; u.data[3] = v[3];
                u.uniform_type = 4;
            }
        }
    }

    fn uniform_matrix4(&mut self, location: i32, m: &[Q16; 16]) {
        let prog_id = self.state.bound_program;
        if let Some(prog) = self.programs.iter_mut().find(|p| p.id == prog_id) {
            if let Some(u) = prog.uniforms.iter_mut().find(|u| u.location == location) {
                u.data = *m;
                u.uniform_type = 16;
            }
        }
    }

    fn delete_shader(&mut self, shader_id: u32) {
        if let Some(idx) = self.shaders.iter().position(|s| s.id == shader_id) {
            self.shaders.remove(idx);
        }
    }

    fn delete_program(&mut self, program_id: u32) {
        if self.state.bound_program == program_id { self.state.bound_program = 0; }
        if let Some(idx) = self.programs.iter().position(|p| p.id == program_id) {
            self.programs.remove(idx);
        }
    }

    // ── Framebuffer operations ────────────────────────────────────────────

    fn gen_framebuffer(&mut self) -> u32 {
        let id = self.next_fbo_id;
        self.next_fbo_id = self.next_fbo_id.saturating_add(1);
        self.framebuffers.push(GlFramebuffer {
            id, attachments: Vec::new(),
            width: 0, height: 0, complete: false,
        });
        id
    }

    fn bind_framebuffer(&mut self, draw: bool, fbo_id: u32) {
        if draw {
            self.state.bound_framebuffer_draw = fbo_id;
        } else {
            self.state.bound_framebuffer_read = fbo_id;
        }
    }

    fn framebuffer_texture(&mut self, fbo_id: u32, point: GlAttachmentPoint,
                           tex_id: u32, level: u32) {
        if let Some(fbo) = self.framebuffers.iter_mut().find(|f| f.id == fbo_id) {
            // Replace or add
            if let Some(att) = fbo.attachments.iter_mut().find(|a| a.point == point) {
                att.texture_id = tex_id;
                att.level = level;
            } else {
                fbo.attachments.push(GlFramebufferAttachment {
                    point, texture_id: tex_id, level, layer: 0,
                });
            }
            // Update dimensions from texture
            if let Some(tex) = self.textures.iter().find(|t| t.id == tex_id) {
                fbo.width = tex.width;
                fbo.height = tex.height;
            }
            fbo.complete = !fbo.attachments.is_empty();
        }
    }

    fn gen_renderbuffer(&mut self) -> u32 {
        let id = self.next_rbo_id;
        self.next_rbo_id = self.next_rbo_id.saturating_add(1);
        self.renderbuffers.push(GlRenderbuffer {
            id, internal_format: 0, width: 0, height: 0, samples: 0,
        });
        id
    }

    fn renderbuffer_storage(&mut self, rbo_id: u32, format: u32, w: u32, h: u32, samples: u32) {
        if let Some(rbo) = self.renderbuffers.iter_mut().find(|r| r.id == rbo_id) {
            rbo.internal_format = format;
            rbo.width = w;
            rbo.height = h;
            rbo.samples = samples;
        }
    }

    fn delete_framebuffer(&mut self, fbo_id: u32) {
        if self.state.bound_framebuffer_draw == fbo_id { self.state.bound_framebuffer_draw = 0; }
        if self.state.bound_framebuffer_read == fbo_id { self.state.bound_framebuffer_read = 0; }
        if let Some(idx) = self.framebuffers.iter().position(|f| f.id == fbo_id) {
            self.framebuffers.remove(idx);
        }
    }

    // ── Draw calls ────────────────────────────────────────────────────────

    fn draw_arrays(&mut self, mode: GlPrimitive, first: u32, count: u32) {
        self.draw_calls = self.draw_calls.saturating_add(1);
        let tris = match mode {
            GlPrimitive::Triangles => count / 3,
            GlPrimitive::TriangleStrip | GlPrimitive::TriangleFan => {
                if count > 2 { count - 2 } else { 0 }
            },
            _ => 0,
        };
        self.triangles_drawn += tris as u64;
    }

    fn draw_elements(&mut self, mode: GlPrimitive, count: u32, _offset: u64) {
        self.draw_calls = self.draw_calls.saturating_add(1);
        let tris = match mode {
            GlPrimitive::Triangles => count / 3,
            GlPrimitive::TriangleStrip | GlPrimitive::TriangleFan => {
                if count > 2 { count - 2 } else { 0 }
            },
            _ => 0,
        };
        self.triangles_drawn += tris as u64;
    }

    fn draw_arrays_instanced(&mut self, mode: GlPrimitive, first: u32,
                             count: u32, instances: u32) {
        self.draw_calls = self.draw_calls.saturating_add(1);
        let tris = match mode {
            GlPrimitive::Triangles => (count / 3) * instances,
            GlPrimitive::TriangleStrip | GlPrimitive::TriangleFan => {
                if count > 2 { (count - 2) * instances } else { 0 }
            },
            _ => 0,
        };
        self.triangles_drawn += tris as u64;
    }

    fn draw_elements_instanced(&mut self, mode: GlPrimitive, count: u32,
                               _offset: u64, instances: u32) {
        self.draw_calls = self.draw_calls.saturating_add(1);
        let tris = match mode {
            GlPrimitive::Triangles => (count / 3) * instances,
            _ => 0,
        };
        self.triangles_drawn += tris as u64;
    }

    // ── State setters ─────────────────────────────────────────────────────

    fn viewport(&mut self, x: i32, y: i32, w: i32, h: i32) {
        self.state.viewport = [x, y, w, h];
    }

    fn scissor(&mut self, x: i32, y: i32, w: i32, h: i32) {
        self.state.scissor = [x, y, w, h];
    }

    fn clear_color(&mut self, r: Q16, g: Q16, b: Q16, a: Q16) {
        self.state.clear_color = [r, g, b, a];
    }

    fn depth_func(&mut self, func: GlDepthFunc) {
        self.state.depth_func = func;
    }

    fn depth_mask(&mut self, enabled: bool) {
        self.state.depth_write_mask = enabled;
    }

    fn blend_func(&mut self, src: GlBlendFactor, dst: GlBlendFactor) {
        self.state.blend_src_rgb = src;
        self.state.blend_dst_rgb = dst;
        self.state.blend_src_alpha = src;
        self.state.blend_dst_alpha = dst;
    }

    fn blend_func_separate(&mut self, src_rgb: GlBlendFactor, dst_rgb: GlBlendFactor,
                           src_a: GlBlendFactor, dst_a: GlBlendFactor) {
        self.state.blend_src_rgb = src_rgb;
        self.state.blend_dst_rgb = dst_rgb;
        self.state.blend_src_alpha = src_a;
        self.state.blend_dst_alpha = dst_a;
    }

    fn cull_face(&mut self, face: GlCullFace) {
        self.state.cull_face = face;
    }

    fn front_face(&mut self, face: GlFrontFace) {
        self.state.front_face = face;
    }

    fn color_mask(&mut self, r: bool, g: bool, b: bool, a: bool) {
        self.state.color_mask = [r, g, b, a];
    }

    fn line_width(&mut self, width_q16: Q16) {
        self.state.line_width_q16 = width_q16;
    }

    fn polygon_offset(&mut self, factor_q16: Q16, units_q16: Q16) {
        self.state.polygon_offset_factor_q16 = factor_q16;
        self.state.polygon_offset_units_q16 = units_q16;
    }

    fn get_error(&mut self) -> GlError {
        let err = self.state.last_error;
        self.state.last_error = GlError::NoError;
        err
    }

    fn reset_stats(&mut self) {
        self.draw_calls = 0;
        self.triangles_drawn = 0;
    }
}

pub fn init() {
    let mut gl = GL_CTX.lock();
    *gl = Some(OpenGlContext::new());
    serial_println!("    GPU: OpenGL-like compatibility layer (state machine, VAO, FBO, shaders) ready");
}
