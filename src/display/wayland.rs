use crate::sync::Mutex;
/// Hoags Wayland — display protocol for client-server compositing
///
/// Implements a Wayland-inspired protocol where:
///   - The compositor is the display server
///   - Clients create surfaces and attach buffers
///   - Compositor composites all surfaces and presents
///   - Input events are routed to the focused surface
///
/// Uses shared memory buffers for zero-copy rendering.
/// Protocol is message-based over IPC message queues.
///
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

static DISPLAY_SERVER: Mutex<Option<DisplayServer>> = Mutex::new(None);

/// Wayland-like object IDs
pub type ObjectId = u32;

/// Protocol message opcodes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WlOpcode {
    // Display
    GetRegistry,
    Sync,
    // Registry
    Bind,
    Global,
    // Compositor
    CreateSurface,
    // Surface
    Attach,
    Commit,
    Damage,
    SetInputRegion,
    // Buffer
    CreateShmPool,
    CreateBuffer,
    Destroy,
    // Seat (input)
    GetPointer,
    GetKeyboard,
    GetTouch,
    // Pointer events
    PointerEnter,
    PointerLeave,
    PointerMotion,
    PointerButton,
    // Keyboard events
    KeyboardKeymap,
    KeyboardEnter,
    KeyboardLeave,
    KeyboardKey,
    KeyboardModifiers,
    // Shell surface
    SetTitle,
    SetAppId,
    Move,
    Resize,
    Maximize,
    Minimize,
    Close,
}

/// A protocol message
#[derive(Debug, Clone)]
pub struct WlMessage {
    pub sender: ObjectId,
    pub opcode: WlOpcode,
    pub args: Vec<WlArg>,
}

/// Message argument types
#[derive(Debug, Clone)]
pub enum WlArg {
    Int(i32),
    Uint(u32),
    Fixed(i32), // 24.8 fixed point
    String(String),
    ObjectId(ObjectId),
    NewId(ObjectId),
    Bytes(Vec<u8>),
    Fd(u32),
}

/// A display client connection
pub struct WlClient {
    pub id: u32,
    pub pid: u32,
    pub objects: BTreeMap<ObjectId, WlObjectType>,
    pub next_id: ObjectId,
    pub surfaces: Vec<ObjectId>,
}

impl WlClient {
    pub fn new(id: u32, pid: u32) -> Self {
        WlClient {
            id,
            pid,
            objects: BTreeMap::new(),
            next_id: 1,
            surfaces: Vec::new(),
        }
    }

    pub fn allocate_id(&mut self) -> ObjectId {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id
    }
}

/// Types of Wayland objects
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WlObjectType {
    Display,
    Registry,
    Compositor,
    Surface,
    Buffer,
    ShmPool,
    Seat,
    Pointer,
    Keyboard,
    Touch,
    ShellSurface,
}

/// A surface (window content)
pub struct WlSurface {
    pub id: ObjectId,
    pub client_id: u32,
    pub width: u32,
    pub height: u32,
    pub buffer: Option<ObjectId>,
    pub title: String,
    pub app_id: String,
    pub committed: bool,
    pub damage: Vec<(i32, i32, u32, u32)>, // damaged regions
    pub input_region: Option<(i32, i32, u32, u32)>,
}

/// A shared memory pool
pub struct WlShmPool {
    pub id: ObjectId,
    pub client_id: u32,
    pub data: Vec<u8>,
    pub size: usize,
}

/// A buffer backed by shared memory
pub struct WlBuffer {
    pub id: ObjectId,
    pub pool_id: ObjectId,
    pub offset: usize,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: PixelFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Argb8888,
    Xrgb8888,
    Rgb888,
    Rgb565,
}

impl PixelFormat {
    pub fn bytes_per_pixel(&self) -> usize {
        match self {
            PixelFormat::Argb8888 | PixelFormat::Xrgb8888 => 4,
            PixelFormat::Rgb888 => 3,
            PixelFormat::Rgb565 => 2,
        }
    }
}

