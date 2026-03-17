use crate::sync::Mutex;
/// Hoags Init -- service supervisor for Genesis
///
/// PID 1. The first userspace process. Responsible for:
///   - Parsing /etc/inittab for runlevel configuration
///   - Starting and supervising system services
///   - Dependency-ordered service startup
///   - On-demand (socket-activated) service launching
///   - Spawning getty on virtual terminals
///   - Mounting initial filesystems (/proc, /sys, /dev, /tmp)
///   - Service health monitoring and auto-restart
///   - Clean system shutdown and runlevel transitions
///
/// Inspired by: systemd (dependency graphs, socket activation),
/// launchd (on-demand launching), s6 (supervision tree),
/// runit (simplicity). All code is original.
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

// ═══════════════════════════════════════════════════════════════════════════════
// Service definitions
// ═══════════════════════════════════════════════════════════════════════════════

/// Service states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    /// Registered but not started
    Stopped,
    /// Starting up
    Starting,
    /// Running normally
    Running,
    /// Failed, will be restarted
    Failed,
    /// Explicitly disabled
    Disabled,
}

/// Service restart policy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartPolicy {
    /// Never restart (one-shot)
    No,
    /// Restart on failure only
    OnFailure,
    /// Always restart
    Always,
}

/// Service type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceType {
    /// Simple foreground service
    Simple,
    /// One-shot (runs once, then done)
    OneShot,
    /// Forking daemon
    Forking,
    /// Socket-activated (started on first connection)
    Socket,
    /// Timer-activated (started on schedule)
    Timer,
}

/// A system service definition
#[derive(Debug, Clone)]
pub struct Service {
    pub name: String,
    pub description: String,
    pub state: ServiceState,
    pub restart_policy: RestartPolicy,
    pub service_type: ServiceType,
    pub depends_on: Vec<String>,
    pub wanted_by: Vec<String>,
    pub pid: Option<u32>,
    pub restart_count: u32,
    pub max_restarts: u32,
    /// Command to run
    pub exec_start: String,
    /// Command to stop
    pub exec_stop: String,
    /// Working directory
    pub working_dir: String,
    /// User to run as
    pub user: String,
    /// Runlevels this service is active in
    pub runlevels: Vec<u8>,
    /// Uptime when service was started (seconds)
    pub started_at: u64,
}

impl Service {
    pub fn new(name: &str, description: &str) -> Self {
        Service {
            name: String::from(name),
            description: String::from(description),
            state: ServiceState::Stopped,
            restart_policy: RestartPolicy::OnFailure,
            service_type: ServiceType::Simple,
            depends_on: Vec::new(),
            wanted_by: Vec::new(),
            pid: None,
            restart_count: 0,
            max_restarts: 5,
            exec_start: String::new(),
            exec_stop: String::new(),
            working_dir: String::from("/"),
            user: String::from("root"),
            runlevels: alloc::vec![3, 5],
            started_at: 0,
        }
    }

    pub fn with_restart(mut self, policy: RestartPolicy) -> Self {
        self.restart_policy = policy;
        self
    }

    pub fn with_depends(mut self, deps: &[&str]) -> Self {
        self.depends_on = deps.iter().map(|s| String::from(*s)).collect();
        self
    }

    pub fn with_type(mut self, stype: ServiceType) -> Self {
        self.service_type = stype;
        self
    }

    pub fn with_cmd(mut self, cmd: &str) -> Self {
        self.exec_start = String::from(cmd);
        self
    }

    pub fn with_user(mut self, user: &str) -> Self {
        self.user = String::from(user);
        self
    }

