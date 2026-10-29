//! Timestamps as the Python daemon wrote them, so the two read each other's rows.
//!
//! SQLite columns hold `datetime.isoformat()`: `2026-10-28T20:40:26.093494+00:00`, the
//! microseconds only when they are not zero. JSON bodies hold pydantic's form, with a `Z`.
//! Both are read back.

use chrono::{DateTime, Timelike, Utc};

/// Now, at microsecond precision: what Python's datetimes, the rows and the JSON carry,
/// so a value written and read back compares equal.
pub fn utcnow() -> DateTime<Utc> {
    let now = Utc::now();
    now.with_nanosecond(now.timestamp_subsec_micros() * 1000)
        .unwrap_or(now)
}

/// `datetime.isoformat()` for an aware UTC datetime.
pub fn iso(dt: &DateTime<Utc>) -> String {
    let micros = dt.timestamp_subsec_micros();
    if micros == 0 {
        dt.format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
    } else {
        format!("{}.{:06}+00:00", dt.format("%Y-%m-%dT%H:%M:%S"), micros)
    }
}

/// `datetime.fromisoformat()` for the forms Ghost writes (`+00:00`, `Z`, or naive).
pub fn parse_iso(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
        .ok()
        .map(|n| n.and_utc())
}

/// pydantic's JSON form: RFC 3339 with `Z`, microseconds when they are not zero.
pub mod json {
    use chrono::{DateTime, SecondsFormat, Utc};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(dt: &DateTime<Utc>, s: S) -> Result<S::Ok, S::Error> {
        let fmt = if dt.timestamp_subsec_micros() == 0 {
            SecondsFormat::Secs
        } else {
            SecondsFormat::Micros
        };
        dt.to_rfc3339_opts(fmt, true).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<DateTime<Utc>, D::Error> {
        let raw = String::deserialize(d)?;
        super::parse_iso(&raw)
            .ok_or_else(|| serde::de::Error::custom(format!("not a datetime: {raw}")))
    }

    pub mod option {
        use chrono::{DateTime, Utc};
        use serde::{Deserialize, Deserializer, Serializer};

        pub fn serialize<S: Serializer>(
            dt: &Option<DateTime<Utc>>,
            s: S,
        ) -> Result<S::Ok, S::Error> {
            match dt {
                Some(dt) => super::serialize(dt, s),
                None => s.serialize_none(),
            }
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(
            d: D,
        ) -> Result<Option<DateTime<Utc>>, D::Error> {
            let raw: Option<String> = Option::deserialize(d)?;
            match raw {
                None => Ok(None),
                Some(s) => super::super::parse_iso(&s)
                    .map(Some)
                    .ok_or_else(|| serde::de::Error::custom(format!("not a datetime: {s}"))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn iso_matches_pythons_isoformat() {
        let dt = Utc.with_ymd_and_hms(2026, 10, 28, 20, 40, 26).unwrap();
        assert_eq!(iso(&dt), "2026-10-28T20:40:26+00:00");
        let dt = dt + chrono::Duration::microseconds(93494);
        assert_eq!(iso(&dt), "2026-10-28T20:40:26.093494+00:00");
        assert_eq!(parse_iso(&iso(&dt)), Some(dt));
        assert_eq!(
            parse_iso("2026-10-19T10:00:00Z"),
            Some(Utc.with_ymd_and_hms(2026, 10, 19, 10, 0, 0).unwrap())
        );
        assert_eq!(
            parse_iso("2026-10-19T10:00:00"),
            Some(Utc.with_ymd_and_hms(2026, 10, 19, 10, 0, 0).unwrap())
        );
        assert_eq!(parse_iso("yesterday"), None);
    }
}
