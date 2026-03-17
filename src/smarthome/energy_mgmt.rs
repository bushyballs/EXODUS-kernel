/// Home energy management system for Genesis
///
/// Solar panel tracking, battery storage management,
/// load balancing, smart grid integration, time-of-use
/// scheduling, and energy usage analytics.
///
/// All energy values in watt-hours (Wh) stored as u32/u64.
/// Power values in watts stored as i32/u32.
/// Q16 fixed-point for efficiency ratios.

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

// Q16 fixed-point helpers
const Q16_ONE: i32 = 1 << 16;
const Q16_HUNDRED: i32 = 100 << 16;

fn q16_from_int(v: i32) -> i32 { v << 16 }
fn q16_mul(a: i32, b: i32) -> i32 { ((a as i64 * b as i64) >> 16) as i32 }
fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 { return 0; }
    (((a as i64) << 16) / (b as i64)) as i32
}
fn q16_pct(part: i32, whole: i32) -> i32 { q16_mul(q16_div(part, whole), Q16_HUNDRED) }

// ---------- enums ----------

#[derive(Clone, Copy, PartialEq)]
pub enum EnergySource {
    Grid,
    Solar,
    Battery,
    Wind,
    Generator,
}

#[derive(Clone, Copy, PartialEq)]
pub enum GridTariff {
    OffPeak,
    MidPeak,
    OnPeak,
    SuperPeak,
    Critical,
}

#[derive(Clone, Copy, PartialEq)]
pub enum BatteryState {
    Idle,
    Charging,
    Discharging,
    Balancing,
    Fault,
    Full,
    Empty,
}

#[derive(Clone, Copy, PartialEq)]
pub enum LoadPriority {
    Critical,    // fridge, medical, security
    High,        // HVAC, water heater
    Medium,      // lighting, entertainment
    Low,         // EV charger, pool pump
    Deferrable,  // laundry, dishwasher
}

#[derive(Clone, Copy, PartialEq)]
pub enum SolarCondition {
    Clear,
    PartlyCloudy,
    Overcast,
    Night,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ScheduleAction {
    ChargeFromGrid,
    DischargeToHome,
    ExportToGrid,
    ReduceLoad,
    DeferLoad,
    BoostSolar,
}

// ---------- data structures ----------

struct SolarArray {
    id: u32,
    panel_count: u16,
    capacity_watts: u32,       // peak capacity
    current_watts: u32,        // current generation
    total_generated_wh: u64,
    efficiency_q16: i32,       // Q16 ratio 0..1
    orientation_deg: u16,      // 0=N, 90=E, 180=S, 270=W
    tilt_deg: u8,
    condition: SolarCondition,
    last_updated: u64,
}

struct BatteryBank {
    id: u32,
    capacity_wh: u32,
    current_wh: u32,
    charge_rate_w: u32,        // max charge watts
    discharge_rate_w: u32,     // max discharge watts
    state: BatteryState,
    cycle_count: u32,
    health_q16: i32,           // Q16 0..1 state of health
    min_soc_pct: u8,           // minimum state of charge %
    max_soc_pct: u8,           // maximum state of charge %
    temperature_c10: u16,      // temperature * 10
    total_charged_wh: u64,
    total_discharged_wh: u64,
}

struct LoadCircuit {
    id: u32,
    name: [u8; 24],
    name_len: usize,
    priority: LoadPriority,
    current_watts: u32,
    max_watts: u32,
    daily_wh: u32,
    enabled: bool,
    deferrable: bool,
    deferred_until: u64,
}

struct TariffSchedule {
    hour: u8,                   // 0-23
    tariff: GridTariff,
    rate_cents_per_kwh: u16,    // price in cents per kWh
}

struct EnergySchedule {
    id: u32,
    hour: u8,
    action: ScheduleAction,
    target_watts: u32,
    enabled: bool,
}

struct GridMeter {
    import_wh_total: u64,
    export_wh_total: u64,
    current_import_w: i32,     // positive = importing, negative = exporting
    voltage_v10: u16,          // voltage * 10
    frequency_mhz: u32,       // frequency * 1000 (e.g. 60000 = 60 Hz)
    grid_connected: bool,
    net_metering: bool,
}

struct DailySnapshot {
    day_index: u32,
    solar_wh: u32,
    grid_import_wh: u32,
    grid_export_wh: u32,
    battery_charged_wh: u32,
    battery_discharged_wh: u32,
    total_consumed_wh: u32,
    peak_watts: u32,
    cost_cents: u32,
}

struct EnergyManager {
    solar_arrays: Vec<SolarArray>,
    batteries: Vec<BatteryBank>,
    loads: Vec<LoadCircuit>,
    tariff_schedule: Vec<TariffSchedule>,
    energy_schedules: Vec<EnergySchedule>,
    grid: GridMeter,
    daily_history: Vec<DailySnapshot>,
    next_solar_id: u32,
    next_battery_id: u32,
    next_load_id: u32,
    next_schedule_id: u32,
    current_hour: u8,
    self_consumption_q16: i32,   // Q16 ratio: how much solar is used locally
    autarky_q16: i32,           // Q16 ratio: self-sufficiency
    total_solar_wh: u64,
    total_grid_import_wh: u64,
    total_grid_export_wh: u64,
    total_cost_cents: u64,
}

static ENERGY: Mutex<Option<EnergyManager>> = Mutex::new(None);

// ---------- implementation ----------

impl EnergyManager {
    fn new() -> Self {
        EnergyManager {
            solar_arrays: Vec::new(),
            batteries: Vec::new(),
            loads: Vec::new(),
            tariff_schedule: Vec::new(),
            energy_schedules: Vec::new(),
            grid: GridMeter {
                import_wh_total: 0,
                export_wh_total: 0,
                current_import_w: 0,
                voltage_v10: 1200,     // 120.0V
                frequency_mhz: 60000,  // 60.0 Hz
                grid_connected: true,
                net_metering: true,
            },
            daily_history: Vec::new(),
            next_solar_id: 1,
            next_battery_id: 1,
            next_load_id: 1,
            next_schedule_id: 1,
            current_hour: 0,
            self_consumption_q16: 0,
            autarky_q16: 0,
            total_solar_wh: 0,
            total_grid_import_wh: 0,
            total_grid_export_wh: 0,
            total_cost_cents: 0,
        }
    }

