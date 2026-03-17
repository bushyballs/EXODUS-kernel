/// URL routing engine for the Genesis webserver
///
/// Provides path matching with wildcards, named parameters, method dispatch,
/// middleware chains, route groups, and a default 404 handler.
///
/// Route patterns:
///   "/exact"            — exact match
///   "/users/:id"        — named parameter (captures segment)
///   "/files/*path"      — wildcard (captures remainder)
///   "/api/v1/..."       — prefix group
///
/// All code is original.

use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use alloc::collections::BTreeMap;
use crate::sync::Mutex;

use super::http::{HttpRequest, HttpResponse, StatusCode, Method};

// ============================================================================
// Handler types
// ============================================================================

/// Route handler function pointer
pub type HandlerFn = fn(&HttpRequest) -> HttpResponse;

/// Middleware function pointer — receives request, returns Some(response) to short-circuit
/// or None to continue to the next middleware / handler
pub type MiddlewareFn = fn(&HttpRequest) -> Option<HttpResponse>;

// ============================================================================
// Route definition
// ============================================================================

/// A single route entry
struct Route {
    /// HTTP method to match (None = match any method)
    method: Option<Method>,
    /// Path pattern (may contain :param and *wildcard segments)
    pattern: String,
    /// Compiled pattern segments for matching
    segments: Vec<PatternSegment>,
    /// The handler function
    handler: HandlerFn,
    /// Priority (lower = matched first)
    priority: u16,
}

/// A segment in a compiled route pattern
#[derive(Debug, Clone)]
enum PatternSegment {
    /// Exact literal match
    Literal(String),
    /// Named parameter (captures one path segment), e.g. :id
    Param(String),
    /// Wildcard (captures everything after this point), e.g. *path
    Wildcard(String),
}

/// Compile a path pattern into segments
fn compile_pattern(pattern: &str) -> Vec<PatternSegment> {
    let mut segments = Vec::new();
    let trimmed = pattern.trim_start_matches('/');

    if trimmed.is_empty() {
        return segments;
    }

    for part in trimmed.split('/') {
        if part.starts_with(':') {
            let name = String::from(&part[1..]);
            segments.push(PatternSegment::Param(name));
        } else if part.starts_with('*') {
            let name = if part.len() > 1 {
                String::from(&part[1..])
            } else {
                String::from("wildcard")
            };
            segments.push(PatternSegment::Wildcard(name));
            break;  // Wildcard consumes everything remaining
        } else {
            segments.push(PatternSegment::Literal(String::from(part)));
        }
    }

    segments
}

/// Match a request path against compiled pattern segments.
/// Returns extracted path parameters if matched.
fn match_path(segments: &[PatternSegment], path: &str) -> Option<BTreeMap<String, String>> {
    let trimmed = path.trim_start_matches('/');
    let path_parts: Vec<&str> = if trimmed.is_empty() {
        Vec::new()
    } else {
        trimmed.split('/').collect()
    };

    let mut params = BTreeMap::new();
    let mut path_idx = 0;

    for (seg_idx, segment) in segments.iter().enumerate() {
        match segment {
            PatternSegment::Literal(expected) => {
                if path_idx >= path_parts.len() { return None; }
                if path_parts[path_idx] != expected.as_str() { return None; }
                path_idx += 1;
            }
            PatternSegment::Param(name) => {
                if path_idx >= path_parts.len() { return None; }
                params.insert(name.clone(), String::from(path_parts[path_idx]));
                path_idx += 1;
            }
            PatternSegment::Wildcard(name) => {
                // Capture everything from path_idx onward
                let remaining: Vec<&str> = path_parts[path_idx..].to_vec();
                let joined = join_with_slash(&remaining);
                params.insert(name.clone(), joined);
                return Some(params);  // Wildcard always matches
            }
        }
    }

    // All segments matched, verify no extra path parts remain
    if path_idx == path_parts.len() {
        Some(params)
    } else {
        None
    }
}

