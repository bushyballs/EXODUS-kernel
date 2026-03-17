/// Screen reader for Genesis — TalkBack/VoiceOver equivalent
///
/// Reads UI elements aloud, navigation by focus, gestures,
/// content descriptions, and accessibility tree traversal.
///
/// Inspired by: TalkBack, VoiceOver, Orca. All code is original.
use crate::sync::Mutex;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Accessibility node type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeRole {
    Button,
    Text,
    EditField,
    CheckBox,
    RadioButton,
    Slider,
    Link,
    Image,
    List,
    ListItem,
    Heading,
    Dialog,
    Tab,
    Menu,
    MenuItem,
    ProgressBar,
    Switch,
    Window,
    Container,
}

/// An accessibility tree node
pub struct AccessibilityNode {
    pub id: u32,
    pub role: NodeRole,
    pub label: String,
    pub description: String,
    pub value: String,
    pub focusable: bool,
    pub focused: bool,
    pub enabled: bool,
    pub checked: Option<bool>,
    pub children: Vec<u32>,
    pub parent: Option<u32>,
    pub bounds: (i32, i32, i32, i32), // x, y, w, h
}

/// Screen reader state
pub struct ScreenReader {
    pub enabled: bool,
    pub nodes: Vec<AccessibilityNode>,
    pub focus_index: usize,
    pub speech_rate: u8,  // 1-10
    pub speech_pitch: u8, // 1-10
    pub verbosity: Verbosity,
    pub explore_by_touch: bool,
    pub announcement_queue: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verbosity {
    Low,
    Medium,
    High,
}

impl ScreenReader {
    const fn new() -> Self {
        ScreenReader {
            enabled: false,
            nodes: Vec::new(),
            focus_index: 0,
            speech_rate: 5,
            speech_pitch: 5,
            verbosity: Verbosity::Medium,
            explore_by_touch: true,
            announcement_queue: Vec::new(),
        }
    }

    pub fn enable(&mut self) {
        self.enabled = true;
        self.announce("Screen reader enabled");
    }

    pub fn disable(&mut self) {
        self.enabled = false;
        self.announcement_queue.clear();
    }

    pub fn announce(&mut self, text: &str) {
        if self.enabled {
            self.announcement_queue.push(String::from(text));
        }
    }

    pub fn next_focus(&mut self) {
        if self.nodes.is_empty() {
            return;
        }
        let focusable: Vec<usize> = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.focusable && n.enabled)
            .map(|(i, _)| i)
            .collect();
        if focusable.is_empty() {
            return;
        }

        let current_pos = focusable.iter().position(|&i| i == self.focus_index);
        let next = match current_pos {
            Some(pos) => focusable[(pos + 1) % focusable.len()],
            None => focusable[0],
        };
        self.set_focus(next);
    }

    pub fn prev_focus(&mut self) {
        if self.nodes.is_empty() {
            return;
        }
        let focusable: Vec<usize> = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.focusable && n.enabled)
            .map(|(i, _)| i)
            .collect();
        if focusable.is_empty() {
            return;
        }

