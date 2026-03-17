/// Translation system for Genesis
///
/// String resource management, plural rules,
/// message formatting, and locale-specific strings.
///
/// Inspired by: Android Resources, gettext. All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Plural category
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plural {
    Zero,
    One,
    Two,
    Few,
    Many,
    Other,
}

/// Get plural category for English
pub fn english_plural(count: u64) -> Plural {
    if count == 1 {
        Plural::One
    } else {
        Plural::Other
    }
}

/// A string resource with optional plurals
pub struct StringResource {
    pub key: String,
    pub value: String,
    pub plurals: BTreeMap<u8, String>, // Plural variant -> string
}

/// Translation catalog for one language
pub struct Catalog {
    pub language: String,
    pub strings: BTreeMap<String, StringResource>,
}

impl Catalog {
    pub fn new(language: &str) -> Self {
        Catalog {
            language: String::from(language),
            strings: BTreeMap::new(),
        }
    }

    pub fn add(&mut self, key: &str, value: &str) {
        self.strings.insert(
            String::from(key),
            StringResource {
                key: String::from(key),
                value: String::from(value),
                plurals: BTreeMap::new(),
            },
        );
    }

    pub fn add_plural(&mut self, key: &str, one: &str, other: &str) {
        let mut plurals = BTreeMap::new();
        plurals.insert(Plural::One as u8, String::from(one));
        plurals.insert(Plural::Other as u8, String::from(other));
        self.strings.insert(
            String::from(key),
            StringResource {
                key: String::from(key),
                value: String::from(other),
                plurals,
            },
        );
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.strings.get(key).map(|r| r.value.as_str())
    }

    pub fn get_plural(&self, key: &str, count: u64) -> Option<&str> {
        let res = self.strings.get(key)?;
        let category = english_plural(count);
        res.plurals
            .get(&(category as u8))
            .map(|s| s.as_str())
            .or(Some(res.value.as_str()))
    }
}

/// Translation manager with multiple catalogs
pub struct TranslationManager {
    pub catalogs: Vec<Catalog>,
    pub current_language: String,
}

impl TranslationManager {
    const fn new() -> Self {
        TranslationManager {
            catalogs: Vec::new(),
            current_language: String::new(),
        }
    }

    pub fn add_catalog(&mut self, catalog: Catalog) {
        self.catalogs.push(catalog);
    }

    pub fn set_language(&mut self, lang: &str) {
        self.current_language = String::from(lang);
    }

    pub fn translate(&self, key: &str) -> String {
        // Try current language first
        for cat in &self.catalogs {
            if cat.language == self.current_language {
                if let Some(val) = cat.get(key) {
                    return String::from(val);
                }
            }
        }
        // Fallback to English
        for cat in &self.catalogs {
            if cat.language == "en" {
                if let Some(val) = cat.get(key) {
                    return String::from(val);
                }
            }
        }
        String::from(key)
    }

    pub fn translate_plural(&self, key: &str, count: u64) -> String {
        for cat in &self.catalogs {
            if cat.language == self.current_language {
                if let Some(val) = cat.get_plural(key, count) {
                    return String::from(val);
                }
            }
        }
        format!("{} {}", count, key)
    }
}

fn create_english_catalog() -> Catalog {
    let mut cat = Catalog::new("en");
    cat.add("app.name", "Hoags OS");
    cat.add("boot.welcome", "Welcome to Hoags OS");
    cat.add("boot.loading", "Loading system...");
    cat.add("login.username", "Username");
    cat.add("login.password", "Password");
    cat.add("login.submit", "Log in");
    cat.add("login.failed", "Login incorrect");
    cat.add("settings.display", "Display");
    cat.add("settings.sound", "Sound");
    cat.add("settings.network", "Network");
    cat.add("settings.security", "Security");
    cat.add("settings.about", "About");
    cat.add("power.shutdown", "Shut down");
    cat.add("power.restart", "Restart");
    cat.add("power.sleep", "Sleep");
    cat.add_plural("notifications.count", "{} notification", "{} notifications");
    cat.add_plural("files.count", "{} file", "{} files");
    cat
}

static TRANSLATIONS: Mutex<TranslationManager> = Mutex::new(TranslationManager::new());

pub fn init() {
    let mut mgr = TRANSLATIONS.lock();
    mgr.add_catalog(create_english_catalog());
    mgr.set_language("en");
    crate::serial_println!("  [i18n] Translation system initialized");
}

pub fn translate(key: &str) -> String {
    TRANSLATIONS.lock().translate(key)
}
