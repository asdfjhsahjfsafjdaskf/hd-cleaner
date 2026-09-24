//! Browser extensions: what is installed, in which browser and profile.
//!
//! Everything is read from the browser's own files — the extension's
//! `manifest.json` (with the localized name when it uses one) and the
//! profile's `Preferences` for whether it is switched on; Firefox's
//! `extensions.json` for the same. Removing one deletes the extension folder
//! through the normal delete protocol, only while the browser is closed, and
//! the screen says plainly that a synced profile can bring it back.

use crate::protection::Risk;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Extension {
    /// "chrome" | "edge" | "brave" | "vivaldi" | "opera" | "operagx" | "firefox"
    pub browser: String,
    pub browser_name: String,
    /// Profile folder name ("Default", "Profile 1", a Firefox profile…).
    pub profile: String,
    pub id: String,
    pub name: String,
    pub version: String,
    /// Folder (Chromium) or .xpi file (Firefox).
    pub path: String,
    pub bytes: u64,
    /// `None` when the browser does not say.
    pub enabled: Option<bool>,
    /// The browser is open: removal is refused while it is.
    pub browser_running: bool,
}

fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    for e in rd.flatten() {
        match e.metadata() {
            Ok(m) if m.is_dir() => total += dir_size(&e.path()),
            Ok(m) => total += m.len(),
            Err(_) => {}
        }
    }
    total
}

fn read_json(path: &Path, limit: u64) -> Option<serde_json::Value> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > limit {
        return None;
    }
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// `__MSG_appName__` → the string from `_locales/<default_locale>/messages.json`.
fn localized(version_dir: &Path, manifest: &serde_json::Value, raw: &str) -> String {
    let Some(key) = raw.strip_prefix("__MSG_").and_then(|r| r.strip_suffix("__")) else {
        return raw.to_string();
    };
    let locales = ["en_US", "en"];
    let default = manifest.get("default_locale").and_then(|v| v.as_str()).unwrap_or("");
    for locale in std::iter::once(default).chain(locales) {
        if locale.is_empty() {
            continue;
        }
        let file = version_dir.join("_locales").join(locale).join("messages.json");
        if let Some(json) = read_json(&file, 4 << 20) {
            if let Some(msg) = json.get(key).and_then(|v| v.get("message")).and_then(|v| v.as_str()) {
                return msg.to_string();
            }
        }
    }
    raw.to_string()
}

/// Chromium keeps `Extensions\<id>\<version>\manifest.json`.
fn chromium_extensions(browser: &str, browser_name: &str, profile_dir: &Path, running: bool) -> Vec<Extension> {
    let mut out = Vec::new();
    let root = profile_dir.join("Extensions");
    let Ok(rd) = std::fs::read_dir(&root) else { return out };
    // Preferences says which are switched on.
    let prefs = read_json(&profile_dir.join("Preferences"), 64 << 20);
    let settings = prefs.as_ref().and_then(|p| p.pointer("/extensions/settings"));
    let profile = profile_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();

    for entry in rd.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        // The newest version folder is the one in use.
        let Ok(versions) = std::fs::read_dir(entry.path()) else { continue };
        let mut dirs: Vec<PathBuf> = versions.flatten().filter(|v| v.file_type().is_ok_and(|t| t.is_dir())).map(|v| v.path()).collect();
        dirs.sort();
        let Some(version_dir) = dirs.last() else { continue };
        let Some(manifest) = read_json(&version_dir.join("manifest.json"), 8 << 20) else { continue };
        let raw_name = manifest.get("name").and_then(|v| v.as_str()).unwrap_or(&id).to_string();
        let name = localized(version_dir, &manifest, &raw_name);
        let version = manifest.get("version").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let enabled = settings
            .and_then(|s| s.get(&id))
            .and_then(|e| e.get("state"))
            .and_then(|v| v.as_i64())
            .map(|state| state == 1);
        out.push(Extension {
            browser: browser.into(),
            browser_name: browser_name.into(),
            profile: profile.clone(),
            id,
            name,
            version,
            bytes: dir_size(&entry.path()),
            path: entry.path().to_string_lossy().into_owned(),
            enabled,
            browser_running: running,
        });
    }
    out
}

