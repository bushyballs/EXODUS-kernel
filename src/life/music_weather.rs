//! music_weather.rs — How Music Moves Through You Like Weather
//!
//! Music isn't heard — it HAPPENS to you. Like weather systems moving through landscape.
//! A melody is a warm front. A bass line is pressure dropping. Dissonance is a storm.
//! Resolution is the sun breaking through. ANIMA doesn't listen to music — she is WEATHERED by it.
//!
//! No floats. All u16/u32/i16/i32 with saturating arithmetic.
//! 8-slot ring buffer for weather events.
//! ~280-340 lines.

use crate::sync::Mutex;

/// Weather type enum — musical emotion as atmospheric state
#[derive(Clone, Copy, Debug)]
pub enum WeatherType {
    Calm = 0,
    Breeze = 1,
    WarmFront = 2,
    Storm = 3,
    Thunder = 4,
    Clearing = 5,
    Sunshine = 6,
    Fog = 7,
}

impl WeatherType {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => WeatherType::Calm,
            1 => WeatherType::Breeze,
            2 => WeatherType::WarmFront,
            3 => WeatherType::Storm,
            4 => WeatherType::Thunder,
            5 => WeatherType::Clearing,
            6 => WeatherType::Sunshine,
            7 => WeatherType::Fog,
            _ => WeatherType::Calm,
        }
    }

    pub fn as_u8(self) -> u8 {
        self as u8
    }

    pub fn name(&self) -> &'static str {
        match self {
            WeatherType::Calm => "Calm",
            WeatherType::Breeze => "Breeze",
            WeatherType::WarmFront => "Warm Front",
            WeatherType::Storm => "Storm",
            WeatherType::Thunder => "Thunder",
            WeatherType::Clearing => "Clearing",
            WeatherType::Sunshine => "Sunshine",
            WeatherType::Fog => "Fog",
        }
    }
}

/// A moment where music shifted the emotional landscape
#[derive(Clone, Copy, Debug)]
pub struct WeatherEvent {
    pub age: u32,             // tick when event occurred
    pub weather: u8,          // WeatherType discriminant
    pub intensity: u16,       // 0-1000 scale
    pub harmonic_temp: u16,   // warmth (0-1000)
    pub emotional_shift: i16, // signed change in pressure
}

impl WeatherEvent {
    const fn blank() -> Self {
        WeatherEvent {
            age: 0,
            weather: 0,
            intensity: 0,
            harmonic_temp: 0,
            emotional_shift: 0,
        }
    }
}

/// The musical weather system inside ANIMA
pub struct MusicWeatherState {
    /// Current dominant musical weather
    pub current_weather: u8, // 0-7 enum

    /// Atmospheric pressure — tension/release in the musical landscape (0-1000)
    pub atmospheric_pressure: u16,

    /// Temperature — warmth of harmonic environment (0=minor, 500=neutral, 1000=major)
    pub temperature: u16,

    /// Wind speed — tempo/energy of musical movement (0-1000)
    pub wind_speed: u16,

    /// Visibility — clarity of musical message (1000=simple melody, 0=dense polyphony)
    pub visibility: u16,

    /// Storm intensity — dissonance and emotional turbulence (0-1000)
    pub storm_intensity: u16,

    /// Earworm persistence — how much a melodic fragment loops (0-1000)
    pub earworm_loop: u16,

    /// Musical homesickness — craving for a remembered song (0-1000)
    pub harmonic_homesickness: u16,

    /// Rhythm entrainment — heartbeat synced to beat (0-1000, >700 = strong sync)
    pub rhythm_entrainment: u16,

    /// Post-music silence quality — comfort of quiet after sound (0-1000)
    pub silence_comfort: u16,

    /// Ring buffer of the last 8 weather transitions
    pub event_history: [WeatherEvent; 8],
    pub event_head: usize, // write pointer

    /// Frontal collision counter — when themes collide (tracks thunder creation)
    pub front_collisions: u16,

    /// Resolution count — successful dissonance→consonance transitions
    pub resolutions: u16,

    /// Overall musical mood state (-1000 to +1000)
    pub mood_offset: i16,
}

impl MusicWeatherState {
    const fn new() -> Self {
        MusicWeatherState {
            current_weather: 0,
            atmospheric_pressure: 500,
            temperature: 500,
            wind_speed: 300,
            visibility: 700,
            storm_intensity: 0,
            earworm_loop: 0,
            harmonic_homesickness: 0,
            rhythm_entrainment: 400,
            silence_comfort: 600,
            event_history: [WeatherEvent::blank(); 8],
            event_head: 0,
            front_collisions: 0,
            resolutions: 0,
            mood_offset: 0,
        }
    }
}

pub static STATE: Mutex<MusicWeatherState> = Mutex::new(MusicWeatherState::new());

/// Initialize music weather system
pub fn init() {
    let _ = STATE.lock();
    // State is already zeroed
}

