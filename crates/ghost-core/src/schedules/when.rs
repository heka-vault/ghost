//! When a schedule runs: the three kinds, how each is written, and the next time after a
//! given one. No clock is read here; every function takes the time it is asked about.
//!
//! The kinds are the ones OpenClaw and Hermes Agent both have (`at`, `every`, `cron`); the
//! forms accepted are theirs too. A cron expression is the five fields of crontab,
//! evaluated in a named time zone by `croner`, which carries a time that falls in a
//! spring-forward gap to the first real moment after it and runs a time that repeats in
//! the autumn once.

use std::str::FromStr;

use chrono::{DateTime, Duration, LocalResult, NaiveDateTime, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use croner::Cron;
use croner::parser::{CronParser, Seconds, Year};
use serde::{Deserialize, Serialize};

/// An interval is a minute at the least, and a year at the most.
pub const MIN_EVERY_SECS: u64 = 60;
pub const MAX_EVERY_SECS: u64 = 366 * 24 * 3600;
/// How many times a schedule is stepped forward looking for a time inside the active hours.
const ACTIVE_STEPS: usize = 5000;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct WhenError(pub String);

fn bad(msg: impl Into<String>) -> WhenError {
    WhenError(msg.into())
}

/// A schedule as it is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Schedule {
    /// Once, at a time.
    At { at: DateTime<Utc> },
    /// Every interval, counted from `anchor` (when the schedule was made or last edited),
    /// so a restart does not move it.
    Every { secs: u64, anchor: DateTime<Utc> },
    /// A five-field cron expression in a time zone.
    Cron { expr: String, tz: String },
}

/// A schedule as it is written: exactly one of `at`, `every` and `cron`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Written {
    /// `2026-11-12T09:00` (the schedule's time zone), a time with an offset or `Z`, or a
    /// delay from now: `20m`, `2h`, `1d`.
    pub at: Option<String>,
    /// `30m`, `2h`, `1d`.
    pub every: Option<String>,
    /// `0 9 * * 1-5`.
    pub cron: Option<String>,
    /// An IANA name (`Europe/London`); the machine's when left out.
    pub tz: Option<String>,
}

/// The machine's time zone by name, UTC when it can't be told.
pub fn machine_tz() -> String {
    iana_time_zone::get_timezone()
        .ok()
        .filter(|name| Tz::from_str(name).is_ok())
        .unwrap_or_else(|| "UTC".to_string())
}

pub fn parse_tz(name: &str) -> Result<Tz, WhenError> {
    Tz::from_str(name.trim()).map_err(|_| {
        bad(format!(
            "{name} is not a time zone (use a name like Europe/London)"
        ))
    })
}

/// `30m`, `2h`, `1d`, `90s`, `1w`, or a bare number of minutes: whole seconds.
pub fn parse_duration(text: &str) -> Result<u64, WhenError> {
    let t = text.trim().to_lowercase();
    let t = t.strip_prefix('+').unwrap_or(&t).trim();
    let split = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
    let (digits, unit) = t.split_at(split);
    let n: u64 = digits
        .parse()
        .map_err(|_| bad(format!("{text} is not a length of time (30m, 2h, 1d)")))?;
    let per = match unit.trim() {
        "s" | "sec" | "secs" | "second" | "seconds" => 1,
        "" | "m" | "min" | "mins" | "minute" | "minutes" => 60,
        "h" | "hr" | "hrs" | "hour" | "hours" => 3600,
        "d" | "day" | "days" => 86400,
        "w" | "week" | "weeks" => 7 * 86400,
        _ => return Err(bad(format!("{text} is not a length of time (30m, 2h, 1d)"))),
    };
    n.checked_mul(per)
        .ok_or_else(|| bad(format!("{text} is too long")))
}

/// `1800` as `30m`.
pub fn say_duration(secs: u64) -> String {
    for (per, unit) in [(7 * 86400, "w"), (86400, "d"), (3600, "h"), (60, "m")] {
        if secs >= per && secs.is_multiple_of(per) {
            return format!("{}{unit}", secs / per);
        }
    }
    format!("{secs}s")
}

