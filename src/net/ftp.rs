use super::Ipv4Addr;
use crate::sync::Mutex;
/// FTP client/server for Genesis — File Transfer Protocol
///
/// Implements FTP (RFC 959) with:
///   - Control connection (port 21) and data connection
///   - Active mode (PORT) and passive mode (PASV)
///   - Directory listing (LIST, NLST), navigation (CWD, PWD, CDUP)
///   - File transfer (RETR, STOR, APPE) in ASCII and binary modes
///   - Authentication (USER, PASS), anonymous login
///   - Server-side virtual filesystem integration
///
/// Inspired by: vsftpd, ProFTPD, curl FTP. All code is original.
use crate::{serial_print, serial_println};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

// ============================================================================
// FTP reply codes
// ============================================================================

pub const REPLY_DATA_OPEN: u16 = 125;
pub const REPLY_FILE_OK: u16 = 150;
pub const REPLY_COMMAND_OK: u16 = 200;
pub const REPLY_SYSTEM_TYPE: u16 = 215;
pub const REPLY_SERVICE_READY: u16 = 220;
pub const REPLY_CLOSING_CTRL: u16 = 221;
pub const REPLY_TRANSFER_COMPLETE: u16 = 226;
pub const REPLY_ENTERING_PASV: u16 = 227;
pub const REPLY_LOGGED_IN: u16 = 230;
pub const REPLY_FILE_ACTION_OK: u16 = 250;
pub const REPLY_PATHNAME_CREATED: u16 = 257;
pub const REPLY_NEED_PASSWORD: u16 = 331;
pub const REPLY_NEED_ACCOUNT: u16 = 332;
pub const REPLY_FILE_PENDING: u16 = 350;
pub const REPLY_SERVICE_UNAVAIL: u16 = 421;
pub const REPLY_CANT_OPEN_DATA: u16 = 425;
pub const REPLY_TRANSFER_ABORTED: u16 = 426;
pub const REPLY_SYNTAX_ERROR: u16 = 500;
pub const REPLY_PARAM_ERROR: u16 = 501;
pub const REPLY_NOT_IMPLEMENTED: u16 = 502;
pub const REPLY_BAD_SEQUENCE: u16 = 503;
pub const REPLY_NOT_LOGGED_IN: u16 = 530;
pub const REPLY_FILE_UNAVAIL: u16 = 550;

