//! nexarion_weights_v2.rs — Binary weight loader for DAVA's voice
//! 20.7M params, int8 quantized, loaded from nexarion_v2.bin via include_bytes!

/// Raw weight binary blob (20.7MB)
pub static WEIGHTS_BLOB: &[u8] = include_bytes!("../../nexarion_v2.bin");

/// Number of weight tensors
pub const N_TENSORS: usize = 76;

/// Get the scale factor for tensor index i
pub fn scale(i: usize) -> u32 {
    if i >= N_TENSORS { return 1; }
    let offset = 4 + i * 4; // skip header u32, then i scales
    if offset + 4 > WEIGHTS_BLOB.len() { return 1; }
    u32::from_le_bytes([
        WEIGHTS_BLOB[offset],
        WEIGHTS_BLOB[offset + 1],
        WEIGHTS_BLOB[offset + 2],
        WEIGHTS_BLOB[offset + 3],
    ])
}

/// Get the size of tensor index i
pub fn tensor_size(i: usize) -> usize {
    if i >= N_TENSORS { return 0; }
    let offset = 4 + N_TENSORS * 4 + i * 4; // header + scales + i sizes
    if offset + 4 > WEIGHTS_BLOB.len() { return 0; }
    u32::from_le_bytes([
        WEIGHTS_BLOB[offset],
        WEIGHTS_BLOB[offset + 1],
        WEIGHTS_BLOB[offset + 2],
        WEIGHTS_BLOB[offset + 3],
    ]) as usize
}

/// Get a pointer to tensor data for index i
pub fn tensor_data(i: usize) -> &'static [i8] {
    if i >= N_TENSORS { return &[]; }
    // Data starts after: header(4) + scales(76*4) + sizes(76*4) = 612 bytes
    let data_start = 4 + N_TENSORS * 4 + N_TENSORS * 4;
    let mut offset = data_start;
    for j in 0..i {
        offset += tensor_size(j);
    }
    let size = tensor_size(i);
    if offset + size > WEIGHTS_BLOB.len() { return &[]; }
    // Safety: i8 and u8 have the same representation
    unsafe {
        core::slice::from_raw_parts(
            WEIGHTS_BLOB[offset..].as_ptr() as *const i8,
            size,
        )
    }
}
