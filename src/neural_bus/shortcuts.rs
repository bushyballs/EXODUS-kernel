use crate::sync::Mutex;
/// Shortcut Engine — AI pre-executes predicted operations
///
/// Evaluates cortex predictions, fires matching shortcuts (preload app,
/// prefetch data, pre-render UI, warm cache), tracks hit/miss ratios,
/// auto-prunes bad shortcuts. Includes a result cache with TTL eviction.
///
/// All Q16 fixed-point. No floats. Zero external deps.
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use super::{SignalKind, Q16, Q16_HALF, Q16_ONE, Q16_TENTH};

// ── Constants ───────────────────────────────────────────────────────

const MAX_SHORTCUTS: usize = 64;
const MAX_PRELOAD_QUEUE: usize = 16;
const MAX_PREFETCH_QUEUE: usize = 16;
const MAX_CACHE_ENTRIES: usize = 128;
const MAX_CACHED_DATA_BYTES: usize = 1024 * 256; // 256KB
const CACHE_DEFAULT_TTL_MS: u64 = 30_000;
const SHORTCUT_COOLDOWN_MS: u64 = 500;
const PRUNE_HIT_RATIO_THRESHOLD: Q16 = Q16_ONE / 5; // 20%

// ── Tick Counter ────────────────────────────────────────────────────

static TICK: Mutex<u64> = Mutex::new(0);

fn current_tick() -> u64 {
    *TICK.lock()
}
fn advance_tick() -> u64 {
    let mut t = TICK.lock();
    *t = t.saturating_add(1);
    *t
}

// ── Trigger & Action Types ──────────────────────────────────────────

#[derive(Clone)]
pub enum ShortcutTrigger {
    PredictedAction(SignalKind),
    PredictedAppLaunch(String),
    PatternMatch(u32),
    TimeOfDay(u8),
    ContextShift(String),
    SequenceDetected(Vec<SignalKind>),
    CpuIdle,
    UserIdle(u32),
}

#[derive(Clone)]
pub enum ShortcutAction {
    PreloadApp(String),
    PrefetchData(String),
    PreRenderUI(u32),
    PreComputeResult(u64),
    WarmCache(String),
    AdjustPriority(u16, i32),
    PreAllocMemory(u32),
    BatchNetworkRequest(String),
    CompressInBackground(String),
}

// ── Shortcut Definition ─────────────────────────────────────────────

pub struct Shortcut {
    pub id: u32,
    pub trigger: ShortcutTrigger,
    pub action: ShortcutAction,
    pub confidence_threshold: Q16,
    pub estimated_savings_ms: u32,
    pub hit_count: u32,
    pub miss_count: u32,
    pub last_fired: u64,
    pub active: bool,
}

impl Shortcut {
    pub fn hit_ratio(&self) -> Q16 {
        let total = self.hit_count + self.miss_count;
        if total == 0 {
            return Q16_HALF;
        }
        ((self.hit_count as i64 * Q16_ONE as i64) / total as i64) as Q16
    }
}

// ── Cache ───────────────────────────────────────────────────────────

pub struct CachedResult {
    pub key: u64,
    pub data: Vec<u8>,
    pub created_at: u64,
    pub ttl_ms: u64,
    pub hit_count: u32,
    pub size_bytes: usize,
}

impl CachedResult {
    pub fn new(key: u64, data: Vec<u8>, created: u64, ttl: u64) -> Self {
        let size = data.len();
        CachedResult {
            key,
            data,
            created_at: created,
            ttl_ms: ttl,
            hit_count: 0,
            size_bytes: size,
        }
    }
    pub fn is_expired(&self, now: u64) -> bool {
        now.saturating_sub(self.created_at) > self.ttl_ms
    }
}

// ── Queue Hints ─────────────────────────────────────────────────────

pub struct PreloadHint {
    pub target: String,
    pub priority: Q16,
    pub predicted_at: u64,
    pub confidence: Q16,
}

pub struct PrefetchHint {
    pub target: String,
    pub priority: Q16,
    pub predicted_at: u64,
    pub confidence: Q16,
}

// ── Shortcut Engine ─────────────────────────────────────────────────

