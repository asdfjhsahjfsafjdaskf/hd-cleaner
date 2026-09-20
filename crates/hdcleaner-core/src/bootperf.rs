//! Startup impact as measured by Windows itself.
//!
//! The `Microsoft-Windows-Diagnostics-Performance/Operational` log records
//! every boot (event 100: total, main-path and post-boot time) and each
//! application (101), driver (102) or service (103) that made a boot slower
//! than usual, with the delay it caused. Reading it needs administrator
//! rights; nothing here is estimated — an entry without events simply has no
//! recorded delay.

use crate::{AppError, Result};
use serde::{Deserialize, Serialize};

const CHANNEL: &str = "Microsoft-Windows-Diagnostics-Performance/Operational";
const QUERY: &str = "*[System[(EventID=100 or EventID=101 or EventID=102 or EventID=103)]]";
const MAX_EVENTS: usize = 4000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Boot {
    pub time_ms: i64,
    pub boot_ms: u64,
    pub main_path_ms: u64,
    pub post_boot_ms: u64,
    pub startup_apps: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Slowdown {
    /// "app" | "driver" | "service"
    pub kind: String,
    pub name: String,
    pub path: String,
    /// Number of boots in which it was recorded.
    pub count: u32,
    pub avg_delay_ms: u64,
    pub max_delay_ms: u64,
    pub last_ms: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BootReport {
    /// Most recent first.
    pub boots: Vec<Boot>,
    /// Sorted by average delay, largest first.
    pub slowdowns: Vec<Slowdown>,
}

impl BootReport {
    /// Every recorded slowdown that belongs to a startup entry (see [`Self::for_startup`]).
    pub fn matching(&self, exe: &str, command: &str) -> Vec<&Slowdown> {
        let exe_l = exe.to_lowercase();
        let Some(root) = exe_l.rsplit_once('\\').map(|(d, _)| format!("{d}\\")) else { return Vec::new() };
        let mut names = vec![exe_l.rsplit('\\').next().unwrap_or("").to_string()];
        let cmd = command.to_lowercase();
        if let Some(i) = cmd.find("--processstart") {
            if let Some(n) = cmd[i + "--processstart".len()..].split_whitespace().next() {
                names.push(n.trim_matches('"').to_string());
            }
        }
        self.slowdowns
            .iter()
            .filter(|s| {
                let p = s.path.to_lowercase();
                let file = p.rsplit('\\').next().unwrap_or("");
                p == exe_l || (p.starts_with(&root) && names.iter().any(|n| n == file))
            })
            .collect()
    }

    /// The recorded slowdown of an executable (case-insensitive path).
    pub fn for_exe(&self, exe: &str) -> Option<&Slowdown> {
        self.slowdowns.iter().filter(|s| s.path.eq_ignore_ascii_case(exe)).max_by_key(|s| s.avg_delay_ms)
    }

    /// Delay recorded for a startup entry, all versions combined. Besides the
    /// exact executable, launchers are followed: an entry like
    /// `…\Discord\Update.exe --processStart Discord.exe` matches the
    /// `…\Discord\app-1.0.x\Discord.exe` that Windows actually measured.
    pub fn for_startup(&self, exe: &str, command: &str) -> Option<Slowdown> {
        let hits = self.matching(exe, command);
        let last = *hits.iter().max_by_key(|s| s.last_ms)?;
        let count: u32 = hits.iter().map(|s| s.count).sum();
        let total: u64 = hits.iter().map(|s| s.avg_delay_ms * s.count as u64).sum();
        Some(Slowdown {
            kind: last.kind.clone(),
            name: last.name.clone(),
            path: last.path.clone(),
            count,
            avg_delay_ms: total / count.max(1) as u64,
            max_delay_ms: hits.iter().map(|s| s.max_delay_ms).max().unwrap_or(0),
            last_ms: last.last_ms,
        })
    }
}

// ---------------------------------------------------------------------------
// Event XML (only the few fields used; values are XML-escaped)
// ---------------------------------------------------------------------------

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

fn tag_value<'a>(xml: &'a str, open: &str) -> Option<&'a str> {
    let start = xml.find(open)? + open.len();
    let rest = &xml[start..];
    let gt = rest.find('>')?;
    let body = &rest[gt + 1..];
    Some(&body[..body.find('<')?])
}

