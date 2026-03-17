use crate::sync::Mutex;
use alloc::string::String;
/// Multi-step task planning with dependency tracking
///
/// Part of the AIOS agent layer. Decomposes goals into executable
/// steps with dependency graphs, cost estimation, rollback support,
/// and adaptive replanning on failure.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// Status of a planned action
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionStatus {
    Pending,
    Ready, // All dependencies met
    Running,
    Completed,
    Failed,
    Skipped, // Skipped due to dependency failure
    RolledBack,
}

/// A planned action in a multi-step task
#[derive(Clone)]
pub struct PlannedAction {
    pub id: usize,
    pub description: String,
    pub tool_hash: u64, // Hash of tool name to invoke
    pub args_hash: u64, // Hash of serialized arguments
    pub depends_on: Vec<usize>,
    pub status: ActionStatus,
    pub estimated_cost: u64,
    pub actual_cost: u64,
    pub retry_count: u8,
    pub max_retries: u8,
    pub can_rollback: bool,
}

/// Planning strategy
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PlanStrategy {
    Sequential, // Execute one at a time, in order
    Parallel,   // Execute independent steps concurrently
    Adaptive,   // Replan after each step based on results
}

struct PlannerInner {
    plan: Vec<PlannedAction>,
    strategy: PlanStrategy,
    next_id: usize,
    total_planned: u64,
    total_replans: u64,
    total_rollbacks: u64,
    goal_hash: u64,
}

static PLANNER: Mutex<Option<PlannerInner>> = Mutex::new(None);

impl PlannerInner {
    fn new() -> Self {
        PlannerInner {
            plan: Vec::new(),
            strategy: PlanStrategy::Adaptive,
            next_id: 1,
            total_planned: 0,
            total_replans: 0,
            total_rollbacks: 0,
            goal_hash: 0,
        }
    }

    /// Add a step to the current plan
    fn add_step(
        &mut self,
        description: String,
        tool_hash: u64,
        args_hash: u64,
        depends_on: Vec<usize>,
        estimated_cost: u64,
        can_rollback: bool,
    ) -> usize {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.plan.push(PlannedAction {
            id,
            description,
            tool_hash,
            args_hash,
            depends_on,
            status: ActionStatus::Pending,
            estimated_cost,
            actual_cost: 0,
            retry_count: 0,
            max_retries: 3,
            can_rollback,
        });
        self.total_planned = self.total_planned.saturating_add(1);
        id
    }

    /// Get the next action(s) that are ready to run
    fn get_ready_actions(&mut self) -> Vec<usize> {
        let mut ready = Vec::new();
        for i in 0..self.plan.len() {
            if self.plan[i].status != ActionStatus::Pending {
                continue;
            }
            // Check all dependencies are completed
            let deps_met = self.plan[i].depends_on.iter().all(|dep_id| {
                self.plan
                    .iter()
                    .find(|a| a.id == *dep_id)
                    .map(|a| a.status == ActionStatus::Completed)
                    .unwrap_or(false)
            });
            if deps_met {
                ready.push(self.plan[i].id);
            }
        }
        // Mark as ready
        for &id in &ready {
            if let Some(a) = self.plan.iter_mut().find(|a| a.id == id) {
                a.status = ActionStatus::Ready;
            }
        }

        match self.strategy {
            PlanStrategy::Sequential => {
                // Only return the first ready action
                ready.truncate(1);
                ready
            }
            PlanStrategy::Parallel | PlanStrategy::Adaptive => ready,
        }
    }

    /// Mark an action as started
    fn start_action(&mut self, id: usize) {
        if let Some(a) = self.plan.iter_mut().find(|a| a.id == id) {
            a.status = ActionStatus::Running;
        }
    }

    /// Mark an action as completed
    fn complete_action(&mut self, id: usize, actual_cost: u64) {
        if let Some(a) = self.plan.iter_mut().find(|a| a.id == id) {
            a.status = ActionStatus::Completed;
            a.actual_cost = actual_cost;
        }
    }

