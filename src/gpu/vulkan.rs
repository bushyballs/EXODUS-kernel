/// Vulkan-like API for Genesis GPU
///
/// Instance creation, logical device, command buffers,
/// render passes, graphics/compute pipelines, descriptor sets,
/// synchronization primitives (fences, semaphores, barriers).
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
    ((a as i64) << 16) as i64 / (b as i64) as Q16
}

// ── Result / error types ──────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
pub enum VkResult {
    Success,
    NotReady,
    Timeout,
    ErrorOutOfDeviceMemory,
    ErrorOutOfHostMemory,
    ErrorDeviceLost,
    ErrorInitFailed,
    ErrorLayerNotPresent,
    ErrorExtensionNotPresent,
    ErrorIncompatibleDriver,
    ErrorTooManyObjects,
    ErrorFeatureNotPresent,
}

// ── Instance ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
pub enum VkApiVersion {
    V10,
    V11,
    V12,
    V13,
}

struct VkInstance {
    id: u32,
    api_version: VkApiVersion,
    app_name: [u8; 64],
    app_name_len: usize,
    engine_name: [u8; 64],
    engine_name_len: usize,
    enabled_layers: Vec<u32>,
    enabled_extensions: Vec<u32>,
    physical_devices: Vec<u32>,
    debug_enabled: bool,
}

// ── Physical device ───────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
pub enum VkPhysicalDeviceType {
    DiscreteGpu,
    IntegratedGpu,
    VirtualGpu,
    Cpu,
    Other,
}

struct VkPhysicalDevice {
    id: u32,
    device_type: VkPhysicalDeviceType,
    vendor_id: u32,
    device_id: u32,
    name: [u8; 64],
    name_len: usize,
    max_image_dim_2d: u32,
    max_image_dim_3d: u32,
    max_viewports: u32,
    max_framebuffer_width: u32,
    max_framebuffer_height: u32,
    max_push_constant_size: u32,
    max_bound_descriptor_sets: u32,
    max_per_stage_descriptor_samplers: u32,
    max_vertex_input_attributes: u32,
    max_compute_workgroup_count: [u32; 3],
    max_compute_workgroup_size: [u32; 3],
    max_compute_workgroup_invocations: u32,
    timestamp_period_q16: Q16,
    queue_family_count: u32,
}

// ── Queue family ──────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
pub struct VkQueueFlags {
    pub graphics: bool,
    pub compute: bool,
    pub transfer: bool,
    pub sparse_binding: bool,
    pub protected: bool,
}

struct VkQueueFamily {
    index: u32,
    flags: VkQueueFlags,
    queue_count: u32,
    timestamp_valid_bits: u32,
    min_image_transfer_granularity: [u32; 3],
}

// ── Logical device ────────────────────────────────────────────────────────

struct VkDeviceQueue {
    family_index: u32,
    queue_index: u32,
    priority_q16: Q16,
}

struct VkDevice {
    id: u32,
    physical_device: u32,
    queues: Vec<VkDeviceQueue>,
    enabled_features: VkDeviceFeatures,
    enabled_extensions: Vec<u32>,
}

#[derive(Clone, Copy)]
struct VkDeviceFeatures {
    geometry_shader: bool,
    tessellation_shader: bool,
    multi_draw_indirect: bool,
    sampler_anisotropy: bool,
    texture_compression_bc: bool,
    vertex_pipeline_stores_and_atomics: bool,
    fragment_stores_and_atomics: bool,
    shader_int64: bool,
    shader_float64: bool,
    sparse_binding: bool,
    multi_viewport: bool,
    wide_lines: bool,
    large_points: bool,
    fill_mode_non_solid: bool,
    depth_clamp: bool,
    depth_bias_clamp: bool,
}

// ── Render pass ───────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
pub enum VkAttachmentLoadOp {
    Load,
    Clear,
    DontCare,
}

#[derive(Clone, Copy, PartialEq)]
pub enum VkAttachmentStoreOp {
    Store,
    DontCare,
}

