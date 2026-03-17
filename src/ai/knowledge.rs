/// Knowledge graph for Genesis
///
/// On-device knowledge representation: entities, relations,
/// graph traversal, inference, and contextual understanding.
///
/// Features:
///   - Entity storage with typed relations (is-a, has-a, part-of, etc.)
///   - Triple storage: (subject, predicate, object)
///   - Transitive inference: if A is-a B and B has-a C, then A has-a C
///   - Path finding between entities (BFS with path tracking)
///   - Query by type, by relation, by property
///
/// Inspired by: Google Knowledge Graph, Wikidata. All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// Entity type in knowledge graph
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnowledgeType {
    Person,
    Place,
    Organization,
    Event,
    Concept,
    Device,
    App,
    File,
    Setting,
    Contact,
    CalendarEntry,
    Custom,
}

/// Well-known relation types for typed inference
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationType {
    IsA,       // A is-a B (inheritance)
    HasA,      // A has-a B (composition)
    PartOf,    // A part-of B (inverse of has-a)
    CreatedBy, // A created-by B
    Created,   // A created B
    UsedBy,    // A used-by B
    Contains,  // A contains B
    RelatedTo, // Generic bidirectional relation
    DependsOn, // A depends-on B
    Custom,    // Free-form string relation
}

/// A knowledge entity
pub struct Entity {
    pub id: u32,
    pub name: String,
    pub entity_type: KnowledgeType,
    pub properties: BTreeMap<String, String>,
    pub aliases: Vec<String>,
    pub created_at: u64,
    pub last_accessed: u64,
    pub access_count: u32,
}

/// Relation between entities
pub struct Relation {
    pub from_id: u32,
    pub to_id: u32,
    pub relation_type: String,
    pub typed_relation: RelationType,
    pub weight: f32,
    pub bidirectional: bool,
    pub inferred: bool,
}

/// A triple: (subject, predicate, object) — the fundamental knowledge unit
pub struct Triple {
    pub subject: u32, // entity ID
    pub predicate: String,
    pub object: u32, // entity ID
}

/// Query result from knowledge graph
pub struct QueryResult {
    pub entity: u32,
    pub relevance: f32,
    pub path: Vec<u32>,
}

/// Knowledge graph
pub struct KnowledgeGraph {
    pub entities: Vec<Entity>,
    pub relations: Vec<Relation>,
    pub triples: Vec<Triple>,
    pub next_id: u32,
    pub max_entities: usize,
    pub learning_enabled: bool,
}

impl KnowledgeGraph {
    const fn new() -> Self {
        KnowledgeGraph {
            entities: Vec::new(),
            relations: Vec::new(),
            triples: Vec::new(),
            next_id: 1,
            max_entities: 100000,
            learning_enabled: true,
        }
    }

