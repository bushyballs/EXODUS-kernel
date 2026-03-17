use crate::sync::Mutex;
/// Fitness tracking for Genesis
///
/// Step counting, distance, calories, workouts,
/// exercise recognition, goals, achievements.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum WorkoutType {
    Walking,
    Running,
    Cycling,
    Swimming,
    Hiking,
    WeightTraining,
    Yoga,
    Cardio,
    Custom,
}

#[derive(Clone, Copy)]
pub struct DailyStats {
    pub date_days: u32, // days since epoch
    pub steps: u32,
    pub distance_m: u32,
    pub calories: u32,
    pub active_minutes: u32,
    pub floors_climbed: u16,
}

#[derive(Clone, Copy)]
struct Workout {
    workout_type: WorkoutType,
    start_time: u64,
    duration_secs: u32,
    calories: u32,
    distance_m: u32,
    avg_hr: u32,
    max_hr: u32,
}

struct FitnessGoal {
    daily_steps: u32,
    daily_calories: u32,
    daily_active_min: u32,
    weekly_workouts: u8,
}

struct FitnessEngine {
    daily_stats: Vec<DailyStats>,
    workouts: Vec<Workout>,
    current_day: DailyStats,
    goals: FitnessGoal,
    streak_days: u32,
    total_workouts: u32,
}

static FITNESS: Mutex<Option<FitnessEngine>> = Mutex::new(None);

impl FitnessEngine {
    fn new() -> Self {
        FitnessEngine {
            daily_stats: Vec::new(),
            workouts: Vec::new(),
            current_day: DailyStats {
                date_days: 0,
                steps: 0,
                distance_m: 0,
                calories: 0,
                active_minutes: 0,
                floors_climbed: 0,
            },
            goals: FitnessGoal {
                daily_steps: 10000,
                daily_calories: 500,
                daily_active_min: 30,
                weekly_workouts: 3,
            },
            streak_days: 0,
            total_workouts: 0,
        }
    }

    fn add_steps(&mut self, steps: u32) {
        self.current_day.steps += steps;
        // Estimate distance (avg stride ~0.75m)
        self.current_day.distance_m += steps * 75 / 100;
        // Estimate calories (avg ~0.04 cal/step)
        self.current_day.calories += steps * 4 / 100;
    }

    fn record_workout(&mut self, workout: Workout) {
        self.current_day.active_minutes += workout.duration_secs / 60;
        self.current_day.calories += workout.calories;
        self.current_day.distance_m += workout.distance_m;
        self.total_workouts = self.total_workouts.saturating_add(1);
        if self.workouts.len() < 1000 {
            self.workouts.push(workout);
        }
    }

    fn goal_progress(&self) -> (u32, u32, u32) {
        let step_pct = (self.current_day.steps * 100) / self.goals.daily_steps.max(1);
        let cal_pct = (self.current_day.calories * 100) / self.goals.daily_calories.max(1);
        let active_pct =
            (self.current_day.active_minutes * 100) / self.goals.daily_active_min.max(1);
        (step_pct.min(100), cal_pct.min(100), active_pct.min(100))
    }

    fn end_day(&mut self) {
        // Check if goals met
        let (s, c, a) = self.goal_progress();
        if s >= 100 && c >= 100 && a >= 100 {
            self.streak_days = self.streak_days.saturating_add(1);
        } else {
            self.streak_days = 0;
        }
        if self.daily_stats.len() < 365 {
            self.daily_stats.push(self.current_day);
        }
        self.current_day = DailyStats {
            date_days: self.current_day.date_days + 1,
            steps: 0,
            distance_m: 0,
            calories: 0,
            active_minutes: 0,
            floors_climbed: 0,
        };
    }
}

pub fn init() {
    let mut f = FITNESS.lock();
    *f = Some(FitnessEngine::new());
    serial_println!("    Health: fitness tracking (steps, workouts, goals) ready");
}
