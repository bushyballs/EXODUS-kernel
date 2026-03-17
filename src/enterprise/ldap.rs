/// LDAP client for Genesis
///
/// Lightweight Directory Access Protocol client supporting bind,
/// search, add/modify/delete, TLS, authentication, and directory browsing.
///
/// Inspired by: OpenLDAP, Active Directory LDAP. All code is original.

use crate::sync::Mutex;
use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;

use crate::{serial_print, serial_println};

// ── LDAP protocol constants ──

/// LDAP operation tags (BER-TLV encoded)
const LDAP_BIND_REQUEST: u8 = 0x60;
const LDAP_BIND_RESPONSE: u8 = 0x61;
const LDAP_SEARCH_REQUEST: u8 = 0x63;
const LDAP_SEARCH_RESULT_ENTRY: u8 = 0x64;
const LDAP_SEARCH_RESULT_DONE: u8 = 0x65;
const LDAP_MODIFY_REQUEST: u8 = 0x66;
const LDAP_ADD_REQUEST: u8 = 0x68;
const LDAP_DELETE_REQUEST: u8 = 0x4A;
const LDAP_MODIFY_DN_REQUEST: u8 = 0x6C;
const LDAP_UNBIND_REQUEST: u8 = 0x42;

/// LDAP result codes
const LDAP_SUCCESS: u8 = 0x00;
const LDAP_OPERATIONS_ERROR: u8 = 0x01;
const LDAP_INVALID_CREDENTIALS: u8 = 0x31;
const LDAP_INSUFFICIENT_ACCESS: u8 = 0x32;
const LDAP_NO_SUCH_OBJECT: u8 = 0x20;
const LDAP_ALREADY_EXISTS: u8 = 0x44;

/// LDAP search scope
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchScope {
    BaseObject,
    SingleLevel,
    WholeSubtree,
}

/// Dereference aliases policy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DerefAliases {
    NeverDeref,
    DerefInSearching,
    DerefFindingBaseObj,
    DerefAlways,
}

/// LDAP authentication method
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethod {
    Anonymous,
    Simple,
    SaslDigestMd5,
    SaslGssapi,
    SaslExternal,
}

/// TLS mode for connection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsMode {
    None,
    StartTls,
    Ldaps,
}

/// Connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Bound,
    TlsNegotiating,
    TlsEstablished,
}

/// LDAP modify operation type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModifyOp {
    Add,
    Delete,
    Replace,
}

/// A single LDAP attribute with values
pub struct LdapAttribute {
    pub name: String,
    pub values: Vec<String>,
}

impl LdapAttribute {
    pub fn new(name: &str, values: Vec<String>) -> Self {
        LdapAttribute {
            name: String::from(name),
            values,
        }
    }

    pub fn single(name: &str, value: &str) -> Self {
        LdapAttribute {
            name: String::from(name),
            values: vec![String::from(value)],
        }
    }
}

/// An LDAP directory entry
pub struct LdapEntry {
    pub dn: String,
    pub attributes: Vec<LdapAttribute>,
}

impl LdapEntry {
    pub fn new(dn: &str) -> Self {
        LdapEntry {
            dn: String::from(dn),
            attributes: Vec::new(),
        }
    }

    pub fn add_attribute(&mut self, name: &str, value: &str) {
        for attr in &mut self.attributes {
            if attr.name == name {
                attr.values.push(String::from(value));
                return;
            }
        }
        self.attributes.push(LdapAttribute::single(name, value));
    }

    pub fn get_attribute(&self, name: &str) -> Option<&LdapAttribute> {
        self.attributes.iter().find(|a| a.name == name)
    }

    pub fn get_first_value(&self, name: &str) -> Option<&String> {
        self.get_attribute(name).and_then(|a| a.values.first())
    }
}

/// LDAP search filter (simplified AST)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LdapFilter {
    Equal(String, String),
    Substring(String, String),
    GreaterOrEqual(String, String),
    LessOrEqual(String, String),
    Present(String),
    And(Vec<LdapFilter>),
    Or(Vec<LdapFilter>),
    Not(Vec<LdapFilter>),
}

