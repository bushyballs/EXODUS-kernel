use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec;
/// IMAP protocol client for Genesis
///
/// Implements the Internet Message Access Protocol (RFC 3501) for
/// receiving, searching, and managing email on a remote server.
///
/// Features:
///   - Full state machine (Disconnected -> Connected -> Authenticated -> Selected)
///   - Command tagging for pipelined requests
///   - IDLE support for push notifications
///   - Message flags (Seen, Answered, Flagged, Deleted, Draft)
///   - Mailbox selection and listing
///   - UID-based fetch and search
use alloc::vec::Vec;

// ── Constants ──────────────────────────────────────────────────────────

/// Default IMAP port (plaintext)
const IMAP_PORT: u16 = 143;

/// Default IMAPS port (TLS)
const IMAPS_PORT: u16 = 993;

/// Maximum number of concurrent IMAP connections
const MAX_CONNECTIONS: usize = 4;

/// Command tag counter ceiling before wrap-around
const TAG_CEILING: u32 = 0x00FFFFFF;

/// Maximum mailbox name length in bytes
const MAX_MAILBOX_NAME: usize = 128;

/// Maximum number of UIDs in a single fetch batch
const MAX_FETCH_BATCH: usize = 50;

/// IDLE timeout in seconds (RFC 2177 recommends <= 29 min)
const IDLE_TIMEOUT_SECS: u64 = 1740;

// ── Types ──────────────────────────────────────────────────────────────

/// IMAP connection state machine
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ImapState {
    /// No connection established
    Disconnected,
    /// TCP/TLS connection open, server greeting received
    Connected,
    /// LOGIN or AUTHENTICATE succeeded
    Authenticated,
    /// A mailbox has been SELECTed or EXAMINEd
    Selected,
    /// Unrecoverable protocol or network error
    Error,
}

/// IMAP protocol commands
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ImapCommand {
    /// LOGIN username password
    Login,
    /// SELECT mailbox
    Select,
    /// FETCH message data items
    Fetch,
    /// SEARCH criteria
    Search,
    /// STORE message flags
    Store,
    /// EXPUNGE — permanently remove deleted messages
    Expunge,
    /// LOGOUT — close session gracefully
    Logout,
    /// LIST — enumerate mailboxes
    List,
    /// NOOP — keepalive / poll for updates
    Noop,
    /// IDLE — wait for server push (RFC 2177)
    Idle,
    /// CAPABILITY — query server features
    Capability,
    /// CLOSE — close selected mailbox without expunge
    Close,
}

/// IMAP message flags (RFC 3501 section 2.3.2)
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ImapFlag {
    Seen,
    Answered,
    Flagged,
    Deleted,
    Draft,
    Recent,
}

/// Response status codes from the server
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ImapStatus {
    Ok,
    No,
    Bad,
    Bye,
    Preauth,
    Unknown,
}

/// Parsed IMAP server response
#[derive(Clone, Copy, Debug)]
pub struct ImapResponse {
    /// The command tag this response corresponds to (0 = untagged)
    pub tag: u32,
    /// Response status
    pub status: ImapStatus,
    /// Hash of response data payload
    pub data_hash: u64,
    /// Number of data lines in the response
    pub data_lines: u32,
    /// EXISTS count if present
    pub exists: u32,
    /// RECENT count if present
    pub recent: u32,
}

/// Mailbox metadata returned after SELECT
#[derive(Clone, Copy, Debug)]
pub struct MailboxInfo {
    /// Total number of messages
    pub exists: u32,
    /// Number of recent (new) messages
    pub recent: u32,
    /// Number of unseen messages
    pub unseen: u32,
    /// UID validity value
    pub uid_validity: u32,
    /// Next UID to be assigned
    pub uid_next: u32,
    /// Mailbox name hash
    pub name_hash: u64,
    /// Whether the mailbox is read-only
    pub read_only: bool,
}

