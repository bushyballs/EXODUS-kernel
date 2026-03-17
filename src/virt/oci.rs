use crate::sync::Mutex;
use crate::{serial_print, serial_println};
/// OCI (Open Container Initiative) runtime specification for Genesis
///
/// Implements OCI image spec, runtime spec, bundle format, config parsing,
/// and lifecycle hooks for standards-compliant container execution.
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_BUNDLES: usize = 64;
const MAX_HOOKS_PER_STAGE: usize = 8;
const MAX_MOUNTS_PER_SPEC: usize = 16;
const MAX_NAMESPACES: usize = 8;
const MAX_RLIMITS: usize = 16;
const OCI_VERSION_MAJOR: u8 = 1;
const OCI_VERSION_MINOR: u8 = 0;
const OCI_VERSION_PATCH: u8 = 2;

// ---------------------------------------------------------------------------
// OCI Image Spec
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MediaType {
    ImageManifest,
    ImageIndex,
    ImageLayer,
    ImageLayerGzip,
    ImageLayerZstd,
    ImageConfig,
    Unknown,
}

#[derive(Clone, Copy)]
pub struct OciDescriptor {
    pub media_type: MediaType,
    pub digest: u64,
    pub size: u64,
    pub platform_arch: u32,
    pub platform_os: u32,
}

#[derive(Clone)]
pub struct OciManifest {
    pub schema_version: u8,
    pub media_type: MediaType,
    pub config: OciDescriptor,
    pub layers: Vec<OciDescriptor>,
    pub annotations_hash: u64,
}

#[derive(Clone)]
pub struct OciImageIndex {
    pub schema_version: u8,
    pub manifests: Vec<OciDescriptor>,
    pub annotations_hash: u64,
}

#[derive(Clone, Copy)]
pub struct OciImageConfig {
    pub architecture: u32,
    pub os: u32,
    pub created_timestamp: u64,
    pub author_hash: u64,
    pub entrypoint_hash: u64,
    pub cmd_hash: u64,
    pub working_dir_hash: u64,
    pub user_hash: u64,
    pub env_count: u16,
    pub exposed_ports_mask: u32,
    pub volumes_count: u8,
    pub labels_hash: u64,
    pub stop_signal: u8,
}

