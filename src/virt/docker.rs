use crate::sync::Mutex;
use crate::{serial_print, serial_println};
/// Docker-compatible container runtime for Genesis
///
/// Provides image layer management, container create/start/stop lifecycle,
/// port mapping, volume mounts, and Docker API-compatible abstractions.
use alloc::vec::Vec;

/// Read the CPU timestamp counter and return a millisecond timestamp.
///
/// TSC frequency is estimated at 3 GHz (3_000_000 ticks/ms).  Replace with
/// a calibrated value once the ACPI/HPET timer subsystem is available.
#[inline]
fn get_timestamp_ms() -> u64 {
    let tsc: u64;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            "shl rdx, 32",
            "or rax, rdx",
            out("rax") tsc,
            out("rdx") _,
            options(nomem, nostack, preserves_flags),
        );
    }
    tsc / 3_000_000
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_IMAGES: usize = 128;
const MAX_CONTAINERS: usize = 64;
const MAX_LAYERS_PER_IMAGE: usize = 32;
const MAX_PORT_MAPPINGS: usize = 16;
const MAX_VOLUME_MOUNTS: usize = 8;
const MAX_ENV_VARS: usize = 32;

// ---------------------------------------------------------------------------
// Image layer model
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LayerType {
    Base,
    Add,
    Copy,
    Run,
    Env,
    Metadata,
}

#[derive(Clone, Copy)]
pub struct ImageLayer {
    pub digest: u64,
    pub parent_digest: u64,
    pub layer_type: LayerType,
    pub size_bytes: u64,
    pub created_timestamp: u64,
    pub compressed_size: u64,
}

#[derive(Clone)]
pub struct DockerImage {
    pub id: u32,
    pub repo_hash: u64,
    pub tag_hash: u64,
    pub manifest_digest: u64,
    pub layers: Vec<ImageLayer>,
    pub total_size: u64,
    pub config_digest: u64,
    pub architecture: u32,
    pub created_timestamp: u64,
    pub entrypoint_hash: u64,
    pub cmd_hash: u64,
}

// ---------------------------------------------------------------------------
// Port mapping
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PortProtocol {
    Tcp,
    Udp,
}

#[derive(Clone, Copy)]
pub struct PortMapping {
    pub host_port: u16,
    pub container_port: u16,
    pub protocol: PortProtocol,
    pub host_ip: u32,
}

// ---------------------------------------------------------------------------
// Volume mounts
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MountType {
    Bind,
    Volume,
    Tmpfs,
    NamedVolume,
}

#[derive(Clone, Copy)]
pub struct VolumeMount {
    pub mount_type: MountType,
    pub source_hash: u64,
    pub target_hash: u64,
    pub read_only: bool,
    pub size_limit_mb: u32,
}

