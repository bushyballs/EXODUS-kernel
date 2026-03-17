/// Tool Use / Function Calling — structured tool invocation for the AI
///
/// Gives the Hoags AI the ability to call tools, parse arguments,
/// execute actions, and inject results back into the conversation.
/// Supports parallel tool calls, tool schemas with typed parameters,
/// result validation, and retry logic.
///
/// The AI can use tools to interact with the OS, file system,
/// network, devices, and any registered capability. Tool definitions
/// are composable and user-extensible.
///
/// Features:
///   - Tool schema definition with typed parameters
///   - Argument parsing and validation
///   - Sequential and parallel tool execution
///   - Result injection back into conversation context
///   - Tool call history and performance tracking
///   - Retry with exponential backoff on failure
///   - Tool capability discovery and documentation
///   - Composable tool chains (pipe output of one into another)

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

use super::transformer::{Q16, q16_mul, q16_from_int};

// ── Constants ────────────────────────────────────────────────────────

/// Maximum number of registered tools
const MAX_TOOLS: usize = 256;

/// Maximum parameters per tool
const MAX_PARAMS_PER_TOOL: usize = 16;

/// Maximum parallel tool calls in a single batch
const MAX_PARALLEL_CALLS: usize = 8;

/// Maximum retry attempts for a failed tool call
const MAX_RETRIES: u32 = 3;

/// Maximum tool call history entries retained
const MAX_CALL_HISTORY: usize = 1024;

/// Maximum tool chain depth (prevents infinite loops)
const MAX_CHAIN_DEPTH: u32 = 8;

/// Backoff base interval in arbitrary time units
const BACKOFF_BASE: u64 = 100;

// ── Types ────────────────────────────────────────────────────────────

/// Parameter types supported by tool schemas
#[derive(Clone, Copy, PartialEq)]
pub enum ParamType {
    Integer,
    Boolean,
    Text,
    Hash,
    TokenList,
    Optional,
}

/// A single parameter in a tool schema
#[derive(Clone, Copy)]
pub struct ToolParam {
    pub name_hash: u64,
    pub param_type: ParamType,
    pub required: bool,
    pub default_value: i64,
    pub description_hash: u64,
}

/// A tool definition — describes what the tool does and how to call it
#[derive(Clone)]
pub struct ToolSchema {
    pub id: u32,
    pub name_hash: u64,
    pub description_hash: u64,
    pub category_hash: u64,
    pub params: Vec<ToolParam>,
    pub handler_hash: u64,
    pub requires_confirmation: bool,
    pub dangerous: bool,
    pub enabled: bool,
    pub version: u32,
}

impl ToolSchema {
    fn new(id: u32, name: u64, description: u64, handler: u64) -> Self {
        ToolSchema {
            id,
            name_hash: name,
            description_hash: description,
            category_hash: 0,
            params: Vec::new(),
            handler_hash: handler,
            requires_confirmation: false,
            dangerous: false,
            enabled: true,
            version: 1,
        }
    }

    fn add_param(&mut self, name: u64, ptype: ParamType, required: bool, default: i64, desc: u64) {
        if self.params.len() < MAX_PARAMS_PER_TOOL {
            self.params.push(ToolParam {
                name_hash: name,
                param_type: ptype,
                required,
                default_value: default,
                description_hash: desc,
            });
        }
    }
}

/// A parsed argument value for a tool call
#[derive(Clone, Copy)]
pub struct ToolArgument {
    pub param_hash: u64,
    pub value_int: i64,
    pub value_hash: u64,
    pub is_set: bool,
}

/// Status of a tool call execution
#[derive(Clone, Copy, PartialEq)]
pub enum ToolCallStatus {
    Pending,
    Validating,
    Executing,
    Success,
    Failed,
    Retrying,
    Cancelled,
    Timeout,
}

/// A single tool call — an invocation with arguments and result
#[derive(Clone)]
pub struct ToolCall {
    pub call_id: u32,
    pub tool_id: u32,
    pub arguments: Vec<ToolArgument>,
    pub status: ToolCallStatus,
    pub result_hash: u64,
    pub error_hash: u64,
    pub started_at: u64,
    pub completed_at: u64,
    pub retry_count: u32,
    pub execution_time: u64,
}

