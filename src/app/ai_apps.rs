/// AI-powered app intelligence for Genesis
///
/// App recommendation, smart permissions, usage insights,
/// resource prediction, intelligent prefetch, crash prediction.
///
/// Inspired by: Google Play Intelligence, Apple App Analytics. All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// App recommendation
pub struct AppRecommendation {
    pub app_name: String,
    pub reason: String,
    pub confidence: f32,
    pub category: AppCategory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppCategory {
    Productivity,
    Communication,
    Entertainment,
    Social,
    Utility,
    Health,
    Education,
    Finance,
    Travel,
    News,
    System,
}

/// App usage insight
pub struct UsageInsight {
    pub app_name: String,
    pub daily_minutes: u32,
    pub weekly_opens: u32,
    pub notification_count: u32,
    pub battery_impact: f32,
    pub data_usage_mb: u32,
    pub trend: UsageTrend,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageTrend {
    Increasing,
    Stable,
    Decreasing,
    New,
    Abandoned,
}

/// Permission recommendation from AI
pub struct PermissionAdvice {
    pub app_name: String,
    pub permission: String,
    pub recommendation: PermissionAction,
    pub reason: String,
    pub confidence: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionAction {
    Grant,
    Deny,
    AskEachTime,
    GrantWhileUsing,
    Revoke,
}

/// Crash prediction
pub struct CrashPrediction {
    pub app_name: String,
    pub probability: f32,
    pub likely_cause: String,
    pub recommendation: String,
}

/// AI app intelligence engine
pub struct AiAppEngine {
    pub enabled: bool,
    pub app_usage: BTreeMap<String, AppUsageData>,
    pub permission_history: Vec<(String, String, bool)>,
    pub crash_history: BTreeMap<String, Vec<u64>>,
    pub installed_apps: Vec<String>,
    pub total_recommendations: u64,
    pub learning_enabled: bool,
}

pub struct AppUsageData {
    pub daily_opens: Vec<u32>,
    pub daily_minutes: Vec<u32>,
    pub last_used: u64,
    pub total_crashes: u32,
    pub category: AppCategory,
    pub notifications_sent: u32,
}

impl AiAppEngine {
    const fn new() -> Self {
        AiAppEngine {
            enabled: true,
            app_usage: BTreeMap::new(),
            permission_history: Vec::new(),
            crash_history: BTreeMap::new(),
            installed_apps: Vec::new(),
            total_recommendations: 0,
            learning_enabled: true,
        }
    }

    /// Record app open event
    pub fn record_open(&mut self, app: &str, category: AppCategory) {
        let now = crate::time::clock::unix_time();
        let entry = self
            .app_usage
            .entry(String::from(app))
            .or_insert(AppUsageData {
                daily_opens: Vec::new(),
                daily_minutes: Vec::new(),
                last_used: 0,
                total_crashes: 0,
                category,
                notifications_sent: 0,
            });
        entry.last_used = now;
        if entry.daily_opens.is_empty() {
            entry.daily_opens.push(0);
        }
        if let Some(last) = entry.daily_opens.last_mut() {
            *last = last.saturating_add(1);
        }
    }

    /// Get usage insights for all apps
    pub fn get_insights(&self) -> Vec<UsageInsight> {
        self.app_usage
            .iter()
            .map(|(name, data)| {
                let daily_avg = if data.daily_minutes.is_empty() {
                    0
                } else {
                    data.daily_minutes.iter().sum::<u32>() / data.daily_minutes.len() as u32
                };
                let weekly_opens = if data.daily_opens.is_empty() {
                    0
                } else {
                    data.daily_opens.iter().rev().take(7).sum()
                };
                let trend = if data.daily_opens.len() < 3 {
                    UsageTrend::New
                } else {
                    let recent: u32 = data.daily_opens.iter().rev().take(3).sum();
                    let older: u32 = data.daily_opens.iter().rev().skip(3).take(3).sum();
                    if recent > older + 3 {
                        UsageTrend::Increasing
                    } else if older > recent + 3 {
                        UsageTrend::Decreasing
                    } else {
                        UsageTrend::Stable
                    }
                };
                UsageInsight {
                    app_name: name.clone(),
                    daily_minutes: daily_avg,
                    weekly_opens,
                    notification_count: data.notifications_sent,
                    battery_impact: daily_avg as f32 * 0.1,
                    data_usage_mb: daily_avg * 2,
                    trend,
                }
            })
            .collect()
    }

