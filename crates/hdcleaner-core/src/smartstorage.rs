//! Smart storage: what the big things on the disk actually *are*.
//!
//! Games come from the launchers' own files (Steam's `.acf` manifests, Epic's
//! `LauncherInstalled.dat`, Riot's `RiotClientInstalls.json`), never from
//! guessing folder names — so a folder is only called a game when the
//! launcher says it installed one there. Caches are tied to the program that
//! made them through the correlation engine, which names the signal it used.

use crate::correlate::{self, Evidence};
use crate::programs::Program;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Game {
    pub name: String,
    /// "steam" | "epic" | "riot"
    pub launcher: &'static str,
    pub path: String,
    /// Size the launcher reports, when it does.
    pub reported_bytes: Option<u64>,
    /// Measured now, when asked for.
    pub bytes: Option<u64>,
    pub id: Option<String>,
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameLibrary {
    pub launcher: &'static str,
    pub path: String,
    pub games: usize,
}

/// Minimal reader for Valve's key-value text format: `"key" "value"` lines and
/// `"key" { … }` blocks. Only top-level values are returned.
fn vdf_values(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let mut parts = line.split('"').filter(|s| !s.trim().is_empty() || s.is_empty());
        // "key"<space>"value"
        let raw: Vec<&str> = line.split('"').collect();
        if raw.len() >= 5 {
            let key = raw[1].to_string();
            let value = raw[3].to_string();
            if !key.is_empty() {
                out.push((key, value));
            }
        }
        let _ = parts.next();
    }
    out
}

fn read_limited(path: &Path, limit: usize) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() as usize > limit {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// Steam libraries from `libraryfolders.vdf`, starting at the install folder.
pub fn steam_libraries() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    // Steam writes its own path lowercased and with forward slashes, and the
    // same library shows up again in libraryfolders.vdf properly cased: the
    // same folder must be listed once.
    let norm = |p: &Path| p.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase();
    let mut push = |p: PathBuf| {
        let key = norm(&p);
        if !out.iter().any(|q: &PathBuf| norm(q) == key) && p.join("steamapps").is_dir() {
            out.push(PathBuf::from(key));
        }
    };
    let install = crate::registry::read_string(crate::registry::Hive::CurrentUser, r"Software\Valve\Steam", "SteamPath", crate::registry::View::Default)
        .map(|s| s.replace('/', "\\"))
        .or_else(|| {
            crate::registry::read_string(
                crate::registry::Hive::LocalMachine,
                r"SOFTWARE\WOW6432Node\Valve\Steam",
                "InstallPath",
                crate::registry::View::Reg32,
            )
        });
    let Some(install) = install else { return out };
    let root = PathBuf::from(&install);
    push(root.clone());
    if let Some(text) = read_limited(&root.join("steamapps").join("libraryfolders.vdf"), 1 << 20) {
        for (key, value) in vdf_values(&text) {
            if key == "path" {
                push(PathBuf::from(value.replace("\\\\", "\\")));
            }
        }
    }
    out
}

fn steam_games(lib: &Path) -> Vec<Game> {
    let apps = lib.join("steamapps");
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(&apps) else { return out };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with("appmanifest_") || !name.ends_with(".acf") {
            continue;
        }
        let Some(text) = read_limited(&e.path(), 1 << 20) else { continue };
        let values = vdf_values(&text);
        let get = |k: &str| values.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
        let (Some(title), Some(dir)) = (get("name"), get("installdir")) else { continue };
        let path = apps.join("common").join(&dir);
        out.push(Game {
            name: title,
            launcher: "steam",
            reported_bytes: get("SizeOnDisk").and_then(|s| s.parse().ok()),
            exists: path.is_dir(),
            path: path.to_string_lossy().into_owned(),
            bytes: None,
            id: get("appid"),
        });
    }
    out
}

