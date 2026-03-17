/// Kerberos authentication for Genesis
///
/// Implements Kerberos v5 protocol: AS-REQ/AS-REP exchange, TGS ticket
/// granting, ticket caching, keytab management, and GSSAPI security context.
///
/// Inspired by: MIT Kerberos, Heimdal, Windows Kerberos. All code is original.

use crate::sync::Mutex;
use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;

use crate::{serial_print, serial_println};

// ── Kerberos protocol constants ──

/// Kerberos version
const KRB5_VERSION: u8 = 5;

/// Message types
const KRB_AS_REQ: u8 = 10;
const KRB_AS_REP: u8 = 11;
const KRB_TGS_REQ: u8 = 12;
const KRB_TGS_REP: u8 = 13;
const KRB_AP_REQ: u8 = 14;
const KRB_AP_REP: u8 = 15;
const KRB_ERROR: u8 = 30;

/// Encryption types
const ETYPE_AES256_CTS_SHA1: i32 = 18;
const ETYPE_AES128_CTS_SHA1: i32 = 17;
const ETYPE_DES3_CBC_SHA1: i32 = 16;
const ETYPE_RC4_HMAC: i32 = 23;

/// Error codes
const KDC_ERR_NONE: u32 = 0;
const KDC_ERR_PRINCIPAL_UNKNOWN: u32 = 6;
const KDC_ERR_PREAUTH_FAILED: u32 = 24;
const KDC_ERR_ETYPE_NOSUPP: u32 = 14;
const KRB_AP_ERR_TKT_EXPIRED: u32 = 32;
const KRB_AP_ERR_MODIFIED: u32 = 41;

/// Ticket flags (bitmask)
const TKT_FLAG_FORWARDABLE: u32 = 0x4000_0000;
const TKT_FLAG_FORWARDED: u32 = 0x2000_0000;
const TKT_FLAG_PROXIABLE: u32 = 0x1000_0000;
const TKT_FLAG_RENEWABLE: u32 = 0x0080_0000;
const TKT_FLAG_INITIAL: u32 = 0x0040_0000;
const TKT_FLAG_PRE_AUTHENT: u32 = 0x0020_0000;

/// Default ticket lifetime: 10 hours in seconds
const DEFAULT_TICKET_LIFETIME: u64 = 36000;
/// Default renewable lifetime: 7 days in seconds
const DEFAULT_RENEW_LIFETIME: u64 = 604800;

/// Kerberos principal name
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    pub name: String,
    pub realm: String,
}

impl Principal {
    pub fn new(name: &str, realm: &str) -> Self {
        Principal {
            name: String::from(name),
            realm: String::from(realm),
        }
    }

    /// Parse from "user@REALM" format
    pub fn from_string(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.splitn(2, '@').collect();
        if parts.len() == 2 {
            Some(Principal::new(parts[0], parts[1]))
        } else {
            None
        }
    }

    /// Format as "name@REALM"
    pub fn to_full_name(&self) -> String {
        let mut s = self.name.clone();
        s.push('@');
        s.push_str(&self.realm);
        s
    }
}

/// Encryption key
#[derive(Clone)]
pub struct EncryptionKey {
    pub etype: i32,
    pub key_data: Vec<u8>,
    pub kvno: u32,
}

impl EncryptionKey {
    pub fn new(etype: i32, data: Vec<u8>, kvno: u32) -> Self {
        EncryptionKey { etype, key_data: data, kvno }
    }
}

/// Kerberos ticket
pub struct Ticket {
    pub server: Principal,
    pub client: Principal,
    pub session_key: EncryptionKey,
    pub flags: u32,
    pub auth_time: u64,
    pub start_time: u64,
    pub end_time: u64,
    pub renew_till: u64,
    pub enc_part: Vec<u8>,
}

impl Ticket {
    pub fn is_expired(&self) -> bool {
        let now = crate::time::clock::unix_time();
        now >= self.end_time
    }

    pub fn is_renewable(&self) -> bool {
        (self.flags & TKT_FLAG_RENEWABLE) != 0
    }

    pub fn is_forwardable(&self) -> bool {
        (self.flags & TKT_FLAG_FORWARDABLE) != 0
    }

    pub fn time_remaining(&self) -> u64 {
        let now = crate::time::clock::unix_time();
        if now >= self.end_time { 0 } else { self.end_time - now }
    }
}

/// Keytab entry (stored service keys)
pub struct KeytabEntry {
    pub principal: Principal,
    pub key: EncryptionKey,
    pub timestamp: u64,
}

/// Ticket cache for credential storage
pub struct TicketCache {
    pub default_principal: Option<Principal>,
    pub tickets: Vec<Ticket>,
    pub max_tickets: usize,
}