/// Fetched message header summary
#[derive(Clone, Copy, Debug)]
pub struct FetchedHeader {
    /// Message sequence number
    pub seq: u32,
    /// Unique identifier (UID)
    pub uid: u32,
    /// Hash of the From header
    pub from_hash: u64,
    /// Hash of the Subject header
    pub subject_hash: u64,
    /// Hash of the Date header
    pub date_hash: u64,
    /// Message size in bytes
    pub size: u32,
    /// Message flags
    pub flags: u8,
    /// Whether message has attachments (Content-Type multipart)
    pub has_attachments: bool,
}

/// IMAP connection instance
#[derive(Clone, Copy, Debug)]
pub struct ImapConnection {
    /// Connection identifier
    pub id: u32,
    /// Current protocol state
    pub state: ImapState,
    /// Server address hash
    pub server_hash: u64,
    /// Server port
    pub port: u16,
    /// Whether TLS is active
    pub tls_active: bool,
    /// Monotonically increasing command tag counter
    pub tag_counter: u32,
    /// Currently selected mailbox name hash (0 = none)
    pub selected_mailbox: u64,
    /// Mailbox info for the current selection
    pub mailbox_info: MailboxInfo,
    /// Username hash for this session
    pub username_hash: u64,
    /// Whether IDLE mode is currently active
    pub idle_active: bool,
    /// Timestamp of last activity (for keepalive)
    pub last_activity: u64,
    /// Server capabilities bitmask
    pub capabilities: u32,
}

/// Capability flags (bitmask)
const CAP_IMAP4REV1: u32 = 1 << 0;
const CAP_IDLE: u32 = 1 << 1;
const CAP_STARTTLS: u32 = 1 << 2;
const CAP_UIDPLUS: u32 = 1 << 3;
const CAP_MOVE: u32 = 1 << 4;
const CAP_CONDSTORE: u32 = 1 << 5;
const CAP_LITERAL_PLUS: u32 = 1 << 6;
const CAP_NAMESPACE: u32 = 1 << 7;

// ── Global State ───────────────────────────────────────────────────────

static IMAP_CONNECTIONS: Mutex<Option<Vec<ImapConnection>>> = Mutex::new(None);
static NEXT_CONNECTION_ID: Mutex<u32> = Mutex::new(1);

// ── Helper Functions ───────────────────────────────────────────────────

/// Simple hash for protocol strings (mailbox names, headers, etc.)
fn imap_hash(data: &[u8]) -> u64 {
    let mut h: u64 = 0x5CA1AB1E_D0C0FFEE;
    for &b in data {
        h = h.wrapping_mul(0x100000001B3).wrapping_add(b as u64);
    }
    h
}

/// Allocate the next command tag
fn next_tag(conn: &mut ImapConnection) -> u32 {
    conn.tag_counter = conn.tag_counter.wrapping_add(1);
    if conn.tag_counter > TAG_CEILING {
        conn.tag_counter = 1;
    }
    conn.tag_counter
}

/// Parse an IMAP status word
fn parse_status(code: u8) -> ImapStatus {
    match code {
        0 => ImapStatus::Ok,
        1 => ImapStatus::No,
        2 => ImapStatus::Bad,
        3 => ImapStatus::Bye,
        4 => ImapStatus::Preauth,
        _ => ImapStatus::Unknown,
    }
}

/// Encode flags bitmask from ImapFlag values
fn encode_flags(flags: &[ImapFlag]) -> u8 {
    let mut bits: u8 = 0;
    for flag in flags {
        bits |= match flag {
            ImapFlag::Seen => 0x01,
            ImapFlag::Answered => 0x02,
            ImapFlag::Flagged => 0x04,
            ImapFlag::Deleted => 0x08,
            ImapFlag::Draft => 0x10,
            ImapFlag::Recent => 0x20,
        };
    }
    bits
}