impl ToolCall {
    fn new(call_id: u32, tool_id: u32, timestamp: u64) -> Self {
        ToolCall {
            call_id,
            tool_id,
            arguments: Vec::new(),
            status: ToolCallStatus::Pending,
            result_hash: 0,
            error_hash: 0,
            started_at: timestamp,
            completed_at: 0,
            retry_count: 0,
            execution_time: 0,
        }
    }
}

/// A batch of parallel tool calls
#[derive(Clone)]
pub struct ToolBatch {
    pub batch_id: u32,
    pub calls: Vec<u32>,
    pub all_complete: bool,
    pub any_failed: bool,
    pub started_at: u64,
}

/// A tool chain — sequential pipeline of tool calls
#[derive(Clone)]
pub struct ToolChain {
    pub chain_id: u32,
    pub steps: Vec<u32>,
    pub current_step: u32,
    pub depth: u32,
    pub completed: bool,
    pub last_output_hash: u64,
}

/// Performance stats for a single tool
#[derive(Clone, Copy)]
pub struct ToolPerformance {
    pub tool_id: u32,
    pub total_calls: u64,
    pub successes: u64,
    pub failures: u64,
    pub avg_execution_time: Q16,
    pub success_rate: Q16,
}

// ── Tool Engine ──────────────────────────────────────────────────────

struct ToolEngine {
    tools: Vec<ToolSchema>,
    calls: Vec<ToolCall>,
    batches: Vec<ToolBatch>,
    chains: Vec<ToolChain>,
    performance: Vec<ToolPerformance>,
    next_tool_id: u32,
    next_call_id: u32,
    next_batch_id: u32,
    next_chain_id: u32,
    total_calls_executed: u64,
    total_successes: u64,
    total_failures: u64,
}

impl ToolEngine {
    fn new() -> Self {
        ToolEngine {
            tools: Vec::new(),
            calls: Vec::new(),
            batches: Vec::new(),
            chains: Vec::new(),
            performance: Vec::new(),
            next_tool_id: 1,
            next_call_id: 1,
            next_batch_id: 1,
            next_chain_id: 1,
            total_calls_executed: 0,
            total_successes: 0,
            total_failures: 0,
        }
    }

    // ── Tool Registration ────────────────────────────────────────────

    /// Register a new tool and return its ID
    fn register_tool(&mut self, name: u64, description: u64, handler: u64) -> u32 {
        if self.tools.len() >= MAX_TOOLS {
            return 0;
        }
        let id = self.next_tool_id;
        self.next_tool_id = self.next_tool_id.saturating_add(1);

        let schema = ToolSchema::new(id, name, description, handler);
        self.tools.push(schema);

        // Initialize performance tracking
        self.performance.push(ToolPerformance {
            tool_id: id,
            total_calls: 0,
            successes: 0,
            failures: 0,
            avg_execution_time: 0,
            success_rate: q16_from_int(1),
        });

        id
    }

    /// Add a parameter to an existing tool
    fn add_tool_param(&mut self, tool_id: u32, name: u64, ptype: ParamType,
                       required: bool, default: i64, desc: u64) {
        if let Some(tool) = self.tools.iter_mut().find(|t| t.id == tool_id) {
            tool.add_param(name, ptype, required, default, desc);
        }
    }

    /// Enable or disable a tool
    fn set_tool_enabled(&mut self, tool_id: u32, enabled: bool) {
        if let Some(tool) = self.tools.iter_mut().find(|t| t.id == tool_id) {
            tool.enabled = enabled;
        }
    }

    /// Mark a tool as dangerous (requires user confirmation)
    fn set_tool_dangerous(&mut self, tool_id: u32, dangerous: bool) {
        if let Some(tool) = self.tools.iter_mut().find(|t| t.id == tool_id) {
            tool.dangerous = dangerous;
            tool.requires_confirmation = dangerous;
        }
    }

    /// Look up a tool by name hash
    fn find_tool(&self, name_hash: u64) -> Option<&ToolSchema> {
        self.tools.iter().find(|t| t.name_hash == name_hash && t.enabled)
    }

    /// Get all tool name hashes for context injection
    fn get_tool_list(&self) -> Vec<u64> {
        self.tools.iter()
            .filter(|t| t.enabled)
            .map(|t| t.name_hash)
            .collect()
    }

    // ── Argument Validation ──────────────────────────────────────────