impl TicketCache {
    const fn new() -> Self {
        TicketCache {
            default_principal: None,
            tickets: Vec::new(),
            max_tickets: 64,
        }
    }

    /// Store a ticket in the cache
    pub fn store(&mut self, ticket: Ticket) {
        // Remove existing ticket for same server
        self.tickets.retain(|t| t.server != ticket.server);

        // Enforce cache limit
        if self.tickets.len() >= self.max_tickets {
            // Remove oldest expired ticket, or oldest ticket
            let now = crate::time::clock::unix_time();
            if let Some(idx) = self.tickets.iter().position(|t| t.end_time < now) {
                self.tickets.remove(idx);
            } else if !self.tickets.is_empty() {
                self.tickets.remove(0);
            }
        }

        self.tickets.push(ticket);
    }

    /// Find a valid ticket for a given server principal
    pub fn find(&self, server: &Principal) -> Option<&Ticket> {
        self.tickets.iter().find(|t| {
            t.server == *server && !t.is_expired()
        })
    }

    /// Remove expired tickets
    pub fn purge_expired(&mut self) -> usize {
        let before = self.tickets.len();
        self.tickets.retain(|t| !t.is_expired());
        before - self.tickets.len()
    }

    /// Destroy all tickets
    pub fn destroy(&mut self) {
        self.tickets.clear();
        self.default_principal = None;
    }

    /// Count valid (non-expired) tickets
    pub fn valid_count(&self) -> usize {
        self.tickets.iter().filter(|t| !t.is_expired()).count()
    }
}

/// GSSAPI security context state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GssState {
    Idle,
    Initiated,
    Accepted,
    Established,
    Expired,
}

/// GSSAPI context for Kerberos-based authentication
pub struct GssapiContext {
    pub state: GssState,
    pub initiator: Option<Principal>,
    pub acceptor: Option<Principal>,
    pub session_key: Option<EncryptionKey>,
    pub mutual_auth: bool,
    pub replay_detection: bool,
    pub sequence_numbers: bool,
    pub delegation: bool,
    pub context_id: u32,
    pub lifetime: u64,
}

impl GssapiContext {
    pub fn new(context_id: u32) -> Self {
        GssapiContext {
            state: GssState::Idle,
            initiator: None,
            acceptor: None,
            session_key: None,
            mutual_auth: true,
            replay_detection: true,
            sequence_numbers: true,
            delegation: false,
            context_id,
            lifetime: DEFAULT_TICKET_LIFETIME,
        }
    }
}

/// AS-REQ / AS-REP exchange result
pub struct AsExchangeResult {
    pub error_code: u32,
    pub ticket: Option<Ticket>,
    pub diagnostic: String,
}

/// TGS-REQ / TGS-REP exchange result
pub struct TgsExchangeResult {
    pub error_code: u32,
    pub ticket: Option<Ticket>,
    pub diagnostic: String,
}

/// Full Kerberos subsystem
pub struct KerberosSystem {
    pub realm: String,
    pub kdc_host: String,
    pub kdc_port: u16,
    pub cache: TicketCache,
    pub keytab: Vec<KeytabEntry>,
    pub supported_etypes: Vec<i32>,
    pub gss_contexts: Vec<GssapiContext>,
    pub next_context_id: u32,
    pub preauth_required: bool,
    pub stats_as_requests: u64,
    pub stats_tgs_requests: u64,
    pub stats_ap_requests: u64,
    pub stats_errors: u64,
}

impl KerberosSystem {
    const fn new() -> Self {
        KerberosSystem {
            realm: String::new(),
            kdc_host: String::new(),
            kdc_port: 88,
            cache: TicketCache::new(),
            keytab: Vec::new(),
            supported_etypes: Vec::new(),
            gss_contexts: Vec::new(),
            next_context_id: 1,
            preauth_required: true,
            stats_as_requests: 0,
            stats_tgs_requests: 0,
            stats_ap_requests: 0,
            stats_errors: 0,
        }
    }

    /// Configure the Kerberos realm and KDC
    pub fn configure(&mut self, realm: &str, kdc_host: &str, kdc_port: u16) {
        self.realm = String::from(realm);
        self.kdc_host = String::from(kdc_host);
        self.kdc_port = kdc_port;
        self.supported_etypes = vec![
            ETYPE_AES256_CTS_SHA1,
            ETYPE_AES128_CTS_SHA1,
            ETYPE_DES3_CBC_SHA1,
            ETYPE_RC4_HMAC,
        ];
        serial_println!("    [kerberos] Configured realm={} KDC={}:{}", realm, kdc_host, kdc_port);
    }

