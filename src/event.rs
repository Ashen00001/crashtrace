use std::collections::BTreeMap;

use chrono::{DateTime, TimeZone, Utc};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Whea,
    Gpu,
    Disk,
    Service,
}

impl Category {
    pub const ALL: [Category; 4] = [
        Category::Whea,
        Category::Gpu,
        Category::Disk,
        Category::Service,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Whea => "WHEA hardware error",
            Category::Gpu => "GPU timeout/reset (TDR, nvlddmkm)",
            Category::Disk => "Disk/NTFS error",
            Category::Service => "Service terminated unexpectedly",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    KernelPower41,
    Unclean6008,
    Planned1074,
    BugCheck1001,
    Whea,
    GpuTimeout,
    NvidiaReset,
    DiskError,
    FsError,
    ServiceCrash,
    BootStart,
    BootLog,
    ShutdownLog,
    KernelShutdown,
    Other,
}

impl EventKind {
    pub fn category(self) -> Option<Category> {
        match self {
            EventKind::Whea => Some(Category::Whea),
            EventKind::GpuTimeout | EventKind::NvidiaReset => Some(Category::Gpu),
            EventKind::DiskError | EventKind::FsError => Some(Category::Disk),
            EventKind::ServiceCrash => Some(Category::Service),
            _ => None,
        }
    }

    pub fn is_crash_report(self) -> bool {
        matches!(
            self,
            EventKind::KernelPower41 | EventKind::Unclean6008 | EventKind::BugCheck1001
        )
    }

    pub fn is_clean_shutdown_marker(self) -> bool {
        matches!(self, EventKind::KernelShutdown | EventKind::ShutdownLog)
    }
}

#[derive(Debug, Clone)]
pub struct Event {
    pub time: DateTime<Utc>,
    pub channel: String,
    pub provider: String,
    pub id: u32,
    pub record_id: u64,
    pub kind: EventKind,
    pub data: BTreeMap<String, String>,
}

pub fn classify(provider: &str, id: u32) -> EventKind {
    let p = provider.to_ascii_lowercase();
    match (p.as_str(), id) {
        ("microsoft-windows-kernel-power", 41) => EventKind::KernelPower41,
        ("eventlog", 6005) => EventKind::BootLog,
        ("eventlog", 6006) => EventKind::ShutdownLog,
        ("eventlog", 6008) => EventKind::Unclean6008,
        ("user32", 1074) => EventKind::Planned1074,
        ("microsoft-windows-kernel-general", 12) => EventKind::BootStart,
        ("microsoft-windows-kernel-general", 13) => EventKind::KernelShutdown,
        ("display", 4101) => EventKind::GpuTimeout,
        ("nvlddmkm", _) => EventKind::NvidiaReset,
        ("disk", 7 | 11 | 15 | 51 | 52 | 153 | 157) => EventKind::DiskError,
        ("storahci" | "stornvme" | "storport", 129 | 130 | 153) => EventKind::DiskError,
        ("service control manager", 7031 | 7034) => EventKind::ServiceCrash,
        _ if (p == "bugcheck" || p.contains("wer-systemerrorreporting")) && id == 1001 => {
            EventKind::BugCheck1001
        }
        _ if p.contains("whea-logger") && matches!(id, 17..=20 | 47) => EventKind::Whea,
        _ if p.contains("ntfs") && id == 55 => EventKind::FsError,
        _ => EventKind::Other,
    }
}

fn parse_num(s: &str) -> Option<u64> {
    let s = s.trim();
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(h) => u64::from_str_radix(h, 16).ok(),
        None => s.parse().ok(),
    }
}

const FILETIME_UNIX_DIFF_SECS: u64 = 11_644_473_600;

impl Event {
    pub fn bugcheck_code(&self) -> Option<u32> {
        let code = match self.kind {
            EventKind::BugCheck1001 => {
                let v = self.data.get("param1")?;
                parse_num(v.split_whitespace().next()?)?
            }
            EventKind::KernelPower41 => parse_num(self.data.get("BugcheckCode")?)?,
            _ => return None,
        };
        u32::try_from(code).ok().filter(|c| *c != 0)
    }

