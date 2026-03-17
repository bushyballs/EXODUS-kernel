use super::route_engine::{estimate_distance, RoadGraph, Route};
use crate::sync::Mutex;
/// Turn-by-turn navigation for Genesis
///
/// Builds on the route_engine to provide real-time navigation guidance.
/// Generates a sequence of maneuver instructions (turns, merges, exits)
/// and tracks the user's progress along the route.
///
/// Features:
///   - Step-by-step instruction generation from a computed route
///   - Position tracking with off-route detection
///   - Automatic recalculation when user deviates from route
///   - ETA estimation based on remaining distance and road speeds
///
/// All coordinates are Q16 fixed-point (i32 * 65536).
/// Distances in meters (i32). Durations in seconds (i32).
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// Q16 scaling constant
const Q16_ONE: i32 = 65536;

/// Distance threshold in meters for "arrived at waypoint" detection
const ARRIVAL_THRESHOLD_M: i32 = 30;

/// Distance threshold in meters for off-route detection
const OFF_ROUTE_THRESHOLD_M: i32 = 50;

/// Distance threshold to advance to next instruction step
const STEP_ADVANCE_THRESHOLD_M: i32 = 25;

/// Maximum angle delta (Q16 degrees) considered "straight ahead"
/// 15 degrees * 65536 = 983040
const STRAIGHT_THRESHOLD_Q16: i32 = 15 * Q16_ONE;

/// Angle threshold for U-turn detection (150 degrees)
const UTURN_THRESHOLD_Q16: i32 = 150 * Q16_ONE;

/// Maximum number of recalculation attempts before giving up
const MAX_RECALC_ATTEMPTS: u32 = 5;

/// Maneuver type for a navigation instruction step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Maneuver {
    /// Continue straight ahead
    Straight,
    /// Turn left at intersection
    TurnLeft,
    /// Turn right at intersection
    TurnRight,
    /// Make a U-turn
    UTurn,
    /// Merge into left lane
    MergeLeft,
    /// Merge into right lane
    MergeRight,
    /// Take highway exit ramp
    ExitRamp,
    /// Arrived at destination or waypoint
    Arrive,
    /// Depart from origin
    Depart,
    /// Enter or navigate a roundabout
    Roundabout,
}

impl Maneuver {
    /// Human-readable description of the maneuver.
    pub fn description(&self) -> &'static str {
        match self {
            Maneuver::Straight => "Continue straight",
            Maneuver::TurnLeft => "Turn left",
            Maneuver::TurnRight => "Turn right",
            Maneuver::UTurn => "Make a U-turn",
            Maneuver::MergeLeft => "Merge left",
            Maneuver::MergeRight => "Merge right",
            Maneuver::ExitRamp => "Take the exit",
            Maneuver::Arrive => "You have arrived",
            Maneuver::Depart => "Depart",
            Maneuver::Roundabout => "Enter the roundabout",
        }
    }
}

/// Cardinal/intercardinal direction for instruction display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    North,
    NorthEast,
    East,
    SouthEast,
    South,
    SouthWest,
    West,
    NorthWest,
}

impl Direction {
    /// Determine direction from a bearing angle (Q16 degrees, 0 = north, clockwise).
    pub fn from_bearing_q16(bearing_q16: i32) -> Self {
        // Normalize to 0..360 * Q16
        let full = 360 * Q16_ONE;
        let mut b = bearing_q16 % full;
        if b < 0 {
            b += full;
        }

        let sector = b / (45 * Q16_ONE);
        match sector {
            0 => Direction::North,
            1 => Direction::NorthEast,
            2 => Direction::East,
            3 => Direction::SouthEast,
            4 => Direction::South,
            5 => Direction::SouthWest,
            6 => Direction::West,
            _ => Direction::NorthWest,
        }
    }

    /// Short label for the direction.
    pub fn label(&self) -> &'static str {
        match self {
            Direction::North => "N",
            Direction::NorthEast => "NE",
            Direction::East => "E",
            Direction::SouthEast => "SE",
            Direction::South => "S",
            Direction::SouthWest => "SW",
            Direction::West => "W",
            Direction::NorthWest => "NW",
        }
    }
}

/// A single navigation instruction.
#[derive(Debug, Clone)]
pub struct NavInstruction {
    /// Step index in the instruction list (0-based)
    pub step: u32,
    /// Type of maneuver to perform
    pub maneuver: Maneuver,
    /// Distance to this maneuver from the previous step (meters)
    pub distance_m: i32,
    /// Hash identifying the street name (actual string lookup external)
    pub street_hash: u64,
    /// Heading direction after the maneuver
    pub direction: Direction,
    /// Latitude at this step (Q16)
    pub lat_q16: i32,
    /// Longitude at this step (Q16)
    pub lon_q16: i32,
}

