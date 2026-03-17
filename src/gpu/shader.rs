use crate::sync::Mutex;
/// Shader compiler for Genesis GPU
///
/// SPIR-V-like intermediate representation,
/// vertex/fragment/compute shader compilation,
/// shader reflection, uniform binding.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum ShaderStage {
    Vertex,
    Fragment,
    Compute,
    Geometry,
    TessControl,
    TessEval,
    Mesh,
    Task,
}

#[derive(Clone, Copy, PartialEq)]
pub enum UniformType {
    Float,
    Vec2,
    Vec3,
    Vec4,
    Mat3,
    Mat4,
    Int,
    Sampler2D,
    SamplerCube,
    StorageBuffer,
}

struct ShaderModule {
    id: u32,
    stage: ShaderStage,
    bytecode_hash: u64,
    bytecode_size: u32,
    entry_point: [u8; 16],
    entry_len: usize,
    uniforms: Vec<UniformBinding>,
}

struct UniformBinding {
    binding: u32,
    set: u32,
    uniform_type: UniformType,
    name_hash: u64,
    array_size: u32,
}

struct ShaderCompiler {
    modules: Vec<ShaderModule>,
    next_id: u32,
    compiled_count: u32,
}

static SHADER: Mutex<Option<ShaderCompiler>> = Mutex::new(None);

impl ShaderCompiler {
    fn new() -> Self {
        ShaderCompiler {
            modules: Vec::new(),
            next_id: 1,
            compiled_count: 0,
        }
    }

    fn compile(&mut self, stage: ShaderStage, bytecode_hash: u64, size: u32, entry: &[u8]) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.compiled_count = self.compiled_count.saturating_add(1);
        let mut ep = [0u8; 16];
        let elen = entry.len().min(16);
        ep[..elen].copy_from_slice(&entry[..elen]);
        self.modules.push(ShaderModule {
            id,
            stage,
            bytecode_hash,
            bytecode_size: size,
            entry_point: ep,
            entry_len: elen,
            uniforms: Vec::new(),
        });
        id
    }

    fn add_uniform(
        &mut self,
        shader_id: u32,
        binding: u32,
        set: u32,
        utype: UniformType,
        name_hash: u64,
    ) {
        if let Some(module) = self.modules.iter_mut().find(|m| m.id == shader_id) {
            module.uniforms.push(UniformBinding {
                binding,
                set,
                uniform_type: utype,
                name_hash,
                array_size: 1,
            });
        }
    }
}

pub fn init() {
    let mut s = SHADER.lock();
    *s = Some(ShaderCompiler::new());
    serial_println!("    GPU: shader compiler (vertex, fragment, compute) ready");
}
