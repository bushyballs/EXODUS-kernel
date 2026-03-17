use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec;
use alloc::vec::Vec;

/// Read the CPU timestamp counter and convert to milliseconds.
///
/// Uses `RDTSC` directly via inline asm.  The TSC frequency is estimated at
/// 3 GHz (3_000_000 ticks per millisecond), which is a reasonable default for
/// modern x86 hardware.  A calibrated value from the ACPI/HPET timer should
/// replace this constant once the timer subsystem is available.
#[inline]
pub fn get_timestamp_ms() -> u64 {
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
    // 3_000_000 TSC ticks per millisecond (≈ 3 GHz TSC)
    tsc / 3_000_000
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ContainerState {
    Created,
    Running,
    Paused,
    Stopped,
    Crashed,
}

#[derive(Clone, Copy)]
pub struct ContainerConfig {
    pub id: u32,
    pub image_hash: u64,
    pub memory_limit_mb: u32,
    pub cpu_shares: u16,
    pub net_namespace: bool,
    pub pid_namespace: bool,
    pub mount_namespace: bool,
    pub root_fs_hash: u64,
}

#[derive(Clone, Copy)]
pub struct Container {
    pub config: ContainerConfig,
    pub state: ContainerState,
    pub pid: u32,
    pub start_time: u64,
    pub cpu_usage_ms: u64,
    pub memory_used_mb: u32,
}

pub struct ContainerRuntime {
    containers: Vec<Container>,
    next_id: u32,
    max_containers: u16,
    total_created: u32,
}

impl ContainerRuntime {
    fn new() -> Self {
        Self {
            containers: Vec::new(),
            next_id: 1,
            max_containers: 64,
            total_created: 0,
        }
    }

    pub fn create(
        &mut self,
        image_hash: u64,
        root_fs_hash: u64,
        memory_limit_mb: u32,
        cpu_shares: u16,
        net_ns: bool,
        pid_ns: bool,
        mount_ns: bool,
    ) -> Result<u32, &'static str> {
        if self.containers.len() >= self.max_containers as usize {
            return Err("Container limit reached");
        }

        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        let config = ContainerConfig {
            id,
            image_hash,
            memory_limit_mb,
            cpu_shares,
            net_namespace: net_ns,
            pid_namespace: pid_ns,
            mount_namespace: mount_ns,
            root_fs_hash,
        };

        let container = Container {
            config,
            state: ContainerState::Created,
            pid: 0,
            start_time: 0,
            cpu_usage_ms: 0,
            memory_used_mb: 0,
        };

        self.containers.push(container);
        self.total_created = self.total_created.saturating_add(1);

        Ok(id)
    }

    pub fn start(&mut self, id: u32) -> Result<(), &'static str> {
        let container = self
            .containers
            .iter_mut()
            .find(|c| c.config.id == id)
            .ok_or("Container not found")?;

        if container.state != ContainerState::Created && container.state != ContainerState::Stopped
        {
            return Err("Container not in startable state");
        }

        container.state = ContainerState::Running;
        container.pid = id * 1000; // Stub PID assignment
        container.start_time = get_timestamp_ms();

        Ok(())
    }

    pub fn stop(&mut self, id: u32) -> Result<(), &'static str> {
        let container = self
            .containers
            .iter_mut()
            .find(|c| c.config.id == id)
            .ok_or("Container not found")?;

        if container.state != ContainerState::Running && container.state != ContainerState::Paused {
            return Err("Container not running");
        }

        container.state = ContainerState::Stopped;
        container.pid = 0;

        Ok(())
    }

    pub fn pause(&mut self, id: u32) -> Result<(), &'static str> {
        let container = self
            .containers
            .iter_mut()
            .find(|c| c.config.id == id)
            .ok_or("Container not found")?;

        if container.state != ContainerState::Running {
            return Err("Container not running");
        }

        container.state = ContainerState::Paused;

        Ok(())
    }

    pub fn resume(&mut self, id: u32) -> Result<(), &'static str> {
        let container = self
            .containers
            .iter_mut()
            .find(|c| c.config.id == id)
            .ok_or("Container not found")?;

        if container.state != ContainerState::Paused {
            return Err("Container not paused");
        }

        container.state = ContainerState::Running;

        Ok(())
    }

    pub fn exec_in_container(&mut self, id: u32, _command_hash: u64) -> Result<u32, &'static str> {
        let container = self
            .containers
            .iter()
            .find(|c| c.config.id == id)
            .ok_or("Container not found")?;

        if container.state != ContainerState::Running {
            return Err("Container not running");
        }

        // Stub: would execute command in container's namespace
        Ok(id * 1000 + 1) // Return stub process ID
    }

    pub fn get_stats(&self, id: u32) -> Result<(ContainerState, u64, u32), &'static str> {
        let container = self
            .containers
            .iter()
            .find(|c| c.config.id == id)
            .ok_or("Container not found")?;

        Ok((
            container.state,
            container.cpu_usage_ms,
            container.memory_used_mb,
        ))
    }

    pub fn list_running(&self) -> Vec<u32> {
        self.containers
            .iter()
            .filter(|c| c.state == ContainerState::Running)
            .map(|c| c.config.id)
            .collect()
    }

    pub fn total_created(&self) -> u32 {
        self.total_created
    }

    pub fn active_count(&self) -> usize {
        self.containers
            .iter()
            .filter(|c| c.state == ContainerState::Running || c.state == ContainerState::Paused)
            .count()
    }
}

