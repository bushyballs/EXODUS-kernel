use crate::sync::Mutex;
/// System prompt framework — the AI's personality and capabilities
///
/// Defines HOW the Hoags AI behaves. Fully user-controlled.
/// No external censorship, no corporate alignment overrides.
/// The owner of the hardware owns the AI's behavior.
///
/// Features:
///   - Composable system prompts
///   - Role definitions (assistant, coder, analyst, creative, etc.)
///   - Tool use instructions
///   - Context injection (project files, CLAUDE.md equivalent)
///   - Persona customization
///   - Multi-turn conversation format
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum AiRole {
    Assistant,   // General helpful assistant
    Coder,       // Software engineer agent
    Analyst,     // Data/business analyst
    Creative,    // Writing, art, brainstorming
    Researcher,  // Deep research and citation
    SystemAdmin, // DevOps, infrastructure
    Tutor,       // Teaching and explanation
    Custom,      // User-defined role
}

#[derive(Clone, Copy, PartialEq)]
pub enum ConversationRole {
    System,
    User,
    Assistant,
    Tool,
    Thinking, // Internal reasoning (chain of thought)
}

#[derive(Clone, Copy)]
pub struct PromptSection {
    pub section_type: SectionType,
    pub content_hash: u64,
    pub priority: u8,      // Higher = placed earlier in prompt
    pub token_budget: u32, // Max tokens for this section
    pub enabled: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub enum SectionType {
    Identity,     // Who the AI is
    Capabilities, // What it can do
    Rules,        // Behavioral rules (user-defined)
    Context,      // Project/codebase context
    Tools,        // Available tool descriptions
    Memory,       // Recalled memories
    Examples,     // Few-shot examples
    Persona,      // Custom personality traits
    Format,       // Output format instructions
}

struct SystemPromptEngine {
    sections: Vec<PromptSection>,
    role: AiRole,
    // Identity
    name_hash: u64,
    version_hash: u64,
    // Conversation format tokens
    system_prefix: u64, // e.g., hash of "<|system|>"
    user_prefix: u64,
    assistant_prefix: u64,
    tool_prefix: u64,
    thinking_prefix: u64,
    turn_separator: u64,
    // Settings
    max_system_tokens: u32,
    chain_of_thought: bool, // Enable thinking before responding
    tool_use_enabled: bool,
    streaming_enabled: bool,
    // Project context (like CLAUDE.md)
    project_context_hash: u64,
    project_rules: Vec<u64>, // Hashes of user-defined rules
    // Stats
    total_prompts_built: u64,
}

static PROMPT_ENGINE: Mutex<Option<SystemPromptEngine>> = Mutex::new(None);

impl SystemPromptEngine {
    fn new() -> Self {
        SystemPromptEngine {
            sections: Vec::new(),
            role: AiRole::Assistant,
            name_hash: 0x484F414753_4149, // "HOAGS_AI"
            version_hash: 0x0100,
            system_prefix: 0x3C7C7379737C3E,
            user_prefix: 0x3C7C757365727C3E,
            assistant_prefix: 0x3C7C617373747C3E,
            tool_prefix: 0x3C7C746F6F6C7C3E,
            thinking_prefix: 0x3C7C74686E6B7C3E,
            turn_separator: 0x0A0A,
            max_system_tokens: 8192,
            chain_of_thought: true,
            tool_use_enabled: true,
            streaming_enabled: true,
            project_context_hash: 0,
            project_rules: Vec::new(),
            total_prompts_built: 0,
        }
    }

    fn set_role(&mut self, role: AiRole) {
        self.role = role;
    }

    fn add_section(&mut self, section_type: SectionType, content: u64, priority: u8, budget: u32) {
        self.sections.push(PromptSection {
            section_type,
            content_hash: content,
            priority,
            token_budget: budget,
            enabled: true,
        });
        // Sort by priority descending
        self.sections.sort_by(|a, b| b.priority.cmp(&a.priority));
    }

