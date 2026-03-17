pub mod agent_teams;
pub mod agentic_loop;
pub mod browser_agent;
pub mod code_agent;
pub mod code_intel;
pub mod evaluator;
pub mod file_agent;
pub mod memory;
pub mod permission;
pub mod planner;
pub mod safety;
pub mod sandbox;
pub mod sessions;
pub mod skills;
pub mod streaming;
pub mod system_agent;
/// Agentic AI framework for Genesis
///
/// Inspired by Claude Code / Codex architectures but built
/// from scratch with Genesis-native integration.
///
/// - Tool-use agentic loop (observe -> think -> act)
/// - Multi-agent teams with coordination
/// - Skills/plugins system
/// - Session management with rewind
/// - Real-time streaming
/// - Browser/UI automation
/// - Codebase awareness (AST, embeddings, RAG)
/// - Sandboxed execution with permissions
pub mod tool_engine;

use crate::{serial_print, serial_println};

pub fn init() {
    tool_engine::init();
    agentic_loop::init();
    agent_teams::init();
    skills::init();
    sessions::init();
    code_intel::init();
    sandbox::init();
    streaming::init();
    planner::init();
    memory::init();
    browser_agent::init();
    file_agent::init();
    code_agent::init();
    system_agent::init();
    safety::init();
    evaluator::init();
    permission::init();
    serial_println!("  Agent framework initialized (tools, loop, teams, skills, sessions, code intel, sandbox, streaming, planner, memory, browser_agent, file_agent, code_agent, system_agent, safety, evaluator, permission)");
}