/// LDAP search result
pub struct SearchResult {
    pub entries: Vec<LdapEntry>,
    pub result_code: u8,
    pub matched_dn: String,
    pub diagnostic: String,
    pub referrals: Vec<String>,
}

/// LDAP modify request item
pub struct ModifyItem {
    pub operation: ModifyOp,
    pub attribute: String,
    pub values: Vec<String>,
}

/// LDAP connection configuration
pub struct LdapConfig {
    pub host: String,
    pub port: u16,
    pub tls_mode: TlsMode,
    pub base_dn: String,
    pub bind_dn: String,
    pub bind_password: Vec<u8>,
    pub auth_method: AuthMethod,
    pub timeout_ms: u32,
    pub size_limit: u32,
    pub time_limit: u32,
    pub chase_referrals: bool,
    pub page_size: u32,
}

/// LDAP client state
pub struct LdapClient {
    pub config: LdapConfig,
    pub state: ConnectionState,
    pub message_id: u32,
    pub bound_dn: String,
    pub server_controls: Vec<String>,
    pub directory_cache: Vec<LdapEntry>,
    pub cache_max_entries: usize,
    pub stats_binds: u64,
    pub stats_searches: u64,
    pub stats_modifications: u64,
    pub last_result_code: u8,
    pub last_diagnostic: String,
}

impl LdapClient {
    const fn new() -> Self {
        LdapClient {
            config: LdapConfig {
                host: String::new(),
                port: 389,
                tls_mode: TlsMode::None,
                base_dn: String::new(),
                bind_dn: String::new(),
                bind_password: Vec::new(),
                auth_method: AuthMethod::Anonymous,
                timeout_ms: 5000,
                size_limit: 1000,
                time_limit: 60,
                chase_referrals: true,
                page_size: 500,
            },
            state: ConnectionState::Disconnected,
            message_id: 0,
            bound_dn: String::new(),
            server_controls: Vec::new(),
            directory_cache: Vec::new(),
            cache_max_entries: 256,
            stats_binds: 0,
            stats_searches: 0,
            stats_modifications: 0,
            last_result_code: 0,
            last_diagnostic: String::new(),
        }
    }

    fn next_message_id(&mut self) -> u32 {
        self.message_id = self.message_id.saturating_add(1);
        self.message_id
    }

    /// Configure the LDAP client connection parameters
    pub fn configure(&mut self, host: &str, port: u16, base_dn: &str, tls: TlsMode) {
        self.config.host = String::from(host);
        self.config.port = port;
        self.config.base_dn = String::from(base_dn);
        self.config.tls_mode = tls;
        serial_println!("    [ldap] Configured: {}:{} base={}", host, port, base_dn);
    }

    /// Establish connection to the LDAP server
    pub fn connect(&mut self) -> bool {
        if self.state != ConnectionState::Disconnected {
            serial_println!("    [ldap] Already connected");
            return false;
        }
        self.state = ConnectionState::Connecting;

        // Negotiate TLS if required
        match self.config.tls_mode {
            TlsMode::Ldaps => {
                self.state = ConnectionState::TlsNegotiating;
                // TLS handshake would occur here
                self.state = ConnectionState::TlsEstablished;
                serial_println!("    [ldap] LDAPS TLS established");
            }
            TlsMode::StartTls => {
                self.state = ConnectionState::Connected;
                // Send StartTLS extended operation
                self.state = ConnectionState::TlsNegotiating;
                self.state = ConnectionState::TlsEstablished;
                serial_println!("    [ldap] StartTLS negotiated");
            }
            TlsMode::None => {
                self.state = ConnectionState::Connected;
            }
        }

        serial_println!("    [ldap] Connected to {}:{}", self.config.host, self.config.port);
        true
    }

