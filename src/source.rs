use std::path::Path;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};

use crate::event::Event;
use crate::xml::{parse_events, Parsed};

/// Decode text that may be UTF-8, UTF-8 with BOM, or UTF-16 (PowerShell 5
/// redirection writes UTF-16 LE with a BOM).
pub fn decode_text(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        let le = bytes[0] == 0xFF;
        #[allow(clippy::chunks_exact_to_as_chunks)]
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| {
                if le {
                    u16::from_le_bytes([c[0], c[1]])
                } else {
                    u16::from_be_bytes([c[0], c[1]])
                }
            })
            .collect();
        return String::from_utf16_lossy(&units);
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8_lossy(bytes).into_owned()
}

pub fn parse_xml_bytes(bytes: &[u8]) -> Parsed {
    parse_events(&decode_text(bytes))
}

/// Read events from an exported `.evtx` file or an XML dump
/// (`wevtutil qe System /f:xml > file.xml`).
pub fn load_file(path: &Path) -> Result<Vec<Event>> {
    let is_evtx = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("evtx"));
    let parsed = if is_evtx {
        load_evtx(path)?
    } else {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        parse_xml_bytes(&bytes)
    };
    if parsed.events.is_empty() {
        bail!("no events found in {}", path.display());
    }
    if parsed.skipped > 0 {
        eprintln!(
            "crashtrace: skipped {} malformed event(s) in {}",
            parsed.skipped,
            path.display()
        );
    }
    Ok(parsed.events)
}

fn load_evtx(path: &Path) -> Result<Parsed> {
    let mut parser =
        evtx::EvtxParser::from_path(path).with_context(|| format!("opening {}", path.display()))?;
    let mut xml = String::new();
    let mut skipped = 0;
    for rec in parser.records() {
        match rec {
            Ok(r) => {
                xml.push_str(&r.data);
                xml.push('\n');
            }
            Err(_) => skipped += 1,
        }
    }
    let mut parsed = parse_events(&xml);
    parsed.skipped += skipped;
    Ok(parsed)
}

pub fn build_query(since: Option<DateTime<Utc>>, until: Option<DateTime<Utc>>) -> String {
    let fmt = |t: DateTime<Utc>| t.to_rfc3339_opts(SecondsFormat::Millis, true);
    let mut conds = Vec::new();
    if let Some(s) = since {
        conds.push(format!("@SystemTime>='{}'", fmt(s)));
    }
    if let Some(u) = until {
        conds.push(format!("@SystemTime<='{}'", fmt(u)));
    }
    if conds.is_empty() {
        "*".to_string()
    } else {
        format!("*[System[TimeCreated[{}]]]", conds.join(" and "))
    }
}

/// Read a live log through `wevtutil qe`. Read-only; the System log needs no
/// elevation for a normal interactive user.
#[cfg(windows)]
pub fn query_live(
    channel: &str,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
) -> Result<Vec<Event>> {
    let out = std::process::Command::new("wevtutil")
        .arg("qe")
        .arg(channel)
        .arg("/f:xml")
        .arg("/e:Events")
        .arg(format!("/q:{}", build_query(since, until)))
        .output()
        .context("running wevtutil (is it on PATH?)")?;
    if !out.status.success() {
        bail!(
            "wevtutil failed for the {channel} log: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let parsed = parse_xml_bytes(&out.stdout);
    if parsed.skipped > 0 {
        eprintln!("crashtrace: skipped {} malformed event(s)", parsed.skipped);
    }
    Ok(parsed.events)
}

#[cfg(not(windows))]
pub fn query_live(
    _channel: &str,
    _since: Option<DateTime<Utc>>,
    _until: Option<DateTime<Utc>>,
) -> Result<Vec<Event>> {
    bail!("live event log access is only available on Windows; use --from <file.evtx|file.xml>")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{t, xml};

    #[test]
    fn decodes_utf16_and_bom() {
        let s = "<Event>ok</Event>";
        let mut utf16 = vec![0xFF, 0xFE];
        for u in s.encode_utf16() {
            utf16.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(decode_text(&utf16), s);
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice(s.as_bytes());
        assert_eq!(decode_text(&bom), s);
    }

    #[test]
    fn query_building() {
        assert_eq!(build_query(None, None), "*");
        let q = build_query(Some(t("03-01 00:00:00")), Some(t("03-02 12:30:00")));
        assert_eq!(
            q,
            "*[System[TimeCreated[@SystemTime>='2031-03-01T00:00:00.000Z' and @SystemTime<='2031-03-02T12:30:00.000Z']]]"
        );
    }

    #[test]
    fn loads_xml_file() {
        let dir = std::env::temp_dir().join(format!("crashtrace-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("fake.xml");
        std::fs::write(
            &p,
            xml("2031-03-01T10:00:00Z", "System", "EventLog", 6008, &[]),
        )
        .unwrap();
        let ev = load_file(&p).unwrap();
        assert_eq!(ev.len(), 1);
        assert!(load_file(&dir.join("missing.xml")).is_err());
        std::fs::write(&p, "not xml").unwrap();
        assert!(load_file(&p).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