#[derive(Clone, Copy, PartialEq)]
pub enum VkImageLayout {
    Undefined,
    General,
    ColorAttachmentOptimal,
    DepthStencilAttachmentOptimal,
    DepthStencilReadOnlyOptimal,
    ShaderReadOnlyOptimal,
    TransferSrcOptimal,
    TransferDstOptimal,
    PresentSrc,
}

struct VkAttachmentDescription {
    format_id: u32,
    samples: u8,
    load_op: VkAttachmentLoadOp,
    store_op: VkAttachmentStoreOp,
    stencil_load_op: VkAttachmentLoadOp,
    stencil_store_op: VkAttachmentStoreOp,
    initial_layout: VkImageLayout,
    final_layout: VkImageLayout,
}

struct VkSubpassDescription {
    color_attachments: Vec<u32>,
    depth_attachment: Option<u32>,
    input_attachments: Vec<u32>,
    resolve_attachments: Vec<u32>,
    preserve_attachments: Vec<u32>,
}

struct VkSubpassDependency {
    src_subpass: u32,
    dst_subpass: u32,
    src_stage_mask: u32,
    dst_stage_mask: u32,
    src_access_mask: u32,
    dst_access_mask: u32,
}

struct VkRenderPass {
    id: u32,
    attachments: Vec<VkAttachmentDescription>,
    subpasses: Vec<VkSubpassDescription>,
    dependencies: Vec<VkSubpassDependency>,
}

// ── Pipeline ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
pub enum VkPipelineBindPoint {
    Graphics,
    Compute,
    RayTracing,
}

#[derive(Clone, Copy, PartialEq)]
pub enum VkDynamicState {
    Viewport,
    Scissor,
    LineWidth,
    DepthBias,
    BlendConstants,
    DepthBounds,
    StencilCompareMask,
    StencilWriteMask,
    StencilReference,
}

struct VkPipelineLayout {
    id: u32,
    descriptor_set_layouts: Vec<u32>,
    push_constant_offset: u32,
    push_constant_size: u32,
}

struct VkGraphicsPipeline {
    id: u32,
    layout_id: u32,
    render_pass_id: u32,
    subpass_index: u32,
    vertex_shader_id: u32,
    fragment_shader_id: u32,
    geometry_shader_id: Option<u32>,
    tess_ctrl_shader_id: Option<u32>,
    tess_eval_shader_id: Option<u32>,
    topology: u32,
    primitive_restart: bool,
    depth_test_enable: bool,
    depth_write_enable: bool,
    depth_compare_op: u32,
    stencil_test_enable: bool,
    blend_enable: bool,
    cull_mode: u32,
    front_face: u32,
    polygon_mode: u32,
    line_width_q16: Q16,
    dynamic_states: Vec<VkDynamicState>,
}

// ── Descriptor sets ───────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
pub enum VkDescriptorType {
    Sampler,
    CombinedImageSampler,
    SampledImage,
    StorageImage,
    UniformTexelBuffer,
    StorageTexelBuffer,
    UniformBuffer,
    StorageBuffer,
    UniformBufferDynamic,
    StorageBufferDynamic,
    InputAttachment,
}

struct VkDescriptorSetLayoutBinding {
    binding: u32,
    descriptor_type: VkDescriptorType,
    descriptor_count: u32,
    stage_flags: u32,
}

struct VkDescriptorSetLayout {
    id: u32,
    bindings: Vec<VkDescriptorSetLayoutBinding>,
}

struct VkDescriptorPool {
    id: u32,
    max_sets: u32,
    allocated_sets: u32,
    pool_sizes: Vec<(VkDescriptorType, u32)>,
}

struct VkDescriptorSet {
    id: u32,
    layout_id: u32,
    pool_id: u32,
    bindings: Vec<VkDescriptorBinding>,
}

struct VkDescriptorBinding {
    binding: u32,
    descriptor_type: VkDescriptorType,
    buffer_handle: u64,
    offset: u64,
    range: u64,
    image_handle: u32,
    sampler_handle: u32,
}

