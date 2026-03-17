use crate::sync::Mutex;
/// Desktop layout customization for Genesis
///
/// Grid and free-form icon placement, panel positions,
/// taskbar configuration, desktop zones, snap regions,
/// multi-monitor layout management.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Q16 fixed-point helpers (16 fractional bits)
// ---------------------------------------------------------------------------

const Q16_SHIFT: i32 = 16;
const Q16_ONE: i32 = 1 << Q16_SHIFT;
const Q16_HALF: i32 = Q16_ONE >> 1;

fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> Q16_SHIFT) as i32
}

fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 {
        return 0;
    }
    (((a as i64) << Q16_SHIFT) / (b as i64)) as i32
}

fn q16_from_int(v: i32) -> i32 {
    v << Q16_SHIFT
}

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub enum LayoutMode {
    Grid,
    FreeForm,
    Hybrid,
}

#[derive(Clone, Copy, PartialEq)]
pub enum PanelPosition {
    Top,
    Bottom,
    Left,
    Right,
}

#[derive(Clone, Copy, PartialEq)]
pub enum TaskbarStyle {
    Full,
    Centered,
    Compact,
    AutoHide,
    Floating,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ZoneType {
    Primary,
    Secondary,
    Widget,
    QuickLaunch,
    StatusArea,
}

#[derive(Clone, Copy, PartialEq)]
pub enum SnapEdge {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    LeftHalf,
    RightHalf,
    TopHalf,
    BottomHalf,
    Center,
    Maximize,
}

#[derive(Clone, Copy, PartialEq)]
pub enum IconSize {
    Small,
    Medium,
    Large,
    ExtraLarge,
}

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct DesktopIcon {
    id: u32,
    app_id: u32,
    grid_col: u16,
    grid_row: u16,
    free_x: i32, // Q16 fractional screen position
    free_y: i32, // Q16 fractional screen position
    size: IconSize,
    pinned: bool,
    sort_order: u16,
    label_hash: u64,
}

#[derive(Clone, Copy)]
struct Panel {
    id: u32,
    position: PanelPosition,
    thickness: u16,
    auto_hide: bool,
    opacity_q16: i32, // 0 = transparent, Q16_ONE = opaque
    color: u32,
    show_clock: bool,
    show_battery: bool,
    show_network: bool,
    show_tray: bool,
    monitor_id: u8,
}

#[derive(Clone, Copy)]
struct TaskbarConfig {
    style: TaskbarStyle,
    position: PanelPosition,
    icon_size: IconSize,
    show_labels: bool,
    show_badges: bool,
    group_windows: bool,
    max_pinned: u8,
    corner_radius: u16,
    height: u16,
    blur_behind: bool,
    color: u32,
    opacity_q16: i32,
}

#[derive(Clone, Copy)]
struct DesktopZone {
    id: u32,
    zone_type: ZoneType,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    monitor_id: u8,
    snap_enabled: bool,
    padding: u16,
}

#[derive(Clone, Copy)]
struct SnapRegion {
    id: u32,
    edge: SnapEdge,
    x_q16: i32,
    y_q16: i32,
    w_q16: i32,
    h_q16: i32,
    active: bool,
    gap: u16,
    monitor_id: u8,
}

#[derive(Clone, Copy)]
struct GridConfig {
    columns: u16,
    rows: u16,
    cell_padding: u16,
    margin_top: u16,
    margin_bottom: u16,
    margin_left: u16,
    margin_right: u16,
    auto_arrange: bool,
    sort_alphabetical: bool,
}

#[derive(Clone, Copy)]
struct MonitorLayout {
    id: u8,
    width: u16,
    height: u16,
    offset_x: i32,
    offset_y: i32,
    scale_q16: i32,
    primary: bool,
    rotation: u16,
}

// ---------------------------------------------------------------------------
// Manager
// ---------------------------------------------------------------------------

struct DesktopLayoutManager {
    mode: LayoutMode,
    icons: Vec<DesktopIcon>,
    panels: Vec<Panel>,
    taskbar: TaskbarConfig,
    zones: Vec<DesktopZone>,
    snap_regions: Vec<SnapRegion>,
    grid: GridConfig,
    monitors: Vec<MonitorLayout>,
    next_icon_id: u32,
    next_panel_id: u32,
    next_zone_id: u32,
    next_snap_id: u32,
    wallpaper_per_monitor: bool,
    desktop_locked: bool,
}

static LAYOUT: Mutex<Option<DesktopLayoutManager>> = Mutex::new(None);

impl DesktopLayoutManager {
    fn new() -> Self {
        let default_taskbar = TaskbarConfig {
            style: TaskbarStyle::Centered,
            position: PanelPosition::Bottom,
            icon_size: IconSize::Medium,
            show_labels: true,
            show_badges: true,
            group_windows: true,
            max_pinned: 12,
            corner_radius: 8,
            height: 48,
            blur_behind: true,
            color: 0xFF1A1A2E,
            opacity_q16: q16_mul(Q16_ONE, q16_from_int(90)) / 100,
        };

        let default_grid = GridConfig {
            columns: 8,
            rows: 6,
            cell_padding: 4,
            margin_top: 32,
            margin_bottom: 64,
            margin_left: 16,
            margin_right: 16,
            auto_arrange: true,
            sort_alphabetical: false,
        };

        DesktopLayoutManager {
            mode: LayoutMode::Grid,
            icons: Vec::new(),
            panels: Vec::new(),
            taskbar: default_taskbar,
            zones: Vec::new(),
            snap_regions: Vec::new(),
            grid: default_grid,
            monitors: Vec::new(),
            next_icon_id: 1,
            next_panel_id: 1,
            next_zone_id: 1,
            next_snap_id: 1,
            wallpaper_per_monitor: false,
            desktop_locked: false,
        }
    }

