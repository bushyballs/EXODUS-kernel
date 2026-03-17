/// Browser / Web Engine subsystem for Genesis
///
/// A minimal but functional web engine built from scratch:
///   1. HTML parser: tokenizer + tree builder
///   2. CSS engine: stylesheet parser, cascade, specificity
///   3. JS interpreter: bytecode VM with stack machine
///   4. DOM: tree structure with events and queries
///   5. Renderer: layout engine (box model) and paint
///   6. Tabs: multi-tab manager with history navigation
///
/// All rendering uses Q16 fixed-point (no floats).
/// No external crates. Runs on bare metal.
use crate::{serial_print, serial_println};

pub mod css_engine;
pub mod dom;
pub mod html_parser;
pub mod js_interp;
pub mod renderer;
pub mod tabs;

pub fn init() {
    html_parser::init();
    css_engine::init();
    js_interp::init();
    dom::init();
    renderer::init();
    tabs::init();
    serial_println!("  Browser: html_parser, css_engine, js_interp, dom, renderer, tabs");
}