fn cron(expr: &str) -> Result<Cron, WhenError> {
    CronParser::builder()
        .seconds(Seconds::Disallowed)
        .year(Year::Disallowed)
        .build()
        .parse(expr)
        .map_err(|e| {
            bad(format!(
                "{expr} is not a cron expression (five fields: minute hour day month weekday): {e}"
            ))
        })
}

/// A local time in a zone as one moment: the earlier of two when the clocks go back over
/// it, the first real moment after it when they go forward over it.
fn local_moment(tz: Tz, local: NaiveDateTime) -> Option<DateTime<Utc>> {
    match tz.from_local_datetime(&local) {
        LocalResult::Single(t) => Some(t.with_timezone(&Utc)),
        LocalResult::Ambiguous(first, _) => Some(first.with_timezone(&Utc)),
        LocalResult::None => (1..=180).find_map(|m| {
            tz.from_local_datetime(&(local + Duration::minutes(m)))
                .earliest()
                .map(|t| t.with_timezone(&Utc))
        }),
    }
}

fn parse_at(text: &str, tz: Tz, now: DateTime<Utc>) -> Result<DateTime<Utc>, WhenError> {
    let t = text.trim();
    if let Ok(with_offset) = DateTime::parse_from_rfc3339(t) {
        return Ok(with_offset.with_timezone(&Utc));
    }
    for format in [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(local) = NaiveDateTime::parse_from_str(t, format) {
            return local_moment(tz, local)
                .ok_or_else(|| bad(format!("{text} is not a time in {tz}")));
        }
    }
    if let Ok(secs) = parse_duration(t) {
        return Ok(now + Duration::seconds(secs as i64));
    }
    Err(bad(format!(
        "{text} is not a time (2026-11-12T09:00, a time with an offset, or a delay like 20m)"
    )))
}

impl Schedule {
    /// A schedule from how it was written, with `now` as the anchor of an interval and
    /// the start of a delay, and `default_tz` where no zone is named.
    pub fn parse(w: &Written, now: DateTime<Utc>, default_tz: &str) -> Result<Self, WhenError> {
        fn given(v: &Option<String>) -> Option<&str> {
            v.as_deref().map(str::trim).filter(|s| !s.is_empty())
        }
        let (at, every, expr) = (given(&w.at), given(&w.every), given(&w.cron));
        let tz_name = given(&w.tz).unwrap_or(default_tz);
        let tz = parse_tz(tz_name)?;
        match (at, every, expr) {
            (Some(at), None, None) => Ok(Schedule::At {
                at: parse_at(at, tz, now)?,
            }),
            (None, Some(every), None) => {
                let secs = parse_duration(every)?;
                if !(MIN_EVERY_SECS..=MAX_EVERY_SECS).contains(&secs) {
                    return Err(bad("every is a minute at the least and a year at the most"));
                }
                Ok(Schedule::Every { secs, anchor: now })
            }
            (None, None, Some(expr)) => {
                cron(expr)?;
                Ok(Schedule::Cron {
                    expr: expr.split_whitespace().collect::<Vec<_>>().join(" "),
                    tz: tz.name().to_string(),
                })
            }
            (None, None, None) => Err(bad(
                "a schedule is one of at (a time), every (30m, 2h, 1d) or cron (0 9 * * 1-5)",
            )),
            _ => Err(bad(
                "a schedule is one of at, every or cron, not two of them",
            )),
        }
    }

    /// Whether the stored schedule still reads: a cron expression and its zone.
    pub fn check(&self) -> Result<(), WhenError> {
        match self {
            Schedule::At { .. } => Ok(()),
            Schedule::Every { secs, .. } => (*secs >= MIN_EVERY_SECS)
                .then_some(())
                .ok_or_else(|| bad("every is a minute at the least")),
            Schedule::Cron { expr, tz } => {
                parse_tz(tz)?;
                cron(expr).map(|_| ())
            }
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Schedule::At { .. } => "at",
            Schedule::Every { .. } => "every",
            Schedule::Cron { .. } => "cron",
        }
    }

    pub fn is_once(&self) -> bool {
        matches!(self, Schedule::At { .. })
    }

