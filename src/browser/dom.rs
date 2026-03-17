use crate::sync::Mutex;
/// DOM tree implementation for Genesis browser
///
/// Provides a document object model with element creation, tree
/// manipulation, attribute management, querying, and a basic
/// event system (addEventListener / dispatchEvent).
use crate::{serial_print, serial_println};
use alloc::vec;
use alloc::vec::Vec;

static DOM_STATE: Mutex<Option<DomState>> = Mutex::new(None);

/// FNV-1a hash
fn dom_hash(s: &[u8]) -> u64 {
    let mut h: u64 = 0xCBF29CE484222325;
    for &b in s {
        h ^= b as u64;
        h = h.wrapping_mul(0x00000100000001B3);
    }
    h
}

/// Node type enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeType {
    Document,
    Element,
    Text,
    Comment,
}

/// A DOM attribute (name/value as byte vectors)
#[derive(Debug, Clone)]
pub struct DomAttribute {
    pub name_hash: u64,
    pub name: Vec<u8>,
    pub value: Vec<u8>,
}

/// Event listener callback identifier
#[derive(Debug, Clone, Copy)]
pub struct ListenerId(pub u32);

/// An event listener entry
#[derive(Debug, Clone)]
pub struct EventListener {
    pub event_hash: u64, // hash of event name (e.g. "click")
    pub listener_id: ListenerId,
    pub node_id: u32,
}

/// An event to dispatch
#[derive(Debug, Clone)]
pub struct DomEvent {
    pub event_hash: u64,
    pub target_id: u32,
    pub bubbles: bool,
    pub cancelled: bool,
}

/// Computed style stub (populated by css_engine + renderer)
#[derive(Debug, Clone)]
pub struct DomStyle {
    pub display: u8, // 0=block, 1=inline, 2=none
    pub color: u32,  // 0xAARRGGBB
    pub background: u32,
    pub font_size: i32, // Q16
    pub width: i32,     // Q16
    pub height: i32,    // Q16
}

impl DomStyle {
    pub fn default_style() -> Self {
        DomStyle {
            display: 0,
            color: 0xFF000000,
            background: 0xFFFFFFFF,
            font_size: 16 * 65536,
            width: 0,
            height: 0,
        }
    }
}

/// A DOM node
#[derive(Debug, Clone)]
pub struct DomNode {
    pub id: u32,
    pub node_type: NodeType,
    pub tag_hash: u64,
    pub tag: Vec<u8>,
    pub attributes: Vec<DomAttribute>,
    pub children: Vec<u32>, // child node IDs
    pub parent_id: Option<u32>,
    pub text: Vec<u8>,
    pub style: DomStyle,
}

/// Persistent DOM state: arena-allocated nodes
struct DomState {
    nodes: Vec<DomNode>,
    next_id: u32,
    listeners: Vec<EventListener>,
    next_listener_id: u32,
    dispatched_events: u64,
}

impl DomState {
    fn alloc_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id
    }

    fn find_node(&self, id: u32) -> Option<usize> {
        self.nodes.iter().position(|n| n.id == id)
    }
}

/// Create a document root node, returns its ID
pub fn create_document() -> u32 {
    let mut guard = DOM_STATE.lock();
    let state = guard.as_mut().unwrap();
    let id = state.alloc_id();
    state.nodes.push(DomNode {
        id,
        node_type: NodeType::Document,
        tag_hash: dom_hash(b"#document"),
        tag: b"#document".to_vec(),
        attributes: Vec::new(),
        children: Vec::new(),
        parent_id: None,
        text: Vec::new(),
        style: DomStyle::default_style(),
    });
    id
}

/// Create an element node, returns its ID
pub fn create_element(tag: &[u8]) -> u32 {
    let mut guard = DOM_STATE.lock();
    let state = guard.as_mut().unwrap();
    let id = state.alloc_id();
    state.nodes.push(DomNode {
        id,
        node_type: NodeType::Element,
        tag_hash: dom_hash(tag),
        tag: tag.to_vec(),
        attributes: Vec::new(),
        children: Vec::new(),
        parent_id: None,
        text: Vec::new(),
        style: DomStyle::default_style(),
    });
    id
}

