// iCalendar parser/generator for Genesis
// Supports VEVENT, VTODO, VFREEBUSY, RRULE recurrence, VALARM

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;
use crate::sync::Mutex;
use crate::{serial_print, serial_println};

const Q16_ONE: i32 = 65536;

// ── Component types ─────────────────────────────────────────────────
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ICalComponent {
    VEvent,
    VTodo,
    VFreeBusy,
    VAlarm,
    VJournal,
    VTimezone,
}

// ── Event status ────────────────────────────────────────────────────
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ICalStatus {
    Tentative,
    Confirmed,
    Cancelled,
    NeedsAction,
    Completed,
    InProcess,
}

// ── Free/busy type ──────────────────────────────────────────────────
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FreeBusyType {
    Free,
    Busy,
    BusyUnavailable,
    BusyTentative,
}

// ── RRULE frequency ─────────────────────────────────────────────────
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RRuleFreq {
    Secondly,
    Minutely,
    Hourly,
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

// ── RRULE weekday ───────────────────────────────────────────────────
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RRuleDay {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

// ── Alarm action ────────────────────────────────────────────────────
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AlarmAction {
    Display,
    Audio,
    Email,
    Procedure,
}

// ── Alarm trigger type ──────────────────────────────────────────────
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AlarmTriggerType {
    BeforeStart,
    AfterStart,
    BeforeEnd,
    AfterEnd,
    Absolute,
}

// ── Recurrence rule ─────────────────────────────────────────────────
#[derive(Clone, Copy)]
pub struct RRule {
    pub freq: RRuleFreq,
    pub interval: u16,
    pub count: u16,
    pub until_timestamp: u64,
    pub by_day: [RRuleDay; 7],
    pub by_day_count: u8,
    pub by_month: [u8; 12],
    pub by_month_count: u8,
    pub by_month_day: [i8; 31],
    pub by_month_day_count: u8,
    pub by_hour: [u8; 24],
    pub by_hour_count: u8,
    pub by_set_pos: i16,
    pub week_start: RRuleDay,
}

impl RRule {
    pub fn new(freq: RRuleFreq) -> Self {
        Self {
            freq,
            interval: 1,
            count: 0,
            until_timestamp: 0,
            by_day: [RRuleDay::Monday; 7],
            by_day_count: 0,
            by_month: [0; 12],
            by_month_count: 0,
            by_month_day: [0; 31],
            by_month_day_count: 0,
            by_hour: [0; 24],
            by_hour_count: 0,
            by_set_pos: 0,
            week_start: RRuleDay::Monday,
        }
    }

    pub fn add_by_day(&mut self, day: RRuleDay) -> bool {
        if self.by_day_count < 7 {
            self.by_day[self.by_day_count as usize] = day;
            self.by_day_count = self.by_day_count.saturating_add(1);
            true
        } else {
            false
        }
    }

    pub fn add_by_month(&mut self, month: u8) -> bool {
        if self.by_month_count < 12 && month >= 1 && month <= 12 {
            self.by_month[self.by_month_count as usize] = month;
            self.by_month_count = self.by_month_count.saturating_add(1);
            true
        } else {
            false
        }
    }

    pub fn add_by_month_day(&mut self, day: i8) -> bool {
        if self.by_month_day_count < 31 && day != 0 && day >= -31 && day <= 31 {
            self.by_month_day[self.by_month_day_count as usize] = day;
            self.by_month_day_count = self.by_month_day_count.saturating_add(1);
            true
        } else {
            false
        }
    }

    pub fn is_finite(&self) -> bool {
        self.count > 0 || self.until_timestamp > 0
    }

    /// Compute next occurrence after given timestamp (simplified)
    pub fn next_occurrence(&self, after: u64) -> u64 {
        let interval_secs = match self.freq {
            RRuleFreq::Secondly => self.interval as u64,
            RRuleFreq::Minutely => (self.interval as u64) * 60,
            RRuleFreq::Hourly => (self.interval as u64) * 3600,
            RRuleFreq::Daily => (self.interval as u64) * 86400,
            RRuleFreq::Weekly => (self.interval as u64) * 604800,
            RRuleFreq::Monthly => (self.interval as u64) * 2592000,
            RRuleFreq::Yearly => (self.interval as u64) * 31536000,
        };
        after.saturating_add(interval_secs)
    }
}

// ── VALARM ──────────────────────────────────────────────────────────
#[derive(Clone, Copy)]
pub struct VAlarm {
    pub action: AlarmAction,
    pub trigger_type: AlarmTriggerType,
    pub trigger_seconds: i32,
    pub trigger_absolute: u64,
    pub repeat_count: u8,
    pub repeat_interval_secs: u32,
    pub description_hash: u64,
    pub summary_hash: u64,
}

impl VAlarm {
    pub fn new(action: AlarmAction, trigger_type: AlarmTriggerType, trigger_seconds: i32) -> Self {
        Self {
            action,
            trigger_type,
            trigger_seconds,
            trigger_absolute: 0,
            repeat_count: 0,
            repeat_interval_secs: 0,
            description_hash: 0,
            summary_hash: 0,
        }
    }

    pub fn should_fire(&self, event_start: u64, event_end: u64, current_time: u64) -> bool {
        let fire_time = match self.trigger_type {
            AlarmTriggerType::BeforeStart => {
                event_start.saturating_sub(self.trigger_seconds as u64)
            }
            AlarmTriggerType::AfterStart => {
                event_start.saturating_add(self.trigger_seconds as u64)
            }
            AlarmTriggerType::BeforeEnd => {
                event_end.saturating_sub(self.trigger_seconds as u64)
            }
            AlarmTriggerType::AfterEnd => {
                event_end.saturating_add(self.trigger_seconds as u64)
            }
            AlarmTriggerType::Absolute => self.trigger_absolute,
        };
        current_time >= fire_time
    }
}

// ── Free/busy period ────────────────────────────────────────────────
#[derive(Clone, Copy)]
pub struct FreeBusyPeriod {
    pub start: u64,
    pub end: u64,
    pub fb_type: FreeBusyType,
}

// ── VEVENT ──────────────────────────────────────────────────────────
#[derive(Clone, Copy)]
pub struct VEvent {
    pub id: u32,
    pub uid_hash: u64,
    pub summary_hash: u64,
    pub description_hash: u64,
    pub location_hash: u64,
    pub dtstart: u64,
    pub dtend: u64,
    pub dtstamp: u64,
    pub created: u64,
    pub last_modified: u64,
    pub status: ICalStatus,
    pub transp_opaque: bool,
    pub sequence: u32,
    pub organizer_hash: u64,
    pub attendee_hashes: [u64; 16],
    pub attendee_count: u8,
    pub categories_hash: u64,
    pub priority: u8,
    pub has_rrule: bool,
    pub rrule: RRule,
    pub alarms: [VAlarm; 4],
    pub alarm_count: u8,
    pub all_day: bool,
}

impl VEvent {
    pub fn new(id: u32, uid_hash: u64, dtstart: u64, dtend: u64) -> Self {
        Self {
            id,
            uid_hash,
            summary_hash: 0,
            description_hash: 0,
            location_hash: 0,
            dtstart,
            dtend,
            dtstamp: 0,
            created: 0,
            last_modified: 0,
            status: ICalStatus::Confirmed,
            transp_opaque: true,
            sequence: 0,
            organizer_hash: 0,
            attendee_hashes: [0; 16],
            attendee_count: 0,
            categories_hash: 0,
            priority: 0,
            has_rrule: false,
            rrule: RRule::new(RRuleFreq::Daily),
            alarms: [VAlarm::new(AlarmAction::Display, AlarmTriggerType::BeforeStart, 0); 4],
            alarm_count: 0,
            all_day: false,
        }
    }

    pub fn add_attendee(&mut self, hash: u64) -> bool {
        if self.attendee_count < 16 {
            self.attendee_hashes[self.attendee_count as usize] = hash;
            self.attendee_count = self.attendee_count.saturating_add(1);
            true
        } else {
            false
        }
    }

    pub fn add_alarm(&mut self, alarm: VAlarm) -> bool {
        if self.alarm_count < 4 {
            self.alarms[self.alarm_count as usize] = alarm;
            self.alarm_count = self.alarm_count.saturating_add(1);
            true
        } else {
            false
        }
    }

    pub fn set_recurrence(&mut self, rrule: RRule) {
        self.rrule = rrule;
        self.has_rrule = true;
    }

    pub fn duration_seconds(&self) -> u64 {
        self.dtend.saturating_sub(self.dtstart)
    }
}

// ── VTODO ───────────────────────────────────────────────────────────
#[derive(Clone, Copy)]
pub struct VTodo {
    pub id: u32,
    pub uid_hash: u64,
    pub summary_hash: u64,
    pub description_hash: u64,
    pub status: ICalStatus,
    pub priority: u8,
    pub percent_complete: u8,
    pub due: u64,
    pub completed: u64,
    pub dtstart: u64,
    pub has_rrule: bool,
    pub rrule: RRule,
}

impl VTodo {
    pub fn new(id: u32, uid_hash: u64) -> Self {
        Self {
            id,
            uid_hash,
            summary_hash: 0,
            description_hash: 0,
            status: ICalStatus::NeedsAction,
            priority: 0,
            percent_complete: 0,
            due: 0,
            completed: 0,
            dtstart: 0,
            has_rrule: false,
            rrule: RRule::new(RRuleFreq::Daily),
        }
    }

    pub fn mark_complete(&mut self, timestamp: u64) {
        self.status = ICalStatus::Completed;
        self.percent_complete = 100;
        self.completed = timestamp;
    }

    pub fn is_overdue(&self, now: u64) -> bool {
        self.due > 0 && now > self.due && self.status != ICalStatus::Completed
    }
}

// ── ICal store ──────────────────────────────────────────────────────
pub struct ICalStore {
    events: Vec<VEvent>,
    todos: Vec<VTodo>,
    freebusy: Vec<FreeBusyPeriod>,
    next_event_id: u32,
    next_todo_id: u32,
    parse_errors: u32,
}

impl ICalStore {
    pub fn new() -> Self {
        Self {
            events: vec![],
            todos: vec![],
            freebusy: vec![],
            next_event_id: 1,
            next_todo_id: 1,
            parse_errors: 0,
        }
    }

    pub fn add_event(&mut self, uid_hash: u64, dtstart: u64, dtend: u64) -> u32 {
        let id = self.next_event_id;
        self.next_event_id = self.next_event_id.saturating_add(1);
        self.events.push(VEvent::new(id, uid_hash, dtstart, dtend));
        id
    }

    pub fn add_todo(&mut self, uid_hash: u64) -> u32 {
        let id = self.next_todo_id;
        self.next_todo_id = self.next_todo_id.saturating_add(1);
        self.todos.push(VTodo::new(id, uid_hash));
        id
    }

    pub fn add_freebusy(&mut self, start: u64, end: u64, fb_type: FreeBusyType) {
        self.freebusy.push(FreeBusyPeriod { start, end, fb_type });
    }

    pub fn get_event(&self, id: u32) -> Option<&VEvent> {
        self.events.iter().find(|e| e.id == id)
    }

    pub fn get_event_mut(&mut self, id: u32) -> Option<&mut VEvent> {
        self.events.iter_mut().find(|e| e.id == id)
    }

    pub fn get_todo(&self, id: u32) -> Option<&VTodo> {
        self.todos.iter().find(|t| t.id == id)
    }

    pub fn get_todo_mut(&mut self, id: u32) -> Option<&mut VTodo> {
        self.todos.iter_mut().find(|t| t.id == id)
    }

    pub fn remove_event(&mut self, id: u32) -> bool {
        if let Some(pos) = self.events.iter().position(|e| e.id == id) {
            self.events.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn remove_todo(&mut self, id: u32) -> bool {
        if let Some(pos) = self.todos.iter().position(|t| t.id == id) {
            self.todos.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn events_in_range(&self, start: u64, end: u64) -> Vec<u32> {
        self.events.iter()
            .filter(|e| e.status != ICalStatus::Cancelled && e.dtstart < end && e.dtend > start)
            .map(|e| e.id)
            .collect()
    }

    pub fn overdue_todos(&self, now: u64) -> Vec<u32> {
        self.todos.iter()
            .filter(|t| t.is_overdue(now))
            .map(|t| t.id)
            .collect()
    }

    pub fn pending_alarms(&self, now: u64) -> Vec<(u32, u8)> {
        let mut alarms = vec![];
        for event in &self.events {
            for i in 0..event.alarm_count as usize {
                if event.alarms[i].should_fire(event.dtstart, event.dtend, now) {
                    alarms.push((event.id, i as u8));
                }
            }
        }
        alarms
    }

    pub fn freebusy_at(&self, time: u64) -> FreeBusyType {
        for fb in &self.freebusy {
            if time >= fb.start && time < fb.end {
                return fb.fb_type;
            }
        }
        FreeBusyType::Free
    }

    pub fn total_events(&self) -> usize {
        self.events.len()
    }

    pub fn total_todos(&self) -> usize {
        self.todos.len()
    }

    pub fn total_freebusy(&self) -> usize {
        self.freebusy.len()
    }

    pub fn parse_errors(&self) -> u32 {
        self.parse_errors
    }

    pub fn report_parse_error(&mut self) {
        self.parse_errors = self.parse_errors.saturating_add(1);
    }
}

static ICAL_STORE: Mutex<Option<ICalStore>> = Mutex::new(None);

pub fn init() {
    let mut store = ICAL_STORE.lock();
    *store = Some(ICalStore::new());
    serial_println!("[CONTACTS] iCalendar parser/generator initialized");
}

pub fn get_ical_store() -> &'static Mutex<Option<ICalStore>> {
    &ICAL_STORE
}