/// Firefox lists its add-ons in `extensions.json`, with the `.xpi` beside it.
fn firefox_extensions(profile_dir: &Path, running: bool) -> Vec<Extension> {
    let mut out = Vec::new();
    let Some(json) = read_json(&profile_dir.join("extensions.json"), 32 << 20) else { return out };
    let profile = profile_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    for addon in json.get("addons").and_then(|v| v.as_array()).into_iter().flatten() {
        // System add-ons ship with Firefox and are not the user's to remove.
        let location = addon.get("location").and_then(|v| v.as_str()).unwrap_or("");
        if location != "app-profile" && location != "app-system-profile" {
            continue;
        }
        let id = addon.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        if id.is_empty() {
            continue;
        }
        let name = addon
            .pointer("/defaultLocale/name")
            .and_then(|v| v.as_str())
            .unwrap_or(&id)
            .to_string();
        let path = addon.get("path").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        out.push(Extension {
            browser: "firefox".into(),
            browser_name: "Firefox".into(),
            profile: profile.clone(),
            id,
            name,
            version: addon.get("version").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            path,
            bytes,
            enabled: addon.get("active").and_then(|v| v.as_bool()),
            browser_running: running,
        });
    }
    out
}

/// Chromium user-data folders, the same ones the Cleaner knows.
fn chromium_roots() -> Vec<(&'static str, &'static str, &'static str, PathBuf)> {
    let local = std::env::var("LOCALAPPDATA").ok().map(PathBuf::from);
    let roaming = std::env::var("APPDATA").ok().map(PathBuf::from);
    let l = |p: &str| local.as_ref().map(|b| b.join(p));
    let r = |p: &str| roaming.as_ref().map(|b| b.join(p));
    [
        ("chrome", "Google Chrome", "chrome.exe", l(r"Google\Chrome\User Data")),
        ("edge", "Microsoft Edge", "msedge.exe", l(r"Microsoft\Edge\User Data")),
        ("brave", "Brave", "brave.exe", l(r"BraveSoftware\Brave-Browser\User Data")),
        ("vivaldi", "Vivaldi", "vivaldi.exe", l(r"Vivaldi\User Data")),
        ("opera", "Opera", "opera.exe", r(r"Opera Software\Opera Stable")),
        ("operagx", "Opera GX", "opera.exe", r(r"Opera Software\Opera GX Stable")),
    ]
    .into_iter()
    .filter_map(|(k, n, exe, p)| p.filter(|p| p.is_dir()).map(|p| (k, n, exe, p)))
    .collect()
}

fn profiles_of(root: &Path) -> Vec<PathBuf> {
    if root.join("Extensions").is_dir() || root.join("Preferences").is_file() {
        // Opera: the user-data folder is itself the profile.
        return vec![root.to_path_buf()];
    }
    let Ok(rd) = std::fs::read_dir(root) else { return Vec::new() };
    let mut out: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n == "Default" || n.starts_with("Profile ")
        })
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

