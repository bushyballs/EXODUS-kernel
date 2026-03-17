use crate::ml::ops;

pub const MAX_MODEL_LAYERS: usize = 8;
pub const MAX_INFER_BUFFER: usize = 64;

pub const ACT_NONE: u8 = 0;
pub const ACT_RELU: u8 = 1;
pub const ACT_SOFTMAX: u8 = 2;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ModelLayer {
    pub weight_offset: usize,
    pub bias_offset: usize,
    pub in_size: usize,
    pub out_size: usize,
    pub activation: u8,
}

impl ModelLayer {
    pub const fn empty() -> Self {
        Self {
            weight_offset: 0,
            bias_offset: 0,
            in_size: 0,
            out_size: 0,
            activation: ACT_NONE,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ModelDef {
    pub layers: [ModelLayer; MAX_MODEL_LAYERS],
    pub layer_count: u8,
    pub total_weight_bytes: usize,
}

impl ModelDef {
    pub const fn empty() -> Self {
        Self {
            layers: [ModelLayer::empty(); MAX_MODEL_LAYERS],
            layer_count: 0,
            total_weight_bytes: 0,
        }
    }
}

pub const ANOMALY_MODEL: ModelDef = ModelDef {
    layers: [
        ModelLayer {
            weight_offset: 0,
            bias_offset: 512,
            in_size: 32,
            out_size: 16,
            activation: ACT_RELU,
        },
        ModelLayer {
            weight_offset: 528,
            bias_offset: 656,
            in_size: 16,
            out_size: 8,
            activation: ACT_RELU,
        },
        ModelLayer {
            weight_offset: 664,
            bias_offset: 680,
            in_size: 8,
            out_size: 2,
            activation: ACT_NONE,
        },
        ModelLayer::empty(),
        ModelLayer::empty(),
        ModelLayer::empty(),
        ModelLayer::empty(),
        ModelLayer::empty(),
    ],
    layer_count: 3,
    total_weight_bytes: 682,
};

pub static ANOMALY_WEIGHTS: [i8; 682] = [
    10, 12, 10, -6, -15, 10, -10, 15, -15, 14, 6, 12, -13, -14, -7, -15, -12, 14, -8, 5, 0, 13,
    -10, 14, -15, 1, 14, -11, -3, 12, 5, 10, -4, -14, 12, -10, 9, -14, -1, 1, -1, -1, 12, 13, 1,
    -10, 1, 2, 10, -14, -12, 10, -13, -12, 12, 1, 6, -13, -13, 11, -11, 10, -5, 3, -6, -1, 1, 9, 4,
    -5, 15, -4, -7, -8, 14, -12, 12, 14, -7, 13, -7, -13, -12, 13, 15, -9, -1, 7, -3, 11, 14, -14,
    -15, 11, 0, -12, 12, -2, -12, -10, -3, -8, 15, 10, -15, 4, 1, -2, -12, 4, 1, 13, -8, 1, 4, 10,
    1, 7, 13, 8, -12, 8, -3, -7, 9, 3, -14, 5, -4, -2, -12, -14, 8, 1, 7, 7, 13, 7, 8, -8, 8, 6,
    -14, -9, -10, 2, -8, -4, 7, 6, -4, -9, -13, 10, 1, 7, -5, 2, 6, 1, -7, 3, 6, 5, 15, 4, 7, -3,
    11, -13, -2, 8, -3, 7, -6, 15, -10, 1, 5, 5, -3, -2, -14, 14, -11, -3, 6, -11, 7, -2, 14, 11,
    12, -12, -6, -7, -5, -8, -5, -5, -15, 3, 8, -1, 4, -11, -10, -2, -10, 10, 3, 12, -5, 8, 6, 12,
    -13, 14, -7, 10, -5, 1, -3, -14, -14, -7, 9, 11, 6, 12, -12, 8, 7, -13, -12, -14, 15, -2, -3,
    -11, -2, 11, -8, -12, 6, 12, -13, 10, 4, 9, 1, 3, 3, -3, 9, 14, -10, -8, -2, 7, 3, 7, 11, 14,
    0, 5, -10, -10, 8, -12, 13, -12, 8, -10, -13, 5, -8, -5, 7, -4, 11, 6, -4, -10, 15, -10, 12,
    11, 10, 13, -2, -2, 2, 7, 5, -11, 5, 0, 7, 11, 0, 6, 10, 15, 10, 14, 14, -5, -8, 6, -4, 11, -6,
    -14, -4, -10, 13, -7, 14, -8, 10, 0, 9, -4, 13, -12, 12, 4, 4, 0, 7, 5, -1, -1, -14, -13, 9,
    -8, 9, 7, 1, -6, 3, 10, -8, -11, -11, 14, -8, 0, -14, -4, 3, -10, -3, -8, -15, -8, 11, 9, -13,
    -11, -6, -4, -2, -1, -1, -10, 11, 0, -15, 11, -4, -10, 15, 15, 4, -11, 2, -12, 5, -4, 12, -9,
    14, -15, -1, 3, -2, -8, -5, 7, 4, 12, -11, -9, -14, -4, -14, -13, 12, 4, 9, 1, -12, -15, -9,
    -8, 2, 10, 5, 10, -8, 10, 12, -1, -2, 5, -5, 7, 9, -5, 2, -4, -10, -5, 4, 6, -12, -11, 12, -3,
    0, 14, 0, -2, 15, 3, -3, -10, -3, -11, -12, -7, 5, -6, -14, -1, 9, -15, -1, -8, 11, -4, -12,
    -5, 9, -1, 14, 3, -7, -4, 4, -15, -6, 3, -7, 13, 14, -7, 4, -3, 11, -9, -6, -6, -14, -13, -11,
    14, -13, -1, -13, -10, -2, 6, -15, 14, -14, 6, 6, -11, 1, -12, -6, 11, -11, -10, -14, -7, 6, 2,
    1, 9, 12, -2, -7, 3, 13, -14, -3, 9, -15, 12, 8, 5, -2, 7, -7, 4, 11, -2, 10, -7, -8, 2, 15,
    -10, -3, -3, -6, 0, 13, 1, -12, -6, -2, 15, -6, 9, 14, -7, 14, 8, 8, 14, -14, 4, -9, 11, -3,
    -7, -4, 3, 13, 0, -7, 11, 15, 9, -10, 8, 8, -4, 9, -4, -2, -1, 13, 2, -2, 0, 7, -1, 5, 7, -12,
    -5, 10, -11, 0, -15, -2, 13, 5, 3, 9, -2, 0, 10, -15, -11, -8, -9, 1, 0, 11, 5, 15, -13, 1, 15,
    -7, 10, -2, -12, 1, 2, -7, 14, 9, 8, 12, -9, -7, -13, 13, -1, 8, -5, -13, -10, 10, -15, 10, 11,
    1, -9, 3, 12, 10, -11, 0, 3, 11, 2, -15, -15, 5, -12, -5, -15, -14, 3, 8, -10, 8, 11, -5, -6,
    -12, 12, -6, -5, -10, 2, -12, 1, -4, -9, 15, 5, 10, -9, 0, -5, 5, 1, 12, 4, 9, 9, -15, -14, 1,
    -7, -11, 15, 15, 13, 6, 5,
];

fn clamp_i8(value: i32) -> i8 {
    if value > i8::MAX as i32 {
        i8::MAX
    } else if value < i8::MIN as i32 {
        i8::MIN
    } else {
        value as i8
    }
}

fn run_layer(
    input: &[i8],
    output: &mut [i8; MAX_INFER_BUFFER],
    layer: ModelLayer,
    weights: &[i8],
) -> bool {
    if layer.in_size == 0 || layer.out_size == 0 {
        return false;
    }
    if layer.in_size > MAX_INFER_BUFFER || layer.out_size > MAX_INFER_BUFFER {
        return false;
    }
    if input.len() < layer.in_size {
        return false;
    }

    let weight_count = match layer.in_size.checked_mul(layer.out_size) {
        Some(v) => v,
        None => return false,
    };
    let bias_count = layer.out_size;

    let weight_end = match layer.weight_offset.checked_add(weight_count) {
        Some(v) => v,
        None => return false,
    };
    let bias_end = match layer.bias_offset.checked_add(bias_count) {
        Some(v) => v,
        None => return false,
    };
    if weight_end > weights.len() || bias_end > weights.len() {
        return false;
    }

    let layer_weights = &weights[layer.weight_offset..weight_end];
    let layer_bias = &weights[layer.bias_offset..bias_end];
    let out_slice = &mut output[..layer.out_size];

    ops::matmul_int8(
        &input[..layer.in_size],
        layer_weights,
        out_slice,
        1,
        layer.in_size,
        layer.out_size,
        0,
        0,
        0,
        256,
        8,
    );

    let mut i = 0usize;
    while i < layer.out_size {
        let with_bias = (out_slice[i] as i32).saturating_add(layer_bias[i] as i32);
        out_slice[i] = clamp_i8(with_bias);
        i = i.saturating_add(1);
    }

    if layer.activation == ACT_RELU {
        ops::relu_i8(out_slice);
    } else if layer.activation == ACT_SOFTMAX {
        let mut probs = [0u16; MAX_INFER_BUFFER];
        ops::softmax_fixed(out_slice, &mut probs[..layer.out_size], layer.out_size);
        let mut j = 0usize;
        while j < layer.out_size {
            let q = (probs[j] as u32).saturating_mul(127) / 65535;
            out_slice[j] = q as i8;
            j = j.saturating_add(1);
        }
    }

    true
}

pub fn inference(input: &[i8], model: &ModelDef, weights: &[i8], output: &mut [i8]) -> usize {
    let layer_count = model.layer_count as usize;
    if layer_count == 0 || layer_count > MAX_MODEL_LAYERS {
        return 0;
    }

    let first_in = model.layers[0].in_size;
    if first_in == 0 || first_in > MAX_INFER_BUFFER || input.len() < first_in {
        return 0;
    }

    let mut buf_a = [0i8; MAX_INFER_BUFFER];
    let mut buf_b = [0i8; MAX_INFER_BUFFER];

    let mut i = 0usize;
    while i < first_in {
        buf_a[i] = input[i];
        i = i.saturating_add(1);
    }

    let mut current_len = first_in;
    let mut current_is_a = true;
    let mut layer_idx = 0usize;
    while layer_idx < layer_count {
        let layer = model.layers[layer_idx];
        if layer.in_size != current_len {
            return 0;
        }

        let ok = if current_is_a {
            run_layer(&buf_a[..current_len], &mut buf_b, layer, weights)
        } else {
            run_layer(&buf_b[..current_len], &mut buf_a, layer, weights)
        };
        if !ok {
            return 0;
        }

        current_len = layer.out_size;
        current_is_a = !current_is_a;
        layer_idx = layer_idx.saturating_add(1);
    }

    let final_buf = if current_is_a { &buf_a } else { &buf_b };
    let copy_len = core::cmp::min(current_len, output.len());
    let mut out_i = 0usize;
    while out_i < copy_len {
        output[out_i] = final_buf[out_i];
        out_i = out_i.saturating_add(1);
    }
    copy_len
}