// ── Synchronization ───────────────────────────────────────────────────────

struct VkFence {
    id: u32,
    signaled: bool,
    device_id: u32,
}

struct VkSemaphore {
    id: u32,
    signaled: bool,
    timeline_value: u64,
    is_timeline: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub enum VkPipelineStage {
    TopOfPipe,
    DrawIndirect,
    VertexInput,
    VertexShader,
    TessControlShader,
    TessEvalShader,
    GeometryShader,
    FragmentShader,
    EarlyFragmentTests,
    LateFragmentTests,
    ColorAttachmentOutput,
    ComputeShader,
    Transfer,
    BottomOfPipe,
    AllGraphics,
    AllCommands,
}

struct VkMemoryBarrier {
    src_stage: VkPipelineStage,
    dst_stage: VkPipelineStage,
    src_access: u32,
    dst_access: u32,
}

// ── Framebuffer ───────────────────────────────────────────────────────────

struct VkFramebuffer {
    id: u32,
    render_pass_id: u32,
    attachments: Vec<u32>,
    width: u32,
    height: u32,
    layers: u32,
}

// ── Command pool / buffer ─────────────────────────────────────────────────

struct VkCommandPool {
    id: u32,
    queue_family_index: u32,
    transient: bool,
    reset_individual: bool,
    allocated_buffers: Vec<u32>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum VkCommandBufferLevel {
    Primary,
    Secondary,
}

// ── Main manager ──────────────────────────────────────────────────────────

struct VulkanManager {
    instances: Vec<VkInstance>,
    physical_devices: Vec<VkPhysicalDevice>,
    queue_families: Vec<VkQueueFamily>,
    devices: Vec<VkDevice>,
    render_passes: Vec<VkRenderPass>,
    pipeline_layouts: Vec<VkPipelineLayout>,
    graphics_pipelines: Vec<VkGraphicsPipeline>,
    descriptor_set_layouts: Vec<VkDescriptorSetLayout>,
    descriptor_pools: Vec<VkDescriptorPool>,
    descriptor_sets: Vec<VkDescriptorSet>,
    fences: Vec<VkFence>,
    semaphores: Vec<VkSemaphore>,
    framebuffers: Vec<VkFramebuffer>,
    command_pools: Vec<VkCommandPool>,
    barriers: Vec<VkMemoryBarrier>,
    next_instance_id: u32,
    next_phys_id: u32,
    next_device_id: u32,
    next_rp_id: u32,
    next_layout_id: u32,
    next_pipeline_id: u32,
    next_dsl_id: u32,
    next_pool_id: u32,
    next_ds_id: u32,
    next_fence_id: u32,
    next_sem_id: u32,
    next_fb_id: u32,
    next_cp_id: u32,
}

static VULKAN: Mutex<Option<VulkanManager>> = Mutex::new(None);

impl VulkanManager {
    fn new() -> Self {
        VulkanManager {
            instances: Vec::new(),
            physical_devices: Vec::new(),
            queue_families: Vec::new(),
            devices: Vec::new(),
            render_passes: Vec::new(),
            pipeline_layouts: Vec::new(),
            graphics_pipelines: Vec::new(),
            descriptor_set_layouts: Vec::new(),
            descriptor_pools: Vec::new(),
            descriptor_sets: Vec::new(),
            fences: Vec::new(),
            semaphores: Vec::new(),
            framebuffers: Vec::new(),
            command_pools: Vec::new(),
            barriers: Vec::new(),
            next_instance_id: 1,
            next_phys_id: 1,
            next_device_id: 1,
            next_rp_id: 1,
            next_layout_id: 1,
            next_pipeline_id: 1,
            next_dsl_id: 1,
            next_pool_id: 1,
            next_ds_id: 1,
            next_fence_id: 1,
            next_sem_id: 1,
            next_fb_id: 1,
            next_cp_id: 1,
        }
    }

