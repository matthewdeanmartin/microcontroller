//! Bounded recurring series, in household wall time (including DST changes).
use crate::messages::{PlanInput, TimeInput};
use crate::screen::clock::Local;
use crate::state::State;
use miniframework::ApiError;

fn days(year: i64, month: u32, day: u32) -> i64 {
    let y = year - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468
}

pub fn occurrence(
    start: u64,
    frequency: &str,
    n: u32,
    offset: fn(u64) -> i64,
) -> Result<u64, ApiError> {
    if start > 4_102_444_800 {
        return Err(ApiError::bad_request("Recurring dates must be before 2100"));
    }
    let local = Local::at(start, offset(start));
    let base_days = days(local.year, local.month, local.day);
    let target_days = match frequency {
        "weekly" => base_days + i64::from(n) * 7,
        "fortnightly" => base_days + i64::from(n) * 14,
        "monthly" => {
            let month = local.year * 12 + i64::from(local.month) - 1 + i64::from(n);
            let year = month.div_euclid(12);
            let m = (month.rem_euclid(12) + 1) as u32;
            let next = month + 1;
            let length =
                days(next.div_euclid(12), (next.rem_euclid(12) + 1) as u32, 1) - days(year, m, 1);
            days(year, m, local.day.min(length as u32))
        }
        _ => {
            return Err(ApiError::bad_request(
                "Repeat weekly, fortnightly or monthly",
            ))
        }
    };
    let wall = target_days * 86400
        + i64::from(local.hour) * 3600
        + i64::from(local.minute) * 60
        + (start % 60) as i64;
    let mut utc = (wall - offset(start)).max(0) as u64;
    for _ in 0..3 {
        utc = (wall - offset(utc)).max(0) as u64;
    }
    if utc as i64 + offset(utc) != wall {
        return Err(ApiError::bad_request(
            "A recurring time falls in a daylight-saving gap; choose another time",
        ));
    }
    Ok(utc)
}

impl State {
    pub fn create_series(
        &mut self,
        host: u32,
        input: &PlanInput,
        now: u64,
        offset: fn(u64) -> i64,
    ) -> Result<u32, ApiError> {
        let Some(repeat) = &input.repeat else {
            return self.create_plan(host, input, now);
        };
        if !(2..=12).contains(&repeat.count) {
            return Err(ApiError::bad_request("A series needs 2 to 12 occurrences"));
        }
        if self
            .plans
            .iter()
            .filter(|p| crate::plans::active(p))
            .count()
            + repeat.count as usize
            > self.limits.active_plans
        {
            return Err(ApiError::new(
                409,
                "full",
                "Not enough room for this series; reduce its occurrences",
            ));
        }
        let times = input
            .times
            .as_ref()
            .filter(|t| !t.is_empty())
            .ok_or_else(|| ApiError::bad_request("Choose a future time before repeating a plan"))?;
        if times.iter().any(|t| t.start <= now) {
            return Err(ApiError::bad_request("Recurring plans need future times"));
        }
        // Prepare every date before mutating state. API mutation rolls back on any error.
        let mut drafts = Vec::new();
        for n in 0..repeat.count {
            let mut draft = input.clone();
            draft.repeat = None;
            draft.times = Some(
                times
                    .iter()
                    .map(|t| {
                        let start = occurrence(t.start, &repeat.frequency, n, offset)?;
                        let end = if t.end == 0 {
                            0
                        } else {
                            start
                                .checked_add(t.end.saturating_sub(t.start))
                                .ok_or_else(|| {
                                    ApiError::bad_request("Invalid recurring end time")
                                })?
                        };
                        Ok(TimeInput { id: 0, start, end })
                    })
                    .collect::<Result<Vec<_>, ApiError>>()?,
            );
            drafts.push(draft);
        }
        let backup = self.to_stored();
        let mail = self.mail.clone();
        let mut ids = Vec::new();
        for draft in drafts {
            match self.create_plan(host, &draft, now) {
                Ok(id) => ids.push(id),
                Err(e) => {
                    *self = State::from_stored(backup, self.limits.clone());
                    self.mail = mail;
                    return Err(e);
                }
            }
        }
        let first = ids[0];
        for p in &mut self.plans {
            if ids.contains(&p.id) {
                p.series = first;
            }
        }
        Ok(first)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn monthly_clamps_from_original_day_without_drifting() {
        let jan31 = (days(2028, 1, 31) * 86400 + 19 * 3600) as u64;
        let feb = Local::at(occurrence(jan31, "monthly", 1, |_| 0).unwrap(), 0);
        let mar = Local::at(occurrence(jan31, "monthly", 2, |_| 0).unwrap(), 0);
        assert_eq!((feb.month, feb.day, feb.hour), (2, 29, 19));
        assert_eq!((mar.month, mar.day), (3, 31));
    }
    #[test]
    fn weekly_preserves_wall_time_across_offset_change() {
        fn offset(t: u64) -> i64 {
            if t >= 1_800_000_000 {
                3600
            } else {
                0
            }
        }
        let start = 1_799_900_000;
        let next = occurrence(start, "weekly", 1, offset).unwrap();
        assert_eq!(next - start, 7 * 86400 - 3600);
    }
}
