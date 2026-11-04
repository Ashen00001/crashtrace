//! Fake, hand-written event fixtures. Every name, time and value here is made up.

use chrono::{DateTime, NaiveDateTime, Utc};

use crate::event::Event;
use crate::xml::parse_events;

pub fn t(s: &str) -> DateTime<Utc> {
    let n = NaiveDateTime::parse_from_str(&format!("2031-{s}"), "%Y-%m-%d %H:%M:%S").unwrap();
    DateTime::from_naive_utc_and_offset(n, Utc)
}

pub fn xml(time: &str, channel: &str, provider: &str, id: u32, data: &[(&str, &str)]) -> String {
    let mut d = String::new();
    for (k, v) in data {
        d.push_str(&format!("<Data Name='{k}'>{v}</Data>"));
    }
    format!(
        "<Event xmlns='http://schemas.microsoft.com/win/2004/08/events/event'><System>\
         <Provider Name='{provider}'/><EventID>{id}</EventID>\
         <TimeCreated SystemTime='{time}'/><EventRecordID>1</EventRecordID>\
         <Channel>{channel}</Channel><Computer>TESTBOX-01</Computer></System>\
         <EventData>{d}</EventData></Event>"
    )
}

/// `at` is `MM-DD HH:MM:SS` in the fake year 2031 (UTC).
fn one(at: &str, channel: &str, provider: &str, id: u32, data: &[(&str, &str)]) -> Event {
    let ts = format!("2031-{}Z", at.replacen(' ', "T", 1));
    let mut evs = parse_events(&xml(&ts, channel, provider, id, data)).events;
    assert_eq!(evs.len(), 1);
    evs.remove(0)
}

pub fn boot(at: &str) -> Vec<Event> {
    let later = {
        let n = t(at) + chrono::Duration::seconds(20);
        n.format("%m-%d %H:%M:%S").to_string()
    };
    vec![
        one(at, "System", "Microsoft-Windows-Kernel-General", 12, &[]),
        one(&later, "System", "EventLog", 6005, &[]),
    ]
}

pub fn beat(at: &str) -> Event {
    one(at, "Application", "Some App", 1000, &[])
}

pub fn kp41(at: &str, data: &[(&str, &str)]) -> Event {
    one(at, "System", "Microsoft-Windows-Kernel-Power", 41, data)
}

pub fn e6008(at: &str) -> Event {
    one(at, "System", "EventLog", 6008, &[])
}

pub fn bugcheck(at: &str, param1: &str) -> Event {
    one(at, "System", "BugCheck", 1001, &[("param1", param1)])
}

pub fn planned(at: &str) -> Event {
    one(at, "System", "User32", 1074, &[])
}

pub fn clean_stop(at: &str) -> Vec<Event> {
    vec![
        one(at, "System", "EventLog", 6006, &[]),
        one(at, "System", "Microsoft-Windows-Kernel-General", 13, &[]),
    ]
}

pub fn whea(at: &str, id: u32) -> Event {
    one(at, "System", "Microsoft-Windows-WHEA-Logger", id, &[])
}

pub fn tdr(at: &str) -> Event {
    one(at, "System", "Display", 4101, &[])
}

pub fn disk(at: &str, id: u32) -> Event {
    one(at, "System", "disk", id, &[])
}

#[allow(dead_code)]
pub fn scm(at: &str) -> Event {
    one(at, "System", "Service Control Manager", 7031, &[])
}

/// Four fake boots (all 2031-03):
/// - boot A 03-01 08:00, last activity 10:30 -> hard power loss, uptime 2h30m
/// - boot B 03-01 11:00, WHEA 11:20, clean stop 12:05
/// - boot C 03-01 13:00, TDR 14:10, last activity 14:11 -> bluescreen 0x50, uptime 1h11m
/// - boot D 03-01 15:00, runs until the report ends
pub fn scenario() -> Vec<Event> {
    let mut v = Vec::new();
    v.extend(boot("03-01 08:00:00"));
    v.push(beat("03-01 09:00:00"));
    v.push(beat("03-01 10:30:00"));
    v.extend(boot("03-01 11:00:00"));
    v.push(kp41(
        "03-01 11:00:40",
        &[
            ("BugcheckCode", "0"),
            ("PowerButtonTimestamp", "0"),
            ("SleepInProgress", "0"),
        ],
    ));
    v.push(e6008("03-01 11:00:45"));
    v.push(whea("03-01 11:20:00", 19));
    v.push(planned("03-01 12:05:00"));
    v.extend(clean_stop("03-01 12:05:10"));
    v.extend(boot("03-01 13:00:00"));
    v.push(beat("03-01 14:00:00"));
    v.push(tdr("03-01 14:10:00"));
    v.push(beat("03-01 14:11:00"));
    v.extend(boot("03-01 15:00:00"));
    v.push(kp41(
        "03-01 15:00:40",
        &[
            ("BugcheckCode", "80"),
            ("PowerButtonTimestamp", "0"),
            ("SleepInProgress", "0"),
        ],
    ));
    v.push(bugcheck(
        "03-01 15:02:00",
        "0x00000050 (0xffffd0, 0x0, 0x1, 0x2)",
    ));
    v.push(beat("03-01 16:00:00"));
    v.sort_by_key(|e| e.time);
    v
}

/// Disk-style event whose device is the first unnamed `<Data>` element.
pub fn disk_on(at: &str, id: u32, device: &str) -> Event {
    let ts = format!("2031-{}Z", at.replacen(' ', "T", 1));
    let xml = format!(
        "<Event xmlns='http://schemas.microsoft.com/win/2004/08/events/event'><System>\
         <Provider Name='disk'/><EventID>{id}</EventID><TimeCreated SystemTime='{ts}'/>\
         <EventRecordID>1</EventRecordID><Channel>System</Channel><Computer>TESTBOX-01</Computer>\
         </System><EventData><Data>{device}</Data><Binary>00</Binary></EventData></Event>"
    );
    parse_events(&xml).events.remove(0)
}