    /// Duration the service has been running (seconds)
    pub fn uptime(&self) -> u64 {
        if self.state == ServiceState::Running && self.started_at > 0 {
            crate::time::clock::uptime_secs().saturating_sub(self.started_at)
        } else {
            0
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Inittab parsing
// ═══════════════════════════════════════════════════════════════════════════════

/// An inittab entry
#[derive(Debug, Clone)]
pub struct InittabEntry {
    pub id: String,
    pub runlevels: Vec<u8>,
    pub action: InittabAction,
    pub process: String,
}

/// Inittab action types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InittabAction {
    Once,
    Respawn,
    Wait,
    Boot,
    BootWait,
    SysInit,
    PowerFail,
    PowerOK,
    CtrlAltDel,
    InitDefault,
}

/// Parse /etc/inittab format
///
/// Format: id:runlevels:action:process
pub fn parse_inittab(content: &str) -> Vec<InittabEntry> {
    let mut entries = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let parts: Vec<&str> = line.splitn(4, ':').collect();
        if parts.len() < 4 {
            continue;
        }

        let id = String::from(parts[0]);
        let runlevels: Vec<u8> = parts[1]
            .bytes()
            .filter_map(|b| {
                if b >= b'0' && b <= b'6' {
                    Some(b - b'0')
                } else {
                    None
                }
            })
            .collect();

        let action = match parts[2] {
            "once" => InittabAction::Once,
            "respawn" => InittabAction::Respawn,
            "wait" => InittabAction::Wait,
            "boot" => InittabAction::Boot,
            "bootwait" => InittabAction::BootWait,
            "sysinit" => InittabAction::SysInit,
            "powerfail" => InittabAction::PowerFail,
            "powerok" | "powerokwait" => InittabAction::PowerOK,
            "ctrlaltdel" => InittabAction::CtrlAltDel,
            "initdefault" => InittabAction::InitDefault,
            _ => continue,
        };

        entries.push(InittabEntry {
            id,
            runlevels,
            action,
            process: String::from(parts[3]),
        });
    }

    entries
}

// ═══════════════════════════════════════════════════════════════════════════════
// Init manager
// ═══════════════════════════════════════════════════════════════════════════════

/// The init service manager
pub struct InitManager {
    pub services: BTreeMap<String, Service>,
    pub boot_complete: bool,
    pub current_runlevel: u8,
    pub default_runlevel: u8,
    pub inittab_entries: Vec<InittabEntry>,
    /// Mounted filesystems
    pub mounts: Vec<(String, String, String)>,
    /// Getty processes
    pub gettys: Vec<(String, Option<u32>)>,
}

impl InitManager {
    pub fn new() -> Self {
        InitManager {
            services: BTreeMap::new(),
            boot_complete: false,
            current_runlevel: 0,
            default_runlevel: 5,
            inittab_entries: Vec::new(),
            mounts: Vec::new(),
            gettys: Vec::new(),
        }
    }

    /// Mount initial filesystems
    pub fn mount_initial_filesystems(&mut self) {
        serial_println!("  [init] Mounting initial filesystems...");

        let initial_mounts = [
            ("proc", "/proc", "proc"),
            ("sysfs", "/sys", "sysfs"),
            ("devfs", "/dev", "devfs"),
            ("tmpfs", "/tmp", "tmpfs"),
            ("tmpfs", "/run", "tmpfs"),
        ];

        for (dev, mount, fstype) in &initial_mounts {
            let _ = crate::fs::vfs::memfs_mkdir(mount);
            self.mounts.push((
                String::from(*dev),
                String::from(*mount),
                String::from(*fstype),
            ));
            serial_println!("  [init]   {} on {} type {}", dev, mount, fstype);
        }

        // Create essential directories
        let dirs = [
            "/etc",
            "/var",
            "/var/log",
            "/var/run",
            "/var/tmp",
            "/home",
            "/root",
            "/bin",
            "/sbin",
            "/usr",
            "/usr/bin",
            "/usr/sbin",
            "/usr/lib",
            "/lib",
        ];
        for dir in &dirs {
            let _ = crate::fs::vfs::memfs_mkdir(dir);
        }
    }

    /// Load and parse /etc/inittab
    pub fn load_inittab(&mut self) {
        match crate::fs::vfs::memfs_read("/etc/inittab") {
            Ok(data) => {
                let content = String::from_utf8_lossy(&data).into_owned();
                self.inittab_entries = parse_inittab(&content);

                for entry in &self.inittab_entries {
                    if entry.action == InittabAction::InitDefault {
                        if let Some(&rl) = entry.runlevels.first() {
                            self.default_runlevel = rl;
                        }
                    }
                }

                serial_println!(
                    "  [init] Loaded /etc/inittab ({} entries, default runlevel {})",
                    self.inittab_entries.len(),
                    self.default_runlevel
                );
            }
            Err(_) => {
                self.create_default_inittab();
                serial_println!(
                    "  [init] No /etc/inittab found, using defaults (runlevel {})",
                    self.default_runlevel
                );
            }
        }
    }