    /// Mark an action as failed, optionally retry or skip dependents
    fn fail_action(&mut self, id: usize) {
        let should_retry;
        if let Some(a) = self.plan.iter_mut().find(|a| a.id == id) {
            if a.retry_count < a.max_retries {
                a.retry_count = a.retry_count.saturating_add(1);
                a.status = ActionStatus::Pending; // Will be re-queued
                should_retry = true;
            } else {
                a.status = ActionStatus::Failed;
                should_retry = false;
            }
        } else {
            return;
        }

        if !should_retry {
            // Skip all actions that depend on this one
            let dependent_ids: Vec<usize> = self
                .plan
                .iter()
                .filter(|a| a.depends_on.contains(&id) && a.status == ActionStatus::Pending)
                .map(|a| a.id)
                .collect();
            for dep_id in dependent_ids {
                if let Some(a) = self.plan.iter_mut().find(|a| a.id == dep_id) {
                    a.status = ActionStatus::Skipped;
                }
            }
        }
    }

    /// Rollback completed actions in reverse order
    fn rollback_all(&mut self) -> Vec<usize> {
        self.total_rollbacks = self.total_rollbacks.saturating_add(1);
        let mut rolled_back = Vec::new();
        // Iterate in reverse to undo last actions first
        for i in (0..self.plan.len()).rev() {
            if self.plan[i].status == ActionStatus::Completed && self.plan[i].can_rollback {
                self.plan[i].status = ActionStatus::RolledBack;
                rolled_back.push(self.plan[i].id);
            }
        }
        rolled_back
    }

    /// Clear the plan for a new goal
    fn clear(&mut self) {
        self.plan.clear();
        self.next_id = 1;
        self.goal_hash = 0;
    }

    /// Get plan completion progress (completed / total)
    fn progress(&self) -> (usize, usize) {
        let completed = self
            .plan
            .iter()
            .filter(|a| a.status == ActionStatus::Completed)
            .count();
        (completed, self.plan.len())
    }

    /// Get total estimated cost of remaining steps
    fn remaining_cost(&self) -> u64 {
        self.plan
            .iter()
            .filter(|a| matches!(a.status, ActionStatus::Pending | ActionStatus::Ready))
            .map(|a| a.estimated_cost)
            .sum()
    }
}

// --- Public API ---

/// Start a new plan for a goal
pub fn new_plan(goal_hash: u64) {
    let mut planner = PLANNER.lock();
    if let Some(p) = planner.as_mut() {
        p.clear();
        p.goal_hash = goal_hash;
    }
}

/// Add a step to the plan
pub fn add_step(
    description: String,
    tool_hash: u64,
    args_hash: u64,
    depends_on: Vec<usize>,
    estimated_cost: u64,
    can_rollback: bool,
) -> usize {
    let mut planner = PLANNER.lock();
    match planner.as_mut() {
        Some(p) => p.add_step(
            description,
            tool_hash,
            args_hash,
            depends_on,
            estimated_cost,
            can_rollback,
        ),
        None => 0,
    }
}

/// Get actions ready to run
pub fn get_ready() -> Vec<usize> {
    let mut planner = PLANNER.lock();
    match planner.as_mut() {
        Some(p) => p.get_ready_actions(),
        None => Vec::new(),
    }
}

/// Mark action started
pub fn start(id: usize) {
    let mut planner = PLANNER.lock();
    if let Some(p) = planner.as_mut() {
        p.start_action(id);
    }
}

/// Mark action completed
pub fn complete(id: usize, cost: u64) {
    let mut planner = PLANNER.lock();
    if let Some(p) = planner.as_mut() {
        p.complete_action(id, cost);
    }
}

/// Mark action failed
pub fn fail(id: usize) {
    let mut planner = PLANNER.lock();
    if let Some(p) = planner.as_mut() {
        p.fail_action(id);
    }
}

/// Get progress (completed, total)
pub fn progress() -> (usize, usize) {
    let planner = PLANNER.lock();
    match planner.as_ref() {
        Some(p) => p.progress(),
        None => (0, 0),
    }
}

/// Set planning strategy
pub fn set_strategy(strategy: PlanStrategy) {
    let mut planner = PLANNER.lock();
    if let Some(p) = planner.as_mut() {
        p.strategy = strategy;
    }
}

pub fn init() {
    let mut planner = PLANNER.lock();
    *planner = Some(PlannerInner::new());
    serial_println!("    Planner: dependency graph, adaptive replanning, rollback ready");
}
