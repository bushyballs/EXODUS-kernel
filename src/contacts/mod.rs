pub mod ai_contacts;
pub mod calendar;
pub mod contact_book;
/// Contacts & calendar for Genesis
pub mod contact_provider;
pub mod event_sync;
pub mod groups;

pub fn init() {
    contact_provider::init();
    calendar::init();
    event_sync::init();
    groups::init();
    ai_contacts::init();
    contact_book::init();
}
