/// B-tree index for Genesis embedded database
///
/// Order-64 B-tree supporting:
///   - Insert with node splitting
///   - Search (exact key lookup)
///   - Delete with underflow rebalancing
///   - Range scan (inclusive min..max)
///   - In-order traversal
///
/// Keys are i64, values are u64 (row pointers / page IDs).
/// No floating-point — all internal math is integer or Q16.
///
/// Inspired by: SQLite B-tree pager, PostgreSQL nbtree, LMDB.
/// All code is original.
use crate::{serial_print, serial_println};

use crate::sync::Mutex;
use alloc::vec;
use alloc::vec::Vec;

/// Q16 fixed-point constant
const Q16_ONE: i32 = 65536;

/// Maximum keys per node (order = MAX_KEYS + 1)
const MAX_KEYS: usize = 63;
/// Minimum keys (except root)
const MIN_KEYS: usize = 31;

/// A key-value pair stored in the B-tree
#[derive(Clone, Copy, Debug)]
pub struct BTreeEntry {
    pub key: i64,
    pub value: u64,
}

/// A single B-tree node
#[derive(Clone)]
struct BTreeNode {
    /// Key-value pairs stored in this node, sorted by key
    entries: Vec<BTreeEntry>,
    /// Child node indices (len = entries.len() + 1 for internal nodes, 0 for leaves)
    children: Vec<usize>,
    /// Whether this is a leaf node
    is_leaf: bool,
    /// Unique node ID
    node_id: u32,
}

impl BTreeNode {
    fn new(is_leaf: bool, node_id: u32) -> Self {
        BTreeNode {
            entries: Vec::new(),
            children: Vec::new(),
            is_leaf,
            node_id,
        }
    }

    fn is_full(&self) -> bool {
        self.entries.len() >= MAX_KEYS
    }

    fn key_count(&self) -> usize {
        self.entries.len()
    }

    /// Binary search for a key. Returns Ok(index) if found, Err(index) for insertion point.
    fn search_key(&self, key: i64) -> Result<usize, usize> {
        let mut lo = 0usize;
        let mut hi = self.entries.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.entries[mid].key == key {
                return Ok(mid);
            } else if self.entries[mid].key < key {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        Err(lo)
    }
}

/// The B-tree index structure
pub struct BTree {
    /// All nodes stored in a flat vector (arena allocation)
    nodes: Vec<BTreeNode>,
    /// Index of the root node
    root: usize,
    /// Next node ID to assign
    next_node_id: u32,
    /// Total number of entries across all nodes
    entry_count: u64,
    /// Tree height
    height: u32,
}

static BTREE_POOL: Mutex<Option<BTreePool>> = Mutex::new(None);

struct BTreePool {
    trees: Vec<BTree>,
    next_tree_id: u32,
}

impl BTree {
    /// Create a new empty B-tree
    pub fn new() -> Self {
        let root_node = BTreeNode::new(true, 0);
        BTree {
            nodes: vec![root_node],
            root: 0,
            next_node_id: 1,
            entry_count: 0,
            height: 1,
        }
    }

    /// Allocate a new node in the arena
    fn alloc_node(&mut self, is_leaf: bool) -> usize {
        let id = self.next_node_id;
        self.next_node_id = self.next_node_id.saturating_add(1);
        let node = BTreeNode::new(is_leaf, id);
        let idx = self.nodes.len();
        self.nodes.push(node);
        idx
    }

    /// Search for an exact key. Returns Some(value) if found.
    pub fn search(&self, key: i64) -> Option<u64> {
        self.search_node(self.root, key)
    }

    fn search_node(&self, node_idx: usize, key: i64) -> Option<u64> {
        let node = &self.nodes[node_idx];
        match node.search_key(key) {
            Ok(i) => Some(node.entries[i].value),
            Err(i) => {
                if node.is_leaf {
                    None
                } else {
                    self.search_node(node.children[i], key)
                }
            }
        }
    }

    /// Insert a key-value pair. If key exists, updates the value.
    pub fn insert(&mut self, key: i64, value: u64) {
        // If root is full, split it first
        if self.nodes[self.root].is_full() {
            let old_root = self.root;
            let new_root_idx = self.alloc_node(false);
            self.nodes[new_root_idx].children.push(old_root);
            self.root = new_root_idx;
            self.split_child(new_root_idx, 0);
            self.height = self.height.saturating_add(1);
        }
        self.insert_non_full(self.root, key, value);
        self.entry_count = self.entry_count.saturating_add(1);
    }