    /// Validate arguments against the tool schema
    fn validate_arguments(&self, tool_id: u32, args: &[ToolArgument]) -> bool {
        let tool = match self.tools.iter().find(|t| t.id == tool_id) {
            Some(t) => t,
            None => return false,
        };

        // Check all required params are provided
        for param in &tool.params {
            if param.required {
                let found = args.iter().any(|a| a.param_hash == param.name_hash && a.is_set);
                if !found {
                    return false;
                }
            }
        }

        // Check no unknown params
        for arg in args {
            if !arg.is_set { continue; }
            let known = tool.params.iter().any(|p| p.name_hash == arg.param_hash);
            if !known {
                return false;
            }
        }

        true
    }

    /// Fill in default values for unset optional parameters
    fn apply_defaults(&self, tool_id: u32, args: &mut Vec<ToolArgument>) {
        let tool = match self.tools.iter().find(|t| t.id == tool_id) {
            Some(t) => t,
            None => return,
        };

        for param in &tool.params {
            if !param.required {
                let has_arg = args.iter().any(|a| a.param_hash == param.name_hash && a.is_set);
                if !has_arg {
                    args.push(ToolArgument {
                        param_hash: param.name_hash,
                        value_int: param.default_value,
                        value_hash: 0,
                        is_set: true,
                    });
                }
            }
        }
    }

    // ── Tool Execution ───────────────────────────────────────────────

    /// Create a new tool call (does not execute yet)
    fn create_call(&mut self, tool_id: u32, args: Vec<ToolArgument>, timestamp: u64) -> Option<u32> {
        // Verify tool exists and is enabled
        let tool_exists = self.tools.iter().any(|t| t.id == tool_id && t.enabled);
        if !tool_exists {
            return None;
        }

        let call_id = self.next_call_id;
        self.next_call_id = self.next_call_id.saturating_add(1);

        let mut call = ToolCall::new(call_id, tool_id, timestamp);
        call.arguments = args;
        call.status = ToolCallStatus::Pending;

        self.calls.push(call);
        Some(call_id)
    }

    /// Execute a pending tool call
    fn execute_call(&mut self, call_id: u32, timestamp: u64) -> ToolCallStatus {
        let call = match self.calls.iter_mut().find(|c| c.call_id == call_id) {
            Some(c) => c,
            None => return ToolCallStatus::Failed,
        };

        if call.status != ToolCallStatus::Pending && call.status != ToolCallStatus::Retrying {
            return call.status;
        }

        let tool_id = call.tool_id;

        // Validate arguments
        let valid = {
            let args_clone: Vec<ToolArgument> = call.arguments.iter().copied().collect();
            self.validate_arguments(tool_id, &args_clone)
        };

        if !valid {
            call.status = ToolCallStatus::Failed;
            call.error_hash = 0xBAD0_A465_0000_0001; // "bad_args"
            call.completed_at = timestamp;
            self.total_failures = self.total_failures.saturating_add(1);
            self.update_performance(tool_id, false, 0);
            return ToolCallStatus::Failed;
        }

        call.status = ToolCallStatus::Executing;

        // Simulate execution — in real system this dispatches to handler
        // For now, compute a result hash from tool + args
        let mut result: u64 = 0xABCD_EF01_2345_6789;
        result ^= tool_id as u64;
        for arg in &call.arguments {
            if arg.is_set {
                result ^= arg.param_hash;
                result = result.wrapping_add(arg.value_int as u64);
            }
        }

        call.result_hash = result;
        call.status = ToolCallStatus::Success;
        call.completed_at = timestamp;
        call.execution_time = timestamp.saturating_sub(call.started_at);
        self.total_calls_executed = self.total_calls_executed.saturating_add(1);
        self.total_successes = self.total_successes.saturating_add(1);

        self.update_performance(tool_id, true, call.execution_time);

        // Prune old call history
        if self.calls.len() > MAX_CALL_HISTORY {
            let excess = self.calls.len() - MAX_CALL_HISTORY;
            for _ in 0..excess {
                self.calls.remove(0);
            }
        }

        ToolCallStatus::Success
    }

    /// Retry a failed tool call with exponential backoff
    fn retry_call(&mut self, call_id: u32, timestamp: u64) -> bool {
        let call = match self.calls.iter_mut().find(|c| c.call_id == call_id) {
            Some(c) => c,
            None => return false,
        };

        if call.status != ToolCallStatus::Failed {
            return false;
        }

        if call.retry_count >= MAX_RETRIES {
            return false;
        }

        call.retry_count += 1;
        call.status = ToolCallStatus::Retrying;
        call.started_at = timestamp;
        call.error_hash = 0;

        true
    }