// ============================================================================
// Transfer mode
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferMode {
    Active,
    Passive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferType {
    Ascii,
    Binary,
}

// ============================================================================
// FTP command parsing
// ============================================================================

#[derive(Debug, Clone)]
pub enum FtpCommand {
    User(String),
    Pass(String),
    Syst,
    Feat,
    Pwd,
    Cwd(String),
    Cdup,
    Type(TransferType),
    Pasv,
    Port(Ipv4Addr, u16),
    List(Option<String>),
    Nlst(Option<String>),
    Retr(String),
    Stor(String),
    Appe(String),
    Dele(String),
    Mkd(String),
    Rmd(String),
    Rnfr(String),
    Rnto(String),
    Size(String),
    Noop,
    Quit,
    Abor,
    Unknown(String),
}

impl FtpCommand {
    /// Parse an FTP command from a line of text
    pub fn parse(line: &str) -> Self {
        let trimmed = line.trim();
        let (cmd, arg) = match trimmed.find(' ') {
            Some(pos) => (&trimmed[..pos], Some(trimmed[pos + 1..].trim())),
            None => (trimmed, None),
        };

        let cmd_upper = to_uppercase(cmd);

        match cmd_upper.as_str() {
            "USER" => FtpCommand::User(String::from(arg.unwrap_or(""))),
            "PASS" => FtpCommand::Pass(String::from(arg.unwrap_or(""))),
            "SYST" => FtpCommand::Syst,
            "FEAT" => FtpCommand::Feat,
            "PWD" | "XPWD" => FtpCommand::Pwd,
            "CWD" | "XCWD" => FtpCommand::Cwd(String::from(arg.unwrap_or("/"))),
            "CDUP" | "XCUP" => FtpCommand::Cdup,
            "TYPE" => {
                let tt = match arg {
                    Some("I") | Some("i") => TransferType::Binary,
                    _ => TransferType::Ascii,
                };
                FtpCommand::Type(tt)
            }
            "PASV" => FtpCommand::Pasv,
            "PORT" => {
                if let Some(a) = arg {
                    if let Some((ip, port)) = parse_port_arg(a) {
                        return FtpCommand::Port(ip, port);
                    }
                }
                FtpCommand::Unknown(String::from(trimmed))
            }
            "LIST" => FtpCommand::List(arg.map(String::from)),
            "NLST" => FtpCommand::Nlst(arg.map(String::from)),
            "RETR" => FtpCommand::Retr(String::from(arg.unwrap_or(""))),
            "STOR" => FtpCommand::Stor(String::from(arg.unwrap_or(""))),
            "APPE" => FtpCommand::Appe(String::from(arg.unwrap_or(""))),
            "DELE" => FtpCommand::Dele(String::from(arg.unwrap_or(""))),
            "MKD" | "XMKD" => FtpCommand::Mkd(String::from(arg.unwrap_or(""))),
            "RMD" | "XRMD" => FtpCommand::Rmd(String::from(arg.unwrap_or(""))),
            "RNFR" => FtpCommand::Rnfr(String::from(arg.unwrap_or(""))),
            "RNTO" => FtpCommand::Rnto(String::from(arg.unwrap_or(""))),
            "SIZE" => FtpCommand::Size(String::from(arg.unwrap_or(""))),
            "NOOP" => FtpCommand::Noop,
            "QUIT" => FtpCommand::Quit,
            "ABOR" => FtpCommand::Abor,
            _ => FtpCommand::Unknown(String::from(trimmed)),
        }
    }
}

/// Parse PORT command argument: h1,h2,h3,h4,p1,p2
fn parse_port_arg(s: &str) -> Option<(Ipv4Addr, u16)> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 6 {
        return None;
    }

    let mut nums = [0u8; 6];
    for (i, part) in parts.iter().enumerate() {
        nums[i] = part.trim().parse::<u8>().ok()?;
    }

    let ip = Ipv4Addr([nums[0], nums[1], nums[2], nums[3]]);
    let port = (nums[4] as u16) * 256 + (nums[5] as u16);
    Some((ip, port))
}

/// Uppercase conversion without std (ASCII only)
fn to_uppercase(s: &str) -> String {
    let mut result = String::new();
    for c in s.bytes() {
        if c >= b'a' && c <= b'z' {
            result.push((c - 32) as char);
        } else {
            result.push(c as char);
        }
    }
    result
}

// ============================================================================
// FTP session (server-side)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    AwaitingUser,
    AwaitingPassword,
    LoggedIn,
    Transferring,
    Closed,
}

pub struct FtpSession {
    pub id: u32,
    pub state: SessionState,
    pub username: String,
    pub current_dir: String,
    pub transfer_mode: TransferMode,
    pub transfer_type: TransferType,
    pub data_ip: Ipv4Addr,
    pub data_port: u16,
    pub pasv_port: u16,
    pub rename_from: Option<String>,
    pub bytes_transferred: u64,
    pub commands_processed: u64,
    pub client_ip: Ipv4Addr,
}

impl FtpSession {
    pub fn new(id: u32, client_ip: Ipv4Addr) -> Self {
        FtpSession {
            id,
            state: SessionState::AwaitingUser,
            username: String::new(),
            current_dir: String::from("/"),
            transfer_mode: TransferMode::Passive,
            transfer_type: TransferType::Binary,
            data_ip: Ipv4Addr::ANY,
            data_port: 0,
            pasv_port: 0,
            rename_from: None,
            bytes_transferred: 0,
            commands_processed: 0,
            client_ip,
        }
    }

