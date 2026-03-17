/// Content provider for Genesis
///
/// Structured data sharing between apps via URIs,
/// CRUD operations, content change notifications.
///
/// Inspired by: Android ContentProvider, iOS Core Data. All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Content URI scheme: content://authority/path/id
pub struct ContentUri {
    pub authority: String,
    pub path: String,
    pub id: Option<u64>,
}

impl ContentUri {
    pub fn parse(uri: &str) -> Option<Self> {
        if !uri.starts_with("content://") {
            return None;
        }
        let rest = &uri[10..];
        let parts: Vec<&str> = rest.splitn(2, '/').collect();
        if parts.is_empty() {
            return None;
        }

        let authority = String::from(parts[0]);
        let (path, id) = if parts.len() > 1 {
            // Check if last segment is numeric
            let path_parts: Vec<&str> = parts[1].rsplitn(2, '/').collect();
            if let Ok(num) = path_parts[0].parse::<u64>() {
                let p = if path_parts.len() > 1 {
                    path_parts[1]
                } else {
                    ""
                };
                (String::from(p), Some(num))
            } else {
                (String::from(parts[1]), None)
            }
        } else {
            (String::new(), None)
        };

        Some(ContentUri {
            authority,
            path,
            id,
        })
    }

    pub fn to_string(&self) -> String {
        match self.id {
            Some(id) => format!("content://{}/{}/{}", self.authority, self.path, id),
            None => format!("content://{}/{}", self.authority, self.path),
        }
    }
}

/// A row of content data
pub struct ContentRow {
    pub id: u64,
    pub columns: BTreeMap<String, String>,
}

/// Content provider implementation
pub struct ContentProvider {
    pub authority: String,
    pub tables: BTreeMap<String, Vec<ContentRow>>,
    pub next_id: u64,
}

impl ContentProvider {
    pub fn new(authority: &str) -> Self {
        ContentProvider {
            authority: String::from(authority),
            tables: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn insert(&mut self, table: &str, columns: BTreeMap<String, String>) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let row = ContentRow { id, columns };
        self.tables
            .entry(String::from(table))
            .or_insert_with(Vec::new)
            .push(row);
        id
    }

    pub fn query(&self, table: &str, id: Option<u64>) -> Vec<&ContentRow> {
        match self.tables.get(table) {
            Some(rows) => {
                if let Some(target_id) = id {
                    rows.iter().filter(|r| r.id == target_id).collect()
                } else {
                    rows.iter().collect()
                }
            }
            None => Vec::new(),
        }
    }

    pub fn update(&mut self, table: &str, id: u64, columns: BTreeMap<String, String>) -> bool {
        if let Some(rows) = self.tables.get_mut(table) {
            if let Some(row) = rows.iter_mut().find(|r| r.id == id) {
                for (k, v) in columns {
                    row.columns.insert(k, v);
                }
                return true;
            }
        }
        false
    }

    pub fn delete(&mut self, table: &str, id: u64) -> bool {
        if let Some(rows) = self.tables.get_mut(table) {
            let len = rows.len();
            rows.retain(|r| r.id != id);
            return rows.len() < len;
        }
        false
    }
}

/// Content provider registry
pub struct ProviderRegistry {
    pub providers: Vec<ContentProvider>,
}

impl ProviderRegistry {
    const fn new() -> Self {
        ProviderRegistry {
            providers: Vec::new(),
        }
    }

    pub fn register(&mut self, provider: ContentProvider) {
        crate::serial_println!("  [content] Registered provider: {}", provider.authority);
        self.providers.push(provider);
    }

    pub fn find(&self, authority: &str) -> Option<&ContentProvider> {
        self.providers.iter().find(|p| p.authority == authority)
    }

    pub fn find_mut(&mut self, authority: &str) -> Option<&mut ContentProvider> {
        self.providers.iter_mut().find(|p| p.authority == authority)
    }
}

static REGISTRY: Mutex<ProviderRegistry> = Mutex::new(ProviderRegistry::new());

pub fn init() {
    // Register built-in content providers
    let mut reg = REGISTRY.lock();
    reg.register(ContentProvider::new("settings"));
    reg.register(ContentProvider::new("contacts"));
    reg.register(ContentProvider::new("media"));
    crate::serial_println!("  [content] Content provider system initialized");
}