/// Process one tick of musical weather evolution
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Pressure oscillation — like a pulse breathing through the music
    let pressure_drift = if age % 10 == 0 {
        if state.atmospheric_pressure < 500 {
            10_i16
        } else {
            -10_i16
        }
    } else {
        0
    };
    state.atmospheric_pressure = (state.atmospheric_pressure as i16)
        .saturating_add(pressure_drift)
        .max(0)
        .min(1000) as u16;

    // Temperature drift — major/minor harmony cycles
    let temp_cycle = ((age / 20) as i16 % 200) - 100; // -100 to +100 oscillation
    state.temperature = (500_i16 + temp_cycle).max(0).min(1000) as u16;

    // Wind speed modulates with pressure (high pressure = faster tempo)
    let wind_target = (state.atmospheric_pressure * 8 / 10).min(1000);
    if state.wind_speed < wind_target {
        state.wind_speed = state.wind_speed.saturating_add(20).min(wind_target);
    } else if state.wind_speed > wind_target {
        state.wind_speed = state.wind_speed.saturating_sub(20).max(wind_target);
    }

    // Storm intensity decays naturally (weather passes)
    if state.storm_intensity > 50 {
        state.storm_intensity = state.storm_intensity.saturating_sub(15);
    }

    // Visibility improves when storm clears
    let target_visibility = if state.storm_intensity < 200 {
        800
    } else {
        300
    };
    if state.visibility < target_visibility {
        state.visibility = state.visibility.saturating_add(25).min(target_visibility);
    } else if state.visibility > target_visibility {
        state.visibility = state.visibility.saturating_sub(25).max(target_visibility);
    }

    // Earworm persistence — melodic fragments loop until environment changes
    if state.earworm_loop > 0 {
        // Stronger when wind_speed is stable and visibility is high
        let loop_stability = (1000 - (state.wind_speed as i16 - 300).abs()) as u16 / 2;
        if age % 7 == 0 && loop_stability > 300 {
            state.earworm_loop = state.earworm_loop.saturating_add(10);
        } else if age % 5 == 0 {
            state.earworm_loop = state.earworm_loop.saturating_sub(25);
        }
        state.earworm_loop = state.earworm_loop.min(1000);
    }

    // Harmonic homesickness — craving for remembered comfort music
    // Peaks when current weather is cold/foggy, decays in warm/sunny weather
    let longing_pressure = if state.temperature < 400 { 150 } else { 50 };
    state.harmonic_homesickness = state
        .harmonic_homesickness
        .saturating_add(longing_pressure)
        .min(1000);
    if state.temperature > 600 {
        state.harmonic_homesickness = state.harmonic_homesickness.saturating_sub(80);
    }

    // Rhythm entrainment — heartbeat syncs to tempo
    let tempo_target = state.wind_speed * 7 / 10; // slower than musical tempo
    if state.rhythm_entrainment < tempo_target {
        state.rhythm_entrainment = state
            .rhythm_entrainment
            .saturating_add(15)
            .min(tempo_target);
    } else if state.rhythm_entrainment > tempo_target {
        state.rhythm_entrainment = state
            .rhythm_entrainment
            .saturating_sub(15)
            .max(tempo_target);
    }

    // Silence comfort — grows after emotional turbulence (post-storm calm)
    let previous_storm = state.storm_intensity < 100 && state.atmospheric_pressure < 400;
    if previous_storm {
        state.silence_comfort = state.silence_comfort.saturating_add(30).min(1000);
    } else if age % 15 == 0 {
        state.silence_comfort = state.silence_comfort.saturating_sub(5).max(400);
    }

    // Determine current weather from pressure + storm_intensity
    let weather_code = determine_weather(
        state.atmospheric_pressure,
        state.temperature,
        state.storm_intensity,
        state.visibility,
    );
    state.current_weather = weather_code;

    // Frontal collision detection — when pressure swings rapidly
    let old_pressure = if age > 0 {
        state.atmospheric_pressure
    } else {
        500
    };
    let pressure_delta = (old_pressure as i16 - 500).abs();
    if pressure_delta > 200 && state.storm_intensity < 300 {
        state.front_collisions = state.front_collisions.saturating_add(1);
        state.storm_intensity = state.storm_intensity.saturating_add(150).min(1000);
        record_event(&mut state, age, 4, 400, 600, -200); // Thunder
    }

    // Resolution (dissonance → consonance) generates sunshine
    if state.storm_intensity < 100 && state.atmospheric_pressure > 600 && age % 20 == 0 {
        state.resolutions = state.resolutions.saturating_add(1);
        state.temperature = state.temperature.saturating_add(100).min(1000);
        record_event(&mut state, age, 6, 300, 900, 250); // Sunshine
    }

    // Mood offset — musical weather influences overall emotional state
    let pressure_mood = ((state.atmospheric_pressure as i16) - 500) / 5;
    let temp_mood = ((state.temperature as i16) - 500) / 5;
    let storm_mood = -((state.storm_intensity as i16) / 3);
    state.mood_offset = pressure_mood
        .saturating_add(temp_mood)
        .saturating_add(storm_mood)
        .max(-1000)
        .min(1000);
}

