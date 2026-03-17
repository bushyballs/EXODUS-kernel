use crate::sync::Mutex;
/// Texture management for Genesis GPU
///
/// 2D/3D/cube textures, mipmaps, sampling,
/// texture compression, atlas packing.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum TextureType {
    Tex2D,
    Tex3D,
    TexCube,
    Tex2DArray,
}

#[derive(Clone, Copy, PartialEq)]
pub enum FilterMode {
    Nearest,
    Linear,
    NearestMipmapNearest,
    LinearMipmapLinear,
}

#[derive(Clone, Copy, PartialEq)]
pub enum WrapMode {
    Repeat,
    MirrorRepeat,
    ClampToEdge,
    ClampToBorder,
}

struct Texture {
    id: u32,
    tex_type: TextureType,
    width: u32,
    height: u32,
    depth: u32,
    mip_levels: u8,
    format: super::pipeline::Format,
    filter_min: FilterMode,
    filter_mag: FilterMode,
    wrap_u: WrapMode,
    wrap_v: WrapMode,
    memory_offset: u64,
    size_bytes: u64,
}

struct TextureManager {
    textures: Vec<Texture>,
    next_id: u32,
    total_memory: u64,
    max_memory: u64,
}

static TEXTURES: Mutex<Option<TextureManager>> = Mutex::new(None);

impl TextureManager {
    fn new() -> Self {
        TextureManager {
            textures: Vec::new(),
            next_id: 1,
            total_memory: 0,
            max_memory: 512 * 1024 * 1024, // 512MB texture budget
        }
    }

    fn create_texture(
        &mut self,
        tex_type: TextureType,
        w: u32,
        h: u32,
        d: u32,
        format: super::pipeline::Format,
        mipmaps: bool,
    ) -> Option<u32> {
        let bpp: u64 = match format {
            super::pipeline::Format::R8Unorm => 1,
            super::pipeline::Format::Rg8Unorm => 2,
            super::pipeline::Format::Rgba8Unorm
            | super::pipeline::Format::Rgba8Srgb
            | super::pipeline::Format::Bgra8Unorm => 4,
            super::pipeline::Format::Rgba16Float => 8,
            super::pipeline::Format::Rgba32Float => 16,
            _ => 4,
        };
        let base_size = w as u64 * h as u64 * d as u64 * bpp;
        let mip_levels = if mipmaps {
            let max_dim = w.max(h);
            (32 - max_dim.leading_zeros()) as u8
        } else {
            1
        };
        // Total with mipmaps ~= base * 4/3
        let total_size = if mipmaps {
            base_size * 4 / 3
        } else {
            base_size
        };

        if self.total_memory + total_size > self.max_memory {
            return None;
        }

        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let offset = self.total_memory;
        self.total_memory += total_size;
        self.textures.push(Texture {
            id,
            tex_type,
            width: w,
            height: h,
            depth: d,
            mip_levels,
            format,
            filter_min: FilterMode::LinearMipmapLinear,
            filter_mag: FilterMode::Linear,
            wrap_u: WrapMode::Repeat,
            wrap_v: WrapMode::Repeat,
            memory_offset: offset,
            size_bytes: total_size,
        });
        Some(id)
    }

    fn destroy_texture(&mut self, tex_id: u32) {
        if let Some(idx) = self.textures.iter().position(|t| t.id == tex_id) {
            self.total_memory -= self.textures[idx].size_bytes;
            self.textures.remove(idx);
        }
    }
}

pub fn init() {
    let mut t = TEXTURES.lock();
    *t = Some(TextureManager::new());
    serial_println!("    GPU: texture manager (2D/3D/cube, mipmaps) ready");
}