    fn add_icon(&mut self, app_id: u32, label_hash: u64) -> u32 {
        if self.icons.len() >= 256 {
            return 0;
        }
        let id = self.next_icon_id;
        self.next_icon_id = self.next_icon_id.saturating_add(1);

        let (col, row) = self.next_free_grid_cell();
        let icon = DesktopIcon {
            id,
            app_id,
            grid_col: col,
            grid_row: row,
            free_x: q16_div(
                q16_from_int(col as i32),
                q16_from_int(self.grid.columns as i32),
            ),
            free_y: q16_div(
                q16_from_int(row as i32),
                q16_from_int(self.grid.rows as i32),
            ),
            size: IconSize::Medium,
            pinned: false,
            sort_order: self.icons.len() as u16,
            label_hash,
        };
        self.icons.push(icon);
        id
    }

    fn next_free_grid_cell(&self) -> (u16, u16) {
        for row in 0..self.grid.rows {
            for col in 0..self.grid.columns {
                let occupied = self
                    .icons
                    .iter()
                    .any(|i| i.grid_col == col && i.grid_row == row);
                if !occupied {
                    return (col, row);
                }
            }
        }
        (0, 0)
    }

    fn move_icon_grid(&mut self, icon_id: u32, col: u16, row: u16) -> bool {
        if self.desktop_locked {
            return false;
        }
        if col >= self.grid.columns || row >= self.grid.rows {
            return false;
        }

        let occupied = self
            .icons
            .iter()
            .any(|i| i.id != icon_id && i.grid_col == col && i.grid_row == row);
        if occupied {
            return false;
        }

        if let Some(icon) = self.icons.iter_mut().find(|i| i.id == icon_id) {
            icon.grid_col = col;
            icon.grid_row = row;
            icon.free_x = q16_div(
                q16_from_int(col as i32),
                q16_from_int(self.grid.columns as i32),
            );
            icon.free_y = q16_div(
                q16_from_int(row as i32),
                q16_from_int(self.grid.rows as i32),
            );
            return true;
        }
        false
    }

    fn move_icon_free(&mut self, icon_id: u32, x_q16: i32, y_q16: i32) -> bool {
        if self.desktop_locked {
            return false;
        }
        if self.mode == LayoutMode::Grid {
            return false;
        }

        if let Some(icon) = self.icons.iter_mut().find(|i| i.id == icon_id) {
            icon.free_x = x_q16.clamp(0, Q16_ONE);
            icon.free_y = y_q16.clamp(0, Q16_ONE);
            return true;
        }
        false
    }

    fn remove_icon(&mut self, icon_id: u32) -> bool {
        let len_before = self.icons.len();
        self.icons.retain(|i| i.id != icon_id);
        self.icons.len() < len_before
    }

    fn pin_icon(&mut self, icon_id: u32, pinned: bool) -> bool {
        if let Some(icon) = self.icons.iter_mut().find(|i| i.id == icon_id) {
            icon.pinned = pinned;
            return true;
        }
        false
    }