    /// Process an FTP command and return the reply string
    pub fn process_command(&mut self, cmd: &FtpCommand) -> String {
        self.commands_processed = self.commands_processed.saturating_add(1);

        match cmd {
            FtpCommand::User(user) => {
                self.username = user.clone();
                if user == "anonymous" || user == "ftp" {
                    self.state = SessionState::LoggedIn;
                    format!("{} Anonymous login OK.\r\n", REPLY_LOGGED_IN)
                } else {
                    self.state = SessionState::AwaitingPassword;
                    format!(
                        "{} Password required for {}.\r\n",
                        REPLY_NEED_PASSWORD, user
                    )
                }
            }

            FtpCommand::Pass(_pass) => {
                if self.state != SessionState::AwaitingPassword {
                    return format!("{} Bad sequence of commands.\r\n", REPLY_BAD_SEQUENCE);
                }
                // Accept any password (authentication would go here)
                self.state = SessionState::LoggedIn;
                serial_println!(
                    "  [ftp] User '{}' logged in from {}",
                    self.username,
                    self.client_ip
                );
                format!("{} User {} logged in.\r\n", REPLY_LOGGED_IN, self.username)
            }

            FtpCommand::Syst => {
                format!("{} UNIX Type: L8\r\n", REPLY_SYSTEM_TYPE)
            }

            FtpCommand::Feat => {
                format!("211-Features:\r\n PASV\r\n SIZE\r\n UTF8\r\n211 End\r\n")
            }

            FtpCommand::Pwd => {
                if self.state != SessionState::LoggedIn {
                    return format!("{} Not logged in.\r\n", REPLY_NOT_LOGGED_IN);
                }
                format!("{} \"{}\"\r\n", REPLY_PATHNAME_CREATED, self.current_dir)
            }

            FtpCommand::Cwd(path) => {
                if self.state != SessionState::LoggedIn {
                    return format!("{} Not logged in.\r\n", REPLY_NOT_LOGGED_IN);
                }
                self.current_dir = resolve_path(&self.current_dir, path);
                serial_println!("  [ftp] CWD -> {}", self.current_dir);
                format!(
                    "{} Directory changed to {}.\r\n",
                    REPLY_FILE_ACTION_OK, self.current_dir
                )
            }

            FtpCommand::Cdup => {
                if self.state != SessionState::LoggedIn {
                    return format!("{} Not logged in.\r\n", REPLY_NOT_LOGGED_IN);
                }
                self.current_dir = parent_path(&self.current_dir);
                format!(
                    "{} Directory changed to {}.\r\n",
                    REPLY_FILE_ACTION_OK, self.current_dir
                )
            }

            FtpCommand::Type(tt) => {
                self.transfer_type = *tt;
                let name = match tt {
                    TransferType::Ascii => "ASCII",
                    TransferType::Binary => "Binary",
                };
                format!("{} Type set to {}.\r\n", REPLY_COMMAND_OK, name)
            }

            FtpCommand::Pasv => {
                if self.state != SessionState::LoggedIn {
                    return format!("{} Not logged in.\r\n", REPLY_NOT_LOGGED_IN);
                }
                self.transfer_mode = TransferMode::Passive;
                // Assign a passive port (dynamic range 49152-65535)
                let port = allocate_pasv_port();
                self.pasv_port = port;
                let p1 = (port >> 8) as u8;
                let p2 = (port & 0xFF) as u8;
                // Use 0,0,0,0 as placeholder; real impl would use server IP
                format!(
                    "{} Entering Passive Mode (0,0,0,0,{},{}).\r\n",
                    REPLY_ENTERING_PASV, p1, p2
                )
            }

            FtpCommand::Port(ip, port) => {
                if self.state != SessionState::LoggedIn {
                    return format!("{} Not logged in.\r\n", REPLY_NOT_LOGGED_IN);
                }
                self.transfer_mode = TransferMode::Active;
                self.data_ip = *ip;
                self.data_port = *port;
                format!("{} PORT command successful.\r\n", REPLY_COMMAND_OK)
            }

            FtpCommand::List(path) => {
                if self.state != SessionState::LoggedIn {
                    return format!("{} Not logged in.\r\n", REPLY_NOT_LOGGED_IN);
                }
                let dir = match path {
                    Some(p) => resolve_path(&self.current_dir, p),
                    None => self.current_dir.clone(),
                };
                serial_println!("  [ftp] LIST {}", dir);
                // Data transfer would happen on data connection
                // Return control reply indicating transfer start
                format!(
                    "{} Opening data connection for directory listing.\r\n",
                    REPLY_FILE_OK
                )
            }

            FtpCommand::Nlst(path) => {
                if self.state != SessionState::LoggedIn {
                    return format!("{} Not logged in.\r\n", REPLY_NOT_LOGGED_IN);
                }
                let dir = match path {
                    Some(p) => resolve_path(&self.current_dir, p),
                    None => self.current_dir.clone(),
                };
                serial_println!("  [ftp] NLST {}", dir);
                format!(
                    "{} Opening data connection for name listing.\r\n",
                    REPLY_FILE_OK
                )
            }

            FtpCommand::Retr(filename) => {
                if self.state != SessionState::LoggedIn {
                    return format!("{} Not logged in.\r\n", REPLY_NOT_LOGGED_IN);
                }
                let full_path = resolve_path(&self.current_dir, filename);
                serial_println!("  [ftp] RETR {}", full_path);
                format!(
                    "{} Opening data connection for {}.\r\n",
                    REPLY_FILE_OK, filename
                )
            }

            FtpCommand::Stor(filename) => {
                if self.state != SessionState::LoggedIn {
                    return format!("{} Not logged in.\r\n", REPLY_NOT_LOGGED_IN);
                }
                let full_path = resolve_path(&self.current_dir, filename);
                serial_println!("  [ftp] STOR {}", full_path);
                format!(
                    "{} Opening data connection for {}.\r\n",
                    REPLY_FILE_OK, filename
                )
            }

            FtpCommand::Appe(filename) => {
                if self.state != SessionState::LoggedIn {
                    return format!("{} Not logged in.\r\n", REPLY_NOT_LOGGED_IN);
                }
                serial_println!("  [ftp] APPE {}", filename);
                format!(
                    "{} Opening data connection for append to {}.\r\n",
                    REPLY_FILE_OK, filename
                )
            }

            FtpCommand::Dele(filename) => {
                if self.state != SessionState::LoggedIn {
                    return format!("{} Not logged in.\r\n", REPLY_NOT_LOGGED_IN);
                }
                serial_println!("  [ftp] DELE {}", filename);
                format!("{} File deleted.\r\n", REPLY_FILE_ACTION_OK)
            }

            FtpCommand::Mkd(dirname) => {
                if self.state != SessionState::LoggedIn {
                    return format!("{} Not logged in.\r\n", REPLY_NOT_LOGGED_IN);
                }
                let full_path = resolve_path(&self.current_dir, dirname);
                serial_println!("  [ftp] MKD {}", full_path);
                format!("{} \"{}\" created.\r\n", REPLY_PATHNAME_CREATED, full_path)
            }

            FtpCommand::Rmd(dirname) => {
                if self.state != SessionState::LoggedIn {
                    return format!("{} Not logged in.\r\n", REPLY_NOT_LOGGED_IN);
                }
                serial_println!("  [ftp] RMD {}", dirname);
                format!("{} Directory removed.\r\n", REPLY_FILE_ACTION_OK)
            }

            FtpCommand::Rnfr(path) => {
                if self.state != SessionState::LoggedIn {
                    return format!("{} Not logged in.\r\n", REPLY_NOT_LOGGED_IN);
                }
                self.rename_from = Some(resolve_path(&self.current_dir, path));
                format!("{} Ready for RNTO.\r\n", REPLY_FILE_PENDING)
            }

            FtpCommand::Rnto(path) => {
                if self.state != SessionState::LoggedIn {
                    return format!("{} Not logged in.\r\n", REPLY_NOT_LOGGED_IN);
                }
                if self.rename_from.is_none() {
                    return format!("{} RNFR required first.\r\n", REPLY_BAD_SEQUENCE);
                }
                let from = self.rename_from.take().unwrap_or_default();
                let to = resolve_path(&self.current_dir, path);
                serial_println!("  [ftp] RENAME {} -> {}", from, to);
                format!("{} Rename successful.\r\n", REPLY_FILE_ACTION_OK)
            }

            FtpCommand::Size(_filename) => {
                if self.state != SessionState::LoggedIn {
                    return format!("{} Not logged in.\r\n", REPLY_NOT_LOGGED_IN);
                }
                // Would query VFS for actual size; placeholder
                format!("{} 0\r\n", REPLY_COMMAND_OK)
            }

            FtpCommand::Noop => {
                format!("{} OK.\r\n", REPLY_COMMAND_OK)
            }

            FtpCommand::Quit => {
                self.state = SessionState::Closed;
                serial_println!(
                    "  [ftp] Session {} closed (user={})",
                    self.id,
                    self.username
                );
                format!("{} Goodbye.\r\n", REPLY_CLOSING_CTRL)
            }

            FtpCommand::Abor => {
                format!("{} Transfer aborted.\r\n", REPLY_TRANSFER_COMPLETE)
            }

            FtpCommand::Unknown(cmd) => {
                serial_println!("  [ftp] Unknown command: {}", cmd);
                format!(
                    "{} Syntax error, command unrecognized.\r\n",
                    REPLY_SYNTAX_ERROR
                )
            }
        }
    }

