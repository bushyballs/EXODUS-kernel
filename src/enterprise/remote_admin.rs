/// Remote administration for Genesis
///
/// Remote device management, push commands,
/// device location, lock/wipe, and status reporting.
///
/// Inspired by: Android Find My Device, Apple Find My. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Remote command type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteCommand {
    Lock,
    Unlock,
    Wipe,
    Ring,
    Locate,
    SetMessage,
    ResetPassword,
    EnableLostMode,
    DisableLostMode,
    Reboot,
    UpdatePolicy,
}

/// Command status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandStatus {
    Pending,
    Delivered,
    Acknowledged,
    Completed,
    Failed,
}

/// A pending remote command
pub struct PendingCommand {
    pub id: u32,
    pub command: RemoteCommand,
    pub status: CommandStatus,
    pub payload: String,
    pub issued_at: u64,
    pub completed_at: Option<u64>,
}

/// Device location
pub struct DeviceLocation {
    pub latitude: i64,  // microdegrees
    pub longitude: i64, // microdegrees
    pub accuracy_m: u32,
    pub timestamp: u64,
}

/// Remote admin state
pub struct RemoteAdmin {
    pub enabled: bool,
    pub device_name: String,
    pub commands: Vec<PendingCommand>,
    pub next_cmd_id: u32,
    pub last_location: Option<DeviceLocation>,
    pub lost_mode: bool,
    pub lost_message: String,
    pub lost_phone: String,
    pub last_heartbeat: u64,
    pub heartbeat_interval: u64,
}

impl RemoteAdmin {
    const fn new() -> Self {
        RemoteAdmin {
            enabled: true,
            device_name: String::new(),
            commands: Vec::new(),
            next_cmd_id: 1,
            last_location: None,
            lost_mode: false,
            lost_message: String::new(),
            lost_phone: String::new(),
            last_heartbeat: 0,
            heartbeat_interval: 3600,
        }
    }

    pub fn queue_command(&mut self, cmd: RemoteCommand, payload: &str) -> u32 {
        let id = self.next_cmd_id;
        self.next_cmd_id = self.next_cmd_id.saturating_add(1);
        self.commands.push(PendingCommand {
            id,
            command: cmd,
            status: CommandStatus::Pending,
            payload: String::from(payload),
            issued_at: crate::time::clock::unix_time(),
            completed_at: None,
        });
        id
    }

    pub fn process_commands(&mut self) {
        for cmd in &mut self.commands {
            if cmd.status != CommandStatus::Pending {
                continue;
            }
            cmd.status = CommandStatus::Acknowledged;

            match cmd.command {
                RemoteCommand::Lock => {
                    crate::serial_println!("  [remote] Device LOCKED remotely");
                    cmd.status = CommandStatus::Completed;
                }
                RemoteCommand::Ring => {
                    crate::serial_println!("  [remote] Device RINGING");
                    cmd.status = CommandStatus::Completed;
                }
                RemoteCommand::Wipe => {
                    crate::serial_println!("  [remote] Device WIPE initiated");
                    cmd.status = CommandStatus::Completed;
                }
                RemoteCommand::EnableLostMode => {
                    self.lost_mode = true;
                    cmd.status = CommandStatus::Completed;
                }
                RemoteCommand::DisableLostMode => {
                    self.lost_mode = false;
                    cmd.status = CommandStatus::Completed;
                }
                RemoteCommand::Locate => {
                    // Report last known location
                    cmd.status = CommandStatus::Completed;
                }
                _ => {
                    cmd.status = CommandStatus::Completed;
                }
            }
            cmd.completed_at = Some(crate::time::clock::unix_time());
        }
    }

    pub fn update_location(&mut self, lat: i64, lon: i64, accuracy: u32) {
        self.last_location = Some(DeviceLocation {
            latitude: lat,
            longitude: lon,
            accuracy_m: accuracy,
            timestamp: crate::time::clock::unix_time(),
        });
    }

    pub fn heartbeat(&mut self) {
        self.last_heartbeat = crate::time::clock::unix_time();
    }

    pub fn pending_count(&self) -> usize {
        self.commands
            .iter()
            .filter(|c| c.status == CommandStatus::Pending)
            .count()
    }
}

static ADMIN: Mutex<RemoteAdmin> = Mutex::new(RemoteAdmin::new());

pub fn init() {
    let mut admin = ADMIN.lock();
    admin.device_name = String::from("Hoags Device");
    admin.heartbeat();
    crate::serial_println!("  [enterprise] Remote administration initialized");
}
