use crate::sync::Mutex;
/// Routing and pathfinding engine for Genesis
///
/// Implements Dijkstra's algorithm and A* search on a road network graph.
/// The road graph consists of RouteNodes (intersections) connected by
/// RouteSegments (road links) with distance and duration weights.
///
/// All coordinates are Q16 fixed-point (i32 * 65536).
/// Distances are in meters (i32). Durations in seconds (i32).
///
/// Road types affect routing preference: highways are faster but longer,
/// residential roads are shorter but slower.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// Q16 scaling constant
const Q16_ONE: i32 = 65536;

/// Maximum number of nodes the graph can hold
const MAX_GRAPH_NODES: usize = 4096;

/// Maximum number of waypoints in a route request
const MAX_WAYPOINTS: usize = 32;

/// Approximate meters per degree of latitude (Q16-friendly integer)
/// 1 degree lat ~ 111,320 meters
const METERS_PER_DEG_LAT: i32 = 111320;

/// Approximate meters per degree of longitude at equator
/// Decreases with cos(lat) but we use equatorial as upper bound
const METERS_PER_DEG_LON_EQ: i32 = 111320;

/// Road type classification for routing weight calculation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoadType {
    /// Major highway / motorway (speed ~110 km/h)
    Highway,
    /// Primary road / arterial (speed ~80 km/h)
    Primary,
    /// Secondary road / collector (speed ~60 km/h)
    Secondary,
    /// Residential street (speed ~40 km/h)
    Residential,
    /// Pedestrian path / trail (speed ~5 km/h)
    Path,
}

impl RoadType {
    /// Typical speed for this road type in meters per second (Q16).
    /// Used to estimate travel duration from distance.
    pub fn speed_mps_q16(&self) -> i32 {
        match self {
            // 110 km/h = 30.56 m/s ~ 30 * 65536 = 1966080
            RoadType::Highway => 30 * Q16_ONE,
            // 80 km/h = 22.22 m/s ~ 22 * 65536 = 1441792
            RoadType::Primary => 22 * Q16_ONE,
            // 60 km/h = 16.67 m/s ~ 17 * 65536 = 1114112
            RoadType::Secondary => 17 * Q16_ONE,
            // 40 km/h = 11.11 m/s ~ 11 * 65536 = 720896
            RoadType::Residential => 11 * Q16_ONE,
            // 5 km/h = 1.39 m/s ~ 1 * 65536 = 65536
            RoadType::Path => 1 * Q16_ONE,
        }
    }

    /// Routing preference weight multiplier (Q16).
    /// Lower = more preferred by the router.
    pub fn weight_q16(&self) -> i32 {
        match self {
            RoadType::Highway => Q16_ONE,                 // 1.0x — most preferred
            RoadType::Primary => Q16_ONE + Q16_ONE / 4,   // 1.25x
            RoadType::Secondary => Q16_ONE + Q16_ONE / 2, // 1.5x
            RoadType::Residential => 2 * Q16_ONE,         // 2.0x
            RoadType::Path => 4 * Q16_ONE,                // 4.0x — least preferred
        }
    }
}

/// A node (intersection) in the road graph.
#[derive(Debug, Clone)]
pub struct RouteNode {
    /// Unique node identifier
    pub id: u32,
    /// Latitude in Q16 fixed-point
    pub lat_q16: i32,
    /// Longitude in Q16 fixed-point
    pub lon_q16: i32,
    /// Indices of neighboring nodes in the graph's node list
    pub neighbors: Vec<u32>,
}

/// A segment (road link) connecting two nodes.
#[derive(Debug, Clone, Copy)]
pub struct RouteSegment {
    /// Source node ID
    pub from: u32,
    /// Destination node ID
    pub to: u32,
    /// Distance in meters
    pub distance_m: i32,
    /// Estimated travel duration in seconds
    pub duration_s: i32,
    /// Type of road
    pub road_type: RoadType,
}

/// A complete route from origin to destination.
#[derive(Debug, Clone)]
pub struct Route {
    /// Ordered list of node IDs along the route
    pub node_ids: Vec<u32>,
    /// Segments making up the route
    pub segments: Vec<RouteSegment>,
    /// Total distance in meters
    pub total_distance_m: i32,
    /// Total estimated duration in seconds
    pub total_duration_s: i32,
    /// Waypoints (intermediate stops)
    pub waypoints: Vec<u32>,
}

/// Internal state for Dijkstra/A* search.
#[derive(Debug, Clone, Copy)]
struct SearchNode {
    node_id: u32,
    /// Cost from start to this node
    g_cost: i32,
    /// Estimated total cost (g + heuristic) for A*
    f_cost: i32,
    /// Previous node in the optimal path
    parent: u32,
    /// Whether this node has been finalized
    visited: bool,
}

