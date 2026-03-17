/// Web APIs for Genesis browser
///
/// Implements a subset of standard web APIs: fetch() (request/response),
/// XMLHttpRequest, WebSocket client, localStorage, sessionStorage,
/// and a lightweight IndexedDB. All storage is in-memory with
/// configurable size limits. No actual network I/O — requests are
/// queued for the OS network stack.

use crate::{serial_print, serial_println};
use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;
use crate::sync::Mutex;

static WEB_API_STATE: Mutex<Option<WebApiState>> = Mutex::new(None);

/// Maximum localStorage entries
const MAX_LOCAL_STORAGE: usize = 512;

/// Maximum sessionStorage entries
const MAX_SESSION_STORAGE: usize = 256;

/// Maximum value size in bytes for storage
const MAX_STORAGE_VALUE_SIZE: usize = 4096;

/// Maximum pending fetch requests
const MAX_PENDING_REQUESTS: usize = 64;

/// Maximum WebSocket connections
const MAX_WEBSOCKETS: usize = 16;

/// Maximum IndexedDB object stores
const MAX_OBJECT_STORES: usize = 32;

/// Maximum records per object store
const MAX_RECORDS_PER_STORE: usize = 1024;

/// Maximum XHR instances
const MAX_XHR: usize = 32;

/// FNV-1a hash
fn web_hash(s: &[u8]) -> u64 {
    let mut h: u64 = 0xCBF29CE484222325;
    for &b in s {
        h ^= b as u64;
        h = h.wrapping_mul(0x00000100000001B3);
    }
    h
}

// ---------------------------------------------------------------------------
// Fetch API
// ---------------------------------------------------------------------------

/// HTTP method
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
    Options,
}

/// Fetch request state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchState {
    Pending,
    Sent,
    HeadersReceived,
    Loading,
    Done,
    Error,
    Aborted,
}

/// An HTTP header (name hash + value bytes)
#[derive(Debug, Clone)]
pub struct HttpHeader {
    pub name_hash: u64,
    pub name: Vec<u8>,
    pub value: Vec<u8>,
}

/// A fetch request
#[derive(Debug, Clone)]
pub struct FetchRequest {
    pub id: u32,
    pub method: HttpMethod,
    pub url_hash: u64,
    pub url: Vec<u8>,
    pub headers: Vec<HttpHeader>,
    pub body: Vec<u8>,
    pub state: FetchState,
    pub promise_id: u32,        // associated Promise for async resolution
    pub timeout_ms: u32,
    pub credentials: bool,      // include cookies
    pub mode: RequestMode,
}

/// Request mode (cors/no-cors/same-origin)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestMode {
    Cors,
    NoCors,
    SameOrigin,
    Navigate,
}

/// A fetch response
#[derive(Debug, Clone)]
pub struct FetchResponse {
    pub request_id: u32,
    pub status: u16,
    pub status_text_hash: u64,
    pub headers: Vec<HttpHeader>,
    pub body: Vec<u8>,
    pub ok: bool,               // status 200-299
    pub redirected: bool,
    pub response_type: ResponseType,
}

/// Response type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseType {
    Basic,
    Cors,
    Error,
    Opaque,
    OpaqueRedirect,
}

// ---------------------------------------------------------------------------
// XMLHttpRequest
// ---------------------------------------------------------------------------

/// XHR ready state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XhrReadyState {
    Unsent = 0,
    Opened = 1,
    HeadersReceived = 2,
    Loading = 3,
    Done = 4,
}

/// An XMLHttpRequest instance
#[derive(Debug, Clone)]
pub struct XmlHttpRequest {
    pub id: u32,
    pub method: HttpMethod,
    pub url_hash: u64,
    pub url: Vec<u8>,
    pub ready_state: XhrReadyState,
    pub status: u16,
    pub response_headers: Vec<HttpHeader>,
    pub request_headers: Vec<HttpHeader>,
    pub response_body: Vec<u8>,
    pub request_body: Vec<u8>,
    pub async_mode: bool,
    pub timeout_ms: u32,
    pub onreadystatechange_cb: Option<u32>,
    pub onload_cb: Option<u32>,
    pub onerror_cb: Option<u32>,
    pub sent: bool,
}

