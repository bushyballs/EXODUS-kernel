use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EventRepeat {
    None,
    Daily,
    Weekly,
    Biweekly,
    Monthly,
    Yearly,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EventStatus {
    Confirmed,
    Tentative,
    Cancelled,
}

#[derive(Clone, Copy)]
pub struct CalendarEvent {
    pub id: u32,
    pub calendar_id: u8,
    pub title_hash: u64,
    pub start_time: u64,
    pub end_time: u64,
    pub all_day: bool,
    pub location_hash: u64,
    pub repeat: EventRepeat,
    pub reminder_minutes: u16,
    pub status: EventStatus,
    pub attendee_count: u8,
    pub organizer_hash: u64,
}

impl CalendarEvent {
    pub fn new(id: u32, calendar_id: u8, title_hash: u64, start_time: u64, end_time: u64) -> Self {
        Self {
            id,
            calendar_id,
            title_hash,
            start_time,
            end_time,
            all_day: false,
            location_hash: 0,
            repeat: EventRepeat::None,
            reminder_minutes: 0,
            status: EventStatus::Confirmed,
            attendee_count: 0,
            organizer_hash: 0,
        }
    }

    pub fn overlaps_with(&self, other: &CalendarEvent) -> bool {
        if self.status == EventStatus::Cancelled || other.status == EventStatus::Cancelled {
            return false;
        }

        (self.start_time < other.end_time) && (self.end_time > other.start_time)
    }
}

#[derive(Clone, Copy)]
pub struct Calendar {
    pub id: u8,
    pub name_hash: u64,
    pub color: u32,
    pub visible: bool,
    pub is_local: bool,
}

impl Calendar {
    pub fn new(id: u8, name_hash: u64, color: u32) -> Self {
        Self {
            id,
            name_hash,
            color,
            visible: true,
            is_local: true,
        }
    }
}

pub struct CalendarManager {
    calendars: Vec<Calendar>,
    events: Vec<CalendarEvent>,
    next_event_id: u32,
}

impl CalendarManager {
    pub fn new() -> Self {
        Self {
            calendars: vec![],
            events: vec![],
            next_event_id: 1,
        }
    }

    pub fn create_calendar(&mut self, name_hash: u64, color: u32) -> u8 {
        let id = self.calendars.len() as u8;
        let calendar = Calendar::new(id, name_hash, color);
        self.calendars.push(calendar);
        id
    }

    pub fn add_event(
        &mut self,
        calendar_id: u8,
        title_hash: u64,
        start_time: u64,
        end_time: u64,
    ) -> u32 {
        let id = self.next_event_id;
        self.next_event_id = self.next_event_id.saturating_add(1);
        let event = CalendarEvent::new(id, calendar_id, title_hash, start_time, end_time);
        self.events.push(event);
        id
    }

    pub fn remove_event(&mut self, id: u32) -> bool {
        if let Some(pos) = self.events.iter().position(|e| e.id == id) {
            self.events.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn get_events_in_range(&self, start: u64, end: u64) -> Vec<u32> {
        self.events
            .iter()
            .filter(|e| {
                e.status != EventStatus::Cancelled && e.start_time < end && e.end_time > start
            })
            .map(|e| e.id)
            .collect()
    }

    pub fn get_upcoming(&self, current_time: u64, n: usize) -> Vec<u32> {
        let mut upcoming: Vec<&CalendarEvent> = self
            .events
            .iter()
            .filter(|e| e.start_time >= current_time && e.status != EventStatus::Cancelled)
            .collect();

        upcoming.sort_by_key(|e| e.start_time);
        upcoming.iter().take(n).map(|e| e.id).collect()
    }

    pub fn check_conflicts(&self, event_id: u32) -> Vec<u32> {
        let event = match self.events.iter().find(|e| e.id == event_id) {
            Some(e) => e,
            None => return vec![],
        };

        self.events
            .iter()
            .filter(|e| {
                e.id != event_id && e.calendar_id == event.calendar_id && e.overlaps_with(event)
            })
            .map(|e| e.id)
            .collect()
    }

    pub fn get_event(&self, id: u32) -> Option<&CalendarEvent> {
        self.events.iter().find(|e| e.id == id)
    }

    pub fn get_event_mut(&mut self, id: u32) -> Option<&mut CalendarEvent> {
        self.events.iter_mut().find(|e| e.id == id)
    }

    pub fn get_calendar(&self, id: u8) -> Option<&Calendar> {
        self.calendars.get(id as usize)
    }

    pub fn total_events(&self) -> usize {
        self.events.len()
    }

    pub fn total_calendars(&self) -> usize {
        self.calendars.len()
    }
}

static CALENDAR: Mutex<Option<CalendarManager>> = Mutex::new(None);

pub fn init() {
    let mut calendar = CALENDAR.lock();
    *calendar = Some(CalendarManager::new());
    serial_println!("[CONTACTS] Calendar manager initialized");
}

pub fn get_calendar() -> &'static Mutex<Option<CalendarManager>> {
    &CALENDAR
}
