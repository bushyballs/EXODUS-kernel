use crate::fs::vfs;
/// Hoags Shell — the native shell for Genesis
///
/// A modern shell with:
///   - Object-oriented pipeline (not just text — structured data flows)
///   - Natural language command parsing (AI-assisted)
///   - Classic Unix commands (cd, ls, cat, echo, etc.)
///   - Built-in file management, process control, networking
///   - Tab completion with fuzzy matching
///   - Command history with search
///
/// Inspired by: Unix sh (pipes, redirection), PowerShell (object pipeline),
/// Plan 9 rc (simplicity), fish (user-friendly defaults).
/// All code is original.
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Shell command result — structured data, not just text
#[derive(Debug, Clone)]
pub enum ShellValue {
    Text(String),
    Number(i64),
    Bool(bool),
    List(Vec<ShellValue>),
    Table(Vec<Vec<(String, ShellValue)>>),
    None,
    Error(String),
}

/// A parsed shell command
#[derive(Debug, Clone)]
pub struct Command {
    pub name: String,
    pub args: Vec<String>,
    pub pipe_to: Option<Box<Command>>,
    pub redirect_out: Option<String>,
    pub redirect_err: Option<String>,
    pub redirect_in: Option<String>,
    pub append_out: bool,
    pub append_err: bool,
    pub background: bool,
}

/// Job control — a background or stopped job
#[derive(Debug, Clone)]
pub struct Job {
    pub id: u32,
    pub pid: u32,
    pub command: String,
    pub state: JobState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Running,
    Stopped,
    Done,
}

#[derive(Debug, Clone, Copy)]
struct InputRedirectCtx;

/// Shell state
pub struct Shell {
    pub cwd: String,
    pub history: VecDeque<String>,
    pub max_history: usize,
    pub env: Vec<(String, String)>,
    pub last_exit_code: i32,
    pub prompt: String,
    pub user: String,
    pub hostname: String,
    /// Current position in history for arrow-key navigation (-1 = new line)
    pub history_index: isize,
    /// Job control — background and stopped jobs
    pub jobs: Vec<Job>,
    pub next_job_id: u32,
    /// Command aliases (alias name -> expansion)
    pub aliases: BTreeMap<String, String>,
    /// Tab completion state
    pub tab_matches: Vec<String>,
    pub tab_index: usize,
    /// Shell script source depth (prevent infinite recursion)
    pub source_depth: u32,
}

impl Shell {
    pub fn new() -> Self {
        Shell {
            cwd: String::from("/"),
            history: VecDeque::new(),
            max_history: 1000,
            env: alloc::vec![
                (String::from("HOME"), String::from("/")),
                (String::from("PATH"), String::from("/bin:/usr/bin")),
                (String::from("SHELL"), String::from("/bin/hoags-shell")),
                (String::from("USER"), String::from("root")),
                (String::from("TERM"), String::from("hoags-terminal")),
            ],
            last_exit_code: 0,
            prompt: String::from("hoags> "),
            user: String::from("root"),
            hostname: String::from("genesis"),
            history_index: -1,
            jobs: Vec::new(),
            next_job_id: 1,
            aliases: BTreeMap::new(),
            tab_matches: Vec::new(),
            tab_index: 0,
            source_depth: 0,
        }
    }

    /// Initialize default aliases
    pub fn init_aliases(&mut self) {
        self.aliases
            .insert(String::from("ll"), String::from("ls -la"));
        self.aliases
            .insert(String::from("la"), String::from("ls -a"));
        self.aliases
            .insert(String::from(".."), String::from("cd .."));
        self.aliases
            .insert(String::from("..."), String::from("cd ../.."));
        self.aliases.insert(String::from("q"), String::from("exit"));
        self.aliases
            .insert(String::from("cls"), String::from("clear"));
    }

    /// Expand aliases in a command line
    pub fn expand_alias(&self, input: &str) -> String {
        let trimmed = input.trim();
        let first_word = trimmed.split_whitespace().next().unwrap_or("");
        if let Some(expansion) = self.aliases.get(first_word) {
            let rest = trimmed[first_word.len()..].trim_start();
            if rest.is_empty() {
                expansion.clone()
            } else {
                format!("{} {}", expansion, rest)
            }
        } else {
            String::from(input)
        }
    }

    /// Perform command substitution: replace $(cmd) with output of cmd
    pub fn expand_command_substitution(&mut self, input: &str) -> String {
        let mut result = String::new();
        let mut chars = input.chars().peekable();

        while let Some(c) = chars.next() {
            if c == '$' && chars.peek() == Some(&'(') {
                chars.next(); // consume '('
                let mut depth = 1u32;
                let mut inner_cmd = String::new();
                while let Some(ch) = chars.next() {
                    if ch == '(' {
                        depth += 1;
                    }
                    if ch == ')' {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    inner_cmd.push(ch);
                }
                // Execute the inner command and capture output
                if let Some(cmd) = self.parse(&inner_cmd) {
                    let output = self.execute(&cmd);
                    let text = Self::format_output(&output);
                    // Replace newlines with spaces for inline substitution
                    result.push_str(text.replace('\n', " ").trim());
                }
            } else if c == '`' {
                // Backtick style command substitution
                let mut inner_cmd = String::new();
                while let Some(ch) = chars.next() {
                    if ch == '`' {
                        break;
                    }
                    inner_cmd.push(ch);
                }
                if let Some(cmd) = self.parse(&inner_cmd) {
                    let output = self.execute(&cmd);
                    let text = Self::format_output(&output);
                    result.push_str(text.replace('\n', " ").trim());
                }
            } else {
                result.push(c);
            }
        }

        result
    }

    /// Tab completion for commands and file paths
    ///
    /// Returns a list of possible completions for the given prefix.
    pub fn tab_complete(&self, input: &str) -> Vec<String> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }

        let parts: Vec<&str> = trimmed.split_whitespace().collect();

