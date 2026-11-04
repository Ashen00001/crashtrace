use std::fmt::Write as _;

use crate::report::Report;
use crate::stats::Bar;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Text,
    Markdown,
    Json,
}

enum Block {
    Para(String),
    Table(Vec<String>, Vec<Vec<String>>),
    Bars(Vec<Bar>),
    Bullets(Vec<String>),
    Sub(String),
}

struct Section {
    title: String,
    blocks: Vec<Block>,
}

fn pct(v: Option<f64>) -> String {
    v.map_or("n/a".into(), |f| format!("{:.0}%", f * 100.0))
}

fn opt(s: &Option<String>) -> String {
    s.clone().unwrap_or_else(|| "?".into())
}

fn sections(r: &Report) -> Vec<Section> {
    let s = &r.summary;
    let mut out = Vec::new();

    let mut sum_rows = vec![
        vec![
            "Window".into(),
            format!(
                "{} to {} ({})",
                r.meta.window_start, r.meta.window_end, r.meta.time_zone
            ),
        ],
        vec!["Boots seen".into(), s.boots.to_string()],
        vec![
            "Logged uptime".into(),
            format!("{} h", r.meta.total_uptime_hours),
        ],
        vec![
            "Unexpected shutdowns".into(),
            s.unexpected_shutdowns.to_string(),
        ],
        vec![
            "  Kernel-Power 41 / EventLog 6008".into(),
            format!("{} / {}", s.kernel_power_41, s.eventlog_6008),
        ],
        vec![
            "Planned restarts/shutdowns (1074)".into(),
            s.planned_shutdowns.to_string(),
        ],
        vec![
            "Bluescreens (bugcheck recorded)".into(),
            s.bluescreens.to_string(),
        ],
    ];
    for (code, n) in &s.bugcheck_codes {
        sum_rows.push(vec![format!("  {code}"), n.to_string()]);
    }
    let whea_ids = s
        .whea_by_id
        .iter()
        .map(|(id, n)| format!("{id}: {n}"))
        .collect::<Vec<_>>()
        .join(", ");
    sum_rows.push(vec![
        "WHEA events".into(),
        if whea_ids.is_empty() {
            s.whea_events.to_string()
        } else {
            format!("{} ({whea_ids})", s.whea_events)
        },
    ]);
    sum_rows.push(vec![
        "GPU timeouts (Display 4101)".into(),
        s.tdr_events.to_string(),
    ]);
    sum_rows.push(vec!["nvlddmkm events".into(), s.nvidia_events.to_string()]);
    sum_rows.push(vec!["Disk/NTFS errors".into(), s.disk_events.to_string()]);
    sum_rows.push(vec![
        "Service crashes (7031/7034)".into(),
        s.service_events.to_string(),
    ]);
    let mut blocks = vec![Block::Table(
        vec!["Metric".into(), "Value".into()],
        sum_rows,
    )];
    if !s.by_class.is_empty() {
        blocks.push(Block::Sub("Crashes by pattern".into()));
        blocks.push(Block::Table(
            vec!["Pattern".into(), "Crashes".into()],
            s.by_class
                .iter()
                .map(|(k, v)| vec![k.clone(), v.to_string()])
                .collect(),
        ));
    }
    out.push(Section {
        title: "Summary".into(),
        blocks,
    });

    if !r.crashes.is_empty() {
        let rows = r
            .crashes
            .iter()
            .map(|c| {
                vec![
                    c.n.to_string(),
                    opt(&c.boot_time),
                    opt(&c.crash_time),
                    opt(&c.uptime),
                    c.class.clone(),
                    c.bugcheck.clone().unwrap_or_else(|| "-".into()),
                ]
            })
            .collect();
        let mut blocks = vec![Block::Table(
            [
                "#",
                "Boot",
                "Last activity (crash)",
                "Uptime",
                "Pattern",
                "Bugcheck",
            ]
            .map(String::from)
            .to_vec(),
            rows,
        )];
        let mut bullets = Vec::new();
        for c in &r.crashes {
            if c.preceding.is_empty() && c.lockup_seconds.is_none() {
                continue;
            }
            for e in &c.preceding {
                bullets.push(format!(
                    "#{}: -{}s  {}",
                    c.n, e.seconds_before_crash, e.description
                ));
            }
            if let Some(l) = c.lockup_seconds {
                bullets.push(format!(
                    "#{}: power button pressed {}s after last activity",
                    c.n, l
                ));
            }
        }
        if !bullets.is_empty() {
            blocks.push(Block::Sub(format!(
                "Notable events in the {} minutes before each crash",
                r.meta.lookback_minutes
            )));
            blocks.push(Block::Bullets(bullets));
        }
        out.push(Section {
            title: "Crash timeline".into(),
            blocks,
        });
    }

    let d = &r.distribution;
    out.push(Section {
        title: "Distribution".into(),
        blocks: vec![
            Block::Sub("By hour of day".into()),
            Block::Bars(d.by_hour.clone()),
            Block::Sub("By day of week".into()),
            Block::Bars(d.by_weekday.clone()),
            Block::Sub("By uptime before the crash".into()),
            Block::Bars(d.by_uptime.clone()),
        ],
    });

    let rows = r
        .correlation
        .iter()
        .map(|c| {
            vec![
                c.label.clone(),
                format!("{}/{}", c.crashes_preceded, c.crashes_with_time),
                pct(c.crash_fraction()),
                format!("{}/{}", c.baseline_windows_with_event, c.baseline_windows),
                pct(c.baseline_fraction()),
            ]
        })
        .collect();
    out.push(Section {
        title: format!(
            "Correlation ({}-minute window)",
            r.meta.correlation_window_minutes
        ),
        blocks: vec![
            Block::Para(
                "Crashes with such an event in the window before them, next to how often an \
                 equally long window of ordinary uptime contains one. A hit rate close to the \
                 baseline means the event is not unusual before crashes."
                    .into(),
            ),
            Block::Table(
                [
                    "Event type",
                    "Crashes preceded",
                    "%",
                    "Normal windows with it",
                    "%",
                ]
                .map(String::from)
                .to_vec(),
                rows,
            ),
        ],
    });

    if !r.eras.is_empty() {
        let rows = r
            .eras
            .iter()
            .map(|e| {
                vec![
                    e.label.clone(),
                    e.start.clone(),
                    e.end.clone(),
                    e.crashes.to_string(),
                    format!("{}", e.uptime_hours),
                    e.crashes_per_100_uptime_hours
                        .map_or("n/a".into(), |v| format!("{v:.2}")),
                    e.flags.join("; "),
                ]
            })
            .collect();
        out.push(Section {
            title: "Eras (from markers)".into(),
            blocks: vec![Block::Table(
                [
                    "Era",
                    "From",
                    "To",
                    "Crashes",
                    "Uptime h",
                    "Per 100 h",
                    "Caveats",
                ]
                .map(String::from)
                .to_vec(),
                rows,
            )],
        });
    }

    out.push(Section {
        title: "Notes".into(),
        blocks: vec![Block::Bullets(r.notes.clone())],
    });
    out
}