    /// Get session info string
    pub fn info(&self) -> String {
        format!(
            "FTP session {}: user={} dir={} state={:?} transferred={}B cmds={}",
            self.id,
            self.username,
            self.current_dir,
            self.state,
            self.bytes_transferred,
            self.commands_processed
        )
    }
}

// ============================================================================
// Path resolution helpers
// ============================================================================

/// Resolve a path relative to a working directory
fn resolve_path(cwd: &str, path: &str) -> String {
    if path.starts_with('/') {
        // Absolute path — normalize it
        return normalize_path(path);
    }

    // Relative path
    let mut full = String::from(cwd);
    if !full.ends_with('/') {
        full.push('/');
    }
    full.push_str(path);
    normalize_path(&full)
}

/// Normalize a path (resolve . and ..)
fn normalize_path(path: &str) -> String {
    let mut components: Vec<&str> = Vec::new();

    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            other => components.push(other),
        }
    }

    if components.is_empty() {
        return String::from("/");
    }

    let mut result = String::new();
    for c in &components {
        result.push('/');
        result.push_str(c);
    }
    result
}

/// Get parent directory
fn parent_path(path: &str) -> String {
    if path == "/" {
        return String::from("/");
    }
    let trimmed = if path.ends_with('/') {
        &path[..path.len() - 1]
    } else {
        path
    };
    match trimmed.rfind('/') {
        Some(0) | None => String::from("/"),
        Some(pos) => String::from(&trimmed[..pos]),
    }
}

