/// Neural-bus syscall handlers for Genesis
///
/// Implements: sys_neural_pulse, sys_neural_poll
///
/// These syscalls bridge userspace programs into the kernel neural bus,
/// allowing applications to emit and consume typed NeuralSignal messages
/// that flow through the AI subsystem.
///
/// All code is original.
use super::errno;

// ─── SYS_NEURAL_PULSE ─────────────────────────────────────────────────────────

/// SYS_NEURAL_PULSE: Emit a signal from userspace into the kernel neural bus.
///
/// Args:
///   kind_idx (RDI): SignalKind discriminant (see neural_bus::SignalKind)
///   source   (RSI): source node ID (u16)
///   strength (RDX): signal strength (i32)
///   val      (R10): integer payload (i64)
pub fn sys_neural_pulse(kind_idx: u32, source: u16, strength: i32, val: i64) -> u64 {
    use crate::neural_bus::{NeuralSignal, SignalKind};

    let kind = match kind_idx {
        0 => SignalKind::AppLaunch,
        1 => SignalKind::AppSwitch,
        5 => SignalKind::TextInput,
        6 => SignalKind::VoiceCommand,
        7 => SignalKind::SearchQuery,
        21 => SignalKind::AnomalyAlert,
        23 => SignalKind::ContextShift,
        _ => SignalKind::Heartbeat,
    };

    let sig = NeuralSignal::new(kind, source, strength).with_int(val);
    crate::neural_bus::emit(sig);
    0
}

// ─── SYS_NEURAL_POLL ──────────────────────────────────────────────────────────

/// SYS_NEURAL_POLL: Retrieve the next pending signal from the kernel neural bus.
///
/// Writes a 32-byte serialized signal record into the user buffer:
///   [0..4]   kind (u32 discriminant)
///   [4..6]   source_node (u16)
///   [6..8]   target_node (u16)
///   [8..12]  strength (i32)
///   [12..20] timestamp (u64 ms)
///   [20..28] integer payload (i64; 0 for non-integer payloads)
///
/// Returns: 1 if a signal was written, 0 if the ring was empty.
pub fn sys_neural_poll(buf_ptr: *mut u8) -> u64 {
    if buf_ptr.is_null() {
        return errno::EINVAL;
    }

    use crate::neural_bus::{SignalPayload, BUS};

    let maybe_sig = BUS.lock().signal_ring.pop();

    if let Some(sig) = maybe_sig {
        let out = unsafe { core::slice::from_raw_parts_mut(buf_ptr, 32) };
        out[0..4].copy_from_slice(&(sig.kind as u32).to_le_bytes());
        out[4..6].copy_from_slice(&sig.source_node.to_le_bytes());
        out[6..8].copy_from_slice(&sig.target_node.to_le_bytes());
        out[8..12].copy_from_slice(&sig.strength.to_le_bytes());
        out[12..20].copy_from_slice(&sig.timestamp.to_le_bytes());

        let payload_val = match sig.payload {
            SignalPayload::Integer(v) => v,
            _ => 0,
        };
        out[20..28].copy_from_slice(&payload_val.to_le_bytes());
        1
    } else {
        0
    }
}
