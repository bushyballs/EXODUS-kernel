use crate::sync::Mutex;
/// CSS parser and cascade engine for Genesis browser
///
/// Parses CSS stylesheets into rules, computes specificity,
/// cascades rules onto elements, and resolves computed styles.
/// Uses Q16 fixed-point for all numeric values.
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

static CSS_ENGINE: Mutex<Option<CssEngineState>> = Mutex::new(None);

/// Q16 fixed-point constant: 1 << 16 = 65536
const Q16_ONE: i32 = 65536;

/// Hash helper (FNV-1a)
fn css_hash(s: &[u8]) -> u64 {
    let mut h: u64 = 0xCBF29CE484222325;
    for &b in s {
        h ^= b as u64;
        h = h.wrapping_mul(0x00000100000001B3);
    }
    h
}

/// A single CSS property (name -> value)
#[derive(Debug, Clone)]
pub struct CssProperty {
    pub name_hash: u64,
    pub name_raw: Vec<u8>,
    pub value: CssValue,
}

/// Parsed CSS value types
#[derive(Debug, Clone)]
pub enum CssValue {
    Length(i32),     // Q16 pixels
    Percentage(i32), // Q16 percentage (100% = 100 * Q16_ONE)
    Color(u32),      // 0xAARRGGBB
    Keyword(u64),    // hash of keyword string
    Number(i32),     // Q16 raw number
    None,
}

/// Specificity: packed as (inline, id_count, class_count, element_count)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Specificity(pub u32);

impl Specificity {
    pub fn new(inline: u8, ids: u8, classes: u8, elements: u8) -> Self {
        Specificity(
            ((inline as u32) << 24)
                | ((ids as u32) << 16)
                | ((classes as u32) << 8)
                | (elements as u32),
        )
    }

    pub fn element() -> Self {
        Self::new(0, 0, 0, 1)
    }
    pub fn class() -> Self {
        Self::new(0, 0, 1, 0)
    }
    pub fn id() -> Self {
        Self::new(0, 1, 0, 0)
    }
    pub fn inline() -> Self {
        Self::new(1, 0, 0, 0)
    }
}

/// Selector type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectorKind {
    Tag,       // div, p, span
    Class,     // .classname
    Id,        // #idname
    Universal, // *
}

/// A parsed selector
#[derive(Debug, Clone)]
pub struct CssSelector {
    pub kind: SelectorKind,
    pub hash: u64,
    pub specificity: Specificity,
}

/// A CSS rule: selector + properties
#[derive(Debug, Clone)]
pub struct CssRule {
    pub selector: CssSelector,
    pub selector_hash: u64,
    pub properties: Vec<CssProperty>,
}