// ---------------------------------------------------------------------------
// WebSocket
// ---------------------------------------------------------------------------

/// WebSocket connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WsState {
    Connecting = 0,
    Open = 1,
    Closing = 2,
    Closed = 3,
}

/// WebSocket message type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WsMessageType {
    Text,
    Binary,
    Ping,
    Pong,
    Close,
}

/// A WebSocket message
#[derive(Debug, Clone)]
pub struct WsMessage {
    pub msg_type: WsMessageType,
    pub data: Vec<u8>,
}

/// A WebSocket connection
#[derive(Debug, Clone)]
pub struct WebSocket {
    pub id: u32,
    pub url_hash: u64,
    pub url: Vec<u8>,
    pub state: WsState,
    pub protocol_hash: u64,
    pub send_queue: Vec<WsMessage>,
    pub recv_queue: Vec<WsMessage>,
    pub onopen_cb: Option<u32>,
    pub onmessage_cb: Option<u32>,
    pub onclose_cb: Option<u32>,
    pub onerror_cb: Option<u32>,
    pub buffered_amount: u32,
}

// ---------------------------------------------------------------------------
// Storage (localStorage / sessionStorage)
// ---------------------------------------------------------------------------

/// A storage entry (key-value pair as byte vectors)
#[derive(Debug, Clone)]
pub struct StorageEntry {
    pub key_hash: u64,
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

/// Storage area type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageKind {
    Local,
    Session,
}

// ---------------------------------------------------------------------------
// IndexedDB Lite
// ---------------------------------------------------------------------------

/// An IndexedDB record
#[derive(Debug, Clone)]
pub struct IdbRecord {
    pub key: i32,               // integer key (auto-increment)
    pub key_hash: u64,          // hash of string key if used
    pub value: Vec<u8>,         // serialized value
    pub alive: bool,
}

/// An IndexedDB object store
#[derive(Debug, Clone)]
pub struct IdbObjectStore {
    pub name_hash: u64,
    pub name: Vec<u8>,
    pub records: Vec<IdbRecord>,
    pub auto_increment: bool,
    pub next_key: i32,
    pub index_hashes: Vec<u64>, // hashes of indexed field names
}

/// IndexedDB transaction mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdbTransactionMode {
    ReadOnly,
    ReadWrite,
    VersionChange,
}

/// An IndexedDB database
#[derive(Debug, Clone)]
pub struct IdbDatabase {
    pub name_hash: u64,
    pub version: u32,
    pub object_stores: Vec<IdbObjectStore>,
}

// ---------------------------------------------------------------------------
// Full state
// ---------------------------------------------------------------------------

/// Full Web API state
pub struct WebApiState {
    pub fetch_requests: Vec<FetchRequest>,
    pub fetch_responses: Vec<FetchResponse>,
    pub xhr_instances: Vec<XmlHttpRequest>,
    pub websockets: Vec<WebSocket>,
    pub local_storage: Vec<StorageEntry>,
    pub session_storage: Vec<StorageEntry>,
    pub idb_databases: Vec<IdbDatabase>,
    pub next_fetch_id: u32,
    pub next_xhr_id: u32,
    pub next_ws_id: u32,
    pub total_requests: u32,
    pub total_bytes_sent: u64,
    pub total_bytes_received: u64,
}

// ---------------------------------------------------------------------------
// Fetch API functions
// ---------------------------------------------------------------------------

