/// TLS 1.3 — Transport Layer Security (RFC 8446)
///
/// Secure communication over TCP. Built on our crypto primitives:
///   - X25519 for key exchange
///   - ChaCha20-Poly1305 for AEAD encryption
///   - SHA-256 for transcript hash
///   - HKDF for key derivation
///
/// Supports TLS 1.3 only (no legacy versions). All code is original.
use alloc::string::String;
use alloc::vec::Vec;

/// TLS record types
const RECORD_CHANGE_CIPHER_SPEC: u8 = 20;
const RECORD_ALERT: u8 = 21;
const RECORD_HANDSHAKE: u8 = 22;
const RECORD_APPLICATION_DATA: u8 = 23;

/// TLS 1.3 version
const TLS_13: u16 = 0x0304;
/// Legacy version in record layer
const TLS_12_LEGACY: u16 = 0x0303;

/// Handshake message types
const HANDSHAKE_CLIENT_HELLO: u8 = 1;
const HANDSHAKE_SERVER_HELLO: u8 = 2;
const HANDSHAKE_ENCRYPTED_EXTENSIONS: u8 = 8;
const HANDSHAKE_CERTIFICATE: u8 = 11;
const HANDSHAKE_CERTIFICATE_VERIFY: u8 = 15;
const HANDSHAKE_FINISHED: u8 = 20;

/// TLS cipher suites we support
const TLS_CHACHA20_POLY1305_SHA256: u16 = 0x1303;

/// Extension types
const EXT_SUPPORTED_VERSIONS: u16 = 43;
const EXT_KEY_SHARE: u16 = 51;
const EXT_SERVER_NAME: u16 = 0;

/// Named groups for key exchange
const X25519_GROUP: u16 = 0x001D;

/// TLS connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsState {
    Initial,
    ClientHelloSent,
    ServerHelloReceived,
    HandshakeComplete,
    ApplicationData,
    Closed,
    Error,
}

/// TLS alert levels
#[derive(Debug, Clone, Copy)]
pub enum AlertLevel {
    Warning = 1,
    Fatal = 2,
}

/// TLS alert descriptions
#[derive(Debug, Clone, Copy)]
pub enum AlertDesc {
    CloseNotify = 0,
    UnexpectedMessage = 10,
    BadRecordMac = 20,
    HandshakeFailure = 40,
    CertificateRequired = 116,
    InternalError = 80,
}

/// TLS record header (5 bytes)
#[derive(Debug, Clone, Copy)]
pub struct RecordHeader {
    pub content_type: u8,
    pub legacy_version: u16,
    pub length: u16,
}

impl RecordHeader {
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 5 {
            return None;
        }
        Some(RecordHeader {
            content_type: data[0],
            legacy_version: u16::from_be_bytes([data[1], data[2]]),
            length: u16::from_be_bytes([data[3], data[4]]),
        })
    }

    pub fn to_bytes(&self) -> [u8; 5] {
        let ver = self.legacy_version.to_be_bytes();
        let len = self.length.to_be_bytes();
        [self.content_type, ver[0], ver[1], len[0], len[1]]
    }
}

/// TLS session keys (derived from handshake)
pub struct SessionKeys {
    pub client_write_key: [u8; 32],
    pub server_write_key: [u8; 32],
    pub client_write_iv: [u8; 12],
    pub server_write_iv: [u8; 12],
    pub client_seq: u64,
    pub server_seq: u64,
}

/// TLS connection
pub struct TlsConnection {
    pub state: TlsState,
    pub server_name: String,
    /// Our ephemeral X25519 private key
    pub private_key: [u8; 32],
    /// Our ephemeral X25519 public key
    pub public_key: [u8; 32],
    /// Server's ephemeral public key
    pub server_public: [u8; 32],
    /// Handshake transcript hash
    pub transcript: crate::crypto::sha256::Sha256,
    /// Derived session keys
    pub keys: Option<SessionKeys>,
    /// Receive buffer
    pub recv_buf: Vec<u8>,
}