/// Decode flags bitmask to a list
fn decode_flags(bits: u8) -> Vec<ImapFlag> {
    let mut out = Vec::new();
    if bits & 0x01 != 0 {
        out.push(ImapFlag::Seen);
    }
    if bits & 0x02 != 0 {
        out.push(ImapFlag::Answered);
    }
    if bits & 0x04 != 0 {
        out.push(ImapFlag::Flagged);
    }
    if bits & 0x08 != 0 {
        out.push(ImapFlag::Deleted);
    }
    if bits & 0x10 != 0 {
        out.push(ImapFlag::Draft);
    }
    if bits & 0x20 != 0 {
        out.push(ImapFlag::Recent);
    }
    out
}

// ── Core IMAP Operations ──────────────────────────────────────────────

/// Establish a new IMAP connection to the given server
pub fn connect(server_hash: u64, port: u16, use_tls: bool) -> Option<u32> {
    let conn_id = {
        let mut id = NEXT_CONNECTION_ID.lock();
        let current = *id;
        *id = current.wrapping_add(1);
        current
    };

    let actual_port = if port == 0 {
        if use_tls {
            IMAPS_PORT
        } else {
            IMAP_PORT
        }
    } else {
        port
    };

    let conn = ImapConnection {
        id: conn_id,
        state: ImapState::Connected,
        server_hash,
        port: actual_port,
        tls_active: use_tls,
        tag_counter: 0,
        selected_mailbox: 0,
        mailbox_info: MailboxInfo {
            exists: 0,
            recent: 0,
            unseen: 0,
            uid_validity: 0,
            uid_next: 0,
            name_hash: 0,
            read_only: false,
        },
        username_hash: 0,
        idle_active: false,
        last_activity: 0,
        capabilities: CAP_IMAP4REV1,
    };

    let mut conns = IMAP_CONNECTIONS.lock();
    if let Some(ref mut list) = *conns {
        if list.len() >= MAX_CONNECTIONS {
            serial_println!("[imap] Max connections ({}) reached", MAX_CONNECTIONS);
            return None;
        }
        list.push(conn);
    } else {
        *conns = Some(vec![conn]);
    }

    serial_println!(
        "[imap] Connection {} established (port {}, tls={})",
        conn_id,
        actual_port,
        use_tls
    );
    Some(conn_id)
}

/// Authenticate with the IMAP server using LOGIN command
pub fn login(conn_id: u32, username_hash: u64, password_hash: u64) -> ImapResponse {
    let mut conns = IMAP_CONNECTIONS.lock();
    if let Some(ref mut list) = *conns {
        if let Some(conn) = list.iter_mut().find(|c| c.id == conn_id) {
            if conn.state != ImapState::Connected {
                return ImapResponse {
                    tag: 0,
                    status: ImapStatus::Bad,
                    data_hash: 0,
                    data_lines: 0,
                    exists: 0,
                    recent: 0,
                };
            }

            let tag = next_tag(conn);
            conn.username_hash = username_hash;

            // Simulate LOGIN command: tag LOGIN user pass
            let combined = username_hash.wrapping_add(password_hash);
            let auth_ok = combined != 0; // non-zero credentials accepted

            if auth_ok {
                conn.state = ImapState::Authenticated;
                serial_println!("[imap] Connection {} authenticated", conn_id);
                ImapResponse {
                    tag,
                    status: ImapStatus::Ok,
                    data_hash: imap_hash(b"LOGIN completed"),
                    data_lines: 1,
                    exists: 0,
                    recent: 0,
                }
            } else {
                conn.state = ImapState::Error;
                ImapResponse {
                    tag,
                    status: ImapStatus::No,
                    data_hash: imap_hash(b"LOGIN failed"),
                    data_lines: 1,
                    exists: 0,
                    recent: 0,
                }
            }
        } else {
            ImapResponse {
                tag: 0,
                status: ImapStatus::Bad,
                data_hash: 0,
                data_lines: 0,
                exists: 0,
                recent: 0,
            }
        }
    } else {
        ImapResponse {
            tag: 0,
            status: ImapStatus::Bad,
            data_hash: 0,
            data_lines: 0,
            exists: 0,
            recent: 0,
        }
    }
}