/// The road network graph.
pub struct RoadGraph {
    /// All nodes in the graph
    pub nodes: Vec<RouteNode>,
    /// All segments (edges) in the graph
    pub segments: Vec<RouteSegment>,
    /// Next available node ID
    next_id: u32,
}

/// Global road graph instance.
pub static ROAD_GRAPH: Mutex<Option<RoadGraph>> = Mutex::new(None);

impl RoadGraph {
    /// Create an empty road graph.
    pub fn new() -> Self {
        RoadGraph {
            nodes: Vec::new(),
            segments: Vec::new(),
            next_id: 1,
        }
    }

    /// Add a node to the graph at the given Q16 coordinates. Returns the node ID.
    pub fn add_node(&mut self, lat_q16: i32, lon_q16: i32) -> u32 {
        if self.nodes.len() >= MAX_GRAPH_NODES {
            serial_println!("[ROUTE] WARNING: graph full ({} nodes)", MAX_GRAPH_NODES);
            return 0;
        }

        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        self.nodes.push(RouteNode {
            id,
            lat_q16,
            lon_q16,
            neighbors: Vec::new(),
        });

        id
    }

    /// Add a bidirectional segment between two nodes.
    pub fn add_segment(&mut self, from: u32, to: u32, road_type: RoadType) -> bool {
        let from_idx = self.find_node_index(from);
        let to_idx = self.find_node_index(to);

        if from_idx.is_none() || to_idx.is_none() {
            return false;
        }

        let fi = from_idx.unwrap();
        let ti = to_idx.unwrap();

        // Calculate distance between nodes
        let distance_m = Self::calculate_distance(
            self.nodes[fi].lat_q16,
            self.nodes[fi].lon_q16,
            self.nodes[ti].lat_q16,
            self.nodes[ti].lon_q16,
        );

        // Estimate duration: distance / speed
        let speed = road_type.speed_mps_q16();
        let duration_s = if speed > 0 {
            ((distance_m as i64) * (Q16_ONE as i64) / (speed as i64)) as i32
        } else {
            distance_m // fallback: 1 second per meter
        };

        // Forward segment
        self.segments.push(RouteSegment {
            from,
            to,
            distance_m,
            duration_s,
            road_type,
        });

        // Reverse segment
        self.segments.push(RouteSegment {
            from: to,
            to: from,
            distance_m,
            duration_s,
            road_type,
        });

        // Update neighbor lists
        self.nodes[fi].neighbors.push(to);
        self.nodes[ti].neighbors.push(from);

        true
    }

    /// Find the index of a node by its ID.
    fn find_node_index(&self, id: u32) -> Option<usize> {
        for (i, node) in self.nodes.iter().enumerate() {
            if node.id == id {
                return Some(i);
            }
        }
        None
    }

    /// Get a node by ID.
    pub fn get_node(&self, id: u32) -> Option<&RouteNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Get all segments originating from a given node ID.
    pub fn get_segments_from(&self, node_id: u32) -> Vec<&RouteSegment> {
        self.segments.iter().filter(|s| s.from == node_id).collect()
    }

    /// Calculate distance in meters between two Q16 lat/lon coordinates.
    /// Uses the equirectangular approximation (fast, accurate enough
    /// for short-to-medium distances).
    pub fn calculate_distance(lat1_q16: i32, lon1_q16: i32, lat2_q16: i32, lon2_q16: i32) -> i32 {
        estimate_distance(lat1_q16, lon1_q16, lat2_q16, lon2_q16)
    }

    /// Find a route from origin to destination using A*.
    pub fn find_route(&self, origin: u32, destination: u32) -> Option<Route> {
        self.a_star(origin, destination)
    }

    /// Find a route visiting all waypoints in order using A*.
    pub fn find_route_with_waypoints(
        &self,
        origin: u32,
        destination: u32,
        waypoints: &[u32],
    ) -> Option<Route> {
        if waypoints.len() > MAX_WAYPOINTS {
            serial_println!("[ROUTE] Too many waypoints (max={})", MAX_WAYPOINTS);
            return None;
        }

        // Build a chain: origin -> wp[0] -> wp[1] -> ... -> destination
        let mut full_nodes: Vec<u32> = Vec::new();
        let mut full_segments: Vec<RouteSegment> = Vec::new();
        let mut total_distance: i32 = 0;
        let mut total_duration: i32 = 0;

        let mut stops = Vec::new();
        stops.push(origin);
        for wp in waypoints {
            stops.push(*wp);
        }
        stops.push(destination);

        let mut i = 0;
        while i + 1 < stops.len() {
            let leg = self.a_star(stops[i], stops[i + 1]);
            match leg {
                Some(route) => {
                    // Append nodes (skip first if not the first leg to avoid duplication)
                    let start_idx = if i == 0 { 0 } else { 1 };
                    let mut j = start_idx;
                    while j < route.node_ids.len() {
                        full_nodes.push(route.node_ids[j]);
                        j += 1;
                    }
                    for seg in &route.segments {
                        full_segments.push(*seg);
                    }
                    total_distance = total_distance.saturating_add(route.total_distance_m);
                    total_duration = total_duration.saturating_add(route.total_duration_s);
                }
                None => {
                    serial_println!(
                        "[ROUTE] No path found between {} and {}",
                        stops[i],
                        stops[i + 1]
                    );
                    return None;
                }
            }
            i += 1;
        }

        Some(Route {
            node_ids: full_nodes,
            segments: full_segments,
            total_distance_m: total_distance,
            total_duration_s: total_duration,
            waypoints: waypoints.to_vec(),
        })
    }

