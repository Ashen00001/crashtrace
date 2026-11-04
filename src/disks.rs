//! `crashtrace disks`: breakdown of disk and file-system error events.

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;

use crate::event::{Event, EventKind};
use crate::render::{bars, md_table, text_table, Format};
use crate::stats::Bar;
use crate::timeline::{build, TimelineConfig};
use crate::tz::Tz;

/// Events closer together than this belong to the same burst.
const BURST_GAP_MINUTES: i64 = 10;

#[derive(Debug, Clone, Serialize)]
pub struct DeviceRow {
    pub device: String,
    pub count: usize,
    pub first: String,
    pub last: String,
    pub by_id: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BurstRow {
    pub device: String,
    pub start: String,
    pub end: String,
    pub seconds: i64,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct CrashDiskRow {
    pub crash: usize,
    pub crash_time: Option<String>,
    pub disk_events_before: usize,
    pub devices: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiskReport {
    pub time_zone: String,
    pub lookback_minutes: i64,
    pub total: usize,
    pub by_event: Vec<Bar>,
    pub by_device: Vec<DeviceRow>,
    pub by_day: Vec<Bar>,
    pub bursts: Vec<BurstRow>,
    pub crashes: Vec<CrashDiskRow>,
    pub crashes_with_disk_events: usize,
    pub crashes_total: usize,
    pub notes: Vec<String>,
}

fn event_label(e: &Event) -> String {
    let p = match e.provider.to_ascii_lowercase().as_str() {
        s if s.contains("ntfs") => "Ntfs".to_string(),
        _ => e.provider.clone(),
    };
    format!("{p} {}", e.id)
}

pub fn build_disk_report(
    events: &[Event],
    report_end: DateTime<Utc>,
    tz: Tz,
    lookback: Duration,
) -> DiskReport {
    let disk: Vec<&Event> = events
        .iter()
        .filter(|e| matches!(e.kind, EventKind::DiskError | EventKind::FsError))
        .collect();

    let mut by_event: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_day: BTreeMap<String, usize> = BTreeMap::new();
    let mut devices: BTreeMap<String, Vec<&Event>> = BTreeMap::new();
    for e in &disk {
        *by_event.entry(event_label(e)).or_insert(0) += 1;
        *by_day.entry(tz.format(e.time, "%Y-%m-%d")).or_insert(0) += 1;
        devices
            .entry(e.device().unwrap_or_else(|| "(unnamed)".into()))
            .or_default()
            .push(e);
    }

    let mut by_device: Vec<DeviceRow> = devices
        .iter()
        .map(|(dev, evs)| {
            let mut by_id = BTreeMap::new();
            for e in evs {
                *by_id.entry(event_label(e)).or_insert(0) += 1;
            }
            DeviceRow {
                device: dev.clone(),
                count: evs.len(),
                first: tz.stamp(evs.iter().map(|e| e.time).min().expect("non-empty")),
                last: tz.stamp(evs.iter().map(|e| e.time).max().expect("non-empty")),
                by_id,
            }
        })
        .collect();
    by_device.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.device.cmp(&b.device)));

    let mut bursts: Vec<BurstRow> = Vec::new();
    for (dev, evs) in &devices {
        let mut times: Vec<DateTime<Utc>> = evs.iter().map(|e| e.time).collect();
        times.sort();
        let mut i = 0;
        while i < times.len() {
            let mut j = i;
            while j + 1 < times.len()
                && times[j + 1] - times[j] <= Duration::minutes(BURST_GAP_MINUTES)
            {
                j += 1;
            }
            bursts.push(BurstRow {
                device: dev.clone(),
                start: tz.stamp(times[i]),
                end: tz.stamp(times[j]),
                seconds: (times[j] - times[i]).num_seconds(),
                count: j - i + 1,
            });
            i = j + 1;
        }
    }
    bursts.sort_by(|a, b| a.start.cmp(&b.start));

    let cfg = TimelineConfig::default();
    let tl = build(events, report_end, &cfg);
    let crashes: Vec<CrashDiskRow> = tl
        .crashes
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let near: Vec<&&Event> = c.crash_time.map_or_else(Vec::new, |ct| {
                disk.iter()
                    .filter(|e| e.time <= ct && ct - e.time <= lookback)
                    .collect()
            });
            let mut devs: Vec<String> = near
                .iter()
                .map(|e| e.device().unwrap_or_else(|| "(unnamed)".into()))
                .collect();
            devs.sort();
            devs.dedup();
            CrashDiskRow {
                crash: i + 1,
                crash_time: c.crash_time.map(|t| tz.stamp(t)),
                disk_events_before: near.len(),
                devices: devs,
            }
        })
        .collect();
    let crashes_with = crashes.iter().filter(|c| c.disk_events_before > 0).count();

    let mut notes = vec![
        "Device names such as \\Device\\Harddisk7\\DR7 are assigned per connection and can \
         change between boots, so a name seen in the log may not match any drive present now. \
         Match it to a drive using the surrounding events (USB/PnP) and what was plugged in."
            .to_string(),
        "Counts show events, not problems: a single failure can write hundreds of events in \
         seconds. Look at bursts, not raw totals."
            .to_string(),
        "This report names where errors were logged. It does not say why.".to_string(),
    ];
    if disk.is_empty() {
        notes.insert(
            0,
            "No disk or NTFS error events found in this window.".into(),
        );
    }

    DiskReport {
        time_zone: tz.label(),
        lookback_minutes: lookback.num_minutes(),
        total: disk.len(),
        by_event: by_event
            .into_iter()
            .map(|(label, count)| Bar { label, count })
            .collect(),
        by_device,
        by_day: by_day
            .into_iter()
            .map(|(label, count)| Bar { label, count })
            .collect(),
        bursts,
        crashes_with_disk_events: crashes_with,
        crashes_total: crashes.len(),
        crashes,
        notes,
    }
}