    // ── Instance management ───────────────────────────────────────────────

    fn create_instance(&mut self, app_name: &[u8], engine_name: &[u8],
                       api_version: VkApiVersion) -> Result<u32, VkResult> {
        let id = self.next_instance_id;
        self.next_instance_id = self.next_instance_id.saturating_add(1);
        let mut aname = [0u8; 64];
        let alen = app_name.len().min(64);
        aname[..alen].copy_from_slice(&app_name[..alen]);
        let mut ename = [0u8; 64];
        let elen = engine_name.len().min(64);
        ename[..elen].copy_from_slice(&engine_name[..elen]);

        self.instances.push(VkInstance {
            id,
            api_version,
            app_name: aname,
            app_name_len: alen,
            engine_name: ename,
            engine_name_len: elen,
            enabled_layers: Vec::new(),
            enabled_extensions: Vec::new(),
            physical_devices: Vec::new(),
            debug_enabled: false,
        });

        // Enumerate a software physical device for this instance
        let phys_id = self.register_software_device();
        if let Some(inst) = self.instances.iter_mut().find(|i| i.id == id) {
            inst.physical_devices.push(phys_id);
        }

        Ok(id)
    }

    fn register_software_device(&mut self) -> u32 {
        let id = self.next_phys_id;
        self.next_phys_id = self.next_phys_id.saturating_add(1);
        let mut name = [0u8; 64];
        let n = b"Genesis Software GPU";
        name[..n.len()].copy_from_slice(n);

        self.physical_devices.push(VkPhysicalDevice {
            id,
            device_type: VkPhysicalDeviceType::Cpu,
            vendor_id: 0x0001,
            device_id: 0x0001,
            name,
            name_len: n.len(),
            max_image_dim_2d: 4096,
            max_image_dim_3d: 256,
            max_viewports: 16,
            max_framebuffer_width: 4096,
            max_framebuffer_height: 4096,
            max_push_constant_size: 128,
            max_bound_descriptor_sets: 8,
            max_per_stage_descriptor_samplers: 16,
            max_vertex_input_attributes: 16,
            max_compute_workgroup_count: [65535, 65535, 65535],
            max_compute_workgroup_size: [256, 256, 64],
            max_compute_workgroup_invocations: 256,
            timestamp_period_q16: q16_from_int(1),
            queue_family_count: 3,
        });

        // Register queue families for this device
        let base_idx = self.queue_families.len() as u32;
        self.queue_families.push(VkQueueFamily {
            index: base_idx,
            flags: VkQueueFlags {
                graphics: true, compute: true, transfer: true,
                sparse_binding: false, protected: false,
            },
            queue_count: 1,
            timestamp_valid_bits: 64,
            min_image_transfer_granularity: [1, 1, 1],
        });
        self.queue_families.push(VkQueueFamily {
            index: base_idx + 1,
            flags: VkQueueFlags {
                graphics: false, compute: true, transfer: true,
                sparse_binding: false, protected: false,
            },
            queue_count: 2,
            timestamp_valid_bits: 64,
            min_image_transfer_granularity: [1, 1, 1],
        });
        self.queue_families.push(VkQueueFamily {
            index: base_idx + 2,
            flags: VkQueueFlags {
                graphics: false, compute: false, transfer: true,
                sparse_binding: false, protected: false,
            },
            queue_count: 1,
            timestamp_valid_bits: 64,
            min_image_transfer_granularity: [1, 1, 1],
        });

        id
    }

    // ── Logical device creation ───────────────────────────────────────────