// ---------------------------------------------------------------------------
// OCI Runtime Spec
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ContainerStatus {
    Creating,
    Created,
    Running,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NamespaceType {
    Pid,
    Network,
    Mount,
    Ipc,
    Uts,
    User,
    Cgroup,
    Time,
}

#[derive(Clone, Copy)]
pub struct NamespaceConfig {
    pub ns_type: NamespaceType,
    pub path_hash: u64,
}

#[derive(Clone, Copy)]
pub struct OciMount {
    pub destination_hash: u64,
    pub source_hash: u64,
    pub fs_type_hash: u64,
    pub options_flags: u32,
    pub read_only: bool,
}

#[derive(Clone, Copy)]
pub struct Rlimit {
    pub limit_type: u16,
    pub hard: u64,
    pub soft: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HookStage {
    Prestart,
    CreateRuntime,
    CreateContainer,
    StartContainer,
    Poststart,
    Poststop,
}

#[derive(Clone, Copy)]
pub struct LifecycleHook {
    pub stage: HookStage,
    pub path_hash: u64,
    pub args_hash: u64,
    pub timeout_secs: u32,
    pub env_hash: u64,
}

#[derive(Clone, Copy)]
pub struct ProcessSpec {
    pub terminal: bool,
    pub user_uid: u32,
    pub user_gid: u32,
    pub additional_gids_mask: u64,
    pub cwd_hash: u64,
    pub entrypoint_hash: u64,
    pub args_hash: u64,
    pub env_count: u16,
    pub capabilities_mask: u64,
    pub no_new_privileges: bool,
    pub oom_score_adj: i32,
    pub selinux_label_hash: u64,
    pub apparmor_profile_hash: u64,
}

#[derive(Clone, Copy)]
pub struct LinuxResources {
    pub memory_limit: u64,
    pub memory_reservation: u64,
    pub memory_swap: u64,
    pub cpu_shares: u32,
    pub cpu_quota: i64,
    pub cpu_period: u64,
    pub cpuset_cpus_mask: u64,
    pub pids_limit: i64,
    pub block_io_weight: u16,
}

#[derive(Clone)]
pub struct RuntimeSpec {
    pub version_major: u8,
    pub version_minor: u8,
    pub version_patch: u8,
    pub hostname_hash: u64,
    pub process: ProcessSpec,
    pub root_path_hash: u64,
    pub root_readonly: bool,
    pub mounts: Vec<OciMount>,
    pub namespaces: Vec<NamespaceConfig>,
    pub rlimits: Vec<Rlimit>,
    pub resources: LinuxResources,
    pub masked_paths_hash: u64,
    pub readonly_paths_hash: u64,
}

// ---------------------------------------------------------------------------
// OCI Bundle
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BundleState {
    Unpacked,
    Configured,
    Ready,
    Active,
    Invalid,
}

#[derive(Clone)]
pub struct OciBundle {
    pub id: u32,
    pub container_id_hash: u64,
    pub bundle_path_hash: u64,
    pub state: BundleState,
    pub status: ContainerStatus,
    pub spec: RuntimeSpec,
    pub hooks: Vec<LifecycleHook>,
    pub image_manifest: OciManifest,
    pub image_config: OciImageConfig,
    pub pid: u32,
    pub created_timestamp: u64,
    pub started_timestamp: u64,
}

// ---------------------------------------------------------------------------
// OCI Runtime Manager
// ---------------------------------------------------------------------------

pub struct OciRuntime {
    bundles: Vec<OciBundle>,
    next_id: u32,
    total_created: u32,
    total_hooks_executed: u32,
}

impl OciRuntime {
    fn new() -> Self {
        Self {
            bundles: Vec::new(),
            next_id: 1,
            total_created: 0,
            total_hooks_executed: 0,
        }
    }

    fn default_resources() -> LinuxResources {
        LinuxResources {
            memory_limit: 0,
            memory_reservation: 0,
            memory_swap: 0,
            cpu_shares: 1024,
            cpu_quota: -1,
            cpu_period: 100_000,
            cpuset_cpus_mask: 0xFFFFFFFFFFFFFFFF,
            pids_limit: -1,
            block_io_weight: 500,
        }
    }

    fn default_process() -> ProcessSpec {
        ProcessSpec {
            terminal: false,
            user_uid: 0,
            user_gid: 0,
            additional_gids_mask: 0,
            cwd_hash: 0x2F, // "/" hash
            entrypoint_hash: 0,
            args_hash: 0,
            env_count: 0,
            capabilities_mask: 0,
            no_new_privileges: true,
            oom_score_adj: 0,
            selinux_label_hash: 0,
            apparmor_profile_hash: 0,
        }
    }

    pub fn create_bundle(
        &mut self,
        container_id_hash: u64,
        bundle_path_hash: u64,
        root_path_hash: u64,
        root_readonly: bool,
        hostname_hash: u64,
    ) -> Result<u32, &'static str> {
        if self.bundles.len() >= MAX_BUNDLES {
            return Err("Bundle limit reached");
        }

        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.total_created = self.total_created.saturating_add(1);

        let spec = RuntimeSpec {
            version_major: OCI_VERSION_MAJOR,
            version_minor: OCI_VERSION_MINOR,
            version_patch: OCI_VERSION_PATCH,
            hostname_hash,
            process: Self::default_process(),
            root_path_hash,
            root_readonly,
            mounts: Vec::new(),
            namespaces: Vec::new(),
            rlimits: Vec::new(),
            resources: Self::default_resources(),
            masked_paths_hash: 0,
            readonly_paths_hash: 0,
        };

        let manifest = OciManifest {
            schema_version: 2,
            media_type: MediaType::ImageManifest,
            config: OciDescriptor {
                media_type: MediaType::ImageConfig,
                digest: 0,
                size: 0,
                platform_arch: 0x8664,
                platform_os: 0x4C696E75, // "Linu"
            },
            layers: Vec::new(),
            annotations_hash: 0,
        };

        let image_config = OciImageConfig {
            architecture: 0x8664,
            os: 0x4C696E75,
            created_timestamp: 0,
            author_hash: 0,
            entrypoint_hash: 0,
            cmd_hash: 0,
            working_dir_hash: 0x2F,
            user_hash: 0,
            env_count: 0,
            exposed_ports_mask: 0,
            volumes_count: 0,
            labels_hash: 0,
            stop_signal: 15, // SIGTERM
        };

        let bundle = OciBundle {
            id,
            container_id_hash,
            bundle_path_hash,
            state: BundleState::Unpacked,
            status: ContainerStatus::Creating,
            spec,
            hooks: Vec::new(),
            image_manifest: manifest,
            image_config,
            pid: 0,
            created_timestamp: 0,
            started_timestamp: 0,
        };

        self.bundles.push(bundle);
        Ok(id)
    }

    pub fn add_namespace(
        &mut self,
        bundle_id: u32,
        ns_type: NamespaceType,
        path_hash: u64,
    ) -> Result<(), &'static str> {
        let bundle = self
            .bundles
            .iter_mut()
            .find(|b| b.id == bundle_id)
            .ok_or("Bundle not found")?;

        if bundle.spec.namespaces.len() >= MAX_NAMESPACES {
            return Err("Namespace limit reached");
        }

        bundle
            .spec
            .namespaces
            .push(NamespaceConfig { ns_type, path_hash });
        Ok(())
    }

    pub fn add_mount(
        &mut self,
        bundle_id: u32,
        destination_hash: u64,
        source_hash: u64,
        fs_type_hash: u64,
        options_flags: u32,
        read_only: bool,
    ) -> Result<(), &'static str> {
        let bundle = self
            .bundles
            .iter_mut()
            .find(|b| b.id == bundle_id)
            .ok_or("Bundle not found")?;

        if bundle.spec.mounts.len() >= MAX_MOUNTS_PER_SPEC {
            return Err("Mount limit reached");
        }

        bundle.spec.mounts.push(OciMount {
            destination_hash,
            source_hash,
            fs_type_hash,
            options_flags,
            read_only,
        });
        Ok(())
    }

    pub fn add_rlimit(
        &mut self,
        bundle_id: u32,
        limit_type: u16,
        hard: u64,
        soft: u64,
    ) -> Result<(), &'static str> {
        let bundle = self
            .bundles
            .iter_mut()
            .find(|b| b.id == bundle_id)
            .ok_or("Bundle not found")?;

        if bundle.spec.rlimits.len() >= MAX_RLIMITS {
            return Err("Rlimit limit reached");
        }

        bundle.spec.rlimits.push(Rlimit {
            limit_type,
            hard,
            soft,
        });
        Ok(())
    }

    pub fn set_resources(
        &mut self,
        bundle_id: u32,
        resources: LinuxResources,
    ) -> Result<(), &'static str> {
        let bundle = self
            .bundles
            .iter_mut()
            .find(|b| b.id == bundle_id)
            .ok_or("Bundle not found")?;

        bundle.spec.resources = resources;
        Ok(())
    }

    pub fn set_process(
        &mut self,
        bundle_id: u32,
        process: ProcessSpec,
    ) -> Result<(), &'static str> {
        let bundle = self
            .bundles
            .iter_mut()
            .find(|b| b.id == bundle_id)
            .ok_or("Bundle not found")?;

        bundle.spec.process = process;
        Ok(())
    }

    pub fn add_hook(
        &mut self,
        bundle_id: u32,
        stage: HookStage,
        path_hash: u64,
        args_hash: u64,
        timeout_secs: u32,
        env_hash: u64,
    ) -> Result<(), &'static str> {
        let bundle = self
            .bundles
            .iter_mut()
            .find(|b| b.id == bundle_id)
            .ok_or("Bundle not found")?;

        let stage_count = bundle.hooks.iter().filter(|h| h.stage == stage).count();
        if stage_count >= MAX_HOOKS_PER_STAGE {
            return Err("Hook limit for stage reached");
        }

        bundle.hooks.push(LifecycleHook {
            stage,
            path_hash,
            args_hash,
            timeout_secs,
            env_hash,
        });
        Ok(())
    }

    pub fn finalize_config(&mut self, bundle_id: u32) -> Result<(), &'static str> {
        let bundle = self
            .bundles
            .iter_mut()
            .find(|b| b.id == bundle_id)
            .ok_or("Bundle not found")?;

        if bundle.state != BundleState::Unpacked {
            return Err("Bundle not in configurable state");
        }

        bundle.state = BundleState::Configured;
        bundle.status = ContainerStatus::Created;
        Ok(())
    }

    pub fn execute_hooks(&mut self, bundle_id: u32, stage: HookStage) -> Result<u32, &'static str> {
        let bundle = self
            .bundles
            .iter()
            .find(|b| b.id == bundle_id)
            .ok_or("Bundle not found")?;

        let count = bundle.hooks.iter().filter(|h| h.stage == stage).count() as u32;

        // Stub: would actually execute each hook binary
        self.total_hooks_executed += count;
        Ok(count)
    }

    pub fn start_bundle(&mut self, bundle_id: u32) -> Result<(), &'static str> {
        // Execute prestart hooks
        let _ = self.execute_hooks(bundle_id, HookStage::Prestart);
        let _ = self.execute_hooks(bundle_id, HookStage::CreateRuntime);
        let _ = self.execute_hooks(bundle_id, HookStage::CreateContainer);

        let bundle = self
            .bundles
            .iter_mut()
            .find(|b| b.id == bundle_id)
            .ok_or("Bundle not found")?;

        if bundle.status != ContainerStatus::Created {
            return Err("Bundle not in created state");
        }

        bundle.status = ContainerStatus::Running;
        bundle.state = BundleState::Active;
        bundle.pid = bundle_id * 1000;
        bundle.started_timestamp = 0;

        // Execute start/poststart hooks (non-blocking conceptually)
        // Note: we already borrowed mutably above, so hooks tracked separately
        self.total_hooks_executed += self
            .bundles
            .iter()
            .find(|b| b.id == bundle_id)
            .map(|b| {
                b.hooks
                    .iter()
                    .filter(|h| {
                        h.stage == HookStage::StartContainer || h.stage == HookStage::Poststart
                    })
                    .count() as u32
            })
            .unwrap_or(0);

        Ok(())
    }

    pub fn stop_bundle(&mut self, bundle_id: u32) -> Result<(), &'static str> {
        let bundle = self
            .bundles
            .iter_mut()
            .find(|b| b.id == bundle_id)
            .ok_or("Bundle not found")?;

        if bundle.status != ContainerStatus::Running {
            return Err("Bundle not running");
        }

        bundle.status = ContainerStatus::Stopped;
        bundle.state = BundleState::Configured;
        bundle.pid = 0;

        // Execute poststop hooks
        let poststop_count = bundle
            .hooks
            .iter()
            .filter(|h| h.stage == HookStage::Poststop)
            .count() as u32;
        self.total_hooks_executed += poststop_count;

        Ok(())
    }

    pub fn delete_bundle(&mut self, bundle_id: u32) -> Result<(), &'static str> {
        let idx = self
            .bundles
            .iter()
            .position(|b| b.id == bundle_id)
            .ok_or("Bundle not found")?;

        if self.bundles[idx].status == ContainerStatus::Running {
            return Err("Cannot delete running bundle");
        }

        self.bundles.remove(idx);
        Ok(())
    }

    pub fn get_bundle_status(
        &self,
        bundle_id: u32,
    ) -> Result<(ContainerStatus, BundleState, u32), &'static str> {
        let bundle = self
            .bundles
            .iter()
            .find(|b| b.id == bundle_id)
            .ok_or("Bundle not found")?;
        Ok((bundle.status, bundle.state, bundle.pid))
    }

    pub fn list_bundles(&self) -> Vec<(u32, ContainerStatus, BundleState)> {
        self.bundles
            .iter()
            .map(|b| (b.id, b.status, b.state))
            .collect()
    }

    pub fn bundle_count(&self) -> usize {
        self.bundles.len()
    }

    pub fn total_hooks_executed(&self) -> u32 {
        self.total_hooks_executed
    }
}