    /// The first run strictly after `after`; None when there is none (a time that has
    /// passed, an expression that never matches again).
    pub fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self {
            Schedule::At { at } => (*at > after).then_some(*at),
            Schedule::Every { secs, anchor } => {
                let secs = (*secs).max(1) as i64;
                let elapsed = (after - *anchor).num_seconds();
                let steps = if elapsed < 0 { 1 } else { elapsed / secs + 1 };
                Some(*anchor + Duration::seconds(steps * secs))
            }
            Schedule::Cron { expr, tz } => {
                let tz = parse_tz(tz).ok()?;
                let start = after.with_timezone(&tz);
                cron(expr)
                    .ok()?
                    .find_next_occurrence(&start, false)
                    .ok()
                    .map(|t| t.with_timezone(&Utc))
            }
        }
    }

    /// As the operator reads it: `at 2026-11-12 09:00`, `every 30m`, `0 9 * * 1-5`.
    pub fn say(&self, tz: &str) -> String {
        match self {
            Schedule::At { at } => {
                let zone = parse_tz(tz).unwrap_or(Tz::UTC);
                format!("at {}", at.with_timezone(&zone).format("%Y-%m-%d %H:%M"))
            }
            Schedule::Every { secs, .. } => format!("every {}", say_duration(*secs)),
            Schedule::Cron { expr, tz } => format!("{expr} ({tz})"),
        }
    }
}

/// The hours of the day a schedule may run in, in its time zone. `start` after `end` runs
/// over midnight (`22:00` to `06:00`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveHours {
    pub start: String,
    pub end: String,
}

fn clock(text: &str) -> Result<NaiveTime, WhenError> {
    NaiveTime::parse_from_str(text.trim(), "%H:%M")
        .map_err(|_| bad(format!("{text} is not a time of day (08:00)")))
}

impl ActiveHours {
    pub fn parse(start: &str, end: &str) -> Result<Self, WhenError> {
        let (s, e) = (clock(start)?, clock(end)?);
        if s == e {
            return Err(bad("active hours need a start and a different end"));
        }
        Ok(Self {
            start: s.format("%H:%M").to_string(),
            end: e.format("%H:%M").to_string(),
        })
    }

    pub fn contains(&self, when: DateTime<Utc>, tz: &str) -> bool {
        let (Ok(start), Ok(end)) = (clock(&self.start), clock(&self.end)) else {
            return true;
        };
        let zone = parse_tz(tz).unwrap_or(Tz::UTC);
        let t = when.with_timezone(&zone).time();
        if start < end {
            start <= t && t < end
        } else {
            t >= start || t < end
        }
    }
}