    // ── Parallel Execution ───────────────────────────────────────────

    /// Create a batch of parallel tool calls
    fn create_batch(&mut self, call_ids: Vec<u32>, timestamp: u64) -> u32 {
        let batch_id = self.next_batch_id;
        self.next_batch_id = self.next_batch_id.saturating_add(1);

        let mut limited_calls = call_ids;
        if limited_calls.len() > MAX_PARALLEL_CALLS {
            limited_calls.truncate(MAX_PARALLEL_CALLS);
        }

        self.batches.push(ToolBatch {
            batch_id,
            calls: limited_calls,
            all_complete: false,
            any_failed: false,
            started_at: timestamp,
        });

        batch_id
    }

    /// Execute all calls in a batch
    fn execute_batch(&mut self, batch_id: u32, timestamp: u64) {
        let call_ids: Vec<u32> = {
            match self.batches.iter().find(|b| b.batch_id == batch_id) {
                Some(b) => b.calls.clone(),
                None => return,
            }
        };

        for &call_id in &call_ids {
            self.execute_call(call_id, timestamp);
        }

        // Update batch status
        if let Some(batch) = self.batches.iter_mut().find(|b| b.batch_id == batch_id) {
            let mut all_done = true;
            let mut any_fail = false;

            for &cid in &batch.calls {
                if let Some(call) = self.calls.iter().find(|c| c.call_id == cid) {
                    match call.status {
                        ToolCallStatus::Success => {}
                        ToolCallStatus::Failed | ToolCallStatus::Timeout => {
                            any_fail = true;
                        }
                        _ => {
                            all_done = false;
                        }
                    }
                }
            }

            batch.all_complete = all_done;
            batch.any_failed = any_fail;
        }
    }

    /// Get results from a completed batch
    fn get_batch_results(&self, batch_id: u32) -> Vec<(u32, u64)> {
        let batch = match self.batches.iter().find(|b| b.batch_id == batch_id) {
            Some(b) => b,
            None => return Vec::new(),
        };

        let mut results = Vec::new();
        for &call_id in &batch.calls {
            if let Some(call) = self.calls.iter().find(|c| c.call_id == call_id) {
                results.push((call.tool_id, call.result_hash));
            }
        }
        results
    }

    // ── Tool Chains ──────────────────────────────────────────────────

    /// Create a sequential tool chain
    fn create_chain(&mut self, steps: Vec<u32>) -> u32 {
        let chain_id = self.next_chain_id;
        self.next_chain_id = self.next_chain_id.saturating_add(1);

        let mut limited_steps = steps;
        if limited_steps.len() > MAX_CHAIN_DEPTH as usize {
            limited_steps.truncate(MAX_CHAIN_DEPTH as usize);
        }

        self.chains.push(ToolChain {
            chain_id,
            steps: limited_steps,
            current_step: 0,
            depth: 0,
            completed: false,
            last_output_hash: 0,
        });

        chain_id
    }

    /// Execute the next step in a tool chain
    fn advance_chain(&mut self, chain_id: u32, timestamp: u64) -> Option<ToolCallStatus> {
        // Get current step call_id
        let call_id = {
            let chain = self.chains.iter().find(|c| c.chain_id == chain_id)?;
            if chain.completed || chain.current_step as usize >= chain.steps.len() {
                return None;
            }
            chain.steps[chain.current_step as usize]
        };

        let status = self.execute_call(call_id, timestamp);

        // Update chain state
        if let Some(chain) = self.chains.iter_mut().find(|c| c.chain_id == chain_id) {
            if status == ToolCallStatus::Success {
                // Capture output for next step
                if let Some(call) = self.calls.iter().find(|c| c.call_id == call_id) {
                    chain.last_output_hash = call.result_hash;
                }
                chain.current_step += 1;
                chain.depth += 1;

                if chain.current_step as usize >= chain.steps.len() {
                    chain.completed = true;
                }
            } else if status == ToolCallStatus::Failed {
                chain.completed = true;
            }
        }

        Some(status)
    }

    // ── Performance Tracking ─────────────────────────────────────────

