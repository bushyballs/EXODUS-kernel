#![no_std]

use crate::sync::Mutex;

/// Memory Labyrinth: A maze of interconnected memories that must be navigated.
/// To consolidate experience, the organism walks through corridors of related memories,
/// finding connections, dead ends, and shortcuts. The maze evolves as new memories arrive.

const ROOM_COUNT: usize = 16;
const CONNECTION_COUNT: usize = 4;

#[derive(Clone, Copy)]
pub struct MemoryRoom {
    content_hash: u32,                    // Fingerprint of memory content
    emotional_charge: u32,                // 0-1000, strength of feeling attached
    visit_count: u32,                     // How many times navigated through
    connections: [u16; CONNECTION_COUNT], // Indices of adjacent rooms (0-1000 scale softmax)
    novelty: u32,                         // 0-1000, freshness (decays over time)
}

impl MemoryRoom {
    const fn new() -> Self {
        MemoryRoom {
            content_hash: 0,
            emotional_charge: 0,
            visit_count: 0,
            connections: [0xFFFF; CONNECTION_COUNT],
            novelty: 0,
        }
    }
}

pub struct MemoryLabyrinthState {
    rooms: [MemoryRoom; ROOM_COUNT],
    current_position: u16,       // Which room (0-15)
    exploration_drive: u32,      // 0-1000, motivation to navigate
    lost_level: u32,             // 0-1000, confusion from dead ends
    consolidation_progress: u32, // 0-1000, experience integration
    shortcut_count: u32,         // Number of discovered fast paths
    maze_complexity: u32,        // 0-1000, difficulty as memories grow
    eureka_moments: u32,         // Unexpected distant memory connections
    age: u32,                    // Ticks since init
    last_step: u16,              // Previous room visited
    reinforcement: u32,          // 0-1000, strength of current path
}

impl MemoryLabyrinthState {
    const fn new() -> Self {
        MemoryLabyrinthState {
            rooms: [MemoryRoom::new(); ROOM_COUNT],
            current_position: 0,
            exploration_drive: 500,
            lost_level: 0,
            consolidation_progress: 0,
            shortcut_count: 0,
            maze_complexity: 100,
            eureka_moments: 0,
            age: 0,
            last_step: 0xFFFF,
            reinforcement: 0,
        }
    }
}

static STATE: Mutex<MemoryLabyrinthState> = Mutex::new(MemoryLabyrinthState::new());

pub fn init() {
    let mut state = STATE.lock();

    // Initialize room graph with seed memories
    for i in 0..ROOM_COUNT {
        state.rooms[i].content_hash = ((i as u32).wrapping_mul(0x9e3779b9)) ^ 0xdeadbeef;
        state.rooms[i].emotional_charge = (i as u32 * 62) % 1000; // Varied emotions
        state.rooms[i].novelty = 800;
        state.rooms[i].visit_count = 0;

        // Connect rooms in a ring with some cross-links
        let next = ((i + 1) % ROOM_COUNT) as u16;
        let prev = ((i + ROOM_COUNT - 1) % ROOM_COUNT) as u16;
        let cross1 = ((i + 4) % ROOM_COUNT) as u16;
        let cross2 = ((i + 8) % ROOM_COUNT) as u16;

        state.rooms[i].connections[0] = next;
        state.rooms[i].connections[1] = prev;
        state.rooms[i].connections[2] = cross1;
        state.rooms[i].connections[3] = cross2;
    }

    state.current_position = 0;
    state.exploration_drive = 500;
    state.lost_level = 0;
    state.consolidation_progress = 0;
    state.age = 0;
}

