use crate::sync::Mutex;
/// Multi-agent teams for Genesis
///
/// Coordinate multiple specialized agents working together.
/// Inspired by Claude Code's experimental agent teams,
/// built as a first-class Genesis feature.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum AgentRole {
    Orchestrator, // Plans and delegates
    Coder,        // Writes code
    Reviewer,     // Reviews code for bugs/quality
    Researcher,   // Searches codebase, reads docs
    Tester,       // Runs tests, validates
    Architect,    // Designs systems, plans implementations
    Explorer,     // Fast codebase exploration
    Debugger,     // Systematic debugging
    Custom,
}

#[derive(Clone, Copy, PartialEq)]
pub enum AgentState {
    Idle,
    Working,
    WaitingForInput,
    WaitingForPeer, // Blocked on another agent's output
    Complete,
    Failed,
}

#[derive(Clone, Copy)]
struct AgentInstance {
    id: u32,
    role: AgentRole,
    state: AgentState,
    session_id: u32,
    parent_id: u32, // 0 = top-level orchestrator
    task_hash: u64,
    turns_used: u32,
    max_turns: u32,
    tokens_used: u64,
    created_at: u64,
    completed_at: u64,
}

#[derive(Clone, Copy)]
struct TeamTask {
    id: u32,
    assigned_agent: u32,
    task_hash: u64,
    depends_on: [u32; 4], // Up to 4 dependencies
    dep_count: u8,
    priority: u8,
    status: AgentState,
}

struct AgentTeamManager {
    agents: Vec<AgentInstance>,
    tasks: Vec<TeamTask>,
    next_agent_id: u32,
    next_task_id: u32,
    max_concurrent: u8,
    total_agents_spawned: u32,
    total_tasks_completed: u32,
}

static TEAM_MGR: Mutex<Option<AgentTeamManager>> = Mutex::new(None);

impl AgentTeamManager {
    fn new() -> Self {
        AgentTeamManager {
            agents: Vec::new(),
            tasks: Vec::new(),
            next_agent_id: 1,
            next_task_id: 1,
            max_concurrent: 8,
            total_agents_spawned: 0,
            total_tasks_completed: 0,
        }
    }

    fn spawn_agent(
        &mut self,
        role: AgentRole,
        parent_id: u32,
        task_hash: u64,
        timestamp: u64,
    ) -> u32 {
        let id = self.next_agent_id;
        self.next_agent_id = self.next_agent_id.saturating_add(1);
        self.total_agents_spawned = self.total_agents_spawned.saturating_add(1);

        self.agents.push(AgentInstance {
            id,
            role,
            state: AgentState::Idle,
            session_id: 0,
            parent_id,
            task_hash,
            turns_used: 0,
            max_turns: 30,
            tokens_used: 0,
            created_at: timestamp,
            completed_at: 0,
        });
        id
    }

    fn create_task(&mut self, task_hash: u64, depends_on: &[u32], priority: u8) -> u32 {
        let id = self.next_task_id;
        self.next_task_id = self.next_task_id.saturating_add(1);
        let mut deps = [0u32; 4];
        let dep_count = depends_on.len().min(4) as u8;
        for (i, &d) in depends_on.iter().take(4).enumerate() {
            deps[i] = d;
        }
        self.tasks.push(TeamTask {
            id,
            assigned_agent: 0,
            task_hash,
            depends_on: deps,
            dep_count,
            priority,
            status: AgentState::Idle,
        });
        id
    }

    fn assign_task(&mut self, task_id: u32, agent_id: u32) -> bool {
        // First check deps are met (immutable borrow)
        let deps_ok = if let Some(t) = self.tasks.iter().find(|t| t.id == task_id) {
            let mut ok = true;
            for i in 0..t.dep_count as usize {
                let dep_id = t.depends_on[i];
                if let Some(dep) = self.tasks.iter().find(|tt| tt.id == dep_id) {
                    if dep.status != AgentState::Complete {
                        ok = false;
                        break;
                    }
                }
            }
            ok
        } else {
            return false;
        };

        if !deps_ok {
            return false;
        }

        // Now do mutable updates
        if let Some(t) = self.tasks.iter_mut().find(|t| t.id == task_id) {
            t.assigned_agent = agent_id;
            t.status = AgentState::Working;
        }
        if let Some(a) = self.agents.iter_mut().find(|a| a.id == agent_id) {
            a.state = AgentState::Working;
        }
        true
    }

    fn complete_agent(&mut self, agent_id: u32, timestamp: u64) {
        if let Some(a) = self.agents.iter_mut().find(|a| a.id == agent_id) {
            a.state = AgentState::Complete;
            a.completed_at = timestamp;
        }
        // Mark assigned tasks complete
        for t in &mut self.tasks {
            if t.assigned_agent == agent_id {
                t.status = AgentState::Complete;
                self.total_tasks_completed = self.total_tasks_completed.saturating_add(1);
            }
        }
    }

    fn get_ready_tasks(&self) -> Vec<u32> {
        self.tasks
            .iter()
            .filter(|t| {
                if t.status != AgentState::Idle {
                    return false;
                }
                // All deps complete?
                for i in 0..t.dep_count as usize {
                    let dep_id = t.depends_on[i];
                    if let Some(dep) = self.tasks.iter().find(|tt| tt.id == dep_id) {
                        if dep.status != AgentState::Complete {
                            return false;
                        }
                    }
                }
                true
            })
            .map(|t| t.id)
            .collect()
    }

    fn get_active_count(&self) -> u8 {
        self.agents
            .iter()
            .filter(|a| a.state == AgentState::Working)
            .count() as u8
    }

    fn can_spawn_more(&self) -> bool {
        self.get_active_count() < self.max_concurrent
    }

    fn orchestrate(&mut self, timestamp: u64) {
        // Auto-assign ready tasks to idle agents or spawn new ones
        let ready = self.get_ready_tasks();
        for task_id in ready {
            if !self.can_spawn_more() {
                break;
            }
            // Find an idle agent or spawn one
            let idle_agent = self
                .agents
                .iter()
                .find(|a| a.state == AgentState::Idle)
                .map(|a| a.id);
            if let Some(agent_id) = idle_agent {
                self.assign_task(task_id, agent_id);
            } else {
                let agent_id = self.spawn_agent(AgentRole::Coder, 0, 0, timestamp);
                self.assign_task(task_id, agent_id);
            }
        }
    }
}

pub fn init() {
    let mut tm = TEAM_MGR.lock();
    *tm = Some(AgentTeamManager::new());
    serial_println!("    Agent teams: multi-agent coordination, task deps, orchestration ready");
}
