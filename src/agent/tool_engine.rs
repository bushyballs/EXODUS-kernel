use crate::sync::Mutex;
/// Tool engine for Genesis agent framework
///
/// Defines the tool registry, tool calling protocol,
/// permission system, and execution pipeline.
/// Inspired by function-calling in Claude/GPT but native to Genesis.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum ToolCategory {
    FileRead,  // read_file, glob, grep
    FileWrite, // write_file, edit_file
    Execute,   // run_command, shell
    Search,    // search_code, search_web
    Network,   // fetch_url, api_call
    System,    // process_list, system_info
    Ui,        // browser_click, screenshot
    Memory,    // store_memory, recall
    Custom,    // user-defined skill tools
}

#[derive(Clone, Copy, PartialEq)]
pub enum PermissionLevel {
    Auto,         // Always allowed (read-only ops)
    Prompt,       // Ask user before executing
    Deny,         // Never allowed
    SessionAllow, // Allowed for this session
}

#[derive(Clone, Copy, PartialEq)]
pub enum ToolStatus {
    Success,
    Error,
    Denied,
    Timeout,
    Pending,
}

#[derive(Clone, Copy)]
pub struct ToolDefinition {
    pub id: u32,
    pub name_hash: u64,
    pub category: ToolCategory,
    pub permission: PermissionLevel,
    pub timeout_ms: u32,
    pub max_output_bytes: u32,
    pub reversible: bool,  // Can this tool's action be undone?
    pub destructive: bool, // Does it modify/delete data?
    pub requires_confirmation: bool,
}

#[derive(Clone, Copy)]
pub struct ToolCall {
    pub call_id: u32,
    pub tool_id: u32,
    pub param_hash: u64, // Hash of serialized params
    pub timestamp: u64,
    pub status: ToolStatus,
    pub duration_ms: u32,
    pub output_hash: u64,
    pub output_size: u32,
    pub session_id: u32,
}

struct ToolEngine {
    tools: Vec<ToolDefinition>,
    call_history: Vec<ToolCall>,
    next_tool_id: u32,
    next_call_id: u32,
    total_calls: u64,
    total_denied: u32,
    total_errors: u32,
    // Permission wildcards: (pattern_hash, permission)
    permission_overrides: Vec<(u64, PermissionLevel)>,
}

static TOOL_ENGINE: Mutex<Option<ToolEngine>> = Mutex::new(None);

impl ToolEngine {
    fn new() -> Self {
        ToolEngine {
            tools: Vec::new(),
            call_history: Vec::new(),
            next_tool_id: 1,
            next_call_id: 1,
            total_calls: 0,
            total_denied: 0,
            total_errors: 0,
            permission_overrides: Vec::new(),
        }
    }

    fn register_tool(&mut self, name_hash: u64, category: ToolCategory, destructive: bool) -> u32 {
        let id = self.next_tool_id;
        self.next_tool_id = self.next_tool_id.saturating_add(1);
        let permission = if destructive {
            PermissionLevel::Prompt
        } else {
            match category {
                ToolCategory::FileRead | ToolCategory::Search | ToolCategory::Memory => {
                    PermissionLevel::Auto
                }
                ToolCategory::FileWrite | ToolCategory::Execute => PermissionLevel::Prompt,
                ToolCategory::Network => PermissionLevel::Prompt,
                ToolCategory::System => PermissionLevel::Prompt,
                ToolCategory::Ui => PermissionLevel::Prompt,
                ToolCategory::Custom => PermissionLevel::Prompt,
            }
        };
        self.tools.push(ToolDefinition {
            id,
            name_hash,
            category,
            permission,
            timeout_ms: 30_000,
            max_output_bytes: 100_000,
            reversible: !destructive,
            destructive,
            requires_confirmation: destructive,
        });
        id
    }