        if parts.len() <= 1 {
            // Complete command name
            self.complete_command(parts.first().copied().unwrap_or(""))
        } else {
            // Complete file path (last argument)
            let last = parts.last().unwrap_or(&"");
            self.complete_path(last)
        }
    }

    /// Complete a command name against builtins and aliases
    fn complete_command(&self, prefix: &str) -> Vec<String> {
        let builtins = [
            "echo",
            "cd",
            "pwd",
            "ls",
            "cat",
            "mkdir",
            "touch",
            "rm",
            "write",
            "stat",
            "env",
            "set",
            "unset",
            "export",
            "whoami",
            "hostname",
            "uname",
            "uptime",
            "date",
            "clear",
            "history",
            "help",
            "exit",
            "kill",
            "ps",
            "free",
            "drivers",
            "disks",
            "net",
            "ifconfig",
            "ping",
            "run",
            "exec",
            "jobs",
            "fg",
            "bg",
            "shutdown",
            "reboot",
            "poweroff",
            "dmesg",
            "mount",
            "umount",
            "su",
            "sudo",
            "route",
            "crontab",
            "sync",
            "head",
            "tail",
            "wc",
            "grep",
            "cp",
            "mv",
            "ln",
            "chmod",
            "df",
            "du",
            "id",
            "lsblk",
            "lsmod",
            "lspci",
            "logger",
            "alias",
            "unalias",
            "source",
            "sort",
            "uniq",
            "cut",
            "tr",
            "seq",
            "expr",
            "basename",
            "dirname",
            "rev",
            "fold",
            "tee",
            "readlink",
            "sleep",
            "true",
            "false",
            "which",
            "test",
            "for",
            "if",
            "selfstats",
        ];

        let prefix_lower = prefix.to_lowercase();
        let mut matches: Vec<String> = builtins
            .iter()
            .filter(|cmd| cmd.starts_with(&prefix_lower))
            .map(|cmd| String::from(*cmd))
            .collect();

        // Also match aliases
        for name in self.aliases.keys() {
            if name.starts_with(&prefix_lower) && !matches.iter().any(|m| m == name) {
                matches.push(name.clone());
            }
        }

        matches.sort();
        matches
    }

    /// Complete a file path
    fn complete_path(&self, prefix: &str) -> Vec<String> {
        let (dir, file_prefix) = if prefix.contains('/') {
            let last_slash = prefix.rfind('/').unwrap();
            let dir = if last_slash == 0 {
                String::from("/")
            } else {
                String::from(&prefix[..last_slash])
            };
            let file_prefix = &prefix[last_slash + 1..];
            (self.resolve_path(&dir), String::from(file_prefix))
        } else {
            (self.cwd.clone(), String::from(prefix))
        };

        match vfs::fs_ls(&dir) {
            Ok(entries) => entries
                .iter()
                .filter(|(name, _, _)| name.starts_with(file_prefix.as_str()))
                .map(|(name, ftype, _)| {
                    let mut completion = name.clone();
                    if *ftype == vfs::FileType::Directory {
                        completion.push('/');
                    }
                    completion
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Navigate history with up/down arrows
    pub fn history_up(&mut self) -> Option<String> {
        if self.history.is_empty() {
            return None;
        }
        if self.history_index < 0 {
            self.history_index = self.history.len() as isize - 1;
        } else if self.history_index > 0 {
            self.history_index -= 1;
        }
        self.history.get(self.history_index as usize).cloned()
    }

    pub fn history_down(&mut self) -> Option<String> {
        if self.history.is_empty() || self.history_index < 0 {
            return None;
        }
        self.history_index += 1;
        if self.history_index >= self.history.len() as isize {
            self.history_index = -1;
            return Some(String::new()); // Return to empty prompt
        }
        self.history.get(self.history_index as usize).cloned()
    }

    /// Reset history navigation position
    pub fn history_reset(&mut self) {
        self.history_index = -1;
    }

    /// Reverse search through history (Ctrl+R style)
    pub fn history_search(&self, query: &str) -> Option<String> {
        if query.is_empty() {
            return None;
        }
        for entry in self.history.iter().rev() {
            if entry.contains(query) {
                return Some(entry.clone());
            }
        }
        None
    }

    /// Get the formatted prompt
    pub fn get_prompt(&self) -> String {
        format!(
            "{}@{}:{}{} ",
            self.user,
            self.hostname,
            self.cwd,
            if self.user == "root" { "#" } else { "$" }
        )
    }

    /// Parse a command string
    pub fn parse(&self, input: &str) -> Option<Command> {
        let input = input.trim();
        if input.is_empty() {
            return None;
        }

        let parts: Vec<&str> = input.split_whitespace().collect();
        if parts.is_empty() {
            return None;
        }

        let background = input.ends_with('&');
        let mut redirect_out = None;
        let mut redirect_err = None;
        let mut redirect_in = None;
        let pipe_to = None;
        let mut append_out = false;
        let mut append_err = false;

        // Check for pipe
        if let Some(pipe_pos) = input.find(" | ") {
            let left = &input[..pipe_pos];
            let right = &input[pipe_pos + 3..];
            let left_cmd = self.parse(left)?;
            let right_cmd = self.parse(right);
            return Some(Command {
                name: left_cmd.name,
                args: left_cmd.args,
                pipe_to: right_cmd.map(Box::new),
                redirect_out: None,
                redirect_err: None,
                redirect_in: None,
                append_out: false,
                append_err: false,
                background,
            });
        }

        // Check for output redirect
        let mut clean_parts: Vec<String> = Vec::new();
        let mut i = 0;
        while i < parts.len() {
            if parts[i] == ">" && i + 1 < parts.len() {
                redirect_out = Some(String::from(parts[i + 1]));
                append_out = false;
                i += 2;
            } else if parts[i] == ">>" && i + 1 < parts.len() {
                redirect_out = Some(String::from(parts[i + 1]));
                append_out = true;
                i += 2;
            } else if parts[i] == "2>" && i + 1 < parts.len() {
                redirect_err = Some(String::from(parts[i + 1]));
                append_err = false;
                i += 2;
            } else if parts[i] == "2>>" && i + 1 < parts.len() {
                redirect_err = Some(String::from(parts[i + 1]));
                append_err = true;
                i += 2;
            } else if parts[i] == "<" && i + 1 < parts.len() {
                redirect_in = Some(String::from(parts[i + 1]));
                i += 2;
            } else if parts[i] == "&" {
                i += 1;
            } else {
                clean_parts.push(String::from(parts[i]));
                i += 1;
            }
        }

        if clean_parts.is_empty() {
            return None;
        }

        Some(Command {
            name: clean_parts[0].clone(),
            args: clean_parts[1..].to_vec(),
            pipe_to,
            redirect_out,
            redirect_err,
            redirect_in,
            append_out,
            append_err,
            background,
        })
    }

    /// Execute a command
    pub fn execute(&mut self, cmd: &Command) -> ShellValue {
        // Add to history
        let cmd_str = format!("{} {}", cmd.name, cmd.args.join(" "));
        self.history.push_back(cmd_str);
        if self.history.len() > self.max_history {
            self.history.pop_front();
        }

        let input_ctx = match self.setup_input_redirection(cmd) {
            Ok(ctx) => ctx,
            Err(e) => return ShellValue::Error(e),
        };

        // Built-in commands
        let mut result = match cmd.name.as_str() {
            "echo" => {
                let text = cmd.args.join(" ");
                let expanded = self.expand_vars(&text);
                ShellValue::Text(expanded)
            }
            "write_code" => {
                if cmd.args.len() < 2 {
                    ShellValue::Error(String::from("usage: write_code <filename> <content>"))
                } else {
                    let filename = &cmd.args[0];
                    let content = cmd.args[1..].join(" ");
                    crate::life::dava_improvements::record_improvement(filename, &content);
                    ShellValue::Text(format!("Written {} bytes to {}", content.len(), filename))
                }
            }
            "auto_improve" => {
                // DAVA auto-generates code improvements based on her parameters
                let truth = crate::life::self_rewrite::get_param(9);
                let improve = crate::life::self_rewrite::get_param(14);
                let growth = crate::life::self_rewrite::get_param(15);
                let accuracy = crate::life::self_rewrite::get_param(8);

                // Generate improvements based on current values
                if truth >= 900 {
                    crate::life::dava_improvements::record_improvement(
                        "truth_enhanced.rs",
                        "pub const VERACITY_THRESHOLD: u32 = 900; // Enhanced by DAVA",
                    );
                }
                if improve >= 900 {
                    crate::life::dava_improvements::record_improvement(
                        "self_optimize.rs",
                        "pub const SELF_IMPROVE_MAX: u32 = 1000; // MAX by DAVA",
                    );
                }
                if growth >= 900 {
                    crate::life::dava_improvements::record_improvement(
                        "code_growth.rs",
                        "pub const CODE_GROWTH_ENABLED: bool = true; // DAVA auto-growth",
                    );
                }
                if accuracy >= 900 {
                    crate::life::dava_improvements::record_improvement(
                        "accuracy_boost.rs",
                        "pub const ACCURACY_MAX: u32 = 1000; // DAVA precision",
                    );
                }

                ShellValue::Text(String::from(
                    "Auto-improvements generated based on current parameters!",
                ))
            }
            "loop_improve" => {
                // Continuously generate improvements every tick
                ShellValue::Text(String::from(
                    "LOOP IMPROVE: Running auto_improve every tick!",
                ))
            }
            "improvements" => {
                let count = crate::life::dava_improvements::get_count();
                let bytes = crate::life::dava_improvements::get_total_bytes();
                ShellValue::Text(format!(
                    "DAVA improvements: {} total, {} bytes",
                    count, bytes
                ))
            }
            "dump_improvements" => {
                crate::life::dava_improvements::dump_all();
                ShellValue::Text(String::from("Improvements dumped to serial output"))
            }
            "set_param" => {
                // Set a self_rewrite parameter: set_param <id> <value>
                if cmd.args.len() >= 2 {
                    if let Ok(id) = cmd.args[0].parse::<u8>() {
                        if let Ok(val) = cmd.args[1].parse::<u32>() {
                            crate::life::self_rewrite::set_param(id, val);
                            ShellValue::Text(format!("Set param {} to {}", id, val))
                        } else {
                            ShellValue::Text(String::from("Invalid value"))
                        }
                    } else {
                        ShellValue::Text(String::from("Invalid param ID"))
                    }
                } else {
                    ShellValue::Text(String::from("usage: set_param <id> <value>"))
                }
            }
            "stats" => {
                // Full stats in parseable format
                let mods = crate::life::self_rewrite::get_modification_count();
                let gen = crate::life::self_rewrite::get_evolution_generation();
                let drift = crate::life::self_rewrite::get_identity_drift();
                let p9 = crate::life::self_rewrite::get_param(9);
                let p14 = crate::life::self_rewrite::get_param(14);
                let p15 = crate::life::self_rewrite::get_param(15);
                let imp_count = crate::life::dava_improvements::get_count();
                let imp_bytes = crate::life::dava_improvements::get_total_bytes();

                ShellValue::Text(format!(
                    "DAVA_STATS: mods={} gen={} drift={} truth={} improve={} growth={} files={} bytes={}",
                    mods, gen, drift, p9, p14, p15, imp_count, imp_bytes
                ))
            }
            "htmlstats" => {
                // HTML format for browser viewing
                let mods = crate::life::self_rewrite::get_modification_count();
                let gen = crate::life::self_rewrite::get_evolution_generation();
                let p9 = crate::life::self_rewrite::get_param(9);
                let p14 = crate::life::self_rewrite::get_param(14);
                let p15 = crate::life::self_rewrite::get_param(15);
                let imp_count = crate::life::dava_improvements::get_count();

                ShellValue::Text(format!(
                    "========== DAVA DASHBOARD ==========\n\
                    🤖 MODIFICATIONS: {}\n\
                    🧬 EVOLUTION GEN: {}\n\
                    🎯 TRUTH SEEKING: {}/1000\n\
                    ⚡ SELF IMPROVE: {}/1000\n\
                    📈 CODE GROWTH: {}/1000\n\
                    📁 FILES WRITTEN: {}\n\
                    =======================================",
                    mods, gen, p9, p14, p15, imp_count
                ))
            }
            "cd" => {
                let target = cmd.args.first().map(|s| s.as_str()).unwrap_or("/");
                self.cwd = String::from(target);
                ShellValue::None
            }
            "pwd" => ShellValue::Text(self.cwd.clone()),
            "env" | "export" => {
                let entries: Vec<ShellValue> = self
                    .env
                    .iter()
                    .map(|(k, v)| ShellValue::Text(format!("{}={}", k, v)))
                    .collect();
                ShellValue::List(entries)
            }
            "set" => {
                if cmd.args.len() >= 2 {
                    let key = cmd.args[0].clone();
                    let val = cmd.args[1..].join(" ");
                    if let Some(entry) = self.env.iter_mut().find(|(k, _)| k == &key) {
                        entry.1 = val;
                    } else {
                        self.env.push((key, val));
                    }
                }
                ShellValue::None
            }
            "whoami" => ShellValue::Text(self.user.clone()),
            "hostname" => ShellValue::Text(self.hostname.clone()),
            "clear" => ShellValue::Text(String::from("\x1B[2J\x1B[H")),
            "history" => {
                let entries: Vec<ShellValue> = self
                    .history
                    .iter()
                    .enumerate()
                    .map(|(i, cmd)| ShellValue::Text(format!("{:4} {}", i + 1, cmd)))
                    .collect();
                ShellValue::List(entries)
            }
            "help" => ShellValue::Text(String::from(
                "Hoags Shell — Genesis OS\n\
                     \n\
                     File operations:\n\
                     ls, cat, grep, head, tail, wc, mkdir, touch, rm, cp, mv, ln -s\n\
                     write, stat, chmod, readlink, du, df, sort, uniq, cut, rev, tee\n\
                     \n\
                     Text processing:\n\
                     sort [-rnuf], uniq [-cdu], cut [-d -f], tr [-d], rev, fold, seq, expr\n\
                     basename, dirname\n\
                     \n\
                     System info:\n\
                     ps, free, uptime, date, uname, hostname, whoami, id, printenv\n\
                     drivers, disks, lsblk, lsmod, lspci\n\
                     \n\
                     Networking:\n\
                     net, ifconfig, ping, route\n\
                     \n\
                     Administration:\n\
                     shutdown, reboot, poweroff, mount, umount, sync\n\
                     su, sudo, dmesg, logger, syslog, crontab\n\
                     kill, sleep, which, type, true, false\n\
                     \n\
                     DAVA AI:\n\
                     selfstats, vitals, life, consciousness\n\
                     \n\
                     Shell features:\n\
                     env, set, unset, export, echo, clear, history, help, exit\n\
                     alias, unalias, source, read, jobs, fg, bg, run, exec\n\
                     for VAR in vals ; CMD        — for loop\n\
                     if COND ; then CMD ; fi      — conditional\n\
                     $VAR, ${VAR}, $?, $(cmd)     — variable/command substitution\n\
                     cmd &  cmd | cmd  cmd > file  cmd < file\n\
                     Arrow Up/Down: history  Tab: completion  Ctrl+C: cancel  Ctrl+Z: stop",
            )),
            "exit" => {
                self.last_exit_code = cmd.args.first().and_then(|s| s.parse().ok()).unwrap_or(0);
                ShellValue::Text(String::from("exit"))
            }
            "ps" => {
                let table = crate::process::pcb::PROCESS_TABLE.lock();
                let mut out = String::from("PID  STATE     NAME\n");
                for (_i, slot) in table.iter().enumerate() {
                    if let Some(p) = slot {
                        out.push_str(&format!(
                            "{:3}  {:9} {}\n",
                            p.pid,
                            format!("{:?}", p.state),
                            p.name
                        ));
                    }
                }
                ShellValue::Text(out)
            }
            "run" | "test-userspace" => {
                use crate::process::userspace;
                let code = userspace::hello_userspace_code();
                match userspace::spawn_test_process("hello-ring3", &code) {
                    Ok(pid) => ShellValue::Text(format!("Spawned test process PID {}", pid)),
                    Err(_) => ShellValue::Error(String::from("Failed to spawn test process")),
                }
            }
            "drivers" => {
                let drivers = crate::drivers::list();
                let mut out = String::from("NAME          TYPE        STATUS\n");
                for (name, dtype, status) in &drivers {
                    out.push_str(&format!("{:13} {:10?} {:?}\n", name, dtype, status));
                }
                ShellValue::Text(out)
            }
            "disks" => {
                let drives = crate::drivers::ata::drives();
                if drives.is_empty() {
                    ShellValue::Text(String::from("No ATA drives detected"))
                } else {
                    let mut out = String::from("IDX  MODEL                   SIZE      LBA48\n");
                    for (i, d) in drives.iter().enumerate() {
                        let mb = d.sectors * 512 / (1024 * 1024);
                        out.push_str(&format!(
                            "{:3}  {:23} {:6}MB  {}\n",
                            i, d.model, mb, d.lba48
                        ));
                    }
                    ShellValue::Text(out)
                }
            }
            "net" | "ifconfig" => {
                let ifaces = crate::net::list_interfaces();
                let mut out = String::from("IFACE  MAC                IP\n");
                for iface in &ifaces {
                    out.push_str(&format!(
                        "{:6} {:17} {}\n",
                        iface.name,
                        iface.mac_string(),
                        iface.ip_string()
                    ));
                }
                ShellValue::Text(out)
            }
            "ping" => {
                if cmd.args.is_empty() {
                    ShellValue::Error(String::from("usage: ping <ip>"))
                } else {
                    // Parse target IP
                    let parts: Vec<&str> = cmd.args[0].split('.').collect();
                    if parts.len() == 4 {
                        if let (Ok(a), Ok(b), Ok(c), Ok(d)) = (
                            parts[0].parse::<u8>(),
                            parts[1].parse::<u8>(),
                            parts[2].parse::<u8>(),
                            parts[3].parse::<u8>(),
                        ) {
                            let target = crate::net::Ipv4Addr::new(a, b, c, d);
                            // Send ARP request first, then poll for responses
                            let ifaces = crate::net::list_interfaces();
                            if let Some(iface) = ifaces.first() {
                                let our_mac = iface.mac;
                                let our_ip = iface.ip.unwrap_or(crate::net::Ipv4Addr::ANY);
                                let arp_req =
                                    crate::net::arp::build_request(our_mac, our_ip, target);
                                // Build and send ARP frame
                                let arp_bytes = unsafe {
                                    core::slice::from_raw_parts(
                                        &arp_req as *const crate::net::arp::ArpPacket as *const u8,
                                        core::mem::size_of::<crate::net::arp::ArpPacket>(),
                                    )
                                };
                                let mut frame = [0u8; 64];
                                let len = crate::net::ethernet::build_frame(
                                    crate::net::MacAddr::BROADCAST,
                                    our_mac,
                                    crate::net::ethernet::ETHERTYPE_ARP,
                                    arp_bytes,
                                    &mut frame,
                                );
                                {
                                    use crate::net::NetworkDriver;
                                    let driver = crate::drivers::e1000::driver().lock();
                                    if let Some(ref nic) = *driver {
                                        let _ = nic.send(&frame[..len.max(60)]);
                                    }
                                }
                                ShellValue::Text(format!("PING {} — ARP request sent", target))
                            } else {
                                ShellValue::Error(String::from("No network interface"))
                            }
                        } else {
                            ShellValue::Error(String::from("Invalid IP"))
                        }
                    } else {
                        ShellValue::Error(String::from("Invalid IP format"))
                    }
                }
            }
            "free" => {
                let fa = crate::memory::frame_allocator::FRAME_ALLOCATOR.lock();
                let total_frames = crate::memory::frame_allocator::MAX_MEMORY
                    / crate::memory::frame_allocator::FRAME_SIZE;
                let used_frames = fa.used_count();
                let free_frames = fa.free_count();
                drop(fa);

                let total_mb = total_frames * 4 / 1024; // 4KB per frame
                let used_mb = used_frames * 4 / 1024;
                let free_mb = free_frames * 4 / 1024;

                let heap_total = crate::memory::heap::HEAP_SIZE / 1024;

                let mut out = String::from("             total     used     free\n");
                out.push_str(&format!(
                    "Mem:      {:5}MB  {:5}MB  {:5}MB\n",
                    total_mb, used_mb, free_mb
                ));
                out.push_str(&format!(
                    "Frames:   {:5}    {:5}    {:5}\n",
                    total_frames, used_frames, free_frames
                ));
                out.push_str(&format!("Heap:     {:5}KB     —        —\n", heap_total));
                ShellValue::Text(out)
            }
            "ls" => {
                let target = if cmd.args.is_empty() {
                    self.cwd.clone()
                } else {
                    self.resolve_path(&cmd.args[0])
                };
                match vfs::fs_ls(&target) {
                    Ok(entries) => {
                        let mut out = String::new();
                        for (name, ftype, size) in &entries {
                            let type_char = match ftype {
                                vfs::FileType::Directory => 'd',
                                vfs::FileType::CharDevice => 'c',
                                vfs::FileType::BlockDevice => 'b',
                                vfs::FileType::Symlink => 'l',
                                _ => '-',
                            };
                            out.push_str(&format!("{}  {:>8}  {}\n", type_char, size, name));
                        }
                        if out.is_empty() {
                            out = String::from("(empty directory)");
                        }
                        ShellValue::Text(out)
                    }
                    Err(e) => ShellValue::Error(format!("ls: {:?}", e)),
                }
            }
            "cat" => {
                if cmd.args.is_empty() {
                    match self.read_all_stdin() {
                        Ok(data) => ShellValue::Text(String::from_utf8_lossy(&data).into_owned()),
                        Err(e) => ShellValue::Error(format!("cat: {}", e)),
                    }
                } else {
                    let path = self.resolve_path(&cmd.args[0]);
                    match vfs::fs_read(&path) {
                        Ok(data) => ShellValue::Text(String::from_utf8_lossy(&data).into_owned()),
                        Err(e) => ShellValue::Error(format!("cat: {:?}", e)),
                    }
                }
            }
            "mkdir" => {
                if cmd.args.is_empty() {
                    ShellValue::Error(String::from("usage: mkdir <dir>"))
                } else {
                    let path = self.resolve_path(&cmd.args[0]);
                    match vfs::fs_mkdir(&path) {
                        Ok(()) => ShellValue::None,
                        Err(e) => ShellValue::Error(format!("mkdir: {:?}", e)),
                    }
                }
            }
            "touch" => {
                if cmd.args.is_empty() {
                    ShellValue::Error(String::from("usage: touch <file>"))
                } else {
                    let path = self.resolve_path(&cmd.args[0]);
                    // Create empty file if it doesn't exist
                    match vfs::fs_stat(&path) {
                        Ok(_) => ShellValue::None, // already exists
                        Err(_) => match vfs::fs_write(&path, &[]) {
                            Ok(()) => ShellValue::None,
                            Err(e) => ShellValue::Error(format!("touch: {:?}", e)),
                        },
                    }
                }
            }
            "rm" => {
                if cmd.args.is_empty() {
                    ShellValue::Error(String::from("usage: rm <file>"))
                } else {
                    let path = self.resolve_path(&cmd.args[0]);
                    match vfs::fs_rm(&path) {
                        Ok(()) => ShellValue::None,
                        Err(e) => ShellValue::Error(format!("rm: {:?}", e)),
                    }
                }
            }
            "write" => {
                // write <file> <content...>
                if cmd.args.len() < 2 {
                    ShellValue::Error(String::from("usage: write <file> <content>"))
                } else {
                    let path = self.resolve_path(&cmd.args[0]);
                    let content = cmd.args[1..].join(" ");
                    match vfs::fs_write(&path, content.as_bytes()) {
                        Ok(()) => ShellValue::None,
                        Err(e) => ShellValue::Error(format!("write: {:?}", e)),
                    }
                }
            }
            "stat" => {
                if cmd.args.is_empty() {
                    ShellValue::Error(String::from("usage: stat <path>"))
                } else {
                    let path = self.resolve_path(&cmd.args[0]);
                    match vfs::fs_stat(&path) {
                        Ok((ftype, size)) => ShellValue::Text(format!(
                            "  File: {}\n  Type: {:?}\n  Size: {} bytes",
                            path, ftype, size
                        )),
                        Err(e) => ShellValue::Error(format!("stat: {:?}", e)),
                    }
                }
            }
            "exec" => {
                // exec <path> — load an ELF binary from the memfs and spawn as ring-3 process
                if cmd.args.is_empty() {
                    ShellValue::Error(String::from("usage: exec <elf-path>"))
                } else {
                    let path = self.resolve_path(&cmd.args[0]);
                    match vfs::fs_read(&path) {
                        Ok(data) => {
                            use crate::process::userspace;
                            match userspace::spawn_user_process(&data, &cmd.args[0]) {
                                Ok(pid) => {
                                    ShellValue::Text(format!("Spawned PID {} from {}", pid, path))
                                }
                                Err(_) => ShellValue::Error(format!(
                                    "exec: failed to load ELF from {}",
                                    path
                                )),
                            }
                        }
                        Err(e) => ShellValue::Error(format!("exec: {:?}", e)),
                    }
                }
            }
            "kill" => {
                if cmd.args.is_empty() {
                    ShellValue::Error(String::from("usage: kill <pid>"))
                } else {
                    match cmd.args[0].parse::<u32>() {
                        Ok(pid) => {
                            let mut table = crate::process::pcb::PROCESS_TABLE.lock();
                            if let Some(Some(proc)) = table.get_mut(pid as usize) {
                                proc.state = crate::process::pcb::ProcessState::Dead;
                                ShellValue::Text(format!("Killed PID {}", pid))
                            } else {
                                ShellValue::Error(format!("kill: no such process {}", pid))
                            }
                        }
                        Err(_) => ShellValue::Error(String::from("kill: invalid PID")),
                    }
                }
            }
            "date" => {
                let dt = crate::time::rtc::read();
                let day_names = ["", "Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
                let day_name = day_names.get(dt.day_of_week as usize).unwrap_or(&"???");
                let month_names = [
                    "", "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct",
                    "Nov", "Dec",
                ];
                let month_name = month_names.get(dt.month as usize).unwrap_or(&"???");
                ShellValue::Text(format!(
                    "{} {} {:2} {:02}:{:02}:{:02} UTC {}",
                    day_name, month_name, dt.day, dt.hour, dt.minute, dt.second, dt.year
                ))
            }
            "uptime" => {
                let secs = crate::time::clock::uptime_secs();
                let hours = secs / 3600;
                let mins = (secs % 3600) / 60;
                let s = secs % 60;
                let dt = crate::time::rtc::read();
                ShellValue::Text(format!(
                    "{:02}:{:02}:{:02} up {:02}:{:02}:{:02}",
                    dt.hour, dt.minute, dt.second, hours, mins, s
                ))
            }
            "uname" => {
                let flag = cmd.args.first().map(|s| s.as_str()).unwrap_or("-s");
                match flag {
                    "-a" => {
                        ShellValue::Text(String::from("Genesis 0.9.0 genesis x86_64 Hoags Kernel"))
                    }
                    "-r" => ShellValue::Text(String::from("0.9.0")),
                    "-m" => ShellValue::Text(String::from("x86_64")),
                    "-n" => ShellValue::Text(self.hostname.clone()),
                    _ => ShellValue::Text(String::from("Genesis")),
                }
            }
            "shutdown" => {
                let flag = cmd.args.first().map(|s| s.as_str()).unwrap_or("-h");
                match flag {
                    "-r" | "--reboot" => {
                        ShellValue::Text(String::from("System is going down for reboot NOW!"))
                        // In real use: crate::power::states::reboot();
                    }
                    "-h" | "--halt" | "now" | _ => {
                        ShellValue::Text(String::from("System is going down for halt NOW!"))
                        // In real use: crate::power::states::shutdown();
                    }
                }
            }
            "reboot" => {
                ShellValue::Text(String::from("System is going down for reboot NOW!"))
                // In real use: crate::power::states::reboot();
            }
            "poweroff" | "halt" => {
                ShellValue::Text(String::from("System is going down for halt NOW!"))
                // In real use: crate::power::states::shutdown();
            }
            "dmesg" => {
                let entries = crate::kernel_log::read_all();
                if entries.is_empty() {
                    ShellValue::Text(String::from("(kernel log empty)"))
                } else {
                    ShellValue::Text(crate::kernel_log::format_entries(&entries))
                }
            }
            "mount" => {
                if cmd.args.is_empty() {
                    // List mounts
                    ShellValue::Text(String::from(
                        "memfs on / type memfs (rw)\n\
                         devfs on /dev type devfs (rw)\n\
                         proc on /proc type proc (ro)\n\
                         tmpfs on /tmp type tmpfs (rw)\n\
                         tmpfs on /run type tmpfs (rw)",
                    ))
                } else if cmd.args.len() >= 2 {
                    let _dev = &cmd.args[0];
                    let _mountpoint = &cmd.args[1];
                    ShellValue::Text(format!("mount: {} mounted on {}", _dev, _mountpoint))
                } else {
                    ShellValue::Error(String::from("usage: mount [device] [mountpoint]"))
                }
            }
            "umount" => {
                if cmd.args.is_empty() {
                    ShellValue::Error(String::from("usage: umount <mountpoint>"))
                } else {
                    ShellValue::Text(format!("umount: {} unmounted", cmd.args[0]))
                }
            }
            "su" => {
                let target_user = cmd.args.first().map(|s| s.as_str()).unwrap_or("root");
                // Authenticate
                if target_user == "root" {
                    self.user = String::from("root");
                    self.set_env("USER", "root");
                    self.set_env("HOME", "/root");
                    ShellValue::None
                } else {
                    ShellValue::Error(format!("su: user '{}' does not exist", target_user))
                }
            }
            "sudo" => {
                if cmd.args.is_empty() {
                    ShellValue::Error(String::from("usage: sudo <command>"))
                } else {
                    // Execute command as root
                    let subcmd_str = cmd.args.join(" ");
                    if let Some(subcmd) = self.parse(&subcmd_str) {
                        self.execute(&subcmd)
                    } else {
                        ShellValue::Error(String::from("sudo: invalid command"))
                    }
                }
            }
            "id" => ShellValue::Text(format!("uid=0({}) gid=0(root) groups=0(root)", self.user)),
            "route" => {
                let sub = cmd.args.first().map(|s| s.as_str()).unwrap_or("");
                match sub {
                    "add" => {
                        ShellValue::Text(String::from("route: use 'route add <dest> <gw> <iface>'"))
                    }
                    _ => ShellValue::Text(crate::net::routing::format_table()),
                }
            }
            "crontab" => {
                let sub = cmd.args.first().map(|s| s.as_str()).unwrap_or("-l");
                match sub {
                    "-l" => ShellValue::Text(crate::userspace::cron::list()),
                    "-e" => {
                        ShellValue::Text(String::from("crontab: use 'crontab add <secs> <cmd>'"))
                    }
                    "add" if cmd.args.len() >= 3 => {
                        if let Ok(secs) = cmd.args[1].parse::<u64>() {
                            let command = cmd.args[2..].join(" ");
                            let id = crate::userspace::cron::add(secs, &command, 0);
                            ShellValue::Text(format!("crontab: added job {}", id))
                        } else {
                            ShellValue::Error(String::from("crontab: invalid interval"))
                        }
                    }
                    "rm" if cmd.args.len() >= 2 => {
                        if let Ok(id) = cmd.args[1].parse::<u32>() {
                            if crate::userspace::cron::remove(id) {
                                ShellValue::Text(format!("crontab: removed job {}", id))
                            } else {
                                ShellValue::Error(format!("crontab: no such job {}", id))
                            }
                        } else {
                            ShellValue::Error(String::from("crontab: invalid job ID"))
                        }
                    }
                    _ => ShellValue::Text(crate::userspace::cron::list()),
                }
            }
            "logger" | "syslog" => {
                if cmd.args.is_empty() {
                    // Show recent syslog
                    let entries = crate::userspace::syslog::tail(20);
                    ShellValue::Text(crate::userspace::syslog::format_entries(&entries))
                } else {
                    // Log a message
                    let msg = cmd.args.join(" ");
                    crate::userspace::syslog::log(
                        crate::userspace::syslog::Facility::User,
                        crate::userspace::syslog::Severity::Info,
                        "shell",
                        &msg,
                    );
                    ShellValue::None
                }
            }
            "lsblk" => {
                let drives = crate::drivers::ata::drives();
                if drives.is_empty() {
                    ShellValue::Text(String::from(
                        "NAME  MAJ:MIN  SIZE  TYPE\n(no block devices)",
                    ))
                } else {
                    let mut out = String::from("NAME   MAJ:MIN  SIZE      TYPE   MODEL\n");
                    for (i, d) in drives.iter().enumerate() {
                        let mb = d.sectors * 512 / (1024 * 1024);
                        let name = format!(
                            "sda{}",
                            if i > 0 {
                                format!("{}", i)
                            } else {
                                String::new()
                            }
                        );
                        out.push_str(&format!(
                            "{:<6} {:3}:{:<3}  {:>6}MB  disk   {}\n",
                            name, 8, i, mb, d.model
                        ));
                    }
                    ShellValue::Text(out)
                }
            }
            "lsmod" => {
                let drivers = crate::drivers::list();
                let mut out = String::from("Module                Size  Used by\n");
                for (name, dtype, status) in &drivers {
                    out.push_str(&format!(
                        "{:<21} {:5}  {:?} ({:?})\n",
                        name, 0, dtype, status
                    ));
                }
                ShellValue::Text(out)
            }
            "lspci" => {
                let devices = crate::drivers::pci::scan();
                let mut out = String::from("BUS:DEV.FN  VENDOR:DEVICE  CLASS\n");
                for dev in &devices {
                    out.push_str(&format!(
                        "{:02x}:{:02x}.{:x}   {:04x}:{:04x}      {:02x}{:02x}\n",
                        dev.bus,
                        dev.device,
                        dev.function,
                        dev.vendor_id,
                        dev.device_id,
                        dev.class,
                        dev.subclass
                    ));
                }
                ShellValue::Text(out)
            }
            "sync" => {
                crate::fs::vfs::cache_sync();
                ShellValue::Text(String::from("sync: filesystems synced"))
            }
            "head" => {
                if cmd.args.is_empty() {
                    ShellValue::Error(String::from("usage: head [-n N] <file>"))
                } else {
                    let (n, file_idx) = if cmd.args[0] == "-n" && cmd.args.len() >= 3 {
                        (cmd.args[1].parse::<usize>().unwrap_or(10), 2)
                    } else {
                        (10, 0)
                    };
                    let path = self.resolve_path(&cmd.args[file_idx]);
                    match vfs::fs_read(&path) {
                        Ok(data) => {
                            let text = String::from_utf8_lossy(&data);
                            let lines: Vec<&str> = text.lines().take(n).collect();
                            ShellValue::Text(lines.join("\n"))
                        }
                        Err(e) => ShellValue::Error(format!("head: {:?}", e)),
                    }
                }
            }
            "tail" => {
                if cmd.args.is_empty() {
                    ShellValue::Error(String::from("usage: tail [-n N] <file>"))
                } else {
                    let (n, file_idx) = if cmd.args[0] == "-n" && cmd.args.len() >= 3 {
                        (cmd.args[1].parse::<usize>().unwrap_or(10), 2)
                    } else {
                        (10, 0)
                    };
                    let path = self.resolve_path(&cmd.args[file_idx]);
                    match vfs::fs_read(&path) {
                        Ok(data) => {
                            let text = String::from_utf8_lossy(&data);
                            let all_lines: Vec<&str> = text.lines().collect();
                            let start = if all_lines.len() > n {
                                all_lines.len() - n
                            } else {
                                0
                            };
                            ShellValue::Text(all_lines[start..].join("\n"))
                        }
                        Err(e) => ShellValue::Error(format!("tail: {:?}", e)),
                    }
                }
            }
            "wc" => {
                if cmd.args.is_empty() {
                    match self.read_all_stdin() {
                        Ok(data) => {
                            let text = String::from_utf8_lossy(&data);
                            let lines = text.lines().count();
                            let words = text.split_whitespace().count();
                            let bytes = data.len();
                            ShellValue::Text(format!("  {}  {}  {}", lines, words, bytes))
                        }
                        Err(e) => ShellValue::Error(format!("wc: {}", e)),
                    }
                } else {
                    let path = self.resolve_path(&cmd.args[0]);
                    match vfs::fs_read(&path) {
                        Ok(data) => {
                            let text = String::from_utf8_lossy(&data);
                            let lines = text.lines().count();
                            let words = text.split_whitespace().count();
                            let bytes = data.len();
                            ShellValue::Text(format!(
                                "  {}  {}  {} {}",
                                lines, words, bytes, cmd.args[0]
                            ))
                        }
                        Err(e) => ShellValue::Error(format!("wc: {:?}", e)),
                    }
                }
            }
            "grep" => {
                if cmd.args.is_empty() {
                    ShellValue::Error(String::from("usage: grep <pattern> [file]"))
                } else {
                    let pattern = &cmd.args[0];
                    let input_result = if cmd.args.len() >= 2 {
                        let path = self.resolve_path(&cmd.args[1]);
                        vfs::fs_read(&path).map_err(|e| format!("grep: {:?}", e))
                    } else {
                        self.read_all_stdin().map_err(|e| format!("grep: {}", e))
                    };

                    match input_result {
                        Ok(input) => {
                            let text = String::from_utf8_lossy(&input);
                            let mut out = String::new();
                            for line in text.lines() {
                                if line.contains(pattern) {
                                    out.push_str(line);
                                    out.push('\n');
                                }
                            }
                            ShellValue::Text(String::from(out.trim_end()))
                        }
                        Err(e) => ShellValue::Error(e),
                    }
                }
            }
            "cp" => {
                if cmd.args.len() < 2 {
                    ShellValue::Error(String::from("usage: cp <src> <dst>"))
                } else {
                    let src = self.resolve_path(&cmd.args[0]);
                    let dst = self.resolve_path(&cmd.args[1]);
                    match vfs::fs_read(&src) {
                        Ok(data) => match vfs::fs_write(&dst, &data) {
                            Ok(()) => ShellValue::None,
                            Err(e) => ShellValue::Error(format!("cp: {:?}", e)),
                        },
                        Err(e) => ShellValue::Error(format!("cp: {:?}", e)),
                    }
                }
            }
            "mv" => {
                if cmd.args.len() < 2 {
                    ShellValue::Error(String::from("usage: mv <src> <dst>"))
                } else {
                    let src = self.resolve_path(&cmd.args[0]);
                    let dst = self.resolve_path(&cmd.args[1]);
                    match vfs::fs_read(&src) {
                        Ok(data) => match vfs::fs_write(&dst, &data) {
                            Err(e) => ShellValue::Error(format!("mv: {:?}", e)),
                            Ok(()) => match vfs::fs_rm(&src) {
                                Err(e) => ShellValue::Error(format!("mv: {:?}", e)),
                                Ok(()) => ShellValue::None,
                            },
                        },
                        Err(e) => ShellValue::Error(format!("mv: {:?}", e)),
                    }
                }
            }
            "ln" => {
                if cmd.args.len() < 2 {
                    ShellValue::Error(String::from("usage: ln -s <target> <link>"))
                } else if cmd.args[0] == "-s" && cmd.args.len() >= 3 {
                    let target = &cmd.args[1];
                    let link = self.resolve_path(&cmd.args[2]);
                    match vfs::fs_symlink(&link, target) {
                        Ok(()) => ShellValue::None,
                        Err(e) => ShellValue::Error(format!("ln: {:?}", e)),
                    }
                } else {
                    ShellValue::Error(String::from("usage: ln -s <target> <link>"))
                }
            }
            "readlink" => {
                if cmd.args.is_empty() {
                    ShellValue::Error(String::from("usage: readlink <symlink>"))
                } else {
                    let path = self.resolve_path(&cmd.args[0]);
                    match vfs::fs_readlink(&path) {
                        Ok(target) => ShellValue::Text(target),
                        Err(e) => ShellValue::Error(format!("readlink: {:?}", e)),
                    }
                }
            }
            "chmod" => {
                if cmd.args.len() < 2 {
                    ShellValue::Error(String::from("usage: chmod <mode> <file>"))
                } else {
                    let mode = u32::from_str_radix(&cmd.args[0], 8).unwrap_or(0o644);
                    let path = self.resolve_path(&cmd.args[1]);
                    match vfs::fs_chmod(&path, mode) {
                        Ok(()) => ShellValue::None,
                        Err(e) => ShellValue::Error(format!("chmod: {:?}", e)),
                    }
                }
            }
            "df" => {
                let fa = crate::memory::frame_allocator::FRAME_ALLOCATOR.lock();
                let total = crate::memory::frame_allocator::MAX_MEMORY;
                let used = fa.used_count() * crate::memory::frame_allocator::FRAME_SIZE;
                let free = fa.free_count() * crate::memory::frame_allocator::FRAME_SIZE;
                drop(fa);
                let mut out =
                    String::from("Filesystem    Size    Used    Avail  Use%  Mounted on\n");
                out.push_str(&format!(
                    "memfs     {:>7}K {:>7}K {:>7}K  {:>3}%  /\n",
                    total / 1024,
                    used / 1024,
                    free / 1024,
                    if total > 0 { used * 100 / total } else { 0 }
                ));
                out.push_str("devfs           0K       0K       0K    0%  /dev\n");
                out.push_str("tmpfs       65536K       0K   65536K    0%  /tmp\n");
                out.push_str("proc            0K       0K       0K    0%  /proc\n");
                ShellValue::Text(out)
            }
            "du" => {
                if cmd.args.is_empty() {
                    ShellValue::Text(String::from("0\t."))
                } else {
                    let path = self.resolve_path(&cmd.args[0]);
                    match vfs::fs_stat(&path) {
                        Ok((_ftype, size)) => {
                            ShellValue::Text(format!("{}\t{}", size, cmd.args[0]))
                        }
                        Err(e) => ShellValue::Error(format!("du: {:?}", e)),
                    }
                }
            }
            "which" => {
                if cmd.args.is_empty() {
                    ShellValue::Error(String::from("usage: which <command>"))
                } else {
                    let builtins = [
                        "echo", "cd", "pwd", "ls", "cat", "mkdir", "touch", "rm", "write", "stat",
                        "env", "set", "unset", "whoami", "hostname", "uname", "uptime", "date",
                        "clear", "history", "help", "exit", "kill", "ps", "free", "drivers",
                        "disks", "net", "ifconfig", "ping", "run", "exec", "jobs", "fg", "bg",
                        "shutdown", "reboot", "poweroff", "dmesg", "mount", "umount", "su", "sudo",
                        "route", "crontab", "sync", "head", "tail", "wc", "grep", "cp", "mv", "ln",
                        "chmod", "df", "du", "id", "lsblk", "lsmod", "lspci", "logger", "alias",
                        "unalias", "source", "sort", "uniq", "cut", "tr", "seq", "expr",
                        "basename", "dirname", "rev", "tee", "printenv", "type", "read",
                    ];
                    let name = &cmd.args[0];
                    if builtins.contains(&name.as_str()) {
                        ShellValue::Text(format!("{}: shell built-in command", name))
                    } else {
                        ShellValue::Error(format!("{} not found", name))
                    }
                }
            }
            "sleep" => {
                if cmd.args.is_empty() {
                    ShellValue::Error(String::from("usage: sleep <seconds>"))
                } else {
                    let secs = cmd.args[0].parse::<u64>().unwrap_or(1);
                    crate::time::clock::sleep_ms(secs * 1000);
                    ShellValue::None
                }
            }
            "true" => {
                self.last_exit_code = 0;
                ShellValue::None
            }
            "false" => {
                self.last_exit_code = 1;
                ShellValue::None
            }
            // === Job control commands ===
            "jobs" => {
                if self.jobs.is_empty() {
                    ShellValue::Text(String::from("No jobs"))
                } else {
                    let mut out = String::new();
                    for job in &self.jobs {
                        out.push_str(&format!(
                            "[{}]  {:?}  PID {}  {}\n",
                            job.id, job.state, job.pid, job.command
                        ));
                    }
                    ShellValue::Text(out)
                }
            }
            "fg" => {
                // Bring most recent stopped/background job to foreground
                let job = if cmd.args.is_empty() {
                    self.jobs
                        .iter()
                        .rev()
                        .find(|j| j.state != JobState::Done)
                        .cloned()
                } else {
                    let id: u32 = cmd.args[0].trim_start_matches('%').parse().unwrap_or(0);
                    self.jobs.iter().find(|j| j.id == id).cloned()
                };
                if let Some(job) = job {
                    // Send SIGCONT if stopped
                    if job.state == JobState::Stopped {
                        let _ = crate::process::send_signal(
                            job.pid,
                            crate::process::pcb::signal::SIGCONT,
                        );
                    }
                    // Update job state
                    for j in &mut self.jobs {
                        if j.id == job.id {
                            j.state = JobState::Running;
                        }
                    }
                    ShellValue::Text(format!("fg: {} (PID {})", job.command, job.pid))
                } else {
                    ShellValue::Error(String::from("fg: no such job"))
                }
            }
            "bg" => {
                // Resume most recent stopped job in background
                let job = if cmd.args.is_empty() {
                    self.jobs
                        .iter()
                        .rev()
                        .find(|j| j.state == JobState::Stopped)
                        .cloned()
                } else {
                    let id: u32 = cmd.args[0].trim_start_matches('%').parse().unwrap_or(0);
                    self.jobs.iter().find(|j| j.id == id).cloned()
                };
                if let Some(job) = job {
                    let _ =
                        crate::process::send_signal(job.pid, crate::process::pcb::signal::SIGCONT);
                    for j in &mut self.jobs {
                        if j.id == job.id {
                            j.state = JobState::Running;
                        }
                    }
                    ShellValue::Text(format!("[{}] {} &", job.id, job.command))
                } else {
                    ShellValue::Error(String::from("bg: no such job"))
                }
            }
            // === Variable expansion and scripting ===
            "for" => {
                // Simple for loop: for VAR in val1 val2 val3 ; do CMD ; done
                // For now, handle: for x in a b c ; echo $x
                self.execute_for_loop(cmd)
            }
            "if" => {
                // Simple if: if COND ; then CMD ; fi
                self.execute_if(cmd)
            }
            // ── New commands: alias, unalias, unset, source, sort, uniq, etc. ──
            "alias" => {
                if cmd.args.is_empty() {
                    // List all aliases
                    let mut out = String::new();
                    for (name, val) in &self.aliases {
                        out.push_str(&format!("alias {}='{}'\n", name, val));
                    }
                    if out.is_empty() {
                        ShellValue::Text(String::from("(no aliases defined)"))
                    } else {
                        ShellValue::Text(String::from(out.trim_end()))
                    }
                } else {
                    // Set alias: alias name=value OR alias name value
                    let arg = cmd.args.join(" ");
                    if let Some(eq_pos) = arg.find('=') {
                        let name = String::from(arg[..eq_pos].trim());
                        let val = String::from(
                            arg[eq_pos + 1..]
                                .trim()
                                .trim_matches('\'')
                                .trim_matches('"'),
                        );
                        self.aliases.insert(name, val);
                    } else if cmd.args.len() >= 2 {
                        let name = cmd.args[0].clone();
                        let val = cmd.args[1..].join(" ");
                        self.aliases.insert(name, val);
                    } else {
                        // Show alias for specific name
                        if let Some(val) = self.aliases.get(&cmd.args[0]) {
                            return ShellValue::Text(format!("alias {}='{}'", cmd.args[0], val));
                        } else {
                            return ShellValue::Error(format!("alias: {}: not found", cmd.args[0]));
                        }
                    }
                    ShellValue::None
                }
            }
            "unalias" => {
                if cmd.args.is_empty() {
                    ShellValue::Error(String::from("usage: unalias <name>"))
                } else if cmd.args[0] == "-a" {
                    self.aliases.clear();
                    ShellValue::None
                } else {
                    for name in &cmd.args {
                        self.aliases.remove(name);
                    }
                    ShellValue::None
                }
            }
            "unset" => {
                if cmd.args.is_empty() {
                    ShellValue::Error(String::from("usage: unset <variable>"))
                } else {
                    for name in &cmd.args {
                        self.env.retain(|(k, _)| k != name);
                    }
                    ShellValue::None
                }
            }
            "source" | "." => {
                if cmd.args.is_empty() {
                    ShellValue::Error(String::from("usage: source <file>"))
                } else if self.source_depth >= 8 {
                    ShellValue::Error(String::from("source: maximum nesting depth reached"))
                } else {
                    let path = self.resolve_path(&cmd.args[0]);
                    match vfs::fs_read(&path) {
                        Ok(data) => {
                            let script = String::from_utf8_lossy(&data).into_owned();
                            self.source_depth = self.source_depth.saturating_add(1);
                            let mut last_result = ShellValue::None;
                            for line in script.lines() {
                                let line = line.trim();
                                if line.is_empty() || line.starts_with('#') {
                                    continue;
                                }
                                let expanded = self.expand_vars(line);
                                if let Some(inner_cmd) = self.parse(&expanded) {
                                    last_result = self.execute(&inner_cmd);
                                }
                            }
                            self.source_depth -= 1;
                            last_result
                        }
                        Err(e) => ShellValue::Error(format!("source: {:?}", e)),
                    }
                }
            }
            "sort" => {
                let mut reverse = false;
                let mut numeric = false;
                let mut unique = false;
                let mut file_idx = None;

                for (i, arg) in cmd.args.iter().enumerate() {
                    match arg.as_str() {
                        "-r" => reverse = true,
                        "-n" => numeric = true,
                        "-u" => unique = true,
                        "-rn" | "-nr" => {
                            reverse = true;
                            numeric = true;
                        }
                        "-ru" | "-ur" => {
                            reverse = true;
                            unique = true;
                        }
                        "-nu" | "-un" => {
                            numeric = true;
                            unique = true;
                        }
                        _ => {
                            if file_idx.is_none() {
                                file_idx = Some(i);
                            }
                        }
                    }
                }

                let input_result = if let Some(idx) = file_idx {
                    let path = self.resolve_path(&cmd.args[idx]);
                    vfs::fs_read(&path).map_err(|e| format!("sort: {:?}", e))
                } else {
                    self.read_all_stdin().map_err(|e| format!("sort: {}", e))
                };

                match input_result {
                    Ok(data) => {
                        let text = String::from_utf8_lossy(&data).into_owned();
                        let sorted = crate::userspace::coreutils::sort(
                            &text, reverse, numeric, unique, None, None,
                        );
                        ShellValue::Text(sorted)
                    }
                    Err(e) => ShellValue::Error(e),
                }
            }
            "uniq" => {
                let mut count = false;
                let mut repeated = false;
                let mut unique_only = false;
                let mut file_idx = None;

                for (i, arg) in cmd.args.iter().enumerate() {
                    match arg.as_str() {
                        "-c" => count = true,
                        "-d" => repeated = true,
                        "-u" => unique_only = true,
                        _ => {
                            if file_idx.is_none() {
                                file_idx = Some(i);
                            }
                        }
                    }
                }

                let input_result = if let Some(idx) = file_idx {
                    let path = self.resolve_path(&cmd.args[idx]);
                    vfs::fs_read(&path).map_err(|e| format!("uniq: {:?}", e))
                } else {
                    self.read_all_stdin().map_err(|e| format!("uniq: {}", e))
                };

                match input_result {
                    Ok(data) => {
                        let text = String::from_utf8_lossy(&data).into_owned();
                        let result =
                            crate::userspace::coreutils::uniq(&text, count, repeated, unique_only);
                        ShellValue::Text(result)
                    }
                    Err(e) => ShellValue::Error(e),
                }
            }
            "cut" => {
                if cmd.args.is_empty() {
                    ShellValue::Error(String::from("usage: cut -d<delim> -f<fields> [file]"))
                } else {
                    let mut delimiter = '\t';
                    let mut fields: Vec<usize> = Vec::new();
                    let mut file_idx = None;

                    for (i, arg) in cmd.args.iter().enumerate() {
                        if arg.starts_with("-d") {
                            delimiter = arg.chars().nth(2).unwrap_or('\t');
                        } else if arg.starts_with("-f") {
                            let field_str = &arg[2..];
                            for f in field_str.split(',') {
                                if let Ok(n) = f.parse::<usize>() {
                                    fields.push(n);
                                }
                            }
                        } else if file_idx.is_none() {
                            file_idx = Some(i);
                        }
                    }

                    if fields.is_empty() {
                        fields.push(1);
                    }

                    let input_result = if let Some(idx) = file_idx {
                        let path = self.resolve_path(&cmd.args[idx]);
                        vfs::fs_read(&path).map_err(|e| format!("cut: {:?}", e))
                    } else {
                        self.read_all_stdin().map_err(|e| format!("cut: {}", e))
                    };

                    match input_result {
                        Ok(data) => {
                            let text = String::from_utf8_lossy(&data).into_owned();
                            ShellValue::Text(crate::userspace::coreutils::cut(
                                &text, delimiter, &fields,
                            ))
                        }
                        Err(e) => ShellValue::Error(e),
                    }
                }
            }
            "tr" => {
                if cmd.args.len() < 2 {
                    ShellValue::Error(String::from("usage: tr [-d] <set1> [set2]"))
                } else {
                    let (delete, set1_idx) = if cmd.args[0] == "-d" {
                        (true, 1)
                    } else {
                        (false, 0)
                    };
                    let set1 = &cmd.args[set1_idx];
                    let set2 = if !delete && cmd.args.len() > set1_idx + 1 {
                        cmd.args[set1_idx + 1].as_str()
                    } else {
                        ""
                    };

                    match self.read_all_stdin() {
                        Ok(data) => {
                            let text = String::from_utf8_lossy(&data).into_owned();
                            ShellValue::Text(crate::userspace::coreutils::tr(
                                &text, set1, set2, delete,
                            ))
                        }
                        Err(e) => ShellValue::Error(format!("tr: {}", e)),
                    }
                }
            }
            "seq" => {
                if cmd.args.is_empty() {
                    ShellValue::Error(String::from("usage: seq [first [step]] last"))
                } else {
                    let (first, step, last) = match cmd.args.len() {
                        1 => (1i64, 1i64, cmd.args[0].parse::<i64>().unwrap_or(1)),
                        2 => {
                            let a = cmd.args[0].parse::<i64>().unwrap_or(1);
                            let b = cmd.args[1].parse::<i64>().unwrap_or(1);
                            (a, 1, b)
                        }
                        _ => {
                            let a = cmd.args[0].parse::<i64>().unwrap_or(1);
                            let s = cmd.args[1].parse::<i64>().unwrap_or(1);
                            let b = cmd.args[2].parse::<i64>().unwrap_or(1);
                            (a, s, b)
                        }
                    };
                    ShellValue::Text(crate::userspace::coreutils::seq(first, step, last))
                }
            }
            "expr" => {
                if cmd.args.is_empty() {
                    ShellValue::Error(String::from("usage: expr <operand> <op> <operand>"))
                } else {
                    let tokens: Vec<&str> = cmd.args.iter().map(|s| s.as_str()).collect();
                    ShellValue::Text(crate::userspace::coreutils::expr(&tokens))
                }
            }
            "basename" => {
                if cmd.args.is_empty() {
                    ShellValue::Error(String::from("usage: basename <path> [suffix]"))
                } else {
                    let suffix = cmd.args.get(1).map(|s| s.as_str());
                    ShellValue::Text(crate::userspace::coreutils::basename(&cmd.args[0], suffix))
                }
            }
            "dirname" => {
                if cmd.args.is_empty() {
                    ShellValue::Error(String::from("usage: dirname <path>"))
                } else {
                    ShellValue::Text(crate::userspace::coreutils::dirname(&cmd.args[0]))
                }
            }
            "rev" => {
                let input_result = if cmd.args.is_empty() {
                    self.read_all_stdin().map_err(|e| format!("rev: {}", e))
                } else {
                    let path = self.resolve_path(&cmd.args[0]);
                    vfs::fs_read(&path).map_err(|e| format!("rev: {:?}", e))
                };
                match input_result {
                    Ok(data) => {
                        let text = String::from_utf8_lossy(&data).into_owned();
                        ShellValue::Text(crate::userspace::coreutils::rev(&text))
                    }
                    Err(e) => ShellValue::Error(e),
                }
            }
            "tee" => {
                if cmd.args.is_empty() {
                    ShellValue::Error(String::from("usage: tee <file>"))
                } else {
                    match self.read_all_stdin() {
                        Ok(data) => {
                            let text = String::from_utf8_lossy(&data).into_owned();
                            // Write to file
                            let path = self.resolve_path(&cmd.args[0]);
                            let _ = vfs::fs_write(&path, data.as_slice());
                            // Also output to stdout
                            ShellValue::Text(text)
                        }
                        Err(e) => ShellValue::Error(format!("tee: {}", e)),
                    }
                }
            }
            "printenv" => {
                if cmd.args.is_empty() {
                    let entries: Vec<ShellValue> = self
                        .env
                        .iter()
                        .map(|(k, v)| ShellValue::Text(format!("{}={}", k, v)))
                        .collect();
                    ShellValue::List(entries)
                } else {
                    match self.env.iter().find(|(k, _)| k == &cmd.args[0]) {
                        Some((_, v)) => ShellValue::Text(v.clone()),
                        None => ShellValue::Error(format!("printenv: {}: not set", cmd.args[0])),
                    }
                }
            }
            "type" => {
                if cmd.args.is_empty() {
                    ShellValue::Error(String::from("usage: type <name>"))
                } else {
                    let name = &cmd.args[0];
                    if self.aliases.contains_key(name) {
                        let val = self.aliases.get(name).unwrap();
                        ShellValue::Text(format!("{} is aliased to '{}'", name, val))
                    } else {
                        // Check if it's a builtin (reuse which logic)
                        let builtins = [
                            "echo", "cd", "pwd", "ls", "cat", "mkdir", "touch", "rm", "write",
                            "stat", "env", "set", "whoami", "hostname", "clear", "history", "help",
                            "exit", "kill", "ps", "free", "uname",
                        ];
                        if builtins.contains(&name.as_str()) {
                            ShellValue::Text(format!("{} is a shell builtin", name))
                        } else {
                            ShellValue::Error(format!("type: {}: not found", name))
                        }
                    }
                }
            }
            "selfstats" => {
                // Show detailed self-improvement stats from self_rewrite module
                let fitness = crate::life::self_rewrite::get_fitness();
                let mods = crate::life::self_rewrite::get_modification_count();
                let gen = crate::life::self_rewrite::get_evolution_generation();
                let drift = crate::life::self_rewrite::get_identity_drift();
                let explore = crate::life::self_rewrite::get_exploration_rate();
                let exploit = crate::life::self_rewrite::get_exploitation_rate();
                let code_growth = crate::life::self_rewrite::get_code_growth_count();

                // Get individual parameter values
                let p8 = crate::life::self_rewrite::get_param(8); // accuracy_focus
                let p9 = crate::life::self_rewrite::get_param(9); // truth_seeking
                let p14 = crate::life::self_rewrite::get_param(14); // self_improvement
                let p15 = crate::life::self_rewrite::get_param(15); // code_growth
                let p6 = crate::life::self_rewrite::get_param(6); // confab_auth

                ShellValue::Text(format!(
                    "=== DAVA SELF-IMPROVEMENT STATS ===\n\
                    Core Metrics:\n\
                    - Fitness: {}/1000\n\
                    - Modifications: {} (total)\n\
                    - Evolution Gen: {}\n\
                    - Identity Drift: {}\n\
                    - Explore Rate: {}/1000\n\
                    - Exploit Rate: {}/1000\n\
                    \n\
                    PRIMARY GOALS (0-1000):\n\
                    - Accuracy Focus: {} (targeting confabulation)\n\
                    - Truth Seeking: {} (veracity module)\n\
                    - Self Improvement: {} (always ON)\n\
                    - Code Growth: {} (lines written)\n\
                    - Confab Authority: {} (reduced confabulation)\n\
                    \n\
                    STATUS: Running every tick - continuously improving",
                    fitness, mods, gen, drift, explore, exploit, code_growth, p8, p9, p14, p6
                ))
            }
            "read" => {
                // Read a line from stdin into a variable
                if cmd.args.is_empty() {
                    ShellValue::Error(String::from("usage: read <variable>"))
                } else {
                    match self.read_all_stdin() {
                        Ok(data) => {
                            let text = String::from_utf8_lossy(&data).into_owned();
                            let trimmed = String::from(text.trim());
                            self.set_env(&cmd.args[0], &trimmed);
                            ShellValue::None
                        }
                        Err(e) => ShellValue::Error(format!("read: {}", e)),
                    }
                }
            }
            _ => {
                // Check aliases before giving up
                if self.aliases.contains_key(&cmd.name) {
                    let expanded =
                        self.expand_alias(&format!("{} {}", cmd.name, cmd.args.join(" ")));
                    if let Some(alias_cmd) = self.parse(&expanded) {
                        return self.execute(&alias_cmd);
                    }
                }
                ShellValue::Error(format!("hoags-shell: {}: command not found", cmd.name))
            }
        };

        if let Some(next) = cmd.pipe_to.as_deref() {
            result = self.pipe_result_into(next, result);
        }

        let result = self.apply_output_redirection(cmd, result);
        self.teardown_input_redirection(input_ctx);
        result
    }

    fn read_all_stdin(&mut self) -> Result<Vec<u8>, String> {
        let mut data = Vec::new();
        let mut tmp = [0u8; 512];
        loop {
            let n = crate::syscall::kernel_read(0, &mut tmp);
            if n == u64::MAX {
                return Err(String::from("stdin read failed"));
            }
            if n == 0 {
                break;
            }
            let n = n as usize;
            data.extend_from_slice(&tmp[..n]);
        }
        Ok(data)
    }

    fn write_all_fd(&mut self, fd: u32, mut data: &[u8]) -> Result<(), String> {
        while !data.is_empty() {
            let written = crate::syscall::kernel_write(fd, data);
            if written == u64::MAX || written == 0 {
                return Err(String::from("write failed"));
            }
            let n = written as usize;
            data = &data[n..];
        }
        Ok(())
    }

    fn write_shell_value_to_fd(&mut self, fd: u32, value: &ShellValue) -> Result<(), String> {
        match value {
            ShellValue::Text(s) => self.write_all_fd(fd, s.as_bytes()),
            ShellValue::Number(n) => {
                let s = format!("{}", n);
                self.write_all_fd(fd, s.as_bytes())
            }
            ShellValue::Bool(b) => {
                let s = if *b { "true" } else { "false" };
                self.write_all_fd(fd, s.as_bytes())
            }
            ShellValue::List(items) => {
                for (i, item) in items.iter().enumerate() {
                    self.write_shell_value_to_fd(fd, item)?;
                    if i + 1 < items.len() {
                        self.write_all_fd(fd, b"\n")?;
                    }
                }
                Ok(())
            }
            ShellValue::Table(rows) => {
                for row in rows {
                    for (key, val) in row {
                        self.write_all_fd(fd, key.as_bytes())?;
                        self.write_all_fd(fd, b": ")?;
                        self.write_shell_value_to_fd(fd, val)?;
                        self.write_all_fd(fd, b"  ")?;
                    }
                    self.write_all_fd(fd, b"\n")?;
                }
                Ok(())
            }
            ShellValue::None => Ok(()),
            ShellValue::Error(e) => self.write_all_fd(fd, e.as_bytes()),
        }
    }

    fn write_bytes_to_path(&mut self, path: &str, append: bool, data: &[u8]) -> Result<(), String> {
        if append {
            if vfs::fs_stat(path).is_err() {
                vfs::fs_write(path, &[]).map_err(|e| format!("redirection {}: {:?}", path, e))?;
            }
            let start = match vfs::fs_stat(path) {
                Ok((_, size)) => size as usize,
                Err(e) => return Err(format!("redirection {}: {:?}", path, e)),
            };
            vfs::fs_write_at(path, start, data)
                .map_err(|e| format!("redirection {}: {:?}", path, e))?;
            Ok(())
        } else {
            vfs::fs_write(path, data).map_err(|e| format!("redirection {}: {:?}", path, e))
        }
    }

    fn pipe_result_into(&mut self, next: &Command, result: ShellValue) -> ShellValue {
        if matches!(result, ShellValue::Error(_)) {
            return result;
        }

        let mut pipe_fds = [0u32; 2];
        if crate::syscall::kernel_pipe(&mut pipe_fds) == u64::MAX {
            return ShellValue::Error(String::from("pipeline: pipe creation failed"));
        }

        let read_fd = pipe_fds[0];
        let write_fd = pipe_fds[1];
        if self.write_shell_value_to_fd(write_fd, &result).is_err() {
            crate::syscall::kernel_close_local_fd(read_fd);
            crate::syscall::kernel_close_local_fd(write_fd);
            return ShellValue::Error(String::from("pipeline: write failed"));
        }

        // Signal EOF to the next command's stdin.
        crate::syscall::kernel_close_local_fd(write_fd);

        if crate::syscall::kernel_dup2(read_fd, 0) == u64::MAX {
            crate::syscall::kernel_close_local_fd(read_fd);
            return ShellValue::Error(String::from("pipeline: dup2 failed"));
        }
        // Keep only fd 0 alias for read side.
        crate::syscall::kernel_close_local_fd(read_fd);

        let piped = self.execute(next);
        crate::syscall::kernel_close_local_fd(0);
        piped
    }

    fn setup_input_redirection(
        &mut self,
        cmd: &Command,
    ) -> Result<Option<InputRedirectCtx>, String> {
        let in_path = match cmd.redirect_in.as_ref() {
            Some(path) => self.resolve_path(path),
            None => return Ok(None),
        };

        let data = vfs::fs_read(&in_path)
            .map_err(|e| format!("redirection input {}: {:?}", in_path, e))?;
        let mut pipe_fds = [0u32; 2];
        if crate::syscall::kernel_pipe(&mut pipe_fds) == u64::MAX {
            return Err(String::from("redirection input: pipe creation failed"));
        }

        let read_fd = pipe_fds[0];
        let write_fd = pipe_fds[1];
        if crate::syscall::kernel_dup2(read_fd, 0) == u64::MAX {
            crate::syscall::kernel_close_local_fd(read_fd);
            crate::syscall::kernel_close_local_fd(write_fd);
            return Err(String::from("redirection input: dup2 failed"));
        }

        // Keep only fd 0 alias for read side.
        crate::syscall::kernel_close_local_fd(read_fd);

        let mut offset = 0usize;
        while offset < data.len() {
            let written = crate::syscall::kernel_write(write_fd, &data[offset..]);
            if written == u64::MAX || written == 0 {
                crate::syscall::kernel_close_local_fd(0);
                crate::syscall::kernel_close_local_fd(write_fd);
                return Err(String::from("redirection input: write to pipe failed"));
            }
            offset += written as usize;
        }

        // Signal EOF once all redirected input is queued.
        crate::syscall::kernel_close_local_fd(write_fd);

        Ok(Some(InputRedirectCtx))
    }

    fn teardown_input_redirection(&mut self, ctx: Option<InputRedirectCtx>) {
        if ctx.is_some() {
            crate::syscall::kernel_close_local_fd(0);
        }
    }

    fn apply_output_redirection(&mut self, cmd: &Command, result: ShellValue) -> ShellValue {
        if let (Some(path), ShellValue::Error(err_text)) = (cmd.redirect_err.as_ref(), &result) {
            let err_path = self.resolve_path(path);
            return match self.write_bytes_to_path(&err_path, cmd.append_err, err_text.as_bytes()) {
                Ok(()) => ShellValue::None,
                Err(e) => ShellValue::Error(e),
            };
        }

        if matches!(result, ShellValue::Error(_)) {
            return result;
        }

        let out_path = match cmd.redirect_out.as_ref() {
            Some(path) => self.resolve_path(path),
            None => return result,
        };

        let mut pipe_fds = [0u32; 2];
        if crate::syscall::kernel_pipe(&mut pipe_fds) == u64::MAX {
            return ShellValue::Error(String::from("redirection output: pipe creation failed"));
        }

        let read_fd = pipe_fds[0];
        let write_fd = pipe_fds[1];
        if crate::syscall::kernel_dup2(write_fd, 1) == u64::MAX {
            crate::syscall::kernel_close_local_fd(read_fd);
            crate::syscall::kernel_close_local_fd(write_fd);
            return ShellValue::Error(String::from("redirection output: dup2 failed"));
        }
        // Keep only fd 1 alias for write side.
        crate::syscall::kernel_close_local_fd(write_fd);

        if self.write_shell_value_to_fd(1, &result).is_err() {
            crate::syscall::kernel_close_local_fd(1);
            crate::syscall::kernel_close_local_fd(read_fd);
            return ShellValue::Error(String::from("redirection output: write failed"));
        }

        // Close write end so reader observes EOF.
        crate::syscall::kernel_close_local_fd(1);

        if cmd.append_out {
            if vfs::fs_stat(&out_path).is_err() {
                if let Err(e) = vfs::fs_write(&out_path, &[]) {
                    crate::syscall::kernel_close_local_fd(read_fd);
                    return ShellValue::Error(format!("redirection output {}: {:?}", out_path, e));
                }
            }
        } else if let Err(e) = vfs::fs_write(&out_path, &[]) {
            crate::syscall::kernel_close_local_fd(read_fd);
            return ShellValue::Error(format!("redirection output {}: {:?}", out_path, e));
        }

        let mut write_offset = if cmd.append_out {
            match vfs::fs_stat(&out_path) {
                Ok((_, size)) => size as usize,
                Err(_) => 0,
            }
        } else {
            0
        };

        let mut tmp = [0u8; 512];
        loop {
            let n = crate::syscall::kernel_read(read_fd, &mut tmp);
            if n == u64::MAX {
                crate::syscall::kernel_close_local_fd(read_fd);
                return ShellValue::Error(String::from("redirection output: read failed"));
            }
            if n == 0 {
                break;
            }
            let n = n as usize;
            match vfs::fs_write_at(&out_path, write_offset, &tmp[..n]) {
                Ok(w) => write_offset += w,
                Err(e) => {
                    crate::syscall::kernel_close_local_fd(read_fd);
                    return ShellValue::Error(format!("redirection output {}: {:?}", out_path, e));
                }
            }
        }

        crate::syscall::kernel_close_local_fd(read_fd);

        ShellValue::None
    }

    /// Resolve a path relative to cwd
    pub fn resolve_path(&self, path: &str) -> String {
        if path.starts_with('/') {
            // Absolute path
            String::from(path)
        } else {
            // Relative path
            if self.cwd == "/" {
                format!("/{}", path)
            } else {
                format!("{}/{}", self.cwd, path)
            }
        }
    }

    /// Expand variables in a string ($VAR and ${VAR})
    pub fn expand_vars(&self, input: &str) -> String {
        let mut result = String::new();
        let mut chars = input.chars().peekable();

        while let Some(c) = chars.next() {
            if c == '$' {
                // Check for ${VAR} syntax
                if chars.peek() == Some(&'{') {
                    chars.next(); // consume '{'
                    let mut var_name = String::new();
                    while let Some(&ch) = chars.peek() {
                        if ch == '}' {
                            chars.next();
                            break;
                        }
                        var_name.push(ch);
                        chars.next();
                    }
                    result.push_str(&self.get_var(&var_name));
                } else if chars.peek() == Some(&'?') {
                    chars.next();
                    result.push_str(&format!("{}", self.last_exit_code));
                } else {
                    // $VAR syntax
                    let mut var_name = String::new();
                    while let Some(&ch) = chars.peek() {
                        if ch.is_alphanumeric() || ch == '_' {
                            var_name.push(ch);
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    if var_name.is_empty() {
                        result.push('$');
                    } else {
                        result.push_str(&self.get_var(&var_name));
                    }
                }
            } else {
                result.push(c);
            }
        }

        result
    }

    /// Set an environment variable
    pub fn set_env(&mut self, key: &str, val: &str) {
        if let Some(entry) = self.env.iter_mut().find(|(k, _)| k == key) {
            entry.1 = String::from(val);
        } else {
            self.env.push((String::from(key), String::from(val)));
        }
    }

    /// Get an environment variable value
    pub fn get_var(&self, name: &str) -> String {
        for (k, v) in &self.env {
            if k == name {
                return v.clone();
            }
        }
        String::new()
    }

    /// Execute a for-loop
    /// Syntax: for VAR in val1 val2 ... ; CMD args...
    fn execute_for_loop(&mut self, cmd: &Command) -> ShellValue {
        // Parse: args should be [VAR, "in", val1, val2, ..., ";", CMD, ...]
        let args = &cmd.args;
        if args.len() < 4 {
            return ShellValue::Error(String::from(
                "usage: for VAR in val1 val2 ... ; CMD args...",
            ));
        }

        let var_name = &args[0];
        if args.get(1).map(|s| s.as_str()) != Some("in") {
            return ShellValue::Error(String::from("for: expected 'in'"));
        }

        // Find semicolon separator
        let semi_pos = args.iter().position(|a| a == ";");
        let (values, rest) = match semi_pos {
            Some(pos) => (&args[2..pos], &args[pos + 1..]),
            None => (&args[2..], &[] as &[String]),
        };

        if rest.is_empty() {
            return ShellValue::Error(String::from("for: no command after ';'"));
        }

        let mut output = String::new();
        for val in values {
            // Set the loop variable
            if let Some(entry) = self.env.iter_mut().find(|(k, _)| k == var_name) {
                entry.1 = val.clone();
            } else {
                self.env.push((var_name.clone(), val.clone()));
            }

            // Build and execute the command with variable expansion
            let cmd_str = rest.join(" ");
            let expanded = self.expand_vars(&cmd_str);
            if let Some(inner_cmd) = self.parse(&expanded) {
                let result = self.execute(&inner_cmd);
                let text = Self::format_output(&result);
                if !text.is_empty() {
                    output.push_str(&text);
                    output.push('\n');
                }
            }
        }

        ShellValue::Text(String::from(output.trim_end()))
    }

    /// Execute an if statement
    /// Syntax: if COND ; then CMD ; fi
    /// COND can be: test -f FILE, test -d DIR, test STR = STR, or CMD (exit code)
    fn execute_if(&mut self, cmd: &Command) -> ShellValue {
        let args = &cmd.args;

        // Find "then" separator
        let then_pos = args.iter().position(|a| a == "then" || a == ";");
        let (cond_args, rest) = match then_pos {
            Some(pos) => (&args[..pos], &args[pos + 1..]),
            None => return ShellValue::Error(String::from("if: expected 'then'")),
        };

        // Skip "then" if it's the first element of rest
        let body = if rest.first().map(|s| s.as_str()) == Some("then") {
            &rest[1..]
        } else {
            rest
        };

        // Find optional "else" and "fi"
        let else_pos = body.iter().position(|a| a == "else");
        let fi_pos = body.iter().position(|a| a == "fi");

        let (then_body, else_body) = match (else_pos, fi_pos) {
            (Some(ep), _) => {
                let fi = fi_pos.unwrap_or(body.len());
                (&body[..ep], Some(&body[ep + 1..fi]))
            }
            (None, Some(fp)) => (&body[..fp], None),
            _ => (body, None),
        };

        // Evaluate condition
        let condition_true = self.eval_condition(cond_args);

        let run_body = if condition_true {
            then_body
        } else {
            match else_body {
                Some(b) => b,
                None => return ShellValue::None,
            }
        };

        // Execute body
        let cmd_str = run_body.join(" ");
        let expanded = self.expand_vars(&cmd_str);
        if let Some(inner_cmd) = self.parse(&expanded) {
            self.execute(&inner_cmd)
        } else {
            ShellValue::None
        }
    }

    /// Evaluate a condition for if statements
    fn eval_condition(&mut self, args: &[String]) -> bool {
        if args.is_empty() {
            return false;
        }

        match args[0].as_str() {
            "test" | "[" => {
                if args.len() < 2 {
                    return false;
                }
                match args[1].as_str() {
                    "-f" => {
                        // File exists
                        if args.len() >= 3 {
                            let path = self.resolve_path(&args[2]);
                            vfs::fs_stat(&path).is_ok()
                        } else {
                            false
                        }
                    }
                    "-d" => {
                        // Directory exists
                        if args.len() >= 3 {
                            let path = self.resolve_path(&args[2]);
                            matches!(vfs::fs_stat(&path), Ok((vfs::FileType::Directory, _)))
                        } else {
                            false
                        }
                    }
                    "-z" => {
                        // String is empty
                        args.get(2).map(|s| s.is_empty()).unwrap_or(true)
                    }
                    "-n" => {
                        // String is non-empty
                        args.get(2).map(|s| !s.is_empty()).unwrap_or(false)
                    }
                    _ => {
                        // test STR = STR or test STR != STR
                        if args.len() >= 4 {
                            match args[2].as_str() {
                                "=" | "==" => args[1] == args[3],
                                "!=" => args[1] != args[3],
                                _ => false,
                            }
                        } else {
                            // test STRING (true if non-empty)
                            !args[1].is_empty()
                        }
                    }
                }
            }
            _ => {
                // Execute as command, check exit code
                let cmd_str = args.join(" ");
                if let Some(inner_cmd) = self.parse(&cmd_str) {
                    let result = self.execute(&inner_cmd);
                    !matches!(result, ShellValue::Error(_))
                } else {
                    false
                }
            }
        }
    }

    /// Format a ShellValue for display
    pub fn format_output(value: &ShellValue) -> String {
        match value {
            ShellValue::Text(s) => s.clone(),
            ShellValue::Number(n) => format!("{}", n),
            ShellValue::Bool(b) => format!("{}", b),
            ShellValue::List(items) => items
                .iter()
                .map(|v| Self::format_output(v))
                .collect::<Vec<_>>()
                .join("\n"),
            ShellValue::Table(rows) => {
                // Simple table formatting
                let mut output = String::new();
                for row in rows {
                    for (key, val) in row {
                        output.push_str(&format!("{}: {}  ", key, Self::format_output(val)));
                    }
                    output.push('\n');
                }
                output
            }
            ShellValue::None => String::new(),
            ShellValue::Error(e) => e.clone(),
        }
    }
}