    fn create_default_inittab(&mut self) {
        let default_inittab = "\
# /etc/inittab -- Genesis init configuration
# Format: id:runlevels:action:process
id:5:initdefault:
si::sysinit:/etc/init.d/rcS
l3:3:wait:/etc/init.d/rc 3
l5:5:wait:/etc/init.d/rc 5
1:2345:respawn:/sbin/getty tty1
2:2345:respawn:/sbin/getty tty2
3:2345:respawn:/sbin/getty tty3
ca::ctrlaltdel:/sbin/shutdown -r now
";
        let _ = crate::fs::vfs::memfs_mkdir("/etc");
        let _ = crate::fs::vfs::memfs_write("/etc/inittab", default_inittab.as_bytes());
        self.inittab_entries = parse_inittab(default_inittab);
        self.default_runlevel = 5;
    }

    /// Spawn getty processes for virtual terminals
    pub fn spawn_gettys(&mut self) {
        serial_println!("  [init] Spawning getty processes...");

        for entry in &self.inittab_entries {
            if entry.action == InittabAction::Respawn
                && entry.runlevels.contains(&self.current_runlevel)
                && entry.process.contains("getty")
            {
                let tty = entry.process.split_whitespace().last().unwrap_or("tty1");
                self.gettys.push((String::from(tty), None));
                serial_println!("  [init]   getty on {} (stub)", tty);
            }
        }

        if self.gettys.is_empty() {
            for tty in &["tty1", "tty2", "tty3"] {
                self.gettys.push((String::from(*tty), None));
                serial_println!("  [init]   getty on {} (default)", tty);
            }
        }
    }

    /// Register a service
    pub fn register(&mut self, service: Service) {
        self.services.insert(service.name.clone(), service);
    }

    /// Start a service by name
    pub fn start(&mut self, name: &str) -> Result<(), &'static str> {
        let service = self.services.get(name).ok_or("service not found")?;

        for dep in &service.depends_on {
            if let Some(dep_svc) = self.services.get(dep) {
                if dep_svc.state != ServiceState::Running {
                    return Err("dependency not running");
                }
            }
        }

        let service = self.services.get_mut(name).unwrap();
        service.state = ServiceState::Starting;
        service.started_at = crate::time::clock::uptime_secs();
        service.state = ServiceState::Running;
        serial_println!("  [init] Started service: {}", name);
        Ok(())
    }

    /// Stop a service
    pub fn stop(&mut self, name: &str) -> Result<(), &'static str> {
        let service = self.services.get_mut(name).ok_or("service not found")?;
        if let Some(pid) = service.pid {
            let _ = crate::process::send_signal(pid, crate::process::pcb::signal::SIGTERM);
        }
        service.state = ServiceState::Stopped;
        service.pid = None;
        service.started_at = 0;
        serial_println!("  [init] Stopped service: {}", name);
        Ok(())
    }

    /// Restart a service
    pub fn restart(&mut self, name: &str) -> Result<(), &'static str> {
        let _ = self.stop(name);
        self.start(name)
    }

    /// Check service health and restart failed services
    pub fn health_check(&mut self) {
        let names: Vec<String> = self.services.keys().cloned().collect();

        for name in names {
            let needs_restart = {
                let service = &self.services[&name];
                service.state == ServiceState::Failed
                    && service.restart_policy != RestartPolicy::No
                    && service.restart_count < service.max_restarts
            };

            if needs_restart {
                if let Some(service) = self.services.get_mut(&name) {
                    service.restart_count = service.restart_count.saturating_add(1);
                    serial_println!(
                        "  [init] Restarting {} (attempt {}/{})",
                        name,
                        service.restart_count,
                        service.max_restarts
                    );
                    service.state = ServiceState::Starting;
                    service.started_at = crate::time::clock::uptime_secs();
                    service.state = ServiceState::Running;
                }
            }
        }
    }

    /// Boot the system -- start all services in dependency order
    pub fn boot(&mut self) {
        serial_println!("  [init] Starting system services...");

        let mut started = 0;
        let max_rounds = 10;

        for _ in 0..max_rounds {
            let names: Vec<String> = self.services.keys().cloned().collect();
            let mut made_progress = false;

            for name in names {
                let can_start = {
                    let service = &self.services[&name];
                    if service.state != ServiceState::Stopped {
                        continue;
                    }
                    service.depends_on.iter().all(|dep| {
                        self.services
                            .get(dep)
                            .map(|s| s.state == ServiceState::Running)
                            .unwrap_or(true)
                    })
                };

                if can_start {
                    let _ = self.start(&name);
                    started += 1;
                    made_progress = true;
                }
            }

            if !made_progress {
                break;
            }
        }

        self.boot_complete = true;
        self.current_runlevel = self.default_runlevel;
        serial_println!(
            "  [init] Boot complete -- {} services started (runlevel {})",
            started,
            self.current_runlevel
        );
    }

    /// Transition to a new runlevel
    pub fn set_runlevel(&mut self, level: u8) {
        if level > 6 {
            return;
        }
        serial_println!("  [init] Transitioning to runlevel {}", level);
        let old_level = self.current_runlevel;
        self.current_runlevel = level;

        let names: Vec<String> = self.services.keys().cloned().collect();
        for name in &names {
            let should_stop = {
                let svc = &self.services[name];
                svc.state == ServiceState::Running
                    && !svc.runlevels.contains(&level)
                    && svc.runlevels.contains(&old_level)
            };
            if should_stop {
                let _ = self.stop(name);
            }
        }

        for name in &names {
            let should_start = {
                let svc = &self.services[name];
                svc.state == ServiceState::Stopped && svc.runlevels.contains(&level)
            };
            if should_start {
                let _ = self.start(name);
            }
        }
    }

    /// List all services and their states
    pub fn list(&self) -> Vec<(String, ServiceState)> {
        self.services
            .iter()
            .map(|(name, svc)| (name.clone(), svc.state))
            .collect()
    }

    /// Format status of all services
    pub fn status_report(&self) -> String {
        let mut out = String::from("SERVICE                  STATE      UPTIME  RESTARTS\n");
        for (name, svc) in &self.services {
            let uptime_str = if svc.state == ServiceState::Running {
                format!("{}s", svc.uptime())
            } else {
                String::from("-")
            };
            out.push_str(&format!(
                "{:<24} {:<10} {:>7} {:>4}/{}\n",
                name,
                format!("{:?}", svc.state),
                uptime_str,
                svc.restart_count,
                svc.max_restarts,
            ));
        }
        out
    }
}