    /// AI permission recommendation
    pub fn advise_permission(&self, app: &str, permission: &str) -> PermissionAdvice {
        let sensitive = [
            "camera",
            "microphone",
            "location",
            "contacts",
            "sms",
            "phone",
        ];
        let is_sensitive = sensitive.iter().any(|s| permission.contains(s));

        let usage = self.app_usage.get(app);
        let used_recently = usage.map_or(false, |u| {
            crate::time::clock::unix_time() - u.last_used < 86400
        });

        let (action, reason) = if is_sensitive && !used_recently {
            (
                PermissionAction::Deny,
                "App hasn't been used recently and requests sensitive permission",
            )
        } else if is_sensitive {
            (
                PermissionAction::GrantWhileUsing,
                "Sensitive permission — grant only while app is in use",
            )
        } else {
            (
                PermissionAction::Grant,
                "Standard permission for app functionality",
            )
        };

        PermissionAdvice {
            app_name: String::from(app),
            permission: String::from(permission),
            recommendation: action,
            reason: String::from(reason),
            confidence: 0.8,
        }
    }

    /// Predict if an app is likely to crash
    pub fn predict_crash(&self, app: &str) -> CrashPrediction {
        let crash_count = self.crash_history.get(app).map_or(0, |c| c.len());
        let probability = match crash_count {
            0 => 0.01,
            1..=2 => 0.1,
            3..=5 => 0.3,
            6..=10 => 0.5,
            _ => 0.8,
        };
        CrashPrediction {
            app_name: String::from(app),
            probability: probability as f32,
            likely_cause: if crash_count > 5 {
                String::from("Recurring memory issues")
            } else {
                String::from("Unknown")
            },
            recommendation: if crash_count > 3 {
                String::from("Consider clearing cache or reinstalling")
            } else {
                String::from("No action needed")
            },
        }
    }

    /// Record app crash
    pub fn record_crash(&mut self, app: &str) {
        let now = crate::time::clock::unix_time();
        self.crash_history
            .entry(String::from(app))
            .or_insert_with(Vec::new)
            .push(now);
        if let Some(data) = self.app_usage.get_mut(app) {
            data.total_crashes = data.total_crashes.saturating_add(1);
        }
    }

    /// Get screen time summary
    pub fn screen_time_summary(&self) -> (u32, Vec<(String, u32)>) {
        let mut total = 0u32;
        let mut by_app: Vec<(String, u32)> = self
            .app_usage
            .iter()
            .map(|(name, data)| {
                let mins = data.daily_minutes.last().copied().unwrap_or(0);
                total += mins;
                (name.clone(), mins)
            })
            .collect();
        by_app.sort_by(|a, b| b.1.cmp(&a.1));
        (total, by_app)
    }
}

static AI_APPS: Mutex<AiAppEngine> = Mutex::new(AiAppEngine::new());

pub fn init() {
    crate::serial_println!(
        "    [ai-apps] AI app intelligence initialized (insights, permissions, crash predict)"
    );
}

pub fn record_open(app: &str, cat: AppCategory) {
    AI_APPS.lock().record_open(app, cat);
}
pub fn record_crash(app: &str) {
    AI_APPS.lock().record_crash(app);
}
pub fn advise_permission(app: &str, perm: &str) -> PermissionAdvice {
    AI_APPS.lock().advise_permission(app, perm)
}
pub fn get_insights() -> Vec<UsageInsight> {
    AI_APPS.lock().get_insights()
}