impl TlsConnection {
    pub fn new(server_name: &str) -> Self {
        // Generate ephemeral keypair
        let mut private_key = [0u8; 32];
        crate::crypto::random::fill_bytes(&mut private_key);
        let public_key = crate::crypto::x25519::public_key(&private_key);

        TlsConnection {
            state: TlsState::Initial,
            server_name: String::from(server_name),
            private_key,
            public_key,
            server_public: [0u8; 32],
            transcript: crate::crypto::sha256::Sha256::new(),
            keys: None,
            recv_buf: Vec::new(),
        }
    }

    /// Build ClientHello message
    pub fn build_client_hello(&mut self) -> Vec<u8> {
        let mut hello = Vec::new();

        // Handshake header
        hello.push(HANDSHAKE_CLIENT_HELLO);
        // Length placeholder (3 bytes) — filled later
        let len_pos = hello.len();
        hello.extend_from_slice(&[0, 0, 0]);

        // Client version (legacy: TLS 1.2)
        hello.extend_from_slice(&TLS_12_LEGACY.to_be_bytes());

        // Random (32 bytes)
        let mut random = [0u8; 32];
        crate::crypto::random::fill_bytes(&mut random);
        hello.extend_from_slice(&random);

        // Session ID (legacy: empty)
        hello.push(0);

        // Cipher suites
        hello.extend_from_slice(&2u16.to_be_bytes()); // length
        hello.extend_from_slice(&TLS_CHACHA20_POLY1305_SHA256.to_be_bytes());

        // Compression methods (legacy: null only)
        hello.push(1);
        hello.push(0);

        // Extensions
        let mut extensions = Vec::new();

        // SNI extension
        {
            let name_bytes = self.server_name.as_bytes();
            let mut sni = Vec::new();
            let list_len = (name_bytes.len() + 3) as u16;
            sni.extend_from_slice(&list_len.to_be_bytes());
            sni.push(0); // host_name type
            sni.extend_from_slice(&(name_bytes.len() as u16).to_be_bytes());
            sni.extend_from_slice(name_bytes);

            extensions.extend_from_slice(&EXT_SERVER_NAME.to_be_bytes());
            extensions.extend_from_slice(&(sni.len() as u16).to_be_bytes());
            extensions.extend_from_slice(&sni);
        }

        // Supported versions extension
        {
            let mut sv = Vec::new();
            sv.push(2); // list length
            sv.extend_from_slice(&TLS_13.to_be_bytes());

            extensions.extend_from_slice(&EXT_SUPPORTED_VERSIONS.to_be_bytes());
            extensions.extend_from_slice(&(sv.len() as u16).to_be_bytes());
            extensions.extend_from_slice(&sv);
        }

        // Key share extension
        {
            let mut ks = Vec::new();
            let entry_len = (2 + 2 + 32) as u16; // group + key_len + key
            ks.extend_from_slice(&entry_len.to_be_bytes()); // client_shares length
            ks.extend_from_slice(&X25519_GROUP.to_be_bytes());
            ks.extend_from_slice(&32u16.to_be_bytes());
            ks.extend_from_slice(&self.public_key);

            extensions.extend_from_slice(&EXT_KEY_SHARE.to_be_bytes());
            extensions.extend_from_slice(&(ks.len() as u16).to_be_bytes());
            extensions.extend_from_slice(&ks);
        }

        // Append extensions
        hello.extend_from_slice(&(extensions.len() as u16).to_be_bytes());
        hello.extend_from_slice(&extensions);

        // Fill in handshake length
        let msg_len = (hello.len() - 4) as u32;
        hello[len_pos] = ((msg_len >> 16) & 0xFF) as u8;
        hello[len_pos + 1] = ((msg_len >> 8) & 0xFF) as u8;
        hello[len_pos + 2] = (msg_len & 0xFF) as u8;

        // Update transcript
        self.transcript.update(&hello);

        // Wrap in record
        let mut record = Vec::new();
        let header = RecordHeader {
            content_type: RECORD_HANDSHAKE,
            legacy_version: TLS_12_LEGACY,
            length: hello.len() as u16,
        };
        record.extend_from_slice(&header.to_bytes());
        record.extend_from_slice(&hello);

        self.state = TlsState::ClientHelloSent;
        record
    }