// ---------------------------------------------------------------------------
// Container state + config
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DockerContainerState {
    Created,
    Running,
    Paused,
    Restarting,
    Exited,
    Dead,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RestartPolicy {
    No,
    Always,
    OnFailure,
    UnlessStopped,
}

#[derive(Clone)]
pub struct DockerContainerConfig {
    pub image_id: u32,
    pub name_hash: u64,
    pub hostname_hash: u64,
    pub memory_limit_mb: u32,
    pub cpu_shares: u16,
    pub cpu_quota_us: u32,
    pub pid_limit: u32,
    pub port_mappings: Vec<PortMapping>,
    pub volume_mounts: Vec<VolumeMount>,
    pub env_hashes: Vec<u64>,
    pub restart_policy: RestartPolicy,
    pub privileged: bool,
    pub network_mode_hash: u64,
    pub user_id: u32,
    pub group_id: u32,
    pub working_dir_hash: u64,
    pub entrypoint_hash: u64,
    pub cmd_hash: u64,
}

#[derive(Clone)]
pub struct DockerContainer {
    pub id: u32,
    pub config: DockerContainerConfig,
    pub state: DockerContainerState,
    pub pid: u32,
    pub exit_code: i32,
    pub start_time: u64,
    pub finish_time: u64,
    pub restart_count: u32,
    pub cpu_usage_ns: u64,
    pub memory_used_mb: u32,
    pub network_rx_bytes: u64,
    pub network_tx_bytes: u64,
    pub block_read_bytes: u64,
    pub block_write_bytes: u64,
    pub oom_killed: bool,
}

// ---------------------------------------------------------------------------
// Docker runtime
// ---------------------------------------------------------------------------

pub struct DockerRuntime {
    images: Vec<DockerImage>,
    containers: Vec<DockerContainer>,
    next_image_id: u32,
    next_container_id: u32,
    total_pulls: u32,
    total_started: u32,
    storage_used_bytes: u64,
}

impl DockerRuntime {
    fn new() -> Self {
        Self {
            images: Vec::new(),
            containers: Vec::new(),
            next_image_id: 1,
            next_container_id: 1,
            total_pulls: 0,
            total_started: 0,
            storage_used_bytes: 0,
        }
    }

    // -- Image management ---------------------------------------------------

    pub fn register_image(
        &mut self,
        repo_hash: u64,
        tag_hash: u64,
        manifest_digest: u64,
        config_digest: u64,
        entrypoint_hash: u64,
        cmd_hash: u64,
    ) -> Result<u32, &'static str> {
        if self.images.len() >= MAX_IMAGES {
            return Err("Image store full");
        }

        // Duplicate check
        if self
            .images
            .iter()
            .any(|i| i.repo_hash == repo_hash && i.tag_hash == tag_hash)
        {
            return Err("Image already registered");
        }

        let id = self.next_image_id;
        self.next_image_id = self.next_image_id.saturating_add(1);
        self.total_pulls = self.total_pulls.saturating_add(1);

        let image = DockerImage {
            id,
            repo_hash,
            tag_hash,
            manifest_digest,
            layers: Vec::new(),
            total_size: 0,
            config_digest,
            architecture: 0x8664, // x86_64
            created_timestamp: 0,
            entrypoint_hash,
            cmd_hash,
        };

        self.images.push(image);
        Ok(id)
    }

    pub fn add_layer_to_image(
        &mut self,
        image_id: u32,
        digest: u64,
        parent_digest: u64,
        layer_type: LayerType,
        size_bytes: u64,
        compressed_size: u64,
    ) -> Result<(), &'static str> {
        let image = self
            .images
            .iter_mut()
            .find(|i| i.id == image_id)
            .ok_or("Image not found")?;

        if image.layers.len() >= MAX_LAYERS_PER_IMAGE {
            return Err("Layer limit per image reached");
        }

        let layer = ImageLayer {
            digest,
            parent_digest,
            layer_type,
            size_bytes,
            created_timestamp: 0,
            compressed_size,
        };

        image.total_size += size_bytes;
        self.storage_used_bytes += size_bytes;
        image.layers.push(layer);
        Ok(())
    }

    pub fn remove_image(&mut self, image_id: u32) -> Result<(), &'static str> {
        // Ensure no running container references this image
        let in_use = self.containers.iter().any(|c| {
            c.config.image_id == image_id
                && c.state != DockerContainerState::Exited
                && c.state != DockerContainerState::Dead
        });
        if in_use {
            return Err("Image in use by running container");
        }

        let idx = self
            .images
            .iter()
            .position(|i| i.id == image_id)
            .ok_or("Image not found")?;

        let freed = self.images[idx].total_size;
        self.images.remove(idx);
        self.storage_used_bytes = self.storage_used_bytes.saturating_sub(freed);
        Ok(())
    }

    pub fn get_image_info(&self, image_id: u32) -> Result<(u64, u64, u64, usize), &'static str> {
        let image = self
            .images
            .iter()
            .find(|i| i.id == image_id)
            .ok_or("Image not found")?;
        Ok((
            image.repo_hash,
            image.tag_hash,
            image.total_size,
            image.layers.len(),
        ))
    }

    pub fn list_images(&self) -> Vec<(u32, u64, u64, u64)> {
        self.images
            .iter()
            .map(|i| (i.id, i.repo_hash, i.tag_hash, i.total_size))
            .collect()
    }

    pub fn image_count(&self) -> usize {
        self.images.len()
    }

    // -- Container lifecycle ------------------------------------------------

    pub fn create_container(&mut self, config: DockerContainerConfig) -> Result<u32, &'static str> {
        if self.containers.len() >= MAX_CONTAINERS {
            return Err("Container limit reached");
        }

        // Validate image exists
        if !self.images.iter().any(|i| i.id == config.image_id) {
            return Err("Image not found");
        }

        if config.port_mappings.len() > MAX_PORT_MAPPINGS {
            return Err("Too many port mappings");
        }
        if config.volume_mounts.len() > MAX_VOLUME_MOUNTS {
            return Err("Too many volume mounts");
        }
        if config.env_hashes.len() > MAX_ENV_VARS {
            return Err("Too many environment variables");
        }

        // Check for host port conflicts
        for pm in config.port_mappings.iter() {
            let conflict = self.containers.iter().any(|c| {
                c.state == DockerContainerState::Running
                    && c.config
                        .port_mappings
                        .iter()
                        .any(|p| p.host_port == pm.host_port && p.protocol == pm.protocol)
            });
            if conflict {
                return Err("Host port already in use");
            }
        }

        let id = self.next_container_id;
        self.next_container_id = self.next_container_id.saturating_add(1);

        let container = DockerContainer {
            id,
            config,
            state: DockerContainerState::Created,
            pid: 0,
            exit_code: 0,
            start_time: get_timestamp_ms(),
            finish_time: 0,
            restart_count: 0,
            cpu_usage_ns: 0,
            memory_used_mb: 0,
            network_rx_bytes: 0,
            network_tx_bytes: 0,
            block_read_bytes: 0,
            block_write_bytes: 0,
            oom_killed: false,
        };

        self.containers.push(container);
        Ok(id)
    }

    pub fn start_container(&mut self, id: u32) -> Result<(), &'static str> {
        let container = self
            .containers
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or("Container not found")?;

        match container.state {
            DockerContainerState::Created | DockerContainerState::Exited => {}
            _ => return Err("Container not in startable state"),
        }

        container.state = DockerContainerState::Running;
        container.pid = id * 1000;
        container.start_time = get_timestamp_ms();
        container.exit_code = 0;
        self.total_started = self.total_started.saturating_add(1);

        Ok(())
    }

    pub fn stop_container(&mut self, id: u32, _timeout_secs: u32) -> Result<(), &'static str> {
        let container = self
            .containers
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or("Container not found")?;

        if container.state != DockerContainerState::Running
            && container.state != DockerContainerState::Paused
        {
            return Err("Container not running");
        }

        container.state = DockerContainerState::Exited;
        container.pid = 0;
        container.finish_time = get_timestamp_ms();
        container.exit_code = 0;

        Ok(())
    }

    pub fn kill_container(&mut self, id: u32, signal: u32) -> Result<(), &'static str> {
        let container = self
            .containers
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or("Container not found")?;

        if container.state != DockerContainerState::Running {
            return Err("Container not running");
        }

        let _ = signal; // Stub: would send signal to container process
        container.state = DockerContainerState::Exited;
        container.pid = 0;
        container.exit_code = 137; // Killed
        container.finish_time = get_timestamp_ms();

        Ok(())
    }

    pub fn pause_container(&mut self, id: u32) -> Result<(), &'static str> {
        let container = self
            .containers
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or("Container not found")?;

        if container.state != DockerContainerState::Running {
            return Err("Container not running");
        }

        container.state = DockerContainerState::Paused;
        Ok(())
    }

    pub fn unpause_container(&mut self, id: u32) -> Result<(), &'static str> {
        let container = self
            .containers
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or("Container not found")?;

        if container.state != DockerContainerState::Paused {
            return Err("Container not paused");
        }

        container.state = DockerContainerState::Running;
        Ok(())
    }

    pub fn restart_container(&mut self, id: u32) -> Result<(), &'static str> {
        let container = self
            .containers
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or("Container not found")?;

        container.state = DockerContainerState::Restarting;
        container.restart_count = container.restart_count.saturating_add(1);

        // Immediately transition to running (stub)
        container.state = DockerContainerState::Running;
        container.pid = id * 1000 + container.restart_count;
        container.start_time = get_timestamp_ms();

        Ok(())
    }

    pub fn remove_container(&mut self, id: u32, force: bool) -> Result<(), &'static str> {
        let idx = self
            .containers
            .iter()
            .position(|c| c.id == id)
            .ok_or("Container not found")?;

        let state = self.containers[idx].state;
        if state == DockerContainerState::Running || state == DockerContainerState::Paused {
            if !force {
                return Err("Cannot remove running container; use force");
            }
        }

        self.containers.remove(idx);
        Ok(())
    }

    pub fn get_container_state(&self, id: u32) -> Result<DockerContainerState, &'static str> {
        let container = self
            .containers
            .iter()
            .find(|c| c.id == id)
            .ok_or("Container not found")?;
        Ok(container.state)
    }

    pub fn get_container_stats(&self, id: u32) -> Result<(u64, u32, u64, u64), &'static str> {
        let c = self
            .containers
            .iter()
            .find(|c| c.id == id)
            .ok_or("Container not found")?;
        Ok((
            c.cpu_usage_ns,
            c.memory_used_mb,
            c.network_rx_bytes,
            c.network_tx_bytes,
        ))
    }

    pub fn list_containers(&self, all: bool) -> Vec<(u32, DockerContainerState, u32)> {
        self.containers
            .iter()
            .filter(|c| all || c.state == DockerContainerState::Running)
            .map(|c| (c.id, c.state, c.config.image_id))
            .collect()
    }

    pub fn running_count(&self) -> usize {
        self.containers
            .iter()
            .filter(|c| c.state == DockerContainerState::Running)
            .count()
    }

    pub fn container_count(&self) -> usize {
        self.containers.len()
    }

    pub fn total_started(&self) -> u32 {
        self.total_started
    }

    pub fn storage_used(&self) -> u64 {
        self.storage_used_bytes
    }
}