fn event_id(xml: &str) -> Option<u32> {
    tag_value(xml, "<EventID")?.trim().parse().ok()
}

fn system_time(xml: &str) -> Option<i64> {
    let i = xml.find("SystemTime=")? + "SystemTime=".len();
    let q = xml.as_bytes().get(i).copied()? as char;
    let rest = &xml[i + 1..];
    parse_iso_ms(&rest[..rest.find(q)?])
}

fn data(xml: &str, name: &str) -> Option<String> {
    for q in ['\'', '"'] {
        let open = format!("<Data Name={q}{name}{q}>");
        if let Some(i) = xml.find(&open) {
            let rest = &xml[i + open.len()..];
            return Some(unescape(&rest[..rest.find("</Data>")?]));
        }
    }
    None
}

/// `2026-09-19T13:56:22.1234567Z` → Unix milliseconds.
pub fn parse_iso_ms(s: &str) -> Option<i64> {
    let (date, time) = s.trim_end_matches('Z').split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>());
    let (y, m, dd) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    let (hms, frac) = time.split_once('.').unwrap_or((time, "0"));
    let mut t = hms.split(':').map(|p| p.parse::<i64>());
    let (hh, mi, ss) = (t.next()?.ok()?, t.next()?.ok()?, t.next()?.ok()?);
    let ms: i64 = format!("{:0<3}", &frac[..frac.len().min(3)]).parse().ok()?;
    // Days from civil (Howard Hinnant).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + dd - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 24 + hh) * 60 + mi) * 60_000 + ss * 1000 + ms)
}

/// Build the report from rendered event XML strings.
pub fn report_from_xml<'a>(events: impl IntoIterator<Item = &'a str>) -> BootReport {
    use std::collections::HashMap;
    let mut boots = Vec::new();
    let mut acc: HashMap<(String, String), (Slowdown, u64)> = HashMap::new();
    let num = |x: &str, n: &str| data(x, n).and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(0);
    for xml in events {
        let Some(id) = event_id(xml) else { continue };
        let when = system_time(xml).unwrap_or(0);
        match id {
            100 => boots.push(Boot {
                time_ms: when,
                boot_ms: num(xml, "BootTime"),
                main_path_ms: num(xml, "MainPathBootTime"),
                post_boot_ms: num(xml, "BootPostBootTime"),
                startup_apps: num(xml, "BootNumStartupApps") as u32,
            }),
            101..=103 => {
                let kind = match id {
                    101 => "app",
                    102 => "driver",
                    _ => "service",
                };
                let path = data(xml, "Path").unwrap_or_default();
                let name = data(xml, "FriendlyName").filter(|s| !s.is_empty()).or_else(|| data(xml, "Name")).unwrap_or_default();
                if path.is_empty() && name.is_empty() {
                    continue;
                }
                let delay = num(xml, "DegradationTime");
                let e = acc.entry((kind.to_string(), path.to_lowercase())).or_insert_with(|| {
                    (Slowdown { kind: kind.into(), name: name.clone(), path: path.clone(), count: 0, avg_delay_ms: 0, max_delay_ms: 0, last_ms: 0 }, 0)
                });
                e.0.count += 1;
                e.1 += delay;
                e.0.max_delay_ms = e.0.max_delay_ms.max(delay);
                if when > e.0.last_ms {
                    e.0.last_ms = when;
                    e.0.name = name;
                }
            }
            _ => {}
        }
    }
    let mut slowdowns: Vec<Slowdown> = acc
        .into_values()
        .map(|(mut s, total)| {
            s.avg_delay_ms = total / s.count.max(1) as u64;
            s
        })
        .collect();
    slowdowns.sort_by(|a, b| b.avg_delay_ms.cmp(&a.avg_delay_ms));
    boots.sort_by(|a, b| b.time_ms.cmp(&a.time_ms));
    BootReport { boots, slowdowns }
}

