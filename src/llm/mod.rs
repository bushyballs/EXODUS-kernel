pub mod attention;
pub mod batch;
pub mod cache_mgr;
pub mod flash_attn;
pub mod generate;
pub mod grammar;
pub mod hw_optimize;
pub mod integration;
pub mod kv_cache;
pub mod learning;
pub mod lora;
pub mod model_io;
pub mod model_registry;
pub mod moe;
pub mod os_customize;
pub mod preferences;
pub mod prompt_cache;
pub mod quantize;
pub mod rlhf;
pub mod rope;
pub mod sampling;
pub mod self_improve;
pub mod speculative;
pub mod system_prompt;
/// Hoags Local LLM Engine — Genesis-native AI
///
/// A full transformer-based language model built entirely
/// from scratch. No Ollama, no prebuilt weights, no external
/// dependencies. This is the Hoags Intelligence Core.
///
/// Architecture:
///   - BPE tokenizer (byte-pair encoding from scratch)
///   - Transformer decoder (GPT-style, causal attention)
///   - RoPE positional encoding (rotary position embeddings)
///   - KV-cache for fast autoregressive generation
///   - INT8/INT4 quantization for Pi/embedded deployment
///   - Speculative decoding for speed
///   - On-device training (backprop, AdamW, gradient accumulation)
///   - RLHF / preference learning
///   - Context window: 128K tokens (sliding window + RoPE)
///   - Custom model format (.hoags)
pub mod tokenizer;
pub mod training;
pub mod transformer;

use crate::{serial_print, serial_println};

pub fn init() {
    // Deferred: LLM subsystem allocates ~83MB during init which causes OOM
    // during early boot. Will be initialized on-demand when first needed.
    serial_println!("  LLM engine: deferred (heavy allocator, available on demand)");
}

pub fn init_full() {
    tokenizer::init();
    transformer::init();
    attention::init();
    kv_cache::init();
    quantize::init();
    training::init();
    rlhf::init();
    generate::init();
    model_io::init();
    system_prompt::init();
    learning::init();
    preferences::init();
    os_customize::init();
    hw_optimize::init();
    integration::init();
    self_improve::init();
    rope::init();
    flash_attn::init();
    speculative::init();
    moe::init();
    lora::init();
    cache_mgr::init();
    prompt_cache::init();
    batch::init();
    sampling::init();
    grammar::init();
    model_registry::init();
    serial_println!("  Hoags LLM engine initialized (transformer, BPE, RoPE, KV-cache, quantize, training, RLHF, generate, learning, preferences, os_customize, hw_optimize, integration, self-improve, rope, flash_attn, speculative, moe, lora, cache_mgr, prompt_cache, batch, sampling, grammar, model_registry)");
}
