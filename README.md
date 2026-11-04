# crashtrace

Turns the Windows event log into a crash timeline, so repeated unexpected
shutdowns can be looked at as data instead of guesswork.

It reads the System log (optionally Application), groups events per boot,
finds unexpected shutdowns, and reports:

1. **Summary**: unexpected shutdowns, bugchecks with codes, WHEA / GPU-timeout / disk counts.
2. **Crash timeline**: boot time, last activity, uptime before the crash, notable events in the minutes before it, and a pattern label.
3. **Distribution**: crashes by hour of day, day of week, and uptime before the crash (terminal bar charts).
4. **Correlation**: how often a WHEA / GPU / disk / service event precedes a crash, next to how often it shows up in ordinary uptime windows.
5. **Eras**: an optional markers file splits the timeline at dated changes (RAM swap, BIOS update) and prints crashes per 100 uptime hours for each era.

Output is aligned text, `--markdown` or `--json`.

## Usage

```
crashtrace --days 120                       # live System log, last 120 days
crashtrace --days 120 --application         # also read the Application log
crashtrace --since 2031-01-01 --until 2031-03-01 --markdown --out report.md
crashtrace --from exported.evtx --tz utc    # offline, any OS
crashtrace --from dump.xml --markers markers.toml --json
```

| Flag | Meaning |
| --- | --- |
| `--from FILE` | Read an exported `.evtx` or XML file (repeatable) instead of the live log |
| `--application` | Also read the live Application log |
| `--since`, `--until`, `--days` | Time window. Dates are `YYYY-MM-DD`, `YYYY-MM-DDTHH:MM[:SS]` or RFC 3339; a bare `--until` date includes that day |
| `--window MIN` | Correlation window before a crash (default 10) |
| `--lookback MIN`, `--last N` | How far back, and how many notable events, to list per crash (15, 5) |
| `--markers FILE` | Markers TOML (below) |
| `--tz` | `local` (default), `utc`, or an offset like `-08:00`; used for display, hour/weekday bins and marker dates |
| `--json`, `--markdown`, `--out FILE` | Output format and destination |

Offline XML can come from `wevtutil qe System /f:xml /e:Events > dump.xml`.

## Disk errors: `crashtrace disks`

```
crashtrace disks --days 120 --markdown
crashtrace disks --from dump.xml --tz utc
```

Breaks disk and NTFS error events down by event ID, device (for example
`\Device\Harddisk1\DR1`), day, and burst (events less than 10 minutes apart on
one device), and counts how many crashes had a disk event in the lookback
window before them. It takes the same window, `--from`, `--tz`, `--lookback`
and output flags as the main report.

Things to keep in mind:

- One failure can write hundreds of events in seconds. Read the bursts table, not the raw total.
- Device names are assigned per connection. A name in the log may not match any drive present now, especially for USB devices.
- Mapping a device to a physical drive is up to you (for example `Get-Disk` and `Get-PhysicalDisk` in PowerShell, which are read-only). crashtrace does not query drive health.

## Markers

Top-level `date = "label"` pairs. Quote the key if it has a time.

```toml
2031-03-04 = "removed 2 DIMMs"
"2031-02-10T14:30" = "BIOS update"
```

Each marker starts a new era. For each era the report shows crashes, logged
uptime hours and crashes per 100 uptime hours, and flags eras with fewer than
5 crashes or 100 hours.

## Event glossary

| Event | Meaning here |
| --- | --- |
| Kernel-Power 41 | System rebooted without a clean shutdown. Logged at the *next* boot; this is the main crash marker. Its `BugcheckCode`, `SleepInProgress` and `PowerButtonTimestamp` fields are used when present. |
| EventLog 6008 | "The previous system shutdown was unexpected." Corroborates 41. The time it prints is unreliable and is ignored. |
| User32 1074 | Planned restart or shutdown. Never counted as a crash. If a boot logged a 1074 but still ended unexpectedly, the crash is labelled "hung during planned shutdown". |
| BugCheck 1001 | A bluescreen was recorded, with the bugcheck code in `param1`. |
| WHEA-Logger 17, 18, 19, 20, 47 | Hardware errors reported through the Windows Hardware Error Architecture (CPU, memory, PCIe). Corrected ones are common on some machines; fatal ones usually precede a reset. |
| Display 4101 | GPU stopped responding and the driver recovered (TDR). |
| nvlddmkm (any ID) | NVIDIA driver events, grouped with TDRs as "GPU timeout/reset". |
| disk 7, 11, 15, 51, 52, 153, 157; storahci/stornvme 129, 130; Ntfs 55 | Bad block, controller error, not ready, paging error, retried I/O, surprise removal, device reset, file system corruption. |
| Service Control Manager 7031, 7034 | A service terminated unexpectedly. Context only. |
| Kernel-General 12 / 13 | OS start / OS shutdown. Used to split the log into boots. |
| EventLog 6005 / 6006 | Event log service started / stopped. Used as boot and clean-shutdown markers. |

A crash is one boot that ended without a clean shutdown; its 41, 6008 and 1001
events are merged into a single row.

### Pattern labels

In priority order: bluescreen (a bugcheck was recorded), hung during planned
shutdown, died in sleep/resume (`SleepInProgress` set), GPU reset before crash
(a TDR/nvlddmkm event within 2 minutes), lockup with power button used
(`PowerButtonTimestamp` set), hard power loss with no bugcheck.

These describe what the log shows. They are not diagnoses.

## How to read the results

- **Crash time is an estimate**: the last event of any kind logged by the dead boot. A hung machine may have stopped responding earlier or later.
- **Uptime includes sleep**, so it overstates active time on machines that sleep.
- **Correlation compares against a baseline.** If WHEA appears before 30% of crashes but also in 28% of normal 10-minute windows, it says nothing. Only a clear gap is interesting.
- **Era rates need exposure.** Three crashes in 40 hours versus none in 20 hours is noise. Small samples are flagged for that reason.
- **Clusters can be chance.** With a dozen crashes, any hour-of-day chart will look uneven.
- crashtrace never states a cause. A crash with no bugcheck and no preceding hardware event looks the same whatever the trigger (RAM, PSU, drivers, firmware).

## Limitations

- The log only reaches back as far as Windows retains it.
- A crash in the first boot of the window has no time or uptime; widen the window.
- No crash-dump analysis; bugcheck codes are looked up in a small built-in name table.
- WHEA payloads (which component failed) are not decoded.
- Live mode shells out to `wevtutil`; offline mode works on any OS.

## Safety model

Read-only. It runs `wevtutil qe` against the System (and optionally Application)
log, or reads a file you give it. It never writes or clears logs, changes
settings, installs anything, or reboots. No elevation needed for the System log.
Reports can contain real timestamps from your machine; machine and user names
from the log are never printed.

## Development

```
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

All fixtures are hand-written fake events with made-up times. Windows-only code
is gated with `cfg(windows)`; CI runs on Ubuntu and Windows.

## License

MIT