/// Create a fetch request, returns request ID
pub fn fetch_create(method: HttpMethod, url: &[u8], promise_id: u32) -> Option<u32> {
    let mut guard = WEB_API_STATE.lock();
    let state = guard.as_mut()?;

    if state.fetch_requests.iter().filter(|r| r.state == FetchState::Pending || r.state == FetchState::Sent).count() >= MAX_PENDING_REQUESTS {
        serial_println!("    web_api: max pending fetches reached");
        return None;
    }

    let id = state.next_fetch_id;
    state.next_fetch_id = state.next_fetch_id.saturating_add(1);
    state.total_requests = state.total_requests.saturating_add(1);

    state.fetch_requests.push(FetchRequest {
        id,
        method,
        url_hash: web_hash(url),
        url: url.to_vec(),
        headers: Vec::new(),
        body: Vec::new(),
        state: FetchState::Pending,
        promise_id,
        timeout_ms: 30000,
        credentials: false,
        mode: RequestMode::Cors,
    });
    Some(id)
}

/// Add a header to a fetch request
pub fn fetch_set_header(request_id: u32, name: &[u8], value: &[u8]) {
    let mut guard = WEB_API_STATE.lock();
    if let Some(state) = guard.as_mut() {
        if let Some(req) = state.fetch_requests.iter_mut().find(|r| r.id == request_id && r.state == FetchState::Pending) {
            req.headers.push(HttpHeader {
                name_hash: web_hash(name),
                name: name.to_vec(),
                value: value.to_vec(),
            });
        }
    }
}

/// Set the body of a fetch request
pub fn fetch_set_body(request_id: u32, body: &[u8]) {
    let mut guard = WEB_API_STATE.lock();
    if let Some(state) = guard.as_mut() {
        if let Some(req) = state.fetch_requests.iter_mut().find(|r| r.id == request_id && r.state == FetchState::Pending) {
            req.body = body.to_vec();
            state.total_bytes_sent += body.len() as u64;
        }
    }
}

/// Mark a fetch as sent (called by network layer)
pub fn fetch_send(request_id: u32) {
    let mut guard = WEB_API_STATE.lock();
    if let Some(state) = guard.as_mut() {
        if let Some(req) = state.fetch_requests.iter_mut().find(|r| r.id == request_id && r.state == FetchState::Pending) {
            req.state = FetchState::Sent;
        }
    }
}

/// Deliver a response for a fetch request (called by network layer)
pub fn fetch_deliver_response(request_id: u32, status: u16, headers: Vec<HttpHeader>, body: Vec<u8>) {
    let mut guard = WEB_API_STATE.lock();
    if let Some(state) = guard.as_mut() {
        if let Some(req) = state.fetch_requests.iter_mut().find(|r| r.id == request_id) {
            req.state = FetchState::Done;
        }
        state.total_bytes_received += body.len() as u64;
        state.fetch_responses.push(FetchResponse {
            request_id,
            status,
            status_text_hash: 0,
            headers,
            body,
            ok: status >= 200 && status <= 299,
            redirected: false,
            response_type: ResponseType::Basic,
        });
    }
}

/// Abort a fetch request
pub fn fetch_abort(request_id: u32) {
    let mut guard = WEB_API_STATE.lock();
    if let Some(state) = guard.as_mut() {
        if let Some(req) = state.fetch_requests.iter_mut().find(|r| r.id == request_id) {
            req.state = FetchState::Aborted;
        }
    }
}

// ---------------------------------------------------------------------------
// XMLHttpRequest functions
// ---------------------------------------------------------------------------

/// Create a new XHR instance
pub fn xhr_create() -> Option<u32> {
    let mut guard = WEB_API_STATE.lock();
    let state = guard.as_mut()?;

    if state.xhr_instances.len() >= MAX_XHR {
        return None;
    }

    let id = state.next_xhr_id;
    state.next_xhr_id = state.next_xhr_id.saturating_add(1);
    state.xhr_instances.push(XmlHttpRequest {
        id,
        method: HttpMethod::Get,
        url_hash: 0,
        url: Vec::new(),
        ready_state: XhrReadyState::Unsent,
        status: 0,
        response_headers: Vec::new(),
        request_headers: Vec::new(),
        response_body: Vec::new(),
        request_body: Vec::new(),
        async_mode: true,
        timeout_ms: 0,
        onreadystatechange_cb: None,
        onload_cb: None,
        onerror_cb: None,
        sent: false,
    });
    Some(id)
}