/// The display server
pub struct DisplayServer {
    pub clients: BTreeMap<u32, WlClient>,
    pub surfaces: BTreeMap<ObjectId, WlSurface>,
    pub shm_pools: BTreeMap<ObjectId, WlShmPool>,
    pub buffers: BTreeMap<ObjectId, WlBuffer>,
    pub next_client_id: u32,
    pub next_global_id: ObjectId,
    pub focused_surface: Option<ObjectId>,
}

impl DisplayServer {
    pub fn new() -> Self {
        DisplayServer {
            clients: BTreeMap::new(),
            surfaces: BTreeMap::new(),
            shm_pools: BTreeMap::new(),
            buffers: BTreeMap::new(),
            next_client_id: 1,
            next_global_id: 1000,
            focused_surface: None,
        }
    }

    /// Connect a new client
    pub fn connect(&mut self, pid: u32) -> u32 {
        let id = self.next_client_id;
        self.next_client_id = self.next_client_id.saturating_add(1);
        self.clients.insert(id, WlClient::new(id, pid));
        serial_println!(
            "    [wayland] Client connected: PID {} -> client {}",
            pid,
            id
        );
        id
    }

    /// Disconnect a client
    pub fn disconnect(&mut self, client_id: u32) {
        // Clean up surfaces
        let surface_ids: Vec<ObjectId> = self
            .surfaces
            .iter()
            .filter(|(_, s)| s.client_id == client_id)
            .map(|(id, _)| *id)
            .collect();
        for id in surface_ids {
            self.surfaces.remove(&id);
        }
        self.clients.remove(&client_id);
        serial_println!("    [wayland] Client disconnected: {}", client_id);
    }

    /// Process a client message
    pub fn process_message(&mut self, client_id: u32, msg: &WlMessage) {
        match msg.opcode {
            WlOpcode::CreateSurface => {
                let surface_id = self.next_global_id;
                self.next_global_id = self.next_global_id.saturating_add(1);

                self.surfaces.insert(
                    surface_id,
                    WlSurface {
                        id: surface_id,
                        client_id,
                        width: 0,
                        height: 0,
                        buffer: None,
                        title: String::new(),
                        app_id: String::new(),
                        committed: false,
                        damage: Vec::new(),
                        input_region: None,
                    },
                );

                if let Some(client) = self.clients.get_mut(&client_id) {
                    client.surfaces.push(surface_id);
                }
            }
            WlOpcode::Attach => {
                if let (Some(WlArg::ObjectId(surface_id)), Some(WlArg::ObjectId(buffer_id))) =
                    (msg.args.get(0), msg.args.get(1))
                {
                    if let Some(surface) = self.surfaces.get_mut(surface_id) {
                        surface.buffer = Some(*buffer_id);
                    }
                }
            }
            WlOpcode::Commit => {
                if let Some(WlArg::ObjectId(surface_id)) = msg.args.first() {
                    if let Some(surface) = self.surfaces.get_mut(surface_id) {
                        surface.committed = true;
                    }
                }
            }
            WlOpcode::SetTitle => {
                if let (Some(WlArg::ObjectId(surface_id)), Some(WlArg::String(title))) =
                    (msg.args.get(0), msg.args.get(1))
                {
                    if let Some(surface) = self.surfaces.get_mut(surface_id) {
                        surface.title = title.clone();
                    }
                }
            }
            WlOpcode::Close => {
                if let Some(WlArg::ObjectId(surface_id)) = msg.args.first() {
                    self.surfaces.remove(surface_id);
                }
            }
            _ => {}
        }
    }

    /// Get all committed surfaces for compositing
    pub fn committed_surfaces(&self) -> Vec<&WlSurface> {
        self.surfaces.values().filter(|s| s.committed).collect()
    }

    /// Set keyboard focus
    pub fn set_focus(&mut self, surface_id: ObjectId) {
        self.focused_surface = Some(surface_id);
    }
}

pub fn init() {
    *DISPLAY_SERVER.lock() = Some(DisplayServer::new());
    serial_println!("    [wayland] Display protocol server initialized");
}