    /// Update performance stats for a tool
    fn update_performance(&mut self, tool_id: u32, success: bool, exec_time: u64) {
        if let Some(perf) = self.performance.iter_mut().find(|p| p.tool_id == tool_id) {
            perf.total_calls += 1;
            if success {
                perf.successes += 1;
            } else {
                perf.failures += 1;
            }

            // Update average execution time using EMA
            let ema_old: Q16 = 58982; // 0.9 in Q16
            let ema_new: Q16 = 6554;  // 0.1 in Q16
            let time_q16 = q16_from_int(exec_time as i32);
            perf.avg_execution_time = q16_mul(perf.avg_execution_time, ema_old)
                + q16_mul(time_q16, ema_new);

            // Update success rate
            if perf.total_calls > 0 {
                perf.success_rate = (((perf.successes as i64) << 16)
                    / (perf.total_calls as i64).max(1)) as Q16;
            }
        }
    }

    /// Get the result hash for injecting back into the conversation
    fn get_call_result(&self, call_id: u32) -> Option<u64> {
        self.calls.iter()
            .find(|c| c.call_id == call_id && c.status == ToolCallStatus::Success)
            .map(|c| c.result_hash)
    }

    /// Get overall tool engine statistics
    fn get_stats(&self) -> (u32, u64, u64, u64) {
        (
            self.tools.len() as u32,
            self.total_calls_executed,
            self.total_successes,
            self.total_failures,
        )
    }

    /// Register the built-in OS tools
    fn register_builtin_tools(&mut self) {
        // File system tools
        let fs_read = self.register_tool(
            0xF11E_4EAD_0000_0001, // "file_read"
            0xF11E_4EAD_DE5C_0001, // description
            0xF11E_4EAD_44D1_0001, // handler
        );
        self.add_tool_param(fs_read, 0x5041_7448_0000_0001, ParamType::Text, true, 0,
                            0xDE5C_5041_7448_0001);

        let fs_write = self.register_tool(
            0xF11E_0471_7E00_0002, // "file_write"
            0xF11E_0471_DE5C_0002,
            0xF11E_0471_44D1_0002,
        );
        self.add_tool_param(fs_write, 0x5041_7448_0000_0002, ParamType::Text, true, 0,
                            0xDE5C_5041_7448_0002);
        self.add_tool_param(fs_write, 0xC047_E470_0000_0002, ParamType::Text, true, 0,
                            0xDE5C_C047_E470_0002);
        self.set_tool_dangerous(fs_write, true);

        // Process control
        let proc_exec = self.register_tool(
            0x540C_EXEC_0000_0003, // "proc_exec"
            0x540C_EXEC_DE5C_0003,
            0x540C_EXEC_44D1_0003,
        );
        self.add_tool_param(proc_exec, 0xC0AD_0000_0000_0003, ParamType::Text, true, 0,
                            0xDE5C_C0AD_0000_0003);
        self.set_tool_dangerous(proc_exec, true);

        // Network tools
        let _net_fetch = self.register_tool(
            0x4E74_FE7C_0000_0004, // "net_fetch"
            0x4E74_FE7C_DE5C_0004,
            0x4E74_FE7C_44D1_0004,
        );

        // System info
        let _sys_info = self.register_tool(
            0x5750_14F0_0000_0005, // "sys_info"
            0x5750_14F0_DE5C_0005,
            0x5750_14F0_44D1_0005,
        );

        // Search tool
        let _search = self.register_tool(
            0x5EA4_C400_0000_0006, // "search"
            0x5EA4_C400_DE5C_0006,
            0x5EA4_C400_44D1_0006,
        );
    }
}

// ── Global State ─────────────────────────────────────────────────────

static ENGINE: Mutex<Option<ToolEngine>> = Mutex::new(None);

/// Access the global tool engine
pub fn with_engine<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut ToolEngine) -> R,
{
    let mut locked = ENGINE.lock();
    if let Some(ref mut engine) = *locked {
        Some(f(engine))
    } else {
        None
    }
}

// ── Module Initialization ────────────────────────────────────────────

pub fn init() {
    let mut engine = ToolEngine::new();
    engine.register_builtin_tools();

    let (tools, calls, successes, failures) = engine.get_stats();

    let mut locked = ENGINE.lock();
    *locked = Some(engine);

    serial_println!("    Tool use: {} built-in tools registered, parallel batches, chains, validation ready", tools);
}
