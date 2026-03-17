use crate::sync::Mutex;
use crate::{serial_print, serial_println};
/// Model version management and switching
///
/// Part of the Hoags LLM engine. Tracks available model versions and
/// enables hot-swapping between them. Each model is identified by a
/// name + version pair, and at most one model may be "active" at a time.
///
/// The registry also tracks parameter counts, quantisation status, and
/// model lineage (which base model a fine-tune derives from) so the
/// rest of the LLM stack can adapt its memory budgets and KV-cache
/// sizes accordingly.
use alloc::string::String;
use alloc::vec::Vec;

/// Quantisation format of the model weights
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuantFormat {
    /// Full 32-bit floating point
    F32,
    /// 16-bit floating point (brain float)
    BF16,
    /// 16-bit floating point (IEEE)
    F16,
    /// 8-bit integer (symmetric)
    Int8,
    /// 4-bit integer (grouped, GPTQ-style)
    Int4,
}

/// Metadata for a registered model
pub struct ModelEntry {
    /// Human-readable name (e.g. "hoags-7b-chat")
    pub name: String,
    /// Monotonically increasing version number
    pub version: u32,
    /// Total parameter count
    pub param_count: u64,
    /// Whether this model is the currently active one
    pub active: bool,
    /// Quantisation format
    pub quant: QuantFormat,
    /// Name of the base model this was fine-tuned from (empty if base)
    pub base_model: String,
    /// Number of transformer layers
    pub num_layers: u32,
    /// Hidden dimension
    pub hidden_dim: u32,
    /// Number of attention heads
    pub num_heads: u32,
    /// Vocabulary size
    pub vocab_size: u32,
    /// Maximum context length
    pub max_ctx: u32,
    /// Estimated memory usage in bytes
    pub memory_bytes: u64,
    /// Timestamp of registration (monotonic counter)
    pub registered_at: u64,
}

impl ModelEntry {
    /// Estimate memory usage from parameter count and quantisation.
    pub fn estimate_memory(param_count: u64, quant: QuantFormat) -> u64 {
        let bytes_per_param = match quant {
            QuantFormat::F32 => 4,
            QuantFormat::BF16 | QuantFormat::F16 => 2,
            QuantFormat::Int8 => 1,
            QuantFormat::Int4 => 1, // 0.5 bytes, but we round up and add overhead
        };
        let base = param_count * bytes_per_param;
        // Add ~10% overhead for KV-cache, activations, etc.
        base + base / 10
    }
}

pub struct ModelRegistry {
    /// All registered models
    pub models: Vec<ModelEntry>,
    /// Monotonic counter for registration timestamps
    counter: u64,
    /// Maximum number of models to track
    max_models: usize,
}

impl ModelRegistry {
    pub fn new() -> Self {
        ModelRegistry {
            models: Vec::new(),
            counter: 0,
            max_models: 64,
        }
    }

    /// Register a new model version.
    pub fn register(&mut self, name: &str, version: u32, params: u64) {
        self.register_full(name, version, params, QuantFormat::F32, "", 0, 0, 0, 0, 0);
    }

    /// Register with full metadata.
    pub fn register_full(
        &mut self,
        name: &str,
        version: u32,
        params: u64,
        quant: QuantFormat,
        base_model: &str,
        num_layers: u32,
        hidden_dim: u32,
        num_heads: u32,
        vocab_size: u32,
        max_ctx: u32,
    ) {
        // Check for duplicate name+version
        for entry in &self.models {
            if entry.name == name && entry.version == version {
                serial_println!(
                    "    [model-registry] Duplicate: '{}' v{} already registered",
                    name,
                    version
                );
                return;
            }
        }

        // Evict oldest if at capacity
        if self.models.len() >= self.max_models {
            self.evict_oldest_inactive();
        }

        self.counter = self.counter.saturating_add(1);
        let memory_bytes = ModelEntry::estimate_memory(params, quant);

        let entry = ModelEntry {
            name: String::from(name),
            version,
            param_count: params,
            active: false,
            quant,
            base_model: String::from(base_model),
            num_layers,
            hidden_dim,
            num_heads,
            vocab_size,
            max_ctx,
            memory_bytes,
            registered_at: self.counter,
        };

        serial_println!(
            "    [model-registry] Registered '{}' v{}: {}M params, {:?}, ~{}MB",
            name,
            version,
            params / 1_000_000,
            quant,
            memory_bytes / (1024 * 1024)
        );

        self.models.push(entry);
    }

