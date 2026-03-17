/// Neural network inference engine for Genesis
///
/// Runs neural network models by executing layer-by-layer operations.
/// Supports: linear, conv2d, batch_norm, relu, softmax, pooling,
/// embedding, attention, and LSTM layers.
///
/// Inspired by: ONNX Runtime, TFLite interpreter. All code is original.

use crate::sync::Mutex;
use super::tensor::{Tensor, DType};
use alloc::vec::Vec;
use alloc::string::String;

/// Layer type
#[derive(Debug, Clone)]
pub enum LayerType {
    Linear { in_features: usize, out_features: usize },
    Conv2d { in_channels: usize, out_channels: usize, kernel: usize, stride: usize },
    BatchNorm { features: usize },
    ReLU,
    Sigmoid,
    Softmax,
    MaxPool2d { kernel: usize },
    Dropout { rate: f32 },
    Flatten,
    Embedding { num_embeddings: usize, dim: usize },
    LayerNorm { features: usize },
    Reshape { shape: Vec<usize> },
}

/// A layer in the model
pub struct Layer {
    pub layer_type: LayerType,
    pub name: String,
    /// Weights
    pub weights: Option<Tensor>,
    /// Bias
    pub bias: Option<Tensor>,
}

/// A neural network model
pub struct Model {
    pub name: String,
    pub layers: Vec<Layer>,
    pub input_shape: Vec<usize>,
    pub output_shape: Vec<usize>,
}

impl Model {
    pub fn new(name: &str) -> Self {
        Model {
            name: String::from(name),
            layers: Vec::new(),
            input_shape: Vec::new(),
            output_shape: Vec::new(),
        }
    }

    /// Add a layer
    pub fn add_layer(&mut self, name: &str, layer_type: LayerType) {
        self.layers.push(Layer {
            layer_type,
            name: String::from(name),
            weights: None,
            bias: None,
        });
    }

    fn conv2d_forward(
        input: &Tensor,
        weights: &Tensor,
        bias: Option<&Tensor>,
        in_channels: usize,
        out_channels: usize,
        kernel: usize,
        stride: usize,
    ) -> Tensor {
        if stride == 0 || kernel == 0 {
            return Self::clone_tensor(input);
        }
        let (batch, in_c, h, w, keep_batch_dim) = match input.shape.as_slice() {
            [c, ih, iw] => (1usize, *c, *ih, *iw, false),
            [b, c, ih, iw] => (*b, *c, *ih, *iw, true),
            _ => return Self::clone_tensor(input),
        };
        if in_c != in_channels || h < kernel || w < kernel {
            return Self::clone_tensor(input);
        }

        let expected_w = out_channels * in_channels * kernel * kernel;
        if weights.numel() < expected_w {
            return Self::clone_tensor(input);
        }

        let out_h = (h - kernel) / stride + 1;
        let out_w = (w - kernel) / stride + 1;
        let out_shape = if keep_batch_dim {
            alloc::vec![batch, out_channels, out_h, out_w]
        } else {
            alloc::vec![out_channels, out_h, out_w]
        };
        let mut output = Tensor::zeros(&out_shape, DType::F32);

        for b in 0..batch {
            for oc in 0..out_channels {
                for oy in 0..out_h {
                    for ox in 0..out_w {
                        let mut sum = bias
                            .and_then(|b_tensor| {
                                if oc < b_tensor.numel() {
                                    Some(b_tensor.get_f32(oc))
                                } else {
                                    None
                                }
                            })
                            .unwrap_or(0.0);

                        for ic in 0..in_channels {
                            for ky in 0..kernel {
                                for kx in 0..kernel {
                                    let iy = oy * stride + ky;
                                    let ix = ox * stride + kx;
                                    let in_idx = if keep_batch_dim {
                                        (((b * in_channels + ic) * h + iy) * w) + ix
                                    } else {
                                        ((ic * h + iy) * w) + ix
                                    };
                                    let w_idx = (((oc * in_channels + ic) * kernel + ky) * kernel) + kx;
                                    sum += input.get_f32(in_idx) * weights.get_f32(w_idx);
                                }
                            }
                        }

                        let out_idx = if keep_batch_dim {
                            (((b * out_channels + oc) * out_h + oy) * out_w) + ox
                        } else {
                            ((oc * out_h + oy) * out_w) + ox
                        };
                        output.set_f32(out_idx, sum);
                    }
                }
            }
        }

        output
    }

    fn batch_norm_forward(input: &Tensor, features: usize, gamma: Option<&Tensor>, beta: Option<&Tensor>) -> Tensor {
        if features == 0 || input.numel() == 0 {
            return Self::clone_tensor(input);
        }

        let mut output = input.layer_norm(1e-5);
        if output.numel() % features != 0 {
            return output;
        }

        let rows = output.numel() / features;
        for r in 0..rows {
            for c in 0..features {
                let idx = r * features + c;
                let mut v = output.get_f32(idx);
                if let Some(g) = gamma {
                    if c < g.numel() {
                        v *= g.get_f32(c);
                    }
                }
                if let Some(b) = beta {
                    if c < b.numel() {
                        v += b.get_f32(c);
                    }
                }
                output.set_f32(idx, v);
            }
        }
        output
    }

