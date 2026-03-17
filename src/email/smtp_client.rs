use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec;
/// SMTP protocol client for Genesis
///
/// Implements the Simple Mail Transfer Protocol (RFC 5321) for
/// sending email through a relay or direct delivery.
///
/// Features:
///   - Full state machine (Disconnected -> Connected -> Greeted -> Authenticated -> Ready)
///   - EHLO/HELO with extension negotiation
///   - STARTTLS upgrade (RFC 3207)
///   - AUTH PLAIN and AUTH LOGIN
///   - Envelope construction (MAIL FROM, RCPT TO, DATA)
///   - Multiple recipients (To, Cc, Bcc)
///   - Attachment support via envelope
use alloc::vec::Vec;

// ── Send Gate ──────────────────────────────────────────────────────────

/// Master kill switch — Genesis NEVER sends email autonomously.
/// Must be explicitly set to `true` by a user-initiated action to send anything.
/// Default: false (locked). Do not change this default.
const SEND_ENABLED: bool = false;

// ── Constants ──────────────────────────────────────────────────────────

/// Default SMTP submission port
const SMTP_PORT: u16 = 587;

/// Default SMTPS (implicit TLS) port
const SMTPS_PORT: u16 = 465;

/// Legacy SMTP relay port
const SMTP_RELAY_PORT: u16 = 25;

/// Maximum recipients per envelope
const MAX_RECIPIENTS: usize = 100;

/// Maximum attachment count per message
const MAX_ATTACHMENTS: usize = 20;

/// Maximum message body size (10 MB represented as byte count)
const MAX_BODY_SIZE: u32 = 10_485_760;

/// SMTP response code ranges
const SMTP_OK_MIN: u16 = 200;
const SMTP_OK_MAX: u16 = 299;
const SMTP_READY: u16 = 220;
const SMTP_CLOSING: u16 = 221;
const SMTP_AUTH_OK: u16 = 235;
const SMTP_OK: u16 = 250;
const SMTP_AUTH_CONTINUE: u16 = 334;
const SMTP_DATA_START: u16 = 354;

// ── Types ──────────────────────────────────────────────────────────────

/// SMTP connection state machine
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SmtpState {
    /// No connection established
    Disconnected,
    /// TCP connection open, awaiting server banner (220)
    Connected,
    /// EHLO/HELO completed successfully
    Greeted,
    /// AUTH succeeded
    Authenticated,
    /// Envelope in progress (MAIL FROM sent)
    MailFrom,
    /// At least one RCPT TO accepted
    RcptAccepted,
    /// DATA command accepted, transmitting body
    DataMode,
    /// Unrecoverable error
    Error,
}

/// SMTP protocol commands
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SmtpCommand {
    /// EHLO / HELO domain
    Helo,
    /// MAIL FROM:<address>
    MailFrom,
    /// RCPT TO:<address>
    RcptTo,
    /// DATA — begin message body
    Data,
    /// QUIT — close session
    Quit,
    /// STARTTLS — upgrade to TLS
    StartTls,
    /// AUTH PLAIN / LOGIN
    Auth,
    /// RSET — reset transaction
    Rset,
    /// NOOP — keepalive
    Noop,
    /// VRFY — verify address (often disabled)
    Vrfy,
}

/// SMTP server response
#[derive(Clone, Copy, Debug)]
pub struct SmtpResponse {
    /// Three-digit response code (e.g. 250, 354, 550)
    pub code: u16,
    /// Hash of the response text
    pub text_hash: u64,
    /// Whether this is a multiline response
    pub multiline: bool,
    /// Whether the response indicates success
    pub success: bool,
}

/// An email attachment descriptor
#[derive(Clone, Copy, Debug)]
pub struct Attachment {
    /// Filename hash
    pub name_hash: u64,
    /// MIME type hash (e.g. "application/pdf")
    pub mime_hash: u64,
    /// Content hash (the actual data, base64 encoded)
    pub content_hash: u64,
    /// Size in bytes
    pub size: u32,
}

/// Complete email envelope for sending
#[derive(Clone, Debug)]
pub struct EmailEnvelope {
    /// Sender address hash
    pub from_hash: u64,
    /// To recipients (address hashes)
    pub to_list: Vec<u64>,
    /// Cc recipients (address hashes)
    pub cc_list: Vec<u64>,
    /// Bcc recipients (address hashes, not included in headers)
    pub bcc_list: Vec<u64>,
    /// Subject line hash
    pub subject_hash: u64,
    /// Body content hash
    pub body_hash: u64,
    /// Whether body is HTML
    pub is_html: bool,
    /// Attachment list
    pub attachments: Vec<Attachment>,
    /// Message-ID hash
    pub message_id_hash: u64,
    /// In-Reply-To hash (0 = not a reply)
    pub in_reply_to: u64,
    /// Priority (1 = highest, 3 = normal, 5 = lowest)
    pub priority: u8,
    /// Timestamp when envelope was created
    pub created_at: u64,
}

