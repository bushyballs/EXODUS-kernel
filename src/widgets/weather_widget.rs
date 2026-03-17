use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec;
/// Weather widget for Genesis
///
/// Current conditions, multi-day forecast, radar overlay,
/// severe weather alerts, saved locations, sunrise/sunset.
///
/// All temperatures in Q16 fixed-point (Celsius * 65536).
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

const Q16_ONE: i32 = 65536;

#[derive(Clone, Copy, PartialEq)]
pub enum WeatherCondition {
    Clear,
    PartlyCloudy,
    Cloudy,
    Rain,
    HeavyRain,
    Thunderstorm,
    Snow,
    Sleet,
    Fog,
    Haze,
    Windy,
    Hail,
    Tornado,
    Hurricane,
    Drizzle,
    Smoke,
}

#[derive(Clone, Copy, PartialEq)]
pub enum AlertSeverity {
    Advisory,
    Watch,
    Warning,
    Extreme,
}

#[derive(Clone, Copy, PartialEq)]
pub enum TemperatureUnit {
    Celsius,
    Fahrenheit,
}

#[derive(Clone, Copy, PartialEq)]
pub enum WindUnit {
    Kph,
    Mph,
    Knots,
    Ms,
}

#[derive(Clone, Copy, PartialEq)]
pub enum RadarLayer {
    Precipitation,
    CloudCover,
    Temperature,
    WindSpeed,
    Lightning,
}

#[derive(Clone, Copy)]
pub struct CurrentConditions {
    pub temperature_q16: i32,
    pub feels_like_q16: i32,
    pub humidity_pct: u8,
    pub wind_speed_q16: i32,
    pub wind_direction_deg: u16,
    pub pressure_hpa: u16,
    pub visibility_m: u32,
    pub uv_index: u8,
    pub dew_point_q16: i32,
    pub condition: WeatherCondition,
    pub cloud_cover_pct: u8,
    pub last_update_epoch: u64,
}

#[derive(Clone, Copy)]
pub struct ForecastDay {
    pub day_offset: u8,
    pub high_q16: i32,
    pub low_q16: i32,
    pub condition: WeatherCondition,
    pub precip_chance_pct: u8,
    pub wind_speed_q16: i32,
    pub wind_dir_deg: u16,
    pub humidity_pct: u8,
    pub sunrise_minutes: u16,
    pub sunset_minutes: u16,
    pub uv_index_max: u8,
}

#[derive(Clone, Copy)]
pub struct HourlyForecast {
    pub hour_offset: u8,
    pub temperature_q16: i32,
    pub condition: WeatherCondition,
    pub precip_chance_pct: u8,
    pub wind_speed_q16: i32,
}

#[derive(Clone, Copy)]
pub struct WeatherAlert {
    pub severity: AlertSeverity,
    pub alert_type_id: u16,
    pub start_epoch: u64,
    pub end_epoch: u64,
    pub acknowledged: bool,
    pub location_id: u8,
}

#[derive(Clone, Copy)]
pub struct SavedLocation {
    pub id: u8,
    pub latitude_q16: i32,
    pub longitude_q16: i32,
    pub is_current: bool,
    pub is_home: bool,
    pub timezone_offset_min: i16,
}

#[derive(Clone, Copy)]
pub struct RadarFrame {
    pub timestamp: u64,
    pub layer: RadarLayer,
    pub zoom_level: u8,
    pub center_lat_q16: i32,
    pub center_lon_q16: i32,
    pub data_size: u32,
}

#[derive(Clone, Copy)]
pub struct SunMoonInfo {
    pub sunrise_minutes: u16,
    pub sunset_minutes: u16,
    pub moon_phase_q16: i32,
    pub daylight_seconds: u32,
}

struct WeatherWidget {
    current: CurrentConditions,
    forecast: [ForecastDay; 10],
    forecast_count: u8,
    hourly: [HourlyForecast; 24],
    hourly_count: u8,
    alerts: Vec<WeatherAlert>,
    locations: Vec<SavedLocation>,
    active_location: u8,
    radar_frames: Vec<RadarFrame>,
    radar_playing: bool,
    radar_frame_idx: u8,
    temp_unit: TemperatureUnit,
    wind_unit: WindUnit,
    sun_moon: SunMoonInfo,
    refresh_interval_secs: u32,
    last_refresh: u64,
    widget_expanded: bool,
    show_hourly: bool,
    show_radar: bool,
    next_alert_id: u16,
}

static WEATHER_WIDGET: Mutex<Option<WeatherWidget>> = Mutex::new(None);