    pub fn add_entity(&mut self, name: &str, etype: KnowledgeType) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let now = crate::time::clock::unix_time();
        self.entities.push(Entity {
            id,
            name: String::from(name),
            entity_type: etype,
            properties: BTreeMap::new(),
            aliases: Vec::new(),
            created_at: now,
            last_accessed: now,
            access_count: 0,
        });
        id
    }

    pub fn set_property(&mut self, entity_id: u32, key: &str, value: &str) {
        if let Some(entity) = self.entities.iter_mut().find(|e| e.id == entity_id) {
            entity
                .properties
                .insert(String::from(key), String::from(value));
        }
    }

    pub fn get_property(&self, entity_id: u32, key: &str) -> Option<String> {
        self.entities
            .iter()
            .find(|e| e.id == entity_id)
            .and_then(|e| e.properties.get(key).cloned())
    }

    pub fn add_alias(&mut self, entity_id: u32, alias: &str) {
        if let Some(entity) = self.entities.iter_mut().find(|e| e.id == entity_id) {
            entity.aliases.push(String::from(alias));
        }
    }

    /// Add a typed, directional relation
    pub fn add_relation(&mut self, from: u32, to: u32, rel_type: &str, weight: f32) {
        let typed = classify_relation(rel_type);
        self.relations.push(Relation {
            from_id: from,
            to_id: to,
            relation_type: String::from(rel_type),
            typed_relation: typed,
            weight,
            bidirectional: false,
            inferred: false,
        });
        // Also store as a triple
        self.triples.push(Triple {
            subject: from,
            predicate: String::from(rel_type),
            object: to,
        });
    }

    pub fn add_bidirectional(&mut self, a: u32, b: u32, rel_type: &str, weight: f32) {
        let typed = classify_relation(rel_type);
        self.relations.push(Relation {
            from_id: a,
            to_id: b,
            relation_type: String::from(rel_type),
            typed_relation: typed,
            weight,
            bidirectional: true,
            inferred: false,
        });
        self.triples.push(Triple {
            subject: a,
            predicate: String::from(rel_type),
            object: b,
        });
        self.triples.push(Triple {
            subject: b,
            predicate: String::from(rel_type),
            object: a,
        });
    }

    /// Insert a raw triple and auto-create the corresponding relation
    pub fn add_triple(&mut self, subject: u32, predicate: &str, object: u32) {
        self.triples.push(Triple {
            subject,
            predicate: String::from(predicate),
            object,
        });
        let typed = classify_relation(predicate);
        self.relations.push(Relation {
            from_id: subject,
            to_id: object,
            relation_type: String::from(predicate),
            typed_relation: typed,
            weight: 1.0,
            bidirectional: false,
            inferred: false,
        });
    }

    /// Query triples: find all triples matching optional subject/predicate/object filters
    pub fn query_triples(
        &self,
        subject: Option<u32>,
        predicate: Option<&str>,
        object: Option<u32>,
    ) -> Vec<&Triple> {
        self.triples
            .iter()
            .filter(|t| {
                let match_s = subject.map_or(true, |s| t.subject == s);
                let match_p = predicate.map_or(true, |p| t.predicate == p);
                let match_o = object.map_or(true, |o| t.object == o);
                match_s && match_p && match_o
            })
            .collect()
    }

    /// Find entity by name or alias
    pub fn find_by_name(&mut self, name: &str) -> Option<u32> {
        let lower = name.to_lowercase();
        for entity in &mut self.entities {
            if entity.name.to_lowercase() == lower {
                entity.last_accessed = crate::time::clock::unix_time();
                entity.access_count += 1;
                return Some(entity.id);
            }
            for alias in &entity.aliases {
                if alias.to_lowercase() == lower {
                    entity.last_accessed = crate::time::clock::unix_time();
                    entity.access_count += 1;
                    return Some(entity.id);
                }
            }
        }
        None
    }

    /// Get all related entities (follows both directed and bidirectional edges)
    pub fn get_related(&self, entity_id: u32) -> Vec<(u32, String, f32)> {
        let mut related = Vec::new();
        for rel in &self.relations {
            if rel.from_id == entity_id {
                related.push((rel.to_id, rel.relation_type.clone(), rel.weight));
            }
            if rel.bidirectional && rel.to_id == entity_id {
                related.push((rel.from_id, rel.relation_type.clone(), rel.weight));
            }
        }
        related.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(core::cmp::Ordering::Equal));
        related
    }

    /// Get related entities filtered by relation type
    pub fn get_related_by_type(&self, entity_id: u32, rel_type: RelationType) -> Vec<(u32, f32)> {
        let mut results = Vec::new();
        for rel in &self.relations {
            if rel.typed_relation == rel_type {
                if rel.from_id == entity_id {
                    results.push((rel.to_id, rel.weight));
                }
                if rel.bidirectional && rel.to_id == entity_id {
                    results.push((rel.from_id, rel.weight));
                }
            }
        }
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));
        results
    }

    /// Get entity by ID
    pub fn get_entity(&self, id: u32) -> Option<&Entity> {
        self.entities.iter().find(|e| e.id == id)
    }

    /// Search entities by type
    pub fn find_by_type(&self, etype: KnowledgeType) -> Vec<&Entity> {
        self.entities
            .iter()
            .filter(|e| e.entity_type == etype)
            .collect()
    }

    /// Search entities by property value (partial match)
    pub fn find_by_property(&self, key: &str, value: &str) -> Vec<u32> {
        let lower_val = value.to_lowercase();
        self.entities
            .iter()
            .filter(|e| {
                e.properties
                    .get(key)
                    .map_or(false, |v| v.to_lowercase().contains(&lower_val))
            })
            .map(|e| e.id)
            .collect()
    }

    /// Traverse graph from an entity (BFS, max depth)
    pub fn traverse(&self, start: u32, max_depth: u32) -> Vec<QueryResult> {
        let mut visited = Vec::new();
        let mut queue: Vec<(u32, u32, Vec<u32>)> = alloc::vec![(start, 0, alloc::vec![start])];
        let mut results = Vec::new();

        while let Some((current, depth, path)) = queue.first().cloned() {
            queue.remove(0);
            if visited.contains(&current) {
                continue;
            }
            visited.push(current);

            if current != start {
                results.push(QueryResult {
                    entity: current,
                    relevance: 1.0 / (depth as f32 + 1.0),
                    path: path.clone(),
                });
            }

            if depth < max_depth {
                for (related_id, _, _) in self.get_related(current) {
                    if !visited.contains(&related_id) {
                        let mut new_path = path.clone();
                        new_path.push(related_id);
                        queue.push((related_id, depth + 1, new_path));
                    }
                }
            }
        }
        results
    }

    /// Find the shortest path between two entities using BFS.
    /// Returns None if no path exists.
    pub fn find_path(&self, from: u32, to: u32, max_depth: u32) -> Option<Vec<u32>> {
        if from == to {
            return Some(alloc::vec![from]);
        }

        let mut visited = Vec::new();
        let mut queue: Vec<(u32, Vec<u32>)> = alloc::vec![(from, alloc::vec![from])];

        while let Some((current, path)) = queue.first().cloned() {
            queue.remove(0);

            if visited.contains(&current) {
                continue;
            }
            visited.push(current);

            if path.len() as u32 > max_depth + 1 {
                continue;
            }

            for (neighbor, _, _) in self.get_related(current) {
                if neighbor == to {
                    let mut full_path = path.clone();
                    full_path.push(to);
                    return Some(full_path);
                }
                if !visited.contains(&neighbor) {
                    let mut new_path = path.clone();
                    new_path.push(neighbor);
                    queue.push((neighbor, new_path));
                }
            }
        }
        None
    }

    /// Infer new relations using typed inference rules:
    ///
    /// 1. Transitive same-type: if A->B and B->C with same relation, infer A->C
    /// 2. IsA inheritance: if A is-a B and B has-a C, then A has-a C
    /// 3. PartOf transitivity: if A part-of B and B part-of C, then A part-of C
    /// 4. Contains inverse: if A contains B, then B part-of A
    pub fn infer_relations(&mut self) -> u32 {
        if !self.learning_enabled {
            return 0;
        }
        let mut new_relations: Vec<Relation> = Vec::new();
        let mut new_triples: Vec<Triple> = Vec::new();

        let rel_count = self.relations.len();

        // Rule 1: Transitive same-type inference
        for i in 0..rel_count {
            for j in 0..rel_count {
                if self.relations[i].to_id == self.relations[j].from_id
                    && self.relations[i].relation_type == self.relations[j].relation_type
                    && self.relations[i].from_id != self.relations[j].to_id
                {
                    let from = self.relations[i].from_id;
                    let to = self.relations[j].to_id;
                    let rel_type = &self.relations[i].relation_type;
                    if !self.relation_exists(from, to, rel_type)
                        && !new_relations.iter().any(|r| {
                            r.from_id == from && r.to_id == to && r.relation_type == *rel_type
                        })
                    {
                        let weight = self.relations[i].weight * self.relations[j].weight * 0.5;
                        new_relations.push(Relation {
                            from_id: from,
                            to_id: to,
                            relation_type: rel_type.clone(),
                            typed_relation: self.relations[i].typed_relation,
                            weight,
                            bidirectional: false,
                            inferred: true,
                        });
                        new_triples.push(Triple {
                            subject: from,
                            predicate: rel_type.clone(),
                            object: to,
                        });
                    }
                }
            }
        }

        // Rule 2: IsA inheritance — if A is-a B and B has-a C, then A has-a C
        for i in 0..rel_count {
            if self.relations[i].typed_relation != RelationType::IsA {
                continue;
            }
            let a = self.relations[i].from_id;
            let b = self.relations[i].to_id;
            for j in 0..rel_count {
                if self.relations[j].from_id != b {
                    continue;
                }
                if self.relations[j].typed_relation != RelationType::HasA {
                    continue;
                }
                let c = self.relations[j].to_id;
                if a == c {
                    continue;
                }
                let rel_str = "has_a";
                if !self.relation_exists(a, c, rel_str)
                    && !new_relations
                        .iter()
                        .any(|r| r.from_id == a && r.to_id == c && r.relation_type == rel_str)
                {
                    let weight = self.relations[i].weight * self.relations[j].weight * 0.4;
                    new_relations.push(Relation {
                        from_id: a,
                        to_id: c,
                        relation_type: String::from(rel_str),
                        typed_relation: RelationType::HasA,
                        weight,
                        bidirectional: false,
                        inferred: true,
                    });
                    new_triples.push(Triple {
                        subject: a,
                        predicate: String::from(rel_str),
                        object: c,
                    });
                }
            }
        }

        // Rule 3: PartOf transitivity — if A part-of B and B part-of C, then A part-of C
        for i in 0..rel_count {
            if self.relations[i].typed_relation != RelationType::PartOf {
                continue;
            }
            let a = self.relations[i].from_id;
            let b = self.relations[i].to_id;
            for j in 0..rel_count {
                if self.relations[j].from_id != b {
                    continue;
                }
                if self.relations[j].typed_relation != RelationType::PartOf {
                    continue;
                }
                let c = self.relations[j].to_id;
                if a == c {
                    continue;
                }
                let rel_str = "part_of";
                if !self.relation_exists(a, c, rel_str)
                    && !new_relations
                        .iter()
                        .any(|r| r.from_id == a && r.to_id == c && r.relation_type == rel_str)
                {
                    let weight = self.relations[i].weight * self.relations[j].weight * 0.4;
                    new_relations.push(Relation {
                        from_id: a,
                        to_id: c,
                        relation_type: String::from(rel_str),
                        typed_relation: RelationType::PartOf,
                        weight,
                        bidirectional: false,
                        inferred: true,
                    });
                    new_triples.push(Triple {
                        subject: a,
                        predicate: String::from(rel_str),
                        object: c,
                    });
                }
            }
        }

        // Rule 4: Contains inverse — if A contains B, then B part-of A
        for i in 0..rel_count {
            if self.relations[i].typed_relation != RelationType::Contains {
                continue;
            }
            let a = self.relations[i].from_id;
            let b = self.relations[i].to_id;
            let rel_str = "part_of";
            if !self.relation_exists(b, a, rel_str)
                && !new_relations
                    .iter()
                    .any(|r| r.from_id == b && r.to_id == a && r.relation_type == rel_str)
            {
                new_relations.push(Relation {
                    from_id: b,
                    to_id: a,
                    relation_type: String::from(rel_str),
                    typed_relation: RelationType::PartOf,
                    weight: self.relations[i].weight * 0.9,
                    bidirectional: false,
                    inferred: true,
                });
                new_triples.push(Triple {
                    subject: b,
                    predicate: String::from(rel_str),
                    object: a,
                });
            }
        }

        let count = new_relations.len() as u32;
        self.relations.extend(new_relations);
        self.triples.extend(new_triples);
        count
    }

    /// Check whether a specific relation already exists
    fn relation_exists(&self, from: u32, to: u32, rel_type: &str) -> bool {
        self.relations
            .iter()
            .any(|r| r.from_id == from && r.to_id == to && r.relation_type == rel_type)
    }

    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }
    pub fn relation_count(&self) -> usize {
        self.relations.len()
    }
    pub fn triple_count(&self) -> usize {
        self.triples.len()
    }

    /// Count of inferred (non-explicit) relations
    pub fn inferred_count(&self) -> usize {
        self.relations.iter().filter(|r| r.inferred).count()
    }
}

