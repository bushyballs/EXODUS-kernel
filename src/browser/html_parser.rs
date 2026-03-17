use crate::sync::Mutex;
/// HTML tokenizer and parser for Genesis browser
///
/// Implements a state-machine tokenizer that converts raw bytes into
/// tokens, then builds a tree of HtmlNode structs. Supports basic
/// HTML entities, self-closing tags, and attribute parsing.
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

static PARSER: Mutex<Option<HtmlParserState>> = Mutex::new(None);

/// Simple hash function for tag names and text (FNV-1a 64-bit)
pub fn str_hash(s: &[u8]) -> u64 {
    let mut h: u64 = 0xCBF29CE484222325;
    for &b in s {
        h ^= b as u64;
        h = h.wrapping_mul(0x00000100000001B3);
    }
    h
}

/// Tokenizer states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenizerState {
    Data,
    TagOpen,
    TagName,
    CloseTagOpen,
    CloseTagName,
    BeforeAttrName,
    AttrName,
    AfterAttrName,
    BeforeAttrValue,
    AttrValueQuoted,
    AttrValueUnquoted,
    SelfClosing,
    EntityInData,
    Comment,
    CommentDash,
    CommentEnd,
}

/// An HTML attribute (name_hash, value bytes)
#[derive(Debug, Clone)]
pub struct HtmlAttribute {
    pub name_hash: u64,
    pub name_raw: Vec<u8>,
    pub value: Vec<u8>,
}

/// A node in the parsed HTML tree
#[derive(Debug, Clone)]
pub struct HtmlNode {
    pub tag_hash: u64,
    pub tag_raw: Vec<u8>,
    pub attributes: Vec<HtmlAttribute>,
    pub children: Vec<HtmlNode>,
    pub text_hash: u64,
    pub text_raw: Vec<u8>,
    pub is_text: bool,
}

/// Token produced by the tokenizer
#[derive(Debug, Clone)]
pub enum HtmlToken {
    StartTag {
        name: Vec<u8>,
        attrs: Vec<HtmlAttribute>,
        self_closing: bool,
    },
    EndTag {
        name: Vec<u8>,
    },
    Text(Vec<u8>),
    Comment(Vec<u8>),
}

/// Persistent parser state
struct HtmlParserState {
    documents_parsed: u64,
}

/// Self-closing (void) tags — matched by hash
const VOID_TAGS: [u64; 10] = [
    0xE40C292C4AE2B8D7, // br
    0xE40C292C4AE2B8CF, // hr
    0xB78017ED2C3E4ADB, // img
    0xEFF2D2B23ACA0E12, // input
    0x7E79C0A0D2C3A3AC, // meta
    0xF02D0B2349A3CB8A, // link
    0xA09C310E0DB942A1, // area
    0x1C9E4238BDA397E1, // base
    0x16C93DCDE22CB8D4, // col
    0x2DD8ADB1A0BFCD24, // embed
];

fn is_void_tag(hash: u64) -> bool {
    VOID_TAGS.iter().any(|&h| h == hash)
}

/// Resolve a basic HTML entity to a byte
fn handle_entity(entity: &[u8]) -> u8 {
    match entity {
        b"amp" => b'&',
        b"lt" => b'<',
        b"gt" => b'>',
        b"quot" => b'"',
        b"apos" => b'\'',
        b"nbsp" => b' ',
        _ => {
            // Numeric entity: &#NN;
            if entity.len() > 1 && entity[0] == b'#' {
                let digits = &entity[1..];
                let mut val: u32 = 0;
                for &d in digits {
                    if d >= b'0' && d <= b'9' {
                        val = val * 10 + (d - b'0') as u32;
                    }
                }
                if val > 0 && val < 128 {
                    val as u8
                } else {
                    b'?'
                }
            } else {
                b'?'
            }
        }
    }
}

