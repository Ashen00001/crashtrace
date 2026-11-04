use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use crate::event::{classify, Event, EventKind};

#[derive(Debug, Default)]
pub struct Parsed {
    pub events: Vec<Event>,
    pub skipped: usize,
}

fn find_event_start(xml: &str, from: usize) -> Option<usize> {
    let mut pos = from;
    while let Some(i) = xml[pos..].find("<Event") {
        let at = pos + i;
        match xml.as_bytes().get(at + 6) {
            Some(b' ' | b'>' | b'\t' | b'\r' | b'\n') => return Some(at),
            _ => pos = at + 6,
        }
    }
    None
}

fn segments(xml: &str) -> Vec<&str> {
    const END: &str = "</Event>";
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(start) = find_event_start(xml, pos) {
        match xml[start..].find(END) {
            Some(e) => {
                let end = start + e + END.len();
                out.push(&xml[start..end]);
                pos = end;
            }
            None => break,
        }
    }
    out
}

fn child<'a, 'i>(node: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    node.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

fn parse_one(seg: &str) -> Option<Event> {
    let doc = roxmltree::Document::parse(seg).ok()?;
    let system = child(doc.root_element(), "System")?;
    let provider = child(system, "Provider")?.attribute("Name")?.to_string();
    let id: u32 = child(system, "EventID")?.text()?.trim().parse().ok()?;
    let stamp = child(system, "TimeCreated")?.attribute("SystemTime")?;
    let time: DateTime<Utc> = DateTime::parse_from_rfc3339(stamp)
        .ok()?
        .with_timezone(&Utc);
    let record_id = child(system, "EventRecordID")
        .and_then(|n| n.text())
        .and_then(|t| t.trim().parse().ok())
        .unwrap_or(0);
    let channel = child(system, "Channel")
        .and_then(|n| n.text())
        .unwrap_or("")
        .to_string();
    let kind = classify(&provider, id);
    let mut data = BTreeMap::new();
    if kind != EventKind::Other {
        if let Some(ed) = child(doc.root_element(), "EventData") {
            for (i, d) in ed
                .children()
                .filter(|c| c.is_element() && c.tag_name().name() == "Data")
                .enumerate()
            {
                let key = d
                    .attribute("Name")
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("data{i}"));
                data.insert(key, d.text().unwrap_or("").trim().to_string());
            }
        }
    }
    Some(Event {
        time,
        channel,
        provider,
        id,
        record_id,
        kind,
        data,
    })
}

/// Parse a blob of one or more `<Event>` elements (wevtutil output, an
/// `/e:Events`-wrapped document, or concatenated evtx records).
pub fn parse_events(xml: &str) -> Parsed {
    let mut out = Parsed::default();
    for seg in segments(xml) {
        match parse_one(seg) {
            Some(e) => out.events.push(e),
            None => out.skipped += 1,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::xml;

    #[test]
    fn parses_events_and_data() {
        let blob = format!(
            "<?xml version=\"1.0\"?>\n<Events>\n{}\n{}\n</Events>",
            xml(
                "2031-03-01T10:00:00.1234567Z",
                "System",
                "Microsoft-Windows-Kernel-Power",
                41,
                &[("BugcheckCode", "80"), ("SleepInProgress", "0")]
            ),
            xml(
                "2031-03-01T10:00:05Z",
                "Application",
                "Some App",
                1000,
                &[("x", "y")]
            ),
        );
        let p = parse_events(&blob);
        assert_eq!(p.skipped, 0);
        assert_eq!(p.events.len(), 2);
        let e = &p.events[0];
        assert_eq!(e.kind, EventKind::KernelPower41);
        assert_eq!(e.bugcheck_code(), Some(0x50));
        assert_eq!(e.channel, "System");
        assert_eq!(e.record_id, 1);
        assert!(p.events[1].data.is_empty(), "Other events drop their data");
    }

    #[test]
    fn skips_malformed_and_unnamed_data() {
        let good = "<Event xmlns='http://schemas.microsoft.com/win/2004/08/events/event'><System><Provider Name='User32'/><EventID Qualifiers='32768'>1074</EventID><TimeCreated SystemTime='2031-03-01T10:00:00Z'/><EventRecordID>9</EventRecordID><Channel>System</Channel><Computer>TESTBOX-01</Computer></System><EventData><Data>proc.exe</Data><Data>restart</Data></EventData></Event>";
        let bad = "<Event><System><Provider Name='x'/></System></Event>";
        let p = parse_events(&format!("{bad}{good}"));
        assert_eq!(p.skipped, 1);
        assert_eq!(p.events.len(), 1);
        assert_eq!(p.events[0].data["data1"], "restart");
    }
}