/// Open an XHR request
pub fn xhr_open(xhr_id: u32, method: HttpMethod, url: &[u8], async_mode: bool) {
    let mut guard = WEB_API_STATE.lock();
    if let Some(state) = guard.as_mut() {
        if let Some(xhr) = state.xhr_instances.iter_mut().find(|x| x.id == xhr_id) {
            xhr.method = method;
            xhr.url = url.to_vec();
            xhr.url_hash = web_hash(url);
            xhr.async_mode = async_mode;
            xhr.ready_state = XhrReadyState::Opened;
        }
    }
}

/// Send an XHR request
pub fn xhr_send(xhr_id: u32, body: &[u8]) {
    let mut guard = WEB_API_STATE.lock();
    if let Some(state) = guard.as_mut() {
        if let Some(xhr) = state.xhr_instances.iter_mut().find(|x| x.id == xhr_id && x.ready_state == XhrReadyState::Opened) {
            xhr.request_body = body.to_vec();
            xhr.sent = true;
            state.total_bytes_sent += body.len() as u64;
            state.total_requests = state.total_requests.saturating_add(1);
        }
    }
}

// ---------------------------------------------------------------------------
// WebSocket functions
// ---------------------------------------------------------------------------

/// Create a WebSocket connection
pub fn ws_create(url: &[u8]) -> Option<u32> {
    let mut guard = WEB_API_STATE.lock();
    let state = guard.as_mut()?;

    if state.websockets.iter().filter(|w| w.state != WsState::Closed).count() >= MAX_WEBSOCKETS {
        serial_println!("    web_api: max websockets reached");
        return None;
    }

    let id = state.next_ws_id;
    state.next_ws_id = state.next_ws_id.saturating_add(1);
    state.websockets.push(WebSocket {
        id,
        url_hash: web_hash(url),
        url: url.to_vec(),
        state: WsState::Connecting,
        protocol_hash: 0,
        send_queue: Vec::new(),
        recv_queue: Vec::new(),
        onopen_cb: None,
        onmessage_cb: None,
        onclose_cb: None,
        onerror_cb: None,
        buffered_amount: 0,
    });
    Some(id)
}

/// Send a text message on a WebSocket
pub fn ws_send_text(ws_id: u32, data: &[u8]) {
    let mut guard = WEB_API_STATE.lock();
    if let Some(state) = guard.as_mut() {
        if let Some(ws) = state.websockets.iter_mut().find(|w| w.id == ws_id && w.state == WsState::Open) {
            let len = data.len() as u32;
            ws.send_queue.push(WsMessage {
                msg_type: WsMessageType::Text,
                data: data.to_vec(),
            });
            ws.buffered_amount += len;
            state.total_bytes_sent += len as u64;
        }
    }
}

/// Send binary data on a WebSocket
pub fn ws_send_binary(ws_id: u32, data: &[u8]) {
    let mut guard = WEB_API_STATE.lock();
    if let Some(state) = guard.as_mut() {
        if let Some(ws) = state.websockets.iter_mut().find(|w| w.id == ws_id && w.state == WsState::Open) {
            let len = data.len() as u32;
            ws.send_queue.push(WsMessage {
                msg_type: WsMessageType::Binary,
                data: data.to_vec(),
            });
            ws.buffered_amount += len;
            state.total_bytes_sent += len as u64;
        }
    }
}

/// Close a WebSocket
pub fn ws_close(ws_id: u32) {
    let mut guard = WEB_API_STATE.lock();
    if let Some(state) = guard.as_mut() {
        if let Some(ws) = state.websockets.iter_mut().find(|w| w.id == ws_id) {
            ws.state = WsState::Closing;
        }
    }
}

/// Deliver a received WebSocket message (called by network layer)
pub fn ws_deliver_message(ws_id: u32, msg: WsMessage) {
    let mut guard = WEB_API_STATE.lock();
    if let Some(state) = guard.as_mut() {
        state.total_bytes_received += msg.data.len() as u64;
        if let Some(ws) = state.websockets.iter_mut().find(|w| w.id == ws_id && w.state == WsState::Open) {
            ws.recv_queue.push(msg);
        }
    }
}