/// An active navigation session.
pub struct NavSession {
    /// The computed route being followed
    pub route: Route,
    /// Generated instruction list
    pub instructions: Vec<NavInstruction>,
    /// Index of the current step in the instruction list
    pub current_step: usize,
    /// Whether the user is currently off-route
    pub off_route: bool,
    /// Estimated time of arrival in seconds from now
    pub eta_seconds: i32,
    /// Remaining distance to destination in meters
    pub distance_remaining: i32,
    /// Current user position: latitude (Q16)
    pub user_lat_q16: i32,
    /// Current user position: longitude (Q16)
    pub user_lon_q16: i32,
    /// Origin node ID
    pub origin: u32,
    /// Destination node ID
    pub destination: u32,
    /// Number of recalculations performed
    pub recalc_count: u32,
    /// Whether navigation is active
    pub active: bool,
}

/// Global navigation session.
pub static NAV_SESSION: Mutex<Option<NavSession>> = Mutex::new(None);

impl NavSession {
    /// Create a new navigation session (not yet started).
    fn new_empty() -> Self {
        NavSession {
            route: Route {
                node_ids: Vec::new(),
                segments: Vec::new(),
                total_distance_m: 0,
                total_duration_s: 0,
                waypoints: Vec::new(),
            },
            instructions: Vec::new(),
            current_step: 0,
            off_route: false,
            eta_seconds: 0,
            distance_remaining: 0,
            user_lat_q16: 0,
            user_lon_q16: 0,
            origin: 0,
            destination: 0,
            recalc_count: 0,
            active: false,
        }
    }

    /// Generate navigation instructions from the current route.
    /// Walks the route segments and classifies each turn.
    fn generate_instructions(&mut self, graph: &RoadGraph) {
        self.instructions.clear();

        if self.route.node_ids.is_empty() {
            return;
        }

        // Depart instruction
        let first_id = self.route.node_ids[0];
        if let Some(first_node) = graph.get_node(first_id) {
            let initial_direction = if self.route.node_ids.len() > 1 {
                let second_id = self.route.node_ids[1];
                if let Some(second_node) = graph.get_node(second_id) {
                    let bearing = compute_bearing(
                        first_node.lat_q16,
                        first_node.lon_q16,
                        second_node.lat_q16,
                        second_node.lon_q16,
                    );
                    Direction::from_bearing_q16(bearing)
                } else {
                    Direction::North
                }
            } else {
                Direction::North
            };

            self.instructions.push(NavInstruction {
                step: 0,
                maneuver: Maneuver::Depart,
                distance_m: 0,
                street_hash: compute_street_hash(first_id, first_id),
                direction: initial_direction,
                lat_q16: first_node.lat_q16,
                lon_q16: first_node.lon_q16,
            });
        }

        // Generate maneuver at each node along the route
        let mut step_counter: u32 = 1;
        let mut i = 1;
        while i < self.route.node_ids.len() {
            let prev_id = self.route.node_ids[i - 1];
            let curr_id = self.route.node_ids[i];

            let prev_node = graph.get_node(prev_id);
            let curr_node = graph.get_node(curr_id);

            if prev_node.is_none() || curr_node.is_none() {
                i += 1;
                continue;
            }

            let pn = prev_node.unwrap();
            let cn = curr_node.unwrap();

            // Distance of this segment
            let seg_dist = estimate_distance(pn.lat_q16, pn.lon_q16, cn.lat_q16, cn.lon_q16);

            // Determine the maneuver type based on angle change
            let maneuver = if i + 1 < self.route.node_ids.len() {
                let next_id = self.route.node_ids[i + 1];
                if let Some(nn) = graph.get_node(next_id) {
                    classify_maneuver(pn, cn, nn)
                } else {
                    Maneuver::Straight
                }
            } else {
                // Last node -> arrival
                Maneuver::Arrive
            };

            // Compute bearing from prev to current
            let bearing = compute_bearing(pn.lat_q16, pn.lon_q16, cn.lat_q16, cn.lon_q16);
            let direction = Direction::from_bearing_q16(bearing);

            self.instructions.push(NavInstruction {
                step: step_counter,
                maneuver,
                distance_m: seg_dist,
                street_hash: compute_street_hash(prev_id, curr_id),
                direction,
                lat_q16: cn.lat_q16,
                lon_q16: cn.lon_q16,
            });

            step_counter += 1;
            i += 1;
        }
    }

