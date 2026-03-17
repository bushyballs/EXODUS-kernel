pub mod ai_apps;
pub mod clipboard;
pub mod data;
pub mod intents;
pub mod ipc_bridge;
pub mod lifecycle;
pub mod manifest;
pub mod notifications;
pub mod package;
pub mod permissions;
pub mod runtime;
pub mod sandbox;
pub mod ui_toolkit;
pub mod update;
/// Application framework for Genesis
///
/// Provides the runtime environment for user-space applications:
/// WASM runtime, UI toolkit, permissions, notifications, clipboard,
/// and package management. Apps are sandboxed by default.
///
/// Inspired by: Android app framework, Wasmtime, Tauri. All code is original.
pub mod wasm;

use crate::{serial_print, serial_println};

/// Initialize the application framework
pub fn init() {
    wasm::init();
    permissions::init();
    notifications::init();
    clipboard::init();
    package::init();
    intents::init();
    lifecycle::init();
    ai_apps::init();
    sandbox::init();
    runtime::init();
    manifest::init();
    update::init();
    data::init();
    ipc_bridge::init();
    serial_println!("  App framework initialized (AI app intelligence, usage prediction, sandbox, runtime, manifest, update, data, ipc_bridge)");
}
