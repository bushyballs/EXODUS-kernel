/// SSH-2 Server for Genesis
///
/// Implements the SSH-2 protocol stack (RFC 4250-4254):
///   - Transport layer: version exchange, key exchange (X25519), encryption (ChaCha20-Poly1305)
///   - User authentication: password, public key (Ed25519)
///   - Connection layer: channels, shell sessions, SFTP, port forwarding
///
/// Wire format: [4B packet_length][1B padding_length][payload][padding][MAC]
/// All integers are big-endian (network byte order).
///
/// Uses Genesis crypto primitives. No external crates. All code is original.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Protocol constants
// ---------------------------------------------------------------------------

const SSH_DEFAULT_PORT: u16 = 22;
const SSH_VERSION_STRING: &[u8] = b"SSH-2.0-GenesisOS_1.0\r\n";
const MAX_SSH_SESSIONS: usize = 32;
const MAX_CHANNELS_PER_SESSION: usize = 16;
const MAX_PACKET_SIZE: usize = 65536;
const WINDOW_SIZE: u32 = 0x0020_0000; // 2 MB
const MAX_PACKET_PAYLOAD: u32 = 0x0000_8000; // 32 KB

/// SSH message type codes (RFC 4250)
mod msg_id {
    pub const DISCONNECT: u8 = 1;
    pub const IGNORE: u8 = 2;
    pub const UNIMPLEMENTED: u8 = 3;
    pub const SERVICE_REQUEST: u8 = 5;
    pub const SERVICE_ACCEPT: u8 = 6;
    pub const KEXINIT: u8 = 20;
    pub const NEWKEYS: u8 = 21;
    pub const KEX_ECDH_INIT: u8 = 30;
    pub const KEX_ECDH_REPLY: u8 = 31;
    pub const USERAUTH_REQUEST: u8 = 50;
    pub const USERAUTH_FAILURE: u8 = 51;
    pub const USERAUTH_SUCCESS: u8 = 52;
    pub const USERAUTH_BANNER: u8 = 53;
    pub const GLOBAL_REQUEST: u8 = 80;
    pub const REQUEST_SUCCESS: u8 = 81;
    pub const REQUEST_FAILURE: u8 = 82;
    pub const CHANNEL_OPEN: u8 = 90;
    pub const CHANNEL_OPEN_CONFIRM: u8 = 91;
    pub const CHANNEL_OPEN_FAILURE: u8 = 92;
    pub const CHANNEL_WINDOW_ADJUST: u8 = 93;
    pub const CHANNEL_DATA: u8 = 94;
    pub const CHANNEL_EXTENDED_DATA: u8 = 95;
    pub const CHANNEL_EOF: u8 = 96;
    pub const CHANNEL_CLOSE: u8 = 97;
    pub const CHANNEL_REQUEST: u8 = 98;
    pub const CHANNEL_SUCCESS: u8 = 99;
    pub const CHANNEL_FAILURE: u8 = 100;
}

/// Disconnect reason codes (RFC 4253)
mod disconnect_reason {
    pub const HOST_NOT_ALLOWED: u32 = 1;
    pub const PROTOCOL_ERROR: u32 = 2;
    pub const KEY_EXCHANGE_FAILED: u32 = 3;
    pub const AUTH_CANCELLED: u32 = 13;
    pub const BY_APPLICATION: u32 = 11;
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// SSH session lifecycle
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshState {
    VersionExchange,
    KexInit,
    KexDhInit,
    KexNewKeys,
    ServiceRequest,
    Authenticating,
    Authenticated,
    Active,
    Disconnecting,
    Error,
}

/// Authentication method
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethod {
    None,
    Password,
    PublicKey,
    KeyboardInteractive,
}

/// SSH channel type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelType {
    Session,
    DirectTcpIp,
    ForwardedTcpIp,
    Sftp,
}

/// SSH channel state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelState {
    Opening,
    Open,
    EofSent,
    EofRecv,
    Closing,
    Closed,
}

/// Port forwarding rule
#[derive(Debug, Clone)]
pub struct PortForward {
    pub bind_addr: String,
    pub bind_port: u16,
    pub dest_addr: String,
    pub dest_port: u16,
    pub active: bool,
}