/// Tokenize raw HTML bytes into a stream of HtmlTokens
pub fn tokenize(data: &[u8]) -> Vec<HtmlToken> {
    let mut tokens = Vec::new();
    let mut state = TokenizerState::Data;
    let mut current_tag: Vec<u8> = Vec::new();
    let mut current_text: Vec<u8> = Vec::new();
    let mut attr_name: Vec<u8> = Vec::new();
    let mut attr_value: Vec<u8> = Vec::new();
    let mut attrs: Vec<HtmlAttribute> = Vec::new();
    let mut is_close = false;
    let mut self_closing = false;
    let mut entity_buf: Vec<u8> = Vec::new();
    let mut quote_char: u8 = b'"';
    let mut comment_buf: Vec<u8> = Vec::new();
    let mut i = 0;

    while i < data.len() {
        let c = data[i];
        match state {
            TokenizerState::Data => {
                if c == b'<' {
                    if !current_text.is_empty() {
                        tokens.push(HtmlToken::Text(current_text.clone()));
                        current_text.clear();
                    }
                    state = TokenizerState::TagOpen;
                } else if c == b'&' {
                    entity_buf.clear();
                    state = TokenizerState::EntityInData;
                } else {
                    current_text.push(c);
                }
            }
            TokenizerState::EntityInData => {
                if c == b';' {
                    current_text.push(handle_entity(&entity_buf));
                    state = TokenizerState::Data;
                } else if entity_buf.len() < 10 {
                    entity_buf.push(c);
                } else {
                    current_text.push(b'&');
                    current_text.extend_from_slice(&entity_buf);
                    state = TokenizerState::Data;
                    continue; // re-process current byte
                }
            }
            TokenizerState::TagOpen => {
                if c == b'/' {
                    is_close = true;
                    state = TokenizerState::CloseTagName;
                    current_tag.clear();
                } else if c == b'!' {
                    // Possible comment: <!--
                    if i + 2 < data.len() && data[i + 1] == b'-' && data[i + 2] == b'-' {
                        i += 2;
                        comment_buf.clear();
                        state = TokenizerState::Comment;
                    } else {
                        state = TokenizerState::Data;
                    }
                } else if c.is_ascii_alphabetic() {
                    current_tag.clear();
                    current_tag.push(c.to_ascii_lowercase());
                    is_close = false;
                    self_closing = false;
                    attrs.clear();
                    state = TokenizerState::TagName;
                } else {
                    current_text.push(b'<');
                    state = TokenizerState::Data;
                    continue;
                }
            }
            TokenizerState::Comment => {
                if c == b'-' {
                    state = TokenizerState::CommentDash;
                } else {
                    comment_buf.push(c);
                }
            }
            TokenizerState::CommentDash => {
                if c == b'-' {
                    state = TokenizerState::CommentEnd;
                } else {
                    comment_buf.push(b'-');
                    comment_buf.push(c);
                    state = TokenizerState::Comment;
                }
            }
            TokenizerState::CommentEnd => {
                if c == b'>' {
                    tokens.push(HtmlToken::Comment(comment_buf.clone()));
                    comment_buf.clear();
                    state = TokenizerState::Data;
                } else {
                    comment_buf.push(b'-');
                    comment_buf.push(b'-');
                    comment_buf.push(c);
                    state = TokenizerState::Comment;
                }
            }
            TokenizerState::TagName => {
                if c == b'>' {
                    tokens.push(HtmlToken::StartTag {
                        name: current_tag.clone(),
                        attrs: attrs.clone(),
                        self_closing,
                    });
                    state = TokenizerState::Data;
                } else if c == b'/' {
                    self_closing = true;
                    state = TokenizerState::SelfClosing;
                } else if c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' {
                    state = TokenizerState::BeforeAttrName;
                } else {
                    current_tag.push(c.to_ascii_lowercase());
                }
            }
            TokenizerState::SelfClosing => {
                if c == b'>' {
                    tokens.push(HtmlToken::StartTag {
                        name: current_tag.clone(),
                        attrs: attrs.clone(),
                        self_closing: true,
                    });
                    state = TokenizerState::Data;
                }
            }
            TokenizerState::CloseTagOpen => {
                // Treat same as start of close tag name
                if c.is_ascii_alphabetic() {
                    current_tag.clear();
                    current_tag.push(c.to_ascii_lowercase());
                    state = TokenizerState::CloseTagName;
                } else {
                    state = TokenizerState::Data;
                }
            }
            TokenizerState::CloseTagName => {
                if c == b'>' {
                    tokens.push(HtmlToken::EndTag {
                        name: current_tag.clone(),
                    });
                    is_close = false;
                    state = TokenizerState::Data;
                } else if c != b' ' && c != b'\t' && c != b'\n' {
                    current_tag.push(c.to_ascii_lowercase());
                }
            }
            TokenizerState::BeforeAttrName => {
                if c == b'>' {
                    tokens.push(HtmlToken::StartTag {
                        name: current_tag.clone(),
                        attrs: attrs.clone(),
                        self_closing,
                    });
                    state = TokenizerState::Data;
                } else if c == b'/' {
                    self_closing = true;
                    state = TokenizerState::SelfClosing;
                } else if c != b' ' && c != b'\t' && c != b'\n' && c != b'\r' {
                    attr_name.clear();
                    attr_name.push(c.to_ascii_lowercase());
                    state = TokenizerState::AttrName;
                }
            }
            TokenizerState::AttrName => {
                if c == b'=' {
                    attr_value.clear();
                    state = TokenizerState::BeforeAttrValue;
                } else if c == b' ' || c == b'\t' || c == b'\n' {
                    state = TokenizerState::AfterAttrName;
                } else if c == b'>' {
                    attrs.push(HtmlAttribute {
                        name_hash: str_hash(&attr_name),
                        name_raw: attr_name.clone(),
                        value: Vec::new(),
                    });
                    tokens.push(HtmlToken::StartTag {
                        name: current_tag.clone(),
                        attrs: attrs.clone(),
                        self_closing,
                    });
                    state = TokenizerState::Data;
                } else {
                    attr_name.push(c.to_ascii_lowercase());
                }
            }
            TokenizerState::AfterAttrName => {
                if c == b'=' {
                    attr_value.clear();
                    state = TokenizerState::BeforeAttrValue;
                } else if c == b'>' {
                    attrs.push(HtmlAttribute {
                        name_hash: str_hash(&attr_name),
                        name_raw: attr_name.clone(),
                        value: Vec::new(),
                    });
                    tokens.push(HtmlToken::StartTag {
                        name: current_tag.clone(),
                        attrs: attrs.clone(),
                        self_closing,
                    });
                    state = TokenizerState::Data;
                } else if c != b' ' && c != b'\t' {
                    // Boolean attribute, start new one
                    attrs.push(HtmlAttribute {
                        name_hash: str_hash(&attr_name),
                        name_raw: attr_name.clone(),
                        value: Vec::new(),
                    });
                    attr_name.clear();
                    attr_name.push(c.to_ascii_lowercase());
                    state = TokenizerState::AttrName;
                }
            }
            TokenizerState::BeforeAttrValue => {
                if c == b'"' || c == b'\'' {
                    quote_char = c;
                    state = TokenizerState::AttrValueQuoted;
                } else if c != b' ' && c != b'\t' {
                    attr_value.push(c);
                    state = TokenizerState::AttrValueUnquoted;
                }
            }
            TokenizerState::AttrValueQuoted => {
                if c == quote_char {
                    attrs.push(HtmlAttribute {
                        name_hash: str_hash(&attr_name),
                        name_raw: attr_name.clone(),
                        value: attr_value.clone(),
                    });
                    state = TokenizerState::BeforeAttrName;
                } else {
                    attr_value.push(c);
                }
            }
            TokenizerState::AttrValueUnquoted => {
                if c == b' ' || c == b'\t' || c == b'\n' {
                    attrs.push(HtmlAttribute {
                        name_hash: str_hash(&attr_name),
                        name_raw: attr_name.clone(),
                        value: attr_value.clone(),
                    });
                    state = TokenizerState::BeforeAttrName;
                } else if c == b'>' {
                    attrs.push(HtmlAttribute {
                        name_hash: str_hash(&attr_name),
                        name_raw: attr_name.clone(),
                        value: attr_value.clone(),
                    });
                    tokens.push(HtmlToken::StartTag {
                        name: current_tag.clone(),
                        attrs: attrs.clone(),
                        self_closing,
                    });
                    state = TokenizerState::Data;
                } else {
                    attr_value.push(c);
                }
            }
        }
        i += 1;
    }
    // Flush remaining text
    if !current_text.is_empty() {
        tokens.push(HtmlToken::Text(current_text));
    }
    tokens
}