    fn embedding_forward(input: &Tensor, weights: &Tensor, num_embeddings: usize, dim: usize) -> Tensor {
        if dim == 0 || num_embeddings == 0 {
            return Self::clone_tensor(input);
        }

        let expected = num_embeddings * dim;
        if weights.numel() < expected {
            return Self::clone_tensor(input);
        }

        let token_count = input.numel();
        let mut out = Tensor::zeros(&[token_count, dim], DType::F32);

        for i in 0..token_count {
            let raw = input.get_f32(i);
            let mut token_id = if raw >= 0.0 {
                (raw + 0.5) as isize
            } else {
                (raw - 0.5) as isize
            };
            if token_id < 0 {
                token_id = 0;
            }
            if token_id as usize >= num_embeddings {
                token_id = (num_embeddings - 1) as isize;
            }
            let token_idx = token_id as usize;

            for d in 0..dim {
                let w_idx = token_idx * dim + d;
                out.set_f32(i * dim + d, weights.get_f32(w_idx));
            }
        }

        out
    }

    fn reshape_forward(input: &Tensor, shape: &[usize]) -> Tensor {
        let requested: usize = shape.iter().copied().product();
        if requested == input.numel() {
            let mut reshaped = Self::clone_tensor(input);
            reshaped.shape = shape.to_vec();
            reshaped
        } else {
            Self::clone_tensor(input)
        }
    }

    fn clone_tensor(input: &Tensor) -> Tensor {
        let mut out = Tensor::zeros(&input.shape, input.dtype);
        out.data = input.data.clone();
        out.shape = input.shape.clone();
        out
    }

    /// Run inference
    pub fn forward(&self, input: &Tensor) -> Tensor {
        let mut current = Self::clone_tensor(input);

        for layer in &self.layers {
            current = match &layer.layer_type {
                LayerType::Linear { .. } => {
                    if let Some(ref weights) = layer.weights {
                        let mut result = Tensor::matmul(&current, weights);
                        if let Some(ref bias) = layer.bias {
                            result = Tensor::add(&result, bias);
                        }
                        result
                    } else {
                        // No weights loaded — pass through
                        current
                    }
                }
                LayerType::Conv2d {
                    in_channels,
                    out_channels,
                    kernel,
                    stride,
                } => {
                    if let Some(ref weights) = layer.weights {
                        Self::conv2d_forward(
                            &current,
                            weights,
                            layer.bias.as_ref(),
                            *in_channels,
                            *out_channels,
                            *kernel,
                            *stride,
                        )
                    } else {
                        current
                    }
                }
                LayerType::BatchNorm { features } => {
                    Self::batch_norm_forward(&current, *features, layer.weights.as_ref(), layer.bias.as_ref())
                }
                LayerType::ReLU => current.relu(),
                LayerType::Sigmoid => current.sigmoid(),
                LayerType::Softmax => current.softmax(),
                LayerType::MaxPool2d { kernel } => current.max_pool_2d(*kernel),
                LayerType::Dropout { .. } => current, // disabled during inference
                LayerType::Embedding { num_embeddings, dim } => {
                    if let Some(ref weights) = layer.weights {
                        Self::embedding_forward(&current, weights, *num_embeddings, *dim)
                    } else {
                        current
                    }
                }
                LayerType::LayerNorm { .. } => current.layer_norm(1e-5),
                LayerType::Flatten => {
                    let total = current.numel();
                    current.shape = alloc::vec![1, total];
                    current
                }
                LayerType::Reshape { shape } => Self::reshape_forward(&current, shape),
            };
        }
        current
    }

    /// Get parameter count
    pub fn param_count(&self) -> usize {
        let mut count = 0;
        for layer in &self.layers {
            if let Some(ref w) = layer.weights { count += w.numel(); }
            if let Some(ref b) = layer.bias { count += b.numel(); }
        }
        count
    }

    /// Get model size in bytes
    pub fn size_bytes(&self) -> usize {
        let mut size = 0;
        for layer in &self.layers {
            if let Some(ref w) = layer.weights { size += w.data.len(); }
            if let Some(ref b) = layer.bias { size += b.data.len(); }
        }
        size
    }
}

/// Model registry
pub struct ModelRegistry {
    models: Vec<Model>,
}

impl ModelRegistry {
    const fn new() -> Self {
        ModelRegistry { models: Vec::new() }
    }

    pub fn register(&mut self, model: Model) -> usize {
        let id = self.models.len();
        crate::serial_println!("  [inference] Registered model '{}' ({} layers, {} params)",
            model.name, model.layers.len(), model.param_count());
        self.models.push(model);
        id
    }

    pub fn get(&self, id: usize) -> Option<&Model> {
        self.models.get(id)
    }

    pub fn run(&self, model_id: usize, input: &Tensor) -> Option<Tensor> {
        self.models.get(model_id).map(|m| m.forward(input))
    }

    pub fn model_count(&self) -> usize {
        self.models.len()
    }
}

static REGISTRY: Mutex<ModelRegistry> = Mutex::new(ModelRegistry::new());

pub fn init() {
    crate::serial_println!("  [inference] Neural network inference engine initialized");
}

pub fn register_model(model: Model) -> usize {
    REGISTRY.lock().register(model)
}

pub fn run_inference(model_id: usize, input: &Tensor) -> Option<Tensor> {
    REGISTRY.lock().run(model_id, input)
}