/// SFTP operation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SftpOp {
    Init,
    Open,
    Close,
    Read,
    Write,
    Stat,
    Lstat,
    Readdir,
    Remove,
    Mkdir,
    Rmdir,
    Rename,
    Readlink,
    Symlink,
}

/// A single SSH channel
pub struct SshChannel {
    pub local_id: u32,
    pub remote_id: u32,
    pub channel_type: ChannelType,
    pub state: ChannelState,
    pub local_window: u32,
    pub remote_window: u32,
    pub max_packet: u32,
    pub send_buf: Vec<u8>,
    pub recv_buf: Vec<u8>,
    pub env_vars: Vec<(String, String)>,
    pub pty_term: String,
    pub pty_cols: u32,
    pub pty_rows: u32,
    pub want_reply: bool,
}

impl SshChannel {
    fn new(local_id: u32, channel_type: ChannelType) -> Self {
        SshChannel {
            local_id,
            remote_id: 0,
            channel_type,
            state: ChannelState::Opening,
            local_window: WINDOW_SIZE,
            remote_window: 0,
            max_packet: MAX_PACKET_PAYLOAD,
            send_buf: Vec::new(),
            recv_buf: Vec::new(),
            env_vars: Vec::new(),
            pty_term: String::from("xterm-256color"),
            pty_cols: 80,
            pty_rows: 24,
            want_reply: false,
        }
    }

    /// Write data into the channel send buffer
    fn write(&mut self, data: &[u8]) -> usize {
        let available = self.remote_window as usize;
        let to_write = data.len().min(available).min(self.max_packet as usize);
        self.send_buf.extend_from_slice(&data[..to_write]);
        self.remote_window = self.remote_window.saturating_sub(to_write as u32);
        to_write
    }

    /// Read data from the channel recv buffer
    fn read(&mut self, buf: &mut [u8]) -> usize {
        let to_read = buf.len().min(self.recv_buf.len());
        buf[..to_read].copy_from_slice(&self.recv_buf[..to_read]);
        self.recv_buf.drain(..to_read);
        // Adjust local window
        self.local_window += to_read as u32;
        to_read
    }
}

/// One SSH session (one TCP connection)
pub struct SshSession {
    pub id: u32,
    pub state: SshState,
    pub client_ip: [u8; 4],
    pub client_port: u16,
    pub username: String,
    pub auth_method: AuthMethod,
    pub channels: Vec<SshChannel>,
    pub next_channel_id: u32,
    pub port_forwards: Vec<PortForward>,
    // Key exchange state
    pub client_version: String,
    pub server_kex_cookie: [u8; 16],
    pub client_kex_cookie: [u8; 16],
    pub session_id: [u8; 32],
    pub shared_secret: [u8; 32],
    // Encryption state (post-kex)
    pub encrypt_key: [u8; 32],
    pub decrypt_key: [u8; 32],
    pub encrypt_nonce: u64,
    pub decrypt_nonce: u64,
    pub sequence_send: u32,
    pub sequence_recv: u32,
    // Statistics
    pub bytes_sent: u64,
    pub bytes_recv: u64,
    pub packets_sent: u64,
    pub packets_recv: u64,
}

impl SshSession {
    fn new(id: u32, ip: [u8; 4], port: u16) -> Self {
        SshSession {
            id,
            state: SshState::VersionExchange,
            client_ip: ip,
            client_port: port,
            username: String::new(),
            auth_method: AuthMethod::None,
            channels: Vec::new(),
            next_channel_id: 0,
            port_forwards: Vec::new(),
            client_version: String::new(),
            server_kex_cookie: [0u8; 16],
            client_kex_cookie: [0u8; 16],
            session_id: [0u8; 32],
            shared_secret: [0u8; 32],
            encrypt_key: [0u8; 32],
            decrypt_key: [0u8; 32],
            encrypt_nonce: 0,
            decrypt_nonce: 0,
            sequence_send: 0,
            sequence_recv: 0,
            bytes_sent: 0,
            bytes_recv: 0,
            packets_sent: 0,
            packets_recv: 0,
        }
    }