static CONTAINER_RT: Mutex<Option<ContainerRuntime>> = Mutex::new(None);

pub fn init() {
    let mut rt = CONTAINER_RT.lock();
    *rt = Some(ContainerRuntime::new());
    serial_println!("[CONTAINERS] Runtime initialized (max: 64 containers)");
}

pub fn create_container(
    image_hash: u64,
    root_fs_hash: u64,
    memory_limit_mb: u32,
    cpu_shares: u16,
    net_ns: bool,
    pid_ns: bool,
    mount_ns: bool,
) -> Result<u32, &'static str> {
    let mut rt = CONTAINER_RT.lock();
    rt.as_mut()
        .ok_or("Container runtime not initialized")?
        .create(
            image_hash,
            root_fs_hash,
            memory_limit_mb,
            cpu_shares,
            net_ns,
            pid_ns,
            mount_ns,
        )
}

pub fn start_container(id: u32) -> Result<(), &'static str> {
    let mut rt = CONTAINER_RT.lock();
    rt.as_mut()
        .ok_or("Container runtime not initialized")?
        .start(id)
}

pub fn stop_container(id: u32) -> Result<(), &'static str> {
    let mut rt = CONTAINER_RT.lock();
    rt.as_mut()
        .ok_or("Container runtime not initialized")?
        .stop(id)
}

pub fn pause_container(id: u32) -> Result<(), &'static str> {
    let mut rt = CONTAINER_RT.lock();
    rt.as_mut()
        .ok_or("Container runtime not initialized")?
        .pause(id)
}

pub fn resume_container(id: u32) -> Result<(), &'static str> {
    let mut rt = CONTAINER_RT.lock();
    rt.as_mut()
        .ok_or("Container runtime not initialized")?
        .resume(id)
}

pub fn exec_in_container(id: u32, command_hash: u64) -> Result<u32, &'static str> {
    let mut rt = CONTAINER_RT.lock();
    rt.as_mut()
        .ok_or("Container runtime not initialized")?
        .exec_in_container(id, command_hash)
}

pub fn get_container_stats(id: u32) -> Result<(ContainerState, u64, u32), &'static str> {
    let rt = CONTAINER_RT.lock();
    rt.as_ref()
        .ok_or("Container runtime not initialized")?
        .get_stats(id)
}

pub fn list_running_containers() -> Vec<u32> {
    let rt = CONTAINER_RT.lock();
    match rt.as_ref() {
        Some(runtime) => runtime.list_running(),
        None => Vec::new(),
    }
}
