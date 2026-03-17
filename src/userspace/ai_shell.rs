/// AI-powered shell for Genesis
///
/// Command prediction, smart completion, natural language commands,
/// error explanation, context-aware suggestions, learning from usage.
///
/// Inspired by: GitHub Copilot CLI, Fish Shell, Zsh AutoSuggestions. All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Command suggestion from AI
pub struct CommandSuggestion {
    pub command: String,
    pub description: String,
    /// Confidence score 0-100 (integer, no floats)
    pub confidence: u32,
    pub source: SuggestionSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionSource {
    History,
    Prediction,
    NaturalLanguage,
    ErrorFix,
    ContextAware,
}

/// Error explanation from AI
pub struct ErrorExplanation {
    pub error_text: String,
    pub explanation: String,
    pub suggested_fix: String,
    /// Confidence score 0-100 (integer, no floats)
    pub confidence: u32,
}

/// Natural language command parse result
pub struct NlCommand {
    pub interpreted_command: String,
    /// Confidence score 0-100 (integer, no floats)
    pub confidence: u32,
    pub alternatives: Vec<String>,
}

/// AI shell engine
pub struct AiShellEngine {
    pub enabled: bool,
    pub command_history: Vec<String>,
    pub command_frequency: BTreeMap<String, u32>,
    pub command_sequences: Vec<(String, String)>,
    pub error_patterns: BTreeMap<String, String>,
    pub context_commands: BTreeMap<String, Vec<String>>,
    pub total_suggestions: u64,
    pub accepted_suggestions: u64,
    pub nl_commands_parsed: u64,
}

impl AiShellEngine {
    const fn new() -> Self {
        AiShellEngine {
            enabled: true,
            command_history: Vec::new(),
            command_frequency: BTreeMap::new(),
            command_sequences: Vec::new(),
            error_patterns: BTreeMap::new(),
            context_commands: BTreeMap::new(),
            total_suggestions: 0,
            accepted_suggestions: 0,
            nl_commands_parsed: 0,
        }
    }

    /// Record a command execution
    pub fn record_command(&mut self, cmd: &str) {
        // Frequency tracking
        let freq = self.command_frequency.entry(String::from(cmd)).or_insert(0);
        *freq = freq.saturating_add(1);

        // Sequence tracking (command pairs)
        if let Some(prev) = self.command_history.last() {
            self.command_sequences
                .push((prev.clone(), String::from(cmd)));
            if self.command_sequences.len() > 500 {
                self.command_sequences.remove(0);
            }
        }

        self.command_history.push(String::from(cmd));
        if self.command_history.len() > 1000 {
            self.command_history.remove(0);
        }
    }

    /// Get command suggestions based on prefix
    pub fn suggest(&mut self, prefix: &str) -> Vec<CommandSuggestion> {
        self.total_suggestions = self.total_suggestions.saturating_add(1);
        let mut suggestions = Vec::new();

        // History-based suggestions
        let prefix_lower = prefix.to_lowercase();
        for (cmd, freq) in &self.command_frequency {
            if cmd.to_lowercase().starts_with(&prefix_lower) {
                suggestions.push(CommandSuggestion {
                    command: cmd.clone(),
                    description: alloc::format!("Used {} times", freq),
                    confidence: (*freq).min(100),
                    source: SuggestionSource::History,
                });
            }
        }

        // Sequence-based prediction
        if let Some(prev) = self.command_history.last() {
            for (p, next) in &self.command_sequences {
                if p == prev && next.starts_with(prefix) {
                    suggestions.push(CommandSuggestion {
                        command: next.clone(),
                        description: alloc::format!("Often follows '{}'", p),
                        confidence: 70,
                        source: SuggestionSource::Prediction,
                    });
                }
            }
        }

        // Built-in command suggestions
        let builtins = [
            ("ls", "List files"),
            ("cd", "Change directory"),
            ("cat", "Display file"),
            ("pwd", "Print working directory"),
            ("ps", "List processes"),
            ("kill", "Kill process"),
            ("mount", "Mount filesystem"),
            ("umount", "Unmount filesystem"),
            ("ifconfig", "Network config"),
            ("ping", "Ping host"),
            ("help", "Show help"),
            ("clear", "Clear screen"),
            ("uname", "System info"),
            ("uptime", "System uptime"),
            ("free", "Memory info"),
            ("df", "Disk usage"),
            ("grep", "Search text"),
            ("find", "Find files"),
            ("chmod", "Change permissions"),
            ("chown", "Change owner"),
            ("history", "Command history"),
            ("alias", "Command aliases"),
        ];
        for (cmd, desc) in &builtins {
            if cmd.starts_with(prefix) && !suggestions.iter().any(|s| s.command == *cmd) {
                suggestions.push(CommandSuggestion {
                    command: String::from(*cmd),
                    description: String::from(*desc),
                    confidence: 50,
                    source: SuggestionSource::ContextAware,
                });
            }
        }

        suggestions.sort_by(|a, b| b.confidence.cmp(&a.confidence));
        suggestions.truncate(5);
        suggestions
    }