    fn create_device(&mut self, physical_device_id: u32,
                     queue_requests: &[(u32, u32, Q16)]) -> Result<u32, VkResult> {
        let _phys = self.physical_devices.iter().find(|p| p.id == physical_device_id)
            .ok_or(VkResult::ErrorInitFailed)?;

        let id = self.next_device_id;
        self.next_device_id = self.next_device_id.saturating_add(1);

        let mut queues = Vec::new();
        for &(family, count, priority) in queue_requests {
            for qi in 0..count {
                queues.push(VkDeviceQueue {
                    family_index: family,
                    queue_index: qi,
                    priority_q16: priority,
                });
            }
        }

        self.devices.push(VkDevice {
            id,
            physical_device: physical_device_id,
            queues,
            enabled_features: VkDeviceFeatures {
                geometry_shader: true,
                tessellation_shader: true,
                multi_draw_indirect: true,
                sampler_anisotropy: true,
                texture_compression_bc: false,
                vertex_pipeline_stores_and_atomics: true,
                fragment_stores_and_atomics: true,
                shader_int64: true,
                shader_float64: false,
                sparse_binding: false,
                multi_viewport: true,
                wide_lines: true,
                large_points: true,
                fill_mode_non_solid: true,
                depth_clamp: true,
                depth_bias_clamp: true,
            },
            enabled_extensions: Vec::new(),
        });

        Ok(id)
    }

    // ── Render pass ───────────────────────────────────────────────────────

    fn create_render_pass(&mut self, attachments: Vec<VkAttachmentDescription>,
                          subpasses: Vec<VkSubpassDescription>,
                          dependencies: Vec<VkSubpassDependency>) -> u32 {
        let id = self.next_rp_id;
        self.next_rp_id = self.next_rp_id.saturating_add(1);
        self.render_passes.push(VkRenderPass { id, attachments, subpasses, dependencies });
        id
    }

    // ── Pipeline layout ───────────────────────────────────────────────────

    fn create_pipeline_layout(&mut self, set_layouts: &[u32],
                              push_constant_offset: u32,
                              push_constant_size: u32) -> u32 {
        let id = self.next_layout_id;
        self.next_layout_id = self.next_layout_id.saturating_add(1);
        let mut desc = Vec::new();
        desc.extend_from_slice(set_layouts);
        self.pipeline_layouts.push(VkPipelineLayout {
            id,
            descriptor_set_layouts: desc,
            push_constant_offset,
            push_constant_size,
        });
        id
    }

    // ── Graphics pipeline ─────────────────────────────────────────────────

    fn create_graphics_pipeline(&mut self, layout_id: u32, render_pass_id: u32,
                                vs: u32, fs: u32, topology: u32,
                                depth_test: bool) -> u32 {
        let id = self.next_pipeline_id;
        self.next_pipeline_id = self.next_pipeline_id.saturating_add(1);
        self.graphics_pipelines.push(VkGraphicsPipeline {
            id,
            layout_id,
            render_pass_id,
            subpass_index: 0,
            vertex_shader_id: vs,
            fragment_shader_id: fs,
            geometry_shader_id: None,
            tess_ctrl_shader_id: None,
            tess_eval_shader_id: None,
            topology,
            primitive_restart: false,
            depth_test_enable: depth_test,
            depth_write_enable: depth_test,
            depth_compare_op: 1, // less
            stencil_test_enable: false,
            blend_enable: false,
            cull_mode: 2, // back
            front_face: 0, // counter-clockwise
            polygon_mode: 0, // fill
            line_width_q16: Q16_ONE,
            dynamic_states: Vec::new(),
        });
        id
    }

    // ── Descriptor set layout ─────────────────────────────────────────────

    fn create_descriptor_set_layout(&mut self,
                                    bindings: Vec<VkDescriptorSetLayoutBinding>) -> u32 {
        let id = self.next_dsl_id;
        self.next_dsl_id = self.next_dsl_id.saturating_add(1);
        self.descriptor_set_layouts.push(VkDescriptorSetLayout { id, bindings });
        id
    }

    // ── Descriptor pool ───────────────────────────────────────────────────

    fn create_descriptor_pool(&mut self, max_sets: u32,
                              pool_sizes: Vec<(VkDescriptorType, u32)>) -> u32 {
        let id = self.next_pool_id;
        self.next_pool_id = self.next_pool_id.saturating_add(1);
        self.descriptor_pools.push(VkDescriptorPool {
            id, max_sets, allocated_sets: 0, pool_sizes,
        });
        id
    }

