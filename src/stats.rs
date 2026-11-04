use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, Duration, Timelike, Utc};
use serde::Serialize;

use crate::event::{bugcheck_label, Category, Event, EventKind};
use crate::timeline::{Boot, Crash, EndKind, Timeline};
use crate::tz::Tz;

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub boots: usize,
    pub unexpected_shutdowns: usize,
    pub kernel_power_41: usize,
    pub eventlog_6008: usize,
    pub planned_shutdowns: usize,
    pub bluescreens: usize,
    pub bugcheck_codes: Vec<(String, usize)>,
    pub whea_events: usize,
    pub whea_by_id: BTreeMap<u32, usize>,
    pub tdr_events: usize,
    pub nvidia_events: usize,
    pub disk_events: usize,
    pub service_events: usize,
    pub by_class: Vec<(String, usize)>,
}

pub fn summarize(events: &[Event], tl: &Timeline) -> Summary {
    let count = |k: EventKind| events.iter().filter(|e| e.kind == k).count();
    let mut whea_by_id = BTreeMap::new();
    for e in events.iter().filter(|e| e.kind == EventKind::Whea) {
        *whea_by_id.entry(e.id).or_insert(0) += 1;
    }
    let mut codes: BTreeMap<u32, usize> = BTreeMap::new();
    for c in &tl.crashes {
        if let Some(code) = c.bugcheck_code {
            *codes.entry(code).or_insert(0) += 1;
        }
    }
    let mut bugcheck_codes: Vec<(String, usize)> = codes
        .into_iter()
        .map(|(c, n)| (bugcheck_label(c), n))
        .collect();
    bugcheck_codes.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let mut classes: BTreeMap<&'static str, usize> = BTreeMap::new();
    for c in &tl.crashes {
        *classes.entry(c.class.label()).or_insert(0) += 1;
    }
    let mut by_class: Vec<(String, usize)> = classes
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    by_class.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Summary {
        boots: tl.boots.len(),
        unexpected_shutdowns: tl.crashes.len(),
        kernel_power_41: count(EventKind::KernelPower41),
        eventlog_6008: count(EventKind::Unclean6008),
        planned_shutdowns: tl.planned_shutdowns,
        bluescreens: tl
            .crashes
            .iter()
            .filter(|c| {
                c.bugcheck_code.is_some() || c.class == crate::timeline::CrashClass::Bluescreen
            })
            .count(),
        bugcheck_codes,
        whea_events: count(EventKind::Whea),
        whea_by_id,
        tdr_events: count(EventKind::GpuTimeout),
        nvidia_events: count(EventKind::NvidiaReset),
        disk_events: count(EventKind::DiskError) + count(EventKind::FsError),
        service_events: count(EventKind::ServiceCrash),
        by_class,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Bar {
    pub label: String,
    pub count: usize,
}

pub fn hour_histogram(times: &[DateTime<Utc>], tz: Tz) -> Vec<Bar> {
    let mut b = [0usize; 24];
    for t in times {
        b[tz.to_naive(*t).hour() as usize] += 1;
    }
    b.iter()
        .enumerate()
        .map(|(h, c)| Bar {
            label: format!("{h:02}h"),
            count: *c,
        })
        .collect()
}

pub fn weekday_histogram(times: &[DateTime<Utc>], tz: Tz) -> Vec<Bar> {
    const NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    let mut b = [0usize; 7];
    for t in times {
        b[tz.to_naive(*t).weekday().num_days_from_monday() as usize] += 1;
    }
    NAMES
        .iter()
        .zip(b)
        .map(|(n, c)| Bar {
            label: (*n).to_string(),
            count: c,
        })
        .collect()
}

const UPTIME_EDGES_H: [(i64, &str); 6] = [
    (1, "< 1h"),
    (3, "1-3h"),
    (6, "3-6h"),
    (12, "6-12h"),
    (24, "12-24h"),
    (48, "1-2d"),
];

pub fn uptime_histogram(uptimes: &[Duration]) -> Vec<Bar> {
    let mut counts = vec![0usize; UPTIME_EDGES_H.len() + 1];
    for u in uptimes {
        let idx = UPTIME_EDGES_H
            .iter()
            .position(|(h, _)| *u < Duration::hours(*h))
            .unwrap_or(UPTIME_EDGES_H.len());
        counts[idx] += 1;
    }
    let mut labels: Vec<&str> = UPTIME_EDGES_H.iter().map(|(_, l)| *l).collect();
    labels.push("> 2d");
    labels
        .into_iter()
        .zip(counts)
        .map(|(l, c)| Bar {
            label: l.to_string(),
            count: c,
        })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct CorrelationRow {
    pub category: Category,
    pub label: String,
    pub crashes_with_time: usize,
    pub crashes_preceded: usize,
    pub baseline_windows: usize,
    pub baseline_windows_with_event: usize,
}

impl CorrelationRow {
    pub fn crash_fraction(&self) -> Option<f64> {
        (self.crashes_with_time > 0)
            .then(|| self.crashes_preceded as f64 / self.crashes_with_time as f64)
    }

    pub fn baseline_fraction(&self) -> Option<f64> {
        (self.baseline_windows > 0)
            .then(|| self.baseline_windows_with_event as f64 / self.baseline_windows as f64)
    }
}

/// How often an event of each category falls in the `window` before a crash,
/// next to how often it falls in an equally long window at random, taken from
/// stretches of uptime that did not end in a crash.
pub fn correlate(
    crashes: &[Crash],
    events: &[Event],
    boots: &[Boot],
    window: Duration,
) -> Vec<CorrelationRow> {
    Category::ALL
        .iter()
        .map(|cat| {
            let mut times: Vec<DateTime<Utc>> = events
                .iter()
                .filter(|e| e.kind.category() == Some(*cat))
                .map(|e| e.time)
                .collect();
            times.sort();
            let has_event_in = |from: DateTime<Utc>, to: DateTime<Utc>| {
                let i = times.partition_point(|t| *t < from);
                i < times.len() && times[i] <= to
            };

            let timed: Vec<DateTime<Utc>> = crashes.iter().filter_map(|c| c.crash_time).collect();
            let preceded = timed
                .iter()
                .filter(|c| has_event_in(**c - window, **c))
                .count();

            let mut windows = 0;
            let mut hits = 0;
            if window > Duration::zero() {
                for b in boots {
                    let end = if b.end_kind == EndKind::Crash {
                        b.end - window
                    } else {
                        b.end
                    };
                    let mut s = b.start;
                    while s + window <= end {
                        windows += 1;
                        if has_event_in(s, s + window) {
                            hits += 1;
                        }
                        s += window;
                    }
                }
            }
            CorrelationRow {
                category: *cat,
                label: cat.label().to_string(),
                crashes_with_time: timed.len(),
                crashes_preceded: preceded,
                baseline_windows: windows,
                baseline_windows_with_event: hits,
            }
        })
        .collect()
}

pub fn rate_per_100h(crashes: usize, uptime: Duration) -> Option<f64> {
    let hours = uptime.num_seconds() as f64 / 3600.0;
    (hours > 0.0).then(|| crashes as f64 / hours * 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;
    use crate::timeline::{build, TimelineConfig};

    fn tl() -> (Vec<Event>, Timeline) {
        let ev = scenario();
        let tl = build(&ev, t("03-01 17:00:00"), &TimelineConfig::default());
        (ev, tl)
    }

    #[test]
    fn summary_counts() {
        let (ev, tl) = tl();
        let s = summarize(&ev, &tl);
        assert_eq!(s.boots, 4);
        assert_eq!(s.unexpected_shutdowns, 2);
        assert_eq!(s.kernel_power_41, 2);
        assert_eq!(s.eventlog_6008, 1);
        assert_eq!(s.planned_shutdowns, 1);
        assert_eq!(s.bluescreens, 1);
        assert_eq!(
            s.bugcheck_codes,
            vec![("0x50 PAGE_FAULT_IN_NONPAGED_AREA".to_string(), 1)]
        );
        assert_eq!(s.whea_events, 1);
        assert_eq!(s.whea_by_id[&19], 1);
        assert_eq!(s.tdr_events, 1);
    }

    #[test]
    fn hour_and_weekday_bucketing_respects_zone() {
        // 2031-03-01 is a Saturday.
        let times = [
            t("03-01 03:30:00"),
            t("03-01 03:59:59"),
            t("03-01 23:00:00"),
        ];
        let h = hour_histogram(&times, Tz::Utc);
        assert_eq!(h[3].count, 2);
        assert_eq!(h[23].count, 1);
        assert_eq!(h.len(), 24);
        let w = weekday_histogram(&times, Tz::Utc);
        assert_eq!(w[5].label, "Sat");
        assert_eq!(w[5].count, 3);
        let pst = Tz::parse("-08:00").unwrap();
        let h = hour_histogram(&times, pst);
        assert_eq!(h[19].count, 2); // 03:xx UTC is 19:xx the previous evening
        let w = weekday_histogram(&times, pst);
        assert_eq!(w[4].count, 2); // Friday
        assert_eq!(w[5].count, 1);
    }

    #[test]
    fn uptime_buckets_edges() {
        let ups = [
            Duration::minutes(59),
            Duration::hours(1),
            Duration::minutes(179),
            Duration::hours(6),
            Duration::hours(47),
            Duration::hours(48),
            Duration::hours(500),
        ];
        let h = uptime_histogram(&ups);
        let counts: Vec<usize> = h.iter().map(|b| b.count).collect();
        assert_eq!(counts, vec![1, 2, 0, 1, 0, 1, 2]);
        assert_eq!(h[6].label, "> 2d");
    }

    #[test]
    fn whea_correlation_window() {
        // crash at 14:11 follows a TDR at 14:10; crash at 10:30 has no hardware event nearby.
        let (ev, tl) = tl();
        let rows = correlate(&tl.crashes, &ev, &tl.boots, Duration::minutes(10));
        let gpu = rows.iter().find(|r| r.category == Category::Gpu).unwrap();
        assert_eq!(gpu.crashes_with_time, 2);
        assert_eq!(gpu.crashes_preceded, 1);
        let whea_row = rows.iter().find(|r| r.category == Category::Whea).unwrap();
        assert_eq!(whea_row.crashes_preceded, 0);
        // tighter window than the 1 minute gap excludes the TDR 14:10 -> 14:11? no: 1 min < 10 s is false
        let rows = correlate(&tl.crashes, &ev, &tl.boots, Duration::seconds(30));
        let gpu = rows.iter().find(|r| r.category == Category::Gpu).unwrap();
        assert_eq!(gpu.crashes_preceded, 0);
    }

    #[test]
    fn baseline_excludes_the_crash_adjacent_window() {
        let (ev, tl) = tl();
        let rows = correlate(&tl.crashes, &ev, &tl.boots, Duration::minutes(10));
        let whea_row = rows.iter().find(|r| r.category == Category::Whea).unwrap();
        // boot B (11:00 -> 12:05:10 clean) has 6 full 10-minute windows; WHEA at 11:20 hits the one starting 11:20.
        // boot A (150 min, crashed) loses its last 10 min: 14 windows. boot C (71 min crashed): 6 windows.
        // boot D runs 15:00 -> 17:00: 12 windows.
        assert_eq!(whea_row.baseline_windows, 14 + 6 + 6 + 12);
        assert_eq!(whea_row.baseline_windows_with_event, 2);
        let f = whea_row.baseline_fraction().unwrap();
        assert!(f > 0.0 && f < 0.1);
    }

    #[test]
    fn rate_math() {
        assert_eq!(rate_per_100h(2, Duration::hours(50)), Some(4.0));
        assert_eq!(rate_per_100h(2, Duration::zero()), None);
    }
}