    /// Split the i-th child of parent_idx
    fn split_child(&mut self, parent_idx: usize, child_pos: usize) {
        let child_idx = self.nodes[parent_idx].children[child_pos];
        let child_is_leaf = self.nodes[child_idx].is_leaf;
        let new_idx = self.alloc_node(child_is_leaf);

        let mid = self.nodes[child_idx].entries.len() / 2;

        // The median entry goes up to the parent
        let median_entry = self.nodes[child_idx].entries[mid];

        // Right half of entries goes to new node
        let right_entries: Vec<BTreeEntry> = self.nodes[child_idx].entries[mid + 1..].to_vec();
        self.nodes[new_idx].entries = right_entries;

        // If internal node, move right half of children too
        if !child_is_leaf {
            let right_children: Vec<usize> = self.nodes[child_idx].children[mid + 1..].to_vec();
            self.nodes[new_idx].children = right_children;
            self.nodes[child_idx].children.truncate(mid + 1);
        }

        // Truncate left child
        self.nodes[child_idx].entries.truncate(mid);

        // Insert median into parent
        self.nodes[parent_idx]
            .entries
            .insert(child_pos, median_entry);
        self.nodes[parent_idx]
            .children
            .insert(child_pos + 1, new_idx);
    }

    /// Insert into a node that is guaranteed not full
    fn insert_non_full(&mut self, node_idx: usize, key: i64, value: u64) {
        let node = &self.nodes[node_idx];
        match node.search_key(key) {
            Ok(i) => {
                // Key exists — update value in place
                self.nodes[node_idx].entries[i].value = value;
                // Don't increment entry_count for updates (caller handles)
                // We handle this by decrementing if it was an update
                self.entry_count = self.entry_count.wrapping_sub(1);
            }
            Err(i) => {
                if self.nodes[node_idx].is_leaf {
                    self.nodes[node_idx]
                        .entries
                        .insert(i, BTreeEntry { key, value });
                } else {
                    // Check if child is full
                    let child_idx = self.nodes[node_idx].children[i];
                    if self.nodes[child_idx].is_full() {
                        self.split_child(node_idx, i);
                        // After split, determine which child to recurse into
                        if key > self.nodes[node_idx].entries[i].key {
                            self.insert_non_full(self.nodes[node_idx].children[i + 1], key, value);
                        } else {
                            self.insert_non_full(self.nodes[node_idx].children[i], key, value);
                        }
                    } else {
                        self.insert_non_full(child_idx, key, value);
                    }
                }
            }
        }
    }

    /// Delete a key from the tree. Returns true if the key was found and removed.
    pub fn delete(&mut self, key: i64) -> bool {
        let found = self.delete_from_node(self.root, key);
        if found {
            self.entry_count = self.entry_count.saturating_sub(1);
            // If root has no entries but has a child, shrink height
            if self.nodes[self.root].entries.is_empty() && !self.nodes[self.root].is_leaf {
                self.root = self.nodes[self.root].children[0];
                self.height = self.height.saturating_sub(1);
            }
        }
        found
    }

    fn delete_from_node(&mut self, node_idx: usize, key: i64) -> bool {
        let node = &self.nodes[node_idx];
        match node.search_key(key) {
            Ok(i) => {
                if self.nodes[node_idx].is_leaf {
                    // Case 1: key in leaf — just remove it
                    self.nodes[node_idx].entries.remove(i);
                    true
                } else {
                    // Case 2: key in internal node — replace with predecessor
                    let pred_child = self.nodes[node_idx].children[i];
                    let pred = self.find_max(pred_child);
                    self.nodes[node_idx].entries[i] = pred;
                    self.delete_from_node(pred_child, pred.key)
                }
            }
            Err(i) => {
                if self.nodes[node_idx].is_leaf {
                    return false; // Key not found
                }
                let child_idx = self.nodes[node_idx].children[i];
                // Ensure child has enough keys before recursing
                if self.nodes[child_idx].key_count() <= MIN_KEYS {
                    self.rebalance_child(node_idx, i);
                    // After rebalance, the child index might have changed
                    // Re-search to find correct position
                    let node = &self.nodes[node_idx];
                    match node.search_key(key) {
                        Ok(j) => {
                            // Key was pushed into this node during merge
                            if self.nodes[node_idx].is_leaf {
                                self.nodes[node_idx].entries.remove(j);
                                return true;
                            }
                            let pred_child = self.nodes[node_idx].children[j];
                            let pred = self.find_max(pred_child);
                            self.nodes[node_idx].entries[j] = pred;
                            return self.delete_from_node(pred_child, pred.key);
                        }
                        Err(j) => {
                            if j < self.nodes[node_idx].children.len() {
                                return self
                                    .delete_from_node(self.nodes[node_idx].children[j], key);
                            }
                            return false;
                        }
                    }
                } else {
                    self.delete_from_node(child_idx, key)
                }
            }
        }
    }

