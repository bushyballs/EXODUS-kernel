use crate::sync::Mutex;
/// Flatpak-like Sandboxed Applications for Genesis
///
/// Provides application sandboxing with:
///   - Runtime environments (shared libraries, base system)
///   - Fine-grained permissions (filesystem, network, devices)
///   - Portal system for controlled access to host resources
///   - Filesystem access controls (read-only, read-write, deny)
///   - D-Bus proxy for inter-process communication filtering
///
/// Inspired by: Flatpak, Snap confinement, Firejail sandboxing.
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Runtimes
// ---------------------------------------------------------------------------

/// A runtime provides the base system libraries for sandboxed apps
#[derive(Debug, Clone)]
pub struct Runtime {
    pub id: String,
    pub name: String,
    pub version: String,
    pub arch: String,
    pub size_bytes: u64,
    pub components: Vec<String>,
    pub sdk_extension: Option<String>,
    pub installed: bool,
    pub ref_count: u32,
}

impl Runtime {
    pub fn new(id: &str, name: &str, version: &str) -> Self {
        Runtime {
            id: String::from(id),
            name: String::from(name),
            version: String::from(version),
            arch: String::from("x86_64"),
            size_bytes: 0,
            components: Vec::new(),
            sdk_extension: None,
            installed: false,
            ref_count: 0,
        }
    }

    /// Full qualified reference: "runtime/org.genesis.Platform/x86_64/0.3"
    pub fn full_ref(&self) -> String {
        alloc::format!("runtime/{}/{}/{}", self.id, self.arch, self.version)
    }
}

// ---------------------------------------------------------------------------
// Permissions
// ---------------------------------------------------------------------------

/// Permission categories for sandboxed apps
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    /// Network access
    Network,
    /// Access to the X11/Wayland display
    Display,
    /// Audio playback/recording
    Audio,
    /// Camera access
    Camera,
    /// Bluetooth access
    Bluetooth,
    /// GPU acceleration
    Gpu,
    /// Access to USB devices
    Usb,
    /// Access to all devices
    AllDevices,
    /// System D-Bus access
    SystemDbus,
    /// Session D-Bus access
    SessionDbus,
    /// Ability to run other sandboxed apps
    SubSandbox,
    /// Access to host SSH agent
    SshAuth,
    /// Ability to send desktop notifications
    Notifications,
}

impl Permission {
    pub fn name(self) -> &'static str {
        match self {
            Permission::Network => "network",
            Permission::Display => "display",
            Permission::Audio => "audio",
            Permission::Camera => "camera",
            Permission::Bluetooth => "bluetooth",
            Permission::Gpu => "gpu",
            Permission::Usb => "usb",
            Permission::AllDevices => "all-devices",
            Permission::SystemDbus => "system-dbus",
            Permission::SessionDbus => "session-dbus",
            Permission::SubSandbox => "sub-sandbox",
            Permission::SshAuth => "ssh-auth",
            Permission::Notifications => "notifications",
        }
    }
}

/// Filesystem access level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsAccess {
    ReadOnly,
    ReadWrite,
    Create, // can create new files but not modify existing
    Deny,
}

impl FsAccess {
    pub fn name(self) -> &'static str {
        match self {
            FsAccess::ReadOnly => "ro",
            FsAccess::ReadWrite => "rw",
            FsAccess::Create => "create",
            FsAccess::Deny => "deny",
        }
    }
}

/// A filesystem access rule
#[derive(Debug, Clone)]
pub struct FsRule {
    pub path: String,
    pub access: FsAccess,
}

// ---------------------------------------------------------------------------
// Portals
// ---------------------------------------------------------------------------

/// Portals provide controlled access to host resources
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalKind {
    FileChooser,
    ScreenCapture,
    Printing,
    Camera,
    Location,
    Notification,
    Clipboard,
    OpenUri,
    Email,
    Secret, // keyring access
    Settings,
    Background,
    RemoteDesktop,
}