// ---------------------------------------------------------------------------
// Global singleton
// ---------------------------------------------------------------------------

static DOCKER_RT: Mutex<Option<DockerRuntime>> = Mutex::new(None);

pub fn init() {
    let mut rt = DOCKER_RT.lock();
    *rt = Some(DockerRuntime::new());
    serial_println!(
        "[DOCKER] Docker-compatible runtime initialized (max: {} containers, {} images)",
        MAX_CONTAINERS,
        MAX_IMAGES
    );
}

// -- Public API wrappers ----------------------------------------------------

pub fn register_image(
    repo_hash: u64,
    tag_hash: u64,
    manifest_digest: u64,
    config_digest: u64,
    entrypoint_hash: u64,
    cmd_hash: u64,
) -> Result<u32, &'static str> {
    let mut rt = DOCKER_RT.lock();
    rt.as_mut()
        .ok_or("Docker runtime not initialized")?
        .register_image(
            repo_hash,
            tag_hash,
            manifest_digest,
            config_digest,
            entrypoint_hash,
            cmd_hash,
        )
}

pub fn add_image_layer(
    image_id: u32,
    digest: u64,
    parent_digest: u64,
    layer_type: LayerType,
    size_bytes: u64,
    compressed_size: u64,
) -> Result<(), &'static str> {
    let mut rt = DOCKER_RT.lock();
    rt.as_mut()
        .ok_or("Docker runtime not initialized")?
        .add_layer_to_image(
            image_id,
            digest,
            parent_digest,
            layer_type,
            size_bytes,
            compressed_size,
        )
}

