use alloc::string::String;
/// Hoags Service Files — systemd-like service definitions
///
/// Service files define how to start, stop, and manage services.
/// Uses our config parser format.
///
/// Example service file:
///   [service]
///   name = hoags-shell
///   description = "Hoags Shell"
///   type = simple
///   exec = /bin/hoags-shell
///   restart = on-failure
///   restart_delay = 1000
///
///   [dependencies]
///   after = display-server, network-manager
///   requires = display-server
use alloc::string::ToString;
use alloc::vec::Vec;

/// Service type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceType {
    Simple,  // main process is the service
    Forking, // forks and parent exits
    Oneshot, // runs once and exits
    Notify,  // sends ready notification
}

/// Restart policy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartPolicy {
    No,
    Always,
    OnFailure,
    OnAbnormal,
}

/// Parsed service file
#[derive(Debug, Clone)]
pub struct ServiceFile {
    pub name: String,
    pub description: String,
    pub service_type: ServiceType,
    pub exec_start: String,
    pub exec_stop: Option<String>,
    pub exec_reload: Option<String>,
    pub working_dir: Option<String>,
    pub user: Option<String>,
    pub group: Option<String>,
    pub restart: RestartPolicy,
    pub restart_delay_ms: u64,
    pub max_restarts: u32,
    pub timeout_start_ms: u64,
    pub timeout_stop_ms: u64,
    pub environment: Vec<(String, String)>,
    pub after: Vec<String>,
    pub requires: Vec<String>,
    pub wants: Vec<String>,
    pub conflicts: Vec<String>,
}

impl ServiceFile {
    /// Parse a service file from config string
    pub fn parse(input: &str) -> Result<Self, &'static str> {
        let config = crate::config::parser::Config::parse(input);

        let name = config
            .get("service", "name")
            .ok_or("missing name")?
            .to_string();

        let service_type = match config.get("service", "type").unwrap_or("simple") {
            "simple" => ServiceType::Simple,
            "forking" => ServiceType::Forking,
            "oneshot" => ServiceType::Oneshot,
            "notify" => ServiceType::Notify,
            _ => ServiceType::Simple,
        };

        let restart = match config.get("service", "restart").unwrap_or("no") {
            "always" => RestartPolicy::Always,
            "on-failure" => RestartPolicy::OnFailure,
            "on-abnormal" => RestartPolicy::OnAbnormal,
            _ => RestartPolicy::No,
        };

        fn parse_list(s: Option<&str>) -> Vec<String> {
            s.map(|v| v.split(',').map(|s| String::from(s.trim())).collect())
                .unwrap_or_default()
        }

        Ok(ServiceFile {
            name,
            description: config.get_or("service", "description", ""),
            service_type,
            exec_start: config
                .get("service", "exec")
                .ok_or("missing exec")?
                .to_string(),
            exec_stop: config.get("service", "exec_stop").map(String::from),
            exec_reload: config.get("service", "exec_reload").map(String::from),
            working_dir: config.get("service", "working_dir").map(String::from),
            user: config.get("service", "user").map(String::from),
            group: config.get("service", "group").map(String::from),
            restart,
            restart_delay_ms: config.get_int("service", "restart_delay").unwrap_or(1000) as u64,
            max_restarts: config.get_int("service", "max_restarts").unwrap_or(5) as u32,
            timeout_start_ms: config.get_int("service", "timeout_start").unwrap_or(30000) as u64,
            timeout_stop_ms: config.get_int("service", "timeout_stop").unwrap_or(10000) as u64,
            environment: Vec::new(),
            after: parse_list(config.get("dependencies", "after")),
            requires: parse_list(config.get("dependencies", "requires")),
            wants: parse_list(config.get("dependencies", "wants")),
            conflicts: parse_list(config.get("dependencies", "conflicts")),
        })
    }
}

/// Built-in service definitions for core OS services
pub fn core_services() -> Vec<ServiceFile> {
    alloc::vec![
        ServiceFile::parse(
            "[service]\nname = display-server\ndescription = \"Hoags Compositor\"\n\
             type = notify\nexec = /bin/hoags-compositor\nrestart = always\n\
             [dependencies]\nafter = network-manager"
        )
        .unwrap(),
        ServiceFile::parse(
            "[service]\nname = network-manager\ndescription = \"Network Manager\"\n\
             type = simple\nexec = /bin/hoags-netmgr\nrestart = always\n\
             [dependencies]"
        )
        .unwrap(),
        ServiceFile::parse(
            "[service]\nname = hoags-shell\ndescription = \"Hoags Shell\"\n\
             type = simple\nexec = /bin/hoags-shell\nrestart = on-failure\n\
             [dependencies]\nafter = display-server, network-manager"
        )
        .unwrap(),
        ServiceFile::parse(
            "[service]\nname = hoags-ai\ndescription = \"Hoags AI Assistant\"\n\
             type = simple\nexec = /bin/hoags-ai\nrestart = on-failure\n\
             [dependencies]\nafter = display-server\nwants = network-manager"
        )
        .unwrap(),
        ServiceFile::parse(
            "[service]\nname = bluetooth\ndescription = \"Bluetooth Service\"\n\
             type = simple\nexec = /bin/hoags-bt\nrestart = on-failure\n\
             [dependencies]"
        )
        .unwrap(),
        ServiceFile::parse(
            "[service]\nname = audio\ndescription = \"Audio Service\"\n\
             type = simple\nexec = /bin/hoags-audio\nrestart = always\n\
             [dependencies]"
        )
        .unwrap(),
    ]
}
