use crate::sync::Mutex;
use alloc::vec;
/// Watch complications for Genesis
///
/// Data sources, templates, refresh scheduling,
/// tap actions, styles, and complication rendering.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

const Q16_ONE: i32 = 65536;

// ── data source types ────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
pub enum DataSourceKind {
    HeartRate,
    Steps,
    Calories,
    Battery,
    Weather,
    Date,
    Timer,
    Stopwatch,
    Sunrise,
    Sunset,
    NextAlarm,
    WorldClock,
    Compass,
    Altitude,
    UvIndex,
    AirQuality,
}

#[derive(Clone, Copy, PartialEq)]
pub enum RefreshRate {
    Realtime,
    EverySecond,
    EveryMinute,
    Every5Minutes,
    Every15Minutes,
    Hourly,
    OnDemand,
}

#[derive(Clone, Copy, PartialEq)]
pub enum TapAction {
    None,
    OpenApp,
    ToggleExpand,
    LaunchWorkout,
    ShowDetail,
    DismissAlert,
    NextValue,
    CustomCallback,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ComplicationStyle {
    Circular,
    Rectangular,
    Gauge,
    InlineText,
    CornerGauge,
    FullWidth,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ComplicationSize {
    Small,
    Medium,
    Large,
    ExtraLarge,
}

// ── data source ──────────────────────────────────────────────

struct DataSource {
    id: u32,
    kind: DataSourceKind,
    refresh: RefreshRate,
    current_value_q16: i32,
    min_value_q16: i32,
    max_value_q16: i32,
    unit_hash: u32,
    last_update_tick: u64,
    error_count: u32,
    enabled: bool,
}

impl DataSource {
    fn new(id: u32, kind: DataSourceKind, refresh: RefreshRate) -> Self {
        DataSource {
            id,
            kind,
            refresh,
            current_value_q16: 0,
            min_value_q16: 0,
            max_value_q16: 100 * Q16_ONE,
            unit_hash: 0,
            last_update_tick: 0,
            error_count: 0,
            enabled: true,
        }
    }

    fn update_value(&mut self, value_q16: i32, tick: u64) {
        self.current_value_q16 = value_q16;
        self.last_update_tick = tick;
    }

    fn normalized_q16(&self) -> i32 {
        let range = self.max_value_q16 - self.min_value_q16;
        if range == 0 {
            return 0;
        }
        let offset = self.current_value_q16 - self.min_value_q16;
        (((offset as i64) << 16) / (range as i64)) as i32
    }

    fn refresh_interval_ms(&self) -> u64 {
        match self.refresh {
            RefreshRate::Realtime => 16,
            RefreshRate::EverySecond => 1000,
            RefreshRate::EveryMinute => 60_000,
            RefreshRate::Every5Minutes => 300_000,
            RefreshRate::Every15Minutes => 900_000,
            RefreshRate::Hourly => 3_600_000,
            RefreshRate::OnDemand => u64::MAX,
        }
    }

    fn needs_refresh(&self, current_tick: u64) -> bool {
        if !self.enabled {
            return false;
        }
        let elapsed = current_tick.saturating_sub(self.last_update_tick);
        elapsed >= self.refresh_interval_ms()
    }
}

// ── complication template ────────────────────────────────────

struct ComplicationTemplate {
    id: u32,
    name: [u8; 24],
    name_len: usize,
    style: ComplicationStyle,
    size: ComplicationSize,
    data_source_id: u32,
    tap_action: TapAction,
    foreground_color: u32,
    background_color: u32,
    accent_color: u32,
    position_x: u16,
    position_y: u16,
    width: u16,
    height: u16,
    show_label: bool,
    show_icon: bool,
    show_range: bool,
    opacity_q16: i32,
    rotation_deg: i32,
    visible: bool,
}

impl ComplicationTemplate {
    fn area(&self) -> u32 {
        (self.width as u32) * (self.height as u32)
    }

    fn center_x(&self) -> u16 {
        self.position_x + self.width / 2
    }

    fn center_y(&self) -> u16 {
        self.position_y + self.height / 2
    }

    fn contains_point(&self, x: u16, y: u16) -> bool {
        x >= self.position_x
            && x < self.position_x + self.width
            && y >= self.position_y
            && y < self.position_y + self.height
    }
}

// ── complication slot ────────────────────────────────────────

struct ComplicationSlot {
    slot_index: u8,
    template: Option<ComplicationTemplate>,
    data_source: Option<DataSource>,
    tap_count: u32,
    last_tap_tick: u64,
    active: bool,
}

impl ComplicationSlot {
    fn new(slot_index: u8) -> Self {
        ComplicationSlot {
            slot_index,
            template: None,
            data_source: None,
            tap_count: 0,
            last_tap_tick: 0,
            active: false,
        }
    }

    fn bind(&mut self, template: ComplicationTemplate, source: DataSource) {
        self.template = Some(template);
        self.data_source = Some(source);
        self.active = true;
    }

    fn handle_tap(&mut self, tick: u64) -> TapAction {
        if !self.active {
            return TapAction::None;
        }
        self.tap_count = self.tap_count.saturating_add(1);
        self.last_tap_tick = tick;
        match &self.template {
            Some(t) => t.tap_action,
            None => TapAction::None,
        }
    }

    fn refresh_if_needed(&mut self, tick: u64) -> bool {
        if let Some(ref mut src) = self.data_source {
            if src.needs_refresh(tick) {
                return true;
            }
        }
        false
    }
}

// ── complication engine ──────────────────────────────────────

struct ComplicationEngine {
    slots: Vec<ComplicationSlot>,
    sources: Vec<DataSource>,
    next_source_id: u32,
    next_template_id: u32,
    max_slots: u8,
    ambient_mode: bool,
    ambient_refresh: RefreshRate,
    global_opacity_q16: i32,
    total_taps: u64,
    total_refreshes: u64,
}

static COMPLICATIONS: Mutex<Option<ComplicationEngine>> = Mutex::new(None);

impl ComplicationEngine {
    fn new() -> Self {
        let mut slots = Vec::new();
        let max = 8u8;
        for i in 0..max {
            slots.push(ComplicationSlot::new(i));
        }
        ComplicationEngine {
            slots,
            sources: Vec::new(),
            next_source_id: 1,
            next_template_id: 1,
            max_slots: max,
            ambient_mode: false,
            ambient_refresh: RefreshRate::EveryMinute,
            global_opacity_q16: Q16_ONE,
            total_taps: 0,
            total_refreshes: 0,
        }
    }

    fn register_source(&mut self, kind: DataSourceKind, refresh: RefreshRate) -> u32 {
        let id = self.next_source_id;
        self.next_source_id = self.next_source_id.saturating_add(1);
        self.sources.push(DataSource::new(id, kind, refresh));
        id
    }

    fn create_template(
        &mut self,
        name: &[u8],
        style: ComplicationStyle,
        size: ComplicationSize,
        source_id: u32,
        tap: TapAction,
        x: u16,
        y: u16,
        w: u16,
        h: u16,
    ) -> u32 {
        let id = self.next_template_id;
        self.next_template_id = self.next_template_id.saturating_add(1);
        let mut n = [0u8; 24];
        let nlen = name.len().min(24);
        n[..nlen].copy_from_slice(&name[..nlen]);
        let template = ComplicationTemplate {
            id,
            name: n,
            name_len: nlen,
            style,
            size,
            data_source_id: source_id,
            tap_action: tap,
            foreground_color: 0xFFFFFF,
            background_color: 0x000000,
            accent_color: 0x00AAFF,
            position_x: x,
            position_y: y,
            width: w,
            height: h,
            show_label: true,
            show_icon: true,
            show_range: style == ComplicationStyle::Gauge
                || style == ComplicationStyle::CornerGauge,
            opacity_q16: Q16_ONE,
            rotation_deg: 0,
            visible: true,
        };

        // Find matching source and bind to next available slot
        if let Some(source) = self.sources.iter().find(|s| s.id == source_id) {
            let src_clone = DataSource::new(source.id, source.kind, source.refresh);
            for slot in self.slots.iter_mut() {
                if !slot.active {
                    slot.bind(template, src_clone);
                    return id;
                }
            }
        }
        id
    }

    fn handle_tap_at(&mut self, x: u16, y: u16, tick: u64) -> TapAction {
        for slot in self.slots.iter_mut() {
            if let Some(ref tmpl) = slot.template {
                if tmpl.contains_point(x, y) {
                    let action = slot.handle_tap(tick);
                    self.total_taps = self.total_taps.saturating_add(1);
                    return action;
                }
            }
        }
        TapAction::None
    }

    fn tick_refresh(&mut self, current_tick: u64) {
        let effective_refresh = if self.ambient_mode {
            self.ambient_refresh
        } else {
            RefreshRate::Realtime
        };
        for slot in self.slots.iter_mut() {
            if slot.refresh_if_needed(current_tick) {
                self.total_refreshes = self.total_refreshes.saturating_add(1);
            }
        }
        let _ = effective_refresh;
    }

    fn set_ambient(&mut self, ambient: bool) {
        self.ambient_mode = ambient;
        if ambient {
            self.global_opacity_q16 = Q16_ONE / 2;
        } else {
            self.global_opacity_q16 = Q16_ONE;
        }
    }

    fn active_slot_count(&self) -> usize {
        self.slots.iter().filter(|s| s.active).count()
    }

    fn update_source_value(&mut self, source_id: u32, value_q16: i32, tick: u64) {
        for src in self.sources.iter_mut() {
            if src.id == source_id {
                src.update_value(value_q16, tick);
            }
        }
        // Also update any slot bound to this source
        for slot in self.slots.iter_mut() {
            if let Some(ref mut ds) = slot.data_source {
                if ds.id == source_id {
                    ds.update_value(value_q16, tick);
                }
            }
        }
    }

    fn remove_slot(&mut self, slot_index: u8) {
        if let Some(slot) = self.slots.iter_mut().find(|s| s.slot_index == slot_index) {
            slot.template = None;
            slot.data_source = None;
            slot.active = false;
        }
    }
}

pub fn init() {
    let mut c = COMPLICATIONS.lock();
    *c = Some(ComplicationEngine::new());
    serial_println!("    Wearable: watch complications ready");
}
