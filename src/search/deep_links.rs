use crate::sync::Mutex;
/// Deep links / app links for Genesis
///
/// URI scheme handling, verified app links,
/// default handler management.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy)]
struct DeepLink {
    id: u32,
    scheme_hash: u64,
    host_hash: u64,
    path_hash: u64,
    target_app_id: u32,
    verified: bool,
    auto_verify: bool,
}

struct DeepLinkRouter {
    links: Vec<DeepLink>,
    default_handlers: Vec<(u64, u32)>, // (domain_hash, app_id)
    next_id: u32,
    total_resolved: u32,
}

static DEEP_LINKS: Mutex<Option<DeepLinkRouter>> = Mutex::new(None);

impl DeepLinkRouter {
    fn new() -> Self {
        DeepLinkRouter {
            links: Vec::new(),
            default_handlers: Vec::new(),
            next_id: 1,
            total_resolved: 0,
        }
    }

    fn register_link(
        &mut self,
        scheme: u64,
        host: u64,
        path: u64,
        app_id: u32,
        auto_verify: bool,
    ) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.links.push(DeepLink {
            id,
            scheme_hash: scheme,
            host_hash: host,
            path_hash: path,
            target_app_id: app_id,
            verified: false,
            auto_verify,
        });
        id
    }

    fn resolve(&mut self, scheme: u64, host: u64) -> Option<u32> {
        self.total_resolved = self.total_resolved.saturating_add(1);
        // Check verified links first
        for link in &self.links {
            if link.scheme_hash == scheme && link.host_hash == host && link.verified {
                return Some(link.target_app_id);
            }
        }
        // Check default handlers
        for &(domain, app_id) in &self.default_handlers {
            if domain == host {
                return Some(app_id);
            }
        }
        // Any matching link
        self.links
            .iter()
            .find(|l| l.scheme_hash == scheme && l.host_hash == host)
            .map(|l| l.target_app_id)
    }

    fn set_default_handler(&mut self, domain_hash: u64, app_id: u32) {
        for entry in &mut self.default_handlers {
            if entry.0 == domain_hash {
                entry.1 = app_id;
                return;
            }
        }
        self.default_handlers.push((domain_hash, app_id));
    }

    fn verify_link(&mut self, link_id: u32) {
        if let Some(link) = self.links.iter_mut().find(|l| l.id == link_id) {
            link.verified = true;
        }
    }
}

pub fn init() {
    let mut dl = DEEP_LINKS.lock();
    *dl = Some(DeepLinkRouter::new());
    serial_println!("    Deep link router ready");
}