pub struct ShortcutEngine {
    pub enabled: bool,
    pub shortcuts: Vec<Shortcut>,
    pub next_id: u32,
    pub cache: BTreeMap<u64, CachedResult>,
    pub total_cache_bytes: usize,
    pub preload_queue: Vec<PreloadHint>,
    pub prefetch_queue: Vec<PrefetchHint>,
    pub total_shortcuts_fired: u64,
    pub total_cache_hits: u64,
    pub total_cache_misses: u64,
    pub total_time_saved_ms: u64,
}

impl ShortcutEngine {
    pub const fn new() -> Self {
        ShortcutEngine {
            enabled: true,
            shortcuts: Vec::new(),
            next_id: 1,
            cache: BTreeMap::new(),
            total_cache_bytes: 0,
            preload_queue: Vec::new(),
            prefetch_queue: Vec::new(),
            total_shortcuts_fired: 0,
            total_cache_hits: 0,
            total_cache_misses: 0,
            total_time_saved_ms: 0,
        }
    }

    pub fn register_shortcut(
        &mut self,
        trigger: ShortcutTrigger,
        action: ShortcutAction,
        threshold: Q16,
        savings_ms: u32,
    ) -> u32 {
        if self.shortcuts.len() >= MAX_SHORTCUTS {
            return 0;
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.shortcuts.push(Shortcut {
            id,
            trigger,
            action,
            confidence_threshold: threshold,
            estimated_savings_ms: savings_ms,
            hit_count: 0,
            miss_count: 0,
            last_fired: 0,
            active: true,
        });
        id
    }

    pub fn evaluate_prediction(&mut self, kind: SignalKind, confidence: Q16) {
        if !self.enabled {
            return;
        }
        let now = current_tick();
        let mut to_fire: Vec<usize> = Vec::new();

        for (i, sc) in self.shortcuts.iter().enumerate() {
            if !sc.active || confidence < sc.confidence_threshold {
                continue;
            }
            if now.saturating_sub(sc.last_fired) < SHORTCUT_COOLDOWN_MS {
                continue;
            }
            if self.trigger_matches(&sc.trigger, &kind) {
                to_fire.push(i);
            }
        }
        for idx in to_fire {
            self.fire_shortcut(idx, now);
        }
    }

    fn trigger_matches(&self, trigger: &ShortcutTrigger, kind: &SignalKind) -> bool {
        match trigger {
            ShortcutTrigger::PredictedAction(expected) => {
                core::mem::discriminant(expected) == core::mem::discriminant(kind)
            }
            ShortcutTrigger::PredictedAppLaunch(_) => matches!(kind, SignalKind::AppLaunch),
            ShortcutTrigger::ContextShift(_) => matches!(kind, SignalKind::ContextShift),
            ShortcutTrigger::CpuIdle => matches!(kind, SignalKind::Heartbeat),
            ShortcutTrigger::PatternMatch(id) => *id > 0 && matches!(kind, SignalKind::Heartbeat),
            ShortcutTrigger::UserIdle(_) => matches!(kind, SignalKind::Heartbeat),
            _ => false,
        }
    }

    pub fn fire_shortcut(&mut self, idx: usize, now: u64) {
        if idx >= self.shortcuts.len() {
            return;
        }
        self.shortcuts[idx].last_fired = now;
        self.total_shortcuts_fired = self.total_shortcuts_fired.saturating_add(1);

        let action = self.shortcuts[idx].action.clone();
        let id = self.shortcuts[idx].id;
        let savings = self.shortcuts[idx].estimated_savings_ms;

        serial_println!("    [shortcuts] FIRE id={} savings={}ms", id, savings);

        match action {
            ShortcutAction::PreloadApp(ref name) => {
                self.enqueue_preload(name.clone(), Q16_ONE, now, Q16_HALF);
            }
            ShortcutAction::PrefetchData(ref path) => {
                self.enqueue_prefetch(path.clone(), Q16_ONE, now, Q16_HALF);
            }
            ShortcutAction::PreRenderUI(component_id) => {
                let key = 0xAA00_0000_0000_0000u64 | component_id as u64;
                self.cache_result(key, alloc::vec![0u8; 64], CACHE_DEFAULT_TTL_MS, now);
            }
            ShortcutAction::PreComputeResult(result_key) => {
                let computed = self.simulate_computation(result_key);
                self.cache_result(result_key, computed, CACHE_DEFAULT_TTL_MS, now);
            }
            ShortcutAction::WarmCache(ref cache_name) => {
                let key = Self::hash_string(cache_name);
                self.cache_result(key, alloc::vec![0xCAu8; 128], CACHE_DEFAULT_TTL_MS * 2, now);
            }
            ShortcutAction::AdjustPriority(node_id, delta) => {
                serial_println!(
                    "    [shortcuts]   -> priority node={} delta={}",
                    node_id,
                    delta
                );
            }
            ShortcutAction::PreAllocMemory(mb) => {
                serial_println!("    [shortcuts]   -> pre-alloc {}MB", mb);
            }
            ShortcutAction::BatchNetworkRequest(ref label) => {
                self.enqueue_prefetch(label.clone(), Q16_HALF, now, Q16_HALF);
            }
            ShortcutAction::CompressInBackground(ref path) => {
                self.enqueue_preload(path.clone(), Q16_ONE / 4, now, Q16_HALF);
            }
        }
    }

    fn simulate_computation(&self, key: u64) -> Vec<u8> {
        let mut result = Vec::with_capacity(64);
        let mut state = if key == 0 {
            0xDEAD_BEEF_CAFE_BABEu64
        } else {
            key
        };
        for _ in 0..64 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            result.push((state & 0xFF) as u8);
        }
        result
    }

