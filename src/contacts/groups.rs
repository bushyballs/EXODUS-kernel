use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec;
use alloc::vec::Vec;

pub struct ContactGroup {
    pub id: u32,
    pub name_hash: u64,
    pub member_ids: Vec<u32>,
    pub color: u32,
    pub is_system: bool,
}

impl ContactGroup {
    pub fn new(id: u32, name_hash: u64, color: u32, is_system: bool) -> Self {
        Self {
            id,
            name_hash,
            member_ids: vec![],
            color,
            is_system,
        }
    }

    pub fn has_member(&self, contact_id: u32) -> bool {
        self.member_ids.contains(&contact_id)
    }
}

pub struct GroupManager {
    groups: Vec<ContactGroup>,
    next_id: u32,
}

impl GroupManager {
    pub fn new() -> Self {
        Self {
            groups: vec![],
            next_id: 1,
        }
    }

    pub fn create_group(&mut self, name_hash: u64, color: u32, is_system: bool) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let group = ContactGroup::new(id, name_hash, color, is_system);
        self.groups.push(group);
        id
    }

    pub fn add_member(&mut self, group_id: u32, contact_id: u32) -> bool {
        if let Some(group) = self.groups.iter_mut().find(|g| g.id == group_id) {
            if !group.has_member(contact_id) {
                group.member_ids.push(contact_id);
                true
            } else {
                false
            }
        } else {
            false
        }
    }

    pub fn remove_member(&mut self, group_id: u32, contact_id: u32) -> bool {
        if let Some(group) = self.groups.iter_mut().find(|g| g.id == group_id) {
            if let Some(pos) = group.member_ids.iter().position(|&id| id == contact_id) {
                group.member_ids.remove(pos);
                true
            } else {
                false
            }
        } else {
            false
        }
    }

    pub fn get_members(&self, group_id: u32) -> Vec<u32> {
        if let Some(group) = self.groups.iter().find(|g| g.id == group_id) {
            group.member_ids.clone()
        } else {
            vec![]
        }
    }

    pub fn get_groups_for_contact(&self, contact_id: u32) -> Vec<u32> {
        self.groups
            .iter()
            .filter(|g| g.has_member(contact_id))
            .map(|g| g.id)
            .collect()
    }

    pub fn delete_group(&mut self, group_id: u32) -> bool {
        // Don't delete system groups
        if let Some(group) = self.groups.iter().find(|g| g.id == group_id) {
            if group.is_system {
                return false;
            }
        }

        if let Some(pos) = self.groups.iter().position(|g| g.id == group_id) {
            self.groups.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn get_group(&self, group_id: u32) -> Option<&ContactGroup> {
        self.groups.iter().find(|g| g.id == group_id)
    }

    pub fn get_group_mut(&mut self, group_id: u32) -> Option<&mut ContactGroup> {
        self.groups.iter_mut().find(|g| g.id == group_id)
    }

    pub fn total_groups(&self) -> usize {
        self.groups.len()
    }

    pub fn get_all_groups(&self) -> Vec<u32> {
        self.groups.iter().map(|g| g.id).collect()
    }
}

static GROUPS: Mutex<Option<GroupManager>> = Mutex::new(None);

pub fn init() {
    let mut groups = GROUPS.lock();
    *groups = Some(GroupManager::new());
    serial_println!("[CONTACTS] Group manager initialized");
}

pub fn get_groups() -> &'static Mutex<Option<GroupManager>> {
    &GROUPS
}
