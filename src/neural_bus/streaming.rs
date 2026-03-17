use crate::sync::Mutex;
/// Streaming — zero-latency data pipeline for Genesis Neural Bus
///
/// Ring buffers, stream channels, multi-stage inference pipeline,
/// data routing with transforms between neural nodes.
///
/// All Q16 fixed-point. No floats. No external deps.
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use super::{q16_from_int, q16_mul, Q16, Q16_HALF, Q16_ONE, Q16_ZERO};

// ── Constants ───────────────────────────────────────────────────────

const DEFAULT_BUFFER_CAPACITY: usize = 256;
const MAX_CHANNELS: usize = 64;
const MAX_PIPELINE_STAGES: usize = 16;

// ── Ring Buffer ─────────────────────────────────────────────────────

pub struct RingBuffer<T: Clone + Default> {
    pub data: Vec<T>,
    pub write_pos: usize,
    pub read_pos: usize,
    pub capacity: usize,
}

impl<T: Clone + Default> RingBuffer<T> {
    pub fn with_capacity(cap: usize) -> Self {
        let cap = if cap == 0 {
            DEFAULT_BUFFER_CAPACITY
        } else {
            cap
        };
        RingBuffer {
            data: Vec::new(),
            write_pos: 0,
            read_pos: 0,
            capacity: cap,
        }
    }

    pub fn push(&mut self, val: T) {
        if self.data.len() < self.capacity {
            self.data.push(val);
        } else {
            self.data[self.write_pos % self.capacity] = val;
        }
        self.write_pos += 1;
        if self.write_pos - self.read_pos > self.capacity {
            self.read_pos = self.write_pos - self.capacity;
        }
    }

    pub fn pop(&mut self) -> Option<T> {
        if self.read_pos >= self.write_pos {
            return None;
        }
        let idx = self.read_pos % self.capacity;
        self.read_pos += 1;
        if idx < self.data.len() {
            Some(self.data[idx].clone())
        } else {
            None
        }
    }

    pub fn len(&self) -> usize {
        self.write_pos.saturating_sub(self.read_pos)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn drain_all(&mut self) -> Vec<T> {
        let mut out = Vec::with_capacity(self.len());
        while let Some(v) = self.pop() {
            out.push(v);
        }
        out
    }
}

// ── Pipeline Stage ──────────────────────────────────────────────────

#[derive(Clone, Copy)]
pub enum StageType {
    Tokenize,
    Embed,
    Attention,
    FFN,
    Decode,
    PostProcess,
}

pub struct PipelineStage {
    pub name: String,
    pub stage_type: StageType,
    pub input_dim: usize,
    pub output_dim: usize,
    pub weights: Vec<Q16>,
    pub bias: Vec<Q16>,
    pub tokens_processed: u64,
}

impl PipelineStage {
    pub fn new(name: String, stage_type: StageType, input_dim: usize, output_dim: usize) -> Self {
        // Initialize weights deterministically
        let weight_count = input_dim * output_dim;
        let mut weights = Vec::with_capacity(weight_count.min(4096));
        let mut state: u64 = 0xDEAD_BEEF;
        for _ in 0..weight_count.min(4096) {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            weights.push(((state % 131) as Q16 - 65) * 100); // small random Q16
        }
        let bias = alloc::vec![Q16_ZERO; output_dim];

        PipelineStage {
            name,
            stage_type,
            input_dim,
            output_dim,
            weights,
            bias,
            tokens_processed: 0,
        }
    }