    /// A* pathfinding. Uses Euclidean distance heuristic (admissible).
    pub fn a_star(&self, origin: u32, destination: u32) -> Option<Route> {
        if self.get_node(origin).is_none() || self.get_node(destination).is_none() {
            return None;
        }

        let dest_node = self.get_node(destination).unwrap();
        let dest_lat = dest_node.lat_q16;
        let dest_lon = dest_node.lon_q16;

        // Initialize search nodes for all graph nodes
        let mut search: Vec<SearchNode> = Vec::new();
        for node in &self.nodes {
            let h = estimate_distance(node.lat_q16, node.lon_q16, dest_lat, dest_lon);
            search.push(SearchNode {
                node_id: node.id,
                g_cost: if node.id == origin { 0 } else { i32::MAX / 2 },
                f_cost: if node.id == origin { h } else { i32::MAX / 2 },
                parent: 0,
                visited: false,
            });
        }

        loop {
            // Find unvisited node with lowest f_cost
            let mut best_idx: Option<usize> = None;
            let mut best_f = i32::MAX;

            for (i, sn) in search.iter().enumerate() {
                if !sn.visited && sn.f_cost < best_f {
                    best_f = sn.f_cost;
                    best_idx = Some(i);
                }
            }

            let current_idx = match best_idx {
                Some(idx) => idx,
                None => return None, // No path exists
            };

            let current_id = search[current_idx].node_id;
            let current_g = search[current_idx].g_cost;

            // Reached destination — reconstruct path
            if current_id == destination {
                return Some(self.reconstruct_route(&search, origin, destination));
            }

            search[current_idx].visited = true;

            // Expand neighbors
            let neighbor_segments = self.get_segments_from(current_id);

            for seg in &neighbor_segments {
                // Find neighbor in search list
                let neighbor_idx = search.iter().position(|sn| sn.node_id == seg.to);
                if neighbor_idx.is_none() {
                    continue;
                }
                let ni = neighbor_idx.unwrap();

                if search[ni].visited {
                    continue;
                }

                // Cost through current node
                let weight = seg.road_type.weight_q16();
                let weighted_dist =
                    ((seg.distance_m as i64) * (weight as i64) / (Q16_ONE as i64)) as i32;
                let tentative_g = current_g.saturating_add(weighted_dist);

                if tentative_g < search[ni].g_cost {
                    let neighbor_node = self.get_node(seg.to).unwrap();
                    let h = estimate_distance(
                        neighbor_node.lat_q16,
                        neighbor_node.lon_q16,
                        dest_lat,
                        dest_lon,
                    );
                    search[ni].g_cost = tentative_g;
                    search[ni].f_cost = tentative_g.saturating_add(h);
                    search[ni].parent = current_id;
                }
            }
        }
    }

    /// Dijkstra's algorithm (A* with zero heuristic). Finds shortest
    /// weighted path. Useful when heuristic is not available.
    pub fn dijkstra(&self, origin: u32, destination: u32) -> Option<Route> {
        if self.get_node(origin).is_none() || self.get_node(destination).is_none() {
            return None;
        }

        let mut search: Vec<SearchNode> = Vec::new();
        for node in &self.nodes {
            search.push(SearchNode {
                node_id: node.id,
                g_cost: if node.id == origin { 0 } else { i32::MAX / 2 },
                f_cost: if node.id == origin { 0 } else { i32::MAX / 2 },
                parent: 0,
                visited: false,
            });
        }

        loop {
            // Find unvisited node with lowest g_cost (no heuristic)
            let mut best_idx: Option<usize> = None;
            let mut best_g = i32::MAX;

            for (i, sn) in search.iter().enumerate() {
                if !sn.visited && sn.g_cost < best_g {
                    best_g = sn.g_cost;
                    best_idx = Some(i);
                }
            }

            let current_idx = match best_idx {
                Some(idx) => idx,
                None => return None,
            };

            let current_id = search[current_idx].node_id;
            let current_g = search[current_idx].g_cost;

            if current_id == destination {
                return Some(self.reconstruct_route(&search, origin, destination));
            }

            search[current_idx].visited = true;

            let neighbor_segments = self.get_segments_from(current_id);

            for seg in &neighbor_segments {
                let neighbor_idx = search.iter().position(|sn| sn.node_id == seg.to);
                if neighbor_idx.is_none() {
                    continue;
                }
                let ni = neighbor_idx.unwrap();

                if search[ni].visited {
                    continue;
                }

                let weight = seg.road_type.weight_q16();
                let weighted_dist =
                    ((seg.distance_m as i64) * (weight as i64) / (Q16_ONE as i64)) as i32;
                let tentative_g = current_g.saturating_add(weighted_dist);

                if tentative_g < search[ni].g_cost {
                    search[ni].g_cost = tentative_g;
                    search[ni].f_cost = tentative_g;
                    search[ni].parent = current_id;
                }
            }
        }
    }

