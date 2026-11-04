use chrono::{DateTime, Duration, Utc};
use serde::Serialize;

use crate::event::Event;
use crate::markers::{build_eras, Marker, MIN_CRASHES_FOR_RATE, MIN_UPTIME_HOURS};
use crate::stats::{
    correlate, hour_histogram, rate_per_100h, summarize, uptime_histogram, weekday_histogram, Bar,
    CorrelationRow, Summary,
};
use crate::timeline::{build, EndKind, TimelineConfig};
use crate::tz::Tz;

#[derive(Debug, Clone)]
pub struct Options {
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    /// Where a still-running last boot ends (live runs pass "now").
    /// Falls back to the last event seen.
    pub assume_running_until: Option<DateTime<Utc>>,
    pub tz: Tz,
    pub correlation_window: Duration,
    pub lookback: Duration,
    pub last_n: usize,
    pub markers: Vec<Marker>,
}

impl Options {
    pub fn new(tz: Tz) -> Self {
        Options {
            since: None,
            until: None,
            assume_running_until: None,
            tz,
            correlation_window: Duration::minutes(10),
            lookback: Duration::minutes(15),
            last_n: 5,
            markers: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Meta {
    pub tool: String,
    pub version: String,
    pub time_zone: String,
    pub window_start: String,
    pub window_end: String,
    pub events_analyzed: usize,
    pub total_uptime_hours: f64,
    pub correlation_window_minutes: i64,
    pub lookback_minutes: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct EventRow {
    pub seconds_before_crash: i64,
    pub time: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CrashRow {
    pub n: usize,
    pub boot_time: Option<String>,
    pub crash_time: Option<String>,
    pub reported_at: String,
    pub uptime_seconds: Option<i64>,
    pub uptime: Option<String>,
    pub class: String,
    pub bugcheck: Option<String>,
    pub power_button_time: Option<String>,
    pub lockup_seconds: Option<i64>,
    pub preceding: Vec<EventRow>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Distribution {
    pub by_hour: Vec<Bar>,
    pub by_weekday: Vec<Bar>,
    pub by_uptime: Vec<Bar>,
    pub crashes_without_time: usize,
    pub crashes_without_uptime: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct EraRow {
    pub label: String,
    pub start: String,
    pub end: String,
    pub crashes: usize,
    pub uptime_hours: f64,
    pub crashes_per_100_uptime_hours: Option<f64>,
    pub flags: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub meta: Meta,
    pub summary: Summary,
    pub crashes: Vec<CrashRow>,
    pub distribution: Distribution,
    pub correlation: Vec<CorrelationRow>,
    pub eras: Vec<EraRow>,
    pub notes: Vec<String>,
}

pub fn fmt_duration(d: Duration) -> String {
    let s = d.num_seconds().max(0);
    let (days, h, m) = (s / 86_400, (s % 86_400) / 3600, (s % 3600) / 60);
    if days > 0 {
        format!("{days}d {h}h {m:02}m")
    } else if h > 0 {
        format!("{h}h {m:02}m")
    } else if m > 0 {
        format!("{m}m {:02}s", s % 60)
    } else {
        format!("{s}s")
    }
}

pub fn build_report(events: Vec<Event>, opts: &Options) -> Report {
    let mut events: Vec<Event> = events
        .into_iter()
        .filter(|e| {
            opts.since.is_none_or(|s| e.time >= s) && opts.until.is_none_or(|u| e.time <= u)
        })
        .collect();
    events.sort_by_key(|e| (e.time, e.record_id));
    events.dedup_by(|a, b| {
        a.time == b.time && a.provider == b.provider && a.id == b.id && a.record_id == b.record_id
    });

    let first = events.first().map(|e| e.time);
    let last = events.last().map(|e| e.time);
    let window_start_opt = opts.since.or(first);
    let window_end_opt = opts.until.or(opts.assume_running_until).or(last);
    let window_start = window_start_opt.unwrap_or(DateTime::UNIX_EPOCH);
    let window_end = window_end_opt.unwrap_or(window_start);
    let stamp_opt =
        |t: Option<DateTime<Utc>>| t.map_or_else(|| "n/a".to_string(), |t| opts.tz.stamp(t));

    let cfg = TimelineConfig {
        lookback: opts.lookback,
        last_n: opts.last_n,
        ..TimelineConfig::default()
    };
    let tl = build(&events, window_end, &cfg);
    let tz = opts.tz;
    let summary = summarize(&events, &tl);

    let crashes: Vec<CrashRow> = tl
        .crashes
        .iter()
        .enumerate()
        .map(|(i, c)| CrashRow {
            n: i + 1,
            boot_time: c.boot_start.map(|t| tz.stamp(t)),
            crash_time: c.crash_time.map(|t| tz.stamp(t)),
            reported_at: tz.stamp(c.reported_at),
            uptime_seconds: c.uptime.map(|d| d.num_seconds()),
            uptime: c.uptime.map(fmt_duration),
            class: c.class.label().to_string(),
            bugcheck: c.bugcheck(),
            power_button_time: c.power_button_time.map(|t| tz.stamp(t)),
            lockup_seconds: c.lockup.map(|d| d.num_seconds()),
            preceding: c
                .preceding
                .iter()
                .map(|e| EventRow {
                    seconds_before_crash: c.crash_time.map_or(0, |ct| (ct - e.time).num_seconds()),
                    time: tz.stamp(e.time),
                    description: e.describe(),
                })
                .collect(),
        })
        .collect();

    let times: Vec<DateTime<Utc>> = tl.crashes.iter().filter_map(|c| c.crash_time).collect();
    let uptimes: Vec<Duration> = tl.crashes.iter().filter_map(|c| c.uptime).collect();
    let distribution = Distribution {
        by_hour: hour_histogram(&times, tz),
        by_weekday: weekday_histogram(&times, tz),
        by_uptime: uptime_histogram(&uptimes),
        crashes_without_time: tl.crashes.len() - times.len(),
        crashes_without_uptime: tl.crashes.len() - uptimes.len(),
    };
    let correlation = correlate(&tl.crashes, &events, &tl.boots, opts.correlation_window);

    let total_uptime = tl
        .boots
        .iter()
        .fold(Duration::zero(), |a, b| a + (b.end - b.start));

    let eras: Vec<EraRow> = if opts.markers.is_empty() || events.is_empty() {
        Vec::new()
    } else {
        build_eras(
            &opts.markers,
            window_start,
            window_end,
            &tl.boots,
            &tl.crashes,
        )
        .into_iter()
        .map(|e| {
            let mut flags = Vec::new();
            if e.crashes < MIN_CRASHES_FOR_RATE {
                flags.push(format!("small sample: {} crashes", e.crashes));
            }
            if e.uptime < Duration::hours(MIN_UPTIME_HOURS) {
                flags.push(format!(
                    "short exposure: {} h of uptime",
                    e.uptime.num_hours()
                ));
            }
            EraRow {
                label: e.label,
                start: tz.stamp(e.start),
                end: tz.stamp(e.end),
                crashes: e.crashes,
                uptime_hours: round1(e.uptime.num_seconds() as f64 / 3600.0),
                crashes_per_100_uptime_hours: rate_per_100h(e.crashes, e.uptime).map(round2),
                flags,
            }
        })
        .collect()
    };

    let mut notes = Vec::new();
    notes.push(
        "This report shows counts and patterns only. It does not identify a cause: a crash \
         with no bugcheck and no preceding hardware event looks the same whether the trigger \
         was RAM, PSU, power delivery, a driver or a hang."
            .to_string(),
    );
    let n = tl.crashes.len();
    if n == 0 {
        notes.push("No unexpected shutdowns were found in this window.".into());
    } else if n < 10 {
        notes.push(format!(
            "Small sample: only {n} crash(es). Clusters in the charts below can easily be chance."
        ));
    }
    if distribution.crashes_without_time > 0 {
        notes.push(format!(
            "{} crash(es) happened in the first boot of the window, so their time is unknown \
             and they are missing from the distribution charts. Widen the window to include them.",
            distribution.crashes_without_time
        ));
    }
    if distribution.crashes_without_uptime > distribution.crashes_without_time {
        notes.push(
            "Some crashes belong to a boot that started before the window, so their uptime is unknown."
                .into(),
        );
    }
    notes.push(
        "Crash times are the last log activity of the dead boot (the system may have been hung \
         for a while before that), and uptime includes time spent asleep."
            .into(),
    );
    if tl.boots.iter().any(|b| b.end_kind == EndKind::Unknown) {
        notes.push(
            "Some boots ended without a recorded shutdown or crash report; their end is their \
             last logged event."
                .into(),
        );
    }
    if !eras.is_empty() {
        notes.push(
            "Era rates are crashes per 100 hours of logged uptime. Eras with fewer than 5 crashes \
             or 100 hours are flagged; do not read a difference between eras as proof that a change \
             helped or hurt unless the gap is large and the sample is not small."
                .into(),
        );
    }

    Report {
        meta: Meta {
            tool: "crashtrace".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            time_zone: tz.label(),
            window_start: stamp_opt(window_start_opt),
            window_end: stamp_opt(window_end_opt),
            events_analyzed: events.len(),
            total_uptime_hours: round1(total_uptime.num_seconds() as f64 / 3600.0),
            correlation_window_minutes: opts.correlation_window.num_minutes(),
            lookback_minutes: opts.lookback.num_minutes(),
        },
        summary,
        crashes,
        distribution,
        correlation,
        eras,
        notes,
    }
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;

    fn opts() -> Options {
        let mut o = Options::new(Tz::Utc);
        o.assume_running_until = Some(t("03-01 17:00:00"));
        o
    }

    #[test]
    fn builds_full_report() {
        let r = build_report(scenario(), &opts());
        assert_eq!(r.summary.unexpected_shutdowns, 2);
        assert_eq!(r.crashes.len(), 2);
        assert_eq!(r.crashes[0].uptime.as_deref(), Some("2h 30m"));
        assert_eq!(
            r.crashes[1].bugcheck.as_deref(),
            Some("0x50 PAGE_FAULT_IN_NONPAGED_AREA")
        );
        assert_eq!(r.crashes[1].preceding.len(), 1);
        assert_eq!(r.crashes[1].preceding[0].seconds_before_crash, 60);
        assert_eq!(r.distribution.by_hour[10].count, 1);
        assert_eq!(r.distribution.by_hour[14].count, 1);
        assert!(r.eras.is_empty());
        assert!(r.notes.iter().any(|n| n.contains("Small sample")));
        assert_eq!(r.meta.total_uptime_hours, 6.8);
    }

    #[test]
    fn since_until_filter_events() {
        let mut o = opts();
        o.since = Some(t("03-01 12:00:00"));
        o.until = Some(t("03-01 16:30:00"));
        let r = build_report(scenario(), &o);
        assert_eq!(r.summary.unexpected_shutdowns, 1);
        assert_eq!(r.meta.window_start, "2031-03-01 12:00:00");
    }

    #[test]
    fn markers_produce_flagged_eras() {
        let mut o = opts();
        o.markers = vec![Marker {
            at: t("03-01 12:30:00"),
            label: "changed RAM".into(),
        }];
        let r = build_report(scenario(), &o);
        assert_eq!(r.eras.len(), 2);
        assert_eq!(r.eras[1].crashes, 1);
        assert!(r.eras[1].crashes_per_100_uptime_hours.unwrap() > 30.0);
        assert!(r
            .eras
            .iter()
            .all(|e| e.flags.iter().any(|f| f.contains("small sample"))));
    }

    #[test]
    fn duration_formatting() {
        assert_eq!(fmt_duration(Duration::seconds(45)), "45s");
        assert_eq!(fmt_duration(Duration::seconds(125)), "2m 05s");
        assert_eq!(fmt_duration(Duration::minutes(150)), "2h 30m");
        assert_eq!(fmt_duration(Duration::hours(50)), "2d 2h 00m");
    }
}