impl PortalKind {
    pub fn name(self) -> &'static str {
        match self {
            PortalKind::FileChooser => "file-chooser",
            PortalKind::ScreenCapture => "screen-capture",
            PortalKind::Printing => "printing",
            PortalKind::Camera => "camera",
            PortalKind::Location => "location",
            PortalKind::Notification => "notification",
            PortalKind::Clipboard => "clipboard",
            PortalKind::OpenUri => "open-uri",
            PortalKind::Email => "email",
            PortalKind::Secret => "secret",
            PortalKind::Settings => "settings",
            PortalKind::Background => "background",
            PortalKind::RemoteDesktop => "remote-desktop",
        }
    }

    pub fn dbus_interface(self) -> &'static str {
        match self {
            PortalKind::FileChooser => "org.freedesktop.portal.FileChooser",
            PortalKind::ScreenCapture => "org.freedesktop.portal.ScreenCast",
            PortalKind::Printing => "org.freedesktop.portal.Print",
            PortalKind::Camera => "org.freedesktop.portal.Camera",
            PortalKind::Location => "org.freedesktop.portal.Location",
            PortalKind::Notification => "org.freedesktop.portal.Notification",
            PortalKind::Clipboard => "org.freedesktop.portal.Clipboard",
            PortalKind::OpenUri => "org.freedesktop.portal.OpenURI",
            PortalKind::Email => "org.freedesktop.portal.Email",
            PortalKind::Secret => "org.freedesktop.portal.Secret",
            PortalKind::Settings => "org.freedesktop.portal.Settings",
            PortalKind::Background => "org.freedesktop.portal.Background",
            PortalKind::RemoteDesktop => "org.freedesktop.portal.RemoteDesktop",
        }
    }
}

/// A portal request from a sandboxed app
#[derive(Debug, Clone)]
pub struct PortalRequest {
    pub app_id: String,
    pub portal: PortalKind,
    pub granted: bool,
    pub timestamp: u64,
}

// ---------------------------------------------------------------------------
// D-Bus Proxy
// ---------------------------------------------------------------------------

/// D-Bus proxy filter rule
#[derive(Debug, Clone)]
pub struct DbusFilter {
    pub bus_name: String,
    pub allowed: bool,
    pub own: bool,  // can own this bus name
    pub talk: bool, // can send messages to this name
    pub see: bool,  // can see this name exists
}

/// D-Bus proxy for a sandboxed app
#[derive(Debug, Clone)]
pub struct DbusProxy {
    pub app_id: String,
    pub session_filters: Vec<DbusFilter>,
    pub system_filters: Vec<DbusFilter>,
    pub log_denials: bool,
    pub denial_count: u64,
}

impl DbusProxy {
    pub fn new(app_id: &str) -> Self {
        DbusProxy {
            app_id: String::from(app_id),
            session_filters: Vec::new(),
            system_filters: Vec::new(),
            log_denials: true,
            denial_count: 0,
        }
    }

    /// Add a session bus filter
    pub fn allow_session(&mut self, bus_name: &str, own: bool, talk: bool) {
        self.session_filters.push(DbusFilter {
            bus_name: String::from(bus_name),
            allowed: true,
            own,
            talk,
            see: true,
        });
    }

    /// Block a session bus name
    pub fn deny_session(&mut self, bus_name: &str) {
        self.session_filters.push(DbusFilter {
            bus_name: String::from(bus_name),
            allowed: false,
            own: false,
            talk: false,
            see: false,
        });
    }

    /// Check if a bus name is allowed on the session bus
    pub fn check_session(&mut self, bus_name: &str) -> bool {
        for filter in &self.session_filters {
            if filter.bus_name == bus_name {
                if !filter.allowed {
                    self.denial_count = self.denial_count.saturating_add(1);
                }
                return filter.allowed;
            }
        }
        // Default deny
        self.denial_count = self.denial_count.saturating_add(1);
        false
    }
}

// ---------------------------------------------------------------------------
// Sandboxed Application
// ---------------------------------------------------------------------------