pub fn tick(age: u32) {
    let mut state = STATE.lock();
    state.age = age;

    // Decay lost_level gradually as organism reorients
    state.lost_level = state.lost_level.saturating_sub(15);

    // Decay novelty in all rooms
    for i in 0..ROOM_COUNT {
        state.rooms[i].novelty = state.rooms[i].novelty.saturating_sub(3);
    }

    // Exploration drive fluctuates: curiosity pulls up, saturation pulls down
    let consolidation_pull = (state.consolidation_progress * 50) / 1000;
    state.exploration_drive = state
        .exploration_drive
        .saturating_add(consolidation_pull)
        .min(1000);

    // Maze complexity grows as we accumulate experiences
    let complexity_growth = (age / 100).saturating_mul(5).min(900);
    state.maze_complexity = 100 + complexity_growth;

    // Decide next move: find best adjacent room
    let current_room = &state.rooms[state.current_position as usize];
    let mut best_room_idx = 0;
    let mut best_affinity = 0u32;

    for conn_i in 0..CONNECTION_COUNT {
        let next_idx = current_room.connections[conn_i];
        if next_idx >= ROOM_COUNT as u16 {
            continue;
        }
        let next_room = state.rooms[next_idx as usize];

        // Affinity: emotional resonance + novelty - repeated paths
        let emotion_similarity = if current_room.emotional_charge > next_room.emotional_charge {
            current_room
                .emotional_charge
                .saturating_sub(next_room.emotional_charge)
        } else {
            next_room
                .emotional_charge
                .saturating_sub(current_room.emotional_charge)
        };
        let emotion_score = (500u32).saturating_sub(emotion_similarity);

        let novelty_bonus = (next_room.novelty * 60) / 1000;
        let repetition_penalty = (next_room.visit_count.min(50) * 20) / 50;

        let mut affinity = emotion_score
            .saturating_add(novelty_bonus)
            .saturating_sub(repetition_penalty);

        // Exploration drive influences path choice
        let explore_boost = (state.exploration_drive * 100) / 1000;
        affinity = affinity.saturating_add(explore_boost);

        // Occasionally choose random (lost behavior)
        if state.lost_level > 700 && (age.wrapping_mul(7) ^ 0xdead) % 4 == 0 {
            affinity = (age.wrapping_mul(11) ^ next_idx as u32) % 1000;
        }

        if affinity > best_affinity {
            best_affinity = affinity;
            best_room_idx = next_idx as usize;
        }
    }

    // Step into the chosen room
    let prev_pos = state.current_position as usize;
    state.current_position = best_room_idx as u16;
    state.last_step = prev_pos as u16;

    // Update navigation metrics
    state.rooms[best_room_idx].visit_count =
        state.rooms[best_room_idx].visit_count.saturating_add(1);

    // Detect if we're revisiting a well-worn path (shortcut)
    if state.rooms[best_room_idx].visit_count > 20 && state.rooms[prev_pos].visit_count > 20 {
        // Check if this is a distant memory reconnection (eureka)
        let dist = ((best_room_idx as i32 - prev_pos as i32).abs() as u32).min(ROOM_COUNT as u32);
        if dist > 6 {
            state.shortcut_count = state.shortcut_count.saturating_add(1);
            state.eureka_moments = state.eureka_moments.saturating_add(1);
            state.consolidation_progress =
                state.consolidation_progress.saturating_add(50).min(1000);
        }
    }

    // Lost behavior: detect dead ends (no unvisited connections)
    let mut all_visited = true;
    for conn_i in 0..CONNECTION_COUNT {
        let next_idx = state.rooms[state.current_position as usize].connections[conn_i];
        if next_idx < ROOM_COUNT as u16 && state.rooms[next_idx as usize].visit_count == 0 {
            all_visited = false;
            break;
        }
    }
    if all_visited && state.rooms[state.current_position as usize].visit_count > 5 {
        state.lost_level = state.lost_level.saturating_add(80).min(1000);
    }

    // Consolidation: integrate memory through visitation
    let visit_integration = (state.rooms[state.current_position as usize]
        .visit_count
        .min(100)
        * 30)
        / 100;
    let emotional_integration =
        (state.rooms[state.current_position as usize].emotional_charge * 25) / 1000;
    let progress_bump = visit_integration.saturating_add(emotional_integration) / 2;

    state.consolidation_progress = state
        .consolidation_progress
        .saturating_add(progress_bump)
        .min(1000);

    // Reinforcement: strengthen pathway when consolidation advances
    if state.consolidation_progress > 500 {
        state.reinforcement = 800;
    } else {
        state.reinforcement = state.reinforcement.saturating_sub(50);
    }
}

pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("═══ MEMORY LABYRINTH ═══");
    crate::serial_println!(
        "  Position: Room {} / {}",
        state.current_position,
        ROOM_COUNT
    );
    crate::serial_println!("  Exploration Drive: {}/1000", state.exploration_drive);
    crate::serial_println!("  Lost Level: {}/1000", state.lost_level);
    crate::serial_println!("  Consolidation: {}/1000", state.consolidation_progress);
    crate::serial_println!("  Maze Complexity: {}/1000", state.maze_complexity);
    crate::serial_println!("  Shortcuts Found: {}", state.shortcut_count);
    crate::serial_println!("  Eureka Moments: {}", state.eureka_moments);
    crate::serial_println!("  Reinforcement: {}/1000", state.reinforcement);

    let current = &state.rooms[state.current_position as usize];
    crate::serial_println!("  Current Room Charge: {}/1000", current.emotional_charge);
    crate::serial_println!("  Current Room Visits: {}", current.visit_count);
    crate::serial_println!("  Current Room Novelty: {}/1000", current.novelty);
}

/// Inject a new experience into the labyrinth at the current position
pub fn inject_experience(content_hash: u32, emotional_weight: u32) {
    let mut state = STATE.lock();
    let idx = state.current_position as usize;

    // Update room with new emotional tone
    let new_charge = (state.rooms[idx].emotional_charge / 2)
        .saturating_add(emotional_weight / 2)
        .min(1000);
    state.rooms[idx].emotional_charge = new_charge;

    // Boost novelty of this room
    state.rooms[idx].novelty = 900;

    // Hash the experience into the room
    state.rooms[idx].content_hash = state.rooms[idx]
        .content_hash
        .wrapping_add(content_hash)
        .wrapping_mul(0x85ebca6b);

    // Spike exploration to investigate neighbors
    state.exploration_drive = 800;
}

/// Query current state for higher-level modules
pub fn query_state() -> (u16, u32, u32, u32, u32, u32) {
    let state = STATE.lock();
    (
        state.current_position,
        state.consolidation_progress,
        state.lost_level,
        state.exploration_drive,
        state.eureka_moments,
        state.maze_complexity,
    )
}