// ---------------------------------------------------------------------------
// Global singleton
// ---------------------------------------------------------------------------

static OCI_RT: Mutex<Option<OciRuntime>> = Mutex::new(None);

pub fn init() {
    let mut rt = OCI_RT.lock();
    *rt = Some(OciRuntime::new());
    serial_println!(
        "[OCI] OCI runtime v{}.{}.{} initialized (max: {} bundles)",
        OCI_VERSION_MAJOR,
        OCI_VERSION_MINOR,
        OCI_VERSION_PATCH,
        MAX_BUNDLES
    );
}

// -- Public API wrappers ----------------------------------------------------

pub fn create_bundle(
    container_id_hash: u64,
    bundle_path_hash: u64,
    root_path_hash: u64,
    root_readonly: bool,
    hostname_hash: u64,
) -> Result<u32, &'static str> {
    let mut rt = OCI_RT.lock();
    rt.as_mut()
        .ok_or("OCI runtime not initialized")?
        .create_bundle(
            container_id_hash,
            bundle_path_hash,
            root_path_hash,
            root_readonly,
            hostname_hash,
        )
}

pub fn add_namespace(
    bundle_id: u32,
    ns_type: NamespaceType,
    path_hash: u64,
) -> Result<(), &'static str> {
    let mut rt = OCI_RT.lock();
    rt.as_mut()
        .ok_or("OCI runtime not initialized")?
        .add_namespace(bundle_id, ns_type, path_hash)
}