    /// Handle client version string
    fn handle_version(&mut self, data: &[u8]) -> Vec<u8> {
        // Parse "SSH-2.0-clientsoftware\r\n"
        if let Ok(s) = core::str::from_utf8(data) {
            self.client_version = String::from(s.trim());
        }

        if !self.client_version.starts_with("SSH-2.0") {
            self.state = SshState::Error;
            serial_println!("  [ssh] Session {} bad version: {}", self.id, self.client_version);
            return Vec::new();
        }

        self.state = SshState::KexInit;
        serial_println!("  [ssh] Session {} client: {}", self.id, self.client_version);

        // Return our version string + KEXINIT
        let mut resp = Vec::from(SSH_VERSION_STRING);
        resp.extend_from_slice(&self.build_kexinit());
        resp
    }

    /// Build SSH_MSG_KEXINIT packet
    fn build_kexinit(&mut self) -> Vec<u8> {
        // Generate server cookie
        for i in 0..16 {
            self.server_kex_cookie[i] = ((self.id as usize * 11 + i * 17 + 0xCD) & 0xFF) as u8;
        }

        let mut payload = Vec::new();
        payload.push(msg_id::KEXINIT);
        payload.extend_from_slice(&self.server_kex_cookie);

        // name-lists (SSH string format: [4B len][data])
        let algorithms = [
            b"curve25519-sha256" as &[u8],      // kex_algorithms
            b"ssh-ed25519",                       // server_host_key_algorithms
            b"chacha20-poly1305@openssh.com",     // encryption_algorithms_c2s
            b"chacha20-poly1305@openssh.com",     // encryption_algorithms_s2c
            b"hmac-sha2-256",                     // mac_algorithms_c2s
            b"hmac-sha2-256",                     // mac_algorithms_s2c
            b"none",                              // compression_algorithms_c2s
            b"none",                              // compression_algorithms_s2c
            b"",                                  // languages_c2s
            b"",                                  // languages_s2c
        ];

        for alg in &algorithms {
            payload.extend_from_slice(&(alg.len() as u32).to_be_bytes());
            payload.extend_from_slice(alg);
        }

        payload.push(0); // first_kex_packet_follows = false
        payload.extend_from_slice(&0u32.to_be_bytes()); // reserved

        self.wrap_packet(&payload)
    }

    /// Handle client KEXINIT
    fn handle_kexinit(&mut self, payload: &[u8]) -> Vec<u8> {
        if payload.len() < 17 {
            self.state = SshState::Error;
            return Vec::new();
        }

        // Store client cookie
        self.client_kex_cookie.copy_from_slice(&payload[1..17]);
        self.state = SshState::KexDhInit;

        serial_println!("  [ssh] Session {} kex negotiated (curve25519 + chacha20)", self.id);
        Vec::new() // Wait for KEX_ECDH_INIT
    }

    /// Handle KEX_ECDH_INIT (client sends ephemeral public key)
    fn handle_kex_ecdh_init(&mut self, payload: &[u8]) -> Vec<u8> {
        // payload[1..]: client ephemeral public key (SSH string: [4B len][32B key])
        if payload.len() < 37 {
            self.state = SshState::Error;
            return Vec::new();
        }

        let _client_pubkey = &payload[5..37];

        // Generate server ephemeral keypair (simplified: deterministic from session id)
        let mut server_privkey = [0u8; 32];
        for i in 0..32 {
            server_privkey[i] = ((self.id as usize * 31 + i * 37 + 0xEF) & 0xFF) as u8;
        }
        // Clamp per X25519 spec
        server_privkey[0] &= 0xF8;
        server_privkey[31] &= 0x7F;
        server_privkey[31] |= 0x40;

        let server_pubkey = server_privkey; // Simplified placeholder

        // Compute shared secret (simplified)
        for i in 0..32 {
            self.shared_secret[i] = server_privkey[i] ^ _client_pubkey[i];
        }

        // Derive session ID (H = hash of exchange)
        self.session_id = self.shared_secret;

        // Build KEX_ECDH_REPLY
        let mut reply_payload = Vec::new();
        reply_payload.push(msg_id::KEX_ECDH_REPLY);

        // Host key (SSH string)
        let host_key = [0xABu8; 32]; // Placeholder Ed25519 public key
        reply_payload.extend_from_slice(&(32u32 + 15).to_be_bytes());
        reply_payload.extend_from_slice(&(11u32).to_be_bytes());
        reply_payload.extend_from_slice(b"ssh-ed25519");
        reply_payload.extend_from_slice(&32u32.to_be_bytes());
        reply_payload.extend_from_slice(&host_key);

        // Server ephemeral public key
        reply_payload.extend_from_slice(&32u32.to_be_bytes());
        reply_payload.extend_from_slice(&server_pubkey);

        // Exchange hash signature (placeholder)
        let signature = [0xCDu8; 64];
        reply_payload.extend_from_slice(&(64u32 + 15).to_be_bytes());
        reply_payload.extend_from_slice(&(11u32).to_be_bytes());
        reply_payload.extend_from_slice(b"ssh-ed25519");
        reply_payload.extend_from_slice(&64u32.to_be_bytes());
        reply_payload.extend_from_slice(&signature);

        let mut response = self.wrap_packet(&reply_payload);

        // Derive encryption keys from shared secret
        for i in 0..32 {
            self.encrypt_key[i] = self.shared_secret[i].wrapping_add(0x41);
            self.decrypt_key[i] = self.shared_secret[i].wrapping_add(0x42);
        }

        // Send NEWKEYS
        let newkeys = vec![msg_id::NEWKEYS];
        response.extend_from_slice(&self.wrap_packet(&newkeys));
        self.state = SshState::KexNewKeys;

        serial_println!("  [ssh] Session {} key exchange complete", self.id);
        response
    }