    /// Device or volume named by a disk/NTFS-style event, e.g. `\Device\Harddisk1\DR1`.
    pub fn device(&self) -> Option<String> {
        match self.kind {
            EventKind::DiskError => self.data.get("data0").cloned(),
            EventKind::FsError => self
                .data
                .get("DriveName")
                .or_else(|| self.data.get("data0"))
                .cloned(),
            _ => None,
        }
        .filter(|d| !d.is_empty())
    }

    pub fn sleep_in_progress(&self) -> bool {
        self.data
            .get("SleepInProgress")
            .and_then(|v| parse_num(v))
            .is_some_and(|v| v != 0)
    }

    pub fn power_button_time(&self) -> Option<DateTime<Utc>> {
        let ticks = parse_num(self.data.get("PowerButtonTimestamp")?)?;
        let secs = ticks / 10_000_000;
        let unix = secs.checked_sub(FILETIME_UNIX_DIFF_SECS)?;
        Utc.timestamp_opt(i64::try_from(unix).ok()?, 0).single()
    }

    pub fn describe(&self) -> String {
        match self.kind {
            EventKind::KernelPower41 => match self.bugcheck_code() {
                Some(c) => format!(
                    "Kernel-Power 41 (unexpected reboot, bugcheck {})",
                    bugcheck_label(c)
                ),
                None => "Kernel-Power 41 (unexpected reboot)".into(),
            },
            EventKind::Unclean6008 => "EventLog 6008 (previous shutdown unexpected)".into(),
            EventKind::Planned1074 => "User32 1074 (planned restart/shutdown)".into(),
            EventKind::BugCheck1001 => match self.bugcheck_code() {
                Some(c) => format!("BugCheck 1001 ({})", bugcheck_label(c)),
                None => "BugCheck 1001".into(),
            },
            EventKind::Whea => format!("WHEA-Logger {} (hardware error)", self.id),
            EventKind::GpuTimeout => "Display 4101 (GPU timeout, driver recovered)".into(),
            EventKind::NvidiaReset => format!("nvlddmkm {} (NVIDIA driver event)", self.id),
            EventKind::DiskError => format!("disk {} (disk error)", self.id),
            EventKind::FsError => format!("Ntfs {} (file system error)", self.id),
            EventKind::ServiceCrash => {
                format!("Service Control Manager {} (service terminated)", self.id)
            }
            EventKind::BootStart => "Kernel-General 12 (OS start)".into(),
            EventKind::BootLog => "EventLog 6005 (event log started)".into(),
            EventKind::ShutdownLog => "EventLog 6006 (event log stopped, clean)".into(),
            EventKind::KernelShutdown => "Kernel-General 13 (OS shutdown)".into(),
            EventKind::Other => format!("{} {}", self.provider, self.id),
        }
    }
}