fn widths(headers: &[String], rows: &[Vec<String>]) -> Vec<usize> {
    let mut w: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for r in rows {
        for (i, c) in r.iter().enumerate() {
            w[i] = w[i].max(c.chars().count());
        }
    }
    w
}

fn text_table(headers: &[String], rows: &[Vec<String>]) -> String {
    let w = widths(headers, rows);
    let line = |cells: &[String]| {
        cells
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{c:<width$}", width = w[i]))
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_string()
    };
    let mut s = line(headers);
    s.push('\n');
    s.push_str(
        &w.iter()
            .map(|n| "-".repeat(*n))
            .collect::<Vec<_>>()
            .join("  "),
    );
    s.push('\n');
    for r in rows {
        s.push_str(&line(r));
        s.push('\n');
    }
    s
}

fn md_table(headers: &[String], rows: &[Vec<String>]) -> String {
    let esc = |c: &String| c.replace('|', "\\|");
    let mut s = format!(
        "| {} |\n",
        headers.iter().map(esc).collect::<Vec<_>>().join(" | ")
    );
    s.push_str(&format!(
        "|{}|\n",
        headers
            .iter()
            .map(|_| " --- ")
            .collect::<Vec<_>>()
            .join("|")
    ));
    for r in rows {
        s.push_str(&format!(
            "| {} |\n",
            r.iter().map(esc).collect::<Vec<_>>().join(" | ")
        ));
    }
    s
}

pub fn bars(bars: &[Bar]) -> String {
    const WIDTH: usize = 30;
    let max = bars.iter().map(|b| b.count).max().unwrap_or(0);
    let lw = bars
        .iter()
        .map(|b| b.label.chars().count())
        .max()
        .unwrap_or(0);
    let mut s = String::new();
    for b in bars {
        let len = if max == 0 {
            0
        } else {
            (b.count * WIDTH).div_ceil(max)
        };
        let _ = writeln!(
            s,
            "{:<lw$} | {:<WIDTH$} {}",
            b.label,
            "#".repeat(len),
            b.count
        );
    }
    s
}

