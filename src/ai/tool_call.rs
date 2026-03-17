use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::format;
/// AI function/tool calling dispatch
///
/// Part of the AIOS AI layer. Register callable tools with typed parameters,
/// validate arguments, dispatch calls, and track execution results.
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// Parameter types for tool definitions
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamType {
    String,
    Integer,
    Float,
    Boolean,
    Array,
    Object,
}

/// A parameter definition
#[derive(Clone)]
pub struct ParamDef {
    pub name: String,
    pub param_type: ParamType,
    pub description: String,
    pub required: bool,
    pub default_value: Option<String>,
}

impl ParamDef {
    pub fn new(name: &str, param_type: ParamType, description: &str) -> Self {
        ParamDef {
            name: String::from(name),
            param_type,
            description: String::from(description),
            required: true,
            default_value: None,
        }
    }

    pub fn optional(mut self, default: &str) -> Self {
        self.required = false;
        self.default_value = Some(String::from(default));
        self
    }
}

/// Execution status of a tool call
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolStatus {
    Success,
    Error,
    Timeout,
    PermissionDenied,
    NotFound,
    InvalidArgs,
}

/// Result of a tool execution
#[derive(Clone)]
pub struct ToolResult {
    pub tool_name: String,
    pub status: ToolStatus,
    pub output: String,
    pub execution_time_ms: u64,
    pub call_id: u64,
}

/// A registered tool the AI can invoke
#[derive(Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Vec<ParamDef>,
    /// Permission level required (0 = public, 1 = user, 2 = system, 3 = admin)
    pub permission_level: u8,
    /// Category for grouping tools
    pub category: String,
    /// Whether this tool is enabled
    pub enabled: bool,
    /// Whether this tool has side effects
    pub has_side_effects: bool,
    /// Maximum execution time in ms
    pub timeout_ms: u64,
}

impl ToolDefinition {
    pub fn new(name: &str, description: &str) -> Self {
        ToolDefinition {
            name: String::from(name),
            description: String::from(description),
            parameters: Vec::new(),
            permission_level: 0,
            category: String::from("general"),
            enabled: true,
            has_side_effects: false,
            timeout_ms: 5000,
        }
    }

    pub fn param(mut self, param: ParamDef) -> Self {
        self.parameters.push(param);
        self
    }

    pub fn permission(mut self, level: u8) -> Self {
        self.permission_level = level;
        self
    }

    pub fn category(mut self, cat: &str) -> Self {
        self.category = String::from(cat);
        self
    }

    pub fn side_effects(mut self) -> Self {
        self.has_side_effects = true;
        self
    }

    pub fn timeout(mut self, ms: u64) -> Self {
        self.timeout_ms = ms;
        self
    }

    /// Validate that the provided arguments match the parameter schema
    fn validate_args(&self, args: &BTreeMap<String, String>) -> Result<(), String> {
        // Check all required parameters are present
        for param in &self.parameters {
            if param.required && !args.contains_key(&param.name) {
                return Err(format!("Missing required parameter: {}", param.name));
            }
        }

        // Type-check provided arguments
        for (key, value) in args {
            let param = match self.parameters.iter().find(|p| p.name == *key) {
                Some(p) => p,
                None => {
                    // Unknown parameter - warn but don't fail
                    continue;
                }
            };

            match param.param_type {
                ParamType::Integer => {
                    if !value.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
                        return Err(format!(
                            "Parameter '{}' must be an integer, got: {}",
                            key, value
                        ));
                    }
                }
                ParamType::Float => {
                    let is_float = value
                        .bytes()
                        .all(|b| b.is_ascii_digit() || b == b'.' || b == b'-');
                    if !is_float {
                        return Err(format!(
                            "Parameter '{}' must be a float, got: {}",
                            key, value
                        ));
                    }
                }
                ParamType::Boolean => {
                    let lower = value.to_lowercase();
                    if lower != "true" && lower != "false" && lower != "0" && lower != "1" {
                        return Err(format!(
                            "Parameter '{}' must be a boolean, got: {}",
                            key, value
                        ));
                    }
                }
                _ => {} // String, Array, Object - accept any string representation
            }
        }

        Ok(())
    }

    /// Build effective arguments by filling in defaults for missing optional params
    fn effective_args(&self, args: &BTreeMap<String, String>) -> BTreeMap<String, String> {
        let mut effective = args.clone();
        for param in &self.parameters {
            if !effective.contains_key(&param.name) {
                if let Some(default) = &param.default_value {
                    effective.insert(param.name.clone(), default.clone());
                }
            }
        }
        effective
    }
}

/// Built-in tool handler function type
type ToolHandler = fn(&BTreeMap<String, String>) -> String;

/// A registered handler for a tool
struct ToolRegistration {
    definition: ToolDefinition,
    handler: Option<ToolHandler>,
}