    fn enqueue_preload(
        &mut self,
        target: String,
        priority: Q16,
        predicted_at: u64,
        confidence: Q16,
    ) {
        if self.preload_queue.iter().any(|h| h.target == target) {
            return;
        }
        if self.preload_queue.len() >= MAX_PRELOAD_QUEUE {
            let lowest = self
                .preload_queue
                .iter()
                .enumerate()
                .min_by_key(|(_, h)| h.priority)
                .map(|(i, _)| i);
            if let Some(i) = lowest {
                self.preload_queue.remove(i);
            }
        }
        self.preload_queue.push(PreloadHint {
            target,
            priority,
            predicted_at,
            confidence,
        });
    }

    fn enqueue_prefetch(
        &mut self,
        target: String,
        priority: Q16,
        predicted_at: u64,
        confidence: Q16,
    ) {
        if self.prefetch_queue.iter().any(|h| h.target == target) {
            return;
        }
        if self.prefetch_queue.len() >= MAX_PREFETCH_QUEUE {
            let lowest = self
                .prefetch_queue
                .iter()
                .enumerate()
                .min_by_key(|(_, h)| h.priority)
                .map(|(i, _)| i);
            if let Some(i) = lowest {
                self.prefetch_queue.remove(i);
            }
        }
        self.prefetch_queue.push(PrefetchHint {
            target,
            priority,
            predicted_at,
            confidence,
        });
    }

    // ── Cache ───────────────────────────────────────────────────────

    pub fn cache_result(&mut self, key: u64, data: Vec<u8>, ttl_ms: u64, now: u64) {
        let size = data.len();
        while self.cache.len() >= MAX_CACHE_ENTRIES
            || self.total_cache_bytes + size > MAX_CACHED_DATA_BYTES
        {
            if !self.evict_one(now) {
                break;
            }
        }
        if let Some(old) = self.cache.remove(&key) {
            self.total_cache_bytes = self.total_cache_bytes.saturating_sub(old.size_bytes);
        }
        let entry = CachedResult::new(key, data, now, ttl_ms);
        self.total_cache_bytes += entry.size_bytes;
        self.cache.insert(key, entry);
    }

    pub fn lookup_cache(&mut self, key: u64) -> Option<&CachedResult> {
        let now = current_tick();
        let expired = self.cache.get(&key).map(|e| e.is_expired(now));
        match expired {
            Some(true) => {
                if let Some(old) = self.cache.remove(&key) {
                    self.total_cache_bytes = self.total_cache_bytes.saturating_sub(old.size_bytes);
                }
                self.total_cache_misses = self.total_cache_misses.saturating_add(1);
                None
            }
            Some(false) => {
                if let Some(e) = self.cache.get_mut(&key) {
                    e.hit_count = e.hit_count.saturating_add(1);
                }
                self.total_cache_hits = self.total_cache_hits.saturating_add(1);
                self.cache.get(&key)
            }
            None => {
                self.total_cache_misses = self.total_cache_misses.saturating_add(1);
                None
            }
        }
    }