        let current_pos = focusable.iter().position(|&i| i == self.focus_index);
        let prev = match current_pos {
            Some(0) => focusable[focusable.len() - 1],
            Some(pos) => focusable[pos - 1],
            None => focusable[0],
        };
        self.set_focus(prev);
    }

    fn set_focus(&mut self, idx: usize) {
        // Unfocus old
        if self.focus_index < self.nodes.len() {
            self.nodes[self.focus_index].focused = false;
        }
        // Focus new
        self.focus_index = idx;
        if idx < self.nodes.len() {
            self.nodes[idx].focused = true;
            let node = &self.nodes[idx];
            let desc = self.describe_node(node);
            self.announce(&desc);
        }
    }

    fn describe_node(&self, node: &AccessibilityNode) -> String {
        let role_str = match node.role {
            NodeRole::Button => "button",
            NodeRole::Text => "text",
            NodeRole::EditField => "edit field",
            NodeRole::CheckBox => "checkbox",
            NodeRole::RadioButton => "radio button",
            NodeRole::Slider => "slider",
            NodeRole::Link => "link",
            NodeRole::Image => "image",
            NodeRole::Heading => "heading",
            NodeRole::Switch => "switch",
            _ => "element",
        };

        let mut desc = if !node.label.is_empty() {
            format!("{}, {}", node.label, role_str)
        } else {
            format!("{}", role_str)
        };

        if let Some(checked) = node.checked {
            desc.push_str(if checked {
                ", checked"
            } else {
                ", not checked"
            });
        }

        if !node.enabled {
            desc.push_str(", disabled");
        }

        if self.verbosity == Verbosity::High && !node.description.is_empty() {
            desc.push_str(&format!(". {}", node.description));
        }

        desc
    }

    pub fn pop_announcement(&mut self) -> Option<String> {
        if self.announcement_queue.is_empty() {
            None
        } else {
            Some(self.announcement_queue.remove(0))
        }
    }

    /// Announce a key name from its Linux key code value.
    ///
    /// Maps well-known key codes to human-readable names; falls back to a
    /// numeric description for unknown codes.
    pub fn announce_key(&mut self, key_code: u16) {
        if !self.enabled {
            return;
        }
        let name: &str = match key_code {
            0x01 => "Escape",
            0x02 => "1",
            0x03 => "2",
            0x04 => "3",
            0x05 => "4",
            0x06 => "5",
            0x07 => "6",
            0x08 => "7",
            0x09 => "8",
            0x0A => "9",
            0x0B => "0",
            0x0C => "Minus",
            0x0D => "Equals",
            0x0E => "Backspace",
            0x0F => "Tab",
            0x10 => "Q",
            0x11 => "W",
            0x12 => "E",
            0x13 => "R",
            0x14 => "T",
            0x15 => "Y",
            0x16 => "U",
            0x17 => "I",
            0x18 => "O",
            0x19 => "P",
            0x1A => "Left Bracket",
            0x1B => "Right Bracket",
            0x1C => "Enter",
            0x1D => "Left Control",
            0x1E => "A",
            0x1F => "S",
            0x20 => "D",
            0x21 => "F",
            0x22 => "G",
            0x23 => "H",
            0x24 => "J",
            0x25 => "K",
            0x26 => "L",
            0x27 => "Semicolon",
            0x28 => "Apostrophe",
            0x29 => "Grave",
            0x2A => "Left Shift",
            0x2B => "Backslash",
            0x2C => "Z",
            0x2D => "X",
            0x2E => "C",
            0x2F => "V",
            0x30 => "B",
            0x31 => "N",
            0x32 => "M",
            0x33 => "Comma",
            0x34 => "Period",
            0x35 => "Slash",
            0x36 => "Right Shift",
            0x38 => "Left Alt",
            0x39 => "Space",
            0x3A => "Caps Lock",
            0x3B => "F1",
            0x3C => "F2",
            0x3D => "F3",
            0x3E => "F4",
            0x3F => "F5",
            0x40 => "F6",
            0x41 => "F7",
            0x42 => "F8",
            0x43 => "F9",
            0x44 => "F10",
            0x45 => "Num Lock",
            0x46 => "Scroll Lock",
            0x57 => "F11",
            0x58 => "F12",
            0x110 => "Left Button",
            0x111 => "Right Button",
            0x112 => "Middle Button",
            _ => "Unknown key",
        };
        // Enqueue via TTS if available; always serial-log as fallback.
        crate::serial_println!("  [a11y] key: {}", name);
        self.announcement_queue.push(String::from(name));
    }

    /// Announce that a widget has received focus.
    ///
    /// Formats a phrase such as "Button: OK, focused" and enqueues it.
    pub fn announce_widget_focus(&mut self, widget_name: &str, widget_type: &str) {
        if !self.enabled {
            return;
        }
        let text = alloc::format!("{}: {}, focused", widget_type, widget_name);
        crate::serial_println!("  [a11y] focus: {}", text);
        self.announcement_queue.push(text);
    }
}

static READER: Mutex<ScreenReader> = Mutex::new(ScreenReader::new());

pub fn init() {
    crate::serial_println!("  [a11y] Screen reader initialized");
}

pub fn enable() {
    READER.lock().enable();
}
pub fn disable() {
    READER.lock().disable();
}
pub fn announce(text: &str) {
    READER.lock().announce(text);
}

/// Announce a key by its Linux key-code value (e.g., 0x10 for Q).
pub fn announce_key(key_code: u16) {
    READER.lock().announce_key(key_code);
}

/// Announce that a named widget of the given type has received focus.
pub fn announce_widget_focus(widget_name: &str, widget_type: &str) {
    READER
        .lock()
        .announce_widget_focus(widget_name, widget_type);
}
