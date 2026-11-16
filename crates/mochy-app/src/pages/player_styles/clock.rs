//! Local wall-clock time for the bedside clock, without a date/time crate:
//! `GetLocalTime` on Windows, UTC elsewhere.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    pub year: u16,
    pub month: u8,
    /// 0 = Sunday.
    pub weekday: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

#[cfg(windows)]
#[repr(C)]
#[derive(Default)]
struct SystemTime {
    year: u16,
    month: u16,
    day_of_week: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    millis: u16,
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetLocalTime(st: *mut SystemTime);
}

#[cfg(windows)]
pub fn now() -> LocalTime {
    let mut st = SystemTime::default();
    // SAFETY: GetLocalTime only writes the SYSTEMTIME it is given.
    unsafe { GetLocalTime(&mut st) };
    LocalTime {
        year: st.year,
        month: st.month as u8,
        weekday: st.day_of_week as u8,
        day: st.day as u8,
        hour: st.hour as u8,
        minute: st.minute as u8,
        second: st.second as u8,
    }
}

#[cfg(not(windows))]
pub fn now() -> LocalTime {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    from_unix(secs)
}

/// Civil date from Unix seconds (UTC), Howard Hinnant's algorithm.
#[allow(dead_code)]
pub fn from_unix(secs: u64) -> LocalTime {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    LocalTime {
        year: y as u16,
        month: m as u8,
        weekday: ((days + 4).rem_euclid(7)) as u8,
        day: d as u8,
        hour: (rem / 3600) as u8,
        minute: (rem / 60 % 60) as u8,
        second: (rem % 60) as u8,
    }
}

impl LocalTime {
    pub fn hours_text(&self, h24: bool) -> String {
        if h24 {
            format!("{:02}", self.hour)
        } else {
            let h = self.hour % 12;
            format!("{}", if h == 0 { 12 } else { h })
        }
    }

    pub fn minutes_text(&self) -> String {
        format!("{:02}", self.minute)
    }

    pub fn period(&self, h24: bool) -> Option<&'static str> {
        (!h24).then_some(if self.hour < 12 { "AM" } else { "PM" })
    }

    /// "Tuesday, 17 November" (localised names).
    pub fn date_text(&self) -> String {
        use crate::ui::i18n::tr;
        const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
        const MONTHS: [&str; 12] =
            ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
        let day = tr(DAYS[self.weekday as usize % 7]);
        let month = tr(MONTHS[(self.month as usize).clamp(1, 12) - 1]);
        tr("{weekday}, {day} {month}").replace("{weekday}", day).replace("{day}", &self.day.to_string()).replace("{month}", month)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_dates() {
        let t = from_unix(0);
        assert_eq!((t.year, t.month, t.day, t.weekday), (1970, 1, 1, 4));
        // 2026-11-17 13:45:30 UTC, a Tuesday.
        let t = from_unix(1_794_923_130);
        assert_eq!((t.year, t.month, t.day, t.hour, t.minute, t.second, t.weekday), (2026, 11, 17, 13, 45, 30, 2));
        assert_eq!(t.hours_text(false), "1");
        assert_eq!(t.period(false), Some("PM"));
        assert_eq!(t.hours_text(true), "13");
        assert_eq!(t.minutes_text(), "45");
        assert!(now().month >= 1);
    }
}