    /// Get the current (next upcoming) navigation instruction.
    pub fn get_next_instruction(&self) -> Option<&NavInstruction> {
        if self.current_step < self.instructions.len() {
            Some(&self.instructions[self.current_step])
        } else {
            None
        }
    }

    /// Get all remaining instructions from the current step onward.
    pub fn get_remaining_instructions(&self) -> &[NavInstruction] {
        if self.current_step < self.instructions.len() {
            &self.instructions[self.current_step..]
        } else {
            &[]
        }
    }

    /// Update the user's current position and advance navigation state.
    /// Returns true if the position was accepted, false if off-route.
    pub fn update_position(&mut self, lat_q16: i32, lon_q16: i32) -> bool {
        self.user_lat_q16 = lat_q16;
        self.user_lon_q16 = lon_q16;

        if !self.active || self.instructions.is_empty() {
            return false;
        }

        // Check if we've reached the current instruction's location
        if let Some(instr) = self.instructions.get(self.current_step) {
            let dist_to_step = estimate_distance(lat_q16, lon_q16, instr.lat_q16, instr.lon_q16);

            if dist_to_step <= STEP_ADVANCE_THRESHOLD_M {
                // Advance to next step
                if instr.maneuver == Maneuver::Arrive {
                    // We've arrived at the destination
                    self.active = false;
                    self.distance_remaining = 0;
                    self.eta_seconds = 0;
                    serial_println!("[NAV] Arrived at destination");
                    return true;
                }

                self.current_step = self.current_step.saturating_add(1);
                serial_println!(
                    "[NAV] Advanced to step {}/{}",
                    self.current_step,
                    self.instructions.len()
                );
            }
        }

        // Check if off-route by measuring distance to nearest route segment
        let off_route = self.is_off_route(lat_q16, lon_q16);
        self.off_route = off_route;

        // Update distance remaining and ETA
        self.update_eta();

        !off_route
    }

    /// Check if the given position is too far from the route.
    pub fn is_off_route(&self, lat_q16: i32, lon_q16: i32) -> bool {
        // Find minimum distance to any remaining instruction point
        let mut min_dist = i32::MAX;

        let mut i = self.current_step;
        while i < self.instructions.len() {
            let instr = &self.instructions[i];
            let dist = estimate_distance(lat_q16, lon_q16, instr.lat_q16, instr.lon_q16);
            if dist < min_dist {
                min_dist = dist;
            }
            i += 1;
        }

        min_dist > OFF_ROUTE_THRESHOLD_M
    }

    /// Recalculate the route from the current position.
    /// Uses the road graph to find a new path to the destination.
    pub fn recalculate(&mut self, graph: &RoadGraph) -> bool {
        if self.recalc_count >= MAX_RECALC_ATTEMPTS {
            serial_println!(
                "[NAV] Max recalculation attempts reached ({})",
                MAX_RECALC_ATTEMPTS
            );
            return false;
        }

        self.recalc_count = self.recalc_count.saturating_add(1);
        serial_println!("[NAV] Recalculating route (attempt {})", self.recalc_count);

        // Find the nearest node to the current position
        let nearest = find_nearest_node(graph, self.user_lat_q16, self.user_lon_q16);
        if nearest == 0 {
            serial_println!("[NAV] No nearby node found for recalculation");
            return false;
        }

        // Compute new route
        let new_route = graph.find_route(nearest, self.destination);
        match new_route {
            Some(route) => {
                self.route = route;
                self.current_step = 0;
                self.off_route = false;
                self.generate_instructions(graph);
                self.update_eta();
                serial_println!(
                    "[NAV] Route recalculated: {}m, {}s",
                    self.route.total_distance_m,
                    self.route.total_duration_s
                );
                true
            }
            None => {
                serial_println!("[NAV] Recalculation failed: no route found");
                false
            }
        }
    }