/// A Flatpak-like sandboxed application
#[derive(Debug, Clone)]
pub struct SandboxedApp {
    pub app_id: String, // e.g., "com.hoagsinc.TextEditor"
    pub name: String,
    pub version: String,
    pub runtime_id: String,
    pub command: String,
    pub permissions: Vec<Permission>,
    pub fs_rules: Vec<FsRule>,
    pub portals_allowed: Vec<PortalKind>,
    pub env_vars: Vec<(String, String)>,
    pub installed: bool,
    pub running: bool,
    pub pid: Option<u32>,
    pub install_size_bytes: u64,
    pub data_dir: String,
    pub cache_dir: String,
}

impl SandboxedApp {
    pub fn new(app_id: &str, name: &str, runtime_id: &str) -> Self {
        SandboxedApp {
            app_id: String::from(app_id),
            name: String::from(name),
            version: String::from("1.0.0"),
            runtime_id: String::from(runtime_id),
            command: String::new(),
            permissions: Vec::new(),
            fs_rules: Vec::new(),
            portals_allowed: Vec::new(),
            env_vars: Vec::new(),
            installed: false,
            running: false,
            pid: None,
            install_size_bytes: 0,
            data_dir: alloc::format!("/var/lib/flatpak/app/{}/data", app_id),
            cache_dir: alloc::format!("/var/lib/flatpak/app/{}/cache", app_id),
        }
    }

    /// Grant a permission
    pub fn grant_permission(&mut self, perm: Permission) {
        if !self.permissions.contains(&perm) {
            self.permissions.push(perm);
        }
    }

    /// Revoke a permission
    pub fn revoke_permission(&mut self, perm: Permission) {
        self.permissions.retain(|p| *p != perm);
    }

    /// Check if app has a specific permission
    pub fn has_permission(&self, perm: Permission) -> bool {
        self.permissions.contains(&perm)
    }

    /// Add a filesystem access rule
    pub fn add_fs_rule(&mut self, path: &str, access: FsAccess) {
        self.fs_rules.push(FsRule {
            path: String::from(path),
            access,
        });
    }

    /// Check filesystem access for a given path
    pub fn check_fs_access(&self, path: &str) -> FsAccess {
        // Find the most specific matching rule
        let mut best_match: Option<&FsRule> = None;
        let mut best_len = 0usize;

        for rule in &self.fs_rules {
            if path.starts_with(&rule.path) && rule.path.len() > best_len {
                best_len = rule.path.len();
                best_match = Some(rule);
            }
        }

        best_match.map(|r| r.access).unwrap_or(FsAccess::Deny)
    }

    /// Allow a portal
    pub fn allow_portal(&mut self, portal: PortalKind) {
        if !self.portals_allowed.contains(&portal) {
            self.portals_allowed.push(portal);
        }
    }

    /// Check if a portal is allowed
    pub fn has_portal(&self, portal: PortalKind) -> bool {
        self.portals_allowed.contains(&portal)
    }

    /// Full reference string
    pub fn full_ref(&self) -> String {
        alloc::format!("app/{}/x86_64/{}", self.app_id, self.version)
    }

