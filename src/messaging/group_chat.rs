use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec;
/// Group chat subsystem for Genesis OS
///
/// Provides:
///   - Group creation, membership management, admin promotion
///   - Per-group settings (muted, pinned, archived, disappearing timer)
///   - Group message send / receive
///   - Leave and mute operations
///   - Message pruning and history management
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of members allowed in a single group.
const MAX_GROUP_MEMBERS: usize = 256;

/// Maximum messages retained per group before oldest are pruned.
const MAX_GROUP_MESSAGES: usize = 8192;

/// Default disappearing message timer (0 = disabled). Value in seconds.
const DEFAULT_DISAPPEARING_TIMER: u64 = 0;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Per-group user settings (each member may have their own view).
#[derive(Clone, Copy)]
pub struct GroupSettings {
    pub muted: bool,
    pub pinned: bool,
    pub archived: bool,
    pub disappearing_timer: u64,
}

impl GroupSettings {
    pub fn default_settings() -> Self {
        Self {
            muted: false,
            pinned: false,
            archived: false,
            disappearing_timer: DEFAULT_DISAPPEARING_TIMER,
        }
    }
}

/// A message within a group conversation.
#[derive(Clone)]
pub struct GroupMessage {
    pub id: u64,
    pub sender_hash: u64,
    pub content_hash: u64,
    pub payload: Vec<u8>,
    pub timestamp: u64,
    pub read_by: Vec<u64>,
}

/// A group conversation.
pub struct Group {
    pub id: u64,
    pub name_hash: u64,
    pub members: Vec<u64>,
    pub admins: Vec<u64>,
    pub created: u64,
    pub messages: Vec<GroupMessage>,
    pub settings: GroupSettings,
    /// Per-member muted status (user_hash stored if muted).
    pub muted_members: Vec<u64>,
}

/// Manager holding all groups.
pub struct GroupManager {
    groups: Vec<Group>,
    next_group_id: u64,
    next_msg_id: u64,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static GROUP_MANAGER: Mutex<Option<GroupManager>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// GroupManager implementation
// ---------------------------------------------------------------------------

impl GroupManager {
    pub fn new() -> Self {
        Self {
            groups: vec![],
            next_group_id: 1,
            next_msg_id: 1,
        }
    }

    // ----- lookup helpers -----

    fn find_group(&self, group_id: u64) -> Option<&Group> {
        self.groups.iter().find(|g| g.id == group_id)
    }

    fn find_group_mut(&mut self, group_id: u64) -> Option<&mut Group> {
        self.groups.iter_mut().find(|g| g.id == group_id)
    }

    // ----- group lifecycle -----

    /// Create a new group.  The creator is automatically an admin and member.
    pub fn create_group(&mut self, name_hash: u64, creator_hash: u64, timestamp: u64) -> u64 {
        let id = self.next_group_id;
        self.next_group_id = self.next_group_id.saturating_add(1);

        let group = Group {
            id,
            name_hash,
            members: vec![creator_hash],
            admins: vec![creator_hash],
            created: timestamp,
            messages: vec![],
            settings: GroupSettings::default_settings(),
            muted_members: vec![],
        };
        self.groups.push(group);
        serial_println!("[group_chat] group {} created by {:#X}", id, creator_hash);
        id
    }

    /// Add a member to a group.  Returns `true` on success.
    pub fn add_member(&mut self, group_id: u64, user_hash: u64) -> bool {
        if let Some(group) = self.find_group_mut(group_id) {
            if group.members.contains(&user_hash) {
                return false; // already a member
            }
            if group.members.len() >= MAX_GROUP_MEMBERS {
                serial_println!("[group_chat] group {} full, cannot add member", group_id);
                return false;
            }
            group.members.push(user_hash);
            serial_println!("[group_chat] added {:#X} to group {}", user_hash, group_id);
            true
        } else {
            false
        }
    }