/// Dispatches AI-generated tool calls to implementations
pub struct ToolDispatcher {
    pub tools: Vec<ToolDefinition>,
    /// Internal registrations with handlers
    registrations: Vec<ToolRegistration>,
    /// Execution history
    history: Vec<ToolResult>,
    /// Maximum history entries
    max_history: usize,
    /// Next call ID
    next_call_id: u64,
    /// Current permission level of the caller
    caller_permission: u8,
    /// Rate limiting: calls per tool per window
    rate_limits: BTreeMap<String, (u32, u32)>, // tool_name -> (max_calls, current_calls)
    /// Total calls dispatched
    total_calls: u64,
    /// Total errors
    total_errors: u64,
}

impl ToolDispatcher {
    pub fn new() -> Self {
        ToolDispatcher {
            tools: Vec::new(),
            registrations: Vec::new(),
            history: Vec::new(),
            max_history: 512,
            next_call_id: 1,
            caller_permission: 1, // Default: user level
            rate_limits: BTreeMap::new(),
            total_calls: 0,
            total_errors: 0,
        }
    }

    /// Register a tool definition without a handler
    pub fn register(&mut self, definition: ToolDefinition) {
        self.tools.push(definition.clone());
        self.registrations.push(ToolRegistration {
            definition,
            handler: None,
        });
    }

    /// Register a tool with a handler function
    pub fn register_with_handler(&mut self, definition: ToolDefinition, handler: ToolHandler) {
        self.tools.push(definition.clone());
        self.registrations.push(ToolRegistration {
            definition,
            handler: Some(handler),
        });
    }

    /// Set the caller's permission level
    pub fn set_permission(&mut self, level: u8) {
        self.caller_permission = level;
    }

    /// Set rate limit for a tool (max calls per window)
    pub fn set_rate_limit(&mut self, tool_name: &str, max_calls: u32) {
        self.rate_limits
            .insert(String::from(tool_name), (max_calls, 0));
    }

    /// Reset rate limit counters (call periodically)
    pub fn reset_rate_limits(&mut self) {
        for (_, (_, current)) in &mut self.rate_limits {
            *current = 0;
        }
    }

    /// Dispatch a tool call with string argument (key=value pairs separated by commas)
    pub fn dispatch(&self, name: &str, args: &str) -> String {
        let parsed_args = parse_args(args);
        match self.dispatch_with_map(name, &parsed_args) {
            Ok(result) => result.output,
            Err(e) => format!("Error: {}", e),
        }
    }

    /// Dispatch a tool call with structured arguments
    pub fn dispatch_with_map(
        &self,
        name: &str,
        args: &BTreeMap<String, String>,
    ) -> Result<ToolResult, String> {
        // Find the tool registration
        let reg = self
            .registrations
            .iter()
            .find(|r| r.definition.name == name)
            .ok_or_else(|| format!("Tool not found: {}", name))?;

        let def = &reg.definition;

        // Check if enabled
        if !def.enabled {
            return Err(format!("Tool '{}' is currently disabled", name));
        }

        // Check permissions
        if def.permission_level > self.caller_permission {
            return Ok(ToolResult {
                tool_name: String::from(name),
                status: ToolStatus::PermissionDenied,
                output: format!(
                    "Permission denied: requires level {}, caller has {}",
                    def.permission_level, self.caller_permission
                ),
                execution_time_ms: 0,
                call_id: 0,
            });
        }

        // Validate arguments
        if let Err(e) = def.validate_args(args) {
            return Ok(ToolResult {
                tool_name: String::from(name),
                status: ToolStatus::InvalidArgs,
                output: e,
                execution_time_ms: 0,
                call_id: 0,
            });
        }

        // Build effective arguments (with defaults)
        let effective = def.effective_args(args);

        // Execute handler
        let output = match &reg.handler {
            Some(handler) => handler(&effective),
            None => {
                // No handler registered - return a synthetic response
                format!(
                    "Tool '{}' called with {} args (no handler registered)",
                    name,
                    effective.len()
                )
            }
        };

        Ok(ToolResult {
            tool_name: String::from(name),
            status: ToolStatus::Success,
            output,
            execution_time_ms: 0,
            call_id: 0,
        })
    }

