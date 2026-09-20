//! Protection Engine: every destructive operation asks this module first.
//!
//! * `Blocked` — never deleted by the application (Windows directory, boot
//!   files, EFI, volume roots, top-level profile/program folders...).
//! * `Dangerous` — allowed only after an explicit, per-item confirmation.
//! * `Review` — ordinary user data; confirmation required.
//! * `Safe` — clearly disposable (temp folders).
//!
//! Rules are deterministic; nothing here is inferred heuristically.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum Risk {
    Safe,
    Review,
    Dangerous,
    Blocked,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Assessment {
    pub risk: Risk,
    /// Stable reason key (translated by the UI).
    pub reason: &'static str,
}

struct Env {
    windows: String,
    program_files: Vec<String>,
    program_data: String,
    users: String,
    profile: String,
    temp_dirs: Vec<String>,
}

fn norm(p: &str) -> String {
    let s = crate::util::from_extended(p.trim()).replace('/', "\\").to_lowercase();
    let mut s = s.trim_end_matches('\\').to_string();
    if s.len() == 2 && s.ends_with(':') {
        s.push('\\');
    }
    s
}

fn env_path(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|v| !v.is_empty()).map(|v| norm(&v))
}

fn env() -> Env {
    let system_drive = env_path("SystemDrive").unwrap_or_else(|| "c:".into());
    let sd = system_drive.trim_end_matches('\\').to_string();
    let mut program_files: Vec<String> =
        ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"].iter().filter_map(|v| env_path(v)).collect();
    program_files.push(format!("{sd}\\program files"));
    program_files.push(format!("{sd}\\program files (x86)"));
    program_files.sort();
    program_files.dedup();
    let mut temp_dirs: Vec<String> = ["TEMP", "TMP"].iter().filter_map(|v| env_path(v)).collect();
    let windows = env_path("SystemRoot").unwrap_or_else(|| format!("{sd}\\windows"));
    temp_dirs.push(format!("{windows}\\temp"));
    if let Some(local) = env_path("LOCALAPPDATA") {
        temp_dirs.push(format!("{local}\\temp"));
    }
    temp_dirs.sort();
    temp_dirs.dedup();
    Env {
        program_data: env_path("ProgramData").unwrap_or_else(|| format!("{sd}\\programdata")),
        users: format!("{sd}\\users"),
        profile: env_path("USERPROFILE").unwrap_or_default(),
        windows,
        program_files,
        temp_dirs,
    }
}

fn is_within(path: &str, dir: &str) -> bool {
    !dir.is_empty()
        && path.len() >= dir.len()
        && path.starts_with(dir)
        && (path.len() == dir.len() || path.as_bytes()[dir.len()] == b'\\' || dir.ends_with('\\'))
}

/// Classify a path for deletion.
pub fn assess(path: &str) -> Assessment {
    assess_with(&env(), path)
}