    fn register_builtin_tools(&mut self) {
        // File tools
        self.register_tool(0x726561645F66696C, ToolCategory::FileRead, false); // read_file
        self.register_tool(0x677265705F636F64, ToolCategory::FileRead, false); // grep_code
        self.register_tool(0x676C6F625F66696C, ToolCategory::FileRead, false); // glob_files
        self.register_tool(0x77726974655F6669, ToolCategory::FileWrite, true); // write_file
        self.register_tool(0x656469745F66696C, ToolCategory::FileWrite, true); // edit_file
                                                                               // Execution
        self.register_tool(0x72756E5F636D6421, ToolCategory::Execute, true); // run_command
        self.register_tool(0x7368656C6C5F6578, ToolCategory::Execute, true); // shell_exec
                                                                             // Search
        self.register_tool(0x7365617263685F63, ToolCategory::Search, false); // search_code
        self.register_tool(0x7365617263685F77, ToolCategory::Search, false); // search_web
                                                                             // Memory
        self.register_tool(0x73746F72655F6D65, ToolCategory::Memory, false); // store_memory
        self.register_tool(0x726563616C6C5F6D, ToolCategory::Memory, false); // recall_memory
                                                                             // UI automation
        self.register_tool(0x62726F777365725F, ToolCategory::Ui, false); // browser_action
        self.register_tool(0x73637265656E7368, ToolCategory::Ui, false); // screenshot
                                                                         // System
        self.register_tool(0x7379735F696E666F, ToolCategory::System, false); // sys_info
        self.register_tool(0x70726F636573735F, ToolCategory::System, false); // process_list
    }

    fn execute_tool(
        &mut self,
        tool_id: u32,
        param_hash: u64,
        session_id: u32,
        timestamp: u64,
    ) -> ToolCall {
        let call_id = self.next_call_id;
        self.next_call_id = self.next_call_id.saturating_add(1);
        self.total_calls = self.total_calls.saturating_add(1);

        // Check permissions
        let tool = self.tools.iter().find(|t| t.id == tool_id);
        let status = match tool {
            None => {
                self.total_errors = self.total_errors.saturating_add(1);
                ToolStatus::Error
            }
            Some(t) => {
                let effective_perm = self.get_effective_permission(t);
                match effective_perm {
                    PermissionLevel::Deny => {
                        self.total_denied = self.total_denied.saturating_add(1);
                        ToolStatus::Denied
                    }
                    PermissionLevel::Prompt => ToolStatus::Pending, // Needs user approval
                    _ => ToolStatus::Success,                       // Auto or SessionAllow
                }
            }
        };

        let call = ToolCall {
            call_id,
            tool_id,
            param_hash,
            timestamp,
            status,
            duration_ms: 0,
            output_hash: 0,
            output_size: 0,
            session_id,
        };
        self.call_history.push(call);
        call
    }

    fn get_effective_permission(&self, tool: &ToolDefinition) -> PermissionLevel {
        // Check overrides first
        for &(pattern, perm) in &self.permission_overrides {
            if pattern == tool.name_hash {
                return perm;
            }
        }
        tool.permission
    }

    fn approve_call(&mut self, call_id: u32) {
        if let Some(call) = self.call_history.iter_mut().find(|c| c.call_id == call_id) {
            call.status = ToolStatus::Success;
        }
    }

    fn deny_call(&mut self, call_id: u32) {
        if let Some(call) = self.call_history.iter_mut().find(|c| c.call_id == call_id) {
            call.status = ToolStatus::Denied;
            self.total_denied = self.total_denied.saturating_add(1);
        }
    }

    fn set_permission_override(&mut self, tool_name_hash: u64, perm: PermissionLevel) {
        for entry in &mut self.permission_overrides {
            if entry.0 == tool_name_hash {
                entry.1 = perm;
                return;
            }
        }
        self.permission_overrides.push((tool_name_hash, perm));
    }

    fn get_history(&self, session_id: u32) -> Vec<ToolCall> {
        self.call_history
            .iter()
            .filter(|c| c.session_id == session_id)
            .copied()
            .collect()
    }

    fn undo_last(&mut self, session_id: u32) -> Option<ToolCall> {
        // Find last reversible successful call in this session
        let idx = self
            .call_history
            .iter()
            .rposition(|c| c.session_id == session_id && c.status == ToolStatus::Success);
        if let Some(i) = idx {
            let call = self.call_history[i];
            if let Some(tool) = self.tools.iter().find(|t| t.id == call.tool_id) {
                if tool.reversible {
                    return Some(call);
                }
            }
        }
        None
    }
}

pub fn init() {
    let mut engine = TOOL_ENGINE.lock();
    let mut e = ToolEngine::new();
    e.register_builtin_tools();
    *engine = Some(e);
    serial_println!("    Tool engine: 15 builtin tools, permission system ready");
}