    /// Find the maximum entry in the subtree rooted at node_idx
    fn find_max(&self, node_idx: usize) -> BTreeEntry {
        let node = &self.nodes[node_idx];
        if node.is_leaf {
            *node.entries.last().unwrap()
        } else {
            let last_child = *node.children.last().unwrap();
            self.find_max(last_child)
        }
    }

    /// Rebalance: ensure child at position i has more than MIN_KEYS
    fn rebalance_child(&mut self, parent_idx: usize, child_pos: usize) {
        let n_children = self.nodes[parent_idx].children.len();

        // Try borrowing from left sibling
        if child_pos > 0 {
            let left_sib_idx = self.nodes[parent_idx].children[child_pos - 1];
            if self.nodes[left_sib_idx].key_count() > MIN_KEYS {
                self.borrow_from_left(parent_idx, child_pos);
                return;
            }
        }
        // Try borrowing from right sibling
        if child_pos + 1 < n_children {
            let right_sib_idx = self.nodes[parent_idx].children[child_pos + 1];
            if self.nodes[right_sib_idx].key_count() > MIN_KEYS {
                self.borrow_from_right(parent_idx, child_pos);
                return;
            }
        }
        // Merge with a sibling
        if child_pos > 0 {
            self.merge_children(parent_idx, child_pos - 1);
        } else {
            self.merge_children(parent_idx, child_pos);
        }
    }

    fn borrow_from_left(&mut self, parent_idx: usize, child_pos: usize) {
        let left_idx = self.nodes[parent_idx].children[child_pos - 1];
        let child_idx = self.nodes[parent_idx].children[child_pos];

        // Parent separator moves down to child
        let separator = self.nodes[parent_idx].entries[child_pos - 1];
        self.nodes[child_idx].entries.insert(0, separator);

        // Last entry of left sibling moves up to parent
        let borrowed = self.nodes[left_idx].entries.pop().unwrap();
        self.nodes[parent_idx].entries[child_pos - 1] = borrowed;

        // Move rightmost child of left sibling if internal
        if !self.nodes[left_idx].is_leaf {
            let moved_child = self.nodes[left_idx].children.pop().unwrap();
            self.nodes[child_idx].children.insert(0, moved_child);
        }
    }

    fn borrow_from_right(&mut self, parent_idx: usize, child_pos: usize) {
        let right_idx = self.nodes[parent_idx].children[child_pos + 1];
        let child_idx = self.nodes[parent_idx].children[child_pos];

        // Parent separator moves down to child
        let separator = self.nodes[parent_idx].entries[child_pos];
        self.nodes[child_idx].entries.push(separator);

        // First entry of right sibling moves up to parent
        let borrowed = self.nodes[right_idx].entries.remove(0);
        self.nodes[parent_idx].entries[child_pos] = borrowed;

        // Move leftmost child of right sibling if internal
        if !self.nodes[right_idx].is_leaf {
            let moved_child = self.nodes[right_idx].children.remove(0);
            self.nodes[child_idx].children.push(moved_child);
        }
    }

    /// Merge children[pos] and children[pos+1] with separator from parent
    fn merge_children(&mut self, parent_idx: usize, pos: usize) {
        let left_idx = self.nodes[parent_idx].children[pos];
        let right_idx = self.nodes[parent_idx].children[pos + 1];

        // Pull separator down from parent
        let separator = self.nodes[parent_idx].entries.remove(pos);
        self.nodes[left_idx].entries.push(separator);

        // Move all entries from right into left
        let right_entries: Vec<BTreeEntry> = self.nodes[right_idx].entries.clone();
        self.nodes[left_idx].entries.extend(right_entries);

        // Move all children from right into left
        if !self.nodes[right_idx].is_leaf {
            let right_children: Vec<usize> = self.nodes[right_idx].children.clone();
            self.nodes[left_idx].children.extend(right_children);
        }

        // Remove right child pointer from parent
        self.nodes[parent_idx].children.remove(pos + 1);
    }

