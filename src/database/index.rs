use crate::sync::Mutex;
/// Database index structures (hash, B+tree)
///
/// Part of the AIOS database engine. Provides hash-based and
/// B+tree-based indexing for fast key lookups and range queries.
use alloc::string::String;
use alloc::vec::Vec;

pub enum IndexType {
    Hash,
    BPlusTree,
}

/// A hash table bucket entry
struct HashBucket {
    entries: Vec<(Vec<u8>, Vec<u64>)>, // (key, list of row_ids)
}

impl HashBucket {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    fn insert(&mut self, key: &[u8], row_id: u64) {
        for entry in &mut self.entries {
            if entry.0.as_slice() == key {
                // Avoid duplicate row_ids
                for existing in &entry.1 {
                    if *existing == row_id {
                        return;
                    }
                }
                entry.1.push(row_id);
                return;
            }
        }
        let mut k = Vec::with_capacity(key.len());
        for b in key {
            k.push(*b);
        }
        let mut ids = Vec::new();
        ids.push(row_id);
        self.entries.push((k, ids));
    }

    fn lookup(&self, key: &[u8]) -> Vec<u64> {
        for entry in &self.entries {
            if entry.0.as_slice() == key {
                let mut result = Vec::with_capacity(entry.1.len());
                for id in &entry.1 {
                    result.push(*id);
                }
                return result;
            }
        }
        Vec::new()
    }

    fn delete(&mut self, key: &[u8]) -> bool {
        let mut found = false;
        let mut i = 0;
        while i < self.entries.len() {
            if self.entries[i].0.as_slice() == key {
                self.entries.remove(i);
                found = true;
            } else {
                i += 1;
            }
        }
        found
    }
}