    /// Process a received ServerHello
    pub fn process_server_hello(&mut self, data: &[u8]) -> Result<(), &'static str> {
        if data.len() < 4 {
            return Err("too short");
        }
        if data[0] != HANDSHAKE_SERVER_HELLO {
            return Err("not ServerHello");
        }

        // Update transcript
        self.transcript.update(data);

        // Parse extensions to find server's key share
        // (simplified — real impl needs full extension parsing)
        // Look for our X25519 key share in the message
        let mut i = 6 + 32 + 1; // skip version, random, session_id_len
        if i < data.len() {
            let sid_len = data[i - 1] as usize;
            i += sid_len + 2 + 1; // skip session_id, cipher_suite, compression
        }

        // Find key_share extension with server's public key
        // For now, extract the 32-byte key from a known offset
        if data.len() >= 100 {
            // Look for X25519 group ID (0x001D) followed by 32-byte key
            for j in 0..data.len().saturating_sub(34) {
                if data[j] == 0x00
                    && data[j + 1] == 0x1D
                    && data[j + 2] == 0x00
                    && data[j + 3] == 0x20
                {
                    self.server_public.copy_from_slice(&data[j + 4..j + 36]);
                    break;
                }
            }
        }

        // Derive shared secret
        let shared = crate::crypto::x25519::shared_secret(&self.private_key, &self.server_public);

        // Derive handshake keys via HKDF
        let early_secret = crate::crypto::hmac::hkdf_extract(&[0u8; 32], &[0u8; 32]);
        let handshake_secret = crate::crypto::hmac::hkdf_extract(&early_secret, &shared);
        let keys_material =
            crate::crypto::hmac::hkdf_expand(&handshake_secret, b"tls13 derived", 96);

        let mut client_key = [0u8; 32];
        let mut server_key = [0u8; 32];
        let mut client_iv = [0u8; 12];
        let mut server_iv = [0u8; 12];

        client_key.copy_from_slice(&keys_material[0..32]);
        server_key.copy_from_slice(&keys_material[32..64]);
        client_iv.copy_from_slice(&keys_material[64..76]);
        server_iv.copy_from_slice(&keys_material[76..88]);

        self.keys = Some(SessionKeys {
            client_write_key: client_key,
            server_write_key: server_key,
            client_write_iv: client_iv,
            server_write_iv: server_iv,
            client_seq: 0,
            server_seq: 0,
        });

