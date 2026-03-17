#![no_std]

use crate::sync::Mutex;

const ROOM_COUNT: usize = 16;
const MAX_CONNECTIONS: usize = 4;
const DECAY_THRESHOLD: u32 = 10_000; // Unvisited for this many ticks → fade

#[derive(Clone, Copy)]
pub struct Room {
    content_hash: u32,
    emotional_value: u16,
    visit_count: u32,
    last_visit: u32,
    is_treasure: bool,
    corridor_connections: [u16; MAX_CONNECTIONS],
    connection_strength: [u16; MAX_CONNECTIONS],
}

impl Room {
    const fn new() -> Self {
        Room {
            content_hash: 0,
            emotional_value: 0,
            visit_count: 0,
            last_visit: 0,
            is_treasure: false,
            corridor_connections: [u16::MAX; MAX_CONNECTIONS],
            connection_strength: [0; MAX_CONNECTIONS],
        }
    }

    fn is_empty(&self) -> bool {
        self.content_hash == 0
    }

    fn decay_check(&mut self, age: u32) {
        if self.is_empty() {
            return;
        }

        let time_since_visit = age.saturating_sub(self.last_visit);
        if time_since_visit > DECAY_THRESHOLD {
            let fade_factor = (time_since_visit - DECAY_THRESHOLD) / 1000;
            if fade_factor > 0 {
                self.content_hash = 0;
                self.emotional_value = 0;
            }
        }
    }
}

pub struct MemoryPalace {
    rooms: [Room; ROOM_COUNT],
    palace_size: u16,
    current_room: u16,
    path_strength: u16,
    treasure_count: u16,
    palace_beauty: u16,
    retrieval_success: u16,
    total_visits: u32,
    age: u32,
}

impl MemoryPalace {
    const fn new() -> Self {
        MemoryPalace {
            rooms: [Room::new(); ROOM_COUNT],
            palace_size: 0,
            current_room: 0,
            path_strength: 0,
            treasure_count: 0,
            palace_beauty: 500,
            retrieval_success: 800,
            total_visits: 0,
            age: 0,
        }
    }

    pub fn store_memory(
        &mut self,
        content_hash: u32,
        emotional_value: u16,
        is_treasure: bool,
    ) -> bool {
        if self.palace_size >= ROOM_COUNT as u16 {
            return false;
        }

        let idx = self.palace_size as usize;
        self.rooms[idx] = Room {
            content_hash,
            emotional_value: emotional_value.min(1000),
            visit_count: 0,
            last_visit: self.age,
            is_treasure,
            corridor_connections: [u16::MAX; MAX_CONNECTIONS],
            connection_strength: [0; MAX_CONNECTIONS],
        };

        if is_treasure {
            self.treasure_count = self.treasure_count.saturating_add(1);
        }

        self.palace_size = self.palace_size.saturating_add(1);
        true
    }

    pub fn connect_rooms(&mut self, room_a: u16, room_b: u16, strength: u16) -> bool {
        if room_a >= self.palace_size || room_b >= self.palace_size {
            return false;
        }

        let idx_a = room_a as usize;
        let strength_clamped = strength.min(1000);

        for i in 0..MAX_CONNECTIONS {
            if self.rooms[idx_a].corridor_connections[i] == u16::MAX {
                self.rooms[idx_a].corridor_connections[i] = room_b;
                self.rooms[idx_a].connection_strength[i] = strength_clamped;
                return true;
            }
        }

        false
    }

    fn walk_palace(&mut self) {
        if self.palace_size == 0 {
            return;
        }

        let current_idx = self.current_room as usize;

        // Update visit tracking on current room
        self.rooms[current_idx].visit_count = self.rooms[current_idx].visit_count.saturating_add(1);
        self.rooms[current_idx].last_visit = self.age;

        // Collect connection candidates first (avoid split borrow)
        let mut connections = [(u16::MAX, 0u16); MAX_CONNECTIONS];
        for i in 0..MAX_CONNECTIONS {
            connections[i] = (
                self.rooms[current_idx].corridor_connections[i],
                self.rooms[current_idx].connection_strength[i],
            );
        }

        let mut best_next_room = u16::MAX;
        let mut best_strength = 0u16;
        let mut best_value = 0u16;

        for i in 0..MAX_CONNECTIONS {
            let (next, strength) = connections[i];
            if next != u16::MAX && (next as usize) < ROOM_COUNT {
                let next_idx = next as usize;
                if !self.rooms[next_idx].is_empty()
                    && self.rooms[next_idx].emotional_value > best_value
                {
                    best_value = self.rooms[next_idx].emotional_value;
                    best_next_room = next;
                    best_strength = strength;
                }
            }
        }

        if best_next_room != u16::MAX {
            self.current_room = best_next_room;
            self.path_strength = best_strength.saturating_add(10).min(1000);
        }

        self.total_visits = self.total_visits.saturating_add(1);
    }