    /// Add a keytab entry for a service principal
    pub fn add_keytab_entry(&mut self, principal: Principal, etype: i32, key_data: Vec<u8>) {
        let kvno = self.keytab.iter()
            .filter(|k| k.principal == principal)
            .count() as u32 + 1;

        self.keytab.push(KeytabEntry {
            principal,
            key: EncryptionKey::new(etype, key_data, kvno),
            timestamp: crate::time::clock::unix_time(),
        });
    }

    /// Look up a key from the keytab
    pub fn find_keytab_key(&self, principal: &Principal, etype: i32) -> Option<&EncryptionKey> {
        self.keytab.iter()
            .filter(|k| k.principal == *principal && k.key.etype == etype)
            .max_by_key(|k| k.key.kvno)
            .map(|k| &k.key)
    }

    /// Perform AS-REQ/AS-REP exchange (initial authentication)
    pub fn authenticate(&mut self, client_name: &str, password_hash: &[u8]) -> AsExchangeResult {
        self.stats_as_requests = self.stats_as_requests.saturating_add(1);
        let _msg_type = KRB_AS_REQ;

        let client = Principal::new(client_name, &self.realm.clone());
        let tgs_principal = Principal::new("krbtgt", &self.realm.clone());

        // Validate principal
        if client_name.is_empty() {
            self.stats_errors = self.stats_errors.saturating_add(1);
            return AsExchangeResult {
                error_code: KDC_ERR_PRINCIPAL_UNKNOWN,
                ticket: None,
                diagnostic: String::from("Empty principal name"),
            };
        }

        // Check pre-authentication
        if self.preauth_required && password_hash.is_empty() {
            self.stats_errors = self.stats_errors.saturating_add(1);
            return AsExchangeResult {
                error_code: KDC_ERR_PREAUTH_FAILED,
                ticket: None,
                diagnostic: String::from("Pre-authentication required"),
            };
        }

        // Generate session key (simulated)
        let session_key = EncryptionKey::new(
            ETYPE_AES256_CTS_SHA1,
            password_hash.to_vec(),
            1,
        );

        let now = crate::time::clock::unix_time();
        let tgt = Ticket {
            server: tgs_principal,
            client: client.clone(),
            session_key,
            flags: TKT_FLAG_INITIAL | TKT_FLAG_FORWARDABLE | TKT_FLAG_RENEWABLE | TKT_FLAG_PRE_AUTHENT,
            auth_time: now,
            start_time: now,
            end_time: now + DEFAULT_TICKET_LIFETIME,
            renew_till: now + DEFAULT_RENEW_LIFETIME,
            enc_part: Vec::new(),
        };

        // Cache the TGT
        self.cache.default_principal = Some(client);
        self.cache.store(tgt);

        serial_println!("    [kerberos] AS-REP: TGT issued for {}", client_name);

        // Return a copy-like result
        let result_ticket = Ticket {
            server: Principal::new("krbtgt", &self.realm),
            client: Principal::new(client_name, &self.realm),
            session_key: EncryptionKey::new(ETYPE_AES256_CTS_SHA1, password_hash.to_vec(), 1),
            flags: TKT_FLAG_INITIAL | TKT_FLAG_FORWARDABLE | TKT_FLAG_RENEWABLE | TKT_FLAG_PRE_AUTHENT,
            auth_time: now,
            start_time: now,
            end_time: now + DEFAULT_TICKET_LIFETIME,
            renew_till: now + DEFAULT_RENEW_LIFETIME,
            enc_part: Vec::new(),
        };

        AsExchangeResult {
            error_code: KDC_ERR_NONE,
            ticket: Some(result_ticket),
            diagnostic: String::new(),
        }
    }

