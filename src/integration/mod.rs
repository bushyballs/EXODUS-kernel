/// Hoags System Integration / Bus Subsystem
///
/// Ties together all Genesis OS subsystems through a unified integration layer:
///   1. Event Bus — System-wide publish/subscribe event routing with topics,
///      priority levels, async delivery queues, and subscriber filtering.
///   2. Service Registry — Service discovery and registration with health
///      checks, dependency injection, capability queries, and lifecycle mgmt.
///   3. D-Bus Compatible IPC — Method calls, signals, properties, bus names,
///      and introspection following a D-Bus-compatible protocol model.
///   4. Automation — Rule-based system automation with triggers, conditions,
///      compound actions, scheduling, and a forward-chaining rules engine.
///   5. Plugin Host — Plugin lifecycle management with sandboxing, API surface
///      control, hot-reload support, and resource accounting.
///
/// All modules use i32 Q16 fixed-point (no f32/f64), Vec from alloc,
/// and crate::sync::Mutex for global state. No external crates.
///
/// Inspired by: D-Bus (freedesktop.org), Android Binder, MQTT (pub/sub),
/// Kubernetes service discovery, OSGi plugin framework.
/// All code is original.

use crate::{serial_print, serial_println};

pub mod event_bus;
pub mod service_registry;
pub mod dbus;
pub mod automation;
pub mod plugin_host;

pub fn init() {
    event_bus::init();
    service_registry::init();
    dbus::init();
    automation::init();
    plugin_host::init();
    serial_println!("  Integration: event bus, service registry, D-Bus IPC, automation, plugin host");
}