// ---------------------------------------------------------------------------
// Reading the log (administrator rights required)
// ---------------------------------------------------------------------------

/// Read the boot performance log. Fails with AccessDenied when not elevated.
pub fn read() -> Result<BootReport> {
    use crate::util::wide;
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ACCESS_DENIED, ERROR_INSUFFICIENT_BUFFER, ERROR_NO_MORE_ITEMS};
    use windows_sys::Win32::System::EventLog::*;
    struct H(EVT_HANDLE);
    impl Drop for H {
        fn drop(&mut self) {
            unsafe { EvtClose(self.0) };
        }
    }
    let (ch, q) = (wide(CHANNEL), wide(QUERY));
    let query = unsafe { EvtQuery(0, ch.as_ptr(), q.as_ptr(), EvtQueryChannelPath | EvtQueryReverseDirection) };
    if query == 0 {
        let e = unsafe { GetLastError() };
        return Err(if e == ERROR_ACCESS_DENIED {
            AppError::AccessDenied { path: CHANNEL.into() }
        } else {
            AppError::from_win32(e, "opening the boot performance log", Some(CHANNEL))
        });
    }
    let query = H(query);
    let mut xmls = Vec::new();
    let mut buf: Vec<u16> = vec![0; 16 * 1024];
    'outer: while xmls.len() < MAX_EVENTS {
        let mut handles: [EVT_HANDLE; 64] = [0; 64];
        let mut got = 0u32;
        if unsafe { EvtNext(query.0, handles.len() as u32, handles.as_mut_ptr(), 5000, 0, &mut got) } == 0 {
            let e = unsafe { GetLastError() };
            if e == ERROR_NO_MORE_ITEMS {
                break;
            }
            return Err(AppError::from_win32(e, "reading the boot performance log", Some(CHANNEL)));
        }
        for &h in &handles[..got as usize] {
            let h = H(h);
            let (mut used, mut props) = (0u32, 0u32);
            let mut ok = unsafe { EvtRender(0, h.0, EvtRenderEventXml, (buf.len() * 2) as u32, buf.as_mut_ptr().cast(), &mut used, &mut props) } != 0;
            if !ok && unsafe { GetLastError() } == ERROR_INSUFFICIENT_BUFFER {
                buf.resize(used as usize / 2 + 1, 0);
                ok = unsafe { EvtRender(0, h.0, EvtRenderEventXml, (buf.len() * 2) as u32, buf.as_mut_ptr().cast(), &mut used, &mut props) } != 0;
            }
            if ok {
                let n = (used as usize / 2).min(buf.len());
                let s = String::from_utf16_lossy(&buf[..n]);
                xmls.push(s.trim_end_matches('\0').to_string());
            }
            if xmls.len() >= MAX_EVENTS {
                break 'outer;
            }
        }
    }
    Ok(report_from_xml(xmls.iter().map(String::as_str)))
}

