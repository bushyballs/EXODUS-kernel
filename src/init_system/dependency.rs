/// Dependency graph resolution for service boot ordering
///
/// Part of the AIOS init_system subsystem.
///
/// Implements a directed acyclic graph (DAG) of service dependencies.
/// Uses Kahn's algorithm for topological sort to determine correct
/// startup order while detecting circular dependencies.
///
/// Original implementation for Hoags OS. No external crates.

use crate::sync::Mutex;
use alloc::vec::Vec;
use alloc::vec;

use crate::{serial_print, serial_println};

// ── FNV-1a helper ──────────────────────────────────────────────────────────

fn fnv1a_hash(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

// ── Edge representation ────────────────────────────────────────────────────

/// A directed edge: `from` depends on `to` (i.e. `to` must start first).
#[derive(Clone, Copy)]
struct Edge {
    from: usize,
    to: usize,
}

/// Node metadata in the dependency graph.
#[derive(Clone)]
struct Node {
    name_hash: u64,
    in_degree: usize,
    /// Indices into the edges vec for outgoing edges from this node.
    out_edges: Vec<usize>,
}

// ── Core graph ─────────────────────────────────────────────────────────────

/// Directed graph of service dependencies for boot ordering.
pub struct DependencyGraph {
    edges: Vec<Edge>,
    nodes: Vec<Node>,
    node_count: usize,
}

impl DependencyGraph {
    pub fn new() -> Self {
        DependencyGraph {
            edges: Vec::new(),
            nodes: Vec::new(),
            node_count: 0,
        }
    }

    /// Add a named node and return its index.
    pub fn add_node(&mut self, name: &str) -> usize {
        let hash = fnv1a_hash(name.as_bytes());
        // Check if node already exists
        for (i, n) in self.nodes.iter().enumerate() {
            if n.name_hash == hash {
                return i;
            }
        }
        let idx = self.node_count;
        self.nodes.push(Node {
            name_hash: hash,
            in_degree: 0,
            out_edges: Vec::new(),
        });
        self.node_count = self.node_count.saturating_add(1);
        idx
    }

    /// Ensure capacity for at least `count` nodes (index-based API).
    pub fn ensure_nodes(&mut self, count: usize) {
        while self.node_count < count {
            self.nodes.push(Node {
                name_hash: 0,
                in_degree: 0,
                out_edges: Vec::new(),
            });
            self.node_count = self.node_count.saturating_add(1);
        }
    }

    /// Add a dependency: service `from` requires service `to`
    /// (`to` must start before `from`).
    pub fn add_dependency(&mut self, from: usize, to: usize) {
        self.ensure_nodes(core::cmp::max(from, to) + 1);

        // Check for duplicate edge
        for e in &self.edges {
            if e.from == from && e.to == to {
                return;
            }
        }

        let edge_idx = self.edges.len();
        self.edges.push(Edge { from, to });
        self.nodes[to].out_edges.push(edge_idx);
        self.nodes[from].in_degree = self.nodes[from].in_degree.saturating_add(1);
    }

    /// Topological sort using Kahn's algorithm.
    /// Returns node indices in valid startup order (dependencies first).
    /// Returns empty vec if a cycle is detected.
    pub fn resolve_order(&self) -> Vec<usize> {
        if self.node_count == 0 {
            return Vec::new();
        }

        // Copy in-degrees so we can mutate them
        let mut in_deg: Vec<usize> = self.nodes.iter().map(|n| n.in_degree).collect();

        // Queue of nodes with zero in-degree
        let mut queue: Vec<usize> = Vec::new();
        for i in 0..self.node_count {
            if in_deg[i] == 0 {
                queue.push(i);
            }
        }

        let mut order: Vec<usize> = Vec::with_capacity(self.node_count);
        let mut head = 0;

        while head < queue.len() {
            let node = queue[head];
            head += 1;
            order.push(node);

            // For each outgoing edge from this node, decrement the
            // in-degree of the target. An edge (from, to) means `to`
            // feeds into `from`, so completing `to` reduces `from`'s deps.
            for &edge_idx in &self.nodes[node].out_edges {
                let target = self.edges[edge_idx].from;
                if in_deg[target] > 0 {
                    in_deg[target] -= 1;
                    if in_deg[target] == 0 {
                        queue.push(target);
                    }
                }
            }
        }

        // If we couldn't order all nodes, there's a cycle
        if order.len() != self.node_count {
            serial_println!("[init_system::dependency] cycle detected in dependency graph");
            return Vec::new();
        }

        order
    }

    /// Check for circular dependencies using iterative DFS with coloring.
    /// White = 0, Grey = 1, Black = 2.
    pub fn has_cycle(&self) -> bool {
        if self.node_count == 0 {
            return false;
        }

        let mut color: Vec<u8> = vec![0u8; self.node_count];

        for start in 0..self.node_count {
            if color[start] != 0 {
                continue;
            }

            // Iterative DFS stack: (node, edge_iterator_index)
            let mut stack: Vec<(usize, usize)> = Vec::new();
            color[start] = 1; // Grey
            stack.push((start, 0));

            while let Some(&mut (node, ref mut ei)) = stack.last_mut() {
                // Build adjacency: node's dependents (edges where node == to)
                let adj = self.adjacents_of(node);
                if *ei < adj.len() {
                    let next = adj[*ei];
                    *ei += 1;
                    match color[next] {
                        1 => return true, // Back-edge: cycle
                        0 => {
                            color[next] = 1;
                            stack.push((next, 0));
                        }
                        _ => {} // Black: already fully explored
                    }
                } else {
                    color[node] = 2; // Black
                    stack.pop();
                }
            }
        }

        false
    }

    /// Get all direct dependencies of a node (nodes that must start before it).
    pub fn dependencies_of(&self, node: usize) -> Vec<usize> {
        if node >= self.node_count {
            return Vec::new();
        }
        let mut deps = Vec::new();
        for e in &self.edges {
            if e.from == node {
                deps.push(e.to);
            }
        }
        deps
    }

    /// Get all nodes that depend on the given node (its "reverse" deps).
    pub fn dependents_of(&self, node: usize) -> Vec<usize> {
        if node >= self.node_count {
            return Vec::new();
        }
        let mut deps = Vec::new();
        for e in &self.edges {
            if e.to == node {
                deps.push(e.from);
            }
        }
        deps
    }

    /// Internal: get outgoing-adjacency list for DFS (from `to` -> `from`).
    fn adjacents_of(&self, node: usize) -> Vec<usize> {
        let mut adj = Vec::new();
        for &edge_idx in &self.nodes[node].out_edges {
            adj.push(self.edges[edge_idx].from);
        }
        adj
    }

    /// Number of nodes in the graph.
    pub fn node_count(&self) -> usize {
        self.node_count
    }

    /// Number of edges in the graph.
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Remove all edges involving a node (for runtime unload).
    pub fn remove_node_edges(&mut self, node: usize) {
        if node >= self.node_count {
            return;
        }
        // Recalculate in-degrees after removing edges
        self.edges.retain(|e| e.from != node && e.to != node);

        // Rebuild in-degrees and out_edges from scratch
        for n in self.nodes.iter_mut() {
            n.in_degree = 0;
            n.out_edges.clear();
        }
        for (i, e) in self.edges.iter().enumerate() {
            self.nodes[e.from].in_degree += 1;
            self.nodes[e.to].out_edges.push(i);
        }
    }
}

// ── Global state ───────────────────────────────────────────────────────────

static DEP_GRAPH: Mutex<Option<DependencyGraph>> = Mutex::new(None);

/// Initialize the dependency graph subsystem.
pub fn init() {
    let mut guard = DEP_GRAPH.lock();
    *guard = Some(DependencyGraph::new());
    serial_println!("[init_system::dependency] dependency graph initialized");
}

/// Add a dependency relationship: `service` requires `dependency`.
pub fn add_dep(service: usize, dependency: usize) {
    let mut guard = DEP_GRAPH.lock();
    let graph = guard.as_mut().expect("dependency graph not initialized");
    graph.add_dependency(service, dependency);
}

/// Compute the startup order. Returns empty if there's a cycle.
pub fn compute_order() -> Vec<usize> {
    let guard = DEP_GRAPH.lock();
    let graph = guard.as_ref().expect("dependency graph not initialized");
    graph.resolve_order()
}

/// Check if the current graph has a cycle.
pub fn check_cycle() -> bool {
    let guard = DEP_GRAPH.lock();
    let graph = guard.as_ref().expect("dependency graph not initialized");
    graph.has_cycle()
}