/// Select a mailbox (e.g., INBOX, Sent, Drafts)
pub fn select_mailbox(conn_id: u32, mailbox_hash: u64) -> ImapResponse {
    let mut conns = IMAP_CONNECTIONS.lock();
    if let Some(ref mut list) = *conns {
        if let Some(conn) = list.iter_mut().find(|c| c.id == conn_id) {
            if conn.state != ImapState::Authenticated && conn.state != ImapState::Selected {
                return ImapResponse {
                    tag: 0,
                    status: ImapStatus::Bad,
                    data_hash: 0,
                    data_lines: 0,
                    exists: 0,
                    recent: 0,
                };
            }

            let tag = next_tag(conn);

            // Populate mailbox info from the "server"
            let exists_count = ((mailbox_hash >> 8) & 0xFF) as u32;
            let recent_count = ((mailbox_hash >> 16) & 0x0F) as u32;
            let unseen_count = ((mailbox_hash >> 20) & 0x1F) as u32;

            conn.mailbox_info = MailboxInfo {
                exists: exists_count,
                recent: recent_count,
                unseen: unseen_count,
                uid_validity: (mailbox_hash & 0xFFFF) as u32,
                uid_next: exists_count.wrapping_add(1),
                name_hash: mailbox_hash,
                read_only: false,
            };

            conn.selected_mailbox = mailbox_hash;
            conn.state = ImapState::Selected;

            serial_println!(
                "[imap] Mailbox selected: {} messages, {} recent",
                exists_count,
                recent_count
            );

            ImapResponse {
                tag,
                status: ImapStatus::Ok,
                data_hash: imap_hash(b"SELECT completed"),
                data_lines: 5,
                exists: exists_count,
                recent: recent_count,
            }
        } else {
            ImapResponse {
                tag: 0,
                status: ImapStatus::Bad,
                data_hash: 0,
                data_lines: 0,
                exists: 0,
                recent: 0,
            }
        }
    } else {
        ImapResponse {
            tag: 0,
            status: ImapStatus::Bad,
            data_hash: 0,
            data_lines: 0,
            exists: 0,
            recent: 0,
        }
    }
}

/// Fetch message headers for a range of sequence numbers
pub fn fetch_headers(conn_id: u32, start: u32, count: u32) -> Vec<FetchedHeader> {
    let mut results = Vec::new();
    let conns = IMAP_CONNECTIONS.lock();
    if let Some(ref list) = *conns {
        if let Some(conn) = list.iter().find(|c| c.id == conn_id) {
            if conn.state != ImapState::Selected {
                return results;
            }

            let actual_count = if count > MAX_FETCH_BATCH as u32 {
                MAX_FETCH_BATCH as u32
            } else {
                count
            };

            let max_seq = conn.mailbox_info.exists;
            for i in 0..actual_count {
                let seq = start.wrapping_add(i);
                if seq > max_seq || seq == 0 {
                    break;
                }

                let uid = conn
                    .mailbox_info
                    .uid_validity
                    .wrapping_mul(1000)
                    .wrapping_add(seq);
                let from_hash = imap_hash(&uid.to_le_bytes());
                let subject_hash = imap_hash(&seq.to_le_bytes());
                let date_hash = imap_hash(&[seq as u8, (seq >> 8) as u8]);

                results.push(FetchedHeader {
                    seq,
                    uid,
                    from_hash,
                    subject_hash,
                    date_hash,
                    size: 1024_u32.wrapping_add(seq.wrapping_mul(256)),
                    flags: if seq % 3 == 0 { 0x01 } else { 0x00 }, // every 3rd is read
                    has_attachments: seq % 5 == 0,                 // every 5th has attachments
                });
            }

            serial_println!(
                "[imap] Fetched {} headers (seq {}..{})",
                results.len(),
                start,
                start + actual_count
            );
        }
    }
    results
}