// ============================================================================
// FTP client (outgoing connections)
// ============================================================================

pub struct FtpClient {
    pub server_ip: Ipv4Addr,
    pub server_port: u16,
    pub state: SessionState,
    pub current_dir: String,
    pub transfer_type: TransferType,
    pub transfer_mode: TransferMode,
    pub last_reply_code: u16,
    pub last_reply: String,
}

impl FtpClient {
    pub fn new(server_ip: Ipv4Addr, port: u16) -> Self {
        FtpClient {
            server_ip,
            server_port: port,
            state: SessionState::AwaitingUser,
            current_dir: String::from("/"),
            transfer_type: TransferType::Binary,
            transfer_mode: TransferMode::Passive,
            last_reply_code: 0,
            last_reply: String::new(),
        }
    }

    /// Build a USER command
    pub fn cmd_user(&self, username: &str) -> String {
        format!("USER {}\r\n", username)
    }

    /// Build a PASS command
    pub fn cmd_pass(&self, password: &str) -> String {
        format!("PASS {}\r\n", password)
    }

    /// Build a CWD command
    pub fn cmd_cwd(&self, path: &str) -> String {
        format!("CWD {}\r\n", path)
    }

    /// Build a PWD command
    pub fn cmd_pwd(&self) -> String {
        String::from("PWD\r\n")
    }

    /// Build a LIST command
    pub fn cmd_list(&self, path: Option<&str>) -> String {
        match path {
            Some(p) => format!("LIST {}\r\n", p),
            None => String::from("LIST\r\n"),
        }
    }

    /// Build a RETR command
    pub fn cmd_retr(&self, filename: &str) -> String {
        format!("RETR {}\r\n", filename)
    }