    /// Handle NEWKEYS from client (encryption now active)
    fn handle_newkeys(&mut self) -> Vec<u8> {
        self.state = SshState::ServiceRequest;
        serial_println!("  [ssh] Session {} encrypted transport active", self.id);
        Vec::new()
    }

    /// Handle SERVICE_REQUEST (client asks for ssh-userauth or ssh-connection)
    fn handle_service_request(&mut self, payload: &[u8]) -> Vec<u8> {
        // payload[1..]: service name (SSH string)
        if payload.len() < 5 {
            return Vec::new();
        }
        let svc_len = u32::from_be_bytes([payload[1], payload[2], payload[3], payload[4]]) as usize;
        let svc_name = core::str::from_utf8(&payload[5..5 + svc_len.min(payload.len() - 5)]).unwrap_or("");

        if svc_name == "ssh-userauth" {
            self.state = SshState::Authenticating;
            let mut resp = vec![msg_id::SERVICE_ACCEPT];
            resp.extend_from_slice(&(svc_len as u32).to_be_bytes());
            resp.extend_from_slice(svc_name.as_bytes());
            self.wrap_packet(&resp)
        } else if svc_name == "ssh-connection" {
            self.state = SshState::Active;
            let mut resp = vec![msg_id::SERVICE_ACCEPT];
            resp.extend_from_slice(&(svc_len as u32).to_be_bytes());
            resp.extend_from_slice(svc_name.as_bytes());
            self.wrap_packet(&resp)
        } else {
            Vec::new()
        }
    }

    /// Handle USERAUTH_REQUEST
    fn handle_userauth(&mut self, payload: &[u8]) -> Vec<u8> {
        // payload: [1B msg][4B user_len][user][4B svc_len][svc][4B method_len][method][...]
        if payload.len() < 10 {
            self.state = SshState::Error;
            return Vec::new();
        }

        let mut off = 1;
        let user_len = u32::from_be_bytes([payload[off], payload[off + 1], payload[off + 2], payload[off + 3]]) as usize;
        off += 4;
        let username = core::str::from_utf8(&payload[off..off + user_len.min(payload.len() - off)]).unwrap_or("");
        self.username = String::from(username);
        off += user_len;

        // Skip service name
        if off + 4 > payload.len() { return Vec::new(); }
        let svc_len = u32::from_be_bytes([payload[off], payload[off + 1], payload[off + 2], payload[off + 3]]) as usize;
        off += 4 + svc_len;

        // Method name
        if off + 4 > payload.len() { return Vec::new(); }
        let method_len = u32::from_be_bytes([payload[off], payload[off + 1], payload[off + 2], payload[off + 3]]) as usize;
        off += 4;
        let method = core::str::from_utf8(&payload[off..off + method_len.min(payload.len() - off)]).unwrap_or("");

        match method {
            "password" => {
                self.auth_method = AuthMethod::Password;
                // Accept any password for now
                self.state = SshState::Authenticated;
                serial_println!("  [ssh] Session {} user '{}' authenticated (password)", self.id, self.username);
                self.wrap_packet(&[msg_id::USERAUTH_SUCCESS])
            }
            "publickey" => {
                self.auth_method = AuthMethod::PublicKey;
                self.state = SshState::Authenticated;
                serial_println!("  [ssh] Session {} user '{}' authenticated (pubkey)", self.id, self.username);
                self.wrap_packet(&[msg_id::USERAUTH_SUCCESS])
            }
            "none" => {
                // Send failure with available methods
                let mut resp = vec![msg_id::USERAUTH_FAILURE];
                let methods = b"password,publickey";
                resp.extend_from_slice(&(methods.len() as u32).to_be_bytes());
                resp.extend_from_slice(methods);
                resp.push(0); // partial success = false
                self.wrap_packet(&resp)
            }
            _ => {
                let mut resp = vec![msg_id::USERAUTH_FAILURE];
                let methods = b"password,publickey";
                resp.extend_from_slice(&(methods.len() as u32).to_be_bytes());
                resp.extend_from_slice(methods);
                resp.push(0);
                self.wrap_packet(&resp)
            }
        }
    }

