pub mod layout;
pub mod smart_stack;
pub mod weather_widget;
/// Widget framework for Genesis
///
/// Home screen widgets, glanceable info, live updates,
/// widget sizing, stacking, smart suggestions.
///
/// Subsystems:
///   - widget_host:    manages widget instances on home screen
///   - widget_provider:system widget data sources (clock, weather, etc.)
///   - smart_stack:    intelligent widget stacking and suggestion
///   - weather_widget: weather data widget
///   - layout:         VBox / HBox / Grid layout engine + geometry types
///   - widget_impl:    Widget trait + Button, Label, TextInput, ProgressBar, ScrollView
///
/// Original implementation for Hoags OS.
pub mod widget_host;
pub mod widget_impl;
pub mod widget_provider;

use crate::{serial_print, serial_println};

pub fn init() {
    widget_host::init();
    widget_provider::init();
    smart_stack::init();
    layout::init();
    widget_impl::init();
    serial_println!(
        "  Widget framework initialized (host, providers, smart stack, layout, widgets)"
    );
}
