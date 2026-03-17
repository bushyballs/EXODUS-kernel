/// Hoags IPC — inter-process communication
///
/// Mechanisms:
///   1. Pipes: unidirectional byte streams (like Unix pipes)
///   2. Message queues: typed message passing between processes
///   3. Shared memory: zero-copy data sharing with synchronization
///   4. Signals: asynchronous process notifications
///   5. Binder: Android-style object RPC
///   6. Unix sockets: AF_UNIX stream/datagram/seqpacket
///   7. POSIX message queues: priority-ordered message passing
///   8. Eventfd: lightweight event notification
///   9. Semaphores: System V counting semaphores
///  10. Futex: fast userspace mutex support
///
/// Inspired by: Unix pipes, Mach ports (message passing),
/// QNX (message-based IPC), L4 (fast IPC). All code is original.
use crate::{serial_print, serial_println};
pub mod binder;
pub mod epoll;
pub mod eventfd;
pub mod futex;
pub mod memfd;
pub mod message;
pub mod mqueue;
pub mod pipe;
pub mod semaphore;
pub mod shm;
pub mod signal;
pub mod signalfd;
pub mod timerfd;
pub mod unix_socket;

pub fn init() {
    pipe::init();
    epoll::init();
    message::init();
    shm::init();
    signal::init();
    binder::init();
    unix_socket::init();
    mqueue::init();
    eventfd::init();
    timerfd::init();
    signalfd::init();
    semaphore::init();
    futex::init();
    memfd::init();
    serial_println!("  IPC: pipes, epoll, messages, shm, signals, binder, unix sockets, mqueue, eventfd, timerfd, semaphores, futex, memfd");
}