    /// Update the ETA and remaining distance based on current position.
    fn update_eta(&mut self) {
        let mut remaining_m: i32 = 0;
        let mut remaining_s: i32 = 0;

        // Sum distance and duration from current step to end
        let mut i = self.current_step;
        while i < self.instructions.len() {
            remaining_m = remaining_m.saturating_add(self.instructions[i].distance_m);
            i += 1;
        }

        // Estimate time based on remaining segments
        let seg_start = if self.current_step > 0 {
            self.current_step - 1
        } else {
            0
        };
        let mut j = seg_start;
        while j < self.route.segments.len() {
            remaining_s = remaining_s.saturating_add(self.route.segments[j].duration_s);
            j += 1;
        }

        // Add distance from current position to next instruction point
        if let Some(next) = self.instructions.get(self.current_step) {
            let dist_to_next = estimate_distance(
                self.user_lat_q16,
                self.user_lon_q16,
                next.lat_q16,
                next.lon_q16,
            );
            remaining_m = remaining_m.saturating_add(dist_to_next);
            // Estimate time at ~15 m/s average speed
            let extra_s = dist_to_next / 15;
            remaining_s = remaining_s.saturating_add(extra_s);
        }

        self.distance_remaining = remaining_m;
        self.eta_seconds = remaining_s;
    }

    /// Get the current ETA in seconds.
    pub fn get_eta(&self) -> i32 {
        self.eta_seconds
    }

    /// Get remaining distance in meters.
    pub fn get_distance_remaining(&self) -> i32 {
        self.distance_remaining
    }

    /// Get the progress as a percentage (0-100) in Q16.
    pub fn get_progress_q16(&self) -> i32 {
        if self.route.total_distance_m == 0 {
            return 0;
        }
        let traveled = self.route.total_distance_m - self.distance_remaining;
        if traveled < 0 {
            return 0;
        }
        let pct =
            ((traveled as i64) * 100 * (Q16_ONE as i64)) / (self.route.total_distance_m as i64);
        pct as i32
    }
}

/// Start a new navigation session on the given route.
pub fn start_nav(graph: &RoadGraph, route: Route, origin: u32, destination: u32) -> bool {
    let mut session = NavSession::new_empty();
    session.route = route;
    session.origin = origin;
    session.destination = destination;
    session.active = true;

    // Generate instructions
    session.generate_instructions(graph);
    session.update_eta();

    let step_count = session.instructions.len();
    let dist = session.route.total_distance_m;
    let dur = session.route.total_duration_s;

    *NAV_SESSION.lock() = Some(session);

    serial_println!(
        "[NAV] Navigation started: {} steps, {}m, {}s ETA",
        step_count,
        dist,
        dur
    );
    true
}

/// Stop the current navigation session.
pub fn stop_nav() {
    let mut guard = NAV_SESSION.lock();
    if let Some(ref mut session) = *guard {
        session.active = false;
        serial_println!("[NAV] Navigation stopped");
    }
    *guard = None;
}

/// Classify the maneuver at node `current` given the approach from `prev`
/// and the departure toward `next`.
fn classify_maneuver(
    prev: &super::route_engine::RouteNode,
    current: &super::route_engine::RouteNode,
    next: &super::route_engine::RouteNode,
) -> Maneuver {
    // Compute incoming bearing (prev -> current)
    let bearing_in = compute_bearing(prev.lat_q16, prev.lon_q16, current.lat_q16, current.lon_q16);

    // Compute outgoing bearing (current -> next)
    let bearing_out = compute_bearing(current.lat_q16, current.lon_q16, next.lat_q16, next.lon_q16);

    // Turn angle = difference between outgoing and incoming bearings
    let full_circle = 360 * Q16_ONE;
    let mut turn_angle = bearing_out - bearing_in;

    // Normalize to -180..180 degrees (Q16)
    let half_circle = 180 * Q16_ONE;
    while turn_angle > half_circle {
        turn_angle -= full_circle;
    }
    while turn_angle < -half_circle {
        turn_angle += full_circle;
    }

    let abs_angle = if turn_angle < 0 {
        -turn_angle
    } else {
        turn_angle
    };

    if abs_angle < STRAIGHT_THRESHOLD_Q16 {
        Maneuver::Straight
    } else if abs_angle > UTURN_THRESHOLD_Q16 {
        Maneuver::UTurn
    } else if turn_angle > 0 {
        // Positive angle = turning right
        if abs_angle > 60 * Q16_ONE {
            Maneuver::TurnRight
        } else {
            Maneuver::MergeRight
        }
    } else {
        // Negative angle = turning left
        if abs_angle > 60 * Q16_ONE {
            Maneuver::TurnLeft
        } else {
            Maneuver::MergeLeft
        }
    }
}