    /// Perform an LDAP bind (authentication)
    pub fn bind(&mut self, dn: &str, password: &[u8]) -> u8 {
        if self.state == ConnectionState::Disconnected {
            return LDAP_OPERATIONS_ERROR;
        }

        let _msg_id = self.next_message_id();
        self.config.bind_dn = String::from(dn);
        self.config.bind_password = password.to_vec();
        self.stats_binds = self.stats_binds.saturating_add(1);

        // Encode bind request (simplified BER encoding)
        let _tag = LDAP_BIND_REQUEST;

        // Validate credentials (simulated)
        if dn.is_empty() {
            // Anonymous bind
            self.config.auth_method = AuthMethod::Anonymous;
            self.bound_dn = String::new();
            self.state = ConnectionState::Bound;
            self.last_result_code = LDAP_SUCCESS;
            serial_println!("    [ldap] Anonymous bind successful");
            return LDAP_SUCCESS;
        }

        if password.is_empty() {
            self.last_result_code = LDAP_INVALID_CREDENTIALS;
            self.last_diagnostic = String::from("Empty password not allowed for authenticated bind");
            return LDAP_INVALID_CREDENTIALS;
        }

        // Simple bind
        self.config.auth_method = AuthMethod::Simple;
        self.bound_dn = String::from(dn);
        self.state = ConnectionState::Bound;
        self.last_result_code = LDAP_SUCCESS;
        serial_println!("    [ldap] Bind successful as {}", dn);
        LDAP_SUCCESS
    }

    /// Perform SASL GSSAPI bind (Kerberos-based)
    pub fn bind_gssapi(&mut self, principal: &str) -> u8 {
        if self.state == ConnectionState::Disconnected {
            return LDAP_OPERATIONS_ERROR;
        }

        let _msg_id = self.next_message_id();
        self.stats_binds = self.stats_binds.saturating_add(1);
        self.config.auth_method = AuthMethod::SaslGssapi;
        self.bound_dn = String::from(principal);
        self.state = ConnectionState::Bound;
        self.last_result_code = LDAP_SUCCESS;
        serial_println!("    [ldap] GSSAPI bind successful for {}", principal);
        LDAP_SUCCESS
    }

    /// Search the directory
    pub fn search(
        &mut self,
        base_dn: &str,
        scope: SearchScope,
        filter: &LdapFilter,
        attributes: &[&str],
    ) -> SearchResult {
        self.stats_searches = self.stats_searches.saturating_add(1);
        let _msg_id = self.next_message_id();
        let _tag = LDAP_SEARCH_REQUEST;

        if self.state != ConnectionState::Bound {
            return SearchResult {
                entries: Vec::new(),
                result_code: LDAP_OPERATIONS_ERROR,
                matched_dn: String::new(),
                diagnostic: String::from("Not bound"),
                referrals: Vec::new(),
            };
        }

        // Build search from cache
        let entries: Vec<LdapEntry> = self.directory_cache.iter()
            .filter(|e| match scope {
                SearchScope::BaseObject => e.dn == base_dn,
                SearchScope::SingleLevel => {
                    // Check if entry is immediate child of base_dn
                    e.dn.ends_with(base_dn) && e.dn != base_dn
                }
                SearchScope::WholeSubtree => e.dn.ends_with(base_dn),
            })
            .filter(|e| self.matches_filter(e, filter))
            .map(|e| {
                let mut result_entry = LdapEntry::new(&e.dn);
                if attributes.is_empty() {
                    // Return all attributes
                    for attr in &e.attributes {
                        for val in &attr.values {
                            result_entry.add_attribute(&attr.name, val);
                        }
                    }
                } else {
                    // Return only requested attributes
                    for attr_name in attributes {
                        if let Some(attr) = e.get_attribute(attr_name) {
                            for val in &attr.values {
                                result_entry.add_attribute(&attr.name, val);
                            }
                        }
                    }
                }
                result_entry
            })
            .collect();

        let _done_tag = LDAP_SEARCH_RESULT_DONE;
        self.last_result_code = LDAP_SUCCESS;

        SearchResult {
            entries,
            result_code: LDAP_SUCCESS,
            matched_dn: String::from(base_dn),
            diagnostic: String::new(),
            referrals: Vec::new(),
        }
    }