impl WeatherWidget {
    fn new() -> Self {
        let default_conditions = CurrentConditions {
            temperature_q16: 22 * Q16_ONE,
            feels_like_q16: 21 * Q16_ONE,
            humidity_pct: 45,
            wind_speed_q16: 10 * Q16_ONE,
            wind_direction_deg: 180,
            pressure_hpa: 1013,
            visibility_m: 10000,
            uv_index: 5,
            dew_point_q16: 12 * Q16_ONE,
            condition: WeatherCondition::Clear,
            cloud_cover_pct: 10,
            last_update_epoch: 0,
        };
        WeatherWidget {
            current: default_conditions,
            forecast: [ForecastDay {
                day_offset: 0,
                high_q16: 0,
                low_q16: 0,
                condition: WeatherCondition::Clear,
                precip_chance_pct: 0,
                wind_speed_q16: 0,
                wind_dir_deg: 0,
                humidity_pct: 0,
                sunrise_minutes: 360,
                sunset_minutes: 1080,
                uv_index_max: 0,
            }; 10],
            forecast_count: 0,
            hourly: [HourlyForecast {
                hour_offset: 0,
                temperature_q16: 0,
                condition: WeatherCondition::Clear,
                precip_chance_pct: 0,
                wind_speed_q16: 0,
            }; 24],
            hourly_count: 0,
            alerts: Vec::new(),
            locations: Vec::new(),
            active_location: 0,
            radar_frames: Vec::new(),
            radar_playing: false,
            radar_frame_idx: 0,
            temp_unit: TemperatureUnit::Celsius,
            wind_unit: WindUnit::Kph,
            sun_moon: SunMoonInfo {
                sunrise_minutes: 360,
                sunset_minutes: 1080,
                moon_phase_q16: 0,
                daylight_seconds: 43200,
            },
            refresh_interval_secs: 1800,
            last_refresh: 0,
            widget_expanded: false,
            show_hourly: false,
            show_radar: false,
            next_alert_id: 1,
        }
    }

    fn update_conditions(&mut self, cond: CurrentConditions) {
        self.current = cond;
    }

    fn set_forecast(&mut self, days: &[ForecastDay]) {
        let count = days.len().min(10);
        for i in 0..count {
            self.forecast[i] = days[i];
        }
        self.forecast_count = count as u8;
    }

    fn set_hourly(&mut self, hours: &[HourlyForecast]) {
        let count = hours.len().min(24);
        for i in 0..count {
            self.hourly[i] = hours[i];
        }
        self.hourly_count = count as u8;
    }

    fn add_alert(&mut self, severity: AlertSeverity, type_id: u16, start: u64, end: u64, loc: u8) {
        self.alerts.push(WeatherAlert {
            severity,
            alert_type_id: type_id,
            start_epoch: start,
            end_epoch: end,
            acknowledged: false,
            location_id: loc,
        });
    }

    fn dismiss_alert(&mut self, idx: usize) {
        if idx < self.alerts.len() {
            self.alerts[idx].acknowledged = true;
        }
    }

    fn active_alerts(&self) -> usize {
        self.alerts.iter().filter(|a| !a.acknowledged).count()
    }

    fn add_location(&mut self, lat_q16: i32, lon_q16: i32, tz_offset: i16) -> u8 {
        let id = self.locations.len() as u8;
        self.locations.push(SavedLocation {
            id,
            latitude_q16: lat_q16,
            longitude_q16: lon_q16,
            is_current: false,
            is_home: self.locations.is_empty(),
            timezone_offset_min: tz_offset,
        });
        id
    }

    fn remove_location(&mut self, id: u8) {
        self.locations.retain(|l| l.id != id);
    }

    fn set_active_location(&mut self, id: u8) {
        self.active_location = id;
    }

    fn convert_temp(&self, temp_q16: i32) -> i32 {
        match self.temp_unit {
            TemperatureUnit::Celsius => temp_q16,
            TemperatureUnit::Fahrenheit => {
                // F = C * 9/5 + 32 in Q16
                let nine_fifths = (9 * Q16_ONE) / 5;
                let product = ((temp_q16 as i64 * nine_fifths as i64) >> 16) as i32;
                product + 32 * Q16_ONE
            }
        }
    }

    fn set_temp_unit(&mut self, unit: TemperatureUnit) {
        self.temp_unit = unit;
    }

    fn set_wind_unit(&mut self, unit: WindUnit) {
        self.wind_unit = unit;
    }

    fn add_radar_frame(&mut self, frame: RadarFrame) {
        self.radar_frames.push(frame);
    }

    fn advance_radar(&mut self) {
        if !self.radar_frames.is_empty() {
            self.radar_frame_idx =
                ((self.radar_frame_idx as usize + 1) % self.radar_frames.len()) as u8;
        }
    }

    fn toggle_radar_playback(&mut self) {
        self.radar_playing = !self.radar_playing;
    }

    fn needs_refresh(&self, now: u64) -> bool {
        (now - self.last_refresh) >= self.refresh_interval_secs as u64
    }

    fn mark_refreshed(&mut self, now: u64) {
        self.last_refresh = now;
    }

    fn update_sun_moon(&mut self, info: SunMoonInfo) {
        self.sun_moon = info;
    }

    fn wind_direction_label(&self) -> &'static str {
        let deg = self.current.wind_direction_deg;
        match deg {
            0..=22 => "N",
            23..=67 => "NE",
            68..=112 => "E",
            113..=157 => "SE",
            158..=202 => "S",
            203..=247 => "SW",
            248..=292 => "W",
            293..=337 => "NW",
            _ => "N",
        }
    }

    fn is_daytime(&self, current_minute: u16) -> bool {
        current_minute >= self.sun_moon.sunrise_minutes
            && current_minute < self.sun_moon.sunset_minutes
    }

    fn has_severe_alerts(&self) -> bool {
        self.alerts.iter().any(|a| {
            !a.acknowledged
                && (a.severity == AlertSeverity::Warning || a.severity == AlertSeverity::Extreme)
        })
    }
}

pub fn init() {
    let mut w = WEATHER_WIDGET.lock();
    *w = Some(WeatherWidget::new());
    serial_println!("    Widgets: weather widget ready (conditions, forecast, radar, alerts)");
}
