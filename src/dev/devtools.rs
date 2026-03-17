/// Developer tools for Genesis — remote debug, system monitor, sysctl
///
/// Provides remote debugging session management, system resource
/// monitoring, device inspection, and developer command interface.
///
/// Inspired by: Android Debug Bridge, Chrome DevTools. All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Remote debug session
pub struct RemoteDebugSession {
    pub id: u32,
    pub client_addr: u32,
    pub port: u16,
    pub connected: bool,
    pub protocol_version: u8,
}

/// System resource snapshot
pub struct SystemSnapshot {
    pub cpu_usage_percent: u8,
    pub memory_used_kb: u64,
    pub memory_total_kb: u64,
    pub process_count: u32,
    pub thread_count: u32,
    pub uptime_secs: u64,
    pub interrupt_count: u64,
    pub context_switches: u64,
}

/// System monitor
pub struct SystemMonitor {
    pub snapshots: Vec<SystemSnapshot>,
    pub max_snapshots: usize,
    pub interval_ms: u32,
}

impl SystemMonitor {
    const fn new() -> Self {
        SystemMonitor {
            snapshots: Vec::new(),
            max_snapshots: 1000,
            interval_ms: 1000,
        }
    }

    pub fn take_snapshot(&mut self) -> SystemSnapshot {
        let snap = SystemSnapshot {
            cpu_usage_percent: 0,
            memory_used_kb: 0,
            memory_total_kb: 0,
            process_count: 0,
            thread_count: 0,
            uptime_secs: crate::time::clock::unix_time(),
            interrupt_count: 0,
            context_switches: 0,
        };
        if self.snapshots.len() >= self.max_snapshots {
            self.snapshots.remove(0);
        }
        self.snapshots.push(SystemSnapshot {
            cpu_usage_percent: snap.cpu_usage_percent,
            memory_used_kb: snap.memory_used_kb,
            memory_total_kb: snap.memory_total_kb,
            process_count: snap.process_count,
            thread_count: snap.thread_count,
            uptime_secs: snap.uptime_secs,
            interrupt_count: snap.interrupt_count,
            context_switches: snap.context_switches,
        });
        snap
    }

    pub fn latest(&self) -> Option<&SystemSnapshot> {
        self.snapshots.last()
    }
}

/// Device inspector — reads device info from sysctl-like interface
pub struct DeviceInspector {
    pub entries: BTreeMap<String, String>,
}

impl DeviceInspector {
    const fn new() -> Self {
        DeviceInspector {
            entries: BTreeMap::new(),
        }
    }

    pub fn populate(&mut self) {
        self.entries
            .insert(String::from("kernel.version"), String::from("1.0.0"));
        self.entries
            .insert(String::from("kernel.name"), String::from("Genesis"));
        self.entries
            .insert(String::from("kernel.arch"), String::from("x86_64"));
        self.entries
            .insert(String::from("kernel.build"), String::from("debug"));
        self.entries
            .insert(String::from("hw.ncpu"), String::from("1"));
        self.entries
            .insert(String::from("hw.physmem"), String::from("0"));
        self.entries
            .insert(String::from("hw.pagesize"), String::from("4096"));
    }

    pub fn get(&self, key: &str) -> Option<&String> {
        self.entries.get(key)
    }

    pub fn set(&mut self, key: &str, value: &str) {
        self.entries.insert(String::from(key), String::from(value));
    }

    pub fn list_all(&self) -> Vec<(&String, &String)> {
        self.entries.iter().collect()
    }
}

/// Developer command handler
pub struct CommandHandler {
    pub history: Vec<String>,
}

impl CommandHandler {
    const fn new() -> Self {
        CommandHandler {
            history: Vec::new(),
        }
    }

    pub fn handle(&mut self, cmd: &str) -> String {
        self.history.push(String::from(cmd));

        let parts: Vec<&str> = cmd.split_whitespace().collect();
        if parts.is_empty() {
            return String::from("No command");
        }

        match parts[0] {
            "help" => {
                String::from("Commands: help, status, sysctl, uname, uptime, test, lsdev, meminfo")
            }
            "uname" => format!("Genesis 1.0.0 x86_64 Hoags Inc"),
            "uptime" => {
                let secs = crate::time::clock::unix_time();
                format!("up {} seconds", secs)
            }
            "sysctl" => {
                if parts.len() < 2 {
                    String::from("Usage: sysctl <key>")
                } else {
                    let inspector = INSPECTOR.lock();
                    match inspector.get(parts[1]) {
                        Some(val) => format!("{} = {}", parts[1], val),
                        None => format!("{}: not found", parts[1]),
                    }
                }
            }
            "test" => {
                let summary = super::testing::run_all();
                format!(
                    "Tests: {} total, {} passed, {} failed, {} skipped",
                    summary.total, summary.passed, summary.failed, summary.skipped
                )
            }
            "lsdev" => {
                let inspector = INSPECTOR.lock();
                let entries = inspector.list_all();
                let mut out = String::from("Device tree:\n");
                for (k, v) in entries {
                    out.push_str(&format!("  {} = {}\n", k, v));
                }
                out
            }
            "meminfo" => {
                let snap = MONITOR.lock().take_snapshot();
                format!(
                    "Memory: {} KB used / {} KB total, {} processes",
                    snap.memory_used_kb, snap.memory_total_kb, snap.process_count
                )
            }
            _ => format!("Unknown command: {}", parts[0]),
        }
    }
}

static MONITOR: Mutex<SystemMonitor> = Mutex::new(SystemMonitor::new());
static INSPECTOR: Mutex<DeviceInspector> = Mutex::new(DeviceInspector::new());
static COMMANDS: Mutex<CommandHandler> = Mutex::new(CommandHandler::new());

pub fn init() {
    INSPECTOR.lock().populate();
    crate::serial_println!(
        "  [devtools] Developer tools initialized (monitor, inspector, commands)"
    );
}

pub fn run_command(cmd: &str) -> String {
    COMMANDS.lock().handle(cmd)
}
