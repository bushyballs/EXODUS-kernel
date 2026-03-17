/// clk — clock framework for hardware clock management
///
/// Manages PLLs, dividers, clock gates, and muxes.
///
/// Rules: no_std, no heap, no floats, no panics, saturating counters.
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

const MAX_CLKS: usize = 64;

#[derive(Copy, Clone, PartialEq)]
pub enum ClkType {
    Fixed,
    Pll,
    Divider,
    Gate,
    Mux,
}

#[derive(Copy, Clone)]
pub struct Clk {
    pub id: u32,
    pub name: [u8; 32],
    pub name_len: u8,
    pub clk_type: ClkType,
    pub rate_hz: u64,
    pub parent_id: u32,
    pub divisor: u32,
    pub prepared: bool,
    pub enabled: bool,
    pub active: bool,
}

impl Clk {
    pub const fn empty() -> Self {
        Clk {
            id: 0,
            name: [0u8; 32],
            name_len: 0,
            clk_type: ClkType::Fixed,
            rate_hz: 0,
            parent_id: 0,
            divisor: 1,
            prepared: false,
            enabled: false,
            active: false,
        }
    }
}

const EMPTY_CLK: Clk = Clk::empty();
static CLK_TABLE: Mutex<[Clk; MAX_CLKS]> = Mutex::new([EMPTY_CLK; MAX_CLKS]);
static CLK_NEXT_ID: AtomicU32 = AtomicU32::new(1);

fn copy_name(dst: &mut [u8; 32], src: &[u8]) -> u8 {
    let len = src.len().min(31);
    let mut i = 0usize;
    while i < len {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
    len as u8
}

pub fn clk_register(name: &[u8], clk_type: ClkType, rate_hz: u64, parent_id: u32) -> Option<u32> {
    let id = CLK_NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let mut clks = CLK_TABLE.lock();
    let mut i = 0usize;
    while i < MAX_CLKS {
        if !clks[i].active {
            clks[i] = Clk::empty();
            clks[i].id = id;
            clks[i].name_len = copy_name(&mut clks[i].name, name);
            clks[i].clk_type = clk_type;
            clks[i].rate_hz = rate_hz;
            clks[i].parent_id = parent_id;
            clks[i].divisor = 1;
            clks[i].active = true;
            return Some(id);
        }
        i = i.saturating_add(1);
    }
    None
}

pub fn clk_enable(id: u32) -> bool {
    let mut clks = CLK_TABLE.lock();
    let mut i = 0usize;
    while i < MAX_CLKS {
        if clks[i].active && clks[i].id == id {
            clks[i].enabled = true;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn clk_disable(id: u32) -> bool {
    let mut clks = CLK_TABLE.lock();
    let mut i = 0usize;
    while i < MAX_CLKS {
        if clks[i].active && clks[i].id == id {
            clks[i].enabled = false;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn clk_get_rate(id: u32) -> Option<u64> {
    let clks = CLK_TABLE.lock();
    let mut i = 0usize;
    while i < MAX_CLKS {
        if clks[i].active && clks[i].id == id {
            return Some(clks[i].rate_hz);
        }
        i = i.saturating_add(1);
    }
    None
}

pub fn clk_set_rate(id: u32, rate_hz: u64) -> bool {
    let mut clks = CLK_TABLE.lock();
    let mut i = 0usize;
    while i < MAX_CLKS {
        if clks[i].active && clks[i].id == id {
            if clks[i].clk_type == ClkType::Fixed {
                return false;
            }
            clks[i].rate_hz = rate_hz;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn clk_set_parent(id: u32, parent_id: u32) -> bool {
    let mut clks = CLK_TABLE.lock();
    let mut i = 0usize;
    while i < MAX_CLKS {
        if clks[i].active && clks[i].id == id {
            clks[i].parent_id = parent_id;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn clk_set_divisor(id: u32, divisor: u32) -> bool {
    if divisor == 0 {
        return false;
    }
    let mut clks = CLK_TABLE.lock();
    let mut i = 0usize;
    while i < MAX_CLKS {
        if clks[i].active && clks[i].id == id {
            clks[i].divisor = divisor;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn clk_prepare_enable(id: u32) -> bool {
    let mut clks = CLK_TABLE.lock();
    let mut i = 0usize;
    while i < MAX_CLKS {
        if clks[i].active && clks[i].id == id {
            clks[i].prepared = true;
            clks[i].enabled = true;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn init() {
    // Register standard platform clocks
    let clocks: &[(&[u8], ClkType, u64)] = &[
        (b"osc24m", ClkType::Fixed, 24_000_000),
        (b"pll-cpu", ClkType::Pll, 1_200_000_000),
        (b"pll-ddr", ClkType::Pll, 533_000_000),
        (b"ahb", ClkType::Divider, 200_000_000),
        (b"apb", ClkType::Divider, 100_000_000),
        (b"uart-clk", ClkType::Gate, 24_000_000),
        (b"usb-clk", ClkType::Gate, 48_000_000),
    ];
    let mut k = 0usize;
    while k < clocks.len() {
        if let Some(id) = clk_register(clocks[k].0, clocks[k].1, clocks[k].2, 0) {
            clk_prepare_enable(id);
        }
        k = k.saturating_add(1);
    }
    serial_println!("[clk] clock framework initialized with 7 clocks");
}
