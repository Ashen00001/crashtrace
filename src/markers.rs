use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use thiserror::Error;

use crate::timeline::{Boot, Crash};
use crate::tz::{parse_when, Tz, TzError};

#[derive(Debug, Error)]
pub enum MarkerError {
    #[error("markers file is not valid TOML: {0}")]
    Toml(String),
    #[error("marker `{0}` must have a string label")]
    NotString(String),
    #[error(transparent)]
    Time(#[from] TzError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    pub at: DateTime<Utc>,
    pub label: String,
}

/// Format: top-level `date = "label"` pairs, e.g. `2031-03-04 = "removed 2 DIMMs"`.
/// Quote the key to include a time: `"2031-03-04T14:30" = "BIOS update"`.
pub fn parse_markers(text: &str, tz: Tz) -> Result<Vec<Marker>, MarkerError> {
    let table: toml::Table = text
        .parse()
        .map_err(|e: toml::de::Error| MarkerError::Toml(e.message().to_string()))?;
    let mut out = Vec::new();
    for (key, val) in table {
        let label = val
            .as_str()
            .ok_or_else(|| MarkerError::NotString(key.clone()))?
            .to_string();
        out.push(Marker {
            at: parse_when(&key, tz, false)?,
            label,
        });
    }
    out.sort_by_key(|m| m.at);
    Ok(out)
}

#[derive(Debug, Clone, Serialize)]
pub struct Era {
    pub label: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub crashes: usize,
    #[serde(skip)]
    pub uptime: Duration,
}

pub const MIN_CRASHES_FOR_RATE: usize = 5;
pub const MIN_UPTIME_HOURS: i64 = 100;

/// Split `[start, end]` at each marker and count crashes and uptime per era.
/// A marker at or before `start` only names the first era; markers after
/// `end` are ignored.
pub fn build_eras(
    markers: &[Marker],
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    boots: &[Boot],
    crashes: &[Crash],
) -> Vec<Era> {
    let mut sorted = markers.to_vec();
    sorted.sort_by_key(|m| m.at);
    let first_label = sorted
        .iter()
        .rev()
        .find(|m| m.at <= start)
        .map_or_else(|| "baseline".to_string(), |m| format!("after: {}", m.label));
    let mut bounds: Vec<(DateTime<Utc>, String)> = vec![(start, first_label)];
    for m in sorted.iter().filter(|m| m.at > start && m.at < end) {
        bounds.push((m.at, format!("after: {}", m.label)));
    }
    let mut eras = Vec::new();
    for (i, (s, label)) in bounds.iter().enumerate() {
        let e = bounds.get(i + 1).map_or(end, |b| b.0);
        let last = i + 1 == bounds.len();
        let n = crashes
            .iter()
            .filter(|c| {
                let t = c.effective_time();
                t >= *s && (t < e || (last && t <= e))
            })
            .count();
        let uptime = boots
            .iter()
            .map(|b| {
                let from = b.start.max(*s);
                let to = b.end.min(e);
                (to - from).max(Duration::zero())
            })
            .fold(Duration::zero(), |a, d| a + d);
        eras.push(Era {
            label: label.clone(),
            start: *s,
            end: e,
            crashes: n,
            uptime,
        });
    }
    eras
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::rate_per_100h;
    use crate::testutil::*;
    use crate::timeline::{build, TimelineConfig};

    #[test]
    fn parses_marker_file() {
        let text = "2031-03-02 = \"removed 2 DIMMs\"\n\"2031-03-01T14:30\" = \"BIOS update\"\n";
        let m = parse_markers(text, Tz::Utc).unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].label, "BIOS update");
        assert_eq!(m[0].at, t("03-01 14:30:00"));
        assert_eq!(m[1].at, t("03-02 00:00:00"));
    }

    #[test]
    fn rejects_bad_markers() {
        assert!(parse_markers("2031-03-02 = 5", Tz::Utc).is_err());
        assert!(parse_markers("soon = \"x\"", Tz::Utc).is_err());
        assert!(parse_markers("= broken", Tz::Utc).is_err());
    }

    #[test]
    fn eras_split_crashes_and_uptime() {
        let ev = scenario();
        let tl = build(&ev, t("03-01 17:00:00"), &TimelineConfig::default());
        let markers = vec![Marker {
            at: t("03-01 12:30:00"),
            label: "removed 2 DIMMs".into(),
        }];
        let eras = build_eras(
            &markers,
            t("03-01 08:00:00"),
            t("03-01 17:00:00"),
            &tl.boots,
            &tl.crashes,
        );
        assert_eq!(eras.len(), 2);
        assert_eq!(eras[0].label, "baseline");
        assert_eq!(eras[1].label, "after: removed 2 DIMMs");
        assert_eq!(eras[0].crashes, 1); // 10:30
        assert_eq!(eras[1].crashes, 1); // 14:11
                                        // before 12:30: boot A 08:00-10:30 (150m) + boot B 11:00-12:05:10 (65m10s)
        assert_eq!(eras[0].uptime, Duration::seconds(150 * 60 + 65 * 60 + 10));
        // after: boot C 13:00-14:11 (71m) + boot D 15:00-17:00 (120m)
        assert_eq!(eras[1].uptime, Duration::minutes(71 + 120));
        let rate = rate_per_100h(eras[1].crashes, eras[1].uptime).unwrap();
        assert!((rate - 1.0 / (191.0 / 60.0) * 100.0).abs() < 1e-9);
    }

    #[test]
    fn marker_outside_window_only_names_or_is_ignored() {
        let ev = scenario();
        let tl = build(&ev, t("03-01 17:00:00"), &TimelineConfig::default());
        let markers = vec![
            Marker {
                at: t("02-20 00:00:00"),
                label: "old change".into(),
            },
            Marker {
                at: t("03-05 00:00:00"),
                label: "future".into(),
            },
        ];
        let eras = build_eras(
            &markers,
            t("03-01 08:00:00"),
            t("03-01 17:00:00"),
            &tl.boots,
            &tl.crashes,
        );
        assert_eq!(eras.len(), 1);
        assert_eq!(eras[0].label, "after: old change");
        assert_eq!(eras[0].crashes, 2);
    }
}