    /// Reconstruct a route from the search results by backtracking parents.
    fn reconstruct_route(&self, search: &[SearchNode], origin: u32, destination: u32) -> Route {
        let mut path_ids: Vec<u32> = Vec::new();
        let mut current = destination;

        // Backtrack from destination to origin
        loop {
            path_ids.push(current);
            if current == origin {
                break;
            }
            // Find parent
            let sn = search.iter().find(|s| s.node_id == current);
            match sn {
                Some(node) => {
                    if node.parent == 0 && current != origin {
                        break; // Broken path
                    }
                    current = node.parent;
                }
                None => break,
            }
        }

        // Reverse to get origin -> destination order
        path_ids.reverse();

        // Collect segments along the path
        let mut segments: Vec<RouteSegment> = Vec::new();
        let mut total_distance: i32 = 0;
        let mut total_duration: i32 = 0;

        let mut i = 0;
        while i + 1 < path_ids.len() {
            let from = path_ids[i];
            let to = path_ids[i + 1];

            // Find the segment
            if let Some(seg) = self.segments.iter().find(|s| s.from == from && s.to == to) {
                total_distance = total_distance.saturating_add(seg.distance_m);
                total_duration = total_duration.saturating_add(seg.duration_s);
                segments.push(*seg);
            }
            i += 1;
        }

        Route {
            node_ids: path_ids,
            segments,
            total_distance_m: total_distance,
            total_duration_s: total_duration,
            waypoints: Vec::new(),
        }
    }

    /// Add a waypoint to an existing route. Re-routes through the waypoint.
    pub fn add_waypoint(&self, route: &Route, waypoint: u32) -> Option<Route> {
        if route.node_ids.is_empty() {
            return None;
        }

        let origin = route.node_ids[0];
        let destination = *route.node_ids.last().unwrap();

        let mut waypoints = route.waypoints.clone();
        waypoints.push(waypoint);

        self.find_route_with_waypoints(origin, destination, &waypoints)
    }

    /// Get the number of nodes in the graph.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Get the number of segments in the graph.
    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }
}

/// Estimate straight-line distance in meters between two Q16 lat/lon points.
/// Uses equirectangular approximation.
pub fn estimate_distance(lat1_q16: i32, lon1_q16: i32, lat2_q16: i32, lon2_q16: i32) -> i32 {
    // Delta in degrees (still Q16)
    let dlat = (lat2_q16 as i64) - (lat1_q16 as i64);
    let dlon = (lon2_q16 as i64) - (lon1_q16 as i64);

    // Convert Q16 degrees to meters
    // meters = degrees_q16 / 65536 * meters_per_degree
    let dy = (dlat * METERS_PER_DEG_LAT as i64) / (Q16_ONE as i64);
    let dx = (dlon * METERS_PER_DEG_LON_EQ as i64) / (Q16_ONE as i64);

    // Approximate sqrt(dx^2 + dy^2) using integer math
    // |dx| + |dy| is the Manhattan distance (overestimates by ~41%)
    // Better: max(|dx|,|dy|) + min(|dx|,|dy|) * 3/8 (octagonal approximation)
    let adx = if dx < 0 { -dx } else { dx };
    let ady = if dy < 0 { -dy } else { dy };

    let (big, small) = if adx > ady { (adx, ady) } else { (ady, adx) };

    // Octagonal distance approximation:
    // d ~= 0.9604 * max + 0.3978 * min
    // Using integer fractions: (big * 983 + small * 407) / 1024
    let approx = (big * 983 + small * 407) / 1024;

    if approx > i32::MAX as i64 {
        i32::MAX
    } else {
        approx as i32
    }
}

/// Initialize the route engine subsystem.
pub fn init() {
    let graph = RoadGraph::new();
    *ROAD_GRAPH.lock() = Some(graph);
    serial_println!(
        "[ROUTE] Route engine initialized (max_nodes={}, max_waypoints={})",
        MAX_GRAPH_NODES,
        MAX_WAYPOINTS
    );
}