    /// Format permissions for display
    pub fn format_permissions(&self) -> String {
        let mut out = alloc::format!("Permissions for {}:\n", self.name);
        for perm in &self.permissions {
            out.push_str(&alloc::format!("  [+] {}\n", perm.name()));
        }
        for rule in &self.fs_rules {
            out.push_str(&alloc::format!(
                "  [fs] {} -> {}\n",
                rule.path,
                rule.access.name()
            ));
        }
        for portal in &self.portals_allowed {
            out.push_str(&alloc::format!("  [portal] {}\n", portal.name()));
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Flatpak Manager
// ---------------------------------------------------------------------------

pub struct FlatpakManager {
    pub runtimes: BTreeMap<String, Runtime>,
    pub apps: BTreeMap<String, SandboxedApp>,
    pub dbus_proxies: BTreeMap<String, DbusProxy>,
    pub portal_log: Vec<PortalRequest>,
    pub install_dir: String,
    pub max_portal_log: usize,
}

impl FlatpakManager {
    pub const fn new() -> Self {
        FlatpakManager {
            runtimes: BTreeMap::new(),
            apps: BTreeMap::new(),
            dbus_proxies: BTreeMap::new(),
            portal_log: Vec::new(),
            install_dir: String::new(),
            max_portal_log: 1024,
        }
    }

    /// Register a runtime
    pub fn add_runtime(&mut self, runtime: Runtime) {
        self.runtimes.insert(runtime.id.clone(), runtime);
    }

    /// Install a sandboxed app
    pub fn install_app(&mut self, mut app: SandboxedApp) -> Result<(), FlatpakError> {
        // Verify runtime exists
        if !self.runtimes.contains_key(&app.runtime_id) {
            return Err(FlatpakError::RuntimeNotFound(app.runtime_id.clone()));
        }

        // Increment runtime ref count
        if let Some(rt) = self.runtimes.get_mut(&app.runtime_id) {
            rt.ref_count = rt.ref_count.saturating_add(1);
        }

        app.installed = true;

        // Create default D-Bus proxy for the app
        let mut proxy = DbusProxy::new(&app.app_id);
        // Allow own name by default
        proxy.allow_session(&app.app_id, true, true);
        // Allow notifications portal if permitted
        if app.has_portal(PortalKind::Notification) {
            proxy.allow_session("org.freedesktop.Notifications", false, true);
        }
        self.dbus_proxies.insert(app.app_id.clone(), proxy);

        serial_println!("  [flatpak] Installed: {} ({})", app.name, app.app_id);
        self.apps.insert(app.app_id.clone(), app);
        Ok(())
    }

    /// Uninstall a sandboxed app
    pub fn uninstall_app(&mut self, app_id: &str) -> Result<(), FlatpakError> {
        let app = self
            .apps
            .remove(app_id)
            .ok_or(FlatpakError::AppNotFound(String::from(app_id)))?;

        // Decrement runtime ref count
        if let Some(rt) = self.runtimes.get_mut(&app.runtime_id) {
            if rt.ref_count > 0 {
                rt.ref_count -= 1;
            }
        }

        self.dbus_proxies.remove(app_id);
        serial_println!("  [flatpak] Uninstalled: {}", app_id);
        Ok(())
    }

    /// Launch a sandboxed app
    pub fn launch(&mut self, app_id: &str) -> Result<u32, FlatpakError> {
        let app = self
            .apps
            .get_mut(app_id)
            .ok_or(FlatpakError::AppNotFound(String::from(app_id)))?;

        if !app.installed {
            return Err(FlatpakError::NotInstalled(String::from(app_id)));
        }
        if app.running {
            return Err(FlatpakError::AlreadyRunning(String::from(app_id)));
        }

        // In a real OS, this would create a namespace, set up seccomp, mount
        // overlay filesystem, launch the app process, etc.
        let pid = crate::process::getpid() + 100; // placeholder PID
        app.running = true;
        app.pid = Some(pid);

        serial_println!("  [flatpak] Launched {} (PID {})", app.name, pid);
        Ok(pid)
    }

    /// Stop a sandboxed app
    pub fn stop(&mut self, app_id: &str) -> Result<(), FlatpakError> {
        let app = self
            .apps
            .get_mut(app_id)
            .ok_or(FlatpakError::AppNotFound(String::from(app_id)))?;

        app.running = false;
        app.pid = None;
        serial_println!("  [flatpak] Stopped {}", app.name);
        Ok(())
    }

    /// Handle a portal request
    pub fn request_portal(&mut self, app_id: &str, portal: PortalKind) -> bool {
        let granted = self
            .apps
            .get(app_id)
            .map(|app| app.has_portal(portal))
            .unwrap_or(false);

        if self.portal_log.len() >= self.max_portal_log {
            self.portal_log.remove(0);
        }
        self.portal_log.push(PortalRequest {
            app_id: String::from(app_id),
            portal,
            granted,
            timestamp: crate::time::clock::uptime_secs(),
        });

        granted
    }

    /// List installed apps
    pub fn list_apps(&self) -> Vec<&SandboxedApp> {
        self.apps.values().collect()
    }

    /// List installed runtimes
    pub fn list_runtimes(&self) -> Vec<&Runtime> {
        self.runtimes.values().collect()
    }

    /// Get portal request log
    pub fn portal_history(&self, n: usize) -> Vec<&PortalRequest> {
        let len = self.portal_log.len();
        let skip = if len > n { len - n } else { 0 };
        self.portal_log.iter().skip(skip).collect()
    }
}

/// Flatpak errors
#[derive(Debug)]
pub enum FlatpakError {
    RuntimeNotFound(String),
    AppNotFound(String),
    NotInstalled(String),
    AlreadyRunning(String),
    PermissionDenied(String),
    InvalidManifest(String),
}

// ---------------------------------------------------------------------------
// Global State
// ---------------------------------------------------------------------------

static FLATPAK: Mutex<FlatpakManager> = Mutex::new(FlatpakManager::new());

/// Initialize the Flatpak-like sandboxing subsystem
pub fn init() {
    let mut mgr = FLATPAK.lock();

    mgr.install_dir = String::from("/var/lib/flatpak");

    // Register the base Genesis runtime
    let mut base_rt = Runtime::new("org.genesis.Platform", "Genesis Platform Runtime", "0.3");
    base_rt.size_bytes = 256 * 1024 * 1024; // 256 MiB
    base_rt.components = vec![
        String::from("glibc"),
        String::from("libstdc++"),
        String::from("openssl"),
        String::from("zlib"),
        String::from("mesa"),
        String::from("wayland"),
        String::from("dbus"),
    ];
    base_rt.sdk_extension = Some(String::from("org.genesis.Sdk"));
    base_rt.installed = true;
    mgr.add_runtime(base_rt);

    // Register a minimal runtime
    let mut minimal_rt = Runtime::new("org.genesis.Minimal", "Genesis Minimal Runtime", "0.3");
    minimal_rt.size_bytes = 32 * 1024 * 1024; // 32 MiB
    minimal_rt.components = vec![String::from("glibc"), String::from("openssl")];
    minimal_rt.installed = true;
    mgr.add_runtime(minimal_rt);

    // Install a sample sandboxed app
    let mut editor = SandboxedApp::new(
        "com.hoagsinc.TextEditor",
        "Genesis Text Editor",
        "org.genesis.Platform",
    );
    editor.version = String::from("1.2.0");
    editor.command = String::from("/app/bin/gedit");
    editor.grant_permission(Permission::Display);
    editor.grant_permission(Permission::Gpu);
    editor.add_fs_rule("/home", FsAccess::ReadWrite);
    editor.add_fs_rule("/tmp", FsAccess::ReadWrite);
    editor.add_fs_rule("/etc", FsAccess::ReadOnly);
    editor.allow_portal(PortalKind::FileChooser);
    editor.allow_portal(PortalKind::Notification);
    editor.allow_portal(PortalKind::Clipboard);
    let _ = mgr.install_app(editor);

    let stats_apps = mgr.apps.len();
    let stats_rts = mgr.runtimes.len();
    serial_println!(
        "  Flatpak: {} runtimes, {} apps installed",
        stats_rts,
        stats_apps
    );
}

/// Install a sandboxed app
pub fn install(app: SandboxedApp) -> Result<(), FlatpakError> {
    FLATPAK.lock().install_app(app)
}

/// Uninstall an app
pub fn uninstall(app_id: &str) -> Result<(), FlatpakError> {
    FLATPAK.lock().uninstall_app(app_id)
}

/// Launch an app
pub fn launch(app_id: &str) -> Result<u32, FlatpakError> {
    FLATPAK.lock().launch(app_id)
}

/// Stop an app
pub fn stop(app_id: &str) -> Result<(), FlatpakError> {
    FLATPAK.lock().stop(app_id)
}

/// List installed apps
pub fn list_apps() -> Vec<String> {
    FLATPAK
        .lock()
        .list_apps()
        .iter()
        .map(|a| {
            alloc::format!(
                "{} ({}) v{} [{}]",
                a.name,
                a.app_id,
                a.version,
                if a.running { "running" } else { "stopped" }
            )
        })
        .collect()
}

/// Request a portal on behalf of an app
pub fn request_portal(app_id: &str, portal: PortalKind) -> bool {
    FLATPAK.lock().request_portal(app_id, portal)
}