fn assess_with(e: &Env, path: &str) -> Assessment {
    let p = norm(path);
    let blocked = |reason| Assessment { risk: Risk::Blocked, reason };
    let bytes = p.as_bytes();

    if p.is_empty() || p.contains('\0') {
        return blocked("invalidPath");
    }
    // Volume roots and UNC share roots.
    if (bytes.len() == 3 && bytes[1] == b':') || (p.starts_with("\\\\") && p[2..].matches('\\').count() <= 1) {
        return blocked("volumeRoot");
    }

    // Temp directories inside Windows are the only allowed exception there.
    for t in &e.temp_dirs {
        if is_within(&p, t) {
            return if p == *t {
                blocked("tempRoot")
            } else {
                Assessment { risk: Risk::Safe, reason: "tempFolder" }
            };
        }
    }
    if is_within(&p, &e.windows) {
        return blocked("windowsDirectory");
    }

    // Files/folders at the root of any volume that Windows needs.
    let (volume, rest) = if bytes.len() > 3 && bytes[1] == b':' { (&p[..2], &p[3..]) } else { ("", "") };
    if !volume.is_empty() {
        let first = rest.split('\\').next().unwrap_or("");
        const ROOT_BLOCK: &[&str] = &[
            "boot",
            "efi",
            "bootmgr",
            "bootnxt",
            "bootsect.bak",
            "pagefile.sys",
            "hiberfil.sys",
            "swapfile.sys",
            "dumpstack.log",
            "dumpstack.log.tmp",
            "system volume information",
            "recovery",
            "$recycle.bin",
            "$winreagent",
            "$sysreset",
            "config.msi",
            "documents and settings",
        ];
        if ROOT_BLOCK.contains(&first) {
            return blocked("bootOrSystemFile");
        }
        if first.starts_with('$') && !rest.contains('\\') {
            return blocked("ntfsMetadata");
        }
    }

    // Top-level containers must never be removed wholesale.
    let mut containers: Vec<&str> = vec![e.program_data.as_str(), e.users.as_str(), e.profile.as_str()];
    containers.extend(e.program_files.iter().map(String::as_str));
    let profile_roots: Vec<String> = if e.profile.is_empty() {
        vec![]
    } else {
        ["appdata", "appdata\\local", "appdata\\roaming", "appdata\\locallow", "documents", "desktop", "downloads",
         "pictures", "videos", "music", "onedrive"]
            .iter()
            .map(|s| format!("{}\\{s}", e.profile))
            .collect()
    };
    containers.extend(profile_roots.iter().map(String::as_str));
    if containers.iter().any(|c| !c.is_empty() && p == *c) {
        return blocked("protectedContainer");
    }
    let public = format!("{}\\public", e.users);
    let default = format!("{}\\default", e.users);
    if p == public || p == default || p == format!("{}\\all users", e.users) {
        return blocked("protectedContainer");
    }
    // Direct children of C:\Users are whole user profiles.
    if is_within(&p, &e.users) && p[e.users.len() + 1..].split('\\').count() == 1 {
        return blocked("userProfileRoot");
    }

    for pf in &e.program_files {
        if is_within(&p, &format!("{pf}\\windowsapps")) {
            return blocked("windowsApps");
        }
        if is_within(&p, &format!("{pf}\\common files")) || is_within(&p, &format!("{pf}\\windows defender")) {
            return Assessment { risk: Risk::Dangerous, reason: "sharedProgramComponents" };
        }
        if is_within(&p, pf) {
            return Assessment { risk: Risk::Dangerous, reason: "installedProgram" };
        }
    }
    if is_within(&p, &format!("{}\\microsoft", e.program_data)) {
        return Assessment { risk: Risk::Dangerous, reason: "systemProgramData" };
    }
    if is_within(&p, &e.program_data) {
        return Assessment { risk: Risk::Dangerous, reason: "sharedProgramData" };
    }
    if !e.profile.is_empty() && is_within(&p, &format!("{}\\ntuser.dat", e.profile)) {
        return blocked("userRegistryHive");
    }
    Assessment { risk: Risk::Review, reason: "userData" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e() -> Env {
        Env {
            windows: "c:\\windows".into(),
            program_files: vec!["c:\\program files".into(), "c:\\program files (x86)".into()],
            program_data: "c:\\programdata".into(),
            users: "c:\\users".into(),
            profile: "c:\\users\\ana".into(),
            temp_dirs: vec!["c:\\users\\ana\\appdata\\local\\temp".into(), "c:\\windows\\temp".into()],
        }
    }

    fn risk(p: &str) -> Risk {
        assess_with(&e(), p).risk
    }

    #[test]
    fn blocks_critical() {
        for p in [
            "C:\\",
            "c:",
            "C:\\Windows",
            "C:\\Windows\\System32",
            "C:\\Windows\\System32\\drivers\\etc\\hosts",
            "C:\\WINDOWS\\WinSxS\\x",
            "\\\\?\\C:\\Windows\\System32",
            "C:/Windows/System32",
            "C:\\pagefile.sys",
            "C:\\EFI\\Microsoft",
            "C:\\Boot",
            "C:\\$MFT",
            "C:\\System Volume Information",
            "C:\\Program Files",
            "C:\\Program Files\\WindowsApps\\x",
            "C:\\Users",
            "C:\\Users\\ana",
            "C:\\Users\\Other",
            "C:\\Users\\ana\\AppData",
            "C:\\Users\\ana\\Documents",
            "C:\\Users\\ana\\AppData\\Local\\Temp",
            "C:\\ProgramData",
            "\\\\server\\share",
            "D:\\",
        ] {
            assert_eq!(risk(p), Risk::Blocked, "{p}");
        }
    }

    #[test]
    fn classifies_others() {
        assert_eq!(risk("C:\\Windows\\Temp\\x.tmp"), Risk::Safe);
        assert_eq!(risk("C:\\Users\\ana\\AppData\\Local\\Temp\\abc"), Risk::Safe);
        assert_eq!(risk("C:\\Program Files\\App"), Risk::Dangerous);
        assert_eq!(risk("C:\\ProgramData\\Vendor"), Risk::Dangerous);
        assert_eq!(risk("C:\\Users\\ana\\Documents\\x.docx"), Risk::Review);
        assert_eq!(risk("C:\\Users\\ana\\AppData\\Roaming\\discord"), Risk::Review);
        assert_eq!(risk("D:\\Games\\big.iso"), Risk::Review);
        // "Windows.old" is not the Windows directory.
        assert_eq!(risk("C:\\Windows.old"), Risk::Review);
        assert_eq!(risk("C:\\WindowsApps-like"), Risk::Review);
    }
}