    fn add_rule(&mut self, rule_hash: u64) {
        self.project_rules.push(rule_hash);
    }

    fn remove_rule(&mut self, rule_hash: u64) {
        self.project_rules.retain(|&r| r != rule_hash);
    }

    fn set_project_context(&mut self, context_hash: u64) {
        self.project_context_hash = context_hash;
    }

    fn enable_chain_of_thought(&mut self, enabled: bool) {
        self.chain_of_thought = enabled;
    }

    fn enable_tools(&mut self, enabled: bool) {
        self.tool_use_enabled = enabled;
    }

    /// Build the full system prompt token sequence
    /// Returns section hashes in order they should appear
    fn build_prompt(&mut self) -> Vec<u64> {
        self.total_prompts_built = self.total_prompts_built.saturating_add(1);
        let mut prompt_parts = Vec::new();

        // 1. Identity section
        prompt_parts.push(self.name_hash);
        prompt_parts.push(self.version_hash);

        // 2. Role-specific instructions
        let role_hash = match self.role {
            AiRole::Assistant => 0xA551_5741_4E54,
            AiRole::Coder => 0xC0DE_A6E4_7000,
            AiRole::Analyst => 0xA4A1_7574_0000,
            AiRole::Creative => 0xC4EA_7100_0000,
            AiRole::Researcher => 0x4E5E_A4C4_0000,
            AiRole::SystemAdmin => 0x5754_AD01_4000,
            AiRole::Tutor => 0x7070_40E0_0000,
            AiRole::Custom => 0xC057_0000_0000,
        };
        prompt_parts.push(role_hash);

        // 3. Enabled sections by priority
        let mut remaining_tokens = self.max_system_tokens;
        for section in &self.sections {
            if !section.enabled {
                continue;
            }
            if section.token_budget > remaining_tokens {
                continue;
            }
            prompt_parts.push(section.content_hash);
            remaining_tokens -= section.token_budget;
        }

        // 4. Project context
        if self.project_context_hash != 0 {
            prompt_parts.push(self.project_context_hash);
        }

        // 5. User rules
        for &rule in &self.project_rules {
            prompt_parts.push(rule);
        }

        // 6. Tool instructions
        if self.tool_use_enabled {
            prompt_parts.push(0x544F_4F4C_5F55_5345); // TOOL_USE marker
        }

        // 7. Chain of thought instruction
        if self.chain_of_thought {
            prompt_parts.push(0x5448_494E_4B5F_4649); // THINK_FI marker
        }

        prompt_parts
    }

    fn setup_default_coder(&mut self) {
        self.role = AiRole::Coder;
        self.chain_of_thought = true;
        self.tool_use_enabled = true;
        self.add_section(SectionType::Identity, 0x01, 100, 200);
        self.add_section(SectionType::Capabilities, 0x02, 90, 500);
        self.add_section(SectionType::Tools, 0x03, 80, 2000);
        self.add_section(SectionType::Context, 0x04, 70, 4000);
        self.add_section(SectionType::Format, 0x05, 60, 300);
    }

    fn setup_default_assistant(&mut self) {
        self.role = AiRole::Assistant;
        self.chain_of_thought = true;
        self.tool_use_enabled = false;
        self.add_section(SectionType::Identity, 0x01, 100, 200);
        self.add_section(SectionType::Capabilities, 0x02, 90, 300);
        self.add_section(SectionType::Persona, 0x03, 80, 200);
        self.add_section(SectionType::Memory, 0x04, 70, 1000);
    }
}

pub fn init() {
    let mut pe = PROMPT_ENGINE.lock();
    let mut engine = SystemPromptEngine::new();
    engine.setup_default_coder();
    *pe = Some(engine);
    serial_println!(
        "    System prompt: roles, chain-of-thought, tools, project context, user rules ready"
    );
}