    /// Dispatch and record in history
    pub fn dispatch_and_record(
        &mut self,
        name: &str,
        args: &BTreeMap<String, String>,
    ) -> ToolResult {
        self.total_calls = self.total_calls.saturating_add(1);
        let call_id = self.next_call_id;
        self.next_call_id = self.next_call_id.saturating_add(1);

        // Rate limiting check
        if let Some((max, current)) = self.rate_limits.get_mut(name) {
            if *current >= *max {
                let result = ToolResult {
                    tool_name: String::from(name),
                    status: ToolStatus::Error,
                    output: format!(
                        "Rate limit exceeded for '{}': {}/{} calls",
                        name, current, max
                    ),
                    execution_time_ms: 0,
                    call_id,
                };
                self.total_errors = self.total_errors.saturating_add(1);
                self.record_result(result.clone());
                return result;
            }
            *current = current.saturating_add(1);
        }

        let mut result = match self.dispatch_with_map(name, args) {
            Ok(r) => r,
            Err(e) => {
                self.total_errors = self.total_errors.saturating_add(1);
                ToolResult {
                    tool_name: String::from(name),
                    status: ToolStatus::NotFound,
                    output: e,
                    execution_time_ms: 0,
                    call_id,
                }
            }
        };

        result.call_id = call_id;
        if result.status != ToolStatus::Success {
            self.total_errors = self.total_errors.saturating_add(1);
        }

        self.record_result(result.clone());
        result
    }

    /// Record a tool result in history
    fn record_result(&mut self, result: ToolResult) {
        self.history.push(result);
        if self.history.len() > self.max_history {
            self.history.remove(0);
        }
    }

    /// List all registered tools
    pub fn list_tools(&self) -> Vec<&ToolDefinition> {
        self.tools.iter().collect()
    }

    /// List tools in a specific category
    pub fn tools_in_category(&self, category: &str) -> Vec<&ToolDefinition> {
        self.tools
            .iter()
            .filter(|t| t.category == category)
            .collect()
    }

    /// Find a tool by name
    pub fn find_tool(&self, name: &str) -> Option<&ToolDefinition> {
        self.tools.iter().find(|t| t.name == name)
    }

    /// Get recent execution history
    pub fn recent_history(&self, n: usize) -> Vec<&ToolResult> {
        let start = if self.history.len() > n {
            self.history.len() - n
        } else {
            0
        };
        self.history[start..].iter().collect()
    }

    /// Enable or disable a tool
    pub fn set_enabled(&mut self, name: &str, enabled: bool) {
        for reg in &mut self.registrations {
            if reg.definition.name == name {
                reg.definition.enabled = enabled;
            }
        }
        for tool in &mut self.tools {
            if tool.name == name {
                tool.enabled = enabled;
            }
        }
    }

    /// Get tool count
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    /// Total calls dispatched
    pub fn total_calls(&self) -> u64 {
        self.total_calls
    }

    /// Total errors
    pub fn total_errors(&self) -> u64 {
        self.total_errors
    }

    /// Success rate
    pub fn success_rate(&self) -> f32 {
        if self.total_calls == 0 {
            return 1.0;
        }
        1.0 - (self.total_errors as f32 / self.total_calls as f32)
    }

    /// Generate a tool schema description (for inclusion in prompts)
    pub fn schema_description(&self) -> String {
        let mut desc = String::from("Available tools:\n");
        for tool in &self.tools {
            if !tool.enabled {
                continue;
            }
            desc.push_str(&format!("\n- {}: {}\n", tool.name, tool.description));
            for param in &tool.parameters {
                let req = if param.required {
                    "required"
                } else {
                    "optional"
                };
                desc.push_str(&format!(
                    "    {}: {:?} ({}) - {}\n",
                    param.name, param.param_type, req, param.description
                ));
            }
        }
        desc
    }
}

// ---------------------------------------------------------------------------
// Built-in tool handlers
// ---------------------------------------------------------------------------

fn handle_echo(args: &BTreeMap<String, String>) -> String {
    args.get("message")
        .cloned()
        .unwrap_or_else(|| String::from(""))
}

fn handle_calc(args: &BTreeMap<String, String>) -> String {
    let expr = match args.get("expression") {
        Some(e) => e,
        None => return String::from("Error: no expression provided"),
    };

    // Simple calculator: supports +, -, *, / on two numbers
    // Format: "NUM OP NUM"
    let parts: Vec<&str> = expr.split_whitespace().collect();
    if parts.len() != 3 {
        return format!("Error: expected 'NUM OP NUM', got: {}", expr);
    }

    let a: f64 = match parse_float(parts[0]) {
        Some(v) => v,
        None => return format!("Error: invalid number: {}", parts[0]),
    };
    let b: f64 = match parse_float(parts[2]) {
        Some(v) => v,
        None => return format!("Error: invalid number: {}", parts[2]),
    };

    let result = match parts[1] {
        "+" => a + b,
        "-" => a - b,
        "*" => a * b,
        "/" => {
            if b == 0.0 {
                return String::from("Error: division by zero");
            }
            a / b
        }
        op => return format!("Error: unknown operator: {}", op),
    };

    format!("{}", result as f32)
}

fn handle_time(_args: &BTreeMap<String, String>) -> String {
    String::from("System time unavailable in no_std (use system clock)")
}