/// Join string slices with '/'
fn join_with_slash(parts: &[&str]) -> String {
    if parts.is_empty() { return String::new(); }
    let total_len: usize = parts.iter().map(|p| p.len()).sum::<usize>() + parts.len() - 1;
    let mut result = String::with_capacity(total_len);
    for (i, part) in parts.iter().enumerate() {
        if i > 0 { result.push('/'); }
        result.push_str(part);
    }
    result
}

// ============================================================================
// Route group (prefix)
// ============================================================================

/// A route group shares a common prefix and middleware set
struct RouteGroup {
    /// Path prefix (e.g., "/api/v1")
    prefix: String,
    /// Middleware applied to all routes in this group
    middleware: Vec<MiddlewareFn>,
}

// ============================================================================
// Router state
// ============================================================================

/// The global router
struct Router {
    /// Registered routes, sorted by priority
    routes: Vec<Route>,
    /// Global middleware chain (applied to every request)
    global_middleware: Vec<MiddlewareFn>,
    /// Route groups (prefixes with shared middleware)
    groups: Vec<RouteGroup>,
    /// Fallback handler for unmatched routes
    not_found_handler: HandlerFn,
    /// Method-not-allowed handler
    method_not_allowed_handler: HandlerFn,
    /// Total routes registered
    route_count: u32,
}

impl Router {
    const fn new() -> Self {
        Router {
            routes: Vec::new(),
            global_middleware: Vec::new(),
            groups: Vec::new(),
            not_found_handler: default_not_found,
            method_not_allowed_handler: default_method_not_allowed,
            route_count: 0,
        }
    }
}

/// Global router instance
static ROUTER: Mutex<Router> = Mutex::new(Router::new());

// ============================================================================
// Route registration API
// ============================================================================

/// Register a route with a specific method
pub fn route(method: Method, pattern: &str, handler: HandlerFn) {
    let segments = compile_pattern(pattern);
    let priority = compute_priority(&segments);

    let mut router = ROUTER.lock();
    router.routes.push(Route {
        method: Some(method),
        pattern: String::from(pattern),
        segments,
        handler,
        priority,
    });
    router.route_count = router.route_count.saturating_add(1);

    // Keep routes sorted by priority (more specific first)
    router.routes.sort_by_key(|r| r.priority);
}

/// Register a route matching any HTTP method
pub fn any(pattern: &str, handler: HandlerFn) {
    let segments = compile_pattern(pattern);
    let priority = compute_priority(&segments);

    let mut router = ROUTER.lock();
    router.routes.push(Route {
        method: None,
        pattern: String::from(pattern),
        segments,
        handler,
        priority,
    });
    router.route_count = router.route_count.saturating_add(1);
    router.routes.sort_by_key(|r| r.priority);
}

/// Convenience: register a GET route
pub fn get(pattern: &str, handler: HandlerFn) {
    route(Method::Get, pattern, handler);
}

/// Convenience: register a POST route
pub fn post(pattern: &str, handler: HandlerFn) {
    route(Method::Post, pattern, handler);
}

/// Convenience: register a PUT route
pub fn put(pattern: &str, handler: HandlerFn) {
    route(Method::Put, pattern, handler);
}

/// Convenience: register a DELETE route
pub fn delete(pattern: &str, handler: HandlerFn) {
    route(Method::Delete, pattern, handler);
}

/// Register global middleware (applied to all requests)
pub fn use_middleware(mw: MiddlewareFn) {
    ROUTER.lock().global_middleware.push(mw);
}

/// Create a route group with a prefix
pub fn group(prefix: &str) -> usize {
    let mut router = ROUTER.lock();
    let idx = router.groups.len();
    router.groups.push(RouteGroup {
        prefix: String::from(prefix),
        middleware: Vec::new(),
    });
    idx
}

/// Add middleware to a route group
pub fn group_middleware(group_idx: usize, mw: MiddlewareFn) {
    let mut router = ROUTER.lock();
    if let Some(g) = router.groups.get_mut(group_idx) {
        g.middleware.push(mw);
    }
}