pub fn render(report: &Report, format: Format) -> String {
    match format {
        Format::Json => {
            let mut s = serde_json::to_string_pretty(report).expect("report serializes");
            s.push('\n');
            s
        }
        Format::Text => render_with(report, false),
        Format::Markdown => render_with(report, true),
    }
}

fn render_with(report: &Report, md: bool) -> String {
    let mut s = String::new();
    if md {
        s.push_str("# Crash report\n\n");
    } else {
        s.push_str("CRASHTRACE REPORT\n=================\n\n");
    }
    for sec in sections(report) {
        if md {
            let _ = writeln!(s, "## {}\n", sec.title);
        } else {
            let _ = writeln!(
                s,
                "{}\n{}",
                sec.title.to_uppercase(),
                "-".repeat(sec.title.len())
            );
        }
        for b in &sec.blocks {
            match b {
                Block::Para(p) => {
                    let _ = writeln!(s, "{p}\n");
                }
                Block::Sub(t) => {
                    if md {
                        let _ = writeln!(s, "### {t}\n");
                    } else {
                        let _ = writeln!(s, "{t}:");
                    }
                }
                Block::Table(h, rows) => {
                    if md {
                        s.push_str(&md_table(h, rows));
                    } else {
                        s.push_str(&text_table(h, rows));
                    }
                    s.push('\n');
                }
                Block::Bars(b) => {
                    if md {
                        let _ = writeln!(s, "```\n{}```\n", bars(b));
                    } else {
                        let _ = writeln!(s, "{}", bars(b));
                    }
                }
                Block::Bullets(items) => {
                    for i in items {
                        let _ = writeln!(s, "- {i}");
                    }
                    s.push('\n');
                }
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markers::Marker;
    use crate::report::{build_report, Options};
    use crate::testutil::*;
    use crate::tz::Tz;

    fn report() -> Report {
        let mut o = Options::new(Tz::Utc);
        o.assume_running_until = Some(t("03-01 17:00:00"));
        o.markers = vec![Marker {
            at: t("03-01 12:30:00"),
            label: "removed 2 DIMMs".into(),
        }];
        build_report(scenario(), &o)
    }

    #[test]
    fn json_round_trips_key_fields() {
        let v: serde_json::Value = serde_json::from_str(&render(&report(), Format::Json)).unwrap();
        assert_eq!(v["summary"]["unexpected_shutdowns"], 2);
        assert_eq!(
            v["crashes"][1]["bugcheck"],
            "0x50 PAGE_FAULT_IN_NONPAGED_AREA"
        );
        assert_eq!(v["crashes"][0]["class"], "hard power loss, no bugcheck");
        assert_eq!(v["eras"].as_array().unwrap().len(), 2);
        assert_eq!(v["distribution"]["by_hour"].as_array().unwrap().len(), 24);
        assert!(!render(&report(), Format::Json).contains("TESTBOX"));
    }

    #[test]
    fn markdown_has_tables_and_fenced_charts() {
        let md = render(&report(), Format::Markdown);
        assert!(md.starts_with("# Crash report"));
        assert!(md.contains("## Summary"));
        assert!(md.contains("| --- | --- |"));
        assert!(md.contains("| 1 | 2031-03-01 08:00:00 | 2031-03-01 10:30:00 | 2h 30m |"));
        assert!(md.contains("```\n00h"));
        assert!(md.contains("## Eras (from markers)"));
        assert!(md.contains("after: removed 2 DIMMs"));
    }

    #[test]
    fn text_tables_are_aligned_and_charts_scale() {
        let txt = render(&report(), Format::Text);
        let header = txt.lines().find(|l| l.starts_with("#  Boot")).unwrap();
        let rule = txt
            .lines()
            .skip_while(|l| !l.starts_with("#  Boot"))
            .nth(1)
            .unwrap();
        assert!(rule.starts_with("-  ----"));
        assert!(header.contains("Last activity (crash)"));
        let b = bars(&[
            Bar {
                label: "a".into(),
                count: 10,
            },
            Bar {
                label: "bb".into(),
                count: 5,
            },
            Bar {
                label: "c".into(),
                count: 0,
            },
        ]);
        let lines: Vec<&str> = b.lines().collect();
        assert!(lines[0].contains(&"#".repeat(30)));
        assert!(lines[1].contains(&"#".repeat(15)) && !lines[1].contains(&"#".repeat(16)));
        assert!(!lines[2].contains('#'));
    }

    #[test]
    fn empty_report_renders() {
        let r = build_report(Vec::new(), &Options::new(Tz::Utc));
        let txt = render(&r, Format::Text);
        assert!(txt.contains("No unexpected shutdowns"));
        assert!(render(&r, Format::Markdown).contains("# Crash report"));
    }
}
