/// Hoags D-Bus Compatible IPC — method calls, signals, properties, introspection
///
/// Implements a D-Bus-compatible inter-process communication protocol for Genesis.
/// Services expose interfaces with methods, signals, and properties. Clients can:
///   - Call methods on remote objects (request/response pattern)
///   - Subscribe to signals (publish/subscribe pattern)
///   - Read/write properties on objects (get/set with change notifications)
///   - Introspect services to discover available interfaces
///   - Own well-known bus names (e.g., "org.genesis.network")
///
/// Messages are identified by u64 hashes rather than string paths for
/// zero-allocation operation in the kernel. Object paths, interface names,
/// and member names are all represented as hashes.
///
/// All numeric values use i32 Q16 fixed-point (65536 = 1.0).
/// No external crates. No f32/f64.
///
/// Inspired by: freedesktop D-Bus, kdbus, Binder (Android), COM (Windows).
/// All code is original.

use crate::{serial_print, serial_println};
use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;
use crate::sync::Mutex;

/// Q16 fixed-point: 65536 = 1.0
type Q16 = i32;
const Q16_ONE: Q16 = 65536;

/// Maximum connections (bus clients)
const MAX_CONNECTIONS: usize = 128;
/// Maximum well-known bus names
const MAX_BUS_NAMES: usize = 256;
/// Maximum objects per connection
const MAX_OBJECTS: usize = 64;
/// Maximum interfaces per object
const MAX_INTERFACES: usize = 16;
/// Maximum methods per interface
const MAX_METHODS: usize = 32;
/// Maximum properties per interface
const MAX_PROPERTIES: usize = 32;
/// Maximum signals per interface
const MAX_SIGNALS: usize = 16;
/// Maximum pending method calls
const MAX_PENDING_CALLS: usize = 256;
/// Maximum signal subscriptions
const MAX_SUBSCRIPTIONS: usize = 512;
/// Maximum message arguments
const MAX_ARGS: usize = 8;

// ---------------------------------------------------------------------------
// MessageType — types of D-Bus messages
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
    /// A method call requesting a response
    MethodCall,
    /// A response to a method call
    MethodReturn,
    /// An error response to a method call
    Error,
    /// A broadcast signal (no response expected)
    Signal,
}

// ---------------------------------------------------------------------------
// ArgType — D-Bus argument types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgType {
    /// 8-bit unsigned integer
    Byte,
    /// 32-bit signed integer
    Int32,
    /// 64-bit unsigned integer
    Uint64,
    /// Q16 fixed-point number
    Fixed,
    /// Boolean value
    Boolean,
    /// String (represented as hash)
    StringHash,
    /// Object path (represented as hash)
    ObjectPath,
    /// Array of bytes
    ByteArray,
}

// ---------------------------------------------------------------------------
// Argument — a single typed argument
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Argument {
    pub arg_type: ArgType,
    pub value: u64,
}

impl Argument {
    pub fn byte(v: u8) -> Self { Argument { arg_type: ArgType::Byte, value: v as u64 } }
    pub fn int32(v: i32) -> Self { Argument { arg_type: ArgType::Int32, value: v as u64 } }
    pub fn uint64(v: u64) -> Self { Argument { arg_type: ArgType::Uint64, value: v } }
    pub fn fixed(v: Q16) -> Self { Argument { arg_type: ArgType::Fixed, value: v as u64 } }
    pub fn boolean(v: bool) -> Self { Argument { arg_type: ArgType::Boolean, value: if v { 1 } else { 0 } } }
    pub fn string_hash(v: u64) -> Self { Argument { arg_type: ArgType::StringHash, value: v } }
    pub fn object_path(v: u64) -> Self { Argument { arg_type: ArgType::ObjectPath, value: v } }
}