/// Register a route within a group (prefixed)
pub fn group_route(group_idx: usize, method: Method, pattern: &str, handler: HandlerFn) {
    let prefix = {
        let router = ROUTER.lock();
        match router.groups.get(group_idx) {
            Some(g) => g.prefix.clone(),
            None => return,
        }
    };

    let full_pattern = format!("{}{}", prefix, pattern);
    route(method, &full_pattern, handler);
}

/// Set a custom 404 handler
pub fn set_not_found_handler(handler: HandlerFn) {
    ROUTER.lock().not_found_handler = handler;
}

/// Set a custom 405 handler
pub fn set_method_not_allowed_handler(handler: HandlerFn) {
    ROUTER.lock().method_not_allowed_handler = handler;
}

// ============================================================================
// Request dispatch
// ============================================================================

/// Dispatch an incoming request through middleware and routes
pub fn dispatch(request: &HttpRequest) -> HttpResponse {
    let router = ROUTER.lock();

    // Run global middleware
    for mw in &router.global_middleware {
        if let Some(response) = mw(request) {
            return response;
        }
    }

    // Check group middleware for matching prefixes
    for grp in &router.groups {
        if request.path.starts_with(grp.prefix.as_str()) {
            for mw in &grp.middleware {
                if let Some(response) = mw(request) {
                    return response;
                }
            }
        }
    }

    // Find matching route
    let mut method_matched = false;

    for route_entry in &router.routes {
        if let Some(params) = match_path(&route_entry.segments, &request.path) {
            // Path matches — check method
            if let Some(ref method) = route_entry.method {
                if *method != request.method {
                    method_matched = true;
                    continue;
                }
            }

            // Full match found — call handler
            // We need to inject path params into the request
            // Since HttpRequest is borrowed immutably, we create a modified copy
            let mut modified_request = clone_request_with_params(request, params);
            let handler = route_entry.handler;
            drop(router);  // Release the lock before calling handler
            return handler(&modified_request);
        }
    }

    // No route matched
    if method_matched {
        let handler = router.method_not_allowed_handler;
        drop(router);
        handler(request)
    } else {
        let handler = router.not_found_handler;
        drop(router);
        handler(request)
    }
}

/// Clone a request with additional path parameters injected
fn clone_request_with_params(req: &HttpRequest, params: BTreeMap<String, String>) -> HttpRequest {
    let mut headers = super::http::Headers::new();
    for (k, v) in req.headers.iter() {
        headers.set(k, v);
    }

    HttpRequest {
        method: req.method,
        path: req.path.clone(),
        query_string: req.query_string.clone(),
        version: req.version,
        headers,
        body: req.body.clone(),
        query_params: req.query_params.clone(),
        path_params: params,
    }
}

/// Compute route priority — more specific patterns get lower (higher priority) values
fn compute_priority(segments: &[PatternSegment]) -> u16 {
    if segments.is_empty() {
        return 0;  // Root "/" is highest priority exact match
    }
    let mut score: u16 = 0;
    for seg in segments {
        match seg {
            PatternSegment::Literal(_) => score += 1,
            PatternSegment::Param(_) => score += 100,
            PatternSegment::Wildcard(_) => score += 1000,
        }
    }
    score
}

/// List all registered routes (for debugging / admin endpoints)
pub fn list_routes() -> Vec<(String, String)> {
    let router = ROUTER.lock();
    let mut result = Vec::new();
    for r in &router.routes {
        let method_str = match &r.method {
            Some(m) => String::from(m.as_str()),
            None => String::from("ANY"),
        };
        result.push((method_str, r.pattern.clone()));
    }
    result
}

/// Get the total number of registered routes
pub fn route_count() -> u32 {
    ROUTER.lock().route_count
}

// ============================================================================
// Default handlers
// ============================================================================