    // --- Solar ---

    fn add_solar_array(&mut self, panel_count: u16, capacity_watts: u32,
                       orientation: u16, tilt: u8) -> u32 {
        let id = self.next_solar_id;
        self.next_solar_id = self.next_solar_id.saturating_add(1);
        self.solar_arrays.push(SolarArray {
            id,
            panel_count,
            capacity_watts,
            current_watts: 0,
            total_generated_wh: 0,
            efficiency_q16: q16_from_int(1) * 85 / 100,  // 85% initial efficiency
            orientation_deg: orientation,
            tilt_deg: tilt,
            condition: SolarCondition::Clear,
            last_updated: 0,
        });
        id
    }

    fn update_solar(&mut self, array_id: u32, watts: u32, condition: SolarCondition, ts: u64) {
        if let Some(arr) = self.solar_arrays.iter_mut().find(|a| a.id == array_id) {
            let effective = q16_mul(watts as i32, arr.efficiency_q16) >> 16;
            arr.current_watts = effective.max(0) as u32;
            arr.condition = condition;
            arr.last_updated = ts;
        }
    }

    fn total_solar_generation(&self) -> u32 {
        self.solar_arrays.iter().map(|a| a.current_watts).sum()
    }

    // --- Battery ---

    fn add_battery(&mut self, capacity_wh: u32, charge_rate: u32, discharge_rate: u32) -> u32 {
        let id = self.next_battery_id;
        self.next_battery_id = self.next_battery_id.saturating_add(1);
        self.batteries.push(BatteryBank {
            id,
            capacity_wh,
            current_wh: capacity_wh / 2,  // start at 50%
            charge_rate_w: charge_rate,
            discharge_rate_w: discharge_rate,
            state: BatteryState::Idle,
            cycle_count: 0,
            health_q16: Q16_ONE,          // 100% health
            min_soc_pct: 10,
            max_soc_pct: 95,
            temperature_c10: 250,         // 25.0C
            total_charged_wh: 0,
            total_discharged_wh: 0,
        });
        id
    }