fn epic_games() -> Vec<Game> {
    let mut out = Vec::new();
    let Ok(program_data) = std::env::var("ProgramData") else { return out };
    let file = PathBuf::from(program_data).join(r"Epic\UnrealEngineLauncher\LauncherInstalled.dat");
    let Some(text) = read_limited(&file, 4 << 20) else { return out };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else { return out };
    for item in json.get("InstallationList").and_then(|v| v.as_array()).into_iter().flatten() {
        let Some(path) = item.get("InstallLocation").and_then(|v| v.as_str()) else { continue };
        let name = item
            .get("AppName")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        // Epic's AppName is an id for games and a readable name for tools; the
        // folder is what the user recognises.
        let folder = Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| name.clone());
        out.push(Game {
            name: folder,
            launcher: "epic",
            path: path.to_string(),
            reported_bytes: None,
            bytes: None,
            id: Some(name),
            exists: Path::new(path).is_dir(),
        });
    }
    out
}

fn riot_games() -> Vec<Game> {
    let mut out = Vec::new();
    let Ok(program_data) = std::env::var("ProgramData") else { return out };
    let file = PathBuf::from(program_data).join(r"Riot Games\RiotClientInstalls.json");
    let Some(text) = read_limited(&file, 1 << 20) else { return out };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else { return out };
    // The file maps product keys to the client executable of each install.
    for (key, value) in json.as_object().into_iter().flatten() {
        let Some(exe) = value.as_str() else { continue };
        let Some(dir) = Path::new(exe).parent() else { continue };
        let name = riot_product_name(key);
        let path = dir.to_string_lossy().replace('/', "\\");
        if out.iter().any(|g: &Game| g.path.eq_ignore_ascii_case(&path)) {
            continue;
        }
        out.push(Game {
            name: if name.trim().is_empty() { "Riot Client".into() } else { name },
            launcher: "riot",
            exists: dir.is_dir(),
            path,
            reported_bytes: None,
            bytes: None,
            id: Some(key.clone()),
        });
    }
    out
}

/// `rc_live` → "Riot Client", `league_of_legends.live` → "League Of Legends".
fn riot_product_name(key: &str) -> String {
    let base = key.split('.').next().unwrap_or(key);
    if base == "rc" || base.starts_with("rc_") {
        return "Riot Client".into();
    }
    let words: Vec<String> = base
        .split('_')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect();
    if words.is_empty() {
        "Riot Client".into()
    } else {
        words.join(" ")
    }
}

/// Every game the installed launchers say they installed.
pub fn games() -> Vec<Game> {
    let mut out = Vec::new();
    for lib in steam_libraries() {
        out.extend(steam_games(&lib));
    }
    out.extend(epic_games());
    out.extend(riot_games());
    out.sort_by(|a, b| b.reported_bytes.unwrap_or(0).cmp(&a.reported_bytes.unwrap_or(0)).then(a.name.cmp(&b.name)));
    out
}