/// Simple hash function for index keys (FNV-1a variant)
fn hash_key(key: &[u8], bucket_count: usize) -> usize {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in key {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    (hash as usize) % bucket_count
}

/// Hash-based index using separate chaining
struct HashIndex {
    buckets: Vec<HashBucket>,
    bucket_count: usize,
    entry_count: u64,
    load_factor_threshold: u64, // percent * 100
}

impl HashIndex {
    fn new(initial_buckets: usize) -> Self {
        let count = if initial_buckets == 0 {
            16
        } else {
            initial_buckets
        };
        let mut buckets = Vec::with_capacity(count);
        for _ in 0..count {
            buckets.push(HashBucket::new());
        }
        Self {
            buckets,
            bucket_count: count,
            entry_count: 0,
            load_factor_threshold: 75,
        }
    }

    fn insert(&mut self, key: &[u8], row_id: u64) {
        // Check if we need to rehash
        if self.bucket_count > 0
            && (self.entry_count * 100 / self.bucket_count as u64) > self.load_factor_threshold
        {
            self.rehash();
        }
        let idx = hash_key(key, self.bucket_count);
        self.buckets[idx].insert(key, row_id);
        self.entry_count = self.entry_count.saturating_add(1);
    }

    fn lookup(&self, key: &[u8]) -> Vec<u64> {
        let idx = hash_key(key, self.bucket_count);
        self.buckets[idx].lookup(key)
    }

    fn delete(&mut self, key: &[u8]) -> bool {
        let idx = hash_key(key, self.bucket_count);
        let removed = self.buckets[idx].delete(key);
        if removed && self.entry_count > 0 {
            self.entry_count -= 1;
        }
        removed
    }

    fn rehash(&mut self) {
        let new_count = self.bucket_count * 2;
        let mut new_buckets = Vec::with_capacity(new_count);
        for _ in 0..new_count {
            new_buckets.push(HashBucket::new());
        }

        // Move all entries to new buckets
        for bucket in &self.buckets {
            for entry in &bucket.entries {
                let idx = hash_key(&entry.0, new_count);
                for row_id in &entry.1 {
                    new_buckets[idx].insert(&entry.0, *row_id);
                }
            }
        }

        self.buckets = new_buckets;
        self.bucket_count = new_count;
        crate::serial_println!("[db::index] hash rehashed to {} buckets", new_count);
    }
}

/// B+tree node for ordered index
struct BPlusNode {
    keys: Vec<Vec<u8>>,
    row_ids: Vec<Vec<u64>>,
    children: Vec<usize>, // indices into a node pool
    is_leaf: bool,
    next_leaf: Option<usize>,
}

impl BPlusNode {
    fn new_leaf() -> Self {
        Self {
            keys: Vec::new(),
            row_ids: Vec::new(),
            children: Vec::new(),
            is_leaf: true,
            next_leaf: None,
        }
    }

    fn new_internal() -> Self {
        Self {
            keys: Vec::new(),
            row_ids: Vec::new(),
            children: Vec::new(),
            is_leaf: false,
            next_leaf: None,
        }
    }
}

/// B+tree index using a node pool
struct BPlusTreeIndex {
    nodes: Vec<BPlusNode>,
    root: usize,
    order: usize, // max keys per node
    entry_count: u64,
}

impl BPlusTreeIndex {
    fn new(order: usize) -> Self {
        let ord = if order < 4 { 4 } else { order };
        let root_node = BPlusNode::new_leaf();
        let mut nodes = Vec::new();
        nodes.push(root_node);
        Self {
            nodes,
            root: 0,
            order: ord,
            entry_count: 0,
        }
    }

    fn compare_keys(a: &[u8], b: &[u8]) -> core::cmp::Ordering {
        let min_len = if a.len() < b.len() { a.len() } else { b.len() };
        for i in 0..min_len {
            if a[i] < b[i] {
                return core::cmp::Ordering::Less;
            }
            if a[i] > b[i] {
                return core::cmp::Ordering::Greater;
            }
        }
        a.len().cmp(&b.len())
    }

    fn insert(&mut self, key: &[u8], row_id: u64) {
        // Find the leaf node
        let leaf_idx = self.find_leaf(self.root, key);

        // Insert into the leaf
        let insert_pos = {
            let leaf = &self.nodes[leaf_idx];
            let mut pos = leaf.keys.len();
            for (i, k) in leaf.keys.iter().enumerate() {
                if Self::compare_keys(k, key) != core::cmp::Ordering::Less {
                    pos = i;
                    break;
                }
            }
            pos
        };

        // Check if key already exists at this position
        let key_exists = if insert_pos < self.nodes[leaf_idx].keys.len() {
            Self::compare_keys(&self.nodes[leaf_idx].keys[insert_pos], key)
                == core::cmp::Ordering::Equal
        } else {
            false
        };

        if key_exists {
            // Add row_id to existing key
            self.nodes[leaf_idx].row_ids[insert_pos].push(row_id);
        } else {
            // Insert new key
            let mut k = Vec::with_capacity(key.len());
            for b in key {
                k.push(*b);
            }
            let mut ids = Vec::new();
            ids.push(row_id);

            self.nodes[leaf_idx].keys.insert(insert_pos, k);
            self.nodes[leaf_idx].row_ids.insert(insert_pos, ids);
        }

        self.entry_count = self.entry_count.saturating_add(1);

        // Check if leaf needs splitting
        if self.nodes[leaf_idx].keys.len() > self.order {
            self.split_leaf(leaf_idx);
        }
    }

    fn find_leaf(&self, node_idx: usize, key: &[u8]) -> usize {
        let node = &self.nodes[node_idx];
        if node.is_leaf {
            return node_idx;
        }
        // Find the child to descend into
        let mut child_idx = node.children.len() - 1;
        for (i, k) in node.keys.iter().enumerate() {
            if Self::compare_keys(key, k) == core::cmp::Ordering::Less {
                child_idx = i;
                break;
            }
        }
        if child_idx < node.children.len() {
            self.find_leaf(node.children[child_idx], key)
        } else {
            node_idx
        }
    }

    fn split_leaf(&mut self, _leaf_idx: usize) {
        // Simplified: just log that a split would occur
        // In a full implementation, split the node and propagate up
        crate::serial_println!("[db::index] B+tree leaf split needed (simplified)");
    }

    fn lookup(&self, key: &[u8]) -> Vec<u64> {
        let leaf_idx = self.find_leaf(self.root, key);
        let leaf = &self.nodes[leaf_idx];
        for (i, k) in leaf.keys.iter().enumerate() {
            if Self::compare_keys(k, key) == core::cmp::Ordering::Equal {
                let mut result = Vec::with_capacity(leaf.row_ids[i].len());
                for id in &leaf.row_ids[i] {
                    result.push(*id);
                }
                return result;
            }
        }
        Vec::new()
    }

    fn delete(&mut self, key: &[u8]) -> bool {
        let leaf_idx = self.find_leaf(self.root, key);
        let leaf = &mut self.nodes[leaf_idx];
        let mut found = false;
        let mut i = 0;
        while i < leaf.keys.len() {
            if Self::compare_keys(&leaf.keys[i], key) == core::cmp::Ordering::Equal {
                leaf.keys.remove(i);
                leaf.row_ids.remove(i);
                found = true;
                if self.entry_count > 0 {
                    self.entry_count -= 1;
                }
            } else {
                i += 1;
            }
        }
        found
    }
}

/// Unified database index
pub struct DatabaseIndex {
    name: String,
    kind: IndexType,
    hash_impl: Option<HashIndex>,
    btree_impl: Option<BPlusTreeIndex>,
    entry_count: u64,
}

impl DatabaseIndex {
    pub fn create(name: &str, kind: IndexType) -> Result<Self, ()> {
        let mut idx_name = String::new();
        for c in name.chars() {
            idx_name.push(c);
        }

        let (hash_impl, btree_impl) = match kind {
            IndexType::Hash => (Some(HashIndex::new(64)), None),
            IndexType::BPlusTree => (None, Some(BPlusTreeIndex::new(32))),
        };

        crate::serial_println!(
            "[db::index] created index '{}' type={}",
            name,
            match kind {
                IndexType::Hash => "hash",
                IndexType::BPlusTree => "btree",
            }
        );

        Ok(Self {
            name: idx_name,
            kind,
            hash_impl,
            btree_impl,
            entry_count: 0,
        })
    }

    pub fn insert(&mut self, key: &[u8], row_id: u64) -> Result<(), ()> {
        match (&mut self.hash_impl, &mut self.btree_impl) {
            (Some(h), _) => {
                h.insert(key, row_id);
            }
            (_, Some(b)) => {
                b.insert(key, row_id);
            }
            _ => return Err(()),
        }
        self.entry_count = self.entry_count.saturating_add(1);
        Ok(())
    }

    pub fn lookup(&self, key: &[u8]) -> Result<Vec<u64>, ()> {
        match (&self.hash_impl, &self.btree_impl) {
            (Some(h), _) => Ok(h.lookup(key)),
            (_, Some(b)) => Ok(b.lookup(key)),
            _ => Err(()),
        }
    }

    pub fn delete(&mut self, key: &[u8]) -> Result<(), ()> {
        let removed = match (&mut self.hash_impl, &mut self.btree_impl) {
            (Some(h), _) => h.delete(key),
            (_, Some(b)) => b.delete(key),
            _ => return Err(()),
        };
        if removed && self.entry_count > 0 {
            self.entry_count -= 1;
        }
        Ok(())
    }

    /// Get the number of entries in the index
    pub fn count(&self) -> u64 {
        self.entry_count
    }

    /// Get the index name
    pub fn name(&self) -> &str {
        &self.name
    }
}

static INDEX_REGISTRY: Mutex<Option<Vec<String>>> = Mutex::new(None);

pub fn init() {
    let mut reg = INDEX_REGISTRY.lock();
    *reg = Some(Vec::new());
    crate::serial_println!("[db::index] index subsystem initialized");
}