pub fn render_disks(r: &DiskReport, format: Format) -> String {
    if format == Format::Json {
        let mut s = serde_json::to_string_pretty(r).expect("report serializes");
        s.push('\n');
        return s;
    }
    let md = format == Format::Markdown;
    let table = |h: &[&str], rows: Vec<Vec<String>>| {
        let h: Vec<String> = h.iter().map(|s| s.to_string()).collect();
        if md {
            md_table(&h, &rows)
        } else {
            text_table(&h, &rows)
        }
    };
    let head = |t: &str| {
        if md {
            format!("## {t}\n\n")
        } else {
            format!("{}\n{}\n", t.to_uppercase(), "-".repeat(t.len()))
        }
    };
    let chart = |b: &[Bar]| {
        if md {
            format!("```\n{}```\n\n", bars(b))
        } else {
            format!("{}\n", bars(b))
        }
    };

    let mut s = String::new();
    s.push_str(if md {
        "# Disk error report\n\n"
    } else {
        "CRASHTRACE DISKS\n================\n\n"
    });
    s.push_str(&format!(
        "{} disk/NTFS error events ({}). Crashes with a disk event in the {} minutes before: {}/{}.\n\n",
        r.total, r.time_zone, r.lookback_minutes, r.crashes_with_disk_events, r.crashes_total
    ));
    if r.total > 0 {
        s.push_str(&head("By event"));
        s.push_str(&chart(&r.by_event));
        s.push_str(&head("By device"));
        s.push_str(&table(
            &["Device", "Events", "First", "Last", "Breakdown"],
            r.by_device
                .iter()
                .map(|d| {
                    vec![
                        d.device.clone(),
                        d.count.to_string(),
                        d.first.clone(),
                        d.last.clone(),
                        d.by_id
                            .iter()
                            .map(|(k, v)| format!("{k}: {v}"))
                            .collect::<Vec<_>>()
                            .join(", "),
                    ]
                })
                .collect(),
        ));
        s.push('\n');
        s.push_str(&head("By day"));
        s.push_str(&chart(&r.by_day));
        s.push_str(&head("Bursts"));
        s.push_str(&table(
            &["Device", "Start", "End", "Seconds", "Events"],
            r.bursts
                .iter()
                .map(|b| {
                    vec![
                        b.device.clone(),
                        b.start.clone(),
                        b.end.clone(),
                        b.seconds.to_string(),
                        b.count.to_string(),
                    ]
                })
                .collect(),
        ));
        s.push('\n');
        let rows: Vec<Vec<String>> = r
            .crashes
            .iter()
            .filter(|c| c.disk_events_before > 0)
            .map(|c| {
                vec![
                    c.crash.to_string(),
                    c.crash_time.clone().unwrap_or_else(|| "?".into()),
                    c.disk_events_before.to_string(),
                    c.devices.join(", "),
                ]
            })
            .collect();
        if !rows.is_empty() {
            s.push_str(&head("Crashes preceded by disk events"));
            s.push_str(&table(&["#", "Last activity", "Events", "Devices"], rows));
            s.push('\n');
        }
    }
    s.push_str(&head("Notes"));
    for n in &r.notes {
        s.push_str(&format!("- {n}\n"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;

    /// Fake: a burst of 51s and 7s on a made-up device just before a crash,
    /// plus one stray error on another device a day earlier.
    fn fixture() -> Vec<Event> {
        let mut v = boot("03-01 08:00:00");
        v.push(disk_on("02-28 12:00:00", 11, "\\Device\\Harddisk9\\DR9"));
        for s in 0..6 {
            v.push(disk_on(
                &format!("03-01 09:58:{:02}", 10 + s),
                51,
                "\\Device\\Harddisk7\\DR7",
            ));
        }
        v.push(disk_on("03-01 09:59:30", 7, "\\Device\\Harddisk7\\DR7"));
        v.push(beat("03-01 09:59:40"));
        v.extend(boot("03-01 10:30:00"));
        v.push(kp41("03-01 10:30:30", &[]));
        v.sort_by_key(|e| e.time);
        v
    }

    fn report() -> DiskReport {
        build_disk_report(
            &fixture(),
            t("03-01 12:00:00"),
            Tz::Utc,
            Duration::minutes(15),
        )
    }

    #[test]
    fn breaks_down_by_event_device_and_day() {
        let r = report();
        assert_eq!(r.total, 8);
        let ev: BTreeMap<&str, usize> = r
            .by_event
            .iter()
            .map(|b| (b.label.as_str(), b.count))
            .collect();
        assert_eq!(ev["disk 51"], 6);
        assert_eq!(ev["disk 7"], 1);
        assert_eq!(ev["disk 11"], 1);
        assert_eq!(r.by_device[0].device, "\\Device\\Harddisk7\\DR7");
        assert_eq!(r.by_device[0].count, 7);
        assert_eq!(r.by_device[0].by_id["disk 51"], 6);
        assert_eq!(r.by_device[1].device, "\\Device\\Harddisk9\\DR9");
        assert_eq!(r.by_day.len(), 2);
        assert_eq!(r.by_day[0].label, "2031-02-28");
    }

    #[test]
    fn groups_bursts() {
        let r = report();
        assert_eq!(r.bursts.len(), 2);
        let big = r.bursts.iter().find(|b| b.count == 7).unwrap();
        assert_eq!(big.seconds, 80);
        assert_eq!(big.device, "\\Device\\Harddisk7\\DR7");
    }

    #[test]
    fn links_disk_events_to_crashes() {
        let r = report();
        assert_eq!(r.crashes_total, 1);
        assert_eq!(r.crashes_with_disk_events, 1);
        assert_eq!(r.crashes[0].disk_events_before, 7);
        assert_eq!(
            r.crashes[0].devices,
            vec!["\\Device\\Harddisk7\\DR7".to_string()]
        );
    }

    #[test]
    fn renders_all_formats() {
        let r = report();
        let txt = render_disks(&r, Format::Text);
        assert!(txt.contains("BY DEVICE"));
        assert!(txt.contains("8 disk/NTFS error events"));
        let md = render_disks(&r, Format::Markdown);
        assert!(md.starts_with("# Disk error report"));
        assert!(md.contains("| Device | Events |"));
        let v: serde_json::Value = serde_json::from_str(&render_disks(&r, Format::Json)).unwrap();
        assert_eq!(v["total"], 8);
        assert!(!txt.contains("TESTBOX"));
    }

    #[test]
    fn empty_is_fine() {
        let r = build_disk_report(&[], t("03-01 12:00:00"), Tz::Utc, Duration::minutes(15));
        assert_eq!(r.total, 0);
        assert!(render_disks(&r, Format::Text).contains("No disk or NTFS error events"));
    }
}
