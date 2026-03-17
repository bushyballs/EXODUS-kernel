use crate::sync::Mutex;
use alloc::collections::BTreeMap;
/// App runtime environment
///
/// Part of the Genesis app framework. Manages the execution
/// lifecycle of applications including start, suspend, resume,
/// stop, and resource tracking per-app.
use alloc::string::String;

/// Runtime state of a running application
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeState {
    Starting,
    Running,
    Suspended,
    Stopping,
}

/// Resource usage counters for a running app
struct ResourceUsage {
    memory_bytes: usize,
    cpu_time_us: u64,
    io_reads: u64,
    io_writes: u64,
    start_time: u64,
    last_active: u64,
}

impl ResourceUsage {
    fn new() -> Self {
        let now = runtime_tick();
        Self {
            memory_bytes: 0,
            cpu_time_us: 0,
            io_reads: 0,
            io_writes: 0,
            start_time: now,
            last_active: now,
        }
    }

    fn uptime(&self) -> u64 {
        runtime_tick().saturating_sub(self.start_time)
    }
}

/// Monotonic tick
static RUNTIME_TICK: Mutex<u64> = Mutex::new(0);

fn runtime_tick() -> u64 {
    let mut t = RUNTIME_TICK.lock();
    *t += 1;
    *t
}

pub struct AppRuntime {
    pub app_id: u64,
    pub state: RuntimeState,
    pub name: String,
    resources: ResourceUsage,
    suspend_count: u32,
    error_count: u32,
    exit_code: Option<i32>,
}

impl AppRuntime {
    pub fn new(app_id: u64, name: &str) -> Self {
        let mut n = String::new();
        for c in name.chars() {
            n.push(c);
        }
        crate::serial_println!(
            "[app::runtime] created runtime for '{}' (id={})",
            name,
            app_id
        );
        Self {
            app_id,
            state: RuntimeState::Starting,
            name: n,
            resources: ResourceUsage::new(),
            suspend_count: 0,
            error_count: 0,
            exit_code: None,
        }
    }

    /// Start the application runtime
    pub fn start(&mut self) -> Result<(), ()> {
        match self.state {
            RuntimeState::Starting => {
                // Validate preconditions
                if self.name.is_empty() {
                    crate::serial_println!(
                        "[app::runtime] error: cannot start app with empty name"
                    );
                    return Err(());
                }

                // Allocate initial resources
                self.resources = ResourceUsage::new();
                self.resources.memory_bytes = 4096; // initial page

                self.state = RuntimeState::Running;
                crate::serial_println!(
                    "[app::runtime] '{}' started (id={})",
                    self.name,
                    self.app_id
                );
                Ok(())
            }
            RuntimeState::Suspended => {
                // Resume from suspended state
                self.state = RuntimeState::Running;
                self.resources.last_active = runtime_tick();
                crate::serial_println!("[app::runtime] '{}' resumed from suspend", self.name);
                Ok(())
            }
            _ => {
                crate::serial_println!(
                    "[app::runtime] error: cannot start in state {:?}",
                    self.state
                );
                Err(())
            }
        }
    }

    /// Suspend the application (free resources)
    pub fn suspend(&mut self) {
        if self.state != RuntimeState::Running {
            crate::serial_println!(
                "[app::runtime] warning: suspend called in state {:?}",
                self.state
            );
            return;
        }

        // Save state snapshot and release non-essential resources
        let freed_memory = self.resources.memory_bytes / 2; // free half of memory
        self.resources.memory_bytes -= freed_memory;
        self.suspend_count = self.suspend_count.saturating_add(1);

        self.state = RuntimeState::Suspended;
        crate::serial_println!(
            "[app::runtime] '{}' suspended (freed {} bytes, suspend #{})",
            self.name,
            freed_memory,
            self.suspend_count
        );
    }

    /// Stop the application
    pub fn stop(&mut self, exit_code: i32) {
        if self.state == RuntimeState::Stopping {
            return;
        }
        self.state = RuntimeState::Stopping;
        self.exit_code = Some(exit_code);

        // Release all resources
        let total_memory = self.resources.memory_bytes;
        self.resources.memory_bytes = 0;
        let uptime = self.resources.uptime();

        crate::serial_println!(
            "[app::runtime] '{}' stopping: exit_code={}, uptime={}, freed {} bytes",
            self.name,
            exit_code,
            uptime,
            total_memory
        );
    }

    /// Record CPU time usage
    pub fn record_cpu_time(&mut self, microseconds: u64) {
        self.resources.cpu_time_us += microseconds;
        self.resources.last_active = runtime_tick();
    }

    /// Record memory allocation
    pub fn record_memory(&mut self, bytes: usize) {
        self.resources.memory_bytes += bytes;
    }

    /// Record an IO operation
    pub fn record_io(&mut self, is_write: bool) {
        if is_write {
            self.resources.io_writes = self.resources.io_writes.saturating_add(1);
        } else {
            self.resources.io_reads = self.resources.io_reads.saturating_add(1);
        }
    }

    /// Record an error
    pub fn record_error(&mut self) {
        self.error_count = self.error_count.saturating_add(1);
        if self.error_count > 100 {
            crate::serial_println!(
                "[app::runtime] '{}' exceeded error threshold ({}), stopping",
                self.name,
                self.error_count
            );
            self.stop(-1);
        }
    }

    /// Get runtime statistics
    pub fn stats(&self) -> (u64, usize, u64, u64) {
        (
            self.resources.cpu_time_us,
            self.resources.memory_bytes,
            self.resources.io_reads,
            self.resources.io_writes,
        )
    }

    /// Get uptime in ticks
    pub fn uptime(&self) -> u64 {
        self.resources.uptime()
    }

    /// Check if the app is healthy (not too many errors, responsive)
    pub fn is_healthy(&self) -> bool {
        self.state == RuntimeState::Running && self.error_count < 50
    }
}

/// Runtime manager that tracks all running apps
struct RuntimeManager {
    apps: BTreeMap<u64, AppRuntime>,
    next_id: u64,
    max_apps: usize,
}

impl RuntimeManager {
    fn new() -> Self {
        Self {
            apps: BTreeMap::new(),
            next_id: 1,
            max_apps: 128,
        }
    }

    fn launch(&mut self, name: &str) -> Result<u64, ()> {
        if self.apps.len() >= self.max_apps {
            crate::serial_println!(
                "[app::runtime] error: max app limit reached ({})",
                self.max_apps
            );
            return Err(());
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let mut runtime = AppRuntime::new(id, name);
        runtime.start()?;
        self.apps.insert(id, runtime);
        Ok(id)
    }

    fn stop_app(&mut self, app_id: u64, exit_code: i32) {
        if let Some(rt) = self.apps.get_mut(&app_id) {
            rt.stop(exit_code);
        }
    }

    fn running_count(&self) -> usize {
        let mut count = 0;
        for (_, rt) in &self.apps {
            if rt.state == RuntimeState::Running {
                count += 1;
            }
        }
        count
    }
}

static RUNTIME_MGR: Mutex<Option<RuntimeManager>> = Mutex::new(None);

pub fn init() {
    let mgr = RuntimeManager::new();
    let mut m = RUNTIME_MGR.lock();
    *m = Some(mgr);
    crate::serial_println!("[app::runtime] runtime manager initialized");
}

/// Launch an app by name
pub fn launch(name: &str) -> Result<u64, ()> {
    let mut m = RUNTIME_MGR.lock();
    match m.as_mut() {
        Some(mgr) => mgr.launch(name),
        None => Err(()),
    }
}