    fn add_panel(&mut self, position: PanelPosition, monitor_id: u8) -> u32 {
        if self.panels.len() >= 8 {
            return 0;
        }
        let id = self.next_panel_id;
        self.next_panel_id = self.next_panel_id.saturating_add(1);

        let panel = Panel {
            id,
            position,
            thickness: 32,
            auto_hide: false,
            opacity_q16: Q16_ONE,
            color: 0xFF202040,
            show_clock: true,
            show_battery: true,
            show_network: true,
            show_tray: true,
            monitor_id,
        };
        self.panels.push(panel);
        id
    }

    fn configure_panel(
        &mut self,
        panel_id: u32,
        auto_hide: bool,
        thickness: u16,
        opacity_q16: i32,
    ) -> bool {
        if let Some(p) = self.panels.iter_mut().find(|p| p.id == panel_id) {
            p.auto_hide = auto_hide;
            p.thickness = thickness.clamp(24, 128);
            p.opacity_q16 = opacity_q16.clamp(Q16_HALF >> 1, Q16_ONE);
            return true;
        }
        false
    }

    fn remove_panel(&mut self, panel_id: u32) -> bool {
        let len_before = self.panels.len();
        self.panels.retain(|p| p.id != panel_id);
        self.panels.len() < len_before
    }

    fn set_taskbar_style(&mut self, style: TaskbarStyle) {
        self.taskbar.style = style;
        match style {
            TaskbarStyle::Compact => {
                self.taskbar.height = 36;
                self.taskbar.show_labels = false;
            }
            TaskbarStyle::Floating => {
                self.taskbar.corner_radius = 16;
                self.taskbar.blur_behind = true;
            }
            TaskbarStyle::AutoHide => {
                self.taskbar.height = 48;
            }
            _ => {}
        }
    }

    fn add_zone(
        &mut self,
        zone_type: ZoneType,
        x: u16,
        y: u16,
        w: u16,
        h: u16,
        monitor_id: u8,
    ) -> u32 {
        if self.zones.len() >= 32 {
            return 0;
        }
        let id = self.next_zone_id;
        self.next_zone_id = self.next_zone_id.saturating_add(1);

        let zone = DesktopZone {
            id,
            zone_type,
            x,
            y,
            width: w,
            height: h,
            monitor_id,
            snap_enabled: true,
            padding: 8,
        };
        self.zones.push(zone);
        id
    }

    fn add_snap_region(&mut self, edge: SnapEdge, monitor_id: u8) -> u32 {
        if self.snap_regions.len() >= 20 {
            return 0;
        }
        let id = self.next_snap_id;
        self.next_snap_id = self.next_snap_id.saturating_add(1);

        let (x, y, w, h) = match edge {
            SnapEdge::LeftHalf => (0, 0, Q16_HALF, Q16_ONE),
            SnapEdge::RightHalf => (Q16_HALF, 0, Q16_HALF, Q16_ONE),
            SnapEdge::TopHalf => (0, 0, Q16_ONE, Q16_HALF),
            SnapEdge::BottomHalf => (0, Q16_HALF, Q16_ONE, Q16_HALF),
            SnapEdge::TopLeft => (0, 0, Q16_HALF, Q16_HALF),
            SnapEdge::TopRight => (Q16_HALF, 0, Q16_HALF, Q16_HALF),
            SnapEdge::BottomLeft => (0, Q16_HALF, Q16_HALF, Q16_HALF),
            SnapEdge::BottomRight => (Q16_HALF, Q16_HALF, Q16_HALF, Q16_HALF),
            SnapEdge::Center => (Q16_ONE >> 2, Q16_ONE >> 2, Q16_HALF, Q16_HALF),
            SnapEdge::Maximize => (0, 0, Q16_ONE, Q16_ONE),
        };

        let region = SnapRegion {
            id,
            edge,
            x_q16: x,
            y_q16: y,
            w_q16: w,
            h_q16: h,
            active: true,
            gap: 4,
            monitor_id,
        };
        self.snap_regions.push(region);
        id
    }

    fn add_monitor(
        &mut self,
        width: u16,
        height: u16,
        offset_x: i32,
        offset_y: i32,
        primary: bool,
    ) -> u8 {
        if self.monitors.len() >= 8 {
            return 0;
        }
        let id = self.monitors.len() as u8 + 1;

        let monitor = MonitorLayout {
            id,
            width,
            height,
            offset_x,
            offset_y,
            scale_q16: Q16_ONE,
            primary,
            rotation: 0,
        };
        self.monitors.push(monitor);
        id
    }