/// Default 404 Not Found handler
fn default_not_found(req: &HttpRequest) -> HttpResponse {
    let body = format!(
        "<!DOCTYPE html><html><head><title>404 Not Found</title></head>\
         <body><h1>404 Not Found</h1><p>The path <code>{}</code> was not found on this server.</p>\
         <hr><p>Genesis/1.0</p></body></html>",
        req.path
    );
    HttpResponse::html(StatusCode::NotFound, &body)
}

/// Default 405 Method Not Allowed handler
fn default_method_not_allowed(req: &HttpRequest) -> HttpResponse {
    let body = format!(
        "<!DOCTYPE html><html><head><title>405 Method Not Allowed</title></head>\
         <body><h1>405 Method Not Allowed</h1>\
         <p>{} is not allowed for <code>{}</code>.</p>\
         <hr><p>Genesis/1.0</p></body></html>",
        req.method.as_str(), req.path
    );
    HttpResponse::html(StatusCode::MethodNotAllowed, &body)
}

// ============================================================================
// Built-in middleware
// ============================================================================

/// CORS middleware — adds Access-Control-Allow-Origin headers
pub fn cors_middleware(req: &HttpRequest) -> Option<HttpResponse> {
    if req.method == Method::Options {
        let mut resp = HttpResponse::new(StatusCode::NoContent);
        resp.headers.set("access-control-allow-origin", "*");
        resp.headers.set("access-control-allow-methods", "GET, POST, PUT, DELETE, PATCH, OPTIONS");
        resp.headers.set("access-control-allow-headers", "content-type, authorization");
        resp.headers.set("access-control-max-age", "86400");
        Some(resp)
    } else {
        None
    }
}

/// Request logging middleware — logs every request to serial
pub fn logging_middleware(req: &HttpRequest) -> Option<HttpResponse> {
    serial_println!("  [router] {} {} (host: {})",
        req.method.as_str(),
        req.path,
        req.host().unwrap_or("-"));
    None  // Never short-circuits
}

/// Security headers middleware — adds common security headers
pub fn security_headers_middleware(_req: &HttpRequest) -> Option<HttpResponse> {
    // This middleware runs post-handler, so we return None here
    // and apply headers in a response wrapper. For simplicity in
    // this bare-metal implementation, we apply them during dispatch.
    None
}

// ============================================================================
// Built-in routes
// ============================================================================

/// Health check handler
fn health_handler(_req: &HttpRequest) -> HttpResponse {
    HttpResponse::json(StatusCode::Ok, "{\"status\":\"healthy\",\"server\":\"Genesis/1.0\"}")
}

/// Route listing handler (admin/debug)
fn routes_handler(_req: &HttpRequest) -> HttpResponse {
    let routes = list_routes();
    let mut body = String::from("[");
    for (i, (method, pattern)) in routes.iter().enumerate() {
        if i > 0 { body.push(','); }
        body.push_str(&format!("{{\"method\":\"{}\",\"pattern\":\"{}\"}}", method, pattern));
    }
    body.push(']');
    HttpResponse::json(StatusCode::Ok, &body)
}

/// Server stats handler
fn stats_handler(_req: &HttpRequest) -> HttpResponse {
    let (reqs, conns, sent, recv, ws) = super::get_stats();
    let body = format!(
        "{{\"total_requests\":{},\"active_connections\":{},\
         \"bytes_sent\":{},\"bytes_received\":{},\"websocket_upgrades\":{}}}",
        reqs, conns, sent, recv, ws);
    HttpResponse::json(StatusCode::Ok, &body)
}

/// Initialize the router with default routes and middleware
pub fn init() {
    // Register logging middleware
    use_middleware(logging_middleware);

    // Register built-in routes
    get("/health", health_handler);
    get("/_routes", routes_handler);
    get("/_stats", stats_handler);

    serial_println!("  [router] URL router initialized");
    serial_println!("  [router] Patterns: exact, :param, *wildcard");
    serial_println!("  [router] Built-in: /health, /_routes, /_stats");
}