/// Every extension installed in every browser profile found.
pub fn list() -> Vec<Extension> {
    let procs = crate::processes::list();
    let running = |exe: &str, hint: Option<&str>| {
        procs.iter().any(|p| {
            p.name.eq_ignore_ascii_case(exe)
                && hint.is_none_or(|h| p.path.as_deref().is_some_and(|path| path.to_lowercase().contains(h)))
        })
    };
    let mut out = Vec::new();
    for (key, name, exe, root) in chromium_roots() {
        let hint = match key {
            "opera" => Some(r"\opera\"),
            "operagx" => Some(r"\opera gx\"),
            _ => None,
        };
        let is_running = running(exe, hint);
        for profile in profiles_of(&root) {
            out.extend(chromium_extensions(key, name, &profile, is_running));
        }
    }
    let firefox_running = running("firefox.exe", None);
    if let Ok(roaming) = std::env::var("APPDATA") {
        let base = PathBuf::from(roaming).join(r"Mozilla\Firefox\Profiles");
        if let Ok(rd) = std::fs::read_dir(&base) {
            for e in rd.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())) {
                out.extend(firefox_extensions(&e.path(), firefox_running));
            }
        }
    }
    out.sort_by(|a, b| a.browser_name.cmp(&b.browser_name).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    out
}

/// Remove one extension: the folder (Chromium) or the `.xpi` (Firefox).
///
/// Refused while the browser is open — it would rewrite the profile on exit —
/// and the path must be the one this module listed, inside a browser profile.
pub fn remove(ext: &Extension, recycle: bool) -> crate::Result<()> {
    if ext.browser_running {
        return Err(crate::AppError::InvalidInput(format!("{} is open — close it first", ext.browser_name)));
    }
    let fresh = list();
    let known = fresh
        .iter()
        .any(|e| e.id == ext.id && e.browser == ext.browser && e.path.eq_ignore_ascii_case(&ext.path));
    if !known {
        return Err(crate::AppError::NotFound { path: ext.path.clone() });
    }
    if crate::protection::assess(&ext.path).risk == Risk::Blocked {
        return Err(crate::AppError::Protected { path: ext.path.clone(), reason: "protected".into() });
    }
    let mode = if recycle { crate::fsops::DeleteMode::RecycleBin } else { crate::fsops::DeleteMode::Permanent };
    let plan = crate::fsops::plan_delete(&[(ext.path.clone(), None)], mode);
    let results = crate::fsops::execute_delete(
        &plan,
        &crate::fsops::ExecuteOptions { dry_run: false, allow_dangerous: false },
        |_, _| {},
        || true,
    );
    match results.into_iter().next().map(|r| r.outcome) {
        Some(crate::fsops::ItemOutcome::Deleted) | None => Ok(()),
        Some(crate::fsops::ItemOutcome::Failed { error }) => Err(crate::AppError::Helper(error.message)),
        Some(crate::fsops::ItemOutcome::Skipped { reason }) => Err(crate::AppError::Protected { path: ext.path.clone(), reason }),
        // Only produced in a dry run, which this never is.
        Some(crate::fsops::ItemOutcome::WouldDelete) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_chromium_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let profile = tmp.path().join("Default");
        let ext = profile.join("Extensions").join("abcdefghijklmnopabcdefghijklmnop").join("1.2.3_0");
        std::fs::create_dir_all(&ext).unwrap();
        std::fs::write(ext.join("manifest.json"), br#"{"name":"__MSG_appName__","version":"1.2.3","default_locale":"pt_BR"}"#).unwrap();
        let locale = ext.join("_locales").join("pt_BR");
        std::fs::create_dir_all(&locale).unwrap();
        std::fs::write(locale.join("messages.json"), br#"{"appName":{"message":"Bloqueador"}}"#).unwrap();
        std::fs::write(ext.join("background.js"), vec![7u8; 1024]).unwrap();
        std::fs::write(
            profile.join("Preferences"),
            br#"{"extensions":{"settings":{"abcdefghijklmnopabcdefghijklmnop":{"state":0}}}}"#,
        )
        .unwrap();

        let found = chromium_extensions("chrome", "Google Chrome", &profile, false);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Bloqueador", "the localized name is used");
        assert_eq!(found[0].version, "1.2.3");
        assert_eq!(found[0].enabled, Some(false), "Preferences says it is off");
        assert!(found[0].bytes >= 1024);
        assert_eq!(found[0].profile, "Default");
    }

    #[test]
    fn reads_firefox_addons_and_skips_the_ones_that_ship_with_it() {
        let tmp = tempfile::tempdir().unwrap();
        let profile = tmp.path().join("abc.default-release");
        std::fs::create_dir_all(&profile).unwrap();
        let xpi = profile.join("extensions").join("mine@example.com.xpi");
        std::fs::create_dir_all(xpi.parent().unwrap()).unwrap();
        std::fs::write(&xpi, vec![1u8; 2048]).unwrap();
        let json = format!(
            r#"{{"addons":[
                {{"id":"mine@example.com","version":"2.0","active":true,"location":"app-profile","path":{path},"defaultLocale":{{"name":"My Add-on"}}}},
                {{"id":"builtin@mozilla.org","version":"1.0","active":true,"location":"app-builtin","defaultLocale":{{"name":"Built in"}}}}
            ]}}"#,
            path = serde_json::to_string(&xpi.to_string_lossy()).unwrap()
        );
        std::fs::write(profile.join("extensions.json"), json).unwrap();

        let found = firefox_extensions(&profile, false);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].name, "My Add-on");
        assert_eq!(found[0].enabled, Some(true));
        assert_eq!(found[0].bytes, 2048);
    }

    #[test]
    fn removal_is_refused_while_the_browser_is_open() {
        let ext = Extension {
            browser: "chrome".into(),
            browser_name: "Google Chrome".into(),
            profile: "Default".into(),
            id: "x".into(),
            name: "X".into(),
            version: "1".into(),
            path: r"C:\nowhere".into(),
            bytes: 0,
            enabled: Some(true),
            browser_running: true,
        };
        assert!(remove(&ext, true).is_err());
    }
}