/// Read directly when elevated, otherwise through the elevated helper.
pub fn read_any() -> Result<BootReport> {
    if crate::system::is_elevated() {
        return read();
    }
    use crate::elevation::{run_elevated_ops, ElevatedOp};
    let r = run_elevated_ops(&[ElevatedOp::ReadBootPerformance])?.into_iter().next().ok_or_else(|| AppError::Helper("no result from helper".into()))?;
    if !r.ok {
        return Err(AppError::Helper(r.message.unwrap_or_default()));
    }
    r.data.and_then(|d| serde_json::from_value(d).ok()).ok_or_else(|| AppError::Corrupt("invalid boot report from helper".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOOT: &str = "<Event><System><EventID>100</EventID><TimeCreated SystemTime='2026-09-19T13:56:22.1234567Z'/></System><EventData>\
        <Data Name='BootTime'>46098</Data><Data Name='MainPathBootTime'>13998</Data><Data Name='BootPostBootTime'>32100</Data><Data Name='BootNumStartupApps'>11</Data></EventData></Event>";

    fn app(when: &str, name: &str, path: &str, delay: u32) -> String {
        format!("<Event><System><EventID Qualifiers=''>101</EventID><TimeCreated SystemTime='{when}'/></System><EventData>\
            <Data Name='Name'>{name}</Data><Data Name='FriendlyName'></Data><Data Name='Path'>{path}</Data>\
            <Data Name='TotalTime'>30000</Data><Data Name='DegradationTime'>{delay}</Data></EventData></Event>")
    }

    #[test]
    fn parses_times() {
        assert_eq!(parse_iso_ms("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(parse_iso_ms("2000-03-01T00:00:01.5Z"), Some(951_868_801_500));
    }

    #[test]
    fn builds_report() {
        let a = app("2026-09-19T13:56:22Z", "Discord.exe", r"C:\Users\x\Discord.exe", 20448);
        let b = app("2026-09-19T03:48:07Z", "Discord.exe", r"C:\Users\x\DISCORD.exe", 29047);
        let c = app("2026-09-19T13:56:22Z", "Spotify &amp; Co.exe", r"C:\Users\x\Spotify.exe", 8172);
        let r = report_from_xml([BOOT, a.as_str(), b.as_str(), c.as_str()]);
        assert_eq!(r.boots.len(), 1);
        assert_eq!(r.boots[0].boot_ms, 46098);
        assert_eq!(r.boots[0].startup_apps, 11);
        assert_eq!(r.slowdowns.len(), 2);
        let d = r.for_exe(r"c:\users\x\discord.exe").unwrap();
        assert_eq!((d.count, d.avg_delay_ms, d.max_delay_ms), (2, 24747, 29047));
        assert_eq!(r.slowdowns[0].name, "Discord.exe", "largest delay first");
        assert_eq!(r.slowdowns[1].name, "Spotify & Co.exe", "XML unescaped");
        assert!(r.for_exe(r"C:\other.exe").is_none());
    }

    #[test]
    fn follows_launchers_and_combines_versions() {
        let a = app("2026-09-19T13:56:22Z", "Discord.exe", r"C:\U\Discord\app-1.0.9258\Discord.exe", 20000);
        let b = app("2026-09-18T13:56:22Z", "Discord.exe", r"C:\U\Discord\app-1.0.9257\Discord.exe", 10000);
        let other = app("2026-09-18T13:56:22Z", "Discord.exe", r"C:\Elsewhere\Discord.exe", 50000);
        let r = report_from_xml([a.as_str(), b.as_str(), other.as_str()]);
        let s = r.for_startup(r"C:\U\Discord\Update.exe", r#""C:\U\Discord\Update.exe" --processStart Discord.exe --process-start-args "--start-inactive""#).unwrap();
        assert_eq!((s.count, s.avg_delay_ms, s.max_delay_ms), (2, 15000, 20000));
        assert!(s.path.contains("9258"), "most recent version reported");
        // No launcher argument: only the executable itself.
        assert!(r.for_startup(r"C:\U\Discord\Update.exe", r#""C:\U\Discord\Update.exe""#).is_none());
        assert_eq!(r.for_startup(r"C:\Elsewhere\Discord.exe", "x").unwrap().avg_delay_ms, 50000);
    }

    #[test]
    fn needs_admin_or_reads() {
        // Unelevated: access denied (never a crash); elevated: a report.
        match read() {
            Ok(_) => {}
            Err(e) => assert!(matches!(e, AppError::AccessDenied { .. } | AppError::Win32 { .. } | AppError::NotFound { .. }), "{e:?}"),
        }
    }
}