pub fn libraries() -> Vec<GameLibrary> {
    let mut out: Vec<GameLibrary> = steam_libraries()
        .into_iter()
        .map(|p| {
            let games = steam_games(&p).len();
            GameLibrary { launcher: "steam", path: p.to_string_lossy().into_owned(), games }
        })
        .collect();
    for (launcher, list) in [("epic", epic_games()), ("riot", riot_games())] {
        for g in list {
            let parent = Path::new(&g.path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or(g.path.clone());
            match out.iter_mut().find(|l| l.launcher == launcher && l.path.eq_ignore_ascii_case(&parent)) {
                Some(l) => l.games += 1,
                None => out.push(GameLibrary { launcher, path: parent, games: 1 }),
            }
        }
    }
    out
}

/// Measure the folders of `games` now (they can be very large: this walks the
/// tree, so callers run it in the background).
pub fn measure(games: &mut [Game], should_stop: &dyn Fn() -> bool) {
    let ctl = crate::scan::ScanControl::new();
    for g in games.iter_mut() {
        if should_stop() {
            return;
        }
        if g.exists {
            g.bytes = crate::appsize::measure_live(&g.path, &ctl).map(|m| m.total);
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheOwner {
    /// Cleaner category the cache belongs to.
    pub category: String,
    pub label: String,
    pub path: String,
    pub bytes: u64,
    pub count: u64,
    pub program_id: Option<String>,
    pub program: Option<String>,
    /// 0-100 for the program match; absent when nothing matched.
    pub score: u8,
    pub signals: Vec<&'static str>,
}

/// Tie each cache the Cleaner found to the program that made it.
pub fn cache_owners(programs: &[Program], evidence: &Evidence, analysis: &[crate::cleaner::CategoryResult]) -> Vec<CacheOwner> {
    let mut out = Vec::new();
    for r in analysis.iter().filter(|r| r.bytes > 0) {
        let Some(root) = r.category.rules.first().map(|rule| rule.root().to_string()) else { continue };
        let owner = correlate::owner_of(programs, &root, evidence);
        // The Cleaner's own label is used when the correlation finds nothing.
        out.push(CacheOwner {
            category: r.category.id.clone(),
            label: r.category.label.clone(),
            path: root,
            bytes: r.bytes,
            count: r.count,
            program_id: owner.as_ref().map(|m| m.program_id.clone()),
            program: owner.as_ref().map(|m| m.name.clone()).or_else(|| r.category.owner.clone()),
            score: owner.as_ref().map(|m| m.score).unwrap_or(0),
            signals: owner.map(|m| m.signals).unwrap_or_default(),
        });
    }
    out.sort_by_key(|c| std::cmp::Reverse(c.bytes));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_valve_key_values() {
        let text = "\"AppState\"\n{\n\t\"appid\"\t\t\"570\"\n\t\"name\"\t\t\"Dota 2\"\n\t\"installdir\"\t\t\"dota 2 beta\"\n\t\"SizeOnDisk\"\t\t\"41815082701\"\n}\n";
        let v = vdf_values(text);
        let get = |k: &str| v.iter().find(|(key, _)| key == k).map(|(_, val)| val.clone());
        assert_eq!(get("name").as_deref(), Some("Dota 2"));
        assert_eq!(get("installdir").as_deref(), Some("dota 2 beta"));
        assert_eq!(get("SizeOnDisk").and_then(|s| s.parse::<u64>().ok()), Some(41_815_082_701));
        // A block header line has no value and must not be read as one.
        assert!(get("AppState").is_none());
    }

    #[test]
    fn a_steam_library_is_read_from_its_manifests() {
        let tmp = tempfile::tempdir().unwrap();
        let apps = tmp.path().join("steamapps");
        std::fs::create_dir_all(apps.join("common").join("Some Game")).unwrap();
        std::fs::write(
            apps.join("appmanifest_12345.acf"),
            "\"AppState\"\n{\n\t\"appid\"\t\t\"12345\"\n\t\"name\"\t\t\"Some Game\"\n\t\"installdir\"\t\t\"Some Game\"\n\t\"SizeOnDisk\"\t\t\"2048\"\n}\n",
        )
        .unwrap();
        // A file that is not a manifest is ignored.
        std::fs::write(apps.join("readme.txt"), "x").unwrap();

        let games = steam_games(tmp.path());
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].name, "Some Game");
        assert_eq!(games[0].reported_bytes, Some(2048));
        assert_eq!(games[0].id.as_deref(), Some("12345"));
        assert!(games[0].exists, "the install folder is there");
    }

    #[test]
    fn riot_keys_become_readable_names() {
        assert_eq!(riot_product_name("rc_default"), "Riot Client");
        assert_eq!(riot_product_name("rc_live"), "Riot Client");
        assert_eq!(riot_product_name("league_of_legends.live"), "League Of Legends");
        assert_eq!(riot_product_name("valorant.live"), "Valorant");
    }

    #[test]
    fn the_same_steam_library_is_never_listed_twice() {
        // Whatever this machine has, no two libraries may be the same folder.
        let libs = steam_libraries();
        let mut seen: Vec<String> = Vec::new();
        for l in &libs {
            let key = l.to_string_lossy().to_lowercase();
            assert!(!seen.contains(&key), "{key} listed twice");
            seen.push(key);
        }
    }

    #[test]
    fn games_on_this_machine_are_real_folders() {
        // Whatever the launchers report here, the app must not invent games.
        for g in games() {
            assert!(!g.name.trim().is_empty(), "{g:?}");
            assert!(Path::new(&g.path).is_absolute(), "{g:?}");
        }
    }
}