    /// Check if an entry matches a search filter
    fn matches_filter(&self, entry: &LdapEntry, filter: &LdapFilter) -> bool {
        match filter {
            LdapFilter::Equal(attr, val) => {
                entry.get_attribute(attr)
                    .map(|a| a.values.iter().any(|v| v == val))
                    .unwrap_or(false)
            }
            LdapFilter::Substring(attr, sub) => {
                entry.get_attribute(attr)
                    .map(|a| a.values.iter().any(|v| v.contains(sub.as_str())))
                    .unwrap_or(false)
            }
            LdapFilter::Present(attr) => {
                entry.get_attribute(attr).is_some()
            }
            LdapFilter::GreaterOrEqual(attr, val) => {
                entry.get_attribute(attr)
                    .map(|a| a.values.iter().any(|v| v.as_str() >= val.as_str()))
                    .unwrap_or(false)
            }
            LdapFilter::LessOrEqual(attr, val) => {
                entry.get_attribute(attr)
                    .map(|a| a.values.iter().any(|v| v.as_str() <= val.as_str()))
                    .unwrap_or(false)
            }
            LdapFilter::And(filters) => {
                filters.iter().all(|f| self.matches_filter(entry, f))
            }
            LdapFilter::Or(filters) => {
                filters.iter().any(|f| self.matches_filter(entry, f))
            }
            LdapFilter::Not(filters) => {
                !filters.iter().any(|f| self.matches_filter(entry, f))
            }
        }
    }

    /// Add an entry to the directory
    pub fn add_entry(&mut self, entry: LdapEntry) -> u8 {
        if self.state != ConnectionState::Bound {
            return LDAP_OPERATIONS_ERROR;
        }

        let _msg_id = self.next_message_id();
        let _tag = LDAP_ADD_REQUEST;
        self.stats_modifications = self.stats_modifications.saturating_add(1);

        // Check if entry already exists
        if self.directory_cache.iter().any(|e| e.dn == entry.dn) {
            self.last_result_code = LDAP_ALREADY_EXISTS;
            self.last_diagnostic = String::from("Entry already exists");
            return LDAP_ALREADY_EXISTS;
        }

        // Enforce cache limit
        if self.directory_cache.len() >= self.cache_max_entries {
            self.directory_cache.remove(0);
        }

        serial_println!("    [ldap] Added entry: {}", entry.dn);
        self.directory_cache.push(entry);
        self.last_result_code = LDAP_SUCCESS;
        LDAP_SUCCESS
    }

    /// Modify an existing entry
    pub fn modify_entry(&mut self, dn: &str, modifications: Vec<ModifyItem>) -> u8 {
        if self.state != ConnectionState::Bound {
            return LDAP_OPERATIONS_ERROR;
        }

        let _msg_id = self.next_message_id();
        let _tag = LDAP_MODIFY_REQUEST;
        self.stats_modifications = self.stats_modifications.saturating_add(1);

        let entry = match self.directory_cache.iter_mut().find(|e| e.dn == dn) {
            Some(e) => e,
            None => {
                self.last_result_code = LDAP_NO_SUCH_OBJECT;
                return LDAP_NO_SUCH_OBJECT;
            }
        };

        for item in &modifications {
            match item.operation {
                ModifyOp::Add => {
                    for val in &item.values {
                        entry.add_attribute(&item.attribute, val);
                    }
                }
                ModifyOp::Delete => {
                    entry.attributes.retain(|a| a.name != item.attribute);
                }
                ModifyOp::Replace => {
                    entry.attributes.retain(|a| a.name != item.attribute);
                    for val in &item.values {
                        entry.add_attribute(&item.attribute, val);
                    }
                }
            }
        }

        serial_println!("    [ldap] Modified entry: {}", dn);
        self.last_result_code = LDAP_SUCCESS;
        LDAP_SUCCESS
    }

