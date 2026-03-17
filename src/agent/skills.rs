use crate::sync::Mutex;
/// Skills/plugins system for Genesis agent
///
/// Loadable skill definitions that extend agent capabilities.
/// Inspired by Claude Code's SKILL.md / slash commands,
/// but integrated as Genesis-native plugin architecture.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum SkillTrigger {
    SlashCommand, // /commit, /review, /test
    AutoDetect,   // Triggered by context (e.g., seeing test failures)
    Explicit,     // Must be invoked by name
    Hook,         // Triggered by tool events (pre/post hooks)
}

#[derive(Clone, Copy, PartialEq)]
pub enum HookEvent {
    PreToolCall,
    PostToolCall,
    PreCommit,
    PostCommit,
    PreFileWrite,
    PostFileWrite,
    OnError,
    OnSessionStart,
    OnSessionEnd,
}

#[derive(Clone, Copy)]
struct Skill {
    id: u32,
    name_hash: u64,
    description_hash: u64,
    trigger: SkillTrigger,
    prompt_hash: u64, // The skill's system prompt
    enabled: bool,
    invocation_count: u32,
    // Permissions this skill needs
    needs_file_read: bool,
    needs_file_write: bool,
    needs_execute: bool,
    needs_network: bool,
}

#[derive(Clone, Copy)]
struct HookRule {
    id: u32,
    event: HookEvent,
    skill_id: u32,
    pattern_hash: u64, // Match pattern (e.g., file glob for PreFileWrite)
    enabled: bool,
    fire_count: u32,
}

struct SkillManager {
    skills: Vec<Skill>,
    hooks: Vec<HookRule>,
    next_skill_id: u32,
    next_hook_id: u32,
    total_invocations: u32,
    hot_reload_enabled: bool,
}

static SKILL_MGR: Mutex<Option<SkillManager>> = Mutex::new(None);

impl SkillManager {
    fn new() -> Self {
        SkillManager {
            skills: Vec::new(),
            hooks: Vec::new(),
            next_skill_id: 1,
            next_hook_id: 1,
            total_invocations: 0,
            hot_reload_enabled: true,
        }
    }

    fn register_skill(&mut self, name_hash: u64, trigger: SkillTrigger, prompt_hash: u64) -> u32 {
        let id = self.next_skill_id;
        self.next_skill_id = self.next_skill_id.saturating_add(1);
        self.skills.push(Skill {
            id,
            name_hash,
            description_hash: 0,
            trigger,
            prompt_hash,
            enabled: true,
            invocation_count: 0,
            needs_file_read: true,
            needs_file_write: false,
            needs_execute: false,
            needs_network: false,
        });
        id
    }

    fn register_builtin_skills(&mut self) {
        // Core coding skills
        self.register_skill(0x636F6D6D69740000, SkillTrigger::SlashCommand, 0x01); // /commit
        self.register_skill(0x7265766965770000, SkillTrigger::SlashCommand, 0x02); // /review
        self.register_skill(0x7465737400000000, SkillTrigger::SlashCommand, 0x03); // /test
        self.register_skill(0x6275696C64000000, SkillTrigger::SlashCommand, 0x04); // /build
        self.register_skill(0x6465627567000000, SkillTrigger::SlashCommand, 0x05); // /debug
        self.register_skill(0x726566616374006F, SkillTrigger::SlashCommand, 0x06); // /refactor
        self.register_skill(0x646F637300000000, SkillTrigger::SlashCommand, 0x07); // /docs
        self.register_skill(0x7365617263680000, SkillTrigger::SlashCommand, 0x08); // /search
        self.register_skill(0x706C616E00000000, SkillTrigger::SlashCommand, 0x09); // /plan
        self.register_skill(0x6578706C61696E00, SkillTrigger::SlashCommand, 0x0A); // /explain
                                                                                   // Auto-detect skills
        self.register_skill(0xA070_6572726F72, SkillTrigger::AutoDetect, 0x20); // error-fixer
        self.register_skill(0xA070_74657374, SkillTrigger::AutoDetect, 0x21); // test-runner
        self.register_skill(0xA070_6C696E74, SkillTrigger::AutoDetect, 0x22); // linter
    }

    fn register_hook(&mut self, event: HookEvent, skill_id: u32, pattern_hash: u64) -> u32 {
        let id = self.next_hook_id;
        self.next_hook_id = self.next_hook_id.saturating_add(1);
        self.hooks.push(HookRule {
            id,
            event,
            skill_id,
            pattern_hash,
            enabled: true,
            fire_count: 0,
        });
        id
    }

    fn invoke_skill(&mut self, skill_id: u32) -> bool {
        if let Some(skill) = self
            .skills
            .iter_mut()
            .find(|s| s.id == skill_id && s.enabled)
        {
            skill.invocation_count = skill.invocation_count.saturating_add(1);
            self.total_invocations = self.total_invocations.saturating_add(1);
            true
        } else {
            false
        }
    }

    fn find_by_command(&self, name_hash: u64) -> Option<u32> {
        self.skills
            .iter()
            .find(|s| {
                s.name_hash == name_hash && s.enabled && s.trigger == SkillTrigger::SlashCommand
            })
            .map(|s| s.id)
    }

    fn fire_hooks(&mut self, event: HookEvent, context_hash: u64) -> Vec<u32> {
        let mut fired = Vec::new();
        for hook in &mut self.hooks {
            if hook.event == event && hook.enabled {
                // Pattern match check (0 = match all)
                if hook.pattern_hash == 0 || hook.pattern_hash == context_hash {
                    hook.fire_count = hook.fire_count.saturating_add(1);
                    fired.push(hook.skill_id);
                }
            }
        }
        fired
    }

    fn enable_skill(&mut self, skill_id: u32) {
        if let Some(s) = self.skills.iter_mut().find(|s| s.id == skill_id) {
            s.enabled = true;
        }
    }

    fn disable_skill(&mut self, skill_id: u32) {
        if let Some(s) = self.skills.iter_mut().find(|s| s.id == skill_id) {
            s.enabled = false;
        }
    }
}

pub fn init() {
    let mut sm = SKILL_MGR.lock();
    let mut mgr = SkillManager::new();
    mgr.register_builtin_skills();
    *sm = Some(mgr);
    serial_println!(
        "    Skills: 13 builtin (/commit, /review, /test, ...), hooks, hot reload ready"
    );
}
