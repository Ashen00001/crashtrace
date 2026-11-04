use chrono::{DateTime, Duration, Utc};
use serde::Serialize;

use crate::event::{bugcheck_label, Category, Event, EventKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EndKind {
    Crash,
    Clean,
    Running,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct Boot {
    pub index: usize,
    pub start: DateTime<Utc>,
    /// False for the partial boot that was already running when the window began.
    pub start_known: bool,
    pub end: DateTime<Utc>,
    pub end_kind: EndKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CrashClass {
    Bluescreen,
    ShutdownHang,
    SleepTransition,
    GpuReset,
    PowerButtonHold,
    HardPowerLoss,
}

impl CrashClass {
    pub fn label(self) -> &'static str {
        match self {
            CrashClass::Bluescreen => "bluescreen",
            CrashClass::ShutdownHang => "hung during planned shutdown",
            CrashClass::SleepTransition => "died in sleep/resume",
            CrashClass::GpuReset => "GPU reset before crash",
            CrashClass::PowerButtonHold => "lockup, power button used",
            CrashClass::HardPowerLoss => "hard power loss, no bugcheck",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Crash {
    pub boot_index: Option<usize>,
    pub reported_at: DateTime<Utc>,
    /// Last log activity of the boot that died (an estimate, see README).
    pub crash_time: Option<DateTime<Utc>>,
    pub boot_start: Option<DateTime<Utc>>,
    pub uptime: Option<Duration>,
    pub bugcheck_code: Option<u32>,
    pub class: CrashClass,
    pub power_button_time: Option<DateTime<Utc>>,
    pub lockup: Option<Duration>,
    pub preceding: Vec<Event>,
}

impl Crash {
    pub fn bugcheck(&self) -> Option<String> {
        self.bugcheck_code.map(bugcheck_label)
    }

    /// Time used when binning or assigning to an era.
    pub fn effective_time(&self) -> DateTime<Utc> {
        self.crash_time.unwrap_or(self.reported_at)
    }
}

#[derive(Debug, Clone)]
pub struct TimelineConfig {
    pub lookback: Duration,
    pub last_n: usize,
    pub gpu_window: Duration,
}

impl Default for TimelineConfig {
    fn default() -> Self {
        TimelineConfig {
            lookback: Duration::minutes(15),
            last_n: 5,
            gpu_window: Duration::minutes(2),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Timeline {
    pub boots: Vec<Boot>,
    pub crashes: Vec<Crash>,
    pub planned_shutdowns: usize,
}

struct BootBuild {
    start: DateTime<Utc>,
    known: bool,
    from_kernel_general: bool,
    has_boot_log: bool,
}

/// `events` must already be restricted to the analysis window. `report_end`
/// is where a still-running last boot is considered to end.
pub fn build(events: &[Event], report_end: DateTime<Utc>, cfg: &TimelineConfig) -> Timeline {
    let mut evs: Vec<&Event> = events.iter().collect();
    evs.sort_by_key(|e| (e.time, e.record_id));
    if evs.is_empty() {
        return Timeline::default();
    }

    let mut builds: Vec<BootBuild> = Vec::new();
    for e in &evs {
        match e.kind {
            EventKind::BootStart => builds.push(BootBuild {
                start: e.time,
                known: true,
                from_kernel_general: true,
                has_boot_log: false,
            }),
            EventKind::BootLog => {
                let attach = builds.last_mut().filter(|b| {
                    b.from_kernel_general
                        && !b.has_boot_log
                        && (e.time - b.start) < Duration::minutes(5)
                });
                match attach {
                    Some(b) => b.has_boot_log = true,
                    None => builds.push(BootBuild {
                        start: e.time,
                        known: true,
                        from_kernel_general: false,
                        has_boot_log: true,
                    }),
                }
            }
            _ => {}
        }
    }
    if builds.first().is_none_or(|b| evs[0].time < b.start) {
        builds.insert(
            0,
            BootBuild {
                start: evs[0].time,
                known: false,
                from_kernel_general: false,
                has_boot_log: false,
            },
        );
    }

    let n = builds.len();
    let mut members: Vec<Vec<&Event>> = vec![Vec::new(); n];
    let mut j = 0;
    for e in &evs {
        while j + 1 < n && builds[j + 1].start <= e.time {
            j += 1;
        }
        members[j].push(e);
    }

    let last_activity = |k: usize| -> DateTime<Utc> {
        members[k]
            .last()
            .map_or(builds[k].start, |e| e.time.max(builds[k].start))
    };

    let mut boots: Vec<Boot> = Vec::with_capacity(n);
    for k in 0..n {
        let b = &builds[k];
        let clean_end = members[k]
            .iter()
            .rev()
            .find(|e| e.kind.is_clean_shutdown_marker())
            .or_else(|| {
                members[k]
                    .iter()
                    .rev()
                    .find(|e| e.kind == EventKind::Planned1074)
            })
            .map(|e| e.time);
        let (end, end_kind) = if let Some(t) = clean_end {
            (t, EndKind::Clean)
        } else if k + 1 == n {
            (report_end.max(last_activity(k)), EndKind::Running)
        } else {
            (last_activity(k), EndKind::Unknown)
        };
        boots.push(Boot {
            index: k,
            start: b.start,
            start_known: b.known,
            end: end.max(b.start),
            end_kind,
        });
    }

    let mut crashes = Vec::new();
    for k in 0..n {
        let reports: Vec<&&Event> = members[k]
            .iter()
            .filter(|e| e.kind.is_crash_report())
            .collect();
        if reports.is_empty() {
            continue;
        }
        let reported_at = reports[0].time;
        let kp41 = reports
            .iter()
            .find(|e| e.kind == EventKind::KernelPower41)
            .copied();
        let bugcheck_code = reports
            .iter()
            .find(|e| e.kind == EventKind::BugCheck1001)
            .and_then(|e| e.bugcheck_code())
            .or_else(|| kp41.and_then(|e| e.bugcheck_code()));
        let had_bugcheck =
            bugcheck_code.is_some() || reports.iter().any(|e| e.kind == EventKind::BugCheck1001);

        let prev = k.checked_sub(1);
        let crash_time = prev.map(last_activity);
        let boot_start = prev.and_then(|p| builds[p].known.then_some(builds[p].start));
        let uptime = match (crash_time, boot_start) {
            (Some(c), Some(s)) => Some((c - s).max(Duration::zero())),
            _ => None,
        };
        let power_button_time = kp41
            .and_then(|e| e.power_button_time())
            .filter(|pb| prev.is_some_and(|p| *pb >= builds[p].start) && *pb <= builds[k].start);
        let lockup = match (power_button_time, crash_time) {
            (Some(pb), Some(c)) if pb >= c => Some(pb - c),
            _ => None,
        };

        let prev_members: &[&Event] = prev.map_or(&[], |p| members[p].as_slice());
        let shutdown_requested = prev_members
            .iter()
            .any(|e| e.kind == EventKind::Planned1074);
        let gpu_before = crash_time.is_some_and(|c| {
            prev_members.iter().any(|e| {
                e.kind.category() == Some(Category::Gpu)
                    && e.time <= c
                    && c - e.time <= cfg.gpu_window
            })
        });
        let class = if had_bugcheck {
            CrashClass::Bluescreen
        } else if shutdown_requested {
            CrashClass::ShutdownHang
        } else if kp41.is_some_and(|e| e.sleep_in_progress()) {
            CrashClass::SleepTransition
        } else if gpu_before {
            CrashClass::GpuReset
        } else if power_button_time.is_some() {
            CrashClass::PowerButtonHold
        } else {
            CrashClass::HardPowerLoss
        };

        let preceding = match crash_time {
            Some(c) => {
                let mut v: Vec<Event> = prev_members
                    .iter()
                    .filter(|e| {
                        (e.kind.category().is_some() || e.kind == EventKind::Planned1074)
                            && e.time <= c
                            && c - e.time <= cfg.lookback
                    })
                    .map(|e| (**e).clone())
                    .collect();
                let skip = v.len().saturating_sub(cfg.last_n);
                v.drain(..skip);
                v
            }
            None => Vec::new(),
        };

        if let Some(p) = prev {
            boots[p].end = crash_time.unwrap_or(boots[p].end).max(boots[p].start);
            boots[p].end_kind = EndKind::Crash;
        }

        crashes.push(Crash {
            boot_index: prev,
            reported_at,
            crash_time,
            boot_start,
            uptime,
            bugcheck_code,
            class,
            power_button_time,
            lockup,
            preceding,
        });
    }

    let planned_shutdowns = evs
        .iter()
        .filter(|e| e.kind == EventKind::Planned1074)
        .count();

    Timeline {
        boots,
        crashes,
        planned_shutdowns,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;

    fn run(events: &[Event], end: &str) -> Timeline {
        build(events, t(end), &TimelineConfig::default())
    }

    #[test]
    fn groups_events_per_boot() {
        let tl = run(&scenario(), "03-01 17:00:00");
        assert_eq!(tl.boots.len(), 4);
        assert_eq!(tl.boots[0].start, t("03-01 08:00:00"));
        assert_eq!(tl.boots[1].start, t("03-01 11:00:00"));
        assert!(tl.boots.iter().all(|b| b.start_known));
        assert_eq!(tl.planned_shutdowns, 1);
    }

    #[test]
    fn finds_crashes_and_uptime_across_reboots() {
        let tl = run(&scenario(), "03-01 17:00:00");
        assert_eq!(tl.crashes.len(), 2);
        let a = &tl.crashes[0];
        assert_eq!(a.boot_index, Some(0));
        assert_eq!(a.crash_time, Some(t("03-01 10:30:00")));
        assert_eq!(a.uptime, Some(Duration::minutes(150)));
        assert_eq!(a.class, CrashClass::HardPowerLoss);
        assert!(a.bugcheck_code.is_none());
        let c = &tl.crashes[1];
        assert_eq!(c.crash_time, Some(t("03-01 14:11:00")));
        assert_eq!(c.uptime, Some(Duration::minutes(71)));
        assert_eq!(c.class, CrashClass::Bluescreen);
        assert_eq!(c.bugcheck_code, Some(0x50));
        assert_eq!(c.bugcheck().unwrap(), "0x50 PAGE_FAULT_IN_NONPAGED_AREA");
    }

    #[test]
    fn dedupes_41_6008_1001_into_one_crash() {
        let tl = run(&scenario(), "03-01 17:00:00");
        // boot D carries both a 41 and a 1001 but is one crash
        assert_eq!(
            tl.crashes
                .iter()
                .filter(|c| c.boot_index == Some(2))
                .count(),
            1
        );
    }

    #[test]
    fn planned_shutdown_is_not_a_crash_and_ends_boot_cleanly() {
        let tl = run(&scenario(), "03-01 17:00:00");
        let b = &tl.boots[1];
        assert_eq!(b.end_kind, EndKind::Clean);
        assert_eq!(b.end, t("03-01 12:05:10"));
        assert_eq!(tl.boots[0].end_kind, EndKind::Crash);
        assert_eq!(tl.boots[3].end_kind, EndKind::Running);
        assert_eq!(tl.boots[3].end, t("03-01 17:00:00"));
    }

    #[test]
    fn preceding_events_respect_lookback_and_count() {
        let mut ev = Vec::new();
        ev.extend(boot("03-01 08:00:00"));
        ev.push(whea("03-01 08:50:00", 19)); // outside 15 min lookback
        for m in 40..=55 {
            ev.push(disk(&format!("03-01 09:{m}:00"), 153));
        }
        ev.push(beat("03-01 09:58:00"));
        ev.extend(boot("03-01 10:30:00"));
        ev.push(kp41("03-01 10:30:30", &[]));
        let tl = run(&ev, "03-01 11:00:00");
        let c = &tl.crashes[0];
        assert_eq!(c.crash_time, Some(t("03-01 09:58:00")));
        assert_eq!(c.preceding.len(), 5);
        assert!(c.preceding.iter().all(|e| e.kind == EventKind::DiskError));
        assert_eq!(c.preceding.last().unwrap().time, t("03-01 09:55:00"));
    }

    #[test]
    fn classifies_gpu_sleep_shutdown_and_power_button() {
        let ft = |secs: i64| ((secs + 11_644_473_600) * 10_000_000).to_string();
        let mk = |kind: &str| {
            let mut ev = Vec::new();
            ev.extend(boot("03-01 08:00:00"));
            match kind {
                "gpu" => ev.push(tdr("03-01 09:59:00")),
                "shutdown" => ev.push(planned("03-01 09:00:00")),
                _ => {}
            }
            ev.push(beat("03-01 10:00:00"));
            ev.extend(boot("03-01 10:30:00"));
            let data: Vec<(&str, String)> = match kind {
                "sleep" => vec![("SleepInProgress", "2".to_string())],
                "button" => vec![("PowerButtonTimestamp", ft(t("03-01 10:20:00").timestamp()))],
                _ => vec![],
            };
            let data_ref: Vec<(&str, &str)> = data.iter().map(|(k, v)| (*k, v.as_str())).collect();
            ev.push(kp41("03-01 10:30:30", &data_ref));
            run(&ev, "03-01 11:00:00").crashes.remove(0)
        };
        assert_eq!(mk("gpu").class, CrashClass::GpuReset);
        assert_eq!(mk("sleep").class, CrashClass::SleepTransition);
        assert_eq!(mk("shutdown").class, CrashClass::ShutdownHang);
        let b = mk("button");
        assert_eq!(b.class, CrashClass::PowerButtonHold);
        assert_eq!(b.lockup, Some(Duration::minutes(20)));
        assert_eq!(mk("plain").class, CrashClass::HardPowerLoss);
    }

    #[test]
    fn partial_first_boot_has_unknown_uptime() {
        let mut ev = vec![beat("03-01 08:00:00"), beat("03-01 08:30:00")];
        ev.extend(boot("03-01 09:00:00"));
        ev.push(kp41("03-01 09:00:30", &[]));
        let tl = run(&ev, "03-01 10:00:00");
        assert!(!tl.boots[0].start_known);
        let c = &tl.crashes[0];
        assert_eq!(c.crash_time, Some(t("03-01 08:30:00")));
        assert!(c.uptime.is_none());
    }

    #[test]
    fn crash_in_first_boot_of_window_has_no_time() {
        let mut ev = boot("03-01 09:00:00");
        ev.push(kp41("03-01 09:00:30", &[]));
        let tl = run(&ev, "03-01 10:00:00");
        assert_eq!(tl.crashes.len(), 1);
        assert!(tl.crashes[0].crash_time.is_none());
        assert_eq!(tl.crashes[0].effective_time(), t("03-01 09:00:30"));
    }

    #[test]
    fn empty_input_is_fine() {
        let tl = run(&[], "03-01 10:00:00");
        assert!(tl.boots.is_empty() && tl.crashes.is_empty());
    }
}