    pub fn process(&self, input: &[Q16]) -> Vec<Q16> {
        let mut output = alloc::vec![Q16_ZERO; self.output_dim];
        match self.stage_type {
            StageType::Tokenize => {
                // Hash-based tokenization: spread input across output dims
                for (i, &val) in input.iter().enumerate() {
                    let idx = i % self.output_dim;
                    output[idx] = output[idx].saturating_add(val);
                }
            }
            StageType::Embed => {
                // Simple embedding lookup via weight matrix multiply
                for j in 0..self.output_dim {
                    let mut sum: i64 = 0;
                    for i in 0..input.len().min(self.input_dim) {
                        let wi = (i * self.output_dim + j) % self.weights.len().max(1);
                        sum += input[i] as i64
                            * self.weights.get(wi).copied().unwrap_or(Q16_ZERO) as i64;
                    }
                    output[j] =
                        ((sum >> 16) as Q16).saturating_add(self.bias.get(j).copied().unwrap_or(0));
                }
            }
            StageType::Attention => {
                // Simplified self-attention: softmax-free dot-product
                let dim = input.len().min(self.output_dim);
                for i in 0..dim {
                    let mut score: i64 = 0;
                    for j in 0..dim {
                        score += input[i] as i64 * input[j] as i64;
                    }
                    let attention = (score >> 16) as Q16;
                    output[i] = q16_mul(
                        input.get(i).copied().unwrap_or(0),
                        attention.clamp(-Q16_ONE, Q16_ONE),
                    );
                }
            }
            StageType::FFN => {
                // Feed-forward: ReLU(W*x + b)
                for j in 0..self.output_dim {
                    let mut sum: i64 = 0;
                    for i in 0..input.len().min(self.input_dim) {
                        let wi = (i * self.output_dim + j) % self.weights.len().max(1);
                        sum += input[i] as i64 * self.weights.get(wi).copied().unwrap_or(0) as i64;
                    }
                    let val =
                        ((sum >> 16) as Q16).saturating_add(self.bias.get(j).copied().unwrap_or(0));
                    output[j] = if val > 0 { val } else { 0 }; // ReLU
                }
            }
            StageType::Decode => {
                // Pass-through with normalization
                let mut max_abs: i64 = 1;
                for &v in input.iter() {
                    let abs = if v < 0 { -(v as i64) } else { v as i64 };
                    if abs > max_abs {
                        max_abs = abs;
                    }
                }
                for i in 0..input.len().min(self.output_dim) {
                    output[i] = ((input[i] as i64 * Q16_ONE as i64) / max_abs) as Q16;
                }
            }
            StageType::PostProcess => {
                // Clamp to valid range
                for i in 0..input.len().min(self.output_dim) {
                    output[i] = input[i].clamp(-Q16_ONE, Q16_ONE);
                }
            }
        }
        output
    }
}

// ── Stream Channel ──────────────────────────────────────────────────

pub struct StreamChannel {
    pub id: u32,
    pub name: String,
    pub source_node: u16,
    pub target_node: u16,
    pub buffer: RingBuffer<Q16>,
    pub throughput_q16: Q16,
    pub latency_ticks: u32,
    pub priority: u8,
    pub active: bool,
    last_send_tick: u64,
    send_count_window: u32,
    window_start_tick: u64,
}

impl StreamChannel {
    pub fn new(
        id: u32,
        name: String,
        source: u16,
        target: u16,
        buf_size: usize,
        priority: u8,
    ) -> Self {
        StreamChannel {
            id,
            name,
            source_node: source,
            target_node: target,
            buffer: RingBuffer::with_capacity(buf_size),
            throughput_q16: Q16_ZERO,
            latency_ticks: 0,
            priority,
            active: true,
            last_send_tick: 0,
            send_count_window: 0,
            window_start_tick: 0,
        }
    }

    pub fn send(&mut self, data: &[Q16], tick: u64) {
        for &val in data {
            self.buffer.push(val);
        }
        if self.last_send_tick > 0 && tick > self.last_send_tick {
            let delta = (tick - self.last_send_tick) as u32;
            self.latency_ticks = (self.latency_ticks * 7 + delta) / 8;
        }
        self.last_send_tick = tick;
        self.send_count_window += data.len() as u32;
        let elapsed = tick.saturating_sub(self.window_start_tick);
        if elapsed >= 1000 {
            let count_q16 = q16_from_int(self.send_count_window as i32);
            let seconds_q16 = (elapsed as i32 * Q16_ONE) / 1000;
            if seconds_q16 > 0 {
                self.throughput_q16 =
                    ((count_q16 as i64 * Q16_ONE as i64) / seconds_q16 as i64) as Q16;
            }
            self.send_count_window = 0;
            self.window_start_tick = tick;
        }
    }

    pub fn recv(&mut self) -> Vec<Q16> {
        self.buffer.drain_all()
    }

