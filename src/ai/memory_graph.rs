use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::String;
/// Long-term AI memory as a knowledge graph
///
/// Part of the AIOS AI layer. Nodes with typed edges, traversal,
/// shortest path (BFS), subgraph extraction, and memory decay
/// (reduce edge weights over time). Supports entity-relation-entity
/// triples and keyword-based recall.
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// Type of relationship between nodes
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdgeType {
    RelatedTo,
    IsA,
    HasProperty,
    PartOf,
    Causes,
    Precedes,
    Contains,
    References,
    Custom(String),
}

/// A directed edge in the knowledge graph
#[derive(Clone)]
pub struct Edge {
    pub target: u64,
    pub edge_type: EdgeType,
    pub weight: f32,
    pub created_at: u64,
    pub label: String,
}

/// A node in the knowledge graph
pub struct MemoryNode {
    pub id: u64,
    pub content: String,
    pub edges: Vec<u64>,
    /// Typed edges with weights
    pub typed_edges: Vec<Edge>,
    /// Keywords extracted from content for search
    pub keywords: Vec<String>,
    /// Access count for popularity tracking
    pub access_count: u64,
    /// Creation timestamp
    pub created_at: u64,
    /// Last accessed timestamp
    pub last_accessed: u64,
    /// Metadata key-value pairs
    pub metadata: BTreeMap<String, String>,
}

impl MemoryNode {
    fn new(id: u64, content: &str, timestamp: u64) -> Self {
        let keywords = extract_keywords(content);
        MemoryNode {
            id,
            content: String::from(content),
            edges: Vec::new(),
            typed_edges: Vec::new(),
            keywords,
            access_count: 0,
            created_at: timestamp,
            last_accessed: timestamp,
            metadata: BTreeMap::new(),
        }
    }
}

/// Persistent knowledge graph for long-term AI memory
pub struct MemoryGraph {
    pub nodes: Vec<MemoryNode>,
    /// Next node ID to assign
    next_id: u64,
    /// Logical clock for timestamps
    clock: u64,
    /// Keyword index: keyword -> list of node IDs containing that keyword
    keyword_index: BTreeMap<String, Vec<u64>>,
    /// Decay rate per tick (multiplied against edge weights)
    decay_rate: f32,
    /// Maximum number of nodes before pruning
    max_nodes: usize,
}

impl MemoryGraph {
    pub fn new() -> Self {
        MemoryGraph {
            nodes: Vec::new(),
            next_id: 1,
            clock: 1,
            keyword_index: BTreeMap::new(),
            decay_rate: 0.999,
            max_nodes: 4096,
        }
    }

    /// Store new content as a node, returning its ID
    pub fn store(&mut self, content: &str) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.clock = self.clock.saturating_add(1);

        let node = MemoryNode::new(id, content, self.clock);

        // Update keyword index
        for kw in &node.keywords {
            self.keyword_index
                .entry(kw.clone())
                .or_insert_with(Vec::new)
                .push(id);
        }

        self.nodes.push(node);

        // Auto-link to related nodes based on keyword overlap
        self.auto_link(id);

        // Prune if over capacity
        if self.nodes.len() > self.max_nodes {
            self.prune_least_accessed(self.max_nodes / 10);
        }

