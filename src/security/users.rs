/// User and group management
///
/// Unix-style UID/GID model with:
///   - Root (UID 0) — superuser
///   - System users (UID 1-999) — services
///   - Regular users (UID 1000+) — humans
///   - Groups for shared access
use crate::serial_println;
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

pub static USER_DB: Mutex<Option<UserDatabase>> = Mutex::new(None);

#[derive(Debug, Clone)]
pub struct User {
    pub uid: u32,
    pub name: String,
    pub home: String,
    pub shell: String,
    pub groups: Vec<u32>,
    pub password_hash: String,
}

#[derive(Debug, Clone)]
pub struct Group {
    pub gid: u32,
    pub name: String,
    pub members: Vec<u32>,
}

pub struct UserDatabase {
    users: BTreeMap<u32, User>,
    groups: BTreeMap<u32, Group>,
    next_uid: u32,
    next_gid: u32,
}

impl UserDatabase {
    pub fn new() -> Self {
        let mut db = UserDatabase {
            users: BTreeMap::new(),
            groups: BTreeMap::new(),
            next_uid: 1000,
            next_gid: 1000,
        };
        db.create_system_accounts();
        db
    }

    fn create_system_accounts(&mut self) {
        // System groups
        let groups = [
            (0, "root"),
            (1, "daemon"),
            (2, "sys"),
            (3, "adm"),
            (4, "tty"),
            (5, "disk"),
            (6, "network"),
            (7, "audio"),
            (8, "video"),
            (9, "input"),
            (10, "wheel"),
        ];
        for (gid, name) in groups {
            self.groups.insert(
                gid,
                Group {
                    gid,
                    name: String::from(name),
                    members: Vec::new(),
                },
            );
        }

        // Root user
        self.users.insert(
            0,
            User {
                uid: 0,
                name: String::from("root"),
                home: String::from("/root"),
                shell: String::from("/bin/hoags-shell"),
                groups: alloc::vec![0, 10],
                password_hash: String::from("!locked"),
            },
        );

        // System service accounts
        let svc = [
            (1, "daemon", "/dev/null"),
            (2, "network", "/var/network"),
            (3, "display", "/var/display"),
            (4, "audio", "/var/audio"),
        ];
        for (uid, name, home) in svc {
            self.users.insert(
                uid,
                User {
                    uid,
                    name: String::from(name),
                    home: String::from(home),
                    shell: String::from("/bin/false"),
                    groups: alloc::vec![uid],
                    password_hash: String::from("!locked"),
                },
            );
        }
    }

    pub fn create_user(&mut self, name: &str, password_hash: &str) -> u32 {
        let uid = self.next_uid;
        self.next_uid = self.next_uid.saturating_add(1);
        let gid = self.next_gid;
        self.next_gid = self.next_gid.saturating_add(1);

        self.groups.insert(
            gid,
            Group {
                gid,
                name: String::from(name),
                members: alloc::vec![uid],
            },
        );

        self.users.insert(
            uid,
            User {
                uid,
                name: String::from(name),
                home: alloc::format!("/home/{}", name),
                shell: String::from("/bin/hoags-shell"),
                groups: alloc::vec![gid],
                password_hash: String::from(password_hash),
            },
        );
        serial_println!("    [users] Created user {} (UID {})", name, uid);
        uid
    }

    pub fn get_user(&self, uid: u32) -> Option<&User> {
        self.users.get(&uid)
    }
    pub fn get_group(&self, gid: u32) -> Option<&Group> {
        self.groups.get(&gid)
    }

    pub fn find_user_by_name(&self, name: &str) -> Option<&User> {
        self.users.values().find(|u| u.name == name)
    }

    pub fn is_in_group(&self, uid: u32, gid: u32) -> bool {
        self.users
            .get(&uid)
            .map(|u| u.groups.contains(&gid))
            .unwrap_or(false)
    }
}

pub fn init() {
    *USER_DB.lock() = Some(UserDatabase::new());
    serial_println!("    [users] User database initialized");
}

pub fn create_user(name: &str, password_hash: &str) -> u32 {
    USER_DB
        .lock()
        .as_mut()
        .map(|db| db.create_user(name, password_hash))
        .unwrap_or(0)
}