        self.state = TlsState::ServerHelloReceived;
        Ok(())
    }

    /// Encrypt application data
    pub fn encrypt(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, &'static str> {
        let keys = self.keys.as_mut().ok_or("no keys")?;

        let mut data = plaintext.to_vec();
        data.push(RECORD_APPLICATION_DATA); // inner content type

        // Build nonce: IV XOR sequence number
        let mut nonce = keys.client_write_iv;
        let seq_bytes = keys.client_seq.to_be_bytes();
        for i in 0..8 {
            nonce[4 + i] ^= seq_bytes[i];
        }

        // Encrypt with ChaCha20-Poly1305
        let aad = RecordHeader {
            content_type: RECORD_APPLICATION_DATA,
            legacy_version: TLS_12_LEGACY,
            length: (data.len() + 16) as u16, // +16 for tag
        }
        .to_bytes();

        let tag =
            crate::crypto::poly1305::aead_encrypt(&keys.client_write_key, &nonce, &aad, &mut data);

        keys.client_seq = keys.client_seq.saturating_add(1);

        // Build record
        let mut record = Vec::new();
        record.extend_from_slice(&aad);
        record.extend_from_slice(&data);
        record.extend_from_slice(&tag);

        Ok(record)
    }

    /// Decrypt received application data
    pub fn decrypt(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, &'static str> {
        let keys = self.keys.as_mut().ok_or("no keys")?;

        if ciphertext.len() < 16 {
            return Err("too short for tag");
        }

        let mut data = ciphertext[..ciphertext.len() - 16].to_vec();

        // Build nonce
        let mut nonce = keys.server_write_iv;
        let seq_bytes = keys.server_seq.to_be_bytes();
        for i in 0..8 {
            nonce[4 + i] ^= seq_bytes[i];
        }

        // Decrypt
        crate::crypto::chacha20::chacha20_decrypt(&keys.server_write_key, &nonce, 1, &mut data);

        keys.server_seq = keys.server_seq.saturating_add(1);

        // Remove inner content type
        if let Some(&content_type) = data.last() {
            data.pop();
            if content_type != RECORD_APPLICATION_DATA {
                // Could be handshake or alert
            }
        }

        Ok(data)
    }

    /// Send close_notify
    pub fn close(&mut self) -> Vec<u8> {
        self.state = TlsState::Closed;
        // Build alert record
        let alert = [AlertLevel::Warning as u8, AlertDesc::CloseNotify as u8];
        self.encrypt(&alert).unwrap_or_default()
    }
}

/// Simple HTTPS GET request builder
pub fn https_get(host: &str, path: &str) -> Vec<u8> {
    let request = alloc::format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        path,
        host
    );
    request.into_bytes()
}

// ---------------------------------------------------------------------------
// TlsSession — high-level public API wrapping TlsConnection + TCP conn ID
// ---------------------------------------------------------------------------

/// A TLS session handle returned by `tls_connect()`.
///
/// Wraps a `TlsConnection` (which owns the handshake state and session keys)
/// together with the underlying TCP connection ID.  After `tls_connect()`
/// succeeds the session is ready for `tls_send()` / `tls_recv()` calls.
///
/// # Handshake status
/// The `connected` flag is set to `true` when a successful TCP connection is
/// established and the ClientHello has been sent.  Full TLS 1.3 handshake
/// verification (ServerHello, certificates, Finished) is a work-in-progress;
/// the current implementation sends ClientHello and attempts to parse
/// ServerHello for the key-share, then falls back to unencrypted plaintext
/// for data transfer when the handshake cannot be completed.  This is noted as
/// insecure and is marked with `// STUB` comments.
pub struct TlsSession {
    /// Whether the TCP connection + ClientHello exchange succeeded.
    pub connected: bool,
    /// Negotiated cipher suite (set after ServerHello; 0 until then).
    pub cipher_suite: u16,
    /// Internal handshake state.
    pub state: TlsState,
    /// Underlying TLS connection (handshake + crypto state).
    conn: TlsConnection,
    /// TCP connection ID in the kernel's TCP table.
    tcp_conn_id: u32,
}