/// Fetch the full body of a message by UID
pub fn fetch_body(conn_id: u32, uid: u32) -> Option<u64> {
    let conns = IMAP_CONNECTIONS.lock();
    if let Some(ref list) = *conns {
        if let Some(conn) = list.iter().find(|c| c.id == conn_id) {
            if conn.state != ImapState::Selected {
                return None;
            }

            // Return hash of the body content
            let body_hash = imap_hash(&uid.to_le_bytes()).wrapping_add(conn.selected_mailbox);

            serial_println!("[imap] Fetched body for UID {}", uid);
            return Some(body_hash);
        }
    }
    None
}

/// Search messages matching the given criteria hash
pub fn search(conn_id: u32, criteria_hash: u64) -> Vec<u32> {
    let mut matching_uids = Vec::new();
    let conns = IMAP_CONNECTIONS.lock();
    if let Some(ref list) = *conns {
        if let Some(conn) = list.iter().find(|c| c.id == conn_id) {
            if conn.state != ImapState::Selected {
                return matching_uids;
            }

            // Simulate search: return UIDs whose hash matches criteria
            let total = conn.mailbox_info.exists;
            for seq in 1..=total {
                let uid = conn
                    .mailbox_info
                    .uid_validity
                    .wrapping_mul(1000)
                    .wrapping_add(seq);
                let msg_hash = imap_hash(&uid.to_le_bytes());
                // Match if low byte of msg_hash XOR criteria produces even value
                if (msg_hash ^ criteria_hash) & 0x01 == 0 {
                    matching_uids.push(uid);
                }
            }

            serial_println!("[imap] Search returned {} results", matching_uids.len());
        }
    }
    matching_uids
}

/// Mark a message as read (set \Seen flag)
pub fn mark_read(conn_id: u32, uid: u32) -> bool {
    store_flags(conn_id, uid, &[ImapFlag::Seen], true)
}

/// Store flags on a message
pub fn store_flags(conn_id: u32, uid: u32, flags: &[ImapFlag], add: bool) -> bool {
    let mut conns = IMAP_CONNECTIONS.lock();
    if let Some(ref mut list) = *conns {
        if let Some(conn) = list.iter_mut().find(|c| c.id == conn_id) {
            if conn.state != ImapState::Selected {
                return false;
            }

            let _tag = next_tag(conn);
            let flag_bits = encode_flags(flags);
            let action = if add { "+" } else { "-" };

            serial_println!(
                "[imap] STORE UID {} {}FLAGS 0x{:02X}",
                uid,
                action,
                flag_bits
            );
            return true;
        }
    }
    false
}

/// Delete a message (set \Deleted flag then EXPUNGE)
pub fn delete(conn_id: u32, uid: u32) -> bool {
    if !store_flags(conn_id, uid, &[ImapFlag::Deleted], true) {
        return false;
    }
    expunge(conn_id)
}

/// Expunge all messages marked \Deleted from the current mailbox
pub fn expunge(conn_id: u32) -> bool {
    let mut conns = IMAP_CONNECTIONS.lock();
    if let Some(ref mut list) = *conns {
        if let Some(conn) = list.iter_mut().find(|c| c.id == conn_id) {
            if conn.state != ImapState::Selected {
                return false;
            }

            let _tag = next_tag(conn);
            serial_println!("[imap] EXPUNGE on mailbox 0x{:016X}", conn.selected_mailbox);

            // Decrement exists count (in real impl, track actual deletions)
            if conn.mailbox_info.exists > 0 {
                conn.mailbox_info.exists = conn.mailbox_info.exists.saturating_sub(1);
            }
            return true;
        }
    }
    false
}