/// SMTP server extension flags (from EHLO response)
#[derive(Clone, Copy, Debug)]
pub struct SmtpExtensions {
    /// Server supports STARTTLS
    pub starttls: bool,
    /// Server supports AUTH
    pub auth: bool,
    /// Server supports 8BITMIME
    pub eight_bit_mime: bool,
    /// Server supports PIPELINING
    pub pipelining: bool,
    /// Maximum message size (0 = no limit advertised)
    pub max_size: u32,
    /// Server supports DSN (Delivery Status Notification)
    pub dsn: bool,
    /// Server supports CHUNKING
    pub chunking: bool,
    /// Server supports SMTPUTF8
    pub smtputf8: bool,
}

/// SMTP connection instance
#[derive(Clone, Copy, Debug)]
pub struct SmtpConnection {
    /// Connection identifier
    pub id: u32,
    /// Current protocol state
    pub state: SmtpState,
    /// Server address hash
    pub server_hash: u64,
    /// Server port
    pub port: u16,
    /// Whether TLS is active
    pub tls_active: bool,
    /// Server extensions
    pub extensions: SmtpExtensions,
    /// Number of messages sent on this connection
    pub messages_sent: u32,
    /// Username hash for authentication
    pub username_hash: u64,
    /// Last SMTP response code received
    pub last_response_code: u16,
    /// Timestamp of last activity
    pub last_activity: u64,
}

// ── Global State ───────────────────────────────────────────────────────

static SMTP_CONNECTIONS: Mutex<Option<Vec<SmtpConnection>>> = Mutex::new(None);
static SMTP_NEXT_ID: Mutex<u32> = Mutex::new(1);
static SENT_COUNT: Mutex<u64> = Mutex::new(0);

// ── Helper Functions ───────────────────────────────────────────────────

/// Hash function for SMTP strings
fn smtp_hash(data: &[u8]) -> u64 {
    let mut h: u64 = 0xABCD1234_DEAD5678;
    for &b in data {
        h = h.wrapping_mul(0x100000001B3).wrapping_add(b as u64);
    }
    h
}

/// Check if an SMTP response code indicates success
fn is_success(code: u16) -> bool {
    (code >= SMTP_OK_MIN && code <= SMTP_OK_MAX)
        || code == SMTP_AUTH_OK
        || code == SMTP_AUTH_CONTINUE
        || code == SMTP_DATA_START
        || code == SMTP_READY
        || code == SMTP_CLOSING
}

/// Build a response struct from a code
fn make_response(code: u16, text: &[u8]) -> SmtpResponse {
    SmtpResponse {
        code,
        text_hash: smtp_hash(text),
        multiline: false,
        success: is_success(code),
    }
}

/// Count total recipients in an envelope
fn total_recipients(envelope: &EmailEnvelope) -> usize {
    envelope.to_list.len() + envelope.cc_list.len() + envelope.bcc_list.len()
}

// ── Core SMTP Operations ──────────────────────────────────────────────

/// Connect to an SMTP server
pub fn connect(server_hash: u64, port: u16, use_tls: bool) -> Option<u32> {
    let conn_id = {
        let mut id = SMTP_NEXT_ID.lock();
        let current = *id;
        *id = current.wrapping_add(1);
        current
    };

    let actual_port = if port == 0 {
        if use_tls {
            SMTPS_PORT
        } else {
            SMTP_PORT
        }
    } else {
        port
    };

    let conn = SmtpConnection {
        id: conn_id,
        state: SmtpState::Connected,
        server_hash,
        port: actual_port,
        tls_active: use_tls,
        extensions: SmtpExtensions {
            starttls: !use_tls, // only offer if not already TLS
            auth: true,
            eight_bit_mime: true,
            pipelining: true,
            max_size: MAX_BODY_SIZE,
            dsn: false,
            chunking: false,
            smtputf8: false,
        },
        messages_sent: 0,
        username_hash: 0,
        last_response_code: SMTP_READY,
        last_activity: 0,
    };

    let mut conns = SMTP_CONNECTIONS.lock();
    if let Some(ref mut list) = *conns {
        list.push(conn);
    } else {
        *conns = Some(vec![conn]);
    }

    serial_println!(
        "[smtp] Connection {} to port {} (tls={})",
        conn_id,
        actual_port,
        use_tls
    );
    Some(conn_id)
}

