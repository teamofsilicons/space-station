//! The 5-field cron behind a `{schedule}` trigger: minute, hour, day of month, month, day of
//! week, always UTC, with `*`, `n`, `a-b`, comma lists and a `/step` on any of them. Each field
//! is a bitmask, so a match is a shift and a test, and `next_after` walks minutes but skips a
//! whole day the moment the date cannot match.

use chrono::{DateTime, Datelike, NaiveTime, TimeDelta, Timelike, Utc};

/// The `(lo, hi)` of each field. Day of week takes `7` as a second spelling of Sunday.
const FIELDS: [(u32, u32); 5] = [(0, 59), (0, 23), (1, 31), (1, 12), (0, 7)];
/// `next_after` gives up after this many steps, so an expression that matches nothing
/// (`0 0 30 2 *`) ends. A real one needs at most four years of day skips (February 29) plus one
/// day of minutes.
const STEPS: usize = 5_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cron([u64; 5]);

impl Cron {
    /// `"*/5 * * * *"`. `None` for anything that is not five fields we understand.
    pub fn parse(expr: &str) -> Option<Cron> {
        let mut parts = expr.split_whitespace();
        let mut bits = [0; 5];
        for (i, (lo, hi)) in FIELDS.iter().enumerate() {
            bits[i] = field(parts.next()?, *lo, *hi)?;
        }
        if parts.next().is_some() {
            return None;
        }
        bits[4] |= (bits[4] >> 7) & 1;
        Some(Cron(bits))
    }

    /// The first minute strictly after `after` that this fires on; `None` when there is none in
    /// the next few years. An occurrence missed while the engine was away comes back in the past.
    pub fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let mut t = after.with_second(0)?.with_nanosecond(0)? + TimeDelta::minutes(1);
        for _ in 0..STEPS {
            if !self.day(t) {
                t = (t.date_naive() + TimeDelta::days(1)).and_time(NaiveTime::MIN).and_utc();
            } else if self.has(0, t.minute()) && self.has(1, t.hour()) {
                return Some(t);
            } else {
                t += TimeDelta::minutes(1);
            }
        }
        None
    }

    fn has(&self, field: usize, value: u32) -> bool {
        self.0[field] >> value & 1 == 1
    }

    /// The month and the day. Cron's rule for the two day fields: when both are restricted a day
    /// matching either one fires; when one is `*` both must match.
    fn day(&self, t: DateTime<Utc>) -> bool {
        let (dom, dow) = (self.has(2, t.day()), self.has(4, t.weekday().num_days_from_sunday()));
        let every = |f: usize| self.0[f] == mask(FIELDS[f].0, FIELDS[f].1);
        self.has(3, t.month()) && if every(2) || every(4) { dom && dow } else { dom || dow }
    }
}

fn mask(lo: u32, hi: u32) -> u64 {
    (lo..=hi).fold(0, |m, v| m | 1 << v)
}

/// One field: comma-separated `*`, `n` or `a-b`, each with an optional `/step`.
fn field(text: &str, lo: u32, hi: u32) -> Option<u64> {
    let mut bits = 0;
    for part in text.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => (range, step.parse().ok().filter(|s| *s > 0)?),
            None => (part, 1),
        };
        let (first, last) = match range.split_once('-') {
            Some((a, b)) => (number(a, lo, hi)?, number(b, lo, hi)?),
            None if range == "*" => (lo, hi),
            // `5/15` is every 15th from 5 on, the way cron reads a bare start with a step.
            None if step > 1 => (number(range, lo, hi)?, hi),
            None => (number(range, lo, hi)?, number(range, lo, hi)?),
        };
        if first > last {
            return None;
        }
        bits |= (first..=last).step_by(step).fold(0u64, |m, v| m | 1 << v);
    }
    Some(bits)
}

fn number(text: &str, lo: u32, hi: u32) -> Option<u32> {
    text.parse().ok().filter(|n| (lo..=hi).contains(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).unwrap().to_utc()
    }

    /// `next` of `expr` after `from`, as an RFC 3339 string.
    fn next(expr: &str, from: &str) -> String {
        Cron::parse(expr).expect(expr).next_after(at(from)).expect("an occurrence").to_rfc3339()
    }

    #[test]
    fn stars_steps_ranges_and_lists_pick_the_next_minute() {
        assert_eq!(next("* * * * *", "2025-03-04T10:00:30Z"), "2025-03-04T10:01:00+00:00");
        assert_eq!(next("*/5 * * * *", "2025-03-04T10:03:00Z"), "2025-03-04T10:05:00+00:00");
        assert_eq!(next("*/5 * * * *", "2025-03-04T10:05:00Z"), "2025-03-04T10:10:00+00:00", "strictly after");
        assert_eq!(next("0 9-17 * * *", "2025-03-04T18:30:00Z"), "2025-03-05T09:00:00+00:00");
        assert_eq!(next("0,30 * * * *", "2025-03-04T10:05:00Z"), "2025-03-04T10:30:00+00:00");
        assert_eq!(next("5/15 * * * *", "2025-03-04T10:06:00Z"), "2025-03-04T10:20:00+00:00");
        assert_eq!(next("0 0 1 1 *", "2025-03-04T10:00:00Z"), "2026-01-01T00:00:00+00:00");
    }

    #[test]
    fn the_day_fields_follow_crons_union_rule() {
        // 2025-03-04 is a Tuesday; 2025-03-10 the next Monday.
        assert_eq!(next("0 12 * * 1", "2025-03-04T10:00:00Z"), "2025-03-10T12:00:00+00:00");
        assert_eq!(next("0 12 * * 7", "2025-03-04T10:00:00Z"), "2025-03-09T12:00:00+00:00", "7 is Sunday, like 0");
        assert_eq!(next("0 12 5 * *", "2025-03-04T10:00:00Z"), "2025-03-05T12:00:00+00:00");
        // Both restricted: the 5th *or* a Monday, never their intersection.
        assert_eq!(next("0 12 5 * 1", "2025-03-04T10:00:00Z"), "2025-03-05T12:00:00+00:00");
        assert_eq!(next("0 12 5 * 1", "2025-03-05T12:00:00Z"), "2025-03-10T12:00:00+00:00");
    }

    #[test]
    fn an_occurrence_missed_while_away_comes_back_in_the_past() {
        let now = at("2025-03-04T10:07:00Z");
        let last = at("2025-03-04T09:00:00Z");
        let due = Cron::parse("*/5 * * * *").unwrap().next_after(last).unwrap();
        assert!(due < now, "the engine fires it once, at 09:05, the moment it is back");
        assert!(Cron::parse("*/5 * * * *").unwrap().next_after(now).unwrap() > now, "and then looks forward");
        assert_eq!(Cron::parse("0 0 30 2 *").unwrap().next_after(now), None, "February 30 never comes");
    }

    #[test]
    fn anything_but_five_fields_we_understand_is_refused() {
        for expr in [
            "* * * *",
            "* * * * * *",
            "",
            "60 * * * *",
            "* 24 * * *",
            "0 0 0 * *",
            "0 0 * 13 *",
            "0 0 * * 8",
            "*/0 * * * *",
            "5-1 * * * *",
            "a * * * *",
            "*/x * * * *",
            "1.5 * * * *",
        ] {
            assert_eq!(Cron::parse(expr), None, "{expr:?} is not a schedule");
        }
        assert!(Cron::parse("  */5   *  * * *  ").is_some(), "any run of spaces separates fields");
    }
}