    fn charge_battery(&mut self, battery_id: u32, watts: u32, duration_sec: u32) -> u32 {
        if let Some(bat) = self.batteries.iter_mut().find(|b| b.id == battery_id) {
            let rate = watts.min(bat.charge_rate_w);
            let max_wh = (bat.capacity_wh as u64 * bat.max_soc_pct as u64 / 100) as u32;
            let wh_to_add = (rate as u64 * duration_sec as u64 / 3600) as u32;
            let actual = wh_to_add.min(max_wh.saturating_sub(bat.current_wh));
            bat.current_wh += actual;
            bat.total_charged_wh += actual as u64;
            bat.state = if bat.current_wh >= max_wh {
                BatteryState::Full
            } else {
                BatteryState::Charging
            };
            return actual;
        }
        0
    }

    fn discharge_battery(&mut self, battery_id: u32, watts: u32, duration_sec: u32) -> u32 {
        if let Some(bat) = self.batteries.iter_mut().find(|b| b.id == battery_id) {
            let rate = watts.min(bat.discharge_rate_w);
            let min_wh = (bat.capacity_wh as u64 * bat.min_soc_pct as u64 / 100) as u32;
            let wh_to_drain = (rate as u64 * duration_sec as u64 / 3600) as u32;
            let actual = wh_to_drain.min(bat.current_wh.saturating_sub(min_wh));
            bat.current_wh -= actual;
            bat.total_discharged_wh += actual as u64;
            if actual > 0 {
                bat.state = if bat.current_wh <= min_wh {
                    BatteryState::Empty
                } else {
                    BatteryState::Discharging
                };
            }
            return actual;
        }
        0
    }

    fn battery_soc_pct(&self, battery_id: u32) -> Option<u8> {
        self.batteries.iter().find(|b| b.id == battery_id)
            .map(|b| ((b.current_wh as u64 * 100) / b.capacity_wh as u64) as u8)
    }

    // --- Load management ---

    fn add_load(&mut self, name: &[u8], priority: LoadPriority, max_watts: u32,
                deferrable: bool) -> u32 {
        let id = self.next_load_id;
        self.next_load_id = self.next_load_id.saturating_add(1);
        let mut n = [0u8; 24];
        let nlen = name.len().min(24);
        n[..nlen].copy_from_slice(&name[..nlen]);
        self.loads.push(LoadCircuit {
            id,
            name: n, name_len: nlen,
            priority,
            current_watts: 0,
            max_watts,
            daily_wh: 0,
            enabled: true,
            deferrable,
            deferred_until: 0,
        });
        id
    }

    fn update_load(&mut self, load_id: u32, watts: u32) {
        if let Some(load) = self.loads.iter_mut().find(|l| l.id == load_id) {
            load.current_watts = watts.min(load.max_watts);
        }
    }

    fn defer_load(&mut self, load_id: u32, until: u64) -> bool {
        if let Some(load) = self.loads.iter_mut().find(|l| l.id == load_id) {
            if load.deferrable {
                load.deferred_until = until;
                load.enabled = false;
                return true;
            }
        }
        false
    }

    fn total_load(&self) -> u32 {
        self.loads.iter().filter(|l| l.enabled).map(|l| l.current_watts).sum()
    }

    fn loads_by_priority(&self, prio: LoadPriority) -> Vec<u32> {
        self.loads.iter().filter(|l| l.priority == prio).map(|l| l.id).collect()
    }

    // --- Smart grid / tariff ---

    fn set_tariff(&mut self, hour: u8, tariff: GridTariff, rate: u16) {
        if let Some(t) = self.tariff_schedule.iter_mut().find(|t| t.hour == hour) {
            t.tariff = tariff;
            t.rate_cents_per_kwh = rate;
        } else {
            self.tariff_schedule.push(TariffSchedule {
                hour, tariff, rate_cents_per_kwh: rate,
            });
        }
    }

    fn current_tariff(&self) -> (GridTariff, u16) {
        self.tariff_schedule.iter()
            .find(|t| t.hour == self.current_hour)
            .map(|t| (t.tariff, t.rate_cents_per_kwh))
            .unwrap_or((GridTariff::MidPeak, 12))
    }