// ---------------------------------------------------------------------------
// Message — a D-Bus message
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Message {
    pub serial: u32,
    pub msg_type: MessageType,
    pub sender: u32,
    pub destination: u32,
    pub object_path_hash: u64,
    pub interface_hash: u64,
    pub member_hash: u64,
    pub reply_serial: u32,
    pub args: Vec<Argument>,
    pub timestamp: u64,
}

impl Message {
    fn new(serial: u32, msg_type: MessageType, sender: u32) -> Self {
        Message {
            serial,
            msg_type,
            sender,
            destination: 0,
            object_path_hash: 0,
            interface_hash: 0,
            member_hash: 0,
            reply_serial: 0,
            args: Vec::new(),
            timestamp: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// MethodDef — method definition in an interface
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct MethodDef {
    name_hash: u64,
    in_args: Vec<ArgType>,
    out_args: Vec<ArgType>,
    handler_hash: u64,
}

// ---------------------------------------------------------------------------
// PropertyDef — property definition in an interface
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct PropertyDef {
    name_hash: u64,
    prop_type: ArgType,
    value: u64,
    readable: bool,
    writable: bool,
    emit_change: bool,
}

// ---------------------------------------------------------------------------
// SignalDef — signal definition in an interface
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct SignalDef {
    name_hash: u64,
    arg_types: Vec<ArgType>,
}

// ---------------------------------------------------------------------------
// Interface — a D-Bus interface on an object
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Interface {
    name_hash: u64,
    methods: Vec<MethodDef>,
    properties: Vec<PropertyDef>,
    signals: Vec<SignalDef>,
}

impl Interface {
    fn new(name_hash: u64) -> Self {
        Interface {
            name_hash,
            methods: Vec::new(),
            properties: Vec::new(),
            signals: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// BusObject — a D-Bus object at a specific path
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct BusObject {
    path_hash: u64,
    interfaces: Vec<Interface>,
}

impl BusObject {
    fn new(path_hash: u64) -> Self {
        BusObject {
            path_hash,
            interfaces: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Connection — a client connected to the bus
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Connection {
    id: u32,
    unique_name: u64,
    objects: Vec<BusObject>,
    active: bool,
    messages_sent: u64,
    messages_received: u64,
}

impl Connection {
    fn new(id: u32, unique_name: u64) -> Self {
        Connection {
            id,
            unique_name,
            objects: Vec::new(),
            active: true,
            messages_sent: 0,
            messages_received: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// BusName — a well-known name on the bus
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct BusName {
    name_hash: u64,
    owner_id: u32,
    queue: Vec<u32>,
}

// ---------------------------------------------------------------------------
// SignalSubscription — a client subscribed to a signal
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct SignalSubscription {
    subscriber_id: u32,
    interface_hash: u64,
    signal_hash: u64,
    source_filter: u32,
}

// ---------------------------------------------------------------------------
// PendingCall — an unresolved method call waiting for a reply
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct PendingCall {
    serial: u32,
    caller_id: u32,
    callee_id: u32,
    timestamp: u64,
    timeout_ms: u32,
}

// ---------------------------------------------------------------------------
// DBusState — main state
// ---------------------------------------------------------------------------

struct DBusState {
    connections: Vec<Connection>,
    bus_names: Vec<BusName>,
    subscriptions: Vec<SignalSubscription>,
    pending_calls: Vec<PendingCall>,
    next_connection_id: u32,
    next_serial: u32,
    total_messages: u64,
    total_method_calls: u64,
    total_signals_emitted: u64,
    total_errors: u64,
    initialized: bool,
}

impl DBusState {
    fn new() -> Self {
        DBusState {
            connections: Vec::new(),
            bus_names: Vec::new(),
            subscriptions: Vec::new(),
            pending_calls: Vec::new(),
            next_connection_id: 1,
            next_serial: 1,
            total_messages: 0,
            total_method_calls: 0,
            total_signals_emitted: 0,
            total_errors: 0,
            initialized: false,
        }
    }

    fn alloc_serial(&mut self) -> u32 {
        let s = self.next_serial;
        self.next_serial = self.next_serial.saturating_add(1);
        s
    }
}

static DBUS: Mutex<Option<DBusState>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Connection management
// ---------------------------------------------------------------------------

/// Connect to the bus. Returns a connection ID (unique name).
pub fn connect(name_hash: u64) -> u32 {
    let mut guard = DBUS.lock();
    if let Some(ref mut state) = *guard {
        if state.connections.len() >= MAX_CONNECTIONS {
            serial_println!("[dbus] ERROR: max connections ({}) reached", MAX_CONNECTIONS);
            return 0;
        }

        let id = state.next_connection_id;
        state.next_connection_id = state.next_connection_id.saturating_add(1);

        let conn = Connection::new(id, name_hash);
        serial_println!("[dbus] Connection {} established (name={:#018X})", id, name_hash);
        state.connections.push(conn);
        id
    } else {
        0
    }
}

/// Disconnect from the bus. Releases owned names and removes objects.
pub fn disconnect(connection_id: u32) -> bool {
    let mut guard = DBUS.lock();
    if let Some(ref mut state) = *guard {
        // Release owned bus names
        for name in &mut state.bus_names {
            if name.owner_id == connection_id {
                // Transfer ownership to next in queue
                if let Some(next) = name.queue.first().copied() {
                    name.owner_id = next;
                    name.queue.remove(0);
                    serial_println!("[dbus] Bus name {:#018X} transferred to connection {}",
                        name.name_hash, next);
                } else {
                    name.owner_id = 0;
                }
            }
            name.queue.retain(|&id| id != connection_id);
        }
        state.bus_names.retain(|n| n.owner_id != 0);

        // Remove subscriptions
        state.subscriptions.retain(|s| s.subscriber_id != connection_id);

        // Remove pending calls
        state.pending_calls.retain(|p| p.caller_id != connection_id && p.callee_id != connection_id);

        // Mark connection inactive
        for conn in &mut state.connections {
            if conn.id == connection_id {
                conn.active = false;
                serial_println!("[dbus] Connection {} disconnected", connection_id);
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Bus name ownership
// ---------------------------------------------------------------------------

/// Request ownership of a well-known bus name.
pub fn request_name(connection_id: u32, name_hash: u64) -> bool {
    let mut guard = DBUS.lock();
    if let Some(ref mut state) = *guard {
        // Check if name is already owned
        for name in &mut state.bus_names {
            if name.name_hash == name_hash {
                if name.owner_id == connection_id {
                    return true; // Already own it
                }
                // Queue for ownership
                if !name.queue.contains(&connection_id) {
                    name.queue.push(connection_id);
                }
                serial_println!("[dbus] Connection {} queued for name {:#018X}",
                    connection_id, name_hash);
                return false;
            }
        }

        if state.bus_names.len() >= MAX_BUS_NAMES {
            serial_println!("[dbus] ERROR: max bus names ({}) reached", MAX_BUS_NAMES);
            return false;
        }

        state.bus_names.push(BusName {
            name_hash,
            owner_id: connection_id,
            queue: Vec::new(),
        });
        serial_println!("[dbus] Connection {} owns name {:#018X}", connection_id, name_hash);
        true
    } else {
        false
    }
}

/// Release ownership of a well-known bus name.
pub fn release_name(connection_id: u32, name_hash: u64) -> bool {
    let mut guard = DBUS.lock();
    if let Some(ref mut state) = *guard {
        for name in &mut state.bus_names {
            if name.name_hash == name_hash && name.owner_id == connection_id {
                if let Some(next) = name.queue.first().copied() {
                    name.owner_id = next;
                    name.queue.remove(0);
                    serial_println!("[dbus] Name {:#018X} transferred to connection {}",
                        name_hash, next);
                } else {
                    name.owner_id = 0;
                }
                serial_println!("[dbus] Connection {} released name {:#018X}",
                    connection_id, name_hash);
                return true;
            }
        }
    }
    false
}

/// Resolve a bus name to its owner connection ID.
pub fn resolve_name(name_hash: u64) -> Option<u32> {
    let guard = DBUS.lock();
    if let Some(ref state) = *guard {
        for name in &state.bus_names {
            if name.name_hash == name_hash && name.owner_id != 0 {
                return Some(name.owner_id);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Object / Interface registration
// ---------------------------------------------------------------------------

/// Register an object at a given path on a connection.
pub fn register_object(connection_id: u32, path_hash: u64) -> bool {
    let mut guard = DBUS.lock();
    if let Some(ref mut state) = *guard {
        for conn in &mut state.connections {
            if conn.id == connection_id {
                if conn.objects.len() >= MAX_OBJECTS {
                    serial_println!("[dbus] ERROR: max objects for connection {}", connection_id);
                    return false;
                }
                // Check duplicate
                for obj in &conn.objects {
                    if obj.path_hash == path_hash {
                        return true; // Already registered
                    }
                }
                conn.objects.push(BusObject::new(path_hash));
                serial_println!("[dbus] Object {:#018X} registered on connection {}",
                    path_hash, connection_id);
                return true;
            }
        }
    }
    false
}

/// Add an interface to an object.
pub fn add_interface(connection_id: u32, path_hash: u64, interface_hash: u64) -> bool {
    let mut guard = DBUS.lock();
    if let Some(ref mut state) = *guard {
        for conn in &mut state.connections {
            if conn.id == connection_id {
                for obj in &mut conn.objects {
                    if obj.path_hash == path_hash {
                        if obj.interfaces.len() >= MAX_INTERFACES {
                            return false;
                        }
                        for iface in &obj.interfaces {
                            if iface.name_hash == interface_hash {
                                return true; // Already exists
                            }
                        }
                        obj.interfaces.push(Interface::new(interface_hash));
                        serial_println!("[dbus] Interface {:#018X} added to object {:#018X}",
                            interface_hash, path_hash);
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Add a method to an interface.
pub fn add_method(
    connection_id: u32,
    path_hash: u64,
    interface_hash: u64,
    method_hash: u64,
    in_args: Vec<ArgType>,
    out_args: Vec<ArgType>,
    handler_hash: u64,
) -> bool {
    let mut guard = DBUS.lock();
    if let Some(ref mut state) = *guard {
        for conn in &mut state.connections {
            if conn.id == connection_id {
                for obj in &mut conn.objects {
                    if obj.path_hash == path_hash {
                        for iface in &mut obj.interfaces {
                            if iface.name_hash == interface_hash {
                                if iface.methods.len() >= MAX_METHODS {
                                    return false;
                                }
                                iface.methods.push(MethodDef {
                                    name_hash: method_hash,
                                    in_args,
                                    out_args,
                                    handler_hash,
                                });
                                return true;
                            }
                        }
                    }
                }
            }
        }
    }
    false
}

/// Add a property to an interface.
pub fn add_property(
    connection_id: u32,
    path_hash: u64,
    interface_hash: u64,
    prop_hash: u64,
    prop_type: ArgType,
    initial_value: u64,
    readable: bool,
    writable: bool,
    emit_change: bool,
) -> bool {
    let mut guard = DBUS.lock();
    if let Some(ref mut state) = *guard {
        for conn in &mut state.connections {
            if conn.id == connection_id {
                for obj in &mut conn.objects {
                    if obj.path_hash == path_hash {
                        for iface in &mut obj.interfaces {
                            if iface.name_hash == interface_hash {
                                if iface.properties.len() >= MAX_PROPERTIES {
                                    return false;
                                }
                                iface.properties.push(PropertyDef {
                                    name_hash: prop_hash,
                                    prop_type,
                                    value: initial_value,
                                    readable,
                                    writable,
                                    emit_change,
                                });
                                return true;
                            }
                        }
                    }
                }
            }
        }
    }
    false
}

/// Add a signal definition to an interface.
pub fn add_signal(
    connection_id: u32,
    path_hash: u64,
    interface_hash: u64,
    signal_hash: u64,
    arg_types: Vec<ArgType>,
) -> bool {
    let mut guard = DBUS.lock();
    if let Some(ref mut state) = *guard {
        for conn in &mut state.connections {
            if conn.id == connection_id {
                for obj in &mut conn.objects {
                    if obj.path_hash == path_hash {
                        for iface in &mut obj.interfaces {
                            if iface.name_hash == interface_hash {
                                if iface.signals.len() >= MAX_SIGNALS {
                                    return false;
                                }
                                iface.signals.push(SignalDef {
                                    name_hash: signal_hash,
                                    arg_types,
                                });
                                return true;
                            }
                        }
                    }
                }
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Method calls
// ---------------------------------------------------------------------------

/// Send a method call message. Returns the serial number for tracking the reply.
pub fn call_method(
    caller_id: u32,
    destination_id: u32,
    object_hash: u64,
    interface_hash: u64,
    method_hash: u64,
    args: Vec<Argument>,
    timestamp: u64,
    timeout_ms: u32,
) -> u32 {
    let mut guard = DBUS.lock();
    if let Some(ref mut state) = *guard {
        let serial = state.alloc_serial();
        state.total_messages = state.total_messages.saturating_add(1);
        state.total_method_calls = state.total_method_calls.saturating_add(1);

        // Verify destination has the method
        let mut method_found = false;
        for conn in &state.connections {
            if conn.id == destination_id && conn.active {
                for obj in &conn.objects {
                    if obj.path_hash == object_hash {
                        for iface in &obj.interfaces {
                            if iface.name_hash == interface_hash {
                                for method in &iface.methods {
                                    if method.name_hash == method_hash {
                                        method_found = true;
                                        break;
                                    }
                                }
                            }
                            if method_found { break; }
                        }
                    }
                    if method_found { break; }
                }
            }
            if method_found { break; }
        }

        if !method_found {
            serial_println!("[dbus] ERROR: method {:#018X} not found on {:#018X}.{:#018X}",
                method_hash, object_hash, interface_hash);
            state.total_errors = state.total_errors.saturating_add(1);
            return 0;
        }

        // Track pending call
        if state.pending_calls.len() < MAX_PENDING_CALLS {
            state.pending_calls.push(PendingCall {
                serial,
                caller_id,
                callee_id: destination_id,
                timestamp,
                timeout_ms,
            });
        }

        // Update sender stats
        for conn in &mut state.connections {
            if conn.id == caller_id {
                conn.messages_sent = conn.messages_sent.saturating_add(1);
            }
            if conn.id == destination_id {
                conn.messages_received = conn.messages_received.saturating_add(1);
            }
        }

        serial_println!("[dbus] MethodCall serial={} {} -> {} ({:#018X}.{:#018X}.{:#018X}, {} args)",
            serial, caller_id, destination_id, object_hash, interface_hash, method_hash,
            args.len());
        serial
    } else {
        0
    }
}

/// Send a method return (reply to a call).
pub fn method_return(
    responder_id: u32,
    reply_serial: u32,
    args: Vec<Argument>,
    timestamp: u64,
) -> bool {
    let mut guard = DBUS.lock();
    if let Some(ref mut state) = *guard {
        state.total_messages = state.total_messages.saturating_add(1);

        // Find and remove pending call
        let mut caller_id = 0u32;
        let mut found = false;
        state.pending_calls.retain(|p| {
            if p.serial == reply_serial && p.callee_id == responder_id {
                caller_id = p.caller_id;
                found = true;
                false // remove
            } else {
                true
            }
        });

        if !found {
            serial_println!("[dbus] ERROR: no pending call for reply serial {}", reply_serial);
            state.total_errors = state.total_errors.saturating_add(1);
            return false;
        }

        for conn in &mut state.connections {
            if conn.id == responder_id { conn.messages_sent = conn.messages_sent.saturating_add(1); }
            if conn.id == caller_id { conn.messages_received = conn.messages_received.saturating_add(1); }
        }

        serial_println!("[dbus] MethodReturn serial={} {} -> {} ({} args)",
            reply_serial, responder_id, caller_id, args.len());
        true
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Signals
// ---------------------------------------------------------------------------

/// Subscribe to a signal.
pub fn subscribe_signal(
    subscriber_id: u32,
    interface_hash: u64,
    signal_hash: u64,
    source_filter: u32,
) -> bool {
    let mut guard = DBUS.lock();
    if let Some(ref mut state) = *guard {
        if state.subscriptions.len() >= MAX_SUBSCRIPTIONS {
            serial_println!("[dbus] ERROR: max signal subscriptions reached");
            return false;
        }
        state.subscriptions.push(SignalSubscription {
            subscriber_id,
            interface_hash,
            signal_hash,
            source_filter,
        });
        serial_println!("[dbus] Connection {} subscribed to signal {:#018X}.{:#018X}",
            subscriber_id, interface_hash, signal_hash);
        true
    } else {
        false
    }
}

/// Emit a signal. Returns number of subscribers notified.
pub fn emit_signal(
    sender_id: u32,
    interface_hash: u64,
    signal_hash: u64,
    args: Vec<Argument>,
    timestamp: u64,
) -> u32 {
    let mut guard = DBUS.lock();
    if let Some(ref mut state) = *guard {
        state.total_messages = state.total_messages.saturating_add(1);
        state.total_signals_emitted = state.total_signals_emitted.saturating_add(1);

        let mut notified = 0u32;
        let subs: Vec<u32> = state.subscriptions.iter()
            .filter(|s| {
                s.interface_hash == interface_hash &&
                s.signal_hash == signal_hash &&
                (s.source_filter == 0 || s.source_filter == sender_id)
            })
            .map(|s| s.subscriber_id)
            .collect();

        for sub_id in &subs {
            for conn in &mut state.connections {
                if conn.id == *sub_id && conn.active {
                    conn.messages_received = conn.messages_received.saturating_add(1);
                    notified += 1;
                    break;
                }
            }
        }

        for conn in &mut state.connections {
            if conn.id == sender_id { conn.messages_sent = conn.messages_sent.saturating_add(1); break; }
        }

        serial_println!("[dbus] Signal {:#018X}.{:#018X} from {} ({} subscribers, {} args)",
            interface_hash, signal_hash, sender_id, notified, args.len());
        notified
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

/// Get a property value. Returns (type_code, value).
pub fn get_property(
    connection_id: u32,
    path_hash: u64,
    interface_hash: u64,
    prop_hash: u64,
) -> Option<(u8, u64)> {
    let guard = DBUS.lock();
    if let Some(ref state) = *guard {
        for conn in &state.connections {
            if conn.id == connection_id {
                for obj in &conn.objects {
                    if obj.path_hash == path_hash {
                        for iface in &obj.interfaces {
                            if iface.name_hash == interface_hash {
                                for prop in &iface.properties {
                                    if prop.name_hash == prop_hash && prop.readable {
                                        let type_code = match prop.prop_type {
                                            ArgType::Byte => 0,
                                            ArgType::Int32 => 1,
                                            ArgType::Uint64 => 2,
                                            ArgType::Fixed => 3,
                                            ArgType::Boolean => 4,
                                            ArgType::StringHash => 5,
                                            ArgType::ObjectPath => 6,
                                            ArgType::ByteArray => 7,
                                        };
                                        return Some((type_code, prop.value));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

/// Set a property value. Returns true on success.
pub fn set_property(
    connection_id: u32,
    path_hash: u64,
    interface_hash: u64,
    prop_hash: u64,
    new_value: u64,
) -> bool {
    let mut guard = DBUS.lock();
    if let Some(ref mut state) = *guard {
        for conn in &mut state.connections {
            if conn.id == connection_id {
                for obj in &mut conn.objects {
                    if obj.path_hash == path_hash {
                        for iface in &mut obj.interfaces {
                            if iface.name_hash == interface_hash {
                                for prop in &mut iface.properties {
                                    if prop.name_hash == prop_hash && prop.writable {
                                        let old_value = prop.value;
                                        prop.value = new_value;
                                        serial_println!("[dbus] Property {:#018X} set: {} -> {}",
                                            prop_hash, old_value, new_value);
                                        return true;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Introspection
// ---------------------------------------------------------------------------

/// Introspect an object: returns list of (interface_hash, method_count, prop_count, signal_count).
pub fn introspect(connection_id: u32, path_hash: u64) -> Vec<(u64, usize, usize, usize)> {
    let guard = DBUS.lock();
    if let Some(ref state) = *guard {
        for conn in &state.connections {
            if conn.id == connection_id {
                for obj in &conn.objects {
                    if obj.path_hash == path_hash {
                        let mut result = Vec::new();
                        for iface in &obj.interfaces {
                            result.push((
                                iface.name_hash,
                                iface.methods.len(),
                                iface.properties.len(),
                                iface.signals.len(),
                            ));
                        }
                        return result;
                    }
                }
            }
        }
    }
    Vec::new()
}

/// List all method hashes on an interface.
pub fn list_methods(connection_id: u32, path_hash: u64, interface_hash: u64) -> Vec<u64> {
    let guard = DBUS.lock();
    if let Some(ref state) = *guard {
        for conn in &state.connections {
            if conn.id == connection_id {
                for obj in &conn.objects {
                    if obj.path_hash == path_hash {
                        for iface in &obj.interfaces {
                            if iface.name_hash == interface_hash {
                                return iface.methods.iter().map(|m| m.name_hash).collect();
                            }
                        }
                    }
                }
            }
        }
    }
    Vec::new()
}

// ---------------------------------------------------------------------------
// Timeout management
// ---------------------------------------------------------------------------

/// Expire pending calls that have exceeded their timeout. Returns number expired.
pub fn expire_pending(current_time: u64) -> u32 {
    let mut guard = DBUS.lock();
    if let Some(ref mut state) = *guard {
        let mut expired = 0u32;
        state.pending_calls.retain(|p| {
            let age = current_time.saturating_sub(p.timestamp);
            if age > p.timeout_ms as u64 {
                serial_println!("[dbus] Pending call serial={} timed out ({}ms)", p.serial, age);
                expired += 1;
                state.total_errors = state.total_errors.saturating_add(1);
                false
            } else {
                true
            }
        });
        expired
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

/// Get bus statistics: (connections, names, messages, calls, signals, errors).
pub fn bus_stats() -> (usize, usize, u64, u64, u64, u64) {
    let guard = DBUS.lock();
    if let Some(ref state) = *guard {
        let active = state.connections.iter().filter(|c| c.active).count();
        (
            active,
            state.bus_names.len(),
            state.total_messages,
            state.total_method_calls,
            state.total_signals_emitted,
            state.total_errors,
        )
    } else {
        (0, 0, 0, 0, 0, 0)
    }
}

/// List all owned bus names as (name_hash, owner_id).
pub fn list_names() -> Vec<(u64, u32)> {
    let guard = DBUS.lock();
    if let Some(ref state) = *guard {
        state.bus_names.iter()
            .filter(|n| n.owner_id != 0)
            .map(|n| (n.name_hash, n.owner_id))
            .collect()
    } else {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    let mut guard = DBUS.lock();
    *guard = Some(DBusState::new());
    if let Some(ref mut state) = *guard {
        state.initialized = true;
    }
    serial_println!("    [integration] D-Bus IPC initialized (methods, signals, properties, introspection)");
}