/// Computed style for an element
#[derive(Debug, Clone)]
pub struct ComputedStyle {
    pub display: DisplayType,
    pub width: i32,  // Q16 px
    pub height: i32, // Q16 px
    pub margin_top: i32,
    pub margin_right: i32,
    pub margin_bottom: i32,
    pub margin_left: i32,
    pub padding_top: i32,
    pub padding_right: i32,
    pub padding_bottom: i32,
    pub padding_left: i32,
    pub border_width: i32, // Q16 px
    pub color: u32,        // 0xAARRGGBB
    pub background: u32,
    pub font_size: i32, // Q16 px
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayType {
    Block,
    Inline,
    None,
}

impl ComputedStyle {
    pub fn default_style() -> Self {
        ComputedStyle {
            display: DisplayType::Block,
            width: 0,
            height: 0,
            margin_top: 0,
            margin_right: 0,
            margin_bottom: 0,
            margin_left: 0,
            padding_top: 0,
            padding_right: 0,
            padding_bottom: 0,
            padding_left: 0,
            border_width: 0,
            color: 0xFF000000,       // black
            background: 0xFFFFFFFF,  // white
            font_size: 16 * Q16_ONE, // 16px default
        }
    }
}

/// CSS engine persistent state
struct CssEngineState {
    stylesheets: Vec<Vec<CssRule>>,
    rules_count: u64,
}

/// Parse a hex color like #RGB, #RRGGBB, or #AARRGGBB
fn parse_color(s: &[u8]) -> u32 {
    if s.is_empty() || s[0] != b'#' {
        return 0xFF000000;
    }
    let hex = &s[1..];
    let mut val: u32 = 0;
    for &c in hex {
        let digit = match c {
            b'0'..=b'9' => (c - b'0') as u32,
            b'a'..=b'f' => (c - b'a' + 10) as u32,
            b'A'..=b'F' => (c - b'A' + 10) as u32,
            _ => 0,
        };
        val = (val << 4) | digit;
    }
    match hex.len() {
        3 => {
            // #RGB -> #RRGGBB
            let r = (val >> 8) & 0x0F;
            let g = (val >> 4) & 0x0F;
            let b = val & 0x0F;
            0xFF000000 | ((r | (r << 4)) << 16) | ((g | (g << 4)) << 8) | (b | (b << 4))
        }
        6 => 0xFF000000 | val,
        8 => val,
        _ => 0xFF000000,
    }
}

/// Parse a numeric value with optional px/% suffix into Q16
fn parse_length(s: &[u8]) -> CssValue {
    let mut i = 0;
    let negative = if i < s.len() && s[i] == b'-' {
        i += 1;
        true
    } else {
        false
    };
    let mut int_part: i32 = 0;
    while i < s.len() && s[i] >= b'0' && s[i] <= b'9' {
        int_part = int_part * 10 + (s[i] - b'0') as i32;
        i += 1;
    }
    if negative {
        int_part = -int_part;
    }
    let q16_val = int_part * Q16_ONE;
    let suffix = &s[i..];
    if suffix == b"%" {
        CssValue::Percentage(q16_val)
    } else {
        CssValue::Length(q16_val)
    }
}

/// Parse a single CSS property declaration: "name: value"
fn parse_property(decl: &[u8]) -> Option<CssProperty> {
    let mut colon_pos = None;
    for (i, &c) in decl.iter().enumerate() {
        if c == b':' {
            colon_pos = Some(i);
            break;
        }
    }
    let colon = colon_pos?;
    let name = trim_bytes(&decl[..colon]);
    let val_raw = trim_bytes(&decl[colon + 1..]);
    if name.is_empty() {
        return None;
    }

    let value = if val_raw.starts_with(b"#") {
        CssValue::Color(parse_color(val_raw))
    } else if !val_raw.is_empty() && (val_raw[0].is_ascii_digit() || val_raw[0] == b'-') {
        parse_length(val_raw)
    } else {
        CssValue::Keyword(css_hash(val_raw))
    };

    Some(CssProperty {
        name_hash: css_hash(name),
        name_raw: name.to_vec(),
        value,
    })
}

fn trim_bytes(s: &[u8]) -> &[u8] {
    let start = s
        .iter()
        .position(|&c| c != b' ' && c != b'\t' && c != b'\n' && c != b'\r')
        .unwrap_or(s.len());
    let end = s
        .iter()
        .rposition(|&c| c != b' ' && c != b'\t' && c != b'\n' && c != b'\r')
        .map(|p| p + 1)
        .unwrap_or(start);
    &s[start..end]
}

/// Parse a selector string into a CssSelector
fn parse_selector(s: &[u8]) -> CssSelector {
    let trimmed = trim_bytes(s);
    if trimmed.is_empty() || trimmed == b"*" {
        return CssSelector {
            kind: SelectorKind::Universal,
            hash: 0,
            specificity: Specificity::new(0, 0, 0, 0),
        };
    }
    match trimmed[0] {
        b'#' => CssSelector {
            kind: SelectorKind::Id,
            hash: css_hash(&trimmed[1..]),
            specificity: Specificity::id(),
        },
        b'.' => CssSelector {
            kind: SelectorKind::Class,
            hash: css_hash(&trimmed[1..]),
            specificity: Specificity::class(),
        },
        _ => CssSelector {
            kind: SelectorKind::Tag,
            hash: css_hash(trimmed),
            specificity: Specificity::element(),
        },
    }
}

/// Parse an entire CSS stylesheet into a list of CssRules
pub fn parse_stylesheet(data: &[u8]) -> Vec<CssRule> {
    let mut rules = Vec::new();
    let mut i = 0;
    while i < data.len() {
        // Find selector (before '{')
        let sel_start = i;
        while i < data.len() && data[i] != b'{' {
            i += 1;
        }
        if i >= data.len() {
            break;
        }
        let selector_bytes = trim_bytes(&data[sel_start..i]);
        let selector = parse_selector(selector_bytes);
        i += 1; // skip '{'

        // Find properties (before '}')
        let block_start = i;
        while i < data.len() && data[i] != b'}' {
            i += 1;
        }
        if i >= data.len() {
            break;
        }
        let block = &data[block_start..i];
        i += 1; // skip '}'

        // Split block by ';' into declarations
        let mut properties = Vec::new();
        let mut decl_start = 0;
        for (j, &c) in block.iter().enumerate() {
            if c == b';' {
                if let Some(prop) = parse_property(&block[decl_start..j]) {
                    properties.push(prop);
                }
                decl_start = j + 1;
            }
        }
        // Handle last declaration without trailing semicolon
        if decl_start < block.len() {
            if let Some(prop) = parse_property(&block[decl_start..]) {
                properties.push(prop);
            }
        }

        if !properties.is_empty() {
            rules.push(CssRule {
                selector_hash: selector.hash,
                selector: selector,
                properties,
            });
        }
    }

    let mut guard = CSS_ENGINE.lock();
    if let Some(ref mut state) = *guard {
        state.rules_count += rules.len() as u64;
    }
    rules
}

/// Check if a selector matches a given element
pub fn match_selector(
    selector: &CssSelector,
    tag_hash: u64,
    id_hash: u64,
    classes: &[u64],
) -> bool {
    match selector.kind {
        SelectorKind::Universal => true,
        SelectorKind::Tag => selector.hash == tag_hash,
        SelectorKind::Id => selector.hash == id_hash,
        SelectorKind::Class => classes.iter().any(|&c| c == selector.hash),
    }
}

/// Cascade: gather all matching rules sorted by specificity, return merged style
pub fn cascade(rules: &[CssRule], tag_hash: u64, id_hash: u64, classes: &[u64]) -> ComputedStyle {
    let mut matched: Vec<(&CssRule, Specificity)> = Vec::new();
    for rule in rules {
        if match_selector(&rule.selector, tag_hash, id_hash, classes) {
            matched.push((rule, rule.selector.specificity));
        }
    }
    // Sort by specificity ascending so higher specificity overrides
    matched.sort_by(|a, b| a.1.cmp(&b.1));

    let mut style = ComputedStyle::default_style();
    let hash_display = css_hash(b"display");
    let hash_width = css_hash(b"width");
    let hash_height = css_hash(b"height");
    let hash_color = css_hash(b"color");
    let hash_background = css_hash(b"background");
    let hash_font_size = css_hash(b"font-size");
    let hash_margin = css_hash(b"margin");
    let hash_padding = css_hash(b"padding");
    let hash_border_width = css_hash(b"border-width");
    let hash_none = css_hash(b"none");
    let hash_inline = css_hash(b"inline");

    for (rule, _) in &matched {
        for prop in &rule.properties {
            if prop.name_hash == hash_display {
                if let CssValue::Keyword(kw) = prop.value {
                    if kw == hash_none {
                        style.display = DisplayType::None;
                    } else if kw == hash_inline {
                        style.display = DisplayType::Inline;
                    } else {
                        style.display = DisplayType::Block;
                    }
                }
            } else if prop.name_hash == hash_width {
                if let CssValue::Length(v) = prop.value {
                    style.width = v;
                }
            } else if prop.name_hash == hash_height {
                if let CssValue::Length(v) = prop.value {
                    style.height = v;
                }
            } else if prop.name_hash == hash_color {
                if let CssValue::Color(c) = prop.value {
                    style.color = c;
                }
            } else if prop.name_hash == hash_background {
                if let CssValue::Color(c) = prop.value {
                    style.background = c;
                }
            } else if prop.name_hash == hash_font_size {
                if let CssValue::Length(v) = prop.value {
                    style.font_size = v;
                }
            } else if prop.name_hash == hash_margin {
                if let CssValue::Length(v) = prop.value {
                    style.margin_top = v;
                    style.margin_right = v;
                    style.margin_bottom = v;
                    style.margin_left = v;
                }
            } else if prop.name_hash == hash_padding {
                if let CssValue::Length(v) = prop.value {
                    style.padding_top = v;
                    style.padding_right = v;
                    style.padding_bottom = v;
                    style.padding_left = v;
                }
            } else if prop.name_hash == hash_border_width {
                if let CssValue::Length(v) = prop.value {
                    style.border_width = v;
                }
            }
        }
    }
    style
}

/// Convenience: compute style for a tag with id and classes against a stylesheet
pub fn compute_style(rules: &[CssRule], tag: &[u8], id: &[u8], classes: &[&[u8]]) -> ComputedStyle {
    let tag_hash = css_hash(tag);
    let id_hash = if id.is_empty() { 0 } else { css_hash(id) };
    let class_hashes: Vec<u64> = classes.iter().map(|c| css_hash(c)).collect();
    cascade(rules, tag_hash, id_hash, &class_hashes)
}

pub fn init() {
    let mut guard = CSS_ENGINE.lock();
    *guard = Some(CssEngineState {
        stylesheets: Vec::new(),
        rules_count: 0,
    });
    serial_println!("    browser::css_engine initialized");
}