    /// Range scan: return all entries with key in [min_key, max_key] (inclusive)
    pub fn range_scan(&self, min_key: i64, max_key: i64) -> Vec<BTreeEntry> {
        let mut results = Vec::new();
        self.range_scan_node(self.root, min_key, max_key, &mut results);
        results
    }

    fn range_scan_node(
        &self,
        node_idx: usize,
        min_key: i64,
        max_key: i64,
        results: &mut Vec<BTreeEntry>,
    ) {
        let node = &self.nodes[node_idx];
        for (i, entry) in node.entries.iter().enumerate() {
            // Visit left subtree if keys could be in range
            if !node.is_leaf && entry.key >= min_key {
                self.range_scan_node(node.children[i], min_key, max_key, results);
            }
            // Collect this entry if in range
            if entry.key >= min_key && entry.key <= max_key {
                results.push(*entry);
            }
            // If we've passed max, no point continuing
            if entry.key > max_key {
                return;
            }
        }
        // Visit rightmost subtree
        if !node.is_leaf && !node.entries.is_empty() {
            let last_key = node.entries.last().unwrap().key;
            if last_key <= max_key {
                let last_child = *node.children.last().unwrap();
                self.range_scan_node(last_child, min_key, max_key, results);
            }
        }
    }

    /// In-order traversal: return all entries sorted by key
    pub fn in_order(&self) -> Vec<BTreeEntry> {
        let mut results = Vec::new();
        self.in_order_node(self.root, &mut results);
        results
    }

    fn in_order_node(&self, node_idx: usize, results: &mut Vec<BTreeEntry>) {
        let node = &self.nodes[node_idx];
        for (i, entry) in node.entries.iter().enumerate() {
            if !node.is_leaf {
                self.in_order_node(node.children[i], results);
            }
            results.push(*entry);
        }
        if !node.is_leaf && !node.children.is_empty() {
            let last_child = *node.children.last().unwrap();
            self.in_order_node(last_child, results);
        }
    }

    /// Return tree statistics: (entry_count, node_count, height)
    pub fn stats(&self) -> (u64, usize, u32) {
        (self.entry_count, self.nodes.len(), self.height)
    }

    /// Calculate the fill ratio of the tree as Q16 fixed-point
    pub fn fill_ratio_q16(&self) -> i32 {
        if self.nodes.is_empty() {
            return 0;
        }
        let total_slots = (self.nodes.len() as i64) * (MAX_KEYS as i64);
        if total_slots == 0 {
            return 0;
        }
        (((self.entry_count as i64) << 16) / total_slots) as i32
    }
}

/// Initialize the B-tree subsystem
pub fn init() {
    let mut guard = BTREE_POOL.lock();
    *guard = Some(BTreePool {
        trees: Vec::new(),
        next_tree_id: 0,
    });
    serial_println!("    B-tree index engine ready (order-64, range scan, rebalancing)");
}

/// Create a new B-tree index, returning its pool ID
pub fn create_index() -> u32 {
    let mut guard = BTREE_POOL.lock();
    if let Some(ref mut pool) = *guard {
        let id = pool.next_tree_id;
        pool.next_tree_id = pool.next_tree_id.saturating_add(1);
        pool.trees.push(BTree::new());
        id
    } else {
        0
    }
}

/// Insert into a pooled B-tree
pub fn pool_insert(tree_id: u32, key: i64, value: u64) {
    let mut guard = BTREE_POOL.lock();
    if let Some(ref mut pool) = *guard {
        if (tree_id as usize) < pool.trees.len() {
            pool.trees[tree_id as usize].insert(key, value);
        }
    }
}

/// Search in a pooled B-tree
pub fn pool_search(tree_id: u32, key: i64) -> Option<u64> {
    let guard = BTREE_POOL.lock();
    if let Some(ref pool) = *guard {
        if (tree_id as usize) < pool.trees.len() {
            return pool.trees[tree_id as usize].search(key);
        }
    }
    None
}

/// Range scan on a pooled B-tree
pub fn pool_range_scan(tree_id: u32, min_key: i64, max_key: i64) -> Vec<BTreeEntry> {
    let guard = BTREE_POOL.lock();
    if let Some(ref pool) = *guard {
        if (tree_id as usize) < pool.trees.len() {
            return pool.trees[tree_id as usize].range_scan(min_key, max_key);
        }
    }
    Vec::new()
}