    fn set_monitor_scale(&mut self, monitor_id: u8, scale_q16: i32) -> bool {
        if let Some(m) = self.monitors.iter_mut().find(|m| m.id == monitor_id) {
            m.scale_q16 = scale_q16.clamp(Q16_HALF, q16_from_int(3));
            return true;
        }
        false
    }

    fn auto_arrange_icons(&mut self) {
        if !self.grid.auto_arrange {
            return;
        }
        let cols = self.grid.columns;
        for (idx, icon) in self.icons.iter_mut().enumerate() {
            let col = (idx as u16) % cols;
            let row = (idx as u16) / cols;
            icon.grid_col = col;
            icon.grid_row = row;
            icon.free_x = q16_div(q16_from_int(col as i32), q16_from_int(cols as i32));
            icon.free_y = q16_div(
                q16_from_int(row as i32),
                q16_from_int(self.grid.rows as i32),
            );
            icon.sort_order = idx as u16;
        }
    }

    fn setup_default_snap_regions(&mut self, monitor_id: u8) {
        let edges = [
            SnapEdge::LeftHalf,
            SnapEdge::RightHalf,
            SnapEdge::TopLeft,
            SnapEdge::TopRight,
            SnapEdge::BottomLeft,
            SnapEdge::BottomRight,
            SnapEdge::Maximize,
        ];
        for edge in &edges {
            self.add_snap_region(*edge, monitor_id);
        }
    }

    fn set_layout_mode(&mut self, mode: LayoutMode) {
        self.mode = mode;
        if mode == LayoutMode::Grid {
            self.auto_arrange_icons();
        }
    }

    fn set_grid_dimensions(&mut self, cols: u16, rows: u16) {
        self.grid.columns = cols.clamp(4, 24);
        self.grid.rows = rows.clamp(3, 16);
        if self.grid.auto_arrange {
            self.auto_arrange_icons();
        }
    }

    fn lock_desktop(&mut self, locked: bool) {
        self.desktop_locked = locked;
    }

    fn icon_count(&self) -> usize {
        self.icons.len()
    }

    fn panel_count(&self) -> usize {
        self.panels.len()
    }

    fn zone_count(&self) -> usize {
        self.zones.len()
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn init() {
    let mut mgr = DesktopLayoutManager::new();
    // Set up primary monitor with default snap regions
    mgr.add_monitor(1920, 1080, 0, 0, true);
    mgr.setup_default_snap_regions(1);
    // Default bottom panel
    mgr.add_panel(PanelPosition::Bottom, 1);

    let mut guard = LAYOUT.lock();
    *guard = Some(mgr);
    serial_println!("    Desktop layout: grid/free-form engine ready");
}

pub fn add_desktop_icon(app_id: u32, label_hash: u64) -> u32 {
    let mut guard = LAYOUT.lock();
    if let Some(mgr) = guard.as_mut() {
        return mgr.add_icon(app_id, label_hash);
    }
    0
}

pub fn move_icon_to_grid(icon_id: u32, col: u16, row: u16) -> bool {
    let mut guard = LAYOUT.lock();
    if let Some(mgr) = guard.as_mut() {
        return mgr.move_icon_grid(icon_id, col, row);
    }
    false
}

pub fn move_icon_to_position(icon_id: u32, x_q16: i32, y_q16: i32) -> bool {
    let mut guard = LAYOUT.lock();
    if let Some(mgr) = guard.as_mut() {
        return mgr.move_icon_free(icon_id, x_q16, y_q16);
    }
    false
}

pub fn set_taskbar(style: TaskbarStyle) {
    let mut guard = LAYOUT.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.set_taskbar_style(style);
    }
}

pub fn add_desktop_zone(
    zone_type: ZoneType,
    x: u16,
    y: u16,
    w: u16,
    h: u16,
    monitor_id: u8,
) -> u32 {
    let mut guard = LAYOUT.lock();
    if let Some(mgr) = guard.as_mut() {
        return mgr.add_zone(zone_type, x, y, w, h, monitor_id);
    }
    0
}

pub fn set_layout_mode(mode: LayoutMode) {
    let mut guard = LAYOUT.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.set_layout_mode(mode);
    }
}

pub fn lock_desktop(locked: bool) {
    let mut guard = LAYOUT.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.lock_desktop(locked);
    }
}

pub fn add_monitor(width: u16, height: u16, offset_x: i32, offset_y: i32, primary: bool) -> u8 {
    let mut guard = LAYOUT.lock();
    if let Some(mgr) = guard.as_mut() {
        return mgr.add_monitor(width, height, offset_x, offset_y, primary);
    }
    0
}