    /// Switch the active model to the given name. Activates the latest
    /// version of that model and deactivates all others.
    pub fn activate(&mut self, name: &str) -> Result<(), ()> {
        // Find the latest version of the requested model
        let mut best_version: Option<u32> = None;
        for entry in &self.models {
            if entry.name == name {
                match best_version {
                    None => best_version = Some(entry.version),
                    Some(v) if entry.version > v => best_version = Some(entry.version),
                    _ => {}
                }
            }
        }

        let target_version = best_version.ok_or(())?;

        // Deactivate all, then activate the target
        for entry in self.models.iter_mut() {
            entry.active = false;
        }
        for entry in self.models.iter_mut() {
            if entry.name == name && entry.version == target_version {
                entry.active = true;
                serial_println!(
                    "    [model-registry] Activated '{}' v{} ({}M params)",
                    name,
                    target_version,
                    entry.param_count / 1_000_000
                );
                return Ok(());
            }
        }

        Err(())
    }

    /// Activate a specific name + version.
    pub fn activate_version(&mut self, name: &str, version: u32) -> Result<(), ()> {
        let mut found = false;
        for entry in self.models.iter_mut() {
            entry.active = false;
            if entry.name == name && entry.version == version {
                found = true;
            }
        }
        if !found {
            return Err(());
        }
        for entry in self.models.iter_mut() {
            if entry.name == name && entry.version == version {
                entry.active = true;
                serial_println!("    [model-registry] Activated '{}' v{}", name, version);
                return Ok(());
            }
        }
        Err(())
    }

    /// Get a reference to the currently active model, if any.
    pub fn active_model(&self) -> Option<&ModelEntry> {
        self.models.iter().find(|e| e.active)
    }

    /// List all registered models.
    pub fn list(&self) -> &[ModelEntry] {
        &self.models
    }

    /// Find all versions of a model by name.
    pub fn versions_of(&self, name: &str) -> Vec<&ModelEntry> {
        self.models.iter().filter(|e| e.name == name).collect()
    }

    /// Remove a specific model entry.
    pub fn unregister(&mut self, name: &str, version: u32) -> bool {
        let before = self.models.len();
        self.models
            .retain(|e| !(e.name == name && e.version == version));
        let removed = self.models.len() < before;
        if removed {
            serial_println!("    [model-registry] Unregistered '{}' v{}", name, version);
        }
        removed
    }

    /// Evict the oldest inactive model to free a slot.
    fn evict_oldest_inactive(&mut self) {
        let mut oldest_idx = None;
        let mut oldest_ts = u64::MAX;
        for (i, entry) in self.models.iter().enumerate() {
            if !entry.active && entry.registered_at < oldest_ts {
                oldest_ts = entry.registered_at;
                oldest_idx = Some(i);
            }
        }
        if let Some(idx) = oldest_idx {
            serial_println!(
                "    [model-registry] Evicting '{}' v{} (oldest inactive)",
                self.models[idx].name,
                self.models[idx].version
            );
            self.models.swap_remove(idx);
        }
    }

    /// Total memory estimated for all registered models.
    pub fn total_memory_estimate(&self) -> u64 {
        self.models.iter().map(|e| e.memory_bytes).sum()
    }

    /// Total models registered.
    pub fn count(&self) -> usize {
        self.models.len()
    }
}

// ── Global Singleton ────────────────────────────────────────────────

struct RegistryState {
    registry: ModelRegistry,
}

static REGISTRY: Mutex<Option<RegistryState>> = Mutex::new(None);

pub fn init() {
    let mut registry = ModelRegistry::new();

    // Register the built-in bootstrap model
    registry.register_full(
        "hoags-bootstrap",
        1,
        1_000_000, // 1M params (tiny bootstrap model)
        QuantFormat::Int8,
        "",
        4,     // 4 layers
        256,   // hidden_dim
        4,     // heads
        32000, // vocab
        4096,  // max_ctx
    );
    let _ = registry.activate("hoags-bootstrap");

    let mut guard = REGISTRY.lock();
    *guard = Some(RegistryState { registry });
    serial_println!("    [model-registry] Subsystem initialised (bootstrap model active)");
}

/// Register a model in the global registry.
pub fn register_model(name: &str, version: u32, params: u64) {
    let mut guard = REGISTRY.lock();
    if let Some(state) = guard.as_mut() {
        state.registry.register(name, version, params);
    }
}

/// Activate a model by name in the global registry.
pub fn activate_model(name: &str) -> Result<(), ()> {
    let mut guard = REGISTRY.lock();
    if let Some(state) = guard.as_mut() {
        state.registry.activate(name)
    } else {
        Err(())
    }
}
