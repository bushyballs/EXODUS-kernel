use alloc::collections::BTreeMap;
/// Configuration file parser
///
/// Format:
///   [section]
///   key = value
///   key = "quoted value with spaces"
///   # comment
///   ; also a comment
use alloc::string::String;

/// A parsed configuration file
#[derive(Debug, Clone)]
pub struct Config {
    pub sections: BTreeMap<String, BTreeMap<String, String>>,
}

impl Config {
    pub fn new() -> Self {
        Config {
            sections: BTreeMap::new(),
        }
    }

    /// Parse a configuration string
    pub fn parse(input: &str) -> Self {
        let mut config = Config::new();
        let mut current_section = String::from("global");

        for line in input.lines() {
            let line = line.trim();

            // Skip empty lines and comments
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }

            // Section header
            if line.starts_with('[') && line.ends_with(']') {
                current_section = String::from(&line[1..line.len() - 1]);
                continue;
            }

            // Key = value
            if let Some(eq_pos) = line.find('=') {
                let key = line[..eq_pos].trim();
                let mut value = line[eq_pos + 1..].trim();

                // Strip quotes
                if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
                    value = &value[1..value.len() - 1];
                }

                config
                    .sections
                    .entry(current_section.clone())
                    .or_insert_with(BTreeMap::new)
                    .insert(String::from(key), String::from(value));
            }
        }

        config
    }

    /// Get a value from the config
    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        self.sections.get(section)?.get(key).map(|s| s.as_str())
    }

    /// Get a value with a default
    pub fn get_or(&self, section: &str, key: &str, default: &str) -> String {
        self.get(section, key).unwrap_or(default).into()
    }

    /// Get a boolean value
    pub fn get_bool(&self, section: &str, key: &str) -> Option<bool> {
        match self.get(section, key)? {
            "true" | "yes" | "1" | "on" => Some(true),
            "false" | "no" | "0" | "off" => Some(false),
            _ => None,
        }
    }

    /// Get an integer value
    pub fn get_int(&self, section: &str, key: &str) -> Option<i64> {
        self.get(section, key)?.parse().ok()
    }

    /// Set a value
    pub fn set(&mut self, section: &str, key: &str, value: &str) {
        self.sections
            .entry(String::from(section))
            .or_insert_with(BTreeMap::new)
            .insert(String::from(key), String::from(value));
    }

    /// Serialize back to config file format
    pub fn to_string(&self) -> String {
        let mut output = String::new();
        for (section, entries) in &self.sections {
            output.push_str(&alloc::format!("[{}]\n", section));
            for (key, value) in entries {
                if value.contains(' ') {
                    output.push_str(&alloc::format!("{} = \"{}\"\n", key, value));
                } else {
                    output.push_str(&alloc::format!("{} = {}\n", key, value));
                }
            }
            output.push('\n');
        }
        output
    }
}
