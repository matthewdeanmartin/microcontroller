//! The instance time zone as a POSIX TZ rule (`EST5EDT,M3.2.0,M11.1.0`),
//! plus civil-date arithmetic. Pure Rust, so the desktop and the board
//! agree, and recurrence across DST is unit-tested
//! (`spec/03-domain.md#recurring-events`).
//!
//! Supported: a standard name and offset, an optional daylight name and
//! offset (default one hour ahead), and `Mm.w.d[/time]` transition rules
//! (default: the US rules). Julian-day rules (`Jn`, `n`) are refused.
use miniframework::ApiError;

/// The household's zone when none is set.
pub const DEFAULT: &str = "EST5EDT,M3.2.0,M11.1.0";

/// Days since 1970-01-01 of a civil date (Howard Hinnant).
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = year - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

/// The civil date of a day number.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// 0 = Sunday.
pub fn weekday(days: i64) -> u32 {
    (days + 4).rem_euclid(7) as u32
}

pub fn days_in_month(year: i64, month: u32) -> u32 {
    let (ny, nm) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    (days_from_civil(ny, nm, 1) - days_from_civil(year, month, 1)) as u32
}

/// The `nth` (1-4, or -1 = last) `wd` of a month, as a day number.
pub fn nth_weekday(year: i64, month: u32, nth: i32, wd: u32) -> Option<i64> {
    let first = days_from_civil(year, month, 1);
    let len = i64::from(days_in_month(year, month));
    if nth > 0 {
        let offset = (i64::from(wd) - i64::from(weekday(first))).rem_euclid(7);
        let day = offset + 7 * i64::from(nth - 1);
        (day < len).then_some(first + day)
    } else if nth == -1 {
        let last = first + len - 1;
        Some(last - (i64::from(weekday(last)) - i64::from(wd)).rem_euclid(7))
    } else {
        None
    }
}