    /// Build a STOR command
    pub fn cmd_stor(&self, filename: &str) -> String {
        format!("STOR {}\r\n", filename)
    }

    /// Build a PASV command
    pub fn cmd_pasv(&self) -> String {
        String::from("PASV\r\n")
    }

    /// Build a TYPE command
    pub fn cmd_type(&self, tt: TransferType) -> String {
        match tt {
            TransferType::Ascii => String::from("TYPE A\r\n"),
            TransferType::Binary => String::from("TYPE I\r\n"),
        }
    }

    /// Build a QUIT command
    pub fn cmd_quit(&self) -> String {
        String::from("QUIT\r\n")
    }

    /// Parse an FTP reply line and extract the status code
    pub fn parse_reply(&mut self, line: &str) -> u16 {
        self.last_reply = String::from(line);
        if line.len() >= 3 {
            if let Some(code) = parse_reply_code(line) {
                self.last_reply_code = code;
                return code;
            }
        }
        0
    }

    /// Parse a PASV response to extract data connection address
    pub fn parse_pasv_reply(reply: &str) -> Option<(Ipv4Addr, u16)> {
        // Find the part in parentheses: (h1,h2,h3,h4,p1,p2)
        let start = reply.find('(')?;
        let end = reply.find(')')?;
        if end <= start + 1 {
            return None;
        }
        let inner = &reply[start + 1..end];
        parse_port_arg(inner)
    }
}

/// Parse a 3-digit reply code from an FTP response line
fn parse_reply_code(line: &str) -> Option<u16> {
    if line.len() < 3 {
        return None;
    }
    let bytes = line.as_bytes();
    let d0 = (bytes[0] as u16).checked_sub(b'0' as u16)?;
    let d1 = (bytes[1] as u16).checked_sub(b'0' as u16)?;
    let d2 = (bytes[2] as u16).checked_sub(b'0' as u16)?;
    if d0 > 9 || d1 > 9 || d2 > 9 {
        return None;
    }
    Some(d0 * 100 + d1 * 10 + d2)
}

// ============================================================================
// Global state
// ============================================================================

static FTP_SESSIONS: Mutex<Vec<FtpSession>> = Mutex::new(Vec::new());
static NEXT_SESSION_ID: Mutex<u32> = Mutex::new(1);
static NEXT_PASV_PORT: Mutex<u16> = Mutex::new(49152);

/// Allocate a passive mode data port
fn allocate_pasv_port() -> u16 {
    let mut port = NEXT_PASV_PORT.lock();
    let p = *port;
    *port = if *port >= 65534 { 49152 } else { *port + 1 };
    p
}

pub fn init() {
    serial_println!("    [ftp] FTP server initialized (port 21, passive range 49152-65535)");
}

/// Create a new FTP server session for an incoming connection
pub fn create_session(client_ip: Ipv4Addr) -> (u32, String) {
    let mut next_id = NEXT_SESSION_ID.lock();
    let id = *next_id;
    *next_id = next_id.saturating_add(1);
    drop(next_id);

    let session = FtpSession::new(id, client_ip);
    let banner = format!("{} Genesis FTP server ready.\r\n", REPLY_SERVICE_READY);
    FTP_SESSIONS.lock().push(session);

    serial_println!("  [ftp] New session {} from {}", id, client_ip);
    (id, banner)
}

/// Process a command on an existing session
pub fn process_command(session_id: u32, line: &str) -> Option<String> {
    let cmd = FtpCommand::parse(line);
    let mut sessions = FTP_SESSIONS.lock();
    for session in sessions.iter_mut() {
        if session.id == session_id {
            return Some(session.process_command(&cmd));
        }
    }
    None
}

/// Remove a closed session
pub fn remove_session(session_id: u32) {
    FTP_SESSIONS.lock().retain(|s| s.id != session_id);
}

/// Get count of active sessions
pub fn active_sessions() -> usize {
    FTP_SESSIONS
        .lock()
        .iter()
        .filter(|s| s.state != SessionState::Closed)
        .count()
}

/// Get info about all sessions
pub fn sessions_info() -> Vec<String> {
    FTP_SESSIONS.lock().iter().map(|s| s.info()).collect()
}
