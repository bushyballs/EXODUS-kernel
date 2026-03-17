use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
/// AI task planning and decomposition
///
/// Part of the AIOS AI layer. Implements a forward-chaining planner
/// that searches for action sequences to achieve goals from an initial state.
/// Actions have preconditions and effects on a symbolic state space.
/// Supports plan validation, step dependencies, and replanning.
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// A state is a set of propositions (string keys) that are true
type State = BTreeMap<String, bool>;

/// A single step in a task plan
pub struct PlanStep {
    pub description: String,
    pub completed: bool,
    pub action_name: String,
    pub dependencies: Vec<usize>,
    pub estimated_cost: u32,
    pub tool_required: Option<String>,
}

impl PlanStep {
    fn from_action(action: &Action, index: usize) -> Self {
        PlanStep {
            description: action.description.clone(),
            completed: false,
            action_name: action.name.clone(),
            dependencies: if index > 0 {
                alloc::vec![index - 1]
            } else {
                Vec::new()
            },
            estimated_cost: action.cost,
            tool_required: action.tool.clone(),
        }
    }
}

/// An action definition with preconditions and effects
#[derive(Clone)]
pub struct Action {
    pub name: String,
    pub description: String,
    /// Preconditions: state keys that must be true for this action to be applicable
    pub preconditions: Vec<String>,
    /// Positive effects: state keys that become true after this action
    pub add_effects: Vec<String>,
    /// Negative effects: state keys that become false after this action
    pub delete_effects: Vec<String>,
    /// Cost for prioritizing cheaper action sequences
    pub cost: u32,
    /// Optional tool required to execute this action
    pub tool: Option<String>,
}

impl Action {
    pub fn new(name: &str, description: &str) -> Self {
        Action {
            name: String::from(name),
            description: String::from(description),
            preconditions: Vec::new(),
            add_effects: Vec::new(),
            delete_effects: Vec::new(),
            cost: 1,
            tool: None,
        }
    }

    pub fn precondition(mut self, prop: &str) -> Self {
        self.preconditions.push(String::from(prop));
        self
    }

    pub fn adds(mut self, prop: &str) -> Self {
        self.add_effects.push(String::from(prop));
        self
    }

    pub fn deletes(mut self, prop: &str) -> Self {
        self.delete_effects.push(String::from(prop));
        self
    }

    pub fn cost(mut self, c: u32) -> Self {
        self.cost = c;
        self
    }

    pub fn with_tool(mut self, tool: &str) -> Self {
        self.tool = Some(String::from(tool));
        self
    }

    /// Check if this action is applicable in the given state
    fn is_applicable(&self, state: &State) -> bool {
        self.preconditions
            .iter()
            .all(|pre| state.get(pre).copied().unwrap_or(false))
    }

    /// Apply this action to a state, returning the new state
    fn apply(&self, state: &State) -> State {
        let mut new_state = state.clone();
        for eff in &self.delete_effects {
            new_state.remove(eff);
        }
        for eff in &self.add_effects {
            new_state.insert(eff.clone(), true);
        }
        new_state
    }
}

/// A planning problem definition
pub struct PlanningProblem {
    pub initial_state: State,
    pub goal: Vec<String>,
    pub actions: Vec<Action>,
}

/// Search node for forward chaining
struct SearchNode {
    state: State,
    plan: Vec<usize>, // indices into the action list
    total_cost: u32,
}

/// Decomposes high-level goals into executable steps
pub struct TaskPlanner {
    pub steps: Vec<PlanStep>,
    /// Library of known actions
    action_library: Vec<Action>,
    /// Template decomposition rules: goal keyword -> list of sub-actions
    decomposition_rules: BTreeMap<String, Vec<String>>,
    /// Maximum search depth for forward chaining
    max_search_depth: usize,
    /// Maximum search nodes to explore (prevent combinatorial explosion)
    max_search_nodes: usize,
}