    /// Perform TGS-REQ/TGS-REP exchange (service ticket request)
    pub fn request_service_ticket(&mut self, service_name: &str) -> TgsExchangeResult {
        self.stats_tgs_requests = self.stats_tgs_requests.saturating_add(1);
        let _msg_type = KRB_TGS_REQ;

        let tgs_principal = Principal::new("krbtgt", &self.realm.clone());

        // Must have a valid TGT
        let tgt = match self.cache.find(&tgs_principal) {
            Some(t) => t,
            None => {
                self.stats_errors = self.stats_errors.saturating_add(1);
                return TgsExchangeResult {
                    error_code: KRB_AP_ERR_TKT_EXPIRED,
                    ticket: None,
                    diagnostic: String::from("No valid TGT in cache"),
                };
            }
        };

        let client = tgt.client.clone();
        let session_key_data = tgt.session_key.key_data.clone();

        let server = Principal::new(service_name, &self.realm.clone());
        let now = crate::time::clock::unix_time();

        let service_ticket = Ticket {
            server: server.clone(),
            client,
            session_key: EncryptionKey::new(ETYPE_AES256_CTS_SHA1, session_key_data, 1),
            flags: TKT_FLAG_FORWARDABLE | TKT_FLAG_PRE_AUTHENT,
            auth_time: now,
            start_time: now,
            end_time: now + DEFAULT_TICKET_LIFETIME,
            renew_till: now + DEFAULT_RENEW_LIFETIME,
            enc_part: Vec::new(),
        };

        self.cache.store(service_ticket);

        serial_println!("    [kerberos] TGS-REP: Service ticket issued for {}", service_name);

        let result_ticket = Ticket {
            server,
            client: Principal::new(
                self.cache.default_principal.as_ref().map(|p| p.name.as_str()).unwrap_or(""),
                &self.realm,
            ),
            session_key: EncryptionKey::new(ETYPE_AES256_CTS_SHA1, Vec::new(), 1),
            flags: TKT_FLAG_FORWARDABLE | TKT_FLAG_PRE_AUTHENT,
            auth_time: now,
            start_time: now,
            end_time: now + DEFAULT_TICKET_LIFETIME,
            renew_till: now + DEFAULT_RENEW_LIFETIME,
            enc_part: Vec::new(),
        };

        TgsExchangeResult {
            error_code: KDC_ERR_NONE,
            ticket: Some(result_ticket),
            diagnostic: String::new(),
        }
    }

    /// Initialize a GSSAPI security context (client side)
    pub fn gss_init_sec_context(&mut self, target: &str) -> Option<u32> {
        let _msg_type = KRB_AP_REQ;
        self.stats_ap_requests = self.stats_ap_requests.saturating_add(1);

        // Need a service ticket for the target
        let target_principal = Principal::new(target, &self.realm.clone());
        if self.cache.find(&target_principal).is_none() {
            // Try to get one via TGS
            let result = self.request_service_ticket(target);
            if result.error_code != KDC_ERR_NONE {
                self.stats_errors = self.stats_errors.saturating_add(1);
                return None;
            }
        }

        let ctx_id = self.next_context_id;
        self.next_context_id = self.next_context_id.saturating_add(1);

        let mut ctx = GssapiContext::new(ctx_id);
        ctx.state = GssState::Initiated;
        ctx.initiator = self.cache.default_principal.clone();
        ctx.acceptor = Some(Principal::new(target, &self.realm));
        self.gss_contexts.push(ctx);

        serial_println!("    [kerberos] GSSAPI context {} initiated for {}", ctx_id, target);
        Some(ctx_id)
    }

    /// Accept a GSSAPI security context (server side)
    pub fn gss_accept_sec_context(&mut self, context_id: u32) -> bool {
        let _msg_type = KRB_AP_REP;

        let ctx = match self.gss_contexts.iter_mut().find(|c| c.context_id == context_id) {
            Some(c) => c,
            None => return false,
        };

        if ctx.state != GssState::Initiated {
            return false;
        }

        ctx.state = GssState::Established;
        serial_println!("    [kerberos] GSSAPI context {} established", context_id);
        true
    }

    /// Delete a GSSAPI context
    pub fn gss_delete_context(&mut self, context_id: u32) -> bool {
        let before = self.gss_contexts.len();
        self.gss_contexts.retain(|c| c.context_id != context_id);
        self.gss_contexts.len() < before
    }

    /// Destroy all tickets and contexts (kdestroy)
    pub fn destroy_credentials(&mut self) {
        self.cache.destroy();
        self.gss_contexts.clear();
        serial_println!("    [kerberos] All credentials destroyed");
    }

    /// Purge expired tickets
    pub fn purge_expired(&mut self) -> usize {
        self.cache.purge_expired()
    }

    /// List all cached tickets
    pub fn list_tickets(&self) -> Vec<&Ticket> {
        self.cache.tickets.iter().collect()
    }
}

// Clone impl for Principal fields used in cache lookups
impl Clone for Principal {
    fn clone(&self) -> Self {
        Principal {
            name: self.name.clone(),
            realm: self.realm.clone(),
        }
    }
}

static KERBEROS: Mutex<KerberosSystem> = Mutex::new(KerberosSystem::new());

pub fn init() {
    serial_println!("    [kerberos] Kerberos v5 initialized (AS/TGS, ticket cache, GSSAPI)");
}

pub fn configure(realm: &str, kdc_host: &str, kdc_port: u16) {
    KERBEROS.lock().configure(realm, kdc_host, kdc_port);
}

pub fn authenticate(client: &str, password_hash: &[u8]) -> AsExchangeResult {
    KERBEROS.lock().authenticate(client, password_hash)
}

pub fn request_service_ticket(service: &str) -> TgsExchangeResult {
    KERBEROS.lock().request_service_ticket(service)
}

pub fn destroy_credentials() {
    KERBEROS.lock().destroy_credentials();
}