pub fn add_mount(
    bundle_id: u32,
    destination_hash: u64,
    source_hash: u64,
    fs_type_hash: u64,
    options_flags: u32,
    read_only: bool,
) -> Result<(), &'static str> {
    let mut rt = OCI_RT.lock();
    rt.as_mut().ok_or("OCI runtime not initialized")?.add_mount(
        bundle_id,
        destination_hash,
        source_hash,
        fs_type_hash,
        options_flags,
        read_only,
    )
}

pub fn add_hook(
    bundle_id: u32,
    stage: HookStage,
    path_hash: u64,
    args_hash: u64,
    timeout_secs: u32,
    env_hash: u64,
) -> Result<(), &'static str> {
    let mut rt = OCI_RT.lock();
    rt.as_mut().ok_or("OCI runtime not initialized")?.add_hook(
        bundle_id,
        stage,
        path_hash,
        args_hash,
        timeout_secs,
        env_hash,
    )
}

pub fn finalize_config(bundle_id: u32) -> Result<(), &'static str> {
    let mut rt = OCI_RT.lock();
    rt.as_mut()
        .ok_or("OCI runtime not initialized")?
        .finalize_config(bundle_id)
}

pub fn start_bundle(bundle_id: u32) -> Result<(), &'static str> {
    let mut rt = OCI_RT.lock();
    rt.as_mut()
        .ok_or("OCI runtime not initialized")?
        .start_bundle(bundle_id)
}

pub fn stop_bundle(bundle_id: u32) -> Result<(), &'static str> {
    let mut rt = OCI_RT.lock();
    rt.as_mut()
        .ok_or("OCI runtime not initialized")?
        .stop_bundle(bundle_id)
}

pub fn delete_bundle(bundle_id: u32) -> Result<(), &'static str> {
    let mut rt = OCI_RT.lock();
    rt.as_mut()
        .ok_or("OCI runtime not initialized")?
        .delete_bundle(bundle_id)
}

pub fn get_bundle_status(
    bundle_id: u32,
) -> Result<(ContainerStatus, BundleState, u32), &'static str> {
    let rt = OCI_RT.lock();
    rt.as_ref()
        .ok_or("OCI runtime not initialized")?
        .get_bundle_status(bundle_id)
}

pub fn list_bundles() -> Vec<(u32, ContainerStatus, BundleState)> {
    let rt = OCI_RT.lock();
    match rt.as_ref() {
        Some(runtime) => runtime.list_bundles(),
        None => Vec::new(),
    }
}

pub fn bundle_count() -> usize {
    let rt = OCI_RT.lock();
    match rt.as_ref() {
        Some(runtime) => runtime.bundle_count(),
        None => 0,
    }
}