    // ── Descriptor set allocation ─────────────────────────────────────────

    fn allocate_descriptor_set(&mut self, pool_id: u32, layout_id: u32) -> Result<u32, VkResult> {
        let pool = self.descriptor_pools.iter_mut().find(|p| p.id == pool_id)
            .ok_or(VkResult::ErrorOutOfHostMemory)?;
        if pool.allocated_sets >= pool.max_sets {
            return Err(VkResult::ErrorTooManyObjects);
        }
        pool.allocated_sets = pool.allocated_sets.saturating_add(1);

        let id = self.next_ds_id;
        self.next_ds_id = self.next_ds_id.saturating_add(1);
        self.descriptor_sets.push(VkDescriptorSet {
            id, layout_id, pool_id,
            bindings: Vec::new(),
        });
        Ok(id)
    }

    fn update_descriptor_set(&mut self, set_id: u32, binding: u32,
                             desc_type: VkDescriptorType,
                             buffer_handle: u64, offset: u64, range: u64) {
        if let Some(ds) = self.descriptor_sets.iter_mut().find(|d| d.id == set_id) {
            // Replace existing or push new
            if let Some(b) = ds.bindings.iter_mut().find(|b| b.binding == binding) {
                b.descriptor_type = desc_type;
                b.buffer_handle = buffer_handle;
                b.offset = offset;
                b.range = range;
            } else {
                ds.bindings.push(VkDescriptorBinding {
                    binding, descriptor_type: desc_type,
                    buffer_handle, offset, range,
                    image_handle: 0, sampler_handle: 0,
                });
            }
        }
    }

    // ── Synchronization ───────────────────────────────────────────────────

    fn create_fence(&mut self, signaled: bool) -> u32 {
        let id = self.next_fence_id;
        self.next_fence_id = self.next_fence_id.saturating_add(1);
        self.fences.push(VkFence { id, signaled, device_id: 0 });
        id
    }

    fn create_semaphore(&mut self, timeline: bool) -> u32 {
        let id = self.next_sem_id;
        self.next_sem_id = self.next_sem_id.saturating_add(1);
        self.semaphores.push(VkSemaphore {
            id, signaled: false, timeline_value: 0, is_timeline: timeline,
        });
        id
    }

    fn wait_fence(&mut self, fence_id: u32) -> VkResult {
        if let Some(f) = self.fences.iter().find(|f| f.id == fence_id) {
            if f.signaled { VkResult::Success } else { VkResult::NotReady }
        } else {
            VkResult::ErrorDeviceLost
        }
    }

    fn reset_fence(&mut self, fence_id: u32) -> VkResult {
        if let Some(f) = self.fences.iter_mut().find(|f| f.id == fence_id) {
            f.signaled = false;
            VkResult::Success
        } else {
            VkResult::ErrorDeviceLost
        }
    }

    fn signal_fence(&mut self, fence_id: u32) {
        if let Some(f) = self.fences.iter_mut().find(|f| f.id == fence_id) {
            f.signaled = true;
        }
    }

    fn signal_semaphore(&mut self, sem_id: u32) {
        if let Some(s) = self.semaphores.iter_mut().find(|s| s.id == sem_id) {
            s.signaled = true;
            if s.is_timeline {
                s.timeline_value = s.timeline_value.saturating_add(1);
            }
        }
    }

    fn wait_semaphore(&mut self, sem_id: u32) -> VkResult {
        if let Some(s) = self.semaphores.iter_mut().find(|s| s.id == sem_id) {
            if s.signaled {
                if !s.is_timeline { s.signaled = false; }
                VkResult::Success
            } else {
                VkResult::NotReady
            }
        } else {
            VkResult::ErrorDeviceLost
        }
    }

    // ── Pipeline barrier ──────────────────────────────────────────────────