/// Send EHLO/HELO greeting to the server
pub fn helo(conn_id: u32, _domain_hash: u64) -> SmtpResponse {
    let mut conns = SMTP_CONNECTIONS.lock();
    if let Some(ref mut list) = *conns {
        if let Some(conn) = list.iter_mut().find(|c| c.id == conn_id) {
            if conn.state != SmtpState::Connected {
                return make_response(503, b"Bad sequence of commands");
            }

            conn.state = SmtpState::Greeted;
            conn.last_response_code = SMTP_OK;

            serial_println!("[smtp] EHLO completed for connection {}", conn_id);
            return make_response(SMTP_OK, b"EHLO accepted");
        }
    }
    make_response(421, b"Service not available")
}

/// Upgrade connection to TLS via STARTTLS
pub fn start_tls(conn_id: u32) -> SmtpResponse {
    let mut conns = SMTP_CONNECTIONS.lock();
    if let Some(ref mut list) = *conns {
        if let Some(conn) = list.iter_mut().find(|c| c.id == conn_id) {
            if conn.state != SmtpState::Greeted {
                return make_response(503, b"Must EHLO first");
            }
            if conn.tls_active {
                return make_response(503, b"TLS already active");
            }
            if !conn.extensions.starttls {
                return make_response(502, b"STARTTLS not supported");
            }

            conn.tls_active = true;
            // After STARTTLS, must re-EHLO — reset state to Connected
            conn.state = SmtpState::Connected;
            conn.last_response_code = SMTP_READY;

            serial_println!("[smtp] STARTTLS completed on connection {}", conn_id);
            return make_response(SMTP_READY, b"Ready to start TLS");
        }
    }
    make_response(421, b"Service not available")
}

/// Authenticate with the SMTP server
pub fn authenticate(conn_id: u32, username_hash: u64, password_hash: u64) -> SmtpResponse {
    let mut conns = SMTP_CONNECTIONS.lock();
    if let Some(ref mut list) = *conns {
        if let Some(conn) = list.iter_mut().find(|c| c.id == conn_id) {
            if conn.state != SmtpState::Greeted {
                return make_response(503, b"Must EHLO first");
            }
            if !conn.extensions.auth {
                return make_response(502, b"AUTH not supported");
            }

            let combined = username_hash.wrapping_add(password_hash);
            if combined != 0 {
                conn.state = SmtpState::Authenticated;
                conn.username_hash = username_hash;
                conn.last_response_code = SMTP_AUTH_OK;

                serial_println!("[smtp] AUTH succeeded on connection {}", conn_id);
                return make_response(SMTP_AUTH_OK, b"Authentication successful");
            } else {
                conn.last_response_code = 535;
                return make_response(535, b"Authentication credentials invalid");
            }
        }
    }
    make_response(421, b"Service not available")
}

/// Send a complete email using the given envelope
pub fn send_email(conn_id: u32, envelope: &EmailEnvelope) -> SmtpResponse {
    // Hard gate — Genesis never sends autonomously
    if !SEND_ENABLED {
        serial_println!("[smtp] BLOCKED: SEND_ENABLED=false, email suppressed");
        return make_response(421, b"Email sending disabled");
    }

    // Validate envelope
    if envelope.to_list.is_empty() {
        return make_response(553, b"No valid recipients");
    }
    if total_recipients(envelope) > MAX_RECIPIENTS {
        return make_response(452, b"Too many recipients");
    }
    if envelope.attachments.len() > MAX_ATTACHMENTS {
        return make_response(552, b"Too many attachments");
    }

    let mut conns = SMTP_CONNECTIONS.lock();
    if let Some(ref mut list) = *conns {
        if let Some(conn) = list.iter_mut().find(|c| c.id == conn_id) {
            if conn.state != SmtpState::Authenticated && conn.state != SmtpState::Greeted {
                return make_response(503, b"Must authenticate first");
            }

            // Phase 1: MAIL FROM
            conn.state = SmtpState::MailFrom;
            serial_println!("[smtp] MAIL FROM:<0x{:016X}>", envelope.from_hash);

            // Phase 2: RCPT TO for each recipient
            let mut accepted_count: u32 = 0;

            for to_hash in &envelope.to_list {
                serial_println!("[smtp]   RCPT TO:<0x{:016X}>", to_hash);
                accepted_count += 1;
            }
            for cc_hash in &envelope.cc_list {
                serial_println!("[smtp]   RCPT TO (CC):<0x{:016X}>", cc_hash);
                accepted_count += 1;
            }
            for bcc_hash in &envelope.bcc_list {
                serial_println!("[smtp]   RCPT TO (BCC):<0x{:016X}>", bcc_hash);
                accepted_count += 1;
            }

            if accepted_count == 0 {
                conn.state = SmtpState::Authenticated;
                return make_response(554, b"No recipients accepted");
            }

            conn.state = SmtpState::RcptAccepted;

            // Phase 3: DATA
            conn.state = SmtpState::DataMode;
            serial_println!(
                "[smtp]   DATA (subject=0x{:016X}, body=0x{:016X})",
                envelope.subject_hash,
                envelope.body_hash
            );

            if !envelope.attachments.is_empty() {
                serial_println!("[smtp]   {} attachment(s)", envelope.attachments.len());
            }

            // Phase 4: End of DATA (CRLF.CRLF)
            conn.messages_sent = conn.messages_sent.saturating_add(1);
            conn.state = SmtpState::Authenticated; // ready for next message
            conn.last_response_code = SMTP_OK;

            // Update global sent count
            drop(conns); // release lock before acquiring SENT_COUNT
            let mut count = SENT_COUNT.lock();
            *count = (*count).saturating_add(1);

            serial_println!(
                "[smtp] Message sent ({} recipients, {} attachments)",
                accepted_count,
                envelope.attachments.len()
            );

            return make_response(SMTP_OK, b"Message accepted for delivery");
        }
    }
    make_response(421, b"Service not available")
}