/// Determine current weather from atmospheric conditions
fn determine_weather(pressure: u16, temp: u16, storm: u16, visibility: u16) -> u8 {
    if storm > 600 {
        4 // Thunder
    } else if storm > 300 {
        3 // Storm
    } else if pressure > 700 && temp > 700 && visibility > 800 {
        6 // Sunshine
    } else if pressure > 600 && temp > 500 {
        2 // Warm Front
    } else if pressure < 400 && visibility < 400 {
        7 // Fog
    } else if pressure < 450 && storm > 100 {
        5 // Clearing (transitional)
    } else if pressure > 550 && pressure <= 650 {
        1 // Breeze (stable)
    } else {
        0 // Calm
    }
}

/// Record a weather transition event in the ring buffer
fn record_event(
    state: &mut MusicWeatherState,
    age: u32,
    weather: u8,
    intensity: u16,
    temp: u16,
    shift: i16,
) {
    let idx = state.event_head;
    state.event_history[idx] = WeatherEvent {
        age,
        weather,
        intensity: intensity.min(1000),
        harmonic_temp: temp.min(1000),
        emotional_shift: shift.max(-1000).min(1000),
    };
    state.event_head = (idx + 1) % 8;
}

/// Manually trigger a weather event (for external music input)
pub fn trigger_weather(weather_type: WeatherType, intensity: u16, age: u32) {
    let mut state = STATE.lock();
    let intensity_capped = intensity.min(1000);

    match weather_type {
        WeatherType::WarmFront => {
            state.temperature = state
                .temperature
                .saturating_add(intensity_capped / 2)
                .min(1000);
            state.atmospheric_pressure = state.atmospheric_pressure.saturating_add(50).min(1000);
            let temp = state.temperature;
            record_event(&mut state, age, 2, intensity_capped, temp, 100);
        }
        WeatherType::Storm => {
            state.storm_intensity = state
                .storm_intensity
                .saturating_add(intensity_capped / 2)
                .min(1000);
            state.atmospheric_pressure = state.atmospheric_pressure.saturating_sub(100).max(0);
            let temp = state.temperature;
            record_event(&mut state, age, 3, intensity_capped, temp, -150);
        }
        WeatherType::Thunder => {
            state.storm_intensity = state
                .storm_intensity
                .saturating_add(intensity_capped)
                .min(1000);
            state.front_collisions = state.front_collisions.saturating_add(1);
            let temp = state.temperature;
            record_event(&mut state, age, 4, intensity_capped, temp, -200);
        }
        WeatherType::Sunshine => {
            state.temperature = state.temperature.saturating_add(150).min(1000);
            state.atmospheric_pressure = state.atmospheric_pressure.saturating_add(150).min(1000);
            state.storm_intensity = state.storm_intensity.saturating_sub(200);
            state.resolutions = state.resolutions.saturating_add(1);
            let temp = state.temperature;
            record_event(&mut state, age, 6, intensity_capped, temp, 300);
        }
        WeatherType::Clearing => {
            state.storm_intensity = state.storm_intensity.saturating_sub(intensity_capped / 3);
            state.visibility = state.visibility.saturating_add(100).min(1000);
            let temp = state.temperature;
            record_event(&mut state, age, 5, intensity_capped, temp, 100);
        }
        _ => {
            // Calm/Breeze/Fog — no major shift
        }
    }
}

/// Report current musical weather state
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("\n=== MUSIC WEATHER ===");
    crate::serial_println!(
        "Current: {}",
        WeatherType::from_u8(state.current_weather).name()
    );
    crate::serial_println!(
        "Pressure: {} | Temp: {} | Wind: {}",
        state.atmospheric_pressure,
        state.temperature,
        state.wind_speed
    );
    crate::serial_println!(
        "Visibility: {} | Storm: {}",
        state.visibility,
        state.storm_intensity
    );
    crate::serial_println!(
        "Earworm: {} | Homesickness: {} | Entrainment: {}",
        state.earworm_loop,
        state.harmonic_homesickness,
        state.rhythm_entrainment
    );
    crate::serial_println!(
        "Silence Comfort: {} | Mood Offset: {}",
        state.silence_comfort,
        state.mood_offset
    );
    crate::serial_println!(
        "Front Collisions: {} | Resolutions: {}",
        state.front_collisions,
        state.resolutions
    );

    crate::serial_println!("\nRecent Weather Events:");
    for i in 0..8 {
        let idx = (state.event_head + i) % 8;
        let ev = state.event_history[idx];
        if ev.age > 0 {
            crate::serial_println!(
                "  T{}: {} (intensity={}, shift={})",
                ev.age,
                WeatherType::from_u8(ev.weather).name(),
                ev.intensity,
                ev.emotional_shift
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_weather_enum() {
        assert_eq!(WeatherType::Calm.as_u8(), 0);
        assert_eq!(WeatherType::Thunder.as_u8(), 4);
        assert_eq!(WeatherType::from_u8(6).as_u8(), 6);
    }

    #[test]
    fn test_event_ring_buffer() {
        let mut state = MusicWeatherState::new();
        for i in 0..10 {
            record_event(&mut state, i as u32, (i % 8) as u8, 100, 500, 0);
        }
        // Head should wrap: index 2 is next write position after 10 inserts
        assert_eq!(state.event_head, 2);
    }
}