impl TaskPlanner {
    pub fn new() -> Self {
        TaskPlanner {
            steps: Vec::new(),
            action_library: Vec::new(),
            decomposition_rules: BTreeMap::new(),
            max_search_depth: 20,
            max_search_nodes: 5000,
        }
    }

    /// Register an action in the library
    pub fn register_action(&mut self, action: Action) {
        self.action_library.push(action);
    }

    /// Register a decomposition rule: when a goal contains `keyword`,
    /// the planner suggests these sub-action names
    pub fn register_decomposition(&mut self, keyword: &str, sub_actions: &[&str]) {
        let actions: Vec<String> = sub_actions.iter().map(|s| String::from(*s)).collect();
        self.decomposition_rules
            .insert(String::from(keyword), actions);
    }

    /// Decompose a high-level goal into plan steps.
    ///
    /// First tries forward-chaining search if the goal maps to a formal
    /// planning problem. Falls back to keyword-based decomposition.
    pub fn decompose(&mut self, goal: &str) -> Vec<PlanStep> {
        self.steps.clear();

        // Try keyword-based decomposition first
        let lower_goal = goal.to_lowercase();
        let mut matched_actions: Vec<String> = Vec::new();

        for (keyword, sub_actions) in &self.decomposition_rules {
            if lower_goal.contains(keyword.as_str()) {
                matched_actions.extend(sub_actions.clone());
            }
        }

        if !matched_actions.is_empty() {
            // Build plan steps from matched decomposition rules
            for (i, action_name) in matched_actions.iter().enumerate() {
                // Find the action in our library
                let action = self.action_library.iter().find(|a| a.name == *action_name);

                let step = match action {
                    Some(a) => PlanStep::from_action(a, i),
                    None => PlanStep {
                        description: format!("Execute: {}", action_name),
                        completed: false,
                        action_name: action_name.clone(),
                        dependencies: if i > 0 {
                            alloc::vec![i - 1]
                        } else {
                            Vec::new()
                        },
                        estimated_cost: 1,
                        tool_required: None,
                    },
                };
                self.steps.push(step);
            }
        } else {
            // Fallback: create generic sub-steps based on goal analysis
            let sub_goals = analyze_goal(goal);
            for (i, sub_goal) in sub_goals.iter().enumerate() {
                self.steps.push(PlanStep {
                    description: sub_goal.clone(),
                    completed: false,
                    action_name: format!("step_{}", i),
                    dependencies: if i > 0 {
                        alloc::vec![i - 1]
                    } else {
                        Vec::new()
                    },
                    estimated_cost: 1,
                    tool_required: None,
                });
            }
        }

        self.steps.clone()
    }

    /// Solve a formal planning problem using forward-chaining BFS
    pub fn solve(&self, problem: &PlanningProblem) -> Option<Vec<PlanStep>> {
        let initial = SearchNode {
            state: problem.initial_state.clone(),
            plan: Vec::new(),
            total_cost: 0,
        };

        // BFS with cost tracking
        let mut open: Vec<SearchNode> = Vec::new();
        open.push(initial);
        let mut nodes_explored = 0usize;

        // Track visited states to avoid cycles (use state fingerprint)
        let mut visited: Vec<u64> = Vec::new();

        while !open.is_empty() && nodes_explored < self.max_search_nodes {
            // Find the node with lowest cost (best-first search)
            let mut best_idx = 0;
            let mut best_cost = u32::MAX;
            for (i, node) in open.iter().enumerate() {
                let heuristic = self.heuristic_cost(&node.state, &problem.goal);
                let f = node.total_cost + heuristic;
                if f < best_cost {
                    best_cost = f;
                    best_idx = i;
                }
            }

            let current = open.remove(best_idx);
            nodes_explored += 1;

            // Check if goal is satisfied
            if self.goal_satisfied(&current.state, &problem.goal) {
                // Convert action indices to PlanSteps
                let steps: Vec<PlanStep> = current
                    .plan
                    .iter()
                    .enumerate()
                    .map(|(i, &action_idx)| PlanStep::from_action(&problem.actions[action_idx], i))
                    .collect();
                return Some(steps);
            }

            // Depth limit
            if current.plan.len() >= self.max_search_depth {
                continue;
            }

            // State fingerprint for cycle detection
            let fp = state_fingerprint(&current.state);
            if visited.contains(&fp) {
                continue;
            }
            visited.push(fp);

            // Expand: try each applicable action
            for (action_idx, action) in problem.actions.iter().enumerate() {
                if action.is_applicable(&current.state) {
                    let new_state = action.apply(&current.state);
                    let mut new_plan = current.plan.clone();
                    new_plan.push(action_idx);

                    open.push(SearchNode {
                        state: new_state,
                        plan: new_plan,
                        total_cost: current.total_cost + action.cost,
                    });
                }
            }
        }

        None // No plan found
    }