    /// Remove a member from a group.  Admins can also be removed; if the
    /// last admin is removed the group becomes admin-less (effectively frozen).
    pub fn remove_member(&mut self, group_id: u64, user_hash: u64) -> bool {
        if let Some(group) = self.find_group_mut(group_id) {
            let before = group.members.len();
            group.members.retain(|&m| m != user_hash);
            if group.members.len() < before {
                // Also strip admin status.
                group.admins.retain(|&a| a != user_hash);
                group.muted_members.retain(|&m| m != user_hash);
                serial_println!(
                    "[group_chat] removed {:#X} from group {}",
                    user_hash,
                    group_id
                );
                return true;
            }
        }
        false
    }

    /// Promote an existing member to admin.
    pub fn promote_admin(&mut self, group_id: u64, user_hash: u64) -> bool {
        if let Some(group) = self.find_group_mut(group_id) {
            if !group.members.contains(&user_hash) {
                return false; // not a member
            }
            if group.admins.contains(&user_hash) {
                return false; // already admin
            }
            group.admins.push(user_hash);
            serial_println!(
                "[group_chat] promoted {:#X} to admin in group {}",
                user_hash,
                group_id
            );
            true
        } else {
            false
        }
    }

    /// Demote an admin back to regular member.
    pub fn demote_admin(&mut self, group_id: u64, user_hash: u64) -> bool {
        if let Some(group) = self.find_group_mut(group_id) {
            let before = group.admins.len();
            group.admins.retain(|&a| a != user_hash);
            group.admins.len() < before
        } else {
            false
        }
    }

    // ----- messaging -----

    /// Send a message to a group.  Returns the message id on success.
    pub fn send_group_message(
        &mut self,
        group_id: u64,
        sender_hash: u64,
        payload: &[u8],
        timestamp: u64,
    ) -> Option<u64> {
        // Verify sender is a member.
        {
            let group = self.find_group(group_id)?;
            if !group.members.contains(&sender_hash) {
                serial_println!(
                    "[group_chat] {:#X} not a member of group {}",
                    sender_hash,
                    group_id
                );
                return None;
            }
        }

        let content_hash = fnv_hash(payload);
        let msg_id = self.next_msg_id;
        self.next_msg_id = self.next_msg_id.saturating_add(1);

        let msg = GroupMessage {
            id: msg_id,
            sender_hash,
            content_hash,
            payload: payload.into(),
            timestamp,
            read_by: vec![sender_hash], // sender has implicitly read it
        };

        let group = self.find_group_mut(group_id)?;
        group.messages.push(msg);

        // Expire old messages based on disappearing timer.
        if group.settings.disappearing_timer > 0 {
            let cutoff = timestamp.saturating_sub(group.settings.disappearing_timer);
            group.messages.retain(|m| m.timestamp >= cutoff);
        }

        // Prune if over limit.
        if group.messages.len() > MAX_GROUP_MESSAGES {
            let excess = group.messages.len() - MAX_GROUP_MESSAGES;
            group.messages.drain(0..excess);
        }

        serial_println!(
            "[group_chat] msg {} sent to group {} by {:#X}",
            msg_id,
            group_id,
            sender_hash
        );
        Some(msg_id)
    }

    /// Mark a message as read by a user.
    pub fn mark_message_read(&mut self, group_id: u64, msg_id: u64, user_hash: u64) -> bool {
        if let Some(group) = self.find_group_mut(group_id) {
            for m in group.messages.iter_mut() {
                if m.id == msg_id {
                    if !m.read_by.contains(&user_hash) {
                        m.read_by.push(user_hash);
                    }
                    return true;
                }
            }
        }
        false
    }

    // ----- member actions -----

    /// A member voluntarily leaves a group.
    pub fn leave_group(&mut self, group_id: u64, user_hash: u64) -> bool {
        self.remove_member(group_id, user_hash)
    }