        id
    }

    /// Store with metadata
    pub fn store_with_meta(&mut self, content: &str, metadata: &[(&str, &str)]) -> u64 {
        let id = self.store(content);
        if let Some(node) = self.find_node_mut(id) {
            for &(key, val) in metadata {
                node.metadata.insert(String::from(key), String::from(val));
            }
        }
        id
    }

    /// Add a typed edge between two nodes
    pub fn add_edge(&mut self, from: u64, to: u64, edge_type: EdgeType, label: &str) {
        self.clock = self.clock.saturating_add(1);
        let clock = self.clock;
        if let Some(node) = self.find_node_mut(from) {
            // Add to flat edge list
            if !node.edges.contains(&to) {
                node.edges.push(to);
            }
            // Add typed edge
            node.typed_edges.push(Edge {
                target: to,
                edge_type,
                weight: 1.0,
                created_at: clock,
                label: String::from(label),
            });
        }
    }

    /// Add a bidirectional edge
    pub fn add_bidi_edge(&mut self, a: u64, b: u64, edge_type: EdgeType, label: &str) {
        self.add_edge(a, b, edge_type.clone(), label);
        self.add_edge(b, a, edge_type, label);
    }

    /// Recall nodes matching a query, ranked by relevance
    pub fn recall(&self, query: &str, top_k: usize) -> Vec<&MemoryNode> {
        let query_keywords = extract_keywords(query);
        if query_keywords.is_empty() {
            return Vec::new();
        }

        // Score each node by keyword overlap + recency + access frequency
        let mut scored: Vec<(usize, f32)> = Vec::new();

        for (idx, node) in self.nodes.iter().enumerate() {
            let mut score = 0.0f32;

            // Keyword overlap scoring
            for qkw in &query_keywords {
                for nkw in &node.keywords {
                    if qkw == nkw {
                        score += 2.0;
                    } else if nkw.contains(qkw.as_str()) || qkw.contains(nkw.as_str()) {
                        score += 1.0;
                    }
                }
            }

            // Direct content match bonus
            let query_lower = query.to_lowercase();
            if node.content.to_lowercase().contains(&query_lower) {
                score += 3.0;
            }

            // Recency bonus: more recent nodes get a small boost
            if self.clock > node.created_at {
                let age = (self.clock - node.created_at) as f32;
                score += 1.0 / (1.0 + age * 0.01);
            }

            // Access frequency bonus
            score += (node.access_count as f32).min(5.0) * 0.2;

            if score > 0.0 {
                scored.push((idx, score));
            }
        }

        // Sort by score descending
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));

        // Return top-k
        scored
            .iter()
            .take(top_k)
            .filter_map(|(idx, _)| self.nodes.get(*idx))
            .collect()
    }

    /// BFS shortest path between two nodes. Returns list of node IDs in the path.
    pub fn shortest_path(&self, from: u64, to: u64) -> Option<Vec<u64>> {
        if from == to {
            return Some(alloc::vec![from]);
        }

        // BFS
        let mut visited: BTreeMap<u64, u64> = BTreeMap::new(); // node -> predecessor
        let mut queue: Vec<u64> = Vec::new();
        queue.push(from);
        visited.insert(from, from);

        while !queue.is_empty() {
            let current = queue.remove(0);

            if let Some(node) = self.find_node(current) {
                for &neighbor in &node.edges {
                    if visited.contains_key(&neighbor) {
                        continue;
                    }
                    visited.insert(neighbor, current);

                    if neighbor == to {
                        // Reconstruct path
                        let mut path = Vec::new();
                        let mut step = to;
                        path.push(step);
                        while step != from {
                            step = *visited.get(&step).unwrap_or(&from);
                            path.push(step);
                        }
                        path.reverse();
                        return Some(path);
                    }

                    queue.push(neighbor);
                }
            }
        }

        None // No path found
    }

    /// Extract a subgraph: all nodes within `depth` hops of the given node
    pub fn subgraph(&self, center: u64, depth: usize) -> Vec<u64> {
        let mut visited: Vec<u64> = Vec::new();
        let mut frontier: Vec<u64> = Vec::new();
        frontier.push(center);
        visited.push(center);

        for _ in 0..depth {
            let mut next_frontier = Vec::new();
            for &node_id in &frontier {
                if let Some(node) = self.find_node(node_id) {
                    for &neighbor in &node.edges {
                        if !visited.contains(&neighbor) {
                            visited.push(neighbor);
                            next_frontier.push(neighbor);
                        }
                    }
                }
            }
            if next_frontier.is_empty() {
                break;
            }
            frontier = next_frontier;
        }

        visited
    }

    /// Apply memory decay: reduce all edge weights by the decay factor
    pub fn apply_decay(&mut self) {
        for node in &mut self.nodes {
            for edge in &mut node.typed_edges {
                edge.weight *= self.decay_rate;
            }
        }
    }

    /// Boost the weight of edges connected to a specific node (reinforcement)
    pub fn reinforce(&mut self, node_id: u64, boost: f32) {
        let clock = self.clock;
        if let Some(node) = self.find_node_mut(node_id) {
            node.access_count += 1;
            node.last_accessed = clock;
            for edge in &mut node.typed_edges {
                edge.weight = (edge.weight + boost).min(10.0);
            }
        }
    }

    /// Get all edges from a node, optionally filtered by type
    pub fn edges_from(&self, node_id: u64, edge_type: Option<&EdgeType>) -> Vec<&Edge> {
        match self.find_node(node_id) {
            Some(node) => match edge_type {
                Some(et) => node
                    .typed_edges
                    .iter()
                    .filter(|e| &e.edge_type == et)
                    .collect(),
                None => node.typed_edges.iter().collect(),
            },
            None => Vec::new(),
        }
    }

    /// Find all nodes of a specific type (via metadata "type" key)
    pub fn nodes_of_type(&self, type_name: &str) -> Vec<&MemoryNode> {
        self.nodes
            .iter()
            .filter(|n| n.metadata.get("type").map(|t| t.as_str()) == Some(type_name))
            .collect()
    }

    /// Count total nodes
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Count total edges
    pub fn edge_count(&self) -> usize {
        self.nodes.iter().map(|n| n.typed_edges.len()).sum()
    }

    /// Remove a node by ID
    pub fn remove_node(&mut self, id: u64) {
        // Remove from keyword index
        if let Some(node) = self.find_node(id) {
            let keywords = node.keywords.clone();
            for kw in &keywords {
                if let Some(ids) = self.keyword_index.get_mut(kw) {
                    ids.retain(|&nid| nid != id);
                }
            }
        }

        // Remove the node
        self.nodes.retain(|n| n.id != id);

        // Remove edges pointing to this node from other nodes
        for node in &mut self.nodes {
            node.edges.retain(|&e| e != id);
            node.typed_edges.retain(|e| e.target != id);
        }
    }

    /// Get a node by ID
    pub fn get_node(&self, id: u64) -> Option<&MemoryNode> {
        self.find_node(id)
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    fn find_node(&self, id: u64) -> Option<&MemoryNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    fn find_node_mut(&mut self, id: u64) -> Option<&mut MemoryNode> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    /// Auto-link new node to existing nodes with keyword overlap
    fn auto_link(&mut self, new_id: u64) {
        let new_keywords: Vec<String> = match self.find_node(new_id) {
            Some(n) => n.keywords.clone(),
            None => return,
        };

        // Collect candidate IDs (nodes sharing keywords)
        let mut candidates: BTreeMap<u64, u32> = BTreeMap::new();
        for kw in &new_keywords {
            if let Some(ids) = self.keyword_index.get(kw) {
                for &id in ids {
                    if id != new_id {
                        *candidates.entry(id).or_insert(0) += 1;
                    }
                }
            }
        }

        // Link to nodes with 2+ shared keywords
        let clock = self.clock;
        for (target_id, overlap) in &candidates {
            if *overlap >= 2 {
                if let Some(node) = self.find_node_mut(new_id) {
                    if !node.edges.contains(target_id) {
                        node.edges.push(*target_id);
                        node.typed_edges.push(Edge {
                            target: *target_id,
                            edge_type: EdgeType::RelatedTo,
                            weight: *overlap as f32 * 0.5,
                            created_at: clock,
                            label: String::from("auto"),
                        });
                    }
                }
            }
        }
    }

    /// Prune the N least-accessed nodes
    fn prune_least_accessed(&mut self, n: usize) {
        if self.nodes.len() <= n {
            return;
        }

        // Find nodes with lowest access counts
        let mut indices: Vec<(usize, u64)> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(i, node)| (i, node.access_count))
            .collect();
        indices.sort_by(|a, b| a.1.cmp(&b.1));

        let to_remove: Vec<u64> = indices
            .iter()
            .take(n)
            .filter_map(|(i, _)| self.nodes.get(*i).map(|n| n.id))
            .collect();

        for id in to_remove {
            self.remove_node(id);
        }
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Extract keywords from content for indexing
fn extract_keywords(content: &str) -> Vec<String> {
    let stop_words = [
        "the", "a", "an", "is", "are", "was", "were", "be", "been", "have", "has", "had", "do",
        "does", "did", "will", "would", "to", "of", "in", "for", "on", "with", "at", "by", "from",
        "and", "but", "or", "not", "no", "it", "its", "this", "that",
    ];

    let mut keywords = Vec::new();
    for chunk in content.split(|c: char| !c.is_alphanumeric()) {
        if chunk.len() < 3 {
            continue;
        }
        let lower = chunk.to_lowercase();
        if stop_words.contains(&lower.as_str()) {
            continue;
        }
        if !keywords.contains(&lower) {
            keywords.push(lower);
        }
    }
    keywords
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static GRAPH: Mutex<Option<MemoryGraph>> = Mutex::new(None);

pub fn init() {
    let graph = MemoryGraph::new();
    *GRAPH.lock() = Some(graph);
    crate::serial_println!("    [memory_graph] Knowledge graph ready (BFS, decay, auto-link)");
}

/// Store a new memory node
pub fn store(content: &str) -> u64 {
    GRAPH.lock().as_mut().map(|g| g.store(content)).unwrap_or(0)
}

/// Recall nodes matching a query
pub fn recall(query: &str, top_k: usize) -> Vec<String> {
    GRAPH
        .lock()
        .as_ref()
        .map(|g| {
            g.recall(query, top_k)
                .iter()
                .map(|n| n.content.clone())
                .collect()
        })
        .unwrap_or_else(Vec::new)
}