    fn cheapest_hours(&self, count: usize) -> Vec<u8> {
        let mut sorted: Vec<_> = self.tariff_schedule.iter().collect();
        sorted.sort_by_key(|t| t.rate_cents_per_kwh);
        sorted.iter().take(count).map(|t| t.hour).collect()
    }

    // --- Energy scheduling ---

    fn add_schedule(&mut self, hour: u8, action: ScheduleAction, target_watts: u32) -> u32 {
        let id = self.next_schedule_id;
        self.next_schedule_id = self.next_schedule_id.saturating_add(1);
        self.energy_schedules.push(EnergySchedule {
            id, hour, action, target_watts, enabled: true,
        });
        id
    }

    fn schedules_for_hour(&self, hour: u8) -> Vec<(u32, ScheduleAction, u32)> {
        self.energy_schedules.iter()
            .filter(|s| s.enabled && s.hour == hour)
            .map(|s| (s.id, s.action, s.target_watts))
            .collect()
    }

    // --- Load balancing ---

    fn balance_loads(&mut self, available_watts: u32) {
        let total = self.total_load();
        if total <= available_watts { return; }
        // Shed deferrable loads first, then low priority, then medium
        let priorities = [LoadPriority::Deferrable, LoadPriority::Low, LoadPriority::Medium];
        let mut remaining_excess = total - available_watts;
        for prio in &priorities {
            if remaining_excess == 0 { break; }
            for load in self.loads.iter_mut() {
                if remaining_excess == 0 { break; }
                if load.priority == *prio && load.enabled {
                    let shed = load.current_watts.min(remaining_excess);
                    remaining_excess -= shed;
                    load.current_watts -= shed;
                    if load.current_watts == 0 {
                        load.enabled = false;
                    }
                }
            }
        }
    }

    // --- Grid meter ---

    fn update_grid_meter(&mut self, import_w: i32) {
        self.grid.current_import_w = import_w;
        if import_w > 0 {
            self.grid.import_wh_total += import_w as u64;
        } else {
            self.grid.export_wh_total += (-import_w) as u64;
        }
    }

    fn is_exporting(&self) -> bool { self.grid.current_import_w < 0 }

    // --- Daily snapshot ---

    fn take_daily_snapshot(&mut self, day_index: u32) {
        let solar: u64 = self.solar_arrays.iter().map(|a| a.total_generated_wh).sum();
        let consumed: u32 = self.loads.iter().map(|l| l.daily_wh).sum();
        let (tariff, rate) = self.current_tariff();
        self.daily_history.push(DailySnapshot {
            day_index,
            solar_wh: solar as u32,
            grid_import_wh: self.grid.import_wh_total as u32,
            grid_export_wh: self.grid.export_wh_total as u32,
            battery_charged_wh: self.batteries.iter().map(|b| b.total_charged_wh as u32).sum(),
            battery_discharged_wh: self.batteries.iter().map(|b| b.total_discharged_wh as u32).sum(),
            total_consumed_wh: consumed,
            peak_watts: self.loads.iter().map(|l| l.max_watts).max().unwrap_or(0),
            cost_cents: ((self.grid.import_wh_total / 1000) as u32) * rate as u32 / 100,
        });
        // Reset daily counters on loads
        for load in &mut self.loads {
            load.daily_wh = 0;
        }
    }

    // --- Autarky / self-consumption ---

    fn update_autarky(&mut self) {
        let solar = self.total_solar_generation() as i32;
        let load = self.total_load() as i32;
        if load > 0 {
            let local = solar.min(load);
            self.self_consumption_q16 = if solar > 0 {
                q16_div(local, solar)
            } else { 0 };
            self.autarky_q16 = q16_div(local, load);
        }
    }

    fn stats(&self) -> (u64, u64, u64, u64) {
        (self.total_solar_wh, self.total_grid_import_wh,
         self.total_grid_export_wh, self.total_cost_cents)
    }
}

pub fn init() {
    let mut em = ENERGY.lock();
    *em = Some(EnergyManager::new());
    serial_println!("    Energy: solar, battery, load balancing, smart grid ready");
}
