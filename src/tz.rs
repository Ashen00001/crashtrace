use chrono::{DateTime, Duration, FixedOffset, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TzError {
    #[error("invalid time zone `{0}` (use utc, local, or an offset like -08:00)")]
    BadZone(String),
    #[error("cannot parse date/time `{0}` (use YYYY-MM-DD, YYYY-MM-DDTHH:MM[:SS], or RFC 3339)")]
    BadTime(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tz {
    Utc,
    Fixed(FixedOffset),
    Local,
}

impl Tz {
    pub fn parse(s: &str) -> Result<Tz, TzError> {
        let t = s.trim();
        match t.to_ascii_lowercase().as_str() {
            "utc" | "z" => return Ok(Tz::Utc),
            "local" => return Ok(Tz::Local),
            _ => {}
        }
        let bad = || TzError::BadZone(s.to_string());
        let (sign, rest) = match t.as_bytes().first() {
            Some(b'+') => (1, &t[1..]),
            Some(b'-') => (-1, &t[1..]),
            _ => return Err(bad()),
        };
        let digits: String = rest.chars().filter(|c| *c != ':').collect();
        if digits.is_empty() || digits.len() > 4 || !digits.chars().all(|c| c.is_ascii_digit()) {
            return Err(bad());
        }
        let (h, m) = if digits.len() <= 2 {
            (digits.parse::<i32>().map_err(|_| bad())?, 0)
        } else {
            let split = digits.len() - 2;
            (
                digits[..split].parse::<i32>().map_err(|_| bad())?,
                digits[split..].parse::<i32>().map_err(|_| bad())?,
            )
        };
        if m >= 60 {
            return Err(bad());
        }
        FixedOffset::east_opt(sign * (h * 3600 + m * 60))
            .map(Tz::Fixed)
            .ok_or_else(bad)
    }

    pub fn to_naive(self, t: DateTime<Utc>) -> NaiveDateTime {
        match self {
            Tz::Utc => t.naive_utc(),
            Tz::Fixed(o) => t.with_timezone(&o).naive_local(),
            Tz::Local => t.with_timezone(&Local).naive_local(),
        }
    }

    pub fn from_naive(self, n: NaiveDateTime) -> Option<DateTime<Utc>> {
        match self {
            Tz::Utc => Some(Utc.from_utc_datetime(&n)),
            Tz::Fixed(o) => o
                .from_local_datetime(&n)
                .single()
                .map(|d| d.with_timezone(&Utc)),
            Tz::Local => Local
                .from_local_datetime(&n)
                .earliest()
                .map(|d| d.with_timezone(&Utc)),
        }
    }

    pub fn format(self, t: DateTime<Utc>, fmt: &str) -> String {
        self.to_naive(t).format(fmt).to_string()
    }

    pub fn stamp(self, t: DateTime<Utc>) -> String {
        self.format(t, "%Y-%m-%d %H:%M:%S")
    }

    pub fn label(self) -> String {
        match self {
            Tz::Utc => "UTC".into(),
            Tz::Local => "local".into(),
            Tz::Fixed(o) => o.to_string(),
        }
    }
}

pub fn parse_when(s: &str, tz: Tz, end_of_day: bool) -> Result<DateTime<Utc>, TzError> {
    let t = s.trim();
    let bad = || TzError::BadTime(s.to_string());
    if let Ok(d) = DateTime::parse_from_rfc3339(t) {
        return Ok(d.with_timezone(&Utc));
    }
    for fmt in [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(n) = NaiveDateTime::parse_from_str(t, fmt) {
            return tz.from_naive(n).ok_or_else(bad);
        }
    }
    let day = NaiveDate::parse_from_str(t, "%Y-%m-%d").map_err(|_| bad())?;
    let mut n = day.and_hms_opt(0, 0, 0).ok_or_else(bad)?;
    if end_of_day {
        n += Duration::days(1);
    }
    tz.from_naive(n).ok_or_else(bad)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_zones() {
        assert_eq!(Tz::parse("utc").unwrap(), Tz::Utc);
        assert_eq!(Tz::parse("LOCAL").unwrap(), Tz::Local);
        assert_eq!(
            Tz::parse("-08:00").unwrap(),
            Tz::Fixed(FixedOffset::west_opt(8 * 3600).unwrap())
        );
        assert_eq!(
            Tz::parse("+0530").unwrap(),
            Tz::Fixed(FixedOffset::east_opt(5 * 3600 + 1800).unwrap())
        );
        assert!(Tz::parse("pacific").is_err());
        assert!(Tz::parse("+99:99").is_err());
    }

    #[test]
    fn parses_times_in_zone() {
        let tz = Tz::parse("-08:00").unwrap();
        let t = parse_when("2031-03-01", tz, false).unwrap();
        assert_eq!(t.to_rfc3339(), "2031-03-01T08:00:00+00:00");
        let t = parse_when("2031-03-01", tz, true).unwrap();
        assert_eq!(t.to_rfc3339(), "2031-03-02T08:00:00+00:00");
        let t = parse_when("2031-03-01T14:30", Tz::Utc, false).unwrap();
        assert_eq!(t.to_rfc3339(), "2031-03-01T14:30:00+00:00");
        assert!(parse_when("tomorrow", Tz::Utc, false).is_err());
    }
}
