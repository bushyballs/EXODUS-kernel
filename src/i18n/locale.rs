/// Locale management for Genesis
///
/// Language codes, region codes, text direction,
/// number/date/currency formatting.
///
/// Inspired by: CLDR, Android Locale. All code is original.
use crate::sync::Mutex;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Text direction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextDirection {
    LeftToRight,
    RightToLeft,
}

/// Number format style
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberStyle {
    Western,  // 1,234.56
    European, // 1.234,56
    Indian,   // 1,23,456
    Arabic,   // ١٬٢٣٤٫٥٦
}

/// Date format order
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateOrder {
    MonthDayYear,
    DayMonthYear,
    YearMonthDay,
}

/// A locale definition
pub struct Locale {
    pub language: String, // ISO 639-1 (en, es, ar, zh, ja)
    pub region: String,   // ISO 3166-1 (US, GB, JP, CN)
    pub direction: TextDirection,
    pub number_style: NumberStyle,
    pub date_order: DateOrder,
    pub currency_symbol: String,
    pub decimal_sep: char,
    pub thousands_sep: char,
    pub time_24h: bool,
}

impl Locale {
    pub fn english_us() -> Self {
        Locale {
            language: String::from("en"),
            region: String::from("US"),
            direction: TextDirection::LeftToRight,
            number_style: NumberStyle::Western,
            date_order: DateOrder::MonthDayYear,
            currency_symbol: String::from("$"),
            decimal_sep: '.',
            thousands_sep: ',',
            time_24h: false,
        }
    }

    pub fn tag(&self) -> String {
        format!("{}-{}", self.language, self.region)
    }

    pub fn format_number(&self, value: i64) -> String {
        let abs = if value < 0 { -value } else { value } as u64;
        let mut digits = alloc::format!("{}", abs);

        // Insert thousands separator
        let sep = self.thousands_sep;
        let group_size = match self.number_style {
            NumberStyle::Indian if digits.len() > 3 => {
                // Indian: first group of 3, then groups of 2
                let mut result = String::new();
                let bytes = digits.as_bytes();
                let len = bytes.len();
                // Last 3 digits
                let first_group = if len > 3 { len - 3 } else { 0 };
                for (i, &b) in bytes[..first_group].iter().enumerate() {
                    if i > 0 && (first_group - i) % 2 == 0 {
                        result.push(sep);
                    }
                    result.push(b as char);
                }
                if first_group > 0 {
                    result.push(sep);
                }
                for &b in &bytes[first_group..] {
                    result.push(b as char);
                }
                if value < 0 {
                    return format!("-{}", result);
                }
                return result;
            }
            _ => 3,
        };

        if digits.len() > group_size {
            let mut result = String::new();
            for (i, c) in digits.chars().rev().enumerate() {
                if i > 0 && i % group_size == 0 {
                    result.push(sep);
                }
                result.push(c);
            }
            digits = result.chars().rev().collect();
        }

        if value < 0 {
            format!("-{}", digits)
        } else {
            digits
        }
    }

    pub fn format_date(&self, year: u16, month: u8, day: u8) -> String {
        match self.date_order {
            DateOrder::MonthDayYear => format!("{:02}/{:02}/{}", month, day, year),
            DateOrder::DayMonthYear => format!("{:02}/{:02}/{}", day, month, year),
            DateOrder::YearMonthDay => format!("{}-{:02}-{:02}", year, month, day),
        }
    }

    pub fn is_rtl(&self) -> bool {
        self.direction == TextDirection::RightToLeft
    }
}

/// Locale manager
pub struct LocaleManager {
    pub current: Locale,
    pub available: Vec<String>, // language tags
    pub fallback: String,
}

impl LocaleManager {
    const fn new() -> Self {
        LocaleManager {
            current: Locale {
                language: String::new(),
                region: String::new(),
                direction: TextDirection::LeftToRight,
                number_style: NumberStyle::Western,
                date_order: DateOrder::MonthDayYear,
                currency_symbol: String::new(),
                decimal_sep: '.',
                thousands_sep: ',',
                time_24h: false,
            },
            available: Vec::new(),
            fallback: String::new(),
        }
    }

    pub fn set_locale(&mut self, locale: Locale) {
        crate::serial_println!("  [i18n] Locale set to {}", locale.tag());
        self.current = locale;
    }
}

static MANAGER: Mutex<LocaleManager> = Mutex::new(LocaleManager::new());

pub fn init() {
    let mut mgr = MANAGER.lock();
    mgr.set_locale(Locale::english_us());
    mgr.available.push(String::from("en-US"));
    mgr.available.push(String::from("es-ES"));
    mgr.available.push(String::from("fr-FR"));
    mgr.available.push(String::from("de-DE"));
    mgr.available.push(String::from("ja-JP"));
    mgr.available.push(String::from("zh-CN"));
    mgr.available.push(String::from("ar-SA"));
    mgr.available.push(String::from("ko-KR"));
    mgr.available.push(String::from("pt-BR"));
    mgr.available.push(String::from("hi-IN"));
    mgr.fallback = String::from("en-US");
    crate::serial_println!(
        "  [i18n] Locale manager initialized ({} locales)",
        mgr.available.len()
    );
}
