/// inotify — filesystem event notification (Linux-compatible)
///
/// Provides kernel→userspace notification of filesystem events.
/// Each consumer creates an InotifyInstance and registers watches on paths.
/// When a matching VFS operation occurs, an event is enqueued into the
/// instance's ring buffer and can be retrieved via inotify_read().
///
/// Design constraints (bare-metal kernel rules):
///   - no_std — no standard library
///   - No heap — no Vec / Box / String — all fixed-size static arrays
///   - No float casts (as f32 / as f64)
///   - Saturating arithmetic on counters, wrapping_add on sequences
///   - No panic — all fallible paths return -1 / None / false
///   - MMIO via read_volatile / write_volatile only
///
/// Path prefix matching: a watch on "/foo" triggers for events on "/foo" and
/// "/foo/bar" (any path whose prefix equals the watched path followed by '/'
/// or an exact match).
///
/// Ring buffer: ring_head is the next-write index; ring_tail is the next-read
/// index.  When head == tail the ring is empty.  When the ring is full,
/// incoming events set IN_Q_OVERFLOW on the most recent event and are dropped.
///
/// Inspired by: Linux fs/notify/inotify (fs/inotify.c). All code is original.
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Capacity limits
// ---------------------------------------------------------------------------

pub const MAX_INOTIFY_INSTANCES: usize = 16;
pub const MAX_INOTIFY_WATCHES: usize = 128;
pub const MAX_INOTIFY_EVENTS: usize = 256;
pub const INOTIFY_EVENT_BUF: usize = 128;

// ---------------------------------------------------------------------------
// Event mask constants (Linux inotify.h compatible)
// ---------------------------------------------------------------------------

pub const IN_ACCESS: u32 = 0x0000_0001; // File accessed
pub const IN_MODIFY: u32 = 0x0000_0002; // File modified
pub const IN_ATTRIB: u32 = 0x0000_0004; // Metadata changed
pub const IN_CLOSE_WRITE: u32 = 0x0000_0008; // Writable file closed
pub const IN_CLOSE_NOWRITE: u32 = 0x0000_0010; // Unwritable file closed
pub const IN_OPEN: u32 = 0x0000_0020; // File opened
pub const IN_MOVED_FROM: u32 = 0x0000_0040; // File renamed from watched dir
pub const IN_MOVED_TO: u32 = 0x0000_0080; // File renamed into watched dir
pub const IN_CREATE: u32 = 0x0000_0100; // File/dir created in watched dir
pub const IN_DELETE: u32 = 0x0000_0200; // File/dir deleted in watched dir
pub const IN_DELETE_SELF: u32 = 0x0000_0400; // Watched file/dir deleted
pub const IN_MOVE_SELF: u32 = 0x0000_0800; // Watched file/dir moved
pub const IN_UNMOUNT: u32 = 0x0000_2000; // Filesystem unmounted
pub const IN_Q_OVERFLOW: u32 = 0x0000_4000; // Event queue overflowed
pub const IN_IGNORED: u32 = 0x0000_8000; // Watch was removed
pub const IN_ISDIR: u32 = 0x4000_0000; // Event against a directory
pub const IN_ALL_EVENTS: u32 = 0x0000_0FFF; // All file events

// ---------------------------------------------------------------------------
// InotifyEvent
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct InotifyEvent {
    pub wd: i32,                       // Watch descriptor (-1 if none)
    pub mask: u32,                     // Event type bitmask
    pub cookie: u32,                   // Rename pairing cookie
    pub name: [u8; INOTIFY_EVENT_BUF], // Filename (NUL-terminated) or empty
    pub name_len: u32,                 // Valid bytes in `name` (excl. NUL)
}

