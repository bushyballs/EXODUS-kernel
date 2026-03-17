use crate::sync::Mutex;
/// Power button menu (shutdown, restart, sleep)
///
/// Part of the Genesis System UI. Displays system power
/// options when the power key is long-pressed.
use alloc::vec::Vec;

/// Power action the user can select
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerAction {
    Shutdown,
    Restart,
    Sleep,
    Hibernate,
    Cancel,
}

pub struct PowerMenu {
    pub visible: bool,
    pub selected: PowerAction,
}

impl PowerMenu {
    pub fn new() -> Self {
        PowerMenu {
            visible: false,
            selected: PowerAction::Shutdown,
        }
    }

    /// Show the power menu overlay.
    pub fn show(&mut self) {
        self.visible = true;
        self.selected = PowerAction::Shutdown; // Default selection
        crate::serial_println!("  [power_menu] displayed");
    }

    /// Hide the power menu without executing an action.
    pub fn hide(&mut self) {
        self.visible = false;
        crate::serial_println!("  [power_menu] hidden");
    }

    /// Move the selection to the next power action.
    pub fn select_next(&mut self) {
        self.selected = match self.selected {
            PowerAction::Shutdown => PowerAction::Restart,
            PowerAction::Restart => PowerAction::Sleep,
            PowerAction::Sleep => PowerAction::Hibernate,
            PowerAction::Hibernate => PowerAction::Cancel,
            PowerAction::Cancel => PowerAction::Shutdown,
        };
        crate::serial_println!("  [power_menu] selected: {:?}", self.selected);
    }

    /// Move the selection to the previous power action.
    pub fn select_prev(&mut self) {
        self.selected = match self.selected {
            PowerAction::Shutdown => PowerAction::Cancel,
            PowerAction::Restart => PowerAction::Shutdown,
            PowerAction::Sleep => PowerAction::Restart,
            PowerAction::Hibernate => PowerAction::Sleep,
            PowerAction::Cancel => PowerAction::Hibernate,
        };
        crate::serial_println!("  [power_menu] selected: {:?}", self.selected);
    }

    /// Execute the currently selected power action.
    ///
    /// In a real system this would trigger ACPI/platform power management.
    /// Here we log the intent and hide the menu.
    pub fn execute(&self) {
        match self.selected {
            PowerAction::Shutdown => {
                crate::serial_println!("  [power_menu] EXECUTE: system shutdown requested");
                // In a real kernel: trigger ACPI S5 or platform shutdown
            }
            PowerAction::Restart => {
                crate::serial_println!("  [power_menu] EXECUTE: system restart requested");
                // In a real kernel: trigger ACPI reset or triple-fault reboot
            }
            PowerAction::Sleep => {
                crate::serial_println!("  [power_menu] EXECUTE: system sleep (S3) requested");
                // In a real kernel: trigger ACPI S3 suspend-to-RAM
            }
            PowerAction::Hibernate => {
                crate::serial_println!("  [power_menu] EXECUTE: system hibernate (S4) requested");
                // In a real kernel: save state to disk, ACPI S4
            }
            PowerAction::Cancel => {
                crate::serial_println!("  [power_menu] cancelled");
            }
        }
    }

    /// Confirm the current selection: execute and hide the menu.
    pub fn confirm(&mut self) {
        self.execute();
        self.visible = false;
    }

    /// Check if the power menu is currently visible.
    pub fn is_visible(&self) -> bool {
        self.visible
    }
}

static POWER_MENU: Mutex<Option<PowerMenu>> = Mutex::new(None);

pub fn init() {
    *POWER_MENU.lock() = Some(PowerMenu::new());
    crate::serial_println!("  [power_menu] Power menu initialized");
}

/// Show the global power menu.
pub fn show() {
    if let Some(ref mut menu) = *POWER_MENU.lock() {
        menu.show();
    }
}

/// Execute the selected action on the global power menu.
pub fn execute() {
    if let Some(ref menu) = *POWER_MENU.lock() {
        menu.execute();
    }
}