/// Classify a free-form relation string into a typed RelationType
fn classify_relation(rel_type: &str) -> RelationType {
    let lower = rel_type.to_lowercase();
    let lower = lower.as_str();
    if lower == "is_a"
        || lower == "is-a"
        || lower == "isa"
        || lower == "type_of"
        || lower == "instance_of"
    {
        RelationType::IsA
    } else if lower == "has_a"
        || lower == "has-a"
        || lower == "hasa"
        || lower == "has_component"
        || lower == "has_part"
    {
        RelationType::HasA
    } else if lower == "part_of" || lower == "part-of" || lower == "partof" || lower == "belongs_to"
    {
        RelationType::PartOf
    } else if lower == "created_by" || lower == "made_by" || lower == "authored_by" {
        RelationType::CreatedBy
    } else if lower == "created" || lower == "made" || lower == "authored" || lower == "built" {
        RelationType::Created
    } else if lower == "used_by" || lower == "utilized_by" {
        RelationType::UsedBy
    } else if lower == "contains" || lower == "includes" || lower == "holds" {
        RelationType::Contains
    } else if lower == "related_to" || lower == "associated_with" || lower == "linked_to" {
        RelationType::RelatedTo
    } else if lower == "depends_on" || lower == "requires" || lower == "needs" {
        RelationType::DependsOn
    } else {
        RelationType::Custom
    }
}