    pub fn recv_up_to(&mut self, max: usize) -> Vec<Q16> {
        let mut out = Vec::with_capacity(max);
        for _ in 0..max {
            match self.buffer.pop() {
                Some(v) => out.push(v),
                None => break,
            }
        }
        out
    }
}

// ── Transform & Route ───────────────────────────────────────────────

#[derive(Clone, Copy)]
pub enum TransformKind {
    PassThrough,
    Scale(Q16),
    Offset(Q16),
    Normalize,
    Quantize,
}

#[derive(Clone, Copy)]
pub struct Route {
    pub source_channel: u32,
    pub target_channel: u32,
    pub transform: TransformKind,
}

impl Route {
    pub fn apply_transform(&self, data: &[Q16]) -> Vec<Q16> {
        match self.transform {
            TransformKind::PassThrough => data.to_vec(),
            TransformKind::Scale(factor) => data.iter().map(|&v| q16_mul(v, factor)).collect(),
            TransformKind::Offset(offset) => {
                data.iter().map(|&v| v.saturating_add(offset)).collect()
            }
            TransformKind::Normalize => {
                let mut sum: i64 = 0;
                for &v in data {
                    sum += if v < 0 { -(v as i64) } else { v as i64 };
                }
                if sum == 0 {
                    return data.to_vec();
                }
                data.iter()
                    .map(|&v| ((v as i64 * Q16_ONE as i64) / sum) as Q16)
                    .collect()
            }
            TransformKind::Quantize => data
                .iter()
                .map(|&v| {
                    if v >= 0 {
                        ((v + Q16_HALF) / Q16_ONE) * Q16_ONE
                    } else {
                        ((v - Q16_HALF) / Q16_ONE) * Q16_ONE
                    }
                })
                .collect(),
        }
    }
}

// ── Inference Pipeline ──────────────────────────────────────────────

pub struct InferencePipeline {
    pub stages: Vec<PipelineStage>,
    pub output_buffer: Vec<Q16>,
    pub is_running: bool,
    pub tokens_processed: u64,
    pub avg_latency_us: u64,
    latency_accumulator: u64,
    latency_sample_count: u64,
}

impl InferencePipeline {
    pub const fn new() -> Self {
        InferencePipeline {
            stages: Vec::new(),
            output_buffer: Vec::new(),
            is_running: false,
            tokens_processed: 0,
            avg_latency_us: 0,
            latency_accumulator: 0,
            latency_sample_count: 0,
        }
    }

    pub fn add_stage(&mut self, stage: PipelineStage) {
        if self.stages.len() < MAX_PIPELINE_STAGES {
            self.stages.push(stage);
        }
    }

    pub fn step(&mut self, input: &[Q16]) -> Vec<Q16> {
        self.is_running = true;
        let mut current = input.to_vec();
        let tick_start = self.tokens_processed;

        for stage in &self.stages {
            current = stage.process(&current);
        }

        self.tokens_processed += current.len() as u64;
        let elapsed = self.tokens_processed - tick_start;
        self.latency_accumulator += elapsed;
        self.latency_sample_count = self.latency_sample_count.saturating_add(1);
        if self.latency_sample_count > 0 {
            self.avg_latency_us = self.latency_accumulator / self.latency_sample_count;
        }
        self.output_buffer = current.clone();
        self.is_running = false;
        current
    }

    pub fn stage_count(&self) -> usize {
        self.stages.len()
    }

    pub fn build_default() -> Self {
        let dim = 64;
        let mut p = Self::new();
        p.add_stage(PipelineStage::new(
            String::from("tokenize"),
            StageType::Tokenize,
            dim,
            dim,
        ));
        p.add_stage(PipelineStage::new(
            String::from("embed"),
            StageType::Embed,
            dim,
            dim,
        ));
        p.add_stage(PipelineStage::new(
            String::from("attention"),
            StageType::Attention,
            dim,
            dim,
        ));
        p.add_stage(PipelineStage::new(
            String::from("ffn"),
            StageType::FFN,
            dim,
            dim,
        ));
        p.add_stage(PipelineStage::new(
            String::from("decode"),
            StageType::Decode,
            dim,
            dim,
        ));
        p.add_stage(PipelineStage::new(
            String::from("postprocess"),
            StageType::PostProcess,
            dim,
            dim,
        ));
        p
    }
}

// ── Stream Router ───────────────────────────────────────────────────

pub struct StreamRouter {
    pub channels: BTreeMap<u32, StreamChannel>,
    pub next_channel_id: u32,
    pub routes: Vec<Route>,
    pub pipeline: InferencePipeline,
    pub tick: u64,
    pub total_routed: u64,
}

impl StreamRouter {
    pub const fn new() -> Self {
        StreamRouter {
            channels: BTreeMap::new(),
            next_channel_id: 1,
            routes: Vec::new(),
            pipeline: InferencePipeline::new(),
            tick: 0,
            total_routed: 0,
        }
    }