// ---------------------------------------------------------------------------
// localStorage / sessionStorage
// ---------------------------------------------------------------------------

/// Set a storage item
pub fn storage_set(kind: StorageKind, key: &[u8], value: &[u8]) -> bool {
    let mut guard = WEB_API_STATE.lock();
    if let Some(state) = guard.as_mut() {
        if value.len() > MAX_STORAGE_VALUE_SIZE {
            serial_println!("    web_api: storage value too large");
            return false;
        }

        let storage = match kind {
            StorageKind::Local => &mut state.local_storage,
            StorageKind::Session => &mut state.session_storage,
        };
        let max = match kind {
            StorageKind::Local => MAX_LOCAL_STORAGE,
            StorageKind::Session => MAX_SESSION_STORAGE,
        };

        let key_hash = web_hash(key);

        // Update existing entry
        if let Some(entry) = storage.iter_mut().find(|e| e.key_hash == key_hash) {
            entry.value = value.to_vec();
            return true;
        }

        if storage.len() >= max {
            serial_println!("    web_api: storage full");
            return false;
        }

        storage.push(StorageEntry {
            key_hash,
            key: key.to_vec(),
            value: value.to_vec(),
        });
        true
    } else {
        false
    }
}

/// Get a storage item (returns value bytes or None)
pub fn storage_get(kind: StorageKind, key: &[u8]) -> Option<Vec<u8>> {
    let guard = WEB_API_STATE.lock();
    let state = guard.as_ref()?;
    let storage = match kind {
        StorageKind::Local => &state.local_storage,
        StorageKind::Session => &state.session_storage,
    };
    let key_hash = web_hash(key);
    storage.iter().find(|e| e.key_hash == key_hash).map(|e| e.value.clone())
}

/// Remove a storage item
pub fn storage_remove(kind: StorageKind, key: &[u8]) {
    let mut guard = WEB_API_STATE.lock();
    if let Some(state) = guard.as_mut() {
        let storage = match kind {
            StorageKind::Local => &mut state.local_storage,
            StorageKind::Session => &mut state.session_storage,
        };
        let key_hash = web_hash(key);
        storage.retain(|e| e.key_hash != key_hash);
    }
}

/// Clear all entries in a storage area
pub fn storage_clear(kind: StorageKind) {
    let mut guard = WEB_API_STATE.lock();
    if let Some(state) = guard.as_mut() {
        match kind {
            StorageKind::Local => state.local_storage.clear(),
            StorageKind::Session => state.session_storage.clear(),
        }
    }
}