    /// Check if all goal propositions are true in the state
    fn goal_satisfied(&self, state: &State, goal: &[String]) -> bool {
        goal.iter().all(|g| state.get(g).copied().unwrap_or(false))
    }

    /// Heuristic: count of unsatisfied goal propositions (admissible)
    fn heuristic_cost(&self, state: &State, goal: &[String]) -> u32 {
        goal.iter()
            .filter(|g| !state.get(*g).copied().unwrap_or(false))
            .count() as u32
    }

    /// Mark a step as completed
    pub fn complete_step(&mut self, index: usize) -> bool {
        if index < self.steps.len() {
            self.steps[index].completed = true;
            true
        } else {
            false
        }
    }

    /// Check if all steps are completed
    pub fn is_complete(&self) -> bool {
        !self.steps.is_empty() && self.steps.iter().all(|s| s.completed)
    }

    /// Get the next executable step (one whose dependencies are all completed)
    pub fn next_step(&self) -> Option<(usize, &PlanStep)> {
        for (i, step) in self.steps.iter().enumerate() {
            if step.completed {
                continue;
            }
            let deps_met = step
                .dependencies
                .iter()
                .all(|&dep| self.steps.get(dep).map(|s| s.completed).unwrap_or(true));
            if deps_met {
                return Some((i, step));
            }
        }
        None
    }

    /// Get progress as a fraction (0.0 to 1.0)
    pub fn progress(&self) -> f32 {
        if self.steps.is_empty() {
            return 0.0;
        }
        let completed = self.steps.iter().filter(|s| s.completed).count();
        completed as f32 / self.steps.len() as f32
    }

    /// Total estimated cost of the plan
    pub fn total_cost(&self) -> u32 {
        self.steps.iter().map(|s| s.estimated_cost).sum()
    }

    /// Reset the plan
    pub fn reset(&mut self) {
        self.steps.clear();
    }

    /// Number of registered actions
    pub fn action_count(&self) -> usize {
        self.action_library.len()
    }
}