impl InotifyEvent {
    pub const fn empty() -> Self {
        InotifyEvent {
            wd: 0,
            mask: 0,
            cookie: 0,
            name: [0u8; INOTIFY_EVENT_BUF],
            name_len: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// InotifyWatch
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct InotifyWatch {
    pub wd: i32,         // Watch descriptor (>= 1, unique per instance)
    pub inst_id: u32,    // Owning instance ID
    pub path: [u8; 256], // Watched path (NUL-terminated)
    pub path_len: u16,   // Valid bytes in `path` (excl. NUL)
    pub mask: u32,       // Events to watch
    pub active: bool,
}

impl InotifyWatch {
    pub const fn empty() -> Self {
        InotifyWatch {
            wd: 0,
            inst_id: 0,
            path: [0u8; 256],
            path_len: 0,
            mask: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// InotifyInstance
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct InotifyInstance {
    pub id: u32,
    pub fd: i32,
    pub event_ring: [InotifyEvent; MAX_INOTIFY_EVENTS],
    pub ring_head: u32, // next-write index (wrapping mod MAX_INOTIFY_EVENTS)
    pub ring_tail: u32, // next-read  index (wrapping mod MAX_INOTIFY_EVENTS)
    pub active: bool,
}

impl InotifyInstance {
    pub const fn empty() -> Self {
        InotifyInstance {
            id: 0,
            fd: 0,
            event_ring: [InotifyEvent::empty(); MAX_INOTIFY_EVENTS],
            ring_head: 0,
            ring_tail: 0,
            active: false,
        }
    }

    /// Number of events currently queued.
    fn pending_count(&self) -> usize {
        let h = self.ring_head as usize;
        let t = self.ring_tail as usize;
        if h >= t {
            h - t
        } else {
            MAX_INOTIFY_EVENTS - t + h
        }
    }

    /// True when the ring has no room for another event (keep one slot empty).
    fn is_full(&self) -> bool {
        self.pending_count() >= MAX_INOTIFY_EVENTS.saturating_sub(1)
    }

    /// Enqueue an event.  On overflow, sets IN_Q_OVERFLOW on the last slot.
    fn enqueue(&mut self, ev: InotifyEvent) {
        if self.is_full() {
            // Mark overflow on the most recently written slot
            if self.ring_head != self.ring_tail {
                let last_written = (self.ring_head as usize).wrapping_add(MAX_INOTIFY_EVENTS - 1)
                    % MAX_INOTIFY_EVENTS;
                self.event_ring[last_written].mask |= IN_Q_OVERFLOW;
            }
            return;
        }
        let idx = self.ring_head as usize;
        self.event_ring[idx] = ev;
        self.ring_head = (self.ring_head.wrapping_add(1)) % (MAX_INOTIFY_EVENTS as u32);
    }

    /// Dequeue up to `max` events into `out`.  Returns the count dequeued.
    fn dequeue(&mut self, out: &mut [InotifyEvent], max: usize) -> usize {
        let mut count = 0usize;
        while count < max && count < out.len() && self.ring_tail != self.ring_head {
            let idx = self.ring_tail as usize;
            out[count] = self.event_ring[idx];
            self.ring_tail = (self.ring_tail.wrapping_add(1)) % (MAX_INOTIFY_EVENTS as u32);
            count = count.saturating_add(1);
        }
        count
    }
}

// ---------------------------------------------------------------------------
// Static tables
// ---------------------------------------------------------------------------

const EMPTY_INSTANCE: InotifyInstance = InotifyInstance::empty();
static INOTIFY_INSTS: Mutex<[InotifyInstance; MAX_INOTIFY_INSTANCES]> =
    Mutex::new([EMPTY_INSTANCE; MAX_INOTIFY_INSTANCES]);

const EMPTY_WATCH: InotifyWatch = InotifyWatch::empty();
static INOTIFY_WATCHES: Mutex<[InotifyWatch; MAX_INOTIFY_WATCHES]> =
    Mutex::new([EMPTY_WATCH; MAX_INOTIFY_WATCHES]);

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Copy up to 255 bytes of `src` into `dst`, NUL-terminating the result.
/// Returns the number of bytes copied (not counting the NUL).
fn copy_path(dst: &mut [u8; 256], src: &[u8]) -> u16 {
    let len = src.len().min(255);
    dst[..len].copy_from_slice(&src[..len]);
    dst[len] = 0;
    len as u16
}

/// Returns `true` if `event_path` falls under the watch at `watch_path[..watch_len]`.
///
/// Matches when:
///   - Exact: event_path == watch_path
///   - Prefix: event_path starts with watch_path + b'/'
fn path_matches(watch_path: &[u8; 256], watch_len: usize, event_path: &[u8]) -> bool {
    let ep = event_path;
    if ep.len() < watch_len {
        return false;
    }
    if &ep[..watch_len] != &watch_path[..watch_len] {
        return false;
    }
    if ep.len() == watch_len {
        return true; // exact
    }
    ep.get(watch_len) == Some(&b'/') // prefix + separator
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new inotify instance.
///
/// Returns the instance ID on success, or `None` if the table is full.
/// The `fd` field is set to `id + 1000` (Linux convention mimicry).
pub fn inotify_init() -> Option<u32> {
    let mut insts = INOTIFY_INSTS.lock();
    for (i, slot) in insts.iter_mut().enumerate() {
        if !slot.active {
            unsafe {
                core::ptr::write_bytes(slot as *mut InotifyInstance, 0, 1);
            }
            slot.id = i as u32;
            slot.fd = i as i32 + 1000;
            slot.active = true;
            return Some(i as u32);
        }
    }
    None
}

/// Destroy an inotify instance and remove all its watches.
///
/// Returns `true` on success.
pub fn inotify_close(inst_id: u32) -> bool {
    // Remove all watches belonging to this instance first.
    {
        let mut watches = INOTIFY_WATCHES.lock();
        for w in watches.iter_mut() {
            if w.active && w.inst_id == inst_id {
                unsafe {
                    core::ptr::write_bytes(w as *mut InotifyWatch, 0, 1);
                }
            }
        }
    }
    let mut insts = INOTIFY_INSTS.lock();
    let idx = inst_id as usize;
    if idx >= MAX_INOTIFY_INSTANCES || !insts[idx].active {
        return false;
    }
    unsafe {
        core::ptr::write_bytes(&mut insts[idx] as *mut InotifyInstance, 0, 1);
    }
    true
}

/// Add or update a watch on `path` for events in `mask`.
///
/// If a watch for this (instance, path) pair already exists, its mask is
/// replaced.  Returns the watch descriptor (wd >= 1) on success, or -1 on
/// error (instance not found, watch table full, path too long).
pub fn inotify_add_watch(inst_id: u32, path: &[u8], mask: u32) -> i32 {
    let inst_idx = inst_id as usize;
    if inst_idx >= MAX_INOTIFY_INSTANCES {
        return -1;
    }
    // Verify instance is active (read-only check; brief lock then release).
    {
        let insts = INOTIFY_INSTS.lock();
        if !insts[inst_idx].active {
            return -1;
        }
    }

    let path_len_u16 = path.len().min(255) as u16;
    let mut watches = INOTIFY_WATCHES.lock();

    // Update existing watch for this (inst_id, path) pair.
    for w in watches.iter_mut() {
        if w.active
            && w.inst_id == inst_id
            && w.path_len == path_len_u16
            && &w.path[..path_len_u16 as usize] == &path[..path_len_u16 as usize]
        {
            w.mask = mask;
            return w.wd;
        }
    }

    // Allocate a new watch slot.  wd = slot_index + 1.
    for (i, slot) in watches.iter_mut().enumerate() {
        if !slot.active {
            slot.active = true;
            slot.inst_id = inst_id;
            slot.wd = (i + 1) as i32;
            slot.mask = mask;
            slot.path_len = copy_path(&mut slot.path, path);
            return slot.wd;
        }
    }

    -1 // watch table full
}

/// Remove a watch descriptor from an inotify instance.
///
/// Returns `true` on success, `false` if the (instance, wd) pair was not found.
pub fn inotify_rm_watch(inst_id: u32, wd: i32) -> bool {
    let mut watches = INOTIFY_WATCHES.lock();
    for slot in watches.iter_mut() {
        if slot.active && slot.inst_id == inst_id && slot.wd == wd {
            unsafe {
                core::ptr::write_bytes(slot as *mut InotifyWatch, 0, 1);
            }
            return true;
        }
    }
    false
}

/// Notify all matching watches of a filesystem event.
///
/// Called by VFS code whenever a file operation completes.
///
/// `path`   — full path of the affected file/directory
/// `mask`   — one of the IN_* event constants
/// `cookie` — rename cookie (non-zero only for IN_MOVED_FROM / IN_MOVED_TO)
/// `name`   — filename component (last segment); may be empty
pub fn inotify_notify(path: &[u8], mask: u32, cookie: u32, name: &[u8]) {
    // Phase 1: collect matching (inst_id, wd) pairs under the WATCHES lock.
    // We avoid holding both locks simultaneously to prevent lock-order issues.
    let mut matched: [(u32, i32); MAX_INOTIFY_WATCHES] = [(0, 0); MAX_INOTIFY_WATCHES];
    let mut match_count: usize = 0;
    {
        let watches = INOTIFY_WATCHES.lock();
        for w in watches.iter() {
            if !w.active || (w.mask & mask) == 0 {
                continue;
            }
            if path_matches(&w.path, w.path_len as usize, path) {
                if match_count < MAX_INOTIFY_WATCHES {
                    matched[match_count] = (w.inst_id, w.wd);
                    match_count = match_count.saturating_add(1);
                }
            }
        }
    }

    if match_count == 0 {
        return;
    }

    // Build the event payload once.
    let mut ev_template = InotifyEvent::empty();
    ev_template.mask = mask;
    ev_template.cookie = cookie;
    let name_copy_len = name.len().min(INOTIFY_EVENT_BUF.saturating_sub(1));
    ev_template.name[..name_copy_len].copy_from_slice(&name[..name_copy_len]);
    ev_template.name[name_copy_len] = 0; // NUL-terminate
    ev_template.name_len = name_copy_len as u32;

    // Phase 2: enqueue into each matched instance under the INSTS lock.
    let mut insts = INOTIFY_INSTS.lock();
    for k in 0..match_count {
        let (inst_id, wd) = matched[k];
        let idx = inst_id as usize;
        if idx >= MAX_INOTIFY_INSTANCES || !insts[idx].active {
            continue;
        }
        let mut ev = ev_template;
        ev.wd = wd;
        insts[idx].enqueue(ev);
    }
}

/// Dequeue up to `max` pending events from instance `inst_id` into `out`.
///
/// Returns the number of events written into `out`.
pub fn inotify_read(inst_id: u32, out: &mut [InotifyEvent], max: usize) -> usize {
    let idx = inst_id as usize;
    if idx >= MAX_INOTIFY_INSTANCES {
        return 0;
    }
    let mut insts = INOTIFY_INSTS.lock();
    if !insts[idx].active {
        return 0;
    }
    insts[idx].dequeue(out, max)
}

/// Return the count of pending events in instance `inst_id`'s ring.
pub fn inotify_pending(inst_id: u32) -> usize {
    let idx = inst_id as usize;
    if idx >= MAX_INOTIFY_INSTANCES {
        return 0;
    }
    let insts = INOTIFY_INSTS.lock();
    if !insts[idx].active {
        return 0;
    }
    insts[idx].pending_count()
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the inotify subsystem.
///
/// Clears both the instance table and the watch table.
pub fn init() {
    {
        let mut insts = INOTIFY_INSTS.lock();
        for slot in insts.iter_mut() {
            unsafe {
                core::ptr::write_bytes(slot as *mut InotifyInstance, 0, 1);
            }
        }
    }
    {
        let mut watches = INOTIFY_WATCHES.lock();
        for slot in watches.iter_mut() {
            unsafe {
                core::ptr::write_bytes(slot as *mut InotifyWatch, 0, 1);
            }
        }
    }
    serial_println!("[inotify] inotify filesystem notifications initialized");
}