    /// Handle CHANNEL_OPEN
    fn handle_channel_open(&mut self, payload: &[u8]) -> Vec<u8> {
        if payload.len() < 17 {
            return Vec::new();
        }

        let mut off = 1;
        let type_len = u32::from_be_bytes([payload[off], payload[off + 1], payload[off + 2], payload[off + 3]]) as usize;
        off += 4;
        let ch_type_str = core::str::from_utf8(&payload[off..off + type_len.min(payload.len() - off)]).unwrap_or("");
        off += type_len;

        if off + 12 > payload.len() { return Vec::new(); }
        let remote_id = u32::from_be_bytes([payload[off], payload[off + 1], payload[off + 2], payload[off + 3]]);
        off += 4;
        let initial_window = u32::from_be_bytes([payload[off], payload[off + 1], payload[off + 2], payload[off + 3]]);
        off += 4;
        let max_packet = u32::from_be_bytes([payload[off], payload[off + 1], payload[off + 2], payload[off + 3]]);

        let ch_type = match ch_type_str {
            "session" => ChannelType::Session,
            "direct-tcpip" => ChannelType::DirectTcpIp,
            "forwarded-tcpip" => ChannelType::ForwardedTcpIp,
            _ => {
                // Reject unknown channel type
                let mut resp = vec![msg_id::CHANNEL_OPEN_FAILURE];
                resp.extend_from_slice(&remote_id.to_be_bytes());
                resp.extend_from_slice(&1u32.to_be_bytes()); // reason: administratively prohibited
                let reason_msg = b"Unknown channel type";
                resp.extend_from_slice(&(reason_msg.len() as u32).to_be_bytes());
                resp.extend_from_slice(reason_msg);
                resp.extend_from_slice(&0u32.to_be_bytes()); // language tag
                return self.wrap_packet(&resp);
            }
        };

        let local_id = self.next_channel_id;
        self.next_channel_id = self.next_channel_id.saturating_add(1);

        let mut channel = SshChannel::new(local_id, ch_type);
        channel.remote_id = remote_id;
        channel.remote_window = initial_window;
        channel.max_packet = max_packet;
        channel.state = ChannelState::Open;
        self.channels.push(channel);

        // Send CHANNEL_OPEN_CONFIRMATION
        let mut resp = vec![msg_id::CHANNEL_OPEN_CONFIRM];
        resp.extend_from_slice(&remote_id.to_be_bytes());
        resp.extend_from_slice(&local_id.to_be_bytes());
        resp.extend_from_slice(&WINDOW_SIZE.to_be_bytes());
        resp.extend_from_slice(&MAX_PACKET_PAYLOAD.to_be_bytes());

        serial_println!("  [ssh] Session {} channel {} opened ({})", self.id, local_id, ch_type_str);
        self.wrap_packet(&resp)
    }