    pub fn create_channel(
        &mut self,
        name: &str,
        source: u16,
        target: u16,
        buf_size: usize,
        priority: u8,
    ) -> u32 {
        if self.channels.len() >= MAX_CHANNELS {
            return 0;
        }
        let id = self.next_channel_id;
        self.next_channel_id = self.next_channel_id.saturating_add(1);
        self.channels.insert(
            id,
            StreamChannel::new(id, String::from(name), source, target, buf_size, priority),
        );
        id
    }

    pub fn send(&mut self, channel_id: u32, data: &[Q16]) -> Result<(), &'static str> {
        self.tick = self.tick.saturating_add(1);
        let tick = self.tick;
        match self.channels.get_mut(&channel_id) {
            Some(ch) if ch.active => {
                ch.send(data, tick);
                self.total_routed += data.len() as u64;
                Ok(())
            }
            Some(_) => Err("channel inactive"),
            None => Err("channel not found"),
        }
    }

    pub fn recv(&mut self, channel_id: u32) -> Option<Vec<Q16>> {
        self.channels.get_mut(&channel_id).and_then(|ch| {
            let data = ch.recv();
            if data.is_empty() {
                None
            } else {
                Some(data)
            }
        })
    }

    pub fn broadcast(&mut self, data: &[Q16], source_node: u16) {
        self.tick = self.tick.saturating_add(1);
        let tick = self.tick;
        let mut routed = 0u64;
        for ch in self.channels.values_mut() {
            if ch.source_node == source_node && ch.active {
                ch.send(data, tick);
                routed += data.len() as u64;
            }
        }
        self.total_routed += routed;
    }

    pub fn add_route(&mut self, source: u32, target: u32, transform: TransformKind) {
        self.routes.push(Route {
            source_channel: source,
            target_channel: target,
            transform,
        });
    }

    pub fn inference_step(&mut self, input: &[Q16]) -> Vec<Q16> {
        self.pipeline.step(input)
    }

    pub fn initialize(&mut self) {
        self.pipeline = InferencePipeline::build_default();
        let ch1 = self.create_channel("kern->neural", 0, 3, DEFAULT_BUFFER_CAPACITY, 0);
        let ch2 = self.create_channel("neural->mem", 3, 1, DEFAULT_BUFFER_CAPACITY, 1);
        let ch3 = self.create_channel("mem->sched", 1, 2, 128, 2);
        let _ch4 = self.create_channel("kern->ipc", 0, 4, DEFAULT_BUFFER_CAPACITY, 3);
        self.add_route(ch1, ch2, TransformKind::PassThrough);
        self.add_route(ch2, ch3, TransformKind::Normalize);
    }

    pub fn active_channel_count(&self) -> usize {
        self.channels.values().filter(|c| c.active).count()
    }
    pub fn route_count(&self) -> usize {
        self.routes.len()
    }
}

// ── Global Instance ─────────────────────────────────────────────────

pub static STREAM: Mutex<StreamRouter> = Mutex::new(StreamRouter::new());

// ── Public API ──────────────────────────────────────────────────────

pub fn init() {
    let mut router = STREAM.lock();
    router.initialize();
    serial_println!(
        "    [streaming] initialized: {} channels, {} pipeline stages",
        router.active_channel_count(),
        router.pipeline.stage_count()
    );
}

pub fn create_channel(name: &str, source: u16, target: u16) -> u32 {
    STREAM
        .lock()
        .create_channel(name, source, target, DEFAULT_BUFFER_CAPACITY, 128)
}

pub fn send(channel_id: u32, data: &[Q16]) -> Result<(), &'static str> {
    STREAM.lock().send(channel_id, data)
}

pub fn recv(channel_id: u32) -> Option<Vec<Q16>> {
    STREAM.lock().recv(channel_id)
}

pub fn broadcast(data: &[Q16], source_node: u16) {
    STREAM.lock().broadcast(data, source_node);
}

pub fn inference_step(input: &[Q16]) -> Vec<Q16> {
    STREAM.lock().inference_step(input)
}

pub fn stats() -> (usize, usize, u64, u64, u64) {
    let r = STREAM.lock();
    (
        r.active_channel_count(),
        r.route_count(),
        r.total_routed,
        r.pipeline.tokens_processed,
        r.pipeline.avg_latency_us,
    )
}