    /// Toggle mute for a member within a group.
    pub fn mute_group(&mut self, group_id: u64, user_hash: u64, mute: bool) -> bool {
        if let Some(group) = self.find_group_mut(group_id) {
            if !group.members.contains(&user_hash) {
                return false;
            }
            if mute {
                if !group.muted_members.contains(&user_hash) {
                    group.muted_members.push(user_hash);
                }
            } else {
                group.muted_members.retain(|&m| m != user_hash);
            }
            serial_println!(
                "[group_chat] {:#X} mute={} in group {}",
                user_hash,
                mute,
                group_id
            );
            true
        } else {
            false
        }
    }

    /// Update group-wide settings.
    pub fn update_settings(&mut self, group_id: u64, settings: GroupSettings) -> bool {
        if let Some(group) = self.find_group_mut(group_id) {
            group.settings = settings;
            true
        } else {
            false
        }
    }

    // ----- queries -----

    /// List all group ids a user belongs to.
    pub fn groups_for_user(&self, user_hash: u64) -> Vec<u64> {
        let mut result = vec![];
        for g in &self.groups {
            if g.members.contains(&user_hash) {
                result.push(g.id);
            }
        }
        result
    }

    /// Get the number of members in a group.
    pub fn member_count(&self, group_id: u64) -> usize {
        if let Some(g) = self.find_group(group_id) {
            g.members.len()
        } else {
            0
        }
    }

    /// Get total messages across all groups.
    pub fn total_message_count(&self) -> usize {
        let mut total: usize = 0;
        for g in &self.groups {
            total += g.messages.len();
        }
        total
    }

    /// Check whether a user is an admin of a group.
    pub fn is_admin(&self, group_id: u64, user_hash: u64) -> bool {
        if let Some(g) = self.find_group(group_id) {
            g.admins.contains(&user_hash)
        } else {
            false
        }
    }

    /// Delete a group entirely.
    pub fn delete_group(&mut self, group_id: u64) -> bool {
        let before = self.groups.len();
        self.groups.retain(|g| g.id != group_id);
        self.groups.len() < before
    }
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

/// Simple FNV-1a hash for content hashing.
fn fnv_hash(data: &[u8]) -> u64 {
    let mut h: u64 = 0xCBF29CE484222325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001B3);
    }
    h
}

// ---------------------------------------------------------------------------
// Public API (through the global mutex)
// ---------------------------------------------------------------------------

pub fn create_group(name_hash: u64, creator_hash: u64, timestamp: u64) -> Option<u64> {
    let mut guard = GROUP_MANAGER.lock();
    let mgr = guard.as_mut()?;
    Some(mgr.create_group(name_hash, creator_hash, timestamp))
}

pub fn add_member(group_id: u64, user_hash: u64) -> bool {
    let mut guard = GROUP_MANAGER.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.add_member(group_id, user_hash)
    } else {
        false
    }
}

pub fn remove_member(group_id: u64, user_hash: u64) -> bool {
    let mut guard = GROUP_MANAGER.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.remove_member(group_id, user_hash)
    } else {
        false
    }
}

pub fn promote_admin(group_id: u64, user_hash: u64) -> bool {
    let mut guard = GROUP_MANAGER.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.promote_admin(group_id, user_hash)
    } else {
        false
    }
}

pub fn send_group_message(
    group_id: u64,
    sender_hash: u64,
    payload: &[u8],
    timestamp: u64,
) -> Option<u64> {
    let mut guard = GROUP_MANAGER.lock();
    let mgr = guard.as_mut()?;
    mgr.send_group_message(group_id, sender_hash, payload, timestamp)
}

pub fn leave_group(group_id: u64, user_hash: u64) -> bool {
    let mut guard = GROUP_MANAGER.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.leave_group(group_id, user_hash)
    } else {
        false
    }
}

pub fn mute_group(group_id: u64, user_hash: u64, mute: bool) -> bool {
    let mut guard = GROUP_MANAGER.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.mute_group(group_id, user_hash, mute)
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    let mut guard = GROUP_MANAGER.lock();
    *guard = Some(GroupManager::new());
    serial_println!("[group_chat] initialised");
}