// PlanStep needs Clone for decompose to return a Vec
impl Clone for PlanStep {
    fn clone(&self) -> Self {
        PlanStep {
            description: self.description.clone(),
            completed: self.completed,
            action_name: self.action_name.clone(),
            dependencies: self.dependencies.clone(),
            estimated_cost: self.estimated_cost,
            tool_required: self.tool_required.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Analyze a goal string and break it into sub-goals
fn analyze_goal(goal: &str) -> Vec<String> {
    let mut sub_goals = Vec::new();
    let _lower = goal.to_lowercase();

    // Check for compound goals (and, then, also)
    let connectors = ["and then", " then ", " and ", " also ", "; "];
    let mut parts: Vec<String> = Vec::new();
    let mut remaining = String::from(goal);

    for conn in &connectors {
        if remaining.to_lowercase().contains(conn) {
            let split: Vec<&str> = remaining.split(conn).collect();
            for part in split {
                let trimmed = part.trim();
                if !trimmed.is_empty() {
                    parts.push(String::from(trimmed));
                }
            }
            remaining = String::new();
            break;
        }
    }

    if parts.is_empty() && !remaining.is_empty() {
        parts.push(remaining);
    }

    // For each part, generate a structured sub-goal
    for part in &parts {
        // Analysis step
        sub_goals.push(format!("Analyze requirements: {}", part));
        // Planning step
        sub_goals.push(format!("Determine approach for: {}", part));
        // Execution step
        sub_goals.push(format!("Execute: {}", part));
        // Verification step
        sub_goals.push(format!("Verify completion: {}", part));
    }

    if sub_goals.is_empty() {
        sub_goals.push(format!("Analyze: {}", goal));
        sub_goals.push(format!("Execute: {}", goal));
        sub_goals.push(format!("Verify: {}", goal));
    }

    sub_goals
}

/// Generate a simple fingerprint of a state for cycle detection
fn state_fingerprint(state: &State) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325; // FNV offset basis
    for (key, &val) in state {
        if val {
            for b in key.bytes() {
                hash ^= b as u64;
                hash = hash.wrapping_mul(0x100000001b3); // FNV prime
            }
        }
    }
    hash
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static PLANNER: Mutex<Option<TaskPlanner>> = Mutex::new(None);

pub fn init() {
    let mut planner = TaskPlanner::new();

    // Register common OS-level actions
    planner.register_action(
        Action::new("scan_files", "Scan filesystem for relevant files")
            .adds("files_scanned")
            .cost(2)
            .with_tool("filesystem"),
    );
    planner.register_action(
        Action::new("analyze_content", "Analyze file content")
            .precondition("files_scanned")
            .adds("content_analyzed")
            .cost(3)
            .with_tool("document_analysis"),
    );
    planner.register_action(
        Action::new("extract_data", "Extract structured data")
            .precondition("content_analyzed")
            .adds("data_extracted")
            .cost(2),
    );
    planner.register_action(
        Action::new("classify_data", "Classify extracted data")
            .precondition("data_extracted")
            .adds("data_classified")
            .cost(2)
            .with_tool("classifier"),
    );
    planner.register_action(
        Action::new("generate_report", "Generate summary report")
            .precondition("data_classified")
            .adds("report_generated")
            .cost(3)
            .with_tool("summarizer"),
    );
    planner.register_action(
        Action::new("check_safety", "Run safety checks on content")
            .adds("safety_checked")
            .cost(1)
            .with_tool("safety_filter"),
    );
    planner.register_action(
        Action::new("search_memory", "Search knowledge graph for context")
            .adds("context_loaded")
            .cost(1)
            .with_tool("memory_graph"),
    );
    planner.register_action(
        Action::new("run_inference", "Run model inference")
            .precondition("context_loaded")
            .adds("inference_complete")
            .cost(5)
            .with_tool("inference"),
    );

    // Register decomposition rules
    planner.register_decomposition(
        "analyze",
        &["scan_files", "analyze_content", "extract_data"],
    );
    planner.register_decomposition(
        "classify",
        &["scan_files", "analyze_content", "classify_data"],
    );
    planner.register_decomposition(
        "summarize",
        &["scan_files", "analyze_content", "generate_report"],
    );
    planner.register_decomposition("search", &["search_memory", "run_inference"]);
    planner.register_decomposition(
        "report",
        &[
            "scan_files",
            "analyze_content",
            "extract_data",
            "classify_data",
            "generate_report",
        ],
    );

    *PLANNER.lock() = Some(planner);
    crate::serial_println!(
        "    [planning] Task planner ready (8 actions, forward-chaining search)"
    );
}

/// Decompose a goal into plan steps
pub fn decompose(goal: &str) -> Vec<PlanStep> {
    PLANNER
        .lock()
        .as_mut()
        .map(|p| p.decompose(goal))
        .unwrap_or_else(Vec::new)
}