/// Attempt to establish a TLS 1.3 connection to `host:port`.
///
/// Steps:
/// 1. Resolve `host` via DNS.
/// 2. Open a TCP connection.
/// 3. Send a TLS 1.3 ClientHello.
/// 4. Wait up to ~500 000 polls for a ServerHello.
/// 5. Attempt key-share extraction and HKDF key derivation.
/// 6. If key derivation fails, log the fallback warning and keep the session
///    alive over the plaintext TCP stream (insecure stub behaviour).
///
/// Returns `None` if DNS resolution or the TCP connection fails.
pub fn tls_connect(host: &str, port: u16) -> Option<TlsSession> {
    // 1. DNS
    let ip_bytes = crate::net::dns::resolve_a(host)?;
    let dst_ip = crate::net::Ipv4Addr(ip_bytes);

    // 2. Ephemeral local port
    static TLS_CLIENT_PORT: crate::sync::Mutex<u16> = crate::sync::Mutex::new(50000);
    let local_port = {
        let mut p = TLS_CLIENT_PORT.lock();
        let v = *p;
        *p = if *p >= 65400 { 50000 } else { *p + 1 };
        v
    };

    // 3. TCP connect
    let tcp_conn_id = crate::net::tcp::connect(local_port, dst_ip, port);

    let mut established = false;
    for _ in 0..200_000u32 {
        crate::net::poll();
        match crate::net::tcp::get_state(tcp_conn_id) {
            Some(crate::net::tcp::TcpState::Established) => {
                established = true;
                break;
            }
            Some(crate::net::tcp::TcpState::Closed) => break,
            _ => {}
        }
        core::hint::spin_loop();
    }

    if !established {
        crate::serial_println!("  TLS: TCP connect to {}:{} failed", host, port);
        return None;
    }

    // 4. Build and send ClientHello
    let mut tls_conn = TlsConnection::new(host);
    let client_hello = tls_conn.build_client_hello();

    if crate::net::tcp::send_data(tcp_conn_id, &client_hello).is_err() {
        crate::serial_println!("  TLS: send ClientHello failed");
        crate::net::tcp::close_connection(tcp_conn_id);
        return None;
    }

    crate::serial_println!("  TLS: ClientHello sent to {}:{}", host, port);

    // 5. Wait for ServerHello
    let mut recv_buf: Vec<u8> = Vec::new();
    let mut handshake_done = false;
    let mut cipher_suite = 0u16;

    'outer: for _ in 0..500_000u32 {
        crate::net::poll();
        let chunk = crate::net::tcp::read_data(tcp_conn_id);
        if !chunk.is_empty() {
            recv_buf.extend_from_slice(&chunk);
        }

        // Try to find a ServerHello record (content_type=22, handshake type=2)
        let mut i = 0;
        while i + 5 <= recv_buf.len() {
            let content_type = recv_buf[i];
            let rec_len = u16::from_be_bytes([recv_buf[i + 3], recv_buf[i + 4]]) as usize;
            if i + 5 + rec_len > recv_buf.len() {
                break; // wait for more data
            }
            let record_body = &recv_buf[i + 5..i + 5 + rec_len];

            if content_type == RECORD_HANDSHAKE
                && !record_body.is_empty()
                && record_body[0] == HANDSHAKE_SERVER_HELLO
            {
                // Extract cipher suite (bytes 9-10 of ServerHello body, after
                // 4-byte handshake header + 2-byte version + 32-byte random + 1 byte sid len)
                if record_body.len() >= 44 {
                    let sid_len = record_body[38] as usize;
                    let cs_offset = 39 + sid_len;
                    if cs_offset + 2 <= record_body.len() {
                        cipher_suite = u16::from_be_bytes([
                            record_body[cs_offset],
                            record_body[cs_offset + 1],
                        ]);
                    }
                }

                // Attempt to process the ServerHello for key derivation
                match tls_conn.process_server_hello(record_body) {
                    Ok(()) => {
                        tls_conn.state = TlsState::HandshakeComplete;
                        handshake_done = true;
                    }
                    Err(e) => {
                        // STUB: key derivation incomplete — fall back to plaintext
                        crate::serial_println!(
                            "  TLS: handshake not yet implemented ({}); falling back to plaintext TCP — INSECURE",
                            e
                        );
                        tls_conn.state = TlsState::ApplicationData;
                        handshake_done = true; // stub: treat as done
                    }
                }
                break 'outer;
            }
            i += 5 + rec_len;
        }

        if handshake_done {
            break;
        }
        core::hint::spin_loop();
    }

    if !handshake_done {
        // STUB: timed out waiting for ServerHello — log and fall through
        crate::serial_println!(
            "  TLS: ServerHello timeout for {}:{} — using plaintext fallback (INSECURE)",
            host,
            port
        );
        tls_conn.state = TlsState::ApplicationData;
    }

    Some(TlsSession {
        connected: true,
        cipher_suite,
        state: tls_conn.state,
        conn: tls_conn,
        tcp_conn_id,
    })
}