/// The first run after `after` that falls inside the active hours. A one-off ignores
/// them: the operator named its time.
pub fn next_run(
    schedule: &Schedule,
    active: Option<&ActiveHours>,
    tz: &str,
    after: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let Some(active) = active.filter(|_| !schedule.is_once()) else {
        return schedule.next_after(after);
    };
    let mut cursor = after;
    for _ in 0..ACTIVE_STEPS {
        let next = schedule.next_after(cursor)?;
        if active.contains(next, tz) {
            return Some(next);
        }
        cursor = next;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn written(
        at: Option<&str>,
        every: Option<&str>,
        cron: Option<&str>,
        tz: Option<&str>,
    ) -> Written {
        Written {
            at: at.map(String::from),
            every: every.map(String::from),
            cron: cron.map(String::from),
            tz: tz.map(String::from),
        }
    }

    #[test]
    fn a_schedule_is_exactly_one_kind() {
        let now = utc("2026-11-11T12:00:00Z");
        assert!(Schedule::parse(&Written::default(), now, "UTC").is_err());
        let two = written(Some("20m"), Some("1h"), None, None);
        assert!(
            Schedule::parse(&two, now, "UTC")
                .unwrap_err()
                .to_string()
                .contains("not two")
        );
        for bad in [
            written(None, Some("5s"), None, None),
            written(None, Some("soon"), None, None),
            written(None, None, Some("every day"), None),
            written(None, None, Some("0 0 9 * * *"), None), // six fields: no seconds
            written(None, None, Some("0 9 * * *"), Some("Mars/Olympus")),
            written(Some("tomorrow-ish"), None, None, None),
        ] {
            assert!(Schedule::parse(&bad, now, "UTC").is_err(), "{bad:?}");
        }
    }

    #[test]
    fn at_reads_a_local_time_an_offset_and_a_delay() {
        let now = utc("2026-11-11T12:00:00Z");
        let at = |s: &str| match Schedule::parse(
            &written(Some(s), None, None, None),
            now,
            "America/Detroit",
        ) {
            Ok(Schedule::At { at }) => at,
            other => panic!("{other:?}"),
        };
        assert_eq!(at("2026-11-12T09:00"), utc("2026-11-12T14:00:00Z")); // EST
        assert_eq!(at("2026-11-12T09:00:00Z"), utc("2026-11-12T09:00:00Z"));
        assert_eq!(at("2026-11-12T09:00:00+01:00"), utc("2026-11-12T08:00:00Z"));
        assert_eq!(at("20m"), utc("2026-11-11T12:20:00Z"));
        // The clocks go back over 01:30 on 2026-11-01: the earlier of the two.
        assert_eq!(at("2026-11-01T01:30"), utc("2026-11-01T05:30:00Z"));
        // And forward over 02:30 on 2027-03-14: the first real moment after it.
        assert_eq!(at("2027-03-14T02:30"), utc("2027-03-14T07:00:00Z"));
    }

    #[test]
    fn at_runs_once_and_never_again() {
        let s = Schedule::At {
            at: utc("2026-11-12T09:00:00Z"),
        };
        assert_eq!(
            s.next_after(utc("2026-11-11T00:00:00Z")),
            Some(utc("2026-11-12T09:00:00Z"))
        );
        assert_eq!(s.next_after(utc("2026-11-12T09:00:00Z")), None);
    }

    #[test]
    fn every_keeps_its_anchor_across_a_restart_and_a_clock_change() {
        let anchor = utc("2026-10-31T12:10:00Z");
        let s = Schedule::Every { secs: 1800, anchor };
        // Whenever it is asked (a restart at any moment), the grid is the anchor's.
        assert_eq!(s.next_after(anchor), Some(utc("2026-10-31T12:40:00Z")));
        assert_eq!(
            s.next_after(utc("2026-10-31T12:39:59Z")),
            Some(utc("2026-10-31T12:40:00Z"))
        );
        assert_eq!(
            s.next_after(utc("2026-10-31T12:40:00Z")),
            Some(utc("2026-10-31T13:10:00Z"))
        );
        assert_eq!(
            s.next_after(utc("2026-11-03T07:00:01Z")),
            Some(utc("2026-11-03T07:10:00Z"))
        );
        // An interval is elapsed time: a day is 24 hours over the night the clocks go
        // back, so the local hour moves (as both leaders do).
        let daily = Schedule::Every {
            secs: 86400,
            anchor: utc("2026-10-31T13:00:00Z"), // 09:00 EDT
        };
        assert_eq!(
            daily.next_after(utc("2026-10-31T13:00:00Z")),
            Some(utc("2026-11-01T13:00:00Z"))
        ); // 08:00 EST
    }

    #[test]
    fn cron_keeps_the_local_hour_across_both_clock_changes() {
        let s = Schedule::parse(
            &written(None, None, Some("0 9 * * *"), Some("America/New_York")),
            utc("2026-10-30T00:00:00Z"),
            "UTC",
        )
        .unwrap();
        // Autumn: 09:00 EDT is 13:00Z, 09:00 EST is 14:00Z.
        assert_eq!(
            s.next_after(utc("2026-10-31T12:59:00Z")),
            Some(utc("2026-10-31T13:00:00Z"))
        );
        assert_eq!(
            s.next_after(utc("2026-10-31T13:00:00Z")),
            Some(utc("2026-11-01T14:00:00Z"))
        );
        assert_eq!(
            s.next_after(utc("2026-11-01T14:00:00Z")),
            Some(utc("2026-11-02T14:00:00Z"))
        );
        // Spring: 2027-03-14, 09:00 EST the day before is 14:00Z, 09:00 EDT is 13:00Z.
        assert_eq!(
            s.next_after(utc("2027-03-13T14:00:00Z")),
            Some(utc("2027-03-14T13:00:00Z"))
        );
        // A restart asks again from any moment and gets the same answer.
        assert_eq!(
            s.next_after(utc("2027-03-14T03:33:00Z")),
            Some(utc("2027-03-14T13:00:00Z"))
        );
    }

    #[test]
    fn cron_runs_once_in_the_repeated_hour_and_after_the_missing_one() {
        let ny = |expr: &str| Schedule::Cron {
            expr: expr.into(),
            tz: "America/New_York".into(),
        };
        // 01:30 happens twice on 2026-11-01 (05:30Z and 06:30Z): one run.
        let twice = ny("30 1 * * *");
        let first = twice.next_after(utc("2026-11-01T04:00:00Z")).unwrap();
        assert_eq!(first, utc("2026-11-01T05:30:00Z"));
        assert_eq!(twice.next_after(first), Some(utc("2026-11-02T06:30:00Z")));
        // 02:30 does not happen on 2027-03-14: it runs at the first moment after the gap.
        let missing = ny("30 2 * * *");
        assert_eq!(
            missing.next_after(utc("2027-03-14T05:00:00Z")),
            Some(utc("2027-03-14T07:00:00Z"))
        );
    }

    #[test]
    fn the_machine_zone_is_the_default_and_a_name_is_kept() {
        let now = utc("2026-11-11T12:00:00Z");
        let s = Schedule::parse(
            &written(None, None, Some("0  9 * * 1-5"), None),
            now,
            "Europe/London",
        )
        .unwrap();
        assert_eq!(
            s,
            Schedule::Cron {
                expr: "0 9 * * 1-5".into(),
                tz: "Europe/London".into()
            }
        );
        assert!(parse_tz(&machine_tz()).is_ok());
        assert_eq!(s.say("Europe/London"), "0 9 * * 1-5 (Europe/London)");
    }

    #[test]
    fn active_hours_move_a_run_to_the_next_one_inside_them() {
        let hours = ActiveHours::parse("08:00", "18:00").unwrap();
        let s = Schedule::Every {
            secs: 3600,
            anchor: utc("2026-11-11T00:30:00Z"),
        };
        // 17:30 UTC is inside; the next, 18:30, is not: the run after is 08:30 tomorrow.
        assert_eq!(
            next_run(&s, Some(&hours), "UTC", utc("2026-11-11T17:00:00Z")),
            Some(utc("2026-11-11T17:30:00Z"))
        );
        assert_eq!(
            next_run(&s, Some(&hours), "UTC", utc("2026-11-11T17:30:00Z")),
            Some(utc("2026-11-12T08:30:00Z"))
        );
        // Over midnight.
        let night = ActiveHours::parse("22:00", "06:00").unwrap();
        assert!(night.contains(utc("2026-11-11T23:00:00Z"), "UTC"));
        assert!(night.contains(utc("2026-11-11T05:59:00Z"), "UTC"));
        assert!(!night.contains(utc("2026-11-11T12:00:00Z"), "UTC"));
        // A one-off is not moved.
        let once = Schedule::At {
            at: utc("2026-11-11T23:00:00Z"),
        };
        assert_eq!(
            next_run(&once, Some(&hours), "UTC", utc("2026-11-11T12:00:00Z")),
            Some(utc("2026-11-11T23:00:00Z"))
        );
        assert!(ActiveHours::parse("8am", "18:00").is_err());
    }

    #[test]
    fn durations_read_and_say() {
        assert_eq!(parse_duration("30m").unwrap(), 1800);
        assert_eq!(parse_duration("2h").unwrap(), 7200);
        assert_eq!(parse_duration("1d").unwrap(), 86400);
        assert_eq!(parse_duration("45").unwrap(), 2700);
        assert_eq!(say_duration(1800), "30m");
        assert_eq!(say_duration(86400), "1d");
        assert_eq!(say_duration(90), "90s");
    }
}