    /// Parse natural language into a command
    pub fn parse_natural_language(&mut self, text: &str) -> NlCommand {
        self.nl_commands_parsed = self.nl_commands_parsed.saturating_add(1);
        let lower = text.to_lowercase();

        let (cmd, alts) = if lower.contains("list") && lower.contains("file") {
            ("ls -la", vec!["ls", "ls -l"])
        } else if lower.contains("show") && lower.contains("process") {
            ("ps aux", vec!["ps", "top"])
        } else if lower.contains("find") && lower.contains("file") {
            ("find . -name", vec!["ls -R", "locate"])
        } else if lower.contains("free") || (lower.contains("show") && lower.contains("memory")) {
            ("free", vec!["meminfo", "cat /proc/meminfo"])
        } else if lower.contains("disk") && (lower.contains("space") || lower.contains("usage")) {
            ("df -h", vec!["du -sh *"])
        } else if lower.contains("network") || lower.contains("ip address") {
            ("ifconfig", vec!["ip addr", "hostname -I"])
        } else if lower.contains("kill") || lower.contains("stop") {
            ("kill", vec!["pkill", "killall"])
        } else if lower.contains("shutdown") || lower.contains("power off") {
            ("shutdown", vec!["poweroff", "halt"])
        } else if lower.contains("reboot") || lower.contains("restart") {
            ("reboot", vec!["shutdown -r now"])
        } else if lower.contains("who") && lower.contains("logged") {
            ("who", vec!["w", "users"])
        } else if lower.contains("date") || lower.contains("time") {
            ("date", vec!["timedatectl", "clock"])
        } else if lower.contains("edit") {
            ("edit", vec!["vi", "nano"])
        } else if lower.contains("search") || lower.contains("grep") {
            ("grep -r", vec!["find . -name", "rg"])
        } else {
            ("help", vec![])
        };

        NlCommand {
            interpreted_command: String::from(cmd),
            confidence: 75,
            alternatives: alts
                .iter()
                .map(|a| String::from(*a))
                .collect::<Vec<String>>(),
        }
    }

    /// Explain an error message
    pub fn explain_error(&self, error: &str) -> ErrorExplanation {
        let lower = error.to_lowercase();

        let (explanation, fix) = if lower.contains("permission denied") {
            (
                "You don't have permission to access this resource.",
                "Try running with 'sudo' or check file permissions with 'ls -la'",
            )
        } else if lower.contains("command not found") {
            (
                "The command you typed is not installed or not in PATH.",
                "Check spelling, or install the package containing this command",
            )
        } else if lower.contains("no such file") {
            (
                "The specified file or directory doesn't exist.",
                "Check the path with 'ls' or use tab completion",
            )
        } else if lower.contains("is a directory") {
            (
                "You tried to use a directory where a file was expected.",
                "Specify a file within the directory, or use 'ls' to see contents",
            )
        } else if lower.contains("disk full") || lower.contains("no space") {
            (
                "The disk is full — no more data can be written.",
                "Free space with 'df -h' to check, then delete unneeded files",
            )
        } else if lower.contains("connection refused") {
            (
                "The target service is not running or not accepting connections.",
                "Check if the service is running with 'ps' or try a different port",
            )
        } else if lower.contains("timeout") {
            (
                "The operation took too long and was aborted.",
                "Check network connectivity or increase timeout value",
            )
        } else if lower.contains("segfault") || lower.contains("segmentation fault") {
            (
                "A program tried to access memory it shouldn't — this is a bug.",
                "Report the crash. Try restarting the application",
            )
        } else {
            (
                "An error occurred during command execution.",
                "Try 'help' for available commands or check syntax",
            )
        };

        ErrorExplanation {
            error_text: String::from(error),
            explanation: String::from(explanation),
            suggested_fix: String::from(fix),
            confidence: 80,
        }
    }

    /// Get context-aware suggestions (based on current directory, recent commands)
    pub fn context_suggestions(&self) -> Vec<CommandSuggestion> {
        let mut suggestions = Vec::new();

        // After 'git status', suggest 'git add' or 'git commit'
        if let Some(last) = self.command_history.last() {
            if last.starts_with("git status") {
                suggestions.push(CommandSuggestion {
                    command: String::from("git add ."),
                    description: String::from("Stage all changes"),
                    confidence: 80,
                    source: SuggestionSource::ContextAware,
                });
            }
            if last.starts_with("ls") {
                suggestions.push(CommandSuggestion {
                    command: String::from("cd"),
                    description: String::from("Change to a listed directory"),
                    confidence: 60,
                    source: SuggestionSource::ContextAware,
                });
            }
        }

        suggestions
    }
}

fn seed_error_patterns(engine: &mut AiShellEngine) {
    let patterns = [
        ("ENOENT", "File not found"),
        ("EACCES", "Permission denied"),
        ("ENOSPC", "No space left on device"),
        ("ENOMEM", "Out of memory"),
        ("ECONNREFUSED", "Connection refused"),
        ("ETIMEDOUT", "Connection timed out"),
    ];
    for (code, desc) in &patterns {
        engine
            .error_patterns
            .insert(String::from(*code), String::from(*desc));
    }
}

static AI_SHELL: Mutex<AiShellEngine> = Mutex::new(AiShellEngine::new());

pub fn init() {
    seed_error_patterns(&mut AI_SHELL.lock());
    crate::serial_println!(
        "    [ai-shell] AI shell assistant initialized (suggest, NL, explain, context)"
    );
}

pub fn record_command(cmd: &str) {
    AI_SHELL.lock().record_command(cmd);
}
pub fn suggest(prefix: &str) -> Vec<CommandSuggestion> {
    AI_SHELL.lock().suggest(prefix)
}
pub fn parse_nl(text: &str) -> NlCommand {
    AI_SHELL.lock().parse_natural_language(text)
}
pub fn explain_error(error: &str) -> ErrorExplanation {
    AI_SHELL.lock().explain_error(error)
}
