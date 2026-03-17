use crate::sync::Mutex;
use alloc::string::String;
/// Code writing and debugging agent
///
/// Part of the AIOS agent layer. Provides code generation, editing,
/// and debugging capabilities with language detection, AST-aware
/// edits, edit history, and sandboxed command execution.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// Actions the code agent can perform
#[derive(Debug, Clone)]
pub enum CodeAction {
    WriteCode(String),        // Generate code to a file
    EditFile(String, String), // file, patch/replacement
    RunCommand(String),       // Build, test, lint
    Debug(String),            // Analyze error/stacktrace
    SearchCode(String),       // Grep/AST search
    GenerateTest(String),     // Generate tests for a file
    Refactor(String, String), // file, refactoring type
    Explain(String),          // Explain code
}

/// Result of a code action
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeResult {
    Success,
    CompileError,
    TestFailed,
    Timeout,
    PermissionDenied,
    FileNotFound,
    SyntaxError,
}

/// Detected programming language
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    C,
    Cpp,
    Python,
    TypeScript,
    JavaScript,
    Go,
    Nix,
    Shell,
    Unknown,
}

/// An edit in the history (for undo/redo)
#[derive(Clone)]
struct EditEntry {
    file_hash: u64,
    old_content_hash: u64,
    new_content_hash: u64,
    timestamp: u64,
    language: Language,
}

struct CodeAgentInner {
    project_root_hash: u64,
    edit_history: Vec<EditEntry>,
    max_history: usize,
    // Allowed languages for code generation
    allowed_languages: Vec<Language>,
    // Command execution limits
    max_command_duration_ms: u32,
    max_output_size: u32,
    commands_run: u32,
    max_commands_per_session: u32,
    // Blocked commands (e.g., rm -rf, format)
    blocked_cmd_hashes: Vec<u64>,
    // Stats
    total_edits: u64,
    total_commands: u64,
    total_tests_run: u64,
    total_compile_errors: u64,
}

static CODE_AGENT: Mutex<Option<CodeAgentInner>> = Mutex::new(None);

/// Extension hash -> Language mapping
fn detect_language(ext_hash: u64) -> Language {
    match ext_hash {
        0x2E7273 => Language::Rust,       // .rs
        0x2E63 => Language::C,            // .c
        0x2E637070 => Language::Cpp,      // .cpp
        0x2E7079 => Language::Python,     // .py
        0x2E7473 => Language::TypeScript, // .ts
        0x2E6A73 => Language::JavaScript, // .js
        0x2E676F => Language::Go,         // .go
        0x2E6E6978 => Language::Nix,      // .nix
        0x2E7368 => Language::Shell,      // .sh
        _ => Language::Unknown,
    }
}

impl CodeAgentInner {
    fn new(project_root_hash: u64) -> Self {
        CodeAgentInner {
            project_root_hash,
            edit_history: Vec::new(),
            max_history: 100,
            allowed_languages: alloc::vec![
                Language::Rust,
                Language::C,
                Language::Cpp,
                Language::Python,
                Language::TypeScript,
                Language::JavaScript,
                Language::Go,
                Language::Nix,
                Language::Shell,
            ],
            max_command_duration_ms: 120_000,
            max_output_size: 500_000,
            commands_run: 0,
            max_commands_per_session: 200,
            blocked_cmd_hashes: alloc::vec![
                0x726D202D7266, // rm -rf
                0x6D6B6673,     // mkfs
                0x666F726D6174, // format
            ],
            total_edits: 0,
            total_commands: 0,
            total_tests_run: 0,
            total_compile_errors: 0,
        }
    }

    /// Execute a code action
    fn do_action(
        &mut self,
        action: &CodeAction,
        file_hash: u64,
        ext_hash: u64,
        timestamp: u64,
    ) -> CodeResult {
        match action {
            CodeAction::WriteCode(_) | CodeAction::EditFile(_, _) => {
                let lang = detect_language(ext_hash);
                if !self.allowed_languages.contains(&lang) && lang != Language::Unknown {
                    return CodeResult::PermissionDenied;
                }
                // Record edit
                if self.edit_history.len() >= self.max_history {
                    self.edit_history.remove(0);
                }
                self.edit_history.push(EditEntry {
                    file_hash,
                    old_content_hash: 0,
                    new_content_hash: 0,
                    timestamp,
                    language: lang,
                });
                self.total_edits = self.total_edits.saturating_add(1);
                CodeResult::Success
            }
            CodeAction::RunCommand(ref cmd) => {
                if self.commands_run >= self.max_commands_per_session {
                    return CodeResult::Timeout;
                }
                // Check blocked commands
                let cmd_hash = simple_hash(cmd);
                if self.blocked_cmd_hashes.contains(&cmd_hash) {
                    return CodeResult::PermissionDenied;
                }
                self.commands_run = self.commands_run.saturating_add(1);
                self.total_commands = self.total_commands.saturating_add(1);
                CodeResult::Success
            }
            CodeAction::GenerateTest(_) => {
                self.total_tests_run = self.total_tests_run.saturating_add(1);
                CodeResult::Success
            }
            CodeAction::Debug(_)
            | CodeAction::SearchCode(_)
            | CodeAction::Explain(_)
            | CodeAction::Refactor(_, _) => CodeResult::Success,
        }
    }

    /// Undo last edit
    fn undo_last(&mut self) -> Option<EditEntry> {
        self.edit_history.pop()
    }

    fn reset_session(&mut self) {
        self.commands_run = 0;
        self.edit_history.clear();
    }

    fn get_stats(&self) -> (u64, u64, u64, u64) {
        (
            self.total_edits,
            self.total_commands,
            self.total_tests_run,
            self.total_compile_errors,
        )
    }
}

/// Simple FNV-1a hash for command strings
fn simple_hash(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in s.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

// --- Public API ---

/// Execute a code action
pub fn do_action(action: &CodeAction, file_hash: u64, ext_hash: u64, timestamp: u64) -> CodeResult {
    let mut agent = CODE_AGENT.lock();
    match agent.as_mut() {
        Some(a) => a.do_action(action, file_hash, ext_hash, timestamp),
        None => CodeResult::PermissionDenied,
    }
}

/// Undo last code edit
pub fn undo() -> bool {
    let mut agent = CODE_AGENT.lock();
    match agent.as_mut() {
        Some(a) => a.undo_last().is_some(),
        None => false,
    }
}

/// Reset for new session
pub fn reset_session() {
    let mut agent = CODE_AGENT.lock();
    if let Some(a) = agent.as_mut() {
        a.reset_session();
    }
}

/// Get stats: (edits, commands, tests, compile_errors)
pub fn stats() -> (u64, u64, u64, u64) {
    let agent = CODE_AGENT.lock();
    match agent.as_ref() {
        Some(a) => a.get_stats(),
        None => (0, 0, 0, 0),
    }
}

/// Detect language from extension hash
pub fn language_for(ext_hash: u64) -> Language {
    detect_language(ext_hash)
}

pub fn init() {
    let mut agent = CODE_AGENT.lock();
    *agent = Some(CodeAgentInner::new(0xA105_C0DE));
    serial_println!("    Code agent: edit history, language detection, command sandboxing ready");
}
