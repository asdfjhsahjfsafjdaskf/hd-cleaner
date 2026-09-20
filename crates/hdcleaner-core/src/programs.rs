//! Installed programs inventory.
//!
//! Sources:
//! * Uninstall keys: `HKLM\…\Uninstall` in the 64-bit and 32-bit
//!   (WOW6432Node) registry views and `HKCU\…\Uninstall`;
//! * AppX / MSIX / Store packages of the current user (see [`crate::appx`]).
//!
//! Registry values are untrusted data: they are only parsed and displayed
//! here. Nothing is executed by this module.

use crate::registry::{expand_env, Hive, Key, View};
use serde::Serialize;

const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ProgramSource {
    /// Classic installer registered in the Uninstall key.
    Win32,
    /// Windows Installer package (MSI product code).
    Msi,
    /// Microsoft Store package.
    Store,
    /// AppX/MSIX package (sideloaded, system or enterprise signed).
    Appx,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Arch {
    X64,
    X86,
    Arm64,
    Neutral,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Scope {
    Machine,
    User,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Program {
    /// Stable identifier: `reg:<hive><view>:<key>` or `appx:<PackageFullName>`.
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub publisher: Option<String>,
    /// `YYYY-MM-DD` when known.
    pub install_date: Option<String>,
    pub install_location: Option<String>,
    /// Folder inferred from the uninstaller / icon path when the installer did
    /// not register `InstallLocation` (lower confidence).
    pub inferred_location: Option<String>,
    pub uninstall_string: Option<String>,
    pub quiet_uninstall_string: Option<String>,
    pub modify_path: Option<String>,
    /// Size declared by the installer (`EstimatedSize`), in bytes.
    pub reported_size: Option<u64>,
    /// `path,index` icon reference (untrusted; only used to extract an icon).
    pub icon: Option<String>,
    pub url: Option<String>,
    pub source: ProgramSource,
    pub arch: Arch,
    pub scope: Scope,
    /// Full registry path of the uninstall entry (display form).
    pub registry_key: Option<String>,
    pub msi_product_code: Option<String>,
    /// Hidden by default: `SystemComponent=1`, updates, child entries.
    pub system_component: bool,
    pub is_update: bool,
    pub no_remove: bool,
    // AppX specific
    pub package_full_name: Option<String>,
    pub package_family_name: Option<String>,
    pub is_framework: bool,
    /// "system" | "store" | "developer" | "enterprise" | "none"
    pub signature_kind: Option<String>,
    pub dependencies: Vec<String>,
}

impl Program {
    pub(crate) fn blank(id: String, name: String, source: ProgramSource) -> Program {
        Program {
            id,
            name,
            version: None,
            publisher: None,
            install_date: None,
            install_location: None,
            inferred_location: None,
            uninstall_string: None,
            quiet_uninstall_string: None,
            modify_path: None,
            reported_size: None,
            icon: None,
            url: None,
            source,
            arch: Arch::Unknown,
            scope: Scope::Machine,
            registry_key: None,
            msi_product_code: None,
            system_component: false,
            is_update: false,
            no_remove: false,
            package_full_name: None,
            package_family_name: None,
            is_framework: false,
            signature_kind: None,
            dependencies: Vec::new(),
        }
    }

    /// Best known folder of the program and whether it came from the
    /// installer's own registration.
    pub fn best_location(&self) -> Option<(&str, bool)> {
        self.install_location
            .as_deref()
            .map(|p| (p, true))
            .or_else(|| self.inferred_location.as_deref().map(|p| (p, false)))
    }
}

fn clean(s: Option<String>) -> Option<String> {
    s.map(|v| v.trim().trim_matches('\0').to_string()).filter(|v| !v.is_empty())
}

/// Normalise a folder value from the registry: quotes, env vars, trailing `\`.
pub fn clean_dir(raw: &str) -> Option<String> {
    let mut s = expand_env(raw.trim().trim_matches('"').trim()).replace('/', "\\");
    while s.ends_with('\\') && s.len() > 3 {
        s.pop();
    }
    let b = s.as_bytes();
    let absolute = (b.len() >= 3 && b[1] == b':' && b[2] == b'\\') || s.starts_with(r"\\");
    (absolute && !s.contains('\0')).then_some(s)
}

/// Extract the executable path from a command line such as
/// `"C:\x\uninst.exe" /S` or `C:\x\uninst.exe --flag`.
pub fn exe_from_command(cmd: &str) -> Option<String> {
    let c = expand_env(cmd.trim());
    let path = if let Some(rest) = c.strip_prefix('"') {
        rest.split('"').next()?.to_string()
    } else {
        let lower = c.to_ascii_lowercase();
        let end = lower.find(".exe").map(|i| i + 4).unwrap_or(c.len());
        c[..end].trim().to_string()
    };
    let lower = path.to_ascii_lowercase();
    if lower.ends_with("msiexec.exe") || lower.ends_with("msiexec") || lower.contains("rundll32") {
        return None;
    }
    clean_dir(&path)
}

/// `"C:\x\app.exe",0` → (`C:\x\app.exe`, 0)
pub fn parse_icon_ref(raw: &str) -> Option<(String, i32)> {
    let s = expand_env(raw.trim());
    let (path, idx) = match s.rfind(',') {
        Some(i) if s[i + 1..].trim().parse::<i32>().is_ok() => (&s[..i], s[i + 1..].trim().parse().unwrap_or(0)),
        _ => (s.as_str(), 0),
    };
    let path = path.trim().trim_matches('"').to_string();
    clean_dir(&path).map(|p| (p, idx))
}

fn parent_dir(path: &str) -> Option<String> {
    let i = path.rfind('\\')?;
    clean_dir(&path[..i])
}

/// Folder inferred from uninstaller or icon, only when it is specific to the
/// program (not a shared container like Program Files, Windows, System32).
fn infer_location(uninstall: Option<&str>, icon: Option<&str>) -> Option<String> {
    let candidates = [uninstall.and_then(exe_from_command), icon.and_then(|i| parse_icon_ref(i).map(|(p, _)| p))];
    for exe in candidates.into_iter().flatten() {
        let Some(dir) = parent_dir(&exe) else { continue };
        let lower = dir.to_lowercase();
        // Installer caches are not the program folder.
        if lower.contains(r"\installer") || lower.contains(r"\package cache") || lower.contains(r"\temp") {
            continue;
        }
        let risk = crate::protection::assess(&dir).risk;
        if risk != crate::protection::Risk::Blocked {
            return Some(dir);
        }
    }
    None
}

fn format_install_date(raw: &str) -> Option<String> {
    let d: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if d.len() == 8 && raw.trim().len() == 8 {
        let (y, m, dd) = (&d[0..4], &d[4..6], &d[6..8]);
        let (yn, mn, dn): (u32, u32, u32) = (y.parse().ok()?, m.parse().ok()?, dd.parse().ok()?);
        if (1990..=2100).contains(&yn) && (1..=12).contains(&mn) && (1..=31).contains(&dn) {
            return Some(format!("{y}-{m}-{dd}"));
        }
    }
    None
}

fn native_arch() -> Arch {
    use windows_sys::Win32::System::SystemInformation::*;
    let mut si: SYSTEM_INFO = unsafe { std::mem::zeroed() };
    unsafe { GetNativeSystemInfo(&mut si) };
    match unsafe { si.Anonymous.Anonymous.wProcessorArchitecture } {
        PROCESSOR_ARCHITECTURE_AMD64 => Arch::X64,
        PROCESSOR_ARCHITECTURE_ARM64 => Arch::Arm64,
        PROCESSOR_ARCHITECTURE_INTEL => Arch::X86,
        _ => Arch::Unknown,
    }
}

fn read_uninstall_root(hive: Hive, view: View, arch: Arch, scope: Scope, out: &mut Vec<Program>) {
    let Some(root) = Key::open(hive, UNINSTALL, view) else { return };
    let view_tag = match view {
        View::Reg32 => "32",
        View::Reg64 => "64",
        View::Default => "",
    };
    for sub in root.subkeys() {
        let Some(k) = root.open_sub(&sub, view) else { continue };
        let Some(name) = clean(k.string("DisplayName")) else { continue };
        let is_msi = k.dword("WindowsInstaller") == Some(1);
        let mut p = Program::blank(
            format!("reg:{}{}:{}", hive.short(), view_tag, sub),
            name,
            if is_msi { ProgramSource::Msi } else { ProgramSource::Win32 },
        );
        p.version = clean(k.string("DisplayVersion"));
        p.publisher = clean(k.string("Publisher"));
        p.install_date = k.string("InstallDate").and_then(|d| format_install_date(&d));
        p.install_location = k.string("InstallLocation").and_then(|d| clean_dir(&d));
        p.uninstall_string = clean(k.string("UninstallString"));
        p.quiet_uninstall_string = clean(k.string("QuietUninstallString"));
        p.modify_path = clean(k.string("ModifyPath"));
        p.reported_size = k.dword("EstimatedSize").map(|kb| kb as u64 * 1024);
        // No DisplayIcon: the uninstaller usually carries the application icon.
        p.icon = clean(k.string("DisplayIcon")).or_else(|| p.uninstall_string.as_deref().and_then(exe_from_command));
        p.url = clean(k.string("URLInfoAbout")).or_else(|| clean(k.string("HelpLink")));
        p.arch = arch;
        p.scope = scope;
        let hive_path = match hive {
            Hive::LocalMachine if view == View::Reg32 => r"HKEY_LOCAL_MACHINE\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
            Hive::LocalMachine => r"HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
            _ => r"HKEY_CURRENT_USER\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        };
        p.registry_key = Some(format!(r"{hive_path}\{sub}"));
        if is_msi && sub.starts_with('{') && sub.ends_with('}') && sub.len() == 38 {
            p.msi_product_code = Some(sub.clone());
        }
        p.system_component = k.dword("SystemComponent") == Some(1);
        let release = k.string("ReleaseType").unwrap_or_default().to_lowercase();
        p.is_update = k.string("ParentKeyName").is_some()
            || release.contains("update")
            || release.contains("hotfix")
            || (sub.starts_with("KB") && sub[2..].chars().take(6).all(|c| c.is_ascii_digit()));
        p.no_remove = k.dword("NoRemove") == Some(1);
        if p.install_location.is_none() {
            p.inferred_location = infer_location(p.uninstall_string.as_deref(), p.icon.as_deref());
        }
        out.push(p);
    }
}

/// All registry-registered programs (machine 64/32-bit and current user).
pub fn registry_programs() -> Vec<Program> {
    let mut out = Vec::new();
    let native = native_arch();
    if native == Arch::X86 {
        read_uninstall_root(Hive::LocalMachine, View::Default, Arch::X86, Scope::Machine, &mut out);
    } else {
        read_uninstall_root(Hive::LocalMachine, View::Reg64, native, Scope::Machine, &mut out);
        read_uninstall_root(Hive::LocalMachine, View::Reg32, Arch::X86, Scope::Machine, &mut out);
    }
    read_uninstall_root(Hive::CurrentUser, View::Default, Arch::Unknown, Scope::User, &mut out);
    out
}

/// Registry + AppX inventory, sorted by name. AppX failures are reported but
/// do not hide the registry programs.
pub fn list_all() -> (Vec<Program>, Option<crate::AppError>) {
    let mut all = registry_programs();
    let appx_err = match crate::appx::packages() {
        Ok(mut pk) => {
            all.append(&mut pk);
            None
        }
        Err(e) => Some(e),
    };
    all.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.id.cmp(&b.id)));
    (all, appx_err)
}

/// Programs whose folder contains `path` (for "Identify installed program").
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Identification {
    pub program_id: String,
    pub name: String,
    pub confidence: crate::appsize::Confidence,
    pub reason: &'static str,
    pub location: String,
}

pub fn identify(programs: &[Program], path: &str) -> Vec<Identification> {
    let p = path.trim_end_matches('\\').to_lowercase();
    let mut out: Vec<Identification> = Vec::new();
    let within = |dir: &str| {
        let d = dir.trim_end_matches('\\').to_lowercase();
        p == d || (p.starts_with(&d) && p.as_bytes().get(d.len()) == Some(&b'\\'))
    };
    for prog in programs {
        if let Some(loc) = &prog.install_location {
            if within(loc) {
                out.push(Identification {
                    program_id: prog.id.clone(),
                    name: prog.name.clone(),
                    confidence: crate::appsize::Confidence::Confirmed,
                    reason: "registeredInstallLocation",
                    location: loc.clone(),
                });
                continue;
            }
        }
        if let Some(loc) = &prog.inferred_location {
            if within(loc) {
                out.push(Identification {
                    program_id: prog.id.clone(),
                    name: prog.name.clone(),
                    confidence: crate::appsize::Confidence::Probable,
                    reason: "uninstallerFolder",
                    location: loc.clone(),
                });
                continue;
            }
        }
        if let Some(fam) = &prog.package_family_name {
            if let Ok(local) = std::env::var("LOCALAPPDATA") {
                let data = format!(r"{local}\Packages\{fam}");
                if within(&data) {
                    out.push(Identification {
                        program_id: prog.id.clone(),
                        name: prog.name.clone(),
                        confidence: crate::appsize::Confidence::Confirmed,
                        reason: "packageDataFolder",
                        location: data,
                    });
                }
            }
        }
    }
    // Most specific folder first (a program inside another program's folder),
    // then the best confidence. On a tie (e.g. Steam games whose uninstaller is
    // steam.exe all "own" the Steam folder) the program whose name matches the
    // file or folder name wins: steam.exe → "Steam", not a game.
    let stem = crate::appsize::norm(p.rsplit('\\').next().unwrap_or("").trim_end_matches(".exe"));
    let named = |i: &Identification| {
        let n = crate::appsize::norm(&i.name);
        let folder = crate::appsize::norm(i.location.trim_end_matches('\\').rsplit('\\').next().unwrap_or(""));
        !n.is_empty() && (stem.starts_with(&n) || (!stem.is_empty() && n.starts_with(&stem)) || folder == n)
    };
    out.sort_by(|a, b| b.location.len().cmp(&a.location.len()).then(a.confidence.cmp(&b.confidence)).then(named(b).cmp(&named(a))));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_folder_goes_to_the_launcher() {
        // Steam games register steam.exe as their uninstaller: all of them
        // "own" the Steam folder. The launcher itself must win for steam.exe.
        let mut game = Program::blank("reg:HKCU:Game".into(), "Overwatch®".into(), ProgramSource::Win32);
        game.inferred_location = Some(r"C:\Games\Steam".into());
        let mut steam = Program::blank("reg:HKLM:Steam".into(), "Steam".into(), ProgramSource::Win32);
        steam.inferred_location = Some(r"C:\Games\Steam".into());
        let r = identify(&[game, steam], r"C:\Games\Steam\steam.exe");
        assert_eq!(r[0].name, "Steam");
    }

    #[test]
    fn command_parsing() {
        assert_eq!(exe_from_command(r#""C:\Program Files\App\uninst.exe" /S"#).unwrap(), r"C:\Program Files\App\uninst.exe");
        assert_eq!(exe_from_command(r"C:\Tools\x\unins000.exe --silent").unwrap(), r"C:\Tools\x\unins000.exe");
        assert!(exe_from_command("MsiExec.exe /X{12345678-1234-1234-1234-123456789012}").is_none());
        assert!(exe_from_command("relative.exe").is_none());
        assert_eq!(parse_icon_ref(r#""C:\A\b.exe",0"#).unwrap(), (r"C:\A\b.exe".to_string(), 0));
        assert_eq!(parse_icon_ref(r"C:\A\b.exe,-101").unwrap(), (r"C:\A\b.exe".to_string(), -101));
        assert_eq!(parse_icon_ref(r"C:\A\icon.ico").unwrap(), (r"C:\A\icon.ico".to_string(), 0));
        assert_eq!(clean_dir(r#""C:\Games\X\""#).unwrap(), r"C:\Games\X");
        assert_eq!(clean_dir("C:/ProgramData/Riot Games/x.ico").unwrap(), r"C:\ProgramData\Riot Games\x.ico");
        assert_eq!(format_install_date("20240131").as_deref(), Some("2024-01-31"));
        assert_eq!(format_install_date("2024-01-31"), None);
    }

    #[test]
    fn inference_skips_shared_folders() {
        let pf = std::env::var("ProgramFiles").unwrap();
        assert_eq!(
            infer_location(Some(&format!(r#""{pf}\Vendor App\uninstall.exe" /S"#)), None).unwrap(),
            format!(r"{pf}\Vendor App")
        );
        // Uninstaller directly in Program Files or System32 → no inference.
        assert!(infer_location(Some(&format!(r"{pf}\uninstall.exe")), None).is_none());
        let win = crate::system::windows_dir();
        assert!(infer_location(Some(&format!(r"{win}\System32\rundll32.exe x")), None).is_none());
    }

    #[test]
    fn reads_real_registry() {
        let list = registry_programs();
        // Every Windows install has at least a few uninstall entries.
        assert!(!list.is_empty());
        assert!(list.iter().all(|p| !p.name.is_empty() && p.registry_key.is_some()));
        let ids: std::collections::HashSet<_> = list.iter().map(|p| &p.id).collect();
        assert_eq!(ids.len(), list.len(), "ids are unique");
    }

    #[test]
    fn identifies_by_folder() {
        let mut p = Program::blank("reg:x".into(), "Demo".into(), ProgramSource::Win32);
        p.install_location = Some(r"C:\Program Files\Demo".into());
        let mut q = Program::blank("reg:y".into(), "Other".into(), ProgramSource::Win32);
        q.inferred_location = Some(r"C:\Tools\Other".into());
        let progs = vec![p, q];
        let r = identify(&progs, r"C:\Program Files\Demo\bin\demo.exe");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].program_id, "reg:x");
        assert!(identify(&progs, r"C:\Program Files\Demo2\x").is_empty(), "prefix must end at a separator");
        assert_eq!(identify(&progs, r"c:\tools\other").len(), 1);
    }
}