    pub fn cache_contains(&mut self, key: u64) -> bool {
        let now = current_tick();
        if let Some(entry) = self.cache.get(&key) {
            if entry.is_expired(now) {
                if let Some(old) = self.cache.remove(&key) {
                    self.total_cache_bytes = self.total_cache_bytes.saturating_sub(old.size_bytes);
                }
                false
            } else {
                true
            }
        } else {
            false
        }
    }

    fn evict_one(&mut self, now: u64) -> bool {
        if self.cache.is_empty() {
            return false;
        }
        let worst = self
            .cache
            .iter()
            .min_by_key(|(_, e)| {
                if e.is_expired(now) {
                    i64::MIN
                } else {
                    e.hit_count as i64 * 1000 - now.saturating_sub(e.created_at) as i64
                }
            })
            .map(|(k, _)| *k);
        if let Some(key) = worst {
            if let Some(old) = self.cache.remove(&key) {
                self.total_cache_bytes = self.total_cache_bytes.saturating_sub(old.size_bytes);
            }
            true
        } else {
            false
        }
    }

    // ── Hit/Miss Tracking ───────────────────────────────────────────

    pub fn report_hit(&mut self, shortcut_id: u32) {
        for sc in self.shortcuts.iter_mut() {
            if sc.id == shortcut_id {
                sc.hit_count = sc.hit_count.saturating_add(1);
                self.total_time_saved_ms += sc.estimated_savings_ms as u64;
                return;
            }
        }
    }

    pub fn report_miss(&mut self, shortcut_id: u32) {
        for sc in self.shortcuts.iter_mut() {
            if sc.id == shortcut_id {
                sc.miss_count = sc.miss_count.saturating_add(1);
                return;
            }
        }
    }

    pub fn prune_bad_shortcuts(&mut self) {
        let mut to_remove: Vec<usize> = Vec::new();
        for (i, sc) in self.shortcuts.iter().enumerate() {
            let total = sc.hit_count + sc.miss_count;
            if total >= 5 && sc.hit_ratio() < PRUNE_HIT_RATIO_THRESHOLD {
                to_remove.push(i);
            }
        }
        for &idx in to_remove.iter().rev() {
            self.shortcuts.remove(idx);
        }
        if !to_remove.is_empty() {
            serial_println!("    [shortcuts] pruned {} bad shortcuts", to_remove.len());
        }
    }

    // ── Tick ────────────────────────────────────────────────────────

    pub fn tick(&mut self) {
        if !self.enabled {
            return;
        }
        let now = advance_tick();
        self.process_preload_queue(now);
        self.process_prefetch_queue(now);
        if now % 100 == 0 {
            self.evict_cache(now);
        }
        if now % 500 == 0 {
            self.prune_bad_shortcuts();
        }
    }

    fn process_preload_queue(&mut self, now: u64) {
        if self.preload_queue.is_empty() {
            return;
        }
        let best = self
            .preload_queue
            .iter()
            .enumerate()
            .max_by_key(|(_, h)| h.priority)
            .map(|(i, _)| i)
            .unwrap_or(0);
        let hint = self.preload_queue.remove(best);
        if now.saturating_sub(hint.predicted_at) > 5000 {
            return;
        }
        let key = Self::hash_string(&hint.target);
        self.cache_result(
            key,
            alloc::vec![0x50, 0x4C, 0x4F, 0x41, 0x44],
            CACHE_DEFAULT_TTL_MS * 3,
            now,
        );
    }

    fn process_prefetch_queue(&mut self, now: u64) {
        if self.prefetch_queue.is_empty() {
            return;
        }
        let best = self
            .prefetch_queue
            .iter()
            .enumerate()
            .max_by_key(|(_, h)| h.priority)
            .map(|(i, _)| i)
            .unwrap_or(0);
        let hint = self.prefetch_queue.remove(best);
        if now.saturating_sub(hint.predicted_at) > 5000 {
            return;
        }
        let key = Self::hash_string(&hint.target);
        let data = self.simulate_computation(key);
        self.cache_result(key, data, CACHE_DEFAULT_TTL_MS * 2, now);
    }

