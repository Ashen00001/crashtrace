use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use chrono::{Duration, Utc};
use clap::Parser;

use crate::markers::parse_markers;
use crate::render::{render, Format};
use crate::report::{build_report, Options};
use crate::source::{decode_text, load_file, query_live};
use crate::tz::{parse_when, Tz};

/// Build a crash timeline from the Windows event log (read-only).
#[derive(Parser, Debug)]
#[command(name = "crashtrace", version, about)]
pub struct Cli {
    /// Read an exported .evtx or XML file instead of the live log (repeatable)
    #[arg(long = "from", value_name = "FILE")]
    pub from: Vec<PathBuf>,

    /// Also read the live Application log (more activity for crash-time estimates)
    #[arg(long)]
    pub application: bool,

    /// Start of the window: YYYY-MM-DD, YYYY-MM-DDTHH:MM[:SS] or RFC 3339
    #[arg(long, conflicts_with = "days")]
    pub since: Option<String>,

    /// End of the window (a bare date includes that whole day)
    #[arg(long)]
    pub until: Option<String>,

    /// Look back this many days from now (or from --until)
    #[arg(long)]
    pub days: Option<u32>,

    /// How far before a crash a WHEA/GPU/disk event counts as "preceding", in minutes
    #[arg(long, default_value_t = 10)]
    pub window: i64,

    /// How far back to list notable events before each crash, in minutes
    #[arg(long, default_value_t = 15)]
    pub lookback: i64,

    /// Maximum notable events listed per crash
    #[arg(long, default_value_t = 5)]
    pub last: usize,

    /// TOML file of dated markers that split the timeline into eras
    #[arg(long, value_name = "FILE")]
    pub markers: Option<PathBuf>,

    /// Time zone for display, hour/weekday bins and marker dates: local, utc or an offset like -08:00
    #[arg(long, default_value = "local")]
    pub tz: String,

    /// Output JSON
    #[arg(long, conflicts_with = "markdown")]
    pub json: bool,

    /// Output Markdown
    #[arg(long)]
    pub markdown: bool,

    /// Write the report to this file instead of stdout
    #[arg(long, value_name = "FILE")]
    pub out: Option<PathBuf>,
}

pub fn execute(cli: &Cli) -> Result<String> {
    if cli.window <= 0 || cli.lookback <= 0 {
        bail!("--window and --lookback must be positive");
    }
    let tz = Tz::parse(&cli.tz)?;
    let now = Utc::now();
    let until = cli
        .until
        .as_deref()
        .map(|s| parse_when(s, tz, true))
        .transpose()?;
    let since = match (&cli.since, cli.days) {
        (Some(s), _) => Some(parse_when(s, tz, false)?),
        (None, Some(d)) => Some(until.unwrap_or(now) - Duration::days(i64::from(d))),
        (None, None) => None,
    };

    let live = cli.from.is_empty();
    let mut events = Vec::new();
    if live {
        events.extend(query_live("System", since, until)?);
        if cli.application {
            events.extend(query_live("Application", since, until)?);
        }
    } else {
        for f in &cli.from {
            events.extend(load_file(f)?);
        }
    }

    let mut opts = Options::new(tz);
    opts.since = since;
    opts.until = until;
    opts.assume_running_until = live.then_some(now);
    opts.correlation_window = Duration::minutes(cli.window);
    opts.lookback = Duration::minutes(cli.lookback);
    opts.last_n = cli.last;
    if let Some(p) = &cli.markers {
        let bytes = std::fs::read(p).with_context(|| format!("reading {}", p.display()))?;
        opts.markers = parse_markers(&decode_text(&bytes), tz)?;
    }

    let report = build_report(events, &opts);
    let format = if cli.json {
        Format::Json
    } else if cli.markdown {
        Format::Markdown
    } else {
        Format::Text
    };
    Ok(render(&report, format))
}

pub fn run(cli: &Cli) -> Result<()> {
    let text = execute(cli)?;
    match &cli.out {
        Some(p) => std::fs::write(p, text).with_context(|| format!("writing {}", p.display()))?,
        None => print!("{text}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;

    fn write_fixture(dir: &std::path::Path) -> PathBuf {
        let mut xml_all = String::from("<Events>");
        for e in scenario() {
            xml_all.push_str(&xml(
                &e.time.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                &e.channel,
                &e.provider,
                e.id,
                &e.data
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_str()))
                    .collect::<Vec<_>>(),
            ));
        }
        xml_all.push_str("</Events>");
        let p = dir.join("scenario.xml");
        std::fs::write(&p, xml_all).unwrap();
        p
    }

    #[test]
    fn end_to_end_from_file() {
        let dir = std::env::temp_dir().join(format!("crashtrace-cli-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = write_fixture(&dir);
        let mk = dir.join("markers.toml");
        std::fs::write(&mk, "\"2031-03-01T12:30\" = \"swapped RAM\"\n").unwrap();
        let cli = Cli::parse_from([
            "crashtrace",
            "--from",
            f.to_str().unwrap(),
            "--tz",
            "utc",
            "--markers",
            mk.to_str().unwrap(),
            "--json",
        ]);
        let out = execute(&cli).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["summary"]["unexpected_shutdowns"], 2);
        assert_eq!(v["eras"].as_array().unwrap().len(), 2);
        let cli = Cli::parse_from([
            "crashtrace",
            "--from",
            f.to_str().unwrap(),
            "--tz",
            "utc",
            "--since",
            "2031-03-01T12:00",
            "--markdown",
        ]);
        let out = execute(&cli).unwrap();
        assert!(out.contains("| 1 |"));
        assert!(!out.contains("| 2 |"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rejects_bad_args() {
        assert!(Cli::try_parse_from(["crashtrace", "--json", "--markdown"]).is_err());
        assert!(
            Cli::try_parse_from(["crashtrace", "--since", "2031-01-01", "--days", "3"]).is_err()
        );
        let cli = Cli::parse_from(["crashtrace", "--from", "x.xml", "--window", "0"]);
        assert!(execute(&cli).is_err());
    }
}