    fn pipeline_barrier(&mut self, src_stage: VkPipelineStage,
                        dst_stage: VkPipelineStage,
                        src_access: u32, dst_access: u32) {
        self.barriers.push(VkMemoryBarrier {
            src_stage, dst_stage, src_access, dst_access,
        });
    }

    // ── Framebuffer ───────────────────────────────────────────────────────

    fn create_framebuffer(&mut self, render_pass_id: u32, attachments: &[u32],
                          width: u32, height: u32, layers: u32) -> u32 {
        let id = self.next_fb_id;
        self.next_fb_id = self.next_fb_id.saturating_add(1);
        let mut att = Vec::new();
        att.extend_from_slice(attachments);
        self.framebuffers.push(VkFramebuffer {
            id, render_pass_id, attachments: att, width, height, layers,
        });
        id
    }

    // ── Command pool ──────────────────────────────────────────────────────

    fn create_command_pool(&mut self, queue_family: u32, transient: bool,
                           reset_individual: bool) -> u32 {
        let id = self.next_cp_id;
        self.next_cp_id = self.next_cp_id.saturating_add(1);
        self.command_pools.push(VkCommandPool {
            id,
            queue_family_index: queue_family,
            transient,
            reset_individual,
            allocated_buffers: Vec::new(),
        });
        id
    }

    fn reset_command_pool(&mut self, pool_id: u32) {
        if let Some(pool) = self.command_pools.iter_mut().find(|p| p.id == pool_id) {
            pool.allocated_buffers.clear();
        }
    }

    // ── Destroy helpers ───────────────────────────────────────────────────

    fn destroy_pipeline(&mut self, pipeline_id: u32) {
        if let Some(idx) = self.graphics_pipelines.iter().position(|p| p.id == pipeline_id) {
            self.graphics_pipelines.remove(idx);
        }
    }

    fn destroy_render_pass(&mut self, rp_id: u32) {
        if let Some(idx) = self.render_passes.iter().position(|r| r.id == rp_id) {
            self.render_passes.remove(idx);
        }
    }

    fn destroy_framebuffer(&mut self, fb_id: u32) {
        if let Some(idx) = self.framebuffers.iter().position(|f| f.id == fb_id) {
            self.framebuffers.remove(idx);
        }
    }

    fn destroy_descriptor_pool(&mut self, pool_id: u32) {
        // Remove all sets from this pool
        self.descriptor_sets.retain(|ds| ds.pool_id != pool_id);
        if let Some(idx) = self.descriptor_pools.iter().position(|p| p.id == pool_id) {
            self.descriptor_pools.remove(idx);
        }
    }

    fn destroy_fence(&mut self, fence_id: u32) {
        if let Some(idx) = self.fences.iter().position(|f| f.id == fence_id) {
            self.fences.remove(idx);
        }
    }

    fn destroy_semaphore(&mut self, sem_id: u32) {
        if let Some(idx) = self.semaphores.iter().position(|s| s.id == sem_id) {
            self.semaphores.remove(idx);
        }
    }

    fn destroy_device(&mut self, device_id: u32) {
        if let Some(idx) = self.devices.iter().position(|d| d.id == device_id) {
            self.devices.remove(idx);
        }
    }

    fn destroy_instance(&mut self, instance_id: u32) {
        if let Some(idx) = self.instances.iter().position(|i| i.id == instance_id) {
            self.instances.remove(idx);
        }
    }
}

pub fn init() {
    let mut vk = VULKAN.lock();
    let mut mgr = VulkanManager::new();
    // Create default instance and device
    let _ = mgr.create_instance(b"Genesis", b"GenesisEngine", VkApiVersion::V13);
    if let Some(phys_id) = mgr.physical_devices.first().map(|p| p.id) {
        let _ = mgr.create_device(phys_id, &[(0, 1, Q16_ONE), (1, 1, q16_div(Q16_ONE, q16_from_int(2)))]);
    }
    *vk = Some(mgr);
    serial_println!("    GPU: Vulkan-like API (instance, device, pipelines, descriptors, sync) ready");
}