/// Parse a list of tokens into a tree of HtmlNodes
fn build_tree(tokens: &[HtmlToken]) -> Vec<HtmlNode> {
    let mut root_children: Vec<HtmlNode> = Vec::new();
    let mut stack: Vec<HtmlNode> = Vec::new();

    for token in tokens {
        match token {
            HtmlToken::StartTag {
                name,
                attrs,
                self_closing,
            } => {
                let tag_hash = str_hash(name);
                let node = HtmlNode {
                    tag_hash,
                    tag_raw: name.clone(),
                    attributes: attrs.clone(),
                    children: Vec::new(),
                    text_hash: 0,
                    text_raw: Vec::new(),
                    is_text: false,
                };
                if *self_closing || is_void_tag(tag_hash) {
                    if let Some(parent) = stack.last_mut() {
                        parent.children.push(node);
                    } else {
                        root_children.push(node);
                    }
                } else {
                    stack.push(node);
                }
            }
            HtmlToken::EndTag { name } => {
                let close_hash = str_hash(name);
                // Pop until we find matching open tag
                if let Some(node) = stack.pop() {
                    // If mismatch, try to recover
                    if node.tag_hash != close_hash && !stack.is_empty() {
                        // Push node's children up then try again
                        let orphans = node.children.clone();
                        if let Some(grandparent) = stack.last_mut() {
                            grandparent.children.push(node);
                            for orphan in orphans {
                                grandparent.children.push(orphan);
                            }
                        } else {
                            root_children.push(node);
                        }
                    } else if let Some(parent) = stack.last_mut() {
                        parent.children.push(node);
                    } else {
                        root_children.push(node);
                    }
                }
            }
            HtmlToken::Text(text) => {
                let text_node = HtmlNode {
                    tag_hash: 0,
                    tag_raw: Vec::new(),
                    attributes: Vec::new(),
                    children: Vec::new(),
                    text_hash: str_hash(text),
                    text_raw: text.clone(),
                    is_text: true,
                };
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(text_node);
                } else {
                    root_children.push(text_node);
                }
            }
            HtmlToken::Comment(_) => {
                // Comments are discarded from the tree
            }
        }
    }
    // Drain any remaining unclosed tags
    while let Some(node) = stack.pop() {
        if let Some(parent) = stack.last_mut() {
            parent.children.push(node);
        } else {
            root_children.push(node);
        }
    }
    root_children
}

/// Parse raw HTML bytes into a tree of HtmlNodes
pub fn parse(data: &[u8]) -> Vec<HtmlNode> {
    let tokens = tokenize(data);
    let tree = build_tree(&tokens);
    let mut guard = PARSER.lock();
    if let Some(ref mut state) = *guard {
        state.documents_parsed = state.documents_parsed.saturating_add(1);
    }
    tree
}

pub fn init() {
    let mut guard = PARSER.lock();
    *guard = Some(HtmlParserState {
        documents_parsed: 0,
    });
    serial_println!("    browser::html_parser initialized");
}
