/// Hoags Scripting & Automation Subsystem
///
/// Provides a complete scripting/automation framework for Genesis OS:
///   1. Script Engine — Built-in interpreted scripting language with lexer,
///      parser, and tree-walk interpreter. Supports variables, functions,
///      control flow (if/else, while, for), arithmetic, and print.
///   2. Task Automation — Event-driven workflow engine. Tasks fire on
///      schedules, system events, file changes, app launches, battery
///      levels, or location triggers. Actions chain scripts, apps,
///      notifications, settings changes, and file operations.
///   3. Macro Recorder — Record and replay user input sequences (key
///      presses, mouse clicks, scroll, touch gestures). Adjustable
///      speed, repeat count, insert delays, edit individual events.
///   4. Plugin System — Extension framework with lifecycle hooks
///      (boot, shutdown, app launch, file open, network, render, etc.).
///      Plugins declare permissions, register hooks, and expose APIs.
///
/// All modules use i32 Q16 fixed-point (no f32/f64), Vec from alloc,
/// and crate::sync::Mutex for global state. No external crates.
///
/// Inspired by: AutoHotkey (macro recording), Tasker (task automation),
/// Lua (embedded scripting), VSCode extensions (plugin system).
/// All code is original.
use crate::{serial_print, serial_println};

pub mod macro_recorder;
pub mod plugin_system;
pub mod script_engine;
pub mod task_automation;

pub fn init() {
    script_engine::init();
    task_automation::init();
    macro_recorder::init();
    plugin_system::init();
    serial_println!("  Scripting: engine, tasks, macros, plugins");
}