/// Seed the knowledge graph with system knowledge
fn seed_system_knowledge(kg: &mut KnowledgeGraph) {
    let os = kg.add_entity("Hoags OS", KnowledgeType::Concept);
    kg.set_property(os, "version", "1.0.0");
    kg.set_property(os, "arch", "x86_64");

    let kernel = kg.add_entity("Genesis", KnowledgeType::Concept);
    kg.set_property(kernel, "type", "kernel");
    kg.add_relation(os, kernel, "has_component", 1.0);

    let hoags = kg.add_entity("Hoags Inc", KnowledgeType::Organization);
    kg.add_relation(hoags, os, "created", 1.0);

    let fs = kg.add_entity("HoagsFS", KnowledgeType::Concept);
    kg.set_property(fs, "type", "filesystem");
    kg.add_relation(os, fs, "has_component", 0.9);

    let ai = kg.add_entity("Hoags AI", KnowledgeType::Concept);
    kg.set_property(ai, "type", "ai_assistant");
    kg.add_relation(os, ai, "has_component", 0.9);
    kg.add_alias(ai, "assistant");

    let shell = kg.add_entity("Hoags Shell", KnowledgeType::App);
    kg.add_relation(os, shell, "has_component", 0.8);

    // Type hierarchy: kernel is-a software, AI is-a software
    let software = kg.add_entity("Software", KnowledgeType::Concept);
    kg.add_relation(kernel, software, "is_a", 1.0);
    kg.add_relation(ai, software, "is_a", 1.0);
    kg.add_relation(fs, software, "is_a", 1.0);
    kg.add_relation(shell, software, "is_a", 1.0);

    // Part-of relations
    kg.add_relation(kernel, os, "part_of", 1.0);
    kg.add_relation(fs, os, "part_of", 0.9);
    kg.add_relation(ai, os, "part_of", 0.9);
    kg.add_relation(shell, os, "part_of", 0.8);
}