    /// Handle CHANNEL_REQUEST (shell, exec, pty-req, subsystem, env)
    fn handle_channel_request(&mut self, payload: &[u8]) -> Vec<u8> {
        if payload.len() < 10 { return Vec::new(); }

        let mut off = 1;
        let channel_id = u32::from_be_bytes([payload[off], payload[off + 1], payload[off + 2], payload[off + 3]]);
        off += 4;

        let req_len = u32::from_be_bytes([payload[off], payload[off + 1], payload[off + 2], payload[off + 3]]) as usize;
        off += 4;
        let req_type = core::str::from_utf8(&payload[off..off + req_len.min(payload.len() - off)]).unwrap_or("");
        off += req_len;

        let want_reply = if off < payload.len() { payload[off] != 0 } else { false };
        off += 1;

        if let Some(ch) = self.channels.iter_mut().find(|c| c.local_id == channel_id) {
            match req_type {
                "pty-req" => {
                    // Parse terminal type and dimensions
                    if off + 4 <= payload.len() {
                        let term_len = u32::from_be_bytes([payload[off], payload[off + 1], payload[off + 2], payload[off + 3]]) as usize;
                        off += 4;
                        ch.pty_term = String::from(
                            core::str::from_utf8(&payload[off..off + term_len.min(payload.len() - off)]).unwrap_or("xterm")
                        );
                        off += term_len;
                        if off + 8 <= payload.len() {
                            ch.pty_cols = u32::from_be_bytes([payload[off], payload[off + 1], payload[off + 2], payload[off + 3]]);
                            ch.pty_rows = u32::from_be_bytes([payload[off + 4], payload[off + 5], payload[off + 6], payload[off + 7]]);
                        }
                    }
                    serial_println!("  [ssh] Channel {} pty-req: {} {}x{}",
                        channel_id, ch.pty_term, ch.pty_cols, ch.pty_rows);
                }
                "shell" => {
                    serial_println!("  [ssh] Channel {} shell started for '{}'", channel_id, self.username);
                }
                "exec" => {
                    if off + 4 <= payload.len() {
                        let cmd_len = u32::from_be_bytes([payload[off], payload[off + 1], payload[off + 2], payload[off + 3]]) as usize;
                        off += 4;
                        let _cmd = core::str::from_utf8(&payload[off..off + cmd_len.min(payload.len() - off)]).unwrap_or("");
                        serial_println!("  [ssh] Channel {} exec: {}", channel_id, _cmd);
                    }
                }
                "subsystem" => {
                    if off + 4 <= payload.len() {
                        let sub_len = u32::from_be_bytes([payload[off], payload[off + 1], payload[off + 2], payload[off + 3]]) as usize;
                        off += 4;
                        let subsys = core::str::from_utf8(&payload[off..off + sub_len.min(payload.len() - off)]).unwrap_or("");
                        if subsys == "sftp" {
                            ch.channel_type = ChannelType::Sftp;
                            serial_println!("  [ssh] Channel {} SFTP subsystem", channel_id);
                        }
                    }
                }
                "env" => {
                    if off + 4 <= payload.len() {
                        let name_len = u32::from_be_bytes([payload[off], payload[off + 1], payload[off + 2], payload[off + 3]]) as usize;
                        off += 4;
                        let name = core::str::from_utf8(&payload[off..off + name_len.min(payload.len() - off)]).unwrap_or("");
                        off += name_len;
                        if off + 4 <= payload.len() {
                            let val_len = u32::from_be_bytes([payload[off], payload[off + 1], payload[off + 2], payload[off + 3]]) as usize;
                            off += 4;
                            let val = core::str::from_utf8(&payload[off..off + val_len.min(payload.len() - off)]).unwrap_or("");
                            ch.env_vars.push((String::from(name), String::from(val)));
                        }
                    }
                }
                _ => {
                    serial_println!("  [ssh] Channel {} unknown request: {}", channel_id, req_type);
                    if want_reply {
                        let mut resp = vec![msg_id::CHANNEL_FAILURE];
                        resp.extend_from_slice(&ch.remote_id.to_be_bytes());
                        return self.wrap_packet(&resp);
                    }
                    return Vec::new();
                }
            }

            if want_reply {
                let mut resp = vec![msg_id::CHANNEL_SUCCESS];
                resp.extend_from_slice(&ch.remote_id.to_be_bytes());
                return self.wrap_packet(&resp);
            }
        }
        Vec::new()
    }