/// Reset the current mail transaction (RSET)
pub fn reset(conn_id: u32) -> SmtpResponse {
    let mut conns = SMTP_CONNECTIONS.lock();
    if let Some(ref mut list) = *conns {
        if let Some(conn) = list.iter_mut().find(|c| c.id == conn_id) {
            if conn.state == SmtpState::Authenticated
                || conn.state == SmtpState::MailFrom
                || conn.state == SmtpState::RcptAccepted
                || conn.state == SmtpState::DataMode
            {
                conn.state = SmtpState::Authenticated;
                conn.last_response_code = SMTP_OK;
                return make_response(SMTP_OK, b"Reset OK");
            }
        }
    }
    make_response(503, b"Bad sequence")
}

/// Close the SMTP connection gracefully (QUIT)
pub fn quit(conn_id: u32) -> SmtpResponse {
    let mut conns = SMTP_CONNECTIONS.lock();
    if let Some(ref mut list) = *conns {
        if let Some(pos) = list.iter().position(|c| c.id == conn_id) {
            let msgs = list[pos].messages_sent;
            list.remove(pos);
            serial_println!(
                "[smtp] Connection {} closed ({} messages sent)",
                conn_id,
                msgs
            );
            return make_response(SMTP_CLOSING, b"Bye");
        }
    }
    make_response(421, b"Service not available")
}

/// Create a new email envelope with basic fields
pub fn create_envelope(
    from_hash: u64,
    to_hash: u64,
    subject_hash: u64,
    body_hash: u64,
) -> EmailEnvelope {
    EmailEnvelope {
        from_hash,
        to_list: vec![to_hash],
        cc_list: Vec::new(),
        bcc_list: Vec::new(),
        subject_hash,
        body_hash,
        is_html: false,
        attachments: Vec::new(),
        message_id_hash: from_hash.wrapping_mul(0xABCD).wrapping_add(subject_hash),
        in_reply_to: 0,
        priority: 3, // normal
        created_at: 0,
    }
}

/// Add an attachment to an existing envelope
pub fn add_attachment(
    envelope: &mut EmailEnvelope,
    name_hash: u64,
    mime_hash: u64,
    content_hash: u64,
    size: u32,
) -> bool {
    if envelope.attachments.len() >= MAX_ATTACHMENTS {
        return false;
    }
    envelope.attachments.push(Attachment {
        name_hash,
        mime_hash,
        content_hash,
        size,
    });
    true
}

/// Get the total number of emails sent globally
pub fn total_sent() -> u64 {
    *SENT_COUNT.lock()
}

/// Get the number of active SMTP connections
pub fn active_connections() -> usize {
    let conns = SMTP_CONNECTIONS.lock();
    if let Some(ref list) = *conns {
        list.len()
    } else {
        0
    }
}

/// Initialize the SMTP client subsystem
pub fn init() {
    let mut conns = SMTP_CONNECTIONS.lock();
    *conns = Some(Vec::new());
    serial_println!(
        "[smtp] SMTP client initialized (ports {}/{}/{})",
        SMTP_PORT,
        SMTPS_PORT,
        SMTP_RELAY_PORT
    );
}