/// Create a text node, returns its ID
pub fn create_text_node(text: &[u8]) -> u32 {
    let mut guard = DOM_STATE.lock();
    let state = guard.as_mut().unwrap();
    let id = state.alloc_id();
    state.nodes.push(DomNode {
        id,
        node_type: NodeType::Text,
        tag_hash: 0,
        tag: Vec::new(),
        attributes: Vec::new(),
        children: Vec::new(),
        parent_id: None,
        text: text.to_vec(),
        style: DomStyle::default_style(),
    });
    id
}

/// Create a comment node, returns its ID
pub fn create_comment(text: &[u8]) -> u32 {
    let mut guard = DOM_STATE.lock();
    let state = guard.as_mut().unwrap();
    let id = state.alloc_id();
    state.nodes.push(DomNode {
        id,
        node_type: NodeType::Comment,
        tag_hash: 0,
        tag: Vec::new(),
        attributes: Vec::new(),
        children: Vec::new(),
        parent_id: None,
        text: text.to_vec(),
        style: DomStyle::default_style(),
    });
    id
}

/// Append child_id to parent_id
pub fn append_child(parent_id: u32, child_id: u32) -> bool {
    let mut guard = DOM_STATE.lock();
    let state = guard.as_mut().unwrap();

    // Prevent appending a node to itself
    if parent_id == child_id {
        return false;
    }

    // Remove child from old parent if any
    let old_parent = state
        .find_node(child_id)
        .and_then(|idx| state.nodes[idx].parent_id);
    if let Some(old_pid) = old_parent {
        if let Some(old_idx) = state.find_node(old_pid) {
            state.nodes[old_idx].children.retain(|&c| c != child_id);
        }
    }

    // Set new parent on child
    if let Some(child_idx) = state.find_node(child_id) {
        state.nodes[child_idx].parent_id = Some(parent_id);
    } else {
        return false;
    }

    // Add child to parent's children list
    if let Some(parent_idx) = state.find_node(parent_id) {
        state.nodes[parent_idx].children.push(child_id);
        true
    } else {
        false
    }
}

/// Remove child_id from parent_id
pub fn remove_child(parent_id: u32, child_id: u32) -> bool {
    let mut guard = DOM_STATE.lock();
    let state = guard.as_mut().unwrap();

    if let Some(parent_idx) = state.find_node(parent_id) {
        let before = state.nodes[parent_idx].children.len();
        state.nodes[parent_idx].children.retain(|&c| c != child_id);
        let removed = state.nodes[parent_idx].children.len() < before;
        if removed {
            if let Some(child_idx) = state.find_node(child_id) {
                state.nodes[child_idx].parent_id = None;
            }
        }
        removed
    } else {
        false
    }
}

/// Set an attribute on an element node
pub fn set_attribute(node_id: u32, name: &[u8], value: &[u8]) -> bool {
    let mut guard = DOM_STATE.lock();
    let state = guard.as_mut().unwrap();
    if let Some(idx) = state.find_node(node_id) {
        let name_hash = dom_hash(name);
        // Update existing or add new
        for attr in state.nodes[idx].attributes.iter_mut() {
            if attr.name_hash == name_hash {
                attr.value = value.to_vec();
                return true;
            }
        }
        state.nodes[idx].attributes.push(DomAttribute {
            name_hash,
            name: name.to_vec(),
            value: value.to_vec(),
        });
        true
    } else {
        false
    }
}

/// Get an attribute value from a node
pub fn get_attribute(node_id: u32, name: &[u8]) -> Option<Vec<u8>> {
    let guard = DOM_STATE.lock();
    let state = guard.as_ref()?;
    let idx = state.find_node(node_id)?;
    let name_hash = dom_hash(name);
    for attr in &state.nodes[idx].attributes {
        if attr.name_hash == name_hash {
            return Some(attr.value.clone());
        }
    }
    None
}

