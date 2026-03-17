use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug)]
pub enum PhoneType {
    Mobile,
    Home,
    Work,
    Fax,
    Other,
}

#[derive(Clone, Copy, Debug)]
pub enum EmailType {
    Personal,
    Work,
    Other,
}

#[derive(Clone, Copy)]
pub struct Contact {
    pub id: u32,
    pub name_hash: u64,
    pub phone_hashes: [u64; 4],
    pub phone_count: u8,
    pub email_hashes: [u64; 4],
    pub email_count: u8,
    pub photo_hash: u64,
    pub company_hash: u64,
    pub starred: bool,
    pub last_contacted: u64,
    pub contact_frequency: u32,
}

impl Contact {
    pub fn new(id: u32, name_hash: u64) -> Self {
        Self {
            id,
            name_hash,
            phone_hashes: [0; 4],
            phone_count: 0,
            email_hashes: [0; 4],
            email_count: 0,
            photo_hash: 0,
            company_hash: 0,
            starred: false,
            last_contacted: 0,
            contact_frequency: 0,
        }
    }

    pub fn add_phone(&mut self, phone_hash: u64) -> bool {
        if self.phone_count < 4 {
            self.phone_hashes[self.phone_count as usize] = phone_hash;
            self.phone_count = self.phone_count.saturating_add(1);
            true
        } else {
            false
        }
    }

    pub fn add_email(&mut self, email_hash: u64) -> bool {
        if self.email_count < 4 {
            self.email_hashes[self.email_count as usize] = email_hash;
            self.email_count = self.email_count.saturating_add(1);
            true
        } else {
            false
        }
    }

    pub fn has_phone(&self, phone_hash: u64) -> bool {
        for i in 0..self.phone_count as usize {
            if self.phone_hashes[i] == phone_hash {
                return true;
            }
        }
        false
    }
}

pub struct ContactProvider {
    contacts: Vec<Contact>,
    next_id: u32,
    total_contacts: u32,
}

impl ContactProvider {
    pub fn new() -> Self {
        Self {
            contacts: vec![],
            next_id: 1,
            total_contacts: 0,
        }
    }

    pub fn add_contact(&mut self, name_hash: u64) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let contact = Contact::new(id, name_hash);
        self.contacts.push(contact);
        self.total_contacts = self.total_contacts.saturating_add(1);
        id
    }

    pub fn remove_contact(&mut self, id: u32) -> bool {
        if let Some(pos) = self.contacts.iter().position(|c| c.id == id) {
            self.contacts.remove(pos);
            self.total_contacts = self.total_contacts.saturating_sub(1);
            true
        } else {
            false
        }
    }

    pub fn find_by_name_hash(&self, name_hash: u64) -> Option<&Contact> {
        self.contacts.iter().find(|c| c.name_hash == name_hash)
    }

    pub fn find_by_phone_hash(&self, phone_hash: u64) -> Option<&Contact> {
        self.contacts.iter().find(|c| c.has_phone(phone_hash))
    }

    pub fn get_starred(&self) -> Vec<u32> {
        self.contacts
            .iter()
            .filter(|c| c.starred)
            .map(|c| c.id)
            .collect()
    }

    pub fn get_frequent(&self, n: usize) -> Vec<u32> {
        let mut sorted: Vec<&Contact> = self.contacts.iter().collect();
        sorted.sort_by(|a, b| b.contact_frequency.cmp(&a.contact_frequency));
        sorted.iter().take(n).map(|c| c.id).collect()
    }

    pub fn merge_duplicates(&mut self) -> u32 {
        let mut merged_count = 0;
        let mut to_remove = vec![];

        for i in 0..self.contacts.len() {
            for j in (i + 1)..self.contacts.len() {
                if self.contacts[i].name_hash == self.contacts[j].name_hash {
                    // Merge j into i
                    let src_id = self.contacts[j].id;

                    // Merge phone numbers
                    for k in 0..self.contacts[j].phone_count as usize {
                        let phone = self.contacts[j].phone_hashes[k];
                        if !self.contacts[i].has_phone(phone) {
                            self.contacts[i].add_phone(phone);
                        }
                    }

                    // Merge email addresses
                    for k in 0..self.contacts[j].email_count as usize {
                        let email = self.contacts[j].email_hashes[k];
                        self.contacts[i].add_email(email);
                    }

                    // Update frequency
                    self.contacts[i].contact_frequency += self.contacts[j].contact_frequency;

                    // Update last contacted
                    if self.contacts[j].last_contacted > self.contacts[i].last_contacted {
                        self.contacts[i].last_contacted = self.contacts[j].last_contacted;
                    }

                    // Preserve starred status
                    if self.contacts[j].starred {
                        self.contacts[i].starred = true;
                    }

                    to_remove.push(src_id);
                    merged_count += 1;
                }
            }
        }

        // Remove merged contacts
        for id in to_remove {
            self.remove_contact(id);
        }

        merged_count
    }

    pub fn get_contact_mut(&mut self, id: u32) -> Option<&mut Contact> {
        self.contacts.iter_mut().find(|c| c.id == id)
    }

    pub fn total_contacts(&self) -> u32 {
        self.total_contacts
    }
}

static CONTACTS: Mutex<Option<ContactProvider>> = Mutex::new(None);

pub fn init() {
    let mut contacts = CONTACTS.lock();
    *contacts = Some(ContactProvider::new());
    serial_println!("[CONTACTS] Contact provider initialized");
}

pub fn get_provider() -> &'static Mutex<Option<ContactProvider>> {
    &CONTACTS
}