fn handle_search(args: &BTreeMap<String, String>) -> String {
    let query = args.get("query").cloned().unwrap_or_default();
    format!(
        "Search results for '{}': (local search not yet indexed)",
        query
    )
}

fn handle_help(args: &BTreeMap<String, String>) -> String {
    let topic = args
        .get("topic")
        .cloned()
        .unwrap_or_else(|| String::from("general"));
    format!(
        "Help for '{}': Use 'list_tools' to see available tools.",
        topic
    )
}

// ---------------------------------------------------------------------------
// Argument parsing
// ---------------------------------------------------------------------------

/// Parse a string like "key1=value1, key2=value2" into a map
fn parse_args(args_str: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    if args_str.is_empty() {
        return map;
    }

    for pair in args_str.split(',') {
        let pair = pair.trim();
        if let Some(eq_pos) = pair.find('=') {
            let key = pair[..eq_pos].trim();
            let value = pair[eq_pos + 1..].trim();
            if !key.is_empty() {
                // Strip surrounding quotes from value if present
                let value = if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
                    &value[1..value.len() - 1]
                } else {
                    value
                };
                map.insert(String::from(key), String::from(value));
            }
        }
    }

    map
}

fn parse_float(s: &str) -> Option<f64> {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return None;
    }

    let mut result = 0.0f64;
    let mut fraction = 0.0f64;
    let mut divisor = 1.0f64;
    let mut negative = false;
    let mut in_fraction = false;
    let mut i = 0;

    if bytes[0] == b'-' {
        negative = true;
        i = 1;
    } else if bytes[0] == b'+' {
        i = 1;
    }

    while i < bytes.len() {
        let b = bytes[i];
        if b == b'.' {
            in_fraction = true;
        } else if b.is_ascii_digit() {
            let d = (b - b'0') as f64;
            if in_fraction {
                divisor *= 10.0;
                fraction += d / divisor;
            } else {
                result = result * 10.0 + d;
            }
        } else {
            return None;
        }
        i += 1;
    }

    let value = result + fraction;
    Some(if negative { -value } else { value })
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static DISPATCHER: Mutex<Option<ToolDispatcher>> = Mutex::new(None);

pub fn init() {
    let mut dispatcher = ToolDispatcher::new();

    // Register built-in tools
    dispatcher.register_with_handler(
        ToolDefinition::new("echo", "Echo back a message")
            .param(ParamDef::new(
                "message",
                ParamType::String,
                "The message to echo",
            ))
            .category("utility"),
        handle_echo,
    );

    dispatcher.register_with_handler(
        ToolDefinition::new("calc", "Simple arithmetic calculator")
            .param(ParamDef::new(
                "expression",
                ParamType::String,
                "Expression: NUM OP NUM",
            ))
            .category("math"),
        handle_calc,
    );

    dispatcher.register_with_handler(
        ToolDefinition::new("time", "Get current system time").category("system"),
        handle_time,
    );

    dispatcher.register_with_handler(
        ToolDefinition::new("search", "Search local content")
            .param(ParamDef::new("query", ParamType::String, "Search query"))
            .param(ParamDef::new("limit", ParamType::Integer, "Max results").optional("10"))
            .category("search"),
        handle_search,
    );

    dispatcher.register_with_handler(
        ToolDefinition::new("help", "Get help on a topic")
            .param(ParamDef::new("topic", ParamType::String, "Help topic").optional("general"))
            .category("utility"),
        handle_help,
    );

    dispatcher.register(
        ToolDefinition::new("file_read", "Read a file from the filesystem")
            .param(ParamDef::new(
                "path",
                ParamType::String,
                "File path to read",
            ))
            .permission(1)
            .category("filesystem"),
    );

    dispatcher.register(
        ToolDefinition::new("file_write", "Write content to a file")
            .param(ParamDef::new(
                "path",
                ParamType::String,
                "File path to write",
            ))
            .param(ParamDef::new(
                "content",
                ParamType::String,
                "Content to write",
            ))
            .permission(2)
            .category("filesystem")
            .side_effects(),
    );

    dispatcher.register(
        ToolDefinition::new("process_list", "List running processes")
            .permission(1)
            .category("system"),
    );

    let count = dispatcher.tool_count();
    *DISPATCHER.lock() = Some(dispatcher);
    crate::serial_println!(
        "    [tool_call] Tool dispatcher ready ({} tools, echo/calc/search/file/process)",
        count
    );
}

/// Dispatch a tool call
pub fn dispatch(name: &str, args: &str) -> String {
    DISPATCHER
        .lock()
        .as_ref()
        .map(|d| d.dispatch(name, args))
        .unwrap_or_else(|| String::from("Error: dispatcher not initialized"))
}