    fn evict_cache(&mut self, now: u64) {
        let expired_keys: Vec<u64> = self
            .cache
            .iter()
            .filter(|(_, v)| v.is_expired(now))
            .map(|(k, _)| *k)
            .collect();
        for key in expired_keys {
            if let Some(old) = self.cache.remove(&key) {
                self.total_cache_bytes = self.total_cache_bytes.saturating_sub(old.size_bytes);
            }
        }
    }

    fn hash_string(s: &str) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in s.as_bytes() {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        hash
    }

    pub fn stats(&self) -> (u64, u64, u64, u64) {
        (
            self.total_shortcuts_fired,
            self.total_cache_hits,
            self.total_cache_misses,
            self.total_time_saved_ms,
        )
    }

    pub fn active_count(&self) -> usize {
        self.shortcuts.iter().filter(|s| s.active).count()
    }

    fn register_builtins(&mut self) {
        let threshold_60 = Q16_HALF + Q16_TENTH;

        self.register_shortcut(
            ShortcutTrigger::PredictedAction(SignalKind::AppLaunch),
            ShortcutAction::PreloadApp(String::from("recent_app")),
            threshold_60,
            200,
        );
        self.register_shortcut(
            ShortcutTrigger::TimeOfDay(8),
            ShortcutAction::WarmCache(String::from("morning_routine")),
            Q16_HALF,
            500,
        );
        self.register_shortcut(
            ShortcutTrigger::ContextShift(String::from("work")),
            ShortcutAction::PreloadApp(String::from("work_suite")),
            Q16_HALF,
            300,
        );
        self.register_shortcut(
            ShortcutTrigger::CpuIdle,
            ShortcutAction::CompressInBackground(String::from("/tmp/compressible")),
            Q16_ONE / 4,
            100,
        );
        self.register_shortcut(
            ShortcutTrigger::PredictedAction(SignalKind::SearchQuery),
            ShortcutAction::WarmCache(String::from("search_index")),
            threshold_60,
            150,
        );
        self.register_shortcut(
            ShortcutTrigger::TimeOfDay(20),
            ShortcutAction::PreloadApp(String::from("media_player")),
            Q16_HALF,
            250,
        );
        self.register_shortcut(
            ShortcutTrigger::UserIdle(60),
            ShortcutAction::PreAllocMemory(16),
            Q16_ONE / 3,
            50,
        );
        self.register_shortcut(
            ShortcutTrigger::PredictedAction(SignalKind::Heartbeat),
            ShortcutAction::BatchNetworkRequest(String::from("predicted_batch")),
            threshold_60,
            300,
        );

        serial_println!(
            "    [shortcuts] registered {} built-in shortcuts",
            self.shortcuts.len()
        );
    }
}

// ── Global Instance ─────────────────────────────────────────────────

pub static SHORTCUTS: Mutex<ShortcutEngine> = Mutex::new(ShortcutEngine::new());

// ── Public API ──────────────────────────────────────────────────────

pub fn init() {
    let mut engine = SHORTCUTS.lock();
    engine.register_builtins();
    serial_println!(
        "    [shortcuts] Shortcut engine initialized ({} active)",
        engine.active_count()
    );
}

pub fn evaluate(kind: SignalKind, confidence: Q16) {
    SHORTCUTS.lock().evaluate_prediction(kind, confidence);
}
pub fn cache_hit(key: u64) -> bool {
    SHORTCUTS.lock().cache_contains(key)
}
pub fn cache_lookup(key: u64) -> Option<Vec<u8>> {
    SHORTCUTS.lock().lookup_cache(key).map(|e| e.data.clone())
}
pub fn cache_store(key: u64, data: Vec<u8>, ttl_ms: u64) {
    let now = current_tick();
    SHORTCUTS.lock().cache_result(key, data, ttl_ms, now);
}
pub fn register(trigger: ShortcutTrigger, action: ShortcutAction) -> u32 {
    SHORTCUTS
        .lock()
        .register_shortcut(trigger, action, Q16_HALF, 100)
}
pub fn report_hit(id: u32) {
    SHORTCUTS.lock().report_hit(id);
}
pub fn report_miss(id: u32) {
    SHORTCUTS.lock().report_miss(id);
}
pub fn tick() {
    SHORTCUTS.lock().tick();
}
pub fn stats() -> (u64, u64, u64, u64) {
    SHORTCUTS.lock().stats()
}
pub fn hash_key(s: &str) -> u64 {
    ShortcutEngine::hash_string(s)
}