/// Query selector by tag hash (simple single-tag selector)
pub fn query_selector(root_id: u32, tag: &[u8]) -> Option<u32> {
    let guard = DOM_STATE.lock();
    let state = guard.as_ref()?;
    let target_hash = dom_hash(tag);
    // BFS through the tree
    let mut queue: Vec<u32> = vec![root_id];
    while !queue.is_empty() {
        let current_id = queue.remove(0);
        if let Some(idx) = state.find_node(current_id) {
            let node = &state.nodes[idx];
            if node.tag_hash == target_hash && node.id != root_id {
                return Some(node.id);
            }
            for &child_id in &node.children {
                queue.push(child_id);
            }
        }
    }
    None
}

/// Get element by id attribute
pub fn get_by_id(root_id: u32, id_value: &[u8]) -> Option<u32> {
    let guard = DOM_STATE.lock();
    let state = guard.as_ref()?;
    let id_attr_hash = dom_hash(b"id");
    let mut queue: Vec<u32> = vec![root_id];
    while !queue.is_empty() {
        let current_id = queue.remove(0);
        if let Some(idx) = state.find_node(current_id) {
            let node = &state.nodes[idx];
            for attr in &node.attributes {
                if attr.name_hash == id_attr_hash && attr.value == id_value {
                    return Some(node.id);
                }
            }
            for &child_id in &node.children {
                queue.push(child_id);
            }
        }
    }
    None
}

/// Collect all text content from a subtree
pub fn get_text_content(node_id: u32) -> Vec<u8> {
    let guard = DOM_STATE.lock();
    let state = match guard.as_ref() {
        Some(s) => s,
        None => return Vec::new(),
    };
    let mut result = Vec::new();
    let mut stack: Vec<u32> = vec![node_id];
    while let Some(current_id) = stack.pop() {
        if let Some(idx) = state.find_node(current_id) {
            let node = &state.nodes[idx];
            if node.node_type == NodeType::Text {
                result.extend_from_slice(&node.text);
            }
            // Push children in reverse so leftmost is processed first
            for &child_id in node.children.iter().rev() {
                stack.push(child_id);
            }
        }
    }
    result
}

/// Add an event listener on a node for a given event name
pub fn add_event_listener(node_id: u32, event_name: &[u8]) -> ListenerId {
    let mut guard = DOM_STATE.lock();
    let state = guard.as_mut().unwrap();
    let lid = ListenerId(state.next_listener_id);
    state.next_listener_id = state.next_listener_id.saturating_add(1);
    state.listeners.push(EventListener {
        event_hash: dom_hash(event_name),
        listener_id: lid,
        node_id,
    });
    lid
}

/// Dispatch an event on a node; returns list of listener IDs that fired
pub fn dispatch_event(target_id: u32, event_name: &[u8], bubbles: bool) -> Vec<ListenerId> {
    let mut guard = DOM_STATE.lock();
    let state = guard.as_mut().unwrap();
    state.dispatched_events = state.dispatched_events.saturating_add(1);
    let event_hash = dom_hash(event_name);

    let mut fired = Vec::new();
    let mut current = Some(target_id);

    while let Some(nid) = current {
        for listener in &state.listeners {
            if listener.node_id == nid && listener.event_hash == event_hash {
                fired.push(listener.listener_id);
            }
        }
        if !bubbles {
            break;
        }
        // Walk up to parent
        current = state
            .find_node(nid)
            .and_then(|idx| state.nodes[idx].parent_id);
    }
    fired
}

/// Get a snapshot of a node (clone) by ID
pub fn get_node(node_id: u32) -> Option<DomNode> {
    let guard = DOM_STATE.lock();
    let state = guard.as_ref()?;
    state.find_node(node_id).map(|idx| state.nodes[idx].clone())
}

/// Get the total count of nodes in the DOM
pub fn node_count() -> usize {
    let guard = DOM_STATE.lock();
    match guard.as_ref() {
        Some(state) => state.nodes.len(),
        None => 0,
    }
}

pub fn init() {
    let mut guard = DOM_STATE.lock();
    *guard = Some(DomState {
        nodes: Vec::new(),
        next_id: 1,
        listeners: Vec::new(),
        next_listener_id: 1,
        dispatched_events: 0,
    });
    serial_println!("    browser::dom initialized");
}