pub fn remove_image(image_id: u32) -> Result<(), &'static str> {
    let mut rt = DOCKER_RT.lock();
    rt.as_mut()
        .ok_or("Docker runtime not initialized")?
        .remove_image(image_id)
}

pub fn list_images() -> Vec<(u32, u64, u64, u64)> {
    let rt = DOCKER_RT.lock();
    match rt.as_ref() {
        Some(runtime) => runtime.list_images(),
        None => Vec::new(),
    }
}

pub fn create_container(config: DockerContainerConfig) -> Result<u32, &'static str> {
    let mut rt = DOCKER_RT.lock();
    rt.as_mut()
        .ok_or("Docker runtime not initialized")?
        .create_container(config)
}

pub fn start_container(id: u32) -> Result<(), &'static str> {
    let mut rt = DOCKER_RT.lock();
    rt.as_mut()
        .ok_or("Docker runtime not initialized")?
        .start_container(id)
}

pub fn stop_container(id: u32, timeout_secs: u32) -> Result<(), &'static str> {
    let mut rt = DOCKER_RT.lock();
    rt.as_mut()
        .ok_or("Docker runtime not initialized")?
        .stop_container(id, timeout_secs)
}

pub fn kill_container(id: u32, signal: u32) -> Result<(), &'static str> {
    let mut rt = DOCKER_RT.lock();
    rt.as_mut()
        .ok_or("Docker runtime not initialized")?
        .kill_container(id, signal)
}

pub fn pause_container(id: u32) -> Result<(), &'static str> {
    let mut rt = DOCKER_RT.lock();
    rt.as_mut()
        .ok_or("Docker runtime not initialized")?
        .pause_container(id)
}

pub fn unpause_container(id: u32) -> Result<(), &'static str> {
    let mut rt = DOCKER_RT.lock();
    rt.as_mut()
        .ok_or("Docker runtime not initialized")?
        .unpause_container(id)
}

pub fn restart_container(id: u32) -> Result<(), &'static str> {
    let mut rt = DOCKER_RT.lock();
    rt.as_mut()
        .ok_or("Docker runtime not initialized")?
        .restart_container(id)
}

pub fn remove_container(id: u32, force: bool) -> Result<(), &'static str> {
    let mut rt = DOCKER_RT.lock();
    rt.as_mut()
        .ok_or("Docker runtime not initialized")?
        .remove_container(id, force)
}

pub fn list_containers(all: bool) -> Vec<(u32, DockerContainerState, u32)> {
    let rt = DOCKER_RT.lock();
    match rt.as_ref() {
        Some(runtime) => runtime.list_containers(all),
        None => Vec::new(),
    }
}

pub fn get_container_stats(id: u32) -> Result<(u64, u32, u64, u64), &'static str> {
    let rt = DOCKER_RT.lock();
    rt.as_ref()
        .ok_or("Docker runtime not initialized")?
        .get_container_stats(id)
}

pub fn running_count() -> usize {
    let rt = DOCKER_RT.lock();
    match rt.as_ref() {
        Some(runtime) => runtime.running_count(),
        None => 0,
    }
}

pub fn storage_used() -> u64 {
    let rt = DOCKER_RT.lock();
    match rt.as_ref() {
        Some(runtime) => runtime.storage_used(),
        None => 0,
    }
}