/// Compute bearing in Q16 degrees (0 = north, clockwise) between two points.
fn compute_bearing(lat1_q16: i32, lon1_q16: i32, lat2_q16: i32, lon2_q16: i32) -> i32 {
    let dlat = (lat2_q16 as i64) - (lat1_q16 as i64);
    let dlon = (lon2_q16 as i64) - (lon1_q16 as i64);

    // atan2 approximation using octant decomposition
    // We compute the angle in Q16 degrees
    if dlat == 0 && dlon == 0 {
        return 0;
    }

    let adlat = if dlat < 0 { -dlat } else { dlat };
    let adlon = if dlon < 0 { -dlon } else { dlon };

    // Approximate atan2 using the ratio and quadrant
    // angle = atan(dlon / dlat) in degrees, adjusted by quadrant
    let ratio_q16 = if adlat > adlon {
        // angle is closer to 0 or 180
        ((adlon * Q16_ONE as i64) / (adlat.max(1))) as i32
    } else {
        // angle is closer to 90 or 270
        ((adlat * Q16_ONE as i64) / (adlon.max(1))) as i32
    };

    // Linear approximation of atan: atan(x) ~ x * 45/65536 for small x (in degrees)
    // More precisely: atan(ratio_q16 / 65536) * (180/pi) * 65536
    // Simplified: angle_q16 ~ ratio * 45 * 65536 / 65536 = ratio * 45
    // But ratio is already Q16, so: angle_q16 = ratio_q16 * 45 / 65536 * 65536
    // That simplifies to: angle_q16 = ratio_q16 * 45
    // Actually for atan approximation of the full range 0..1:
    // atan(x) ~ x * 45 degrees for x in [0, 1]
    // ratio_q16 is in [0, 65536], so:
    let base_angle = ((ratio_q16 as i64) * 45) as i32;

    // Determine quadrant-adjusted bearing
    let bearing = if adlat > adlon {
        // Primary direction is N or S
        if dlat > 0 && dlon >= 0 {
            // NE quadrant: bearing = base_angle
            base_angle
        } else if dlat > 0 && dlon < 0 {
            // NW quadrant: bearing = 360 - base_angle
            360 * Q16_ONE - base_angle
        } else if dlat < 0 && dlon >= 0 {
            // SE quadrant: bearing = 180 - base_angle
            180 * Q16_ONE - base_angle
        } else {
            // SW quadrant: bearing = 180 + base_angle
            180 * Q16_ONE + base_angle
        }
    } else {
        // Primary direction is E or W
        if dlon > 0 && dlat >= 0 {
            // NE quadrant: bearing = 90 - base_angle
            90 * Q16_ONE - base_angle
        } else if dlon > 0 && dlat < 0 {
            // SE quadrant: bearing = 90 + base_angle
            90 * Q16_ONE + base_angle
        } else if dlon < 0 && dlat >= 0 {
            // NW quadrant: bearing = 270 + base_angle
            270 * Q16_ONE + base_angle
        } else {
            // SW quadrant: bearing = 270 - base_angle
            270 * Q16_ONE - base_angle
        }
    };

    // Normalize to 0..360
    let full = 360 * Q16_ONE;
    let mut b = bearing % full;
    if b < 0 {
        b += full;
    }
    b
}

/// Compute a street hash from segment node IDs.
/// Used as a placeholder for street name identification.
fn compute_street_hash(from_id: u32, to_id: u32) -> u64 {
    let mut hash: u64 = 0xCBF29CE484222325;
    let prime: u64 = 0x00000100000001B3;

    hash ^= from_id as u64;
    hash = hash.wrapping_mul(prime);
    hash ^= to_id as u64;
    hash = hash.wrapping_mul(prime);

    hash
}

/// Find the nearest node in the graph to a Q16 lat/lon position.
/// Returns the node ID, or 0 if the graph is empty.
fn find_nearest_node(graph: &RoadGraph, lat_q16: i32, lon_q16: i32) -> u32 {
    let mut best_id: u32 = 0;
    let mut best_dist = i32::MAX;

    for node in &graph.nodes {
        let dist = estimate_distance(lat_q16, lon_q16, node.lat_q16, node.lon_q16);
        if dist < best_dist {
            best_dist = dist;
            best_id = node.id;
        }
    }

    best_id
}

/// Initialize the navigation subsystem.
pub fn init() {
    *NAV_SESSION.lock() = None;
    serial_println!(
        "[NAV] Navigation subsystem initialized (off_route={}m, arrive={}m)",
        OFF_ROUTE_THRESHOLD_M,
        ARRIVAL_THRESHOLD_M
    );
}