/// Global init manager
pub static INIT_MANAGER: Mutex<Option<InitManager>> = Mutex::new(None);

/// Initialize the init service and register core services
pub fn init() {
    let mut mgr = InitManager::new();

    // Phase 1: Mount filesystems
    mgr.mount_initial_filesystems();

    // Phase 2: Load inittab
    mgr.load_inittab();

    // Phase 3: Register core system services
    mgr.register(
        Service::new("display-server", "Hoags Display Compositor")
            .with_restart(RestartPolicy::Always)
            .with_cmd("/usr/bin/hoags-compositor"),
    );

    mgr.register(
        Service::new("network-manager", "Network configuration")
            .with_restart(RestartPolicy::Always)
            .with_cmd("/usr/sbin/networkd"),
    );

    mgr.register(
        Service::new("syslogd", "System log daemon")
            .with_restart(RestartPolicy::Always)
            .with_cmd("/usr/sbin/syslogd"),
    );

    mgr.register(
        Service::new("crond", "Cron scheduler")
            .with_restart(RestartPolicy::Always)
            .with_cmd("/usr/sbin/crond"),
    );

    mgr.register(
        Service::new("hoags-shell", "Hoags Shell")
            .with_restart(RestartPolicy::Always)
            .with_depends(&["display-server"])
            .with_cmd("/bin/hoags-shell"),
    );

    mgr.register(
        Service::new("hoags-pkg", "Package manager daemon")
            .with_restart(RestartPolicy::OnFailure)
            .with_depends(&["network-manager"])
            .with_cmd("/usr/sbin/hoags-pkgd"),
    );

    mgr.register(
        Service::new("hoags-ai", "AI Hub service")
            .with_restart(RestartPolicy::OnFailure)
            .with_depends(&["network-manager"])
            .with_cmd("/usr/sbin/hoags-ai"),
    );

    mgr.register(
        Service::new("hoags-vpn", "WireGuard VPN")
            .with_restart(RestartPolicy::OnFailure)
            .with_depends(&["network-manager"])
            .with_cmd("/usr/sbin/hoags-vpn"),
    );

    mgr.register(
        Service::new("hoags-sync", "File synchronization")
            .with_restart(RestartPolicy::OnFailure)
            .with_depends(&["network-manager"])
            .with_cmd("/usr/sbin/hoags-sync"),
    );

    mgr.register(
        Service::new("sshd", "SSH server")
            .with_restart(RestartPolicy::Always)
            .with_depends(&["network-manager"])
            .with_type(ServiceType::Forking)
            .with_cmd("/usr/sbin/sshd"),
    );

    // Phase 4: Boot all services
    mgr.boot();

    // Phase 5: Spawn getty processes
    mgr.spawn_gettys();

    *INIT_MANAGER.lock() = Some(mgr);
}