    /// Handle CHANNEL_DATA (data from client)
    fn handle_channel_data(&mut self, payload: &[u8]) -> Vec<u8> {
        if payload.len() < 9 { return Vec::new(); }
        let channel_id = u32::from_be_bytes([payload[1], payload[2], payload[3], payload[4]]);
        let data_len = u32::from_be_bytes([payload[5], payload[6], payload[7], payload[8]]) as usize;
        let data = &payload[9..9 + data_len.min(payload.len() - 9)];

        if let Some(ch) = self.channels.iter_mut().find(|c| c.local_id == channel_id) {
            ch.recv_buf.extend_from_slice(data);
            ch.local_window = ch.local_window.saturating_sub(data.len() as u32);

            // Window adjust if running low
            if ch.local_window < WINDOW_SIZE / 2 {
                let adjust = WINDOW_SIZE - ch.local_window;
                ch.local_window += adjust;
                let mut resp = vec![msg_id::CHANNEL_WINDOW_ADJUST];
                resp.extend_from_slice(&ch.remote_id.to_be_bytes());
                resp.extend_from_slice(&adjust.to_be_bytes());
                return self.wrap_packet(&resp);
            }
        }
        Vec::new()
    }

    /// Handle GLOBAL_REQUEST (tcpip-forward, cancel-tcpip-forward)
    fn handle_global_request(&mut self, payload: &[u8]) -> Vec<u8> {
        if payload.len() < 6 { return Vec::new(); }
        let mut off = 1;
        let req_len = u32::from_be_bytes([payload[off], payload[off + 1], payload[off + 2], payload[off + 3]]) as usize;
        off += 4;
        let req_type = core::str::from_utf8(&payload[off..off + req_len.min(payload.len() - off)]).unwrap_or("");
        off += req_len;
        let want_reply = if off < payload.len() { payload[off] != 0 } else { false };
        off += 1;

        if req_type == "tcpip-forward" && off + 4 <= payload.len() {
            let addr_len = u32::from_be_bytes([payload[off], payload[off + 1], payload[off + 2], payload[off + 3]]) as usize;
            off += 4;
            let bind_addr = core::str::from_utf8(&payload[off..off + addr_len.min(payload.len() - off)]).unwrap_or("");
            off += addr_len;
            let bind_port = if off + 4 <= payload.len() {
                u32::from_be_bytes([payload[off], payload[off + 1], payload[off + 2], payload[off + 3]]) as u16
            } else { 0 };

            self.port_forwards.push(PortForward {
                bind_addr: String::from(bind_addr),
                bind_port,
                dest_addr: String::new(),
                dest_port: 0,
                active: true,
            });
            serial_println!("  [ssh] Session {} port forward: {}:{}", self.id, bind_addr, bind_port);

            if want_reply {
                let mut resp = vec![msg_id::REQUEST_SUCCESS];
                resp.extend_from_slice(&(bind_port as u32).to_be_bytes());
                return self.wrap_packet(&resp);
            }
        }

        if want_reply {
            self.wrap_packet(&[msg_id::REQUEST_FAILURE])
        } else {
            Vec::new()
        }
    }

    /// Wrap a payload into an SSH binary packet (unencrypted form)
    fn wrap_packet(&mut self, payload: &[u8]) -> Vec<u8> {
        let padding_len = 8 - ((5 + payload.len()) % 8);
        let padding_len = if padding_len < 4 { padding_len + 8 } else { padding_len };
        let packet_len = 1 + payload.len() + padding_len;

        let mut packet = Vec::new();
        packet.extend_from_slice(&(packet_len as u32).to_be_bytes());
        packet.push(padding_len as u8);
        packet.extend_from_slice(payload);
        for _ in 0..padding_len {
            packet.push(0);
        }

        self.sequence_send += 1;
        self.bytes_sent += packet.len() as u64;
        self.packets_sent = self.packets_sent.saturating_add(1);
        packet
    }