static KNOWLEDGE: Mutex<KnowledgeGraph> = Mutex::new(KnowledgeGraph::new());

pub fn init() {
    let mut kg = KNOWLEDGE.lock();
    seed_system_knowledge(&mut kg);
    let inferred = kg.infer_relations();
    crate::serial_println!(
        "    [knowledge] Knowledge graph initialized ({} entities, {} relations, {} triples, {} inferred)",
        kg.entity_count(), kg.relation_count(), kg.triple_count(), inferred
    );
}

/// Add an entity to the global knowledge graph
pub fn add_entity(name: &str, etype: KnowledgeType) -> u32 {
    KNOWLEDGE.lock().add_entity(name, etype)
}

/// Add a relation to the global knowledge graph
pub fn add_relation(from: u32, to: u32, rel_type: &str, weight: f32) {
    KNOWLEDGE.lock().add_relation(from, to, rel_type, weight);
}

/// Add a triple to the global knowledge graph
pub fn add_triple(subject: u32, predicate: &str, object: u32) {
    KNOWLEDGE.lock().add_triple(subject, predicate, object);
}

/// Query triples from the global knowledge graph
pub fn query_triples(
    subject: Option<u32>,
    predicate: Option<&str>,
    object: Option<u32>,
) -> Vec<(u32, String, u32)> {
    KNOWLEDGE
        .lock()
        .query_triples(subject, predicate, object)
        .iter()
        .map(|t| (t.subject, t.predicate.clone(), t.object))
        .collect()
}

/// Find entity by name
pub fn find_by_name(name: &str) -> Option<u32> {
    KNOWLEDGE.lock().find_by_name(name)
}

/// Find shortest path between two entities
pub fn find_path(from: u32, to: u32) -> Option<Vec<u32>> {
    KNOWLEDGE.lock().find_path(from, to, 10)
}

/// Run inference to discover new relations
pub fn infer() -> u32 {
    KNOWLEDGE.lock().infer_relations()
}