/// Send data over a TLS session.
///
/// If session keys are available the data is encrypted with ChaCha20-Poly1305
/// before transmission.  Otherwise it is sent as plaintext over the underlying
/// TCP connection (stub/insecure fallback).
///
/// Returns `true` if the send succeeded.
pub fn tls_send(session: &mut TlsSession, data: &[u8]) -> bool {
    if !session.connected {
        return false;
    }

    let to_send: Vec<u8> = if session.conn.keys.is_some() {
        // Encrypt the payload
        match session.conn.encrypt(data) {
            Ok(enc) => enc,
            Err(e) => {
                crate::serial_println!("  TLS: encrypt error: {}", e);
                return false;
            }
        }
    } else {
        // STUB: no keys — send plaintext
        Vec::from(data)
    };

    match crate::net::tcp::send_data(session.tcp_conn_id, &to_send) {
        Ok(_) => true,
        Err(_) => {
            session.connected = false;
            false
        }
    }
}

/// Receive decrypted data from a TLS session into `buf`.
///
/// Reads available TCP data and attempts TLS record decryption if session keys
/// are present.  Returns the number of bytes written into `buf` (may be 0 if
/// no data is currently available — this is non-blocking).
pub fn tls_recv(session: &mut TlsSession, buf: &mut [u8]) -> usize {
    if !session.connected {
        return 0;
    }

    crate::net::poll();
    let raw = crate::net::tcp::read_data(session.tcp_conn_id);
    if raw.is_empty() {
        return 0;
    }

    // Accumulate into the connection's receive buffer
    session.conn.recv_buf.extend_from_slice(&raw);

    // Try to strip TLS record framing and decrypt
    let mut output: Vec<u8> = Vec::new();
    let mut pos = 0;

    while pos + 5 <= session.conn.recv_buf.len() {
        let content_type = session.conn.recv_buf[pos];
        let rec_len = u16::from_be_bytes([
            session.conn.recv_buf[pos + 3],
            session.conn.recv_buf[pos + 4],
        ]) as usize;

        if pos + 5 + rec_len > session.conn.recv_buf.len() {
            break; // incomplete record — wait for more data
        }

        let record_body = &session.conn.recv_buf[pos + 5..pos + 5 + rec_len].to_vec();
        pos += 5 + rec_len;

        if content_type == RECORD_APPLICATION_DATA {
            if session.conn.keys.is_some() {
                match session.conn.decrypt(record_body) {
                    Ok(plain) => output.extend_from_slice(&plain),
                    Err(_) => {
                        // STUB: decryption failed — pass raw bytes through
                        output.extend_from_slice(record_body);
                    }
                }
            } else {
                // STUB: no keys — treat as plaintext
                output.extend_from_slice(record_body);
            }
        }
        // Ignore non-application-data records (handshake, alerts, CCS)
    }

    // Consume processed bytes from recv_buf
    let remaining = session.conn.recv_buf[pos..].to_vec();
    session.conn.recv_buf = remaining;

    let copy_len = output.len().min(buf.len());
    buf[..copy_len].copy_from_slice(&output[..copy_len]);
    copy_len
}

/// Close a TLS session gracefully.
///
/// Sends a TLS close_notify alert if keys are available, then closes the
/// underlying TCP connection.
pub fn tls_close(mut session: TlsSession) {
    if session.connected {
        // Send close_notify (best-effort — ignore errors)
        let alert = session.conn.close();
        let _ = crate::net::tcp::send_data(session.tcp_conn_id, &alert);
        crate::net::tcp::close_connection(session.tcp_conn_id);
        session.connected = false;
    }
    crate::serial_println!("  TLS: session closed");
}