/// A local date and time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Local {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub weekday: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl Local {
    /// Days since the epoch of this local date.
    pub fn days(&self) -> i64 {
        days_from_civil(self.year, self.month, self.day)
    }

    /// Seconds into the local day.
    pub fn time(&self) -> i64 {
        i64::from(self.hour) * 3600 + i64::from(self.minute) * 60 + i64::from(self.second)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rule {
    month: u32,
    /// 1-4, or 5 = last.
    week: u32,
    weekday: u32,
    /// Local seconds after midnight.
    time: i64,
}

impl Rule {
    fn day(&self, year: i64) -> i64 {
        let nth = if self.week >= 5 { -1 } else { self.week as i32 };
        nth_weekday(year, self.month, nth, self.weekday).expect("weeks 1-4 always exist")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Zone {
    /// Seconds east of UTC (EST: -18000).
    std: i64,
    dst: Option<(i64, Rule, Rule)>,
}

fn bad(why: &str) -> ApiError {
    ApiError::bad_request(format!("Time zone: {why} (e.g. {DEFAULT})"))
}

/// `[+-]hh[:mm[:ss]]` → seconds, POSIX sign (west positive).
fn parse_offset(s: &str) -> Result<(i64, &str), ApiError> {
    let (sign, rest) = match s.as_bytes().first() {
        Some(b'-') => (-1, &s[1..]),
        Some(b'+') => (1, &s[1..]),
        _ => (1, s),
    };
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == ':'))
        .unwrap_or(rest.len());
    let (num, rest) = rest.split_at(end);
    let mut parts = num.split(':').map(|p| p.parse::<i64>());
    let h = parts
        .next()
        .and_then(Result::ok)
        .ok_or_else(|| bad("missing offset"))?;
    let m = parts
        .next()
        .transpose()
        .map_err(|_| bad("bad minutes"))?
        .unwrap_or(0);
    let sec = parts
        .next()
        .transpose()
        .map_err(|_| bad("bad seconds"))?
        .unwrap_or(0);
    if h > 24 || m > 59 || sec > 59 {
        return Err(bad("offset out of range"));
    }
    Ok((sign * (h * 3600 + m * 60 + sec), rest))
}

fn parse_name(s: &str) -> Result<(&str, &str), ApiError> {
    if let Some(rest) = s.strip_prefix('<') {
        let end = rest.find('>').ok_or_else(|| bad("unclosed <name>"))?;
        return Ok((&rest[..end], &rest[end + 1..]));
    }
    let end = s
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(s.len());
    if end < 3 {
        return Err(bad("zone names have at least 3 letters"));
    }
    Ok(s.split_at(end))
}

fn parse_rule(s: &str) -> Result<Rule, ApiError> {
    let (date, time) = match s.split_once('/') {
        Some((d, t)) => (d, Some(t)),
        None => (s, None),
    };
    let fields: Vec<&str> = date
        .strip_prefix('M')
        .ok_or_else(|| bad("only Mm.w.d rules are supported"))?
        .split('.')
        .collect();
    let [m, w, d] = fields.as_slice() else {
        return Err(bad("a rule is Mmonth.week.day"));
    };
    let num = |v: &str| v.parse::<u32>().map_err(|_| bad("bad rule number"));
    let (month, week, weekday) = (num(m)?, num(w)?, num(d)?);
    if !(1..=12).contains(&month) || !(1..=5).contains(&week) || weekday > 6 {
        return Err(bad("rule out of range"));
    }
    let time = match time {
        Some(t) => parse_offset(t)?.0,
        None => 2 * 3600,
    };
    Ok(Rule {
        month,
        week,
        weekday,
        time,
    })
}

impl Zone {
    pub fn parse(spec: &str) -> Result<Zone, ApiError> {
        let spec = spec.trim();
        let spec = if spec.is_empty() { DEFAULT } else { spec };
        let (_, rest) = parse_name(spec)?;
        let (west, rest) = parse_offset(rest)?;
        let std = -west;
        if rest.is_empty() {
            return Ok(Zone { std, dst: None });
        }
        let (_, rest) = parse_name(rest)?;
        let (dst, rest) = if rest.starts_with(',') || rest.is_empty() {
            (std + 3600, rest)
        } else {
            let (west, rest) = parse_offset(rest)?;
            (-west, rest)
        };
        let rules = rest.strip_prefix(',').unwrap_or("M3.2.0,M11.1.0");
        let rules = if rules.is_empty() {
            "M3.2.0,M11.1.0"
        } else {
            rules
        };
        let (start, end) = rules
            .split_once(',')
            .ok_or_else(|| bad("two rules are needed"))?;
        Ok(Zone {
            std,
            dst: Some((dst, parse_rule(start)?, parse_rule(end)?)),
        })
    }

    /// Seconds east of UTC at the instant `utc`.
    pub fn offset(&self, utc: u64) -> i64 {
        let Some((dst, start, end)) = self.dst else {
            return self.std;
        };
        let utc = utc as i64;
        let (year, _, _) = civil_from_days((utc + self.std).div_euclid(86_400));
        // Each transition happens at a local time in the offset before it.
        let on = start.day(year) * 86_400 + start.time - self.std;
        let off = end.day(year) * 86_400 + end.time - dst;
        let summer = if on < off {
            utc >= on && utc < off
        } else {
            utc >= on || utc < off
        };
        if summer {
            dst
        } else {
            self.std
        }
    }

    pub fn local(&self, utc: u64) -> Local {
        let secs = utc as i64 + self.offset(utc);
        let days = secs.div_euclid(86_400);
        let rem = secs.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        Local {
            year,
            month,
            day,
            weekday: weekday(days),
            hour: (rem / 3600) as u32,
            minute: (rem % 3600 / 60) as u32,
            second: (rem % 60) as u32,
        }
    }

    /// The UTC instant of a local wall-clock time (days since the epoch and
    /// seconds into the day). A time skipped by a spring-forward is moved
    /// an hour later; an ambiguous one (fall back) takes the first.
    pub fn to_utc(&self, days: i64, time: i64) -> u64 {
        let wall = days * 86_400 + time;
        let mut best = None;
        for guess in [self.std, self.dst.map_or(self.std, |d| d.0)] {
            let utc = wall - guess;
            if utc >= 0 && utc + self.offset(utc as u64) == wall {
                best = Some(best.map_or(utc, |b: i64| b.min(utc)));
            }
        }
        let utc = best.unwrap_or_else(|| wall - self.std);
        utc.max(0) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(y: i64, m: u32, d: u32, h: i64, min: i64) -> u64 {
        (days_from_civil(y, m, d) * 86_400 + h * 3600 + min * 60) as u64
    }

    #[test]
    fn civil_round_trip_and_weekdays() {
        for z in [-1000, 0, 19_000, 20_754, 40_000] {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z);
        }
        assert_eq!(weekday(days_from_civil(2026, 10, 21)), 3, "a Wednesday");
        assert_eq!(days_in_month(2028, 2), 29);
        assert_eq!(
            nth_weekday(2026, 11, 1, 5),
            Some(days_from_civil(2026, 11, 6)),
            "first Friday"
        );
        assert_eq!(
            nth_weekday(2026, 11, -1, 0),
            Some(days_from_civil(2026, 11, 29)),
            "last Sunday"
        );
        assert_eq!(nth_weekday(2026, 2, 5, 0), None);
    }

    #[test]
    fn us_eastern_switches_on_the_right_days() {
        let z = Zone::parse(DEFAULT).unwrap();
        // 2026: DST from Sun Mar 8 07:00 UTC to Sun Nov 1 06:00 UTC.
        assert_eq!(z.offset(utc(2026, 3, 8, 6, 59)), -5 * 3600);
        assert_eq!(z.offset(utc(2026, 3, 8, 7, 0)), -4 * 3600);
        assert_eq!(z.offset(utc(2026, 11, 1, 5, 59)), -4 * 3600);
        assert_eq!(z.offset(utc(2026, 11, 1, 6, 0)), -5 * 3600);
        let l = z.local(utc(2026, 10, 24, 23, 0));
        assert_eq!((l.month, l.day, l.hour, l.weekday), (10, 24, 19, 6));
    }

    #[test]
    fn wall_clock_to_utc_across_dst() {
        let z = Zone::parse(DEFAULT).unwrap();
        // 19:00 local before and after the change.
        assert_eq!(
            z.to_utc(days_from_civil(2026, 10, 30), 19 * 3600),
            utc(2026, 10, 30, 23, 0)
        );
        assert_eq!(
            z.to_utc(days_from_civil(2026, 11, 6), 19 * 3600),
            utc(2026, 11, 7, 0, 0)
        );
        // 02:30 on the spring-forward day doesn't exist: an hour later.
        assert_eq!(
            z.to_utc(days_from_civil(2026, 3, 8), 2 * 3600 + 1800),
            utc(2026, 3, 8, 7, 30)
        );
        // 01:30 on the fall-back day happens twice: the first one.
        assert_eq!(
            z.to_utc(days_from_civil(2026, 11, 1), 3600 + 1800),
            utc(2026, 11, 1, 5, 30)
        );
    }

    #[test]
    fn other_zones() {
        assert_eq!(Zone::parse("UTC0").unwrap().offset(0), 0);
        let cet = Zone::parse("CET-1CEST,M3.5.0,M10.5.0/3").unwrap();
        assert_eq!(cet.offset(utc(2026, 7, 1, 0, 0)), 2 * 3600);
        assert_eq!(cet.offset(utc(2026, 12, 1, 0, 0)), 3600);
        let syd = Zone::parse("AEST-10AEDT,M10.1.0,M4.1.0/3").unwrap();
        assert_eq!(
            syd.offset(utc(2026, 1, 15, 0, 0)),
            11 * 3600,
            "southern summer"
        );
        assert_eq!(syd.offset(utc(2026, 7, 15, 0, 0)), 10 * 3600);
        let india = Zone::parse("<+0530>-5:30").unwrap();
        assert_eq!(india.offset(0), 5 * 3600 + 1800);
        assert!(Zone::parse("EST5EDT,J60,J300").is_err());
        assert!(Zone::parse("X5").is_err());
    }
}
