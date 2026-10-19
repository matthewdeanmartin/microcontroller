//! Local time for the screen. The board knows UTC (SNTP) and the household
//! zone as a POSIX TZ string; the caller supplies the UTC offset for an
//! instant (newlib's `localtime_r` on the board, a fixed offset on desktop),
//! and everything else is calendar arithmetic here.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Local {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    /// 0 = Sunday.
    pub weekday: u32,
    pub hour: u32,
    pub minute: u32,
}

const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

impl Local {
    /// `utc` seconds shifted by `offset` seconds.
    pub fn at(utc: u64, offset: i64) -> Local {
        let t = utc as i64 + offset;
        let days = t.div_euclid(86_400);
        let rem = t.rem_euclid(86_400);
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        Local {
            year: yoe + era * 400 + i64::from(month <= 2),
            month,
            day,
            weekday: (days + 4).rem_euclid(7) as u32,
            hour: (rem / 3600) as u32,
            minute: (rem % 3600 / 60) as u32,
        }
    }

    /// "2:05 PM"
    pub fn clock(&self) -> String {
        let h = match self.hour % 12 {
            0 => 12,
            h => h,
        };
        format!(
            "{h}:{:02} {}",
            self.minute,
            if self.hour < 12 { "AM" } else { "PM" }
        )
    }

    /// "2:05" (the idle clock; AM/PM drawn small beside it)
    pub fn clock_digits(&self) -> String {
        let h = match self.hour % 12 {
            0 => 12,
            h => h,
        };
        format!("{h}:{:02}", self.minute)
    }

    pub fn am_pm(&self) -> &'static str {
        if self.hour < 12 {
            "AM"
        } else {
            "PM"
        }
    }

    pub fn day_name(&self) -> &'static str {
        DAYS[self.weekday as usize]
    }

    /// "Saturday, October 17"
    pub fn long_date(&self) -> String {
        format!(
            "{}, {} {}",
            WEEKDAYS[self.weekday as usize],
            MONTHS[(self.month - 1) as usize],
            self.day
        )
    }

    fn same_day(&self, other: &Local) -> bool {
        (self.year, self.month, self.day) == (other.year, other.month, other.day)
    }
}

/// "Today 2:00 PM", "Tonight 7:00 PM", "Tomorrow 1:00 PM", "Sat 2:00 PM".
pub fn when(start: u64, now: u64, offset: i64) -> String {
    let at = Local::at(start, offset);
    let today = Local::at(now, offset);
    let tomorrow = Local::at(now + 86_400, offset);
    let day = if at.same_day(&today) {
        if at.hour >= 17 {
            "Tonight"
        } else {
            "Today"
        }
    } else if at.same_day(&tomorrow) {
        "Tomorrow"
    } else {
        at.day_name()
    };
    format!("{day} {}", at.clock())
}

/// "in 3 h 20 min", "in 45 min", "now", "2 h ago".
pub fn relative(start: u64, now: u64) -> String {
    if start > now {
        let mins = (start - now).div_ceil(60);
        match (mins / 60, mins % 60) {
            (0, m) => format!("in {m} min"),
            (h, 0) if h < 48 => format!("in {h} h"),
            (h, m) if h < 48 => format!("in {h} h {m} min"),
            (h, _) => format!("in {} days", h / 24),
        }
    } else {
        let mins = (now - start) / 60;
        match mins {
            0..=1 => "just now".into(),
            m if m < 60 => format!("{m} min ago"),
            m if m < 48 * 60 => format!("{} h ago", m / 60),
            m => format!("{} days ago", m / 1440),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAT_2PM_EDT: u64 = 1_792_260_000; // 2026-10-17 18:00 UTC
    const EDT: i64 = -4 * 3600;

    #[test]
    fn calendar() {
        let l = Local::at(SAT_2PM_EDT, EDT);
        assert_eq!(
            (l.year, l.month, l.day, l.weekday, l.hour, l.minute),
            (2026, 10, 17, 6, 14, 0)
        );
        assert_eq!(l.clock(), "2:00 PM");
        assert_eq!(l.long_date(), "Saturday, October 17");
        assert_eq!(Local::at(0, 0).weekday, 4, "1970-01-01 was a Thursday");
        assert_eq!(Local::at(SAT_2PM_EDT - 14 * 3600, EDT).clock(), "12:00 AM");
    }

    #[test]
    fn when_and_relative() {
        let now = SAT_2PM_EDT - 3 * 3600; // 11 AM
        assert_eq!(when(SAT_2PM_EDT, now, EDT), "Today 2:00 PM");
        assert_eq!(when(SAT_2PM_EDT + 5 * 3600, now, EDT), "Tonight 7:00 PM");
        assert_eq!(when(SAT_2PM_EDT + 23 * 3600, now, EDT), "Tomorrow 1:00 PM");
        assert_eq!(when(SAT_2PM_EDT + 3 * 86_400, now, EDT), "Tue 2:00 PM");
        assert_eq!(relative(now + 200 * 60, now), "in 3 h 20 min");
        assert_eq!(relative(now + 45 * 60 - 30, now), "in 45 min");
        assert_eq!(relative(now - 12 * 60, now), "12 min ago");
    }
}