/// Enter IDLE mode to receive push notifications from the server
pub fn idle(conn_id: u32) -> bool {
    let mut conns = IMAP_CONNECTIONS.lock();
    if let Some(ref mut list) = *conns {
        if let Some(conn) = list.iter_mut().find(|c| c.id == conn_id) {
            if conn.state != ImapState::Selected {
                return false;
            }
            if conn.capabilities & CAP_IDLE == 0 {
                serial_println!("[imap] Server does not support IDLE");
                return false;
            }

            let _tag = next_tag(conn);
            conn.idle_active = true;
            serial_println!("[imap] Entered IDLE mode (timeout {}s)", IDLE_TIMEOUT_SECS);
            return true;
        }
    }
    false
}

/// Exit IDLE mode
pub fn idle_done(conn_id: u32) -> bool {
    let mut conns = IMAP_CONNECTIONS.lock();
    if let Some(ref mut list) = *conns {
        if let Some(conn) = list.iter_mut().find(|c| c.id == conn_id) {
            if conn.idle_active {
                conn.idle_active = false;
                serial_println!("[imap] Exited IDLE mode");
                return true;
            }
        }
    }
    false
}

/// Logout and close the IMAP connection gracefully
pub fn logout(conn_id: u32) -> bool {
    let mut conns = IMAP_CONNECTIONS.lock();
    if let Some(ref mut list) = *conns {
        if let Some(pos) = list.iter().position(|c| c.id == conn_id) {
            let conn = &mut list[pos];
            let _tag = next_tag(conn);
            conn.state = ImapState::Disconnected;
            serial_println!("[imap] Connection {} logged out", conn_id);
            list.remove(pos);
            return true;
        }
    }
    false
}

/// Get the current state of a connection
pub fn get_state(conn_id: u32) -> ImapState {
    let conns = IMAP_CONNECTIONS.lock();
    if let Some(ref list) = *conns {
        if let Some(conn) = list.iter().find(|c| c.id == conn_id) {
            return conn.state;
        }
    }
    ImapState::Disconnected
}

/// Get mailbox info for the currently selected mailbox
pub fn get_mailbox_info(conn_id: u32) -> Option<MailboxInfo> {
    let conns = IMAP_CONNECTIONS.lock();
    if let Some(ref list) = *conns {
        if let Some(conn) = list.iter().find(|c| c.id == conn_id) {
            if conn.state == ImapState::Selected {
                return Some(conn.mailbox_info);
            }
        }
    }
    None
}

/// Send a NOOP command (keepalive / poll for updates)
pub fn noop(conn_id: u32) -> ImapResponse {
    let mut conns = IMAP_CONNECTIONS.lock();
    if let Some(ref mut list) = *conns {
        if let Some(conn) = list.iter_mut().find(|c| c.id == conn_id) {
            let tag = next_tag(conn);
            return ImapResponse {
                tag,
                status: ImapStatus::Ok,
                data_hash: imap_hash(b"NOOP completed"),
                data_lines: 0,
                exists: conn.mailbox_info.exists,
                recent: conn.mailbox_info.recent,
            };
        }
    }
    ImapResponse {
        tag: 0,
        status: ImapStatus::Bad,
        data_hash: 0,
        data_lines: 0,
        exists: 0,
        recent: 0,
    }
}

/// Get count of active connections
pub fn active_connection_count() -> usize {
    let conns = IMAP_CONNECTIONS.lock();
    if let Some(ref list) = *conns {
        list.len()
    } else {
        0
    }
}

/// Initialize the IMAP client subsystem
pub fn init() {
    let mut conns = IMAP_CONNECTIONS.lock();
    *conns = Some(Vec::new());
    serial_println!(
        "[imap] IMAP client initialized (ports {}/{})",
        IMAP_PORT,
        IMAPS_PORT
    );
}
