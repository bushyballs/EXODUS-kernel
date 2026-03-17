pub mod ai_notifications;
pub mod bubbles;
/// Notifications for Genesis
pub mod channels;
pub mod grouping;

/// Initialize the entire notifications subsystem
pub fn init() {
    channels::init();
    bubbles::init();
    grouping::init();
    ai_notifications::init();
}