    fn decay_unvisited(&mut self) {
        for i in 0..ROOM_COUNT {
            self.rooms[i].decay_check(self.age);
            if self.rooms[i].is_empty() && i < self.palace_size as usize {
                if i == self.palace_size as usize - 1 {
                    self.palace_size = self.palace_size.saturating_sub(1);
                }
            }
        }
    }

    fn update_beauty(&mut self) {
        if self.palace_size == 0 {
            self.palace_beauty = 500;
            return;
        }

        let mut beauty_sum: u32 = 0;
        let mut avg_visits: u32 = 0;

        for i in 0..(self.palace_size as usize) {
            let room = &self.rooms[i];
            beauty_sum = beauty_sum.saturating_add(room.emotional_value as u32);
            avg_visits = avg_visits.saturating_add(room.visit_count);
        }

        let base_beauty = (beauty_sum / (self.palace_size as u32).max(1)) as u16;
        let visit_bonus = ((avg_visits / (self.palace_size as u32).max(1)).min(1000)) as u16;

        self.palace_beauty = ((base_beauty as u32 + visit_bonus as u32) / 2).min(1000) as u16;
    }

    fn update_retrieval_success(&mut self) {
        if self.total_visits == 0 {
            self.retrieval_success = 500;
            return;
        }

        let current_idx = self.current_room as usize;
        if current_idx >= ROOM_COUNT {
            self.retrieval_success = 300;
            return;
        }

        let current = &self.rooms[current_idx];
        let success_base = current.emotional_value;

        let frequency_bonus = if current.visit_count > 100 { 100 } else { 0 };
        let treasure_bonus = if current.is_treasure { 150 } else { 0 };
        let path_bonus = self.path_strength / 2;

        self.retrieval_success =
            (success_base as u32 + frequency_bonus + treasure_bonus + path_bonus as u32).min(1000)
                as u16;
    }

    fn decay_empty_rooms(&mut self) {
        let mut shift_count = 0;
        for i in 0..(self.palace_size as usize) {
            if self.rooms[i].is_empty() {
                shift_count += 1;
            } else if shift_count > 0 {
                self.rooms[i - shift_count] = self.rooms[i];
                self.rooms[i] = Room::new();
            }
        }

        self.palace_size = (self.palace_size as usize - shift_count) as u16;

        if self.current_room >= self.palace_size && self.palace_size > 0 {
            self.current_room = self.palace_size.saturating_sub(1);
        }
    }

    pub fn tick(&mut self, age: u32) {
        self.age = age;

        if age % 10 == 0 {
            self.decay_unvisited();
            self.decay_empty_rooms();
        }

        if self.palace_size > 0 {
            self.walk_palace();
        }

        self.update_beauty();
        self.update_retrieval_success();
    }

    pub fn report(&self) -> MemoryPalaceReport {
        MemoryPalaceReport {
            palace_size: self.palace_size,
            current_room: self.current_room,
            path_strength: self.path_strength,
            treasure_count: self.treasure_count,
            palace_beauty: self.palace_beauty,
            retrieval_success: self.retrieval_success,
            total_visits: self.total_visits,
        }
    }
}

pub struct MemoryPalaceReport {
    pub palace_size: u16,
    pub current_room: u16,
    pub path_strength: u16,
    pub treasure_count: u16,
    pub palace_beauty: u16,
    pub retrieval_success: u16,
    pub total_visits: u32,
}

static STATE: Mutex<MemoryPalace> = Mutex::new(MemoryPalace::new());

pub fn init() {
    let mut palace = STATE.lock();
    palace.palace_size = 0;
    palace.palace_beauty = 500;
    palace.retrieval_success = 800;
}

pub fn store_memory(content_hash: u32, emotional_value: u16, is_treasure: bool) -> bool {
    let mut palace = STATE.lock();
    palace.store_memory(content_hash, emotional_value, is_treasure)
}

pub fn connect_rooms(room_a: u16, room_b: u16, strength: u16) -> bool {
    let mut palace = STATE.lock();
    palace.connect_rooms(room_a, room_b, strength)
}

pub fn tick(age: u32) {
    let mut palace = STATE.lock();
    palace.tick(age);
}

pub fn report() -> MemoryPalaceReport {
    let palace = STATE.lock();
    palace.report()
}

pub fn current_room() -> u16 {
    let palace = STATE.lock();
    palace.current_room
}

pub fn palace_beauty() -> u16 {
    let palace = STATE.lock();
    palace.palace_beauty
}

pub fn treasure_count() -> u16 {
    let palace = STATE.lock();
    palace.treasure_count
}