/// Get the number of entries in a storage area
pub fn storage_length(kind: StorageKind) -> usize {
    let guard = WEB_API_STATE.lock();
    if let Some(state) = guard.as_ref() {
        match kind {
            StorageKind::Local => state.local_storage.len(),
            StorageKind::Session => state.session_storage.len(),
        }
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// IndexedDB Lite
// ---------------------------------------------------------------------------

/// Open or create an IndexedDB database
pub fn idb_open(name: &[u8], version: u32) -> Option<u64> {
    let mut guard = WEB_API_STATE.lock();
    let state = guard.as_mut()?;
    let name_hash = web_hash(name);

    if let Some(db) = state.idb_databases.iter_mut().find(|d| d.name_hash == name_hash) {
        db.version = version;
        return Some(name_hash);
    }

    state.idb_databases.push(IdbDatabase {
        name_hash,
        version,
        object_stores: Vec::new(),
    });
    Some(name_hash)
}

/// Create an object store in a database
pub fn idb_create_store(db_hash: u64, store_name: &[u8], auto_increment: bool) -> bool {
    let mut guard = WEB_API_STATE.lock();
    if let Some(state) = guard.as_mut() {
        if let Some(db) = state.idb_databases.iter_mut().find(|d| d.name_hash == db_hash) {
            if db.object_stores.len() >= MAX_OBJECT_STORES {
                return false;
            }
            let store_hash = web_hash(store_name);
            if db.object_stores.iter().any(|s| s.name_hash == store_hash) {
                return false; // already exists
            }
            db.object_stores.push(IdbObjectStore {
                name_hash: store_hash,
                name: store_name.to_vec(),
                records: Vec::new(),
                auto_increment,
                next_key: 1,
                index_hashes: Vec::new(),
            });
            return true;
        }
    }
    false
}

/// Put a record into an object store (auto-increment key), returns key
pub fn idb_put(db_hash: u64, store_hash: u64, value: &[u8]) -> Option<i32> {
    let mut guard = WEB_API_STATE.lock();
    let state = guard.as_mut()?;
    let db = state.idb_databases.iter_mut().find(|d| d.name_hash == db_hash)?;
    let store = db.object_stores.iter_mut().find(|s| s.name_hash == store_hash)?;

    if store.records.iter().filter(|r| r.alive).count() >= MAX_RECORDS_PER_STORE {
        serial_println!("    web_api: idb store full");
        return None;
    }

    let key = store.next_key;
    store.next_key = store.next_key.saturating_add(1);
    store.records.push(IdbRecord {
        key,
        key_hash: 0,
        value: value.to_vec(),
        alive: true,
    });
    Some(key)
}

/// Get a record by integer key
pub fn idb_get(db_hash: u64, store_hash: u64, key: i32) -> Option<Vec<u8>> {
    let guard = WEB_API_STATE.lock();
    let state = guard.as_ref()?;
    let db = state.idb_databases.iter().find(|d| d.name_hash == db_hash)?;
    let store = db.object_stores.iter().find(|s| s.name_hash == store_hash)?;
    store.records.iter().find(|r| r.key == key && r.alive).map(|r| r.value.clone())
}

/// Delete a record by integer key
pub fn idb_delete(db_hash: u64, store_hash: u64, key: i32) -> bool {
    let mut guard = WEB_API_STATE.lock();
    if let Some(state) = guard.as_mut() {
        if let Some(db) = state.idb_databases.iter_mut().find(|d| d.name_hash == db_hash) {
            if let Some(store) = db.object_stores.iter_mut().find(|s| s.name_hash == store_hash) {
                if let Some(rec) = store.records.iter_mut().find(|r| r.key == key && r.alive) {
                    rec.alive = false;
                    return true;
                }
            }
        }
    }
    false
}

/// Count alive records in a store
pub fn idb_count(db_hash: u64, store_hash: u64) -> usize {
    let guard = WEB_API_STATE.lock();
    if let Some(state) = guard.as_ref() {
        if let Some(db) = state.idb_databases.iter().find(|d| d.name_hash == db_hash) {
            if let Some(store) = db.object_stores.iter().find(|s| s.name_hash == store_hash) {
                return store.records.iter().filter(|r| r.alive).count();
            }
        }
    }
    0
}

/// Clear all records in an object store
pub fn idb_clear_store(db_hash: u64, store_hash: u64) {
    let mut guard = WEB_API_STATE.lock();
    if let Some(state) = guard.as_mut() {
        if let Some(db) = state.idb_databases.iter_mut().find(|d| d.name_hash == db_hash) {
            if let Some(store) = db.object_stores.iter_mut().find(|s| s.name_hash == store_hash) {
                store.records.clear();
                store.next_key = 1;
            }
        }
    }
}

/// Initialize the Web API subsystem
pub fn init() {
    let mut guard = WEB_API_STATE.lock();
    *guard = Some(WebApiState {
        fetch_requests: Vec::new(),
        fetch_responses: Vec::new(),
        xhr_instances: Vec::new(),
        websockets: Vec::new(),
        local_storage: Vec::new(),
        session_storage: Vec::new(),
        idb_databases: Vec::new(),
        next_fetch_id: 1,
        next_xhr_id: 1,
        next_ws_id: 1,
        total_requests: 0,
        total_bytes_sent: 0,
        total_bytes_received: 0,
    });
    serial_println!("    browser::web_api initialized");
}