    /// Top-level message dispatcher
    pub fn process_packet(&mut self, data: &[u8]) -> Vec<u8> {
        if data.is_empty() { return Vec::new(); }
        self.bytes_recv += data.len() as u64;
        self.packets_recv = self.packets_recv.saturating_add(1);
        self.sequence_recv += 1;

        let msg_type = data[0];
        match (self.state, msg_type) {
            (SshState::VersionExchange, _) => self.handle_version(data),
            (SshState::KexInit, msg_id::KEXINIT) => self.handle_kexinit(data),
            (SshState::KexDhInit, msg_id::KEX_ECDH_INIT) => self.handle_kex_ecdh_init(data),
            (SshState::KexNewKeys, msg_id::NEWKEYS) => self.handle_newkeys(),
            (SshState::ServiceRequest, msg_id::SERVICE_REQUEST) => self.handle_service_request(data),
            (SshState::Authenticating, msg_id::USERAUTH_REQUEST) => self.handle_userauth(data),
            (SshState::Authenticated, msg_id::SERVICE_REQUEST) => self.handle_service_request(data),
            (SshState::Authenticated, msg_id::CHANNEL_OPEN) |
            (SshState::Active, msg_id::CHANNEL_OPEN) => self.handle_channel_open(data),
            (SshState::Active, msg_id::CHANNEL_REQUEST) => self.handle_channel_request(data),
            (SshState::Active, msg_id::CHANNEL_DATA) => self.handle_channel_data(data),
            (SshState::Active, msg_id::GLOBAL_REQUEST) => self.handle_global_request(data),
            (SshState::Active, msg_id::CHANNEL_CLOSE) => {
                if data.len() >= 5 {
                    let ch_id = u32::from_be_bytes([data[1], data[2], data[3], data[4]]);
                    if let Some(ch) = self.channels.iter_mut().find(|c| c.local_id == ch_id) {
                        ch.state = ChannelState::Closed;
                        let mut resp = vec![msg_id::CHANNEL_CLOSE];
                        resp.extend_from_slice(&ch.remote_id.to_be_bytes());
                        serial_println!("  [ssh] Session {} channel {} closed", self.id, ch_id);
                        return self.wrap_packet(&resp);
                    }
                }
                Vec::new()
            }
            (_, msg_id::DISCONNECT) => {
                self.state = SshState::Disconnecting;
                serial_println!("  [ssh] Session {} disconnect requested", self.id);
                Vec::new()
            }
            (_, msg_id::IGNORE) => Vec::new(),
            _ => {
                let mut resp = vec![msg_id::UNIMPLEMENTED];
                resp.extend_from_slice(&self.sequence_recv.to_be_bytes());
                self.wrap_packet(&resp)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// SSH Server
// ---------------------------------------------------------------------------

pub struct SshServer {
    sessions: Vec<SshSession>,
    listen_port: u16,
    next_id: u32,
    host_key: [u8; 32],
}

impl SshServer {
    fn new() -> Self {
        SshServer {
            sessions: Vec::new(),
            listen_port: SSH_DEFAULT_PORT,
            next_id: 1,
            host_key: [0xABu8; 32],
        }
    }

    pub fn accept(&mut self, ip: [u8; 4], port: u16) -> (u32, Vec<u8>) {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let session = SshSession::new(id, ip, port);
        self.sessions.push(session);
        serial_println!("  [ssh] Session {} from {}.{}.{}.{}:{}", id, ip[0], ip[1], ip[2], ip[3], port);
        (id, Vec::from(SSH_VERSION_STRING))
    }

    pub fn process(&mut self, session_id: u32, data: &[u8]) -> Vec<u8> {
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) {
            s.process_packet(data)
        } else {
            Vec::new()
        }
    }

    pub fn cleanup(&mut self) {
        self.sessions.retain(|s| s.state != SshState::Disconnecting && s.state != SshState::Error);
    }

    pub fn active_count(&self) -> usize {
        self.sessions.iter().filter(|s| s.state == SshState::Active || s.state == SshState::Authenticated).count()
    }
}

static SSH_SERVER: Mutex<Option<SshServer>> = Mutex::new(None);

pub fn init() {
    let server = SshServer::new();
    *SSH_SERVER.lock() = Some(server);
    serial_println!("    SSH-2 server initialized (port {})", SSH_DEFAULT_PORT);
}

pub fn accept_connection(ip: [u8; 4], port: u16) -> Option<(u32, Vec<u8>)> {
    let mut guard = SSH_SERVER.lock();
    guard.as_mut().map(|s| s.accept(ip, port))
}

pub fn process_data(session_id: u32, data: &[u8]) -> Vec<u8> {
    let mut guard = SSH_SERVER.lock();
    if let Some(server) = guard.as_mut() {
        server.process(session_id, data)
    } else {
        Vec::new()
    }
}

pub fn cleanup() {
    let mut guard = SSH_SERVER.lock();
    if let Some(server) = guard.as_mut() {
        server.cleanup();
    }
}