pub fn bugcheck_label(code: u32) -> String {
    let name = match code {
        0x0A => "IRQL_NOT_LESS_OR_EQUAL",
        0x19 => "BAD_POOL_HEADER",
        0x1A => "MEMORY_MANAGEMENT",
        0x1E => "KMODE_EXCEPTION_NOT_HANDLED",
        0x3B => "SYSTEM_SERVICE_EXCEPTION",
        0x4E => "PFN_LIST_CORRUPT",
        0x50 => "PAGE_FAULT_IN_NONPAGED_AREA",
        0x7A => "KERNEL_DATA_INPAGE_ERROR",
        0x7B => "INACCESSIBLE_BOOT_DEVICE",
        0x7E => "SYSTEM_THREAD_EXCEPTION_NOT_HANDLED",
        0x9F => "DRIVER_POWER_STATE_FAILURE",
        0xC2 => "BAD_POOL_CALLER",
        0xC5 => "DRIVER_CORRUPTED_EXPOOL",
        0xD1 => "DRIVER_IRQL_NOT_LESS_OR_EQUAL",
        0xEF => "CRITICAL_PROCESS_DIED",
        0xF4 => "CRITICAL_OBJECT_TERMINATION",
        0x101 => "CLOCK_WATCHDOG_TIMEOUT",
        0x109 => "CRITICAL_STRUCTURE_CORRUPTION",
        0x116 => "VIDEO_TDR_ERROR",
        0x117 => "VIDEO_TDR_TIMEOUT_DETECTED",
        0x124 => "WHEA_UNCORRECTABLE_ERROR",
        0x133 => "DPC_WATCHDOG_VIOLATION",
        0x139 => "KERNEL_SECURITY_CHECK_FAILURE",
        0x141 => "VIDEO_ENGINE_TIMEOUT_DETECTED",
        _ => "",
    };
    if name.is_empty() {
        format!("0x{code:X}")
    } else {
        format!("0x{code:X} {name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: EventKind, data: &[(&str, &str)]) -> Event {
        Event {
            time: Utc.with_ymd_and_hms(2031, 3, 1, 0, 0, 0).unwrap(),
            channel: "System".into(),
            provider: "p".into(),
            id: 1,
            record_id: 1,
            kind,
            data: data
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn classifies_known_events() {
        assert_eq!(
            classify("Microsoft-Windows-Kernel-Power", 41),
            EventKind::KernelPower41
        );
        assert_eq!(classify("EventLog", 6008), EventKind::Unclean6008);
        assert_eq!(classify("User32", 1074), EventKind::Planned1074);
        assert_eq!(
            classify("Microsoft-Windows-WHEA-Logger", 47),
            EventKind::Whea
        );
        assert_eq!(
            classify("Microsoft-Windows-WHEA-Logger", 1),
            EventKind::Other
        );
        assert_eq!(classify("Display", 4101), EventKind::GpuTimeout);
        assert_eq!(classify("nvlddmkm", 14), EventKind::NvidiaReset);
        assert_eq!(classify("disk", 153), EventKind::DiskError);
        assert_eq!(classify("Ntfs", 55), EventKind::FsError);
        assert_eq!(
            classify("Service Control Manager", 7034),
            EventKind::ServiceCrash
        );
        assert_eq!(classify("Service Control Manager", 7036), EventKind::Other);
        assert_eq!(classify("BugCheck", 1001), EventKind::BugCheck1001);
    }

    #[test]
    fn parses_bugcheck_codes() {
        let e = ev(
            EventKind::BugCheck1001,
            &[("param1", "0x0000001e (0xffffffffc0000005, 0x1, 0x0, 0x0)")],
        );
        assert_eq!(e.bugcheck_code(), Some(0x1E));
        let e = ev(EventKind::KernelPower41, &[("BugcheckCode", "80")]);
        assert_eq!(e.bugcheck_code(), Some(0x50));
        let e = ev(EventKind::KernelPower41, &[("BugcheckCode", "0")]);
        assert_eq!(e.bugcheck_code(), None);
        assert_eq!(bugcheck_label(0x116), "0x116 VIDEO_TDR_ERROR");
        assert_eq!(bugcheck_label(0xABCD), "0xABCD");
    }

    #[test]
    fn parses_power_button_and_sleep() {
        let ft = (1_900_000_000u64 + FILETIME_UNIX_DIFF_SECS) * 10_000_000;
        let e = ev(
            EventKind::KernelPower41,
            &[
                ("PowerButtonTimestamp", &ft.to_string()),
                ("SleepInProgress", "0x2"),
            ],
        );
        assert_eq!(e.power_button_time().unwrap().timestamp(), 1_900_000_000);
        assert!(e.sleep_in_progress());
        let e = ev(
            EventKind::KernelPower41,
            &[("PowerButtonTimestamp", "0"), ("SleepInProgress", "0")],
        );
        assert!(e.power_button_time().is_none());
        assert!(!e.sleep_in_progress());
    }
}
