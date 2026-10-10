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