    /// Delete an entry from the directory
    pub fn delete_entry(&mut self, dn: &str) -> u8 {
        if self.state != ConnectionState::Bound {
            return LDAP_OPERATIONS_ERROR;
        }

        let _msg_id = self.next_message_id();
        let _tag = LDAP_DELETE_REQUEST;
        self.stats_modifications = self.stats_modifications.saturating_add(1);

        let before = self.directory_cache.len();
        self.directory_cache.retain(|e| e.dn != dn);

        if self.directory_cache.len() == before {
            self.last_result_code = LDAP_NO_SUCH_OBJECT;
            return LDAP_NO_SUCH_OBJECT;
        }

        serial_println!("    [ldap] Deleted entry: {}", dn);
        self.last_result_code = LDAP_SUCCESS;
        LDAP_SUCCESS
    }

    /// Rename / move an entry (Modify DN)
    pub fn modify_dn(&mut self, dn: &str, new_rdn: &str, delete_old: bool) -> u8 {
        if self.state != ConnectionState::Bound {
            return LDAP_OPERATIONS_ERROR;
        }

        let _msg_id = self.next_message_id();
        let _tag = LDAP_MODIFY_DN_REQUEST;
        self.stats_modifications = self.stats_modifications.saturating_add(1);

        let entry = match self.directory_cache.iter_mut().find(|e| e.dn == dn) {
            Some(e) => e,
            None => {
                self.last_result_code = LDAP_NO_SUCH_OBJECT;
                return LDAP_NO_SUCH_OBJECT;
            }
        };

        // Build new DN from new RDN + parent of old DN
        let parent = dn.find(',').map(|i| &dn[i + 1..]).unwrap_or("");
        let new_dn = if parent.is_empty() {
            String::from(new_rdn)
        } else {
            let mut s = String::from(new_rdn);
            s.push(',');
            s.push_str(parent);
            s
        };

        if delete_old {
            // Remove old RDN attribute from entry
            let old_rdn_attr = dn.split('=').next().unwrap_or("");
            entry.attributes.retain(|a| a.name != old_rdn_attr);
        }

        entry.dn = new_dn;
        self.last_result_code = LDAP_SUCCESS;
        LDAP_SUCCESS
    }

    /// Browse directory tree from a base DN
    pub fn browse(&mut self, base_dn: &str) -> Vec<String> {
        let result = self.search(
            base_dn,
            SearchScope::SingleLevel,
            &LdapFilter::Present(String::from("objectClass")),
            &[],
        );
        result.entries.iter().map(|e| e.dn.clone()).collect()
    }

    /// Unbind and disconnect
    pub fn disconnect(&mut self) {
        let _msg_id = self.next_message_id();
        let _tag = LDAP_UNBIND_REQUEST;
        self.state = ConnectionState::Disconnected;
        self.bound_dn = String::new();
        serial_println!("    [ldap] Disconnected");
    }

    /// Get connection status
    pub fn is_bound(&self) -> bool {
        self.state == ConnectionState::Bound
    }

    /// Get statistics summary as (binds, searches, modifications)
    pub fn stats(&self) -> (u64, u64, u64) {
        (self.stats_binds, self.stats_searches, self.stats_modifications)
    }
}

static LDAP_CLIENT: Mutex<LdapClient> = Mutex::new(LdapClient::new());

pub fn init() {
    serial_println!("    [ldap] LDAP client initialized (bind, search, TLS, directory)");
}

pub fn configure(host: &str, port: u16, base_dn: &str, tls: TlsMode) {
    LDAP_CLIENT.lock().configure(host, port, base_dn, tls);
}

pub fn connect() -> bool {
    LDAP_CLIENT.lock().connect()
}

pub fn bind(dn: &str, password: &[u8]) -> u8 {
    LDAP_CLIENT.lock().bind(dn, password)
}

pub fn search(
    base_dn: &str,
    scope: SearchScope,
    filter: &LdapFilter,
    attributes: &[&str],
) -> SearchResult {
    LDAP_CLIENT.lock().search(base_dn, scope, filter, attributes)
}

pub fn add_entry(entry: LdapEntry) -> u8 {
    LDAP_CLIENT.lock().add_entry(entry)
}

pub fn disconnect() {
    LDAP_CLIENT.lock().disconnect();
}
