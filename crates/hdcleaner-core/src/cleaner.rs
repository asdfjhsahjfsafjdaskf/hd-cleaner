//! Cleaner: temporary files, caches, browser data and recent-item lists.
//!
//! Always two steps. `analyze` only lists what each category would remove
//! (files with size and date, registry values with their text). `clean`
//! removes only items from that list, and only if they are unchanged (same
//! size and modification time) and still inside the category's fixed
//! folders. Links and junctions are never followed or removed through.
//! Temporary folders only offer files older than 24 hours (installers in
//! progress keep theirs). Browsers and apps must be closed for their data to
//! be cleaned. Registry lists are exported to a .reg file before removal.

use crate::registry::{Hive, Key, View};
use crate::util::wide;
use crate::{AppError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const MAX_ITEMS: usize = 200_000;
const TEMP_MIN_AGE_MS: i64 = 24 * 3600 * 1000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Group {
    System,
    Apps,
    Browsers,
    Privacy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum Risk {
    Safe,
    Review,
    Dangerous,
}

/// How a category finds its items.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Rule {
    /// Every file below `dir` (optionally only older than `min_age_ms`).
    Dir { dir: String, recursive: bool, min_age_ms: Option<i64> },
    /// Named files directly in `dir`.
    Files { dir: String, names: Vec<String> },
    /// Files in `dir` whose name starts with `prefix` and ends with `suffix`.
    Match { dir: String, prefix: String, suffix: String },
}

impl Rule {
    fn root(&self) -> &str {
        match self {
            Rule::Dir { dir, .. } | Rule::Files { dir, .. } | Rule::Match { dir, .. } => dir,
        }
    }
}

/// Categories whose items are not plain files.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Special {
    RecycleBin,
    /// Download list inside Chromium `History` databases (rows removed, history kept).
    ChromiumDownloads { databases: Vec<String> },
    /// Values of fixed HKCU "most recently used" keys.
    RegistryMru { keys: Vec<String> },
    /// Visited pages inside Firefox `places.sqlite` (bookmarks are kept).
    FirefoxHistory { databases: Vec<String> },
    /// Download entries inside Firefox `places.sqlite` (annotations only).
    FirefoxDownloads { databases: Vec<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Category {
    /// Stable id, e.g. `userTemp`, `browser.chrome.cache`, `privacy.runMru`.
    pub id: String,
    pub group: Group,
    /// Translation key of the label (`cleaner.cat_<label>`).
    pub label: String,
    /// Browser or app display name, when the category belongs to one.
    pub owner: Option<String>,
    pub risk: Risk,
    pub default_on: bool,
    /// Machine-wide folders: removal goes through the elevated helper.
    pub admin: bool,
    pub rules: Vec<Rule>,
    pub special: Option<Special>,
    /// Executable names that must not be running (browser / app).
    pub processes: Vec<String>,
    /// Extra folder hint to tell apart programs sharing an exe name (Opera / Opera GX).
    pub process_path_hint: Option<String>,
    /// Translation key of a warning shown with the category.
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CleanItem {
    /// File path, or `HKCU\key → value` for registry items.
    pub path: String,
    pub size: u64,
    /// Last write time (FILETIME ticks) for files; 0 otherwise.
    pub mtime: u64,
    /// Text shown for registry items / download entries.
    pub display: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CategoryResult {
    pub category: Category,
    pub items: Vec<CleanItem>,
    pub count: u64,
    pub bytes: u64,
    /// The browser/app is running: its data cannot be cleaned now.
    pub running: bool,
    /// Items not listed because they are newer than 24 hours (temp folders).
    pub recent_skipped: u64,
    /// Machine-wide category that a standard user cannot even list: the
    /// elevated helper must analyze it (`analyze_admin`).
    pub admin_pending: bool,
    pub truncated: bool,
    pub error: Option<crate::ErrorPayload>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CleanOutcome {
    pub removed: u64,
    pub freed: u64,
    /// In use by another program (left alone).
    pub in_use: u64,
    /// Changed since the analysis (left alone).
    pub changed: u64,
    pub denied: u64,
    pub failed: u64,
    pub errors: Vec<String>,
}

impl CleanOutcome {
    fn merge(&mut self, o: CleanOutcome) {
        self.removed += o.removed;
        self.freed += o.freed;
        self.in_use += o.in_use;
        self.changed += o.changed;
        self.denied += o.denied;
        self.failed += o.failed;
        self.errors.extend(o.errors);
        self.errors.truncate(50);
    }
}

// ---------------------------------------------------------------------------
// Catalog
// ---------------------------------------------------------------------------

fn env(v: &str) -> Option<PathBuf> {
    std::env::var_os(v).map(PathBuf::from)
}

fn s(p: &Path) -> String {
    p.to_string_lossy().to_string()
}

fn cat(id: &str, group: Group, label: &str, risk: Risk, default_on: bool, rules: Vec<Rule>) -> Category {
    Category {
        id: id.into(),
        group,
        label: label.into(),
        owner: None,
        risk,
        default_on,
        admin: false,
        rules,
        special: None,
        processes: Vec::new(),
        process_path_hint: None,
        warning: None,
    }
}

fn dir(p: PathBuf) -> Rule {
    Rule::Dir { dir: s(&p), recursive: true, min_age_ms: None }
}

fn files(d: &Path, names: &[&str]) -> Rule {
    Rule::Files { dir: s(d), names: names.iter().map(|n| n.to_string()).collect() }
}

struct Chromium {
    key: &'static str,
    name: &'static str,
    exe: &'static str,
    path_hint: Option<&'static str>,
    /// User Data folder (profiles inside) — or the single profile for Opera.
    data: Option<PathBuf>,
    /// Where Opera keeps its caches (LocalAppData mirror of the profile).
    cache_mirror: Option<PathBuf>,
}

fn chromium_browsers() -> Vec<Chromium> {
    let local = env("LOCALAPPDATA");
    let roaming = env("APPDATA");
    let l = |p: &str| local.as_ref().map(|b| b.join(p));
    let r = |p: &str| roaming.as_ref().map(|b| b.join(p));
    vec![
        Chromium { key: "chrome", name: "Google Chrome", exe: "chrome.exe", path_hint: None, data: l(r"Google\Chrome\User Data"), cache_mirror: None },
        Chromium { key: "edge", name: "Microsoft Edge", exe: "msedge.exe", path_hint: None, data: l(r"Microsoft\Edge\User Data"), cache_mirror: None },
        Chromium { key: "brave", name: "Brave", exe: "brave.exe", path_hint: None, data: l(r"BraveSoftware\Brave-Browser\User Data"), cache_mirror: None },
        Chromium { key: "vivaldi", name: "Vivaldi", exe: "vivaldi.exe", path_hint: None, data: l(r"Vivaldi\User Data"), cache_mirror: None },
        Chromium {
            key: "opera",
            name: "Opera",
            exe: "opera.exe",
            path_hint: Some(r"\opera\"),
            data: r(r"Opera Software\Opera Stable"),
            cache_mirror: l(r"Opera Software\Opera Stable"),
        },
        Chromium {
            key: "operagx",
            name: "Opera GX",
            exe: "opera.exe",
            path_hint: Some(r"\opera gx\"),
            data: r(r"Opera Software\Opera GX Stable"),
            cache_mirror: l(r"Opera Software\Opera GX Stable"),
        },
    ]
}

/// Profile folders of a Chromium browser (each with its own History, cache...).
fn chromium_profiles(b: &Chromium) -> Vec<(PathBuf, Option<PathBuf>)> {
    let Some(data) = &b.data else { return Vec::new() };
    if !data.is_dir() {
        return Vec::new();
    }
    if b.cache_mirror.is_some() {
        // Opera: the folder itself is the profile (newer versions use Default inside).
        let (p, c) = if data.join("Default").is_dir() {
            (data.join("Default"), b.cache_mirror.as_ref().map(|m| m.join("Default")))
        } else {
            (data.clone(), b.cache_mirror.clone())
        };
        return vec![(p, c)];
    }
    let Ok(rd) = std::fs::read_dir(data) else { return Vec::new() };
    let mut out: Vec<(PathBuf, Option<PathBuf>)> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            n == "Default" || n.starts_with("Profile ")
        })
        .map(|e| (e.path(), None))
        .collect();
    out.sort();
    out
}

fn firefox_profiles() -> Vec<(PathBuf, Option<PathBuf>)> {
    let (Some(roaming), local) = (env("APPDATA"), env("LOCALAPPDATA")) else { return Vec::new() };
    let base = roaming.join(r"Mozilla\Firefox\Profiles");
    let Ok(rd) = std::fs::read_dir(&base) else { return Vec::new() };
    rd.flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| {
            let cache = local.as_ref().map(|l| l.join(r"Mozilla\Firefox\Profiles").join(e.file_name()));
            (e.path(), cache)
        })
        .collect()
}

/// Installed programs, read once per run (used to name discovered caches and
/// to know which executable belongs to them).
fn installed_index() -> &'static Vec<(String, String, Option<String>)> {
    static INDEX: std::sync::OnceLock<Vec<(String, String, Option<String>)>> = std::sync::OnceLock::new();
    INDEX.get_or_init(|| {
        crate::programs::registry_programs()
            .into_iter()
            .filter(|p| !p.system_component && !p.is_update)
            .map(|p| {
                let exe = p.icon.as_deref().and_then(crate::programs::parse_icon_ref).map(|(path, _)| path).filter(|p| p.to_lowercase().ends_with(".exe"));
                (crate::appsize::norm(&p.name), p.name.clone(), exe)
            })
            .filter(|(n, _, _)| n.len() >= 3)
            .collect()
    })
}

/// Folders inside AppData that are caches of some program.
const CACHE_DIRS: &[&str] = &["Cache", "Code Cache", "GPUCache", "CachedData", "GrShaderCache", "ShaderCache", "Cache_Data", "blob_storage"];
/// AppData folders that belong to Windows, to the browsers already covered
/// above, or that are not caches at all.
const SKIP_APPDATA: &[&str] = &[
    "microsoft", "packages", "temp", "programs", "google", "bravesoftware", "mozilla", "opera software", "vivaldi", "chromium", "discord", "discordptb",
    "discordcanary", "spotify", "steam", "code", "hdcleaner", "nvidia", "amd", "intel", "d3dscache", "crashdumps", "connecteddevicesplatform", "comms",
    "publishers", "windows", "elevateddiagnostics", "iconcache", "application data", "history", "cookies",
    // This application itself (WebView2 data of the current and former name).
    "app.hdcleaner.desktop", "app.nexuscleaner.desktop",
];

/// Cache folders of programs that are not in the fixed list above: any
/// `%AppData%\<app>\Cache`-style folder, named after the installed program
/// when one matches.
fn discovered_app_caches(covered: &std::collections::HashSet<String>) -> Vec<Category> {
    let mut out: Vec<Category> = Vec::new();
    for base in [env("APPDATA"), env("LOCALAPPDATA")].into_iter().flatten() {
        let Ok(rd) = std::fs::read_dir(&base) else { continue };
        for e in rd.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())) {
            let folder = e.file_name().to_string_lossy().to_string();
            if SKIP_APPDATA.contains(&folder.to_lowercase().as_str()) {
                continue;
            }
            // Cache folders directly inside, or one level deeper (Electron apps).
            let mut roots: Vec<PathBuf> = CACHE_DIRS.iter().map(|c| e.path().join(c)).filter(|p| p.is_dir()).collect();
            if let Ok(sub) = std::fs::read_dir(e.path()) {
                for s in sub.flatten().filter(|s| s.file_type().is_ok_and(|t| t.is_dir())).take(40) {
                    roots.extend(CACHE_DIRS.iter().map(|c| s.path().join(c)).filter(|p| p.is_dir()));
                }
            }
            roots.retain(|p| {
                let l = s(p).to_lowercase();
                let already = covered.iter().any(|c| l == *c || l.starts_with(&format!("{c}\\")));
                // Empty cache folders would only add noise to the list.
                let has_content = std::fs::read_dir(p).map(|mut d| d.next().is_some()).unwrap_or(false);
                !already && has_content
            });
            if roots.is_empty() {
                continue;
            }
            // Name it after the installed program when one matches the folder.
            let norm = crate::appsize::norm(&folder);
            let program = installed_index().iter().find(|(n, _, _)| *n == norm || n.starts_with(&norm) && norm.len() >= 4);
            let mut procs = vec![format!("{folder}.exe")];
            if let Some((_, _, Some(exe))) = program {
                if let Some(name) = exe.rsplit('\\').next() {
                    if !procs.iter().any(|p| p.eq_ignore_ascii_case(name)) {
                        procs.push(name.to_string());
                    }
                }
            }
            let key: String = folder.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
            if key.is_empty() || out.iter().any(|c| c.id == format!("app.{key}")) {
                continue;
            }
            let mut rules: Vec<Rule> = roots.into_iter().map(dir).collect();
            dedup_rules(&mut rules);
            // An installed program with this name: treat it like the known
            // apps. Otherwise the folder alone is a weaker signal, so the
            // category is shown but not pre-selected.
            let (risk, on) = if program.is_some() { (Risk::Safe, true) } else { (Risk::Review, false) };
            let mut c = cat(&format!("app.{key}"), Group::Apps, "appCache", Risk::Review, on, rules);
            c.risk = risk;
            c.owner = Some(program.map(|(_, name, _)| name.clone()).unwrap_or(folder));
            c.processes = procs;
            out.push(c);
        }
    }
    out.sort_by(|a, b| a.owner.cmp(&b.owner));
    out
}

/// Every category that applies to this computer.
pub fn catalog() -> Vec<Category> {
    let mut out = Vec::new();
    let local = env("LOCALAPPDATA");
    let roaming = env("APPDATA");
    let win = PathBuf::from(crate::system::windows_dir());
    let pdata = env("ProgramData").unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));

    // --- System -----------------------------------------------------------
    if let Some(t) = env("TEMP") {
        out.push(cat("userTemp", Group::System, "userTemp", Risk::Safe, true, vec![Rule::Dir { dir: s(&t), recursive: true, min_age_ms: Some(TEMP_MIN_AGE_MS) }]));
    }
    let mut wt = cat("windowsTemp", Group::System, "windowsTemp", Risk::Safe, true, vec![Rule::Dir { dir: s(&win.join("Temp")), recursive: true, min_age_ms: Some(TEMP_MIN_AGE_MS) }]);
    wt.admin = true;
    out.push(wt);
    if let Some(l) = &local {
        let mut c = cat("crashDumps", Group::System, "crashDumps", Risk::Review, false, vec![dir(l.join("CrashDumps"))]);
        c.warning = Some("dumpsWarning".into());
        out.push(c);
        out.push(cat(
            "werReports",
            Group::System,
            "werReports",
            Risk::Safe,
            true,
            vec![dir(l.join(r"Microsoft\Windows\WER\ReportArchive")), dir(l.join(r"Microsoft\Windows\WER\ReportQueue"))],
        ));
        let explorer = l.join(r"Microsoft\Windows\Explorer");
        let mut th = cat(
            "thumbnailCache",
            Group::System,
            "thumbnailCache",
            Risk::Safe,
            false,
            vec![Rule::Match { dir: s(&explorer), prefix: "thumbcache_".into(), suffix: ".db".into() }],
        );
        th.warning = Some("thumbWarning".into());
        out.push(th);
        let mut sh = cat(
            "shaderCache",
            Group::System,
            "shaderCache",
            Risk::Review,
            false,
            vec![dir(l.join("D3DSCache")), dir(l.join(r"NVIDIA\DXCache")), dir(l.join(r"NVIDIA\GLCache")), dir(l.join(r"AMD\DxCache")), dir(l.join(r"AMD\GLCache"))],
        );
        sh.warning = Some("shaderWarning".into());
        out.push(sh);
    }
    let mut sysdumps = cat(
        "systemDumps",
        Group::System,
        "systemDumps",
        Risk::Review,
        false,
        vec![dir(win.join("Minidump")), files(&win, &["MEMORY.DMP"]), dir(win.join("LiveKernelReports"))],
    );
    sysdumps.admin = true;
    sysdumps.warning = Some("dumpsWarning".into());
    out.push(sysdumps);
    let mut syswer = cat(
        "systemWer",
        Group::System,
        "systemWer",
        Risk::Safe,
        true,
        vec![dir(pdata.join(r"Microsoft\Windows\WER\ReportArchive")), dir(pdata.join(r"Microsoft\Windows\WER\ReportQueue"))],
    );
    syswer.admin = true;
    out.push(syswer);
    let mut rb = cat("recycleBin", Group::System, "recycleBin", Risk::Review, false, Vec::new());
    rb.special = Some(Special::RecycleBin);
    rb.warning = Some("recycleWarning".into());
    out.push(rb);

    // --- Known apps (caches and logs; the app must be closed) -------------
    let mut app = |key: &str, name: &str, procs: &[&str], rules: Vec<Rule>| {
        if rules.iter().any(|r| Path::new(r.root()).exists()) {
            let mut c = cat(&format!("app.{key}"), Group::Apps, "appCache", Risk::Safe, true, rules);
            c.owner = Some(name.into());
            c.processes = procs.iter().map(|p| p.to_string()).collect();
            out.push(c);
        }
    };
    if let Some(r) = &roaming {
        for (key, name, folder, exe) in [
            ("discord", "Discord", "discord", "Discord.exe"),
            ("discordptb", "Discord PTB", "discordptb", "DiscordPTB.exe"),
            ("discordcanary", "Discord Canary", "discordcanary", "DiscordCanary.exe"),
        ] {
            let b = r.join(folder);
            app(key, name, &[exe], vec![dir(b.join("Cache")), dir(b.join("Code Cache")), dir(b.join("GPUCache"))]);
        }
        let code = r.join("Code");
        app("vscode", "Visual Studio Code", &["Code.exe"], vec![dir(code.join("Cache")), dir(code.join("CachedData")), dir(code.join("Code Cache")), dir(code.join("GPUCache")), dir(code.join("logs"))]);
        let teams = r.join(r"Microsoft\Teams");
        app("teams", "Microsoft Teams (classic)", &["Teams.exe"], vec![dir(teams.join("Cache")), dir(teams.join("Code Cache")), dir(teams.join("GPUCache")), dir(teams.join("logs"))]);
    }
    if let Some(l) = &local {
        app("spotify", "Spotify", &["Spotify.exe"], vec![dir(l.join(r"Spotify\Storage")), dir(l.join(r"Spotify\Browser\Cache"))]);
        app("steam", "Steam", &["steam.exe", "steamwebhelper.exe"], vec![dir(l.join(r"Steam\htmlcache"))]);
    }

    // --- Browsers ----------------------------------------------------------
    for b in chromium_browsers() {
        let profiles = chromium_profiles(&b);
        if profiles.is_empty() {
            continue;
        }
        let root = b.data.clone().unwrap();
        let mk = |kind: &str, label: &str, risk: Risk, default_on: bool, rules: Vec<Rule>| {
            let mut c = cat(&format!("browser.{}.{kind}", b.key), Group::Browsers, label, risk, default_on, rules);
            c.owner = Some(b.name.into());
            c.processes = vec![b.exe.into()];
            c.process_path_hint = b.path_hint.map(Into::into);
            c
        };
        let mut cache = Vec::new();
        let mut history = Vec::new();
        let mut cookies = Vec::new();
        let mut sessions = Vec::new();
        let mut site = Vec::new();
        let mut dbs = Vec::new();
        for (p, mirror) in &profiles {
            let c = mirror.clone().unwrap_or_else(|| p.clone());
            cache.extend([dir(c.join(r"Cache\Cache_Data")), dir(c.join("Cache")), dir(p.join("Code Cache")), dir(p.join("GPUCache")), dir(c.join("Code Cache")), dir(c.join("GPUCache"))]);
            history.push(files(p, &["History", "History-journal", "Visited Links", "Top Sites", "Top Sites-journal", "Shortcuts", "Shortcuts-journal"]));
            cookies.push(files(p, &["Cookies", "Cookies-journal"]));
            cookies.push(files(&p.join("Network"), &["Cookies", "Cookies-journal"]));
            sessions.push(dir(p.join("Sessions")));
            sessions.push(files(p, &["Current Session", "Current Tabs", "Last Session", "Last Tabs"]));
            site.extend([dir(p.join("Local Storage")), dir(p.join("IndexedDB")), dir(p.join("Session Storage")), dir(p.join(r"Service Worker\CacheStorage"))]);
            dbs.push(s(&p.join("History")));
        }
        // Root-level caches (ShaderCache etc.) for the User Data layout.
        if b.cache_mirror.is_none() {
            cache.extend([dir(root.join("ShaderCache")), dir(root.join("GrShaderCache")), dir(root.join("GraphiteDawnCache"))]);
        }
        dedup_rules(&mut cache);
        out.push(mk("cache", "browserCache", Risk::Safe, true, cache));
        let mut h = mk("history", "browserHistory", Risk::Review, false, history);
        h.warning = Some("historyWarning".into());
        out.push(h);
        let mut d = mk("downloads", "browserDownloads", Risk::Review, false, Vec::new());
        d.special = Some(Special::ChromiumDownloads { databases: dbs });
        out.push(d);
        let mut ck = mk("cookies", "browserCookies", Risk::Dangerous, false, cookies);
        ck.warning = Some("cookiesWarning".into());
        out.push(ck);
        let mut ss = mk("sessions", "browserSessions", Risk::Dangerous, false, sessions);
        ss.warning = Some("sessionsWarning".into());
        out.push(ss);
        let mut sd = mk("siteData", "browserSiteData", Risk::Dangerous, false, site);
        sd.warning = Some("siteDataWarning".into());
        out.push(sd);
    }
    let ff = firefox_profiles();
    if !ff.is_empty() {
        let mk = |kind: &str, label: &str, risk: Risk, default_on: bool, rules: Vec<Rule>| {
            let mut c = cat(&format!("browser.firefox.{kind}"), Group::Browsers, label, risk, default_on, rules);
            c.owner = Some("Mozilla Firefox".into());
            c.processes = vec!["firefox.exe".into()];
            c
        };
        let mut cache = Vec::new();
        let mut cookies = Vec::new();
        let mut sessions = Vec::new();
        let mut site = Vec::new();
        let places: Vec<String> = ff.iter().map(|(p, _)| s(&p.join("places.sqlite"))).filter(|p| Path::new(p).is_file()).collect();
        for (p, c) in &ff {
            if let Some(c) = c {
                cache.extend([dir(c.join("cache2")), dir(c.join("startupCache")), dir(c.join("thumbnails"))]);
            }
            cookies.push(files(p, &["cookies.sqlite", "cookies.sqlite-wal", "cookies.sqlite-shm"]));
            sessions.push(files(p, &["sessionstore.jsonlz4"]));
            sessions.push(dir(p.join("sessionstore-backups")));
            site.push(dir(p.join(r"storage\default")));
        }
        out.push(mk("cache", "browserCache", Risk::Safe, true, cache));
        if !places.is_empty() {
            let mut h = mk("history", "browserHistory", Risk::Review, false, Vec::new());
            h.special = Some(Special::FirefoxHistory { databases: places.clone() });
            h.warning = Some("historyWarning".into());
            out.push(h);
            let mut d = mk("downloads", "browserDownloads", Risk::Review, false, Vec::new());
            d.special = Some(Special::FirefoxDownloads { databases: places });
            out.push(d);
        }
        let mut ck = mk("cookies", "browserCookies", Risk::Dangerous, false, cookies);
        ck.warning = Some("cookiesWarning".into());
        out.push(ck);
        let mut ss = mk("sessions", "browserSessions", Risk::Dangerous, false, sessions);
        ss.warning = Some("sessionsWarning".into());
        out.push(ss);
        let mut sd = mk("siteData", "browserSiteData", Risk::Dangerous, false, site);
        sd.warning = Some("siteDataWarning".into());
        out.push(sd);
    }

    // --- Privacy: recent items --------------------------------------------
    if let Some(r) = &roaming {
        let recent = r.join(r"Microsoft\Windows\Recent");
        out.push(cat(
            "privacy.recentFiles",
            Group::Privacy,
            "recentFiles",
            Risk::Review,
            false,
            vec![Rule::Match { dir: s(&recent), prefix: String::new(), suffix: ".lnk".into() }],
        ));
        let mut jl = cat(
            "privacy.jumpLists",
            Group::Privacy,
            "jumpLists",
            Risk::Review,
            false,
            vec![
                Rule::Match { dir: s(&recent.join("AutomaticDestinations")), prefix: String::new(), suffix: ".automaticDestinations-ms".into() },
                Rule::Match { dir: s(&recent.join("CustomDestinations")), prefix: String::new(), suffix: ".customDestinations-ms".into() },
            ],
        );
        jl.warning = Some("jumpListWarning".into());
        out.push(jl);
    }
    let mru = |id: &str, label: &str, keys: Vec<String>| {
        let mut c = cat(id, Group::Privacy, label, Risk::Review, false, Vec::new());
        c.special = Some(Special::RegistryMru { keys });
        c
    };
    // Caches of any other program with an AppData cache folder.
    let covered: std::collections::HashSet<String> = out.iter().flat_map(|c| c.rules.iter().map(|r| r.root().to_lowercase())).collect();
    let discovered = discovered_app_caches(&covered);
    let apps_end = out.iter().rposition(|c| c.group == Group::Apps).map(|i| i + 1).unwrap_or(out.len());
    for (i, c) in discovered.into_iter().enumerate() {
        out.insert(apps_end + i, c);
    }

    out.push(mru("privacy.runMru", "runMru", vec![RUN_MRU.into()]));
    out.push(mru("privacy.typedPaths", "typedPaths", vec![TYPED_PATHS.into()]));
    let office = office_mru_keys();
    if !office.is_empty() {
        out.push(mru("privacy.officeMru", "officeMru", office));
    }
    out
}

fn dedup_rules(rules: &mut Vec<Rule>) {
    let mut seen = std::collections::HashSet::new();
    rules.retain(|r| seen.insert(r.root().to_lowercase()));
    // A folder inside another listed folder would be counted twice.
    let roots: Vec<String> = rules.iter().map(|r| r.root().to_lowercase()).collect();
    rules.retain(|r| {
        let me = r.root().to_lowercase();
        !roots.iter().any(|o| o != &me && me.starts_with(&format!("{o}\\")))
    });
}

// ---------------------------------------------------------------------------
// Registry MRU lists
// ---------------------------------------------------------------------------

const RUN_MRU: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\RunMRU";
const TYPED_PATHS: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\TypedPaths";

/// Office "File MRU" / "Place MRU" keys of the current user.
fn office_mru_keys() -> Vec<String> {
    let mut out = Vec::new();
    for ver in ["16.0", "15.0", "14.0"] {
        for app in ["Word", "Excel", "PowerPoint", "Access", "Publisher", "Visio"] {
            let base = format!(r"Software\Microsoft\Office\{ver}\{app}");
            for list in ["File MRU", "Place MRU"] {
                if Key::open(Hive::CurrentUser, &format!(r"{base}\{list}"), View::Default).is_some() {
                    out.push(format!(r"{base}\{list}"));
                }
            }
            if let Some(um) = Key::open(Hive::CurrentUser, &format!(r"{base}\User MRU"), View::Default) {
                for id in um.subkeys() {
                    for list in ["File MRU", "Place MRU"] {
                        let k = format!(r"{base}\User MRU\{id}\{list}");
                        if Key::open(Hive::CurrentUser, &k, View::Default).is_some() {
                            out.push(k);
                        }
                    }
                }
            }
        }
    }
    out
}

/// Only these key shapes can be emptied (checked again before removal).
fn mru_key_allowed(key: &str) -> bool {
    let k = key.to_lowercase();
    if k.contains("..") {
        return false;
    }
    k == RUN_MRU.to_lowercase()
        || k == TYPED_PATHS.to_lowercase()
        || (k.starts_with(r"software\microsoft\office\") && (k.ends_with(r"\file mru") || k.ends_with(r"\place mru")))
}

/// Which values of an MRU key are list entries (settings like "Max Display" stay).
fn mru_value_is_entry(key: &str, name: &str) -> bool {
    let k = key.to_lowercase();
    let n = name.to_lowercase();
    if k.ends_with(r"\runmru") {
        return n == "mrulist" || (n.len() == 1 && n.chars().all(|c| c.is_ascii_lowercase()));
    }
    if k.ends_with(r"\typedpaths") {
        return n.starts_with("url");
    }
    n.starts_with("item ")
}

fn mru_display(key: &str, data: &str) -> String {
    if key.to_lowercase().ends_with(r"\runmru") {
        return data.trim_end_matches("\\1").to_string();
    }
    // Office: "[F00000000][T01D...][O00000000]*C:\path\file.docx"
    match data.rsplit_once('*') {
        Some((_, p)) => p.to_string(),
        None => data.to_string(),
    }
}

fn mru_items(keys: &[String]) -> Vec<CleanItem> {
    let mut out = Vec::new();
    for key in keys {
        let Some(k) = Key::open(Hive::CurrentUser, key, View::Default) else { continue };
        for name in k.value_names() {
            if !mru_value_is_entry(key, &name) {
                continue;
            }
            let data = k.string(&name).unwrap_or_default();
            let display = if name.eq_ignore_ascii_case("MRUList") { None } else { Some(mru_display(key, &data)) };
            out.push(CleanItem { path: format!(r"HKCU\{key} → {name}"), size: 0, mtime: 0, display });
        }
    }
    out
}

fn split_reg_item(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix(r"HKCU\")?;
    rest.split_once(" → ")
}

// ---------------------------------------------------------------------------
// Analysis
// ---------------------------------------------------------------------------

fn file_attrs(md: &std::fs::Metadata) -> u32 {
    use std::os::windows::fs::MetadataExt;
    md.file_attributes()
}

const REPARSE: u32 = 0x400;

fn mtime_of(md: &std::fs::Metadata) -> u64 {
    use std::os::windows::fs::MetadataExt;
    md.last_write_time()
}

fn walk(dir: &Path, recursive: bool, min_age_ms: Option<i64>, now_ft: u64, out: &mut Vec<CleanItem>, recent: &mut u64) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if out.len() >= MAX_ITEMS {
            return;
        }
        let Ok(md) = e.metadata() else { continue };
        // Never follow or collect links / junctions / mount points.
        if file_attrs(&md) & REPARSE != 0 {
            continue;
        }
        if md.is_dir() {
            if recursive {
                walk(&e.path(), true, min_age_ms, now_ft, out, recent);
            }
            continue;
        }
        let mt = mtime_of(&md);
        if let Some(age) = min_age_ms {
            let age_ft = (age as u64) * 10_000;
            if now_ft.saturating_sub(mt) < age_ft {
                *recent += 1;
                continue;
            }
        }
        out.push(CleanItem { path: s(&e.path()), size: md.len(), mtime: mt, display: None });
    }
}

fn now_filetime() -> u64 {
    crate::util::unix_ms_to_filetime(crate::util::now_unix_ms())
}

fn rule_items(rule: &Rule, now_ft: u64, out: &mut Vec<CleanItem>, recent: &mut u64) {
    match rule {
        Rule::Dir { dir, recursive, min_age_ms } => {
            let d = Path::new(dir);
            // The root itself must be a real folder (not a link somewhere else).
            if std::fs::symlink_metadata(d).is_ok_and(|m| m.is_dir() && file_attrs(&m) & REPARSE == 0) {
                walk(d, *recursive, *min_age_ms, now_ft, out, recent);
            }
        }
        Rule::Files { dir, names } => {
            for n in names {
                let p = Path::new(dir).join(n);
                if let Ok(md) = std::fs::symlink_metadata(&p) {
                    if md.is_file() && file_attrs(&md) & REPARSE == 0 {
                        out.push(CleanItem { path: s(&p), size: md.len(), mtime: mtime_of(&md), display: None });
                    }
                }
            }
        }
        Rule::Match { dir, prefix, suffix } => {
            let Ok(rd) = std::fs::read_dir(dir) else { return };
            let (p, sfx) = (prefix.to_lowercase(), suffix.to_lowercase());
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_lowercase();
                if !name.starts_with(&p) || !name.ends_with(&sfx) {
                    continue;
                }
                let Ok(md) = e.metadata() else { continue };
                if md.is_file() && file_attrs(&md) & REPARSE == 0 {
                    out.push(CleanItem { path: s(&e.path()), size: md.len(), mtime: mtime_of(&md), display: None });
                }
            }
        }
    }
}

/// Whether a browser/app of the category is running now.
pub fn is_running(c: &Category, procs: &[crate::processes::ProcInfo]) -> bool {
    if c.processes.is_empty() {
        return false;
    }
    procs.iter().any(|p| {
        c.processes.iter().any(|n| n.eq_ignore_ascii_case(&p.name))
            && c.process_path_hint.as_ref().is_none_or(|h| p.path.as_deref().is_some_and(|x| x.to_lowercase().contains(h.as_str())))
    })
}

/// Processes of the browser/app a category belongs to.
pub fn running_processes(c: &Category, procs: &[crate::processes::ProcInfo]) -> Vec<crate::processes::ProcInfo> {
    procs
        .iter()
        .filter(|p| {
            c.processes.iter().any(|n| n.eq_ignore_ascii_case(&p.name))
                && c.process_path_hint.as_ref().is_none_or(|h| p.path.as_deref().is_some_and(|x| x.to_lowercase().contains(h.as_str())))
        })
        .cloned()
        .collect()
}

/// Ask the category's program to close (like clicking its X) and wait a
/// little. Nothing is forced: a program with unsaved work stays open and is
/// reported back to the caller.
pub fn close_program(id: &str, wait: std::time::Duration) -> Result<Vec<crate::processes::ProcInfo>> {
    let c = catalog().into_iter().find(|c| c.id == id).ok_or_else(|| AppError::NotFound { path: format!("cleaner category {id}") })?;
    if c.processes.is_empty() {
        return Err(AppError::NotSupported("this category has no program to close".into()));
    }
    let running = running_processes(&c, &crate::processes::list());
    let pids: Vec<u32> = running.iter().map(|p| p.pid).collect();
    for pid in &pids {
        crate::processes::request_close(*pid);
    }
    let still = crate::processes::wait_for_exit(&pids, wait);
    Ok(running.into_iter().filter(|p| still.contains(&p.pid)).collect())
}

/// SID of the current user (its Recycle Bin folder is named after it).
fn current_user_sid() -> Option<String> {
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let token = crate::util::OwnedHandle::new(token)?;
        let mut buf = vec![0u8; 256];
        let mut len = 0u32;
        if GetTokenInformation(token.raw(), TokenUser, buf.as_mut_ptr().cast(), buf.len() as u32, &mut len) == 0 {
            return None;
        }
        let sid = (*(buf.as_ptr() as *const TOKEN_USER)).User.Sid;
        let mut out = std::ptr::null_mut();
        if ConvertSidToStringSidW(sid, &mut out) == 0 {
            return None;
        }
        let mut n = 0;
        while *out.add(n) != 0 {
            n += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(out, n));
        windows_sys::Win32::Foundation::LocalFree(out.cast());
        Some(s)
    }
}

/// The current user's Recycle Bin folders, one per fixed drive.
fn recycle_dirs() -> Vec<PathBuf> {
    let Some(sid) = current_user_sid() else { return Vec::new() };
    crate::disk::list_drives()
        .into_iter()
        .filter(|d| d.kind == crate::disk::DriveKind::Fixed)
        .map(|d| Path::new(&d.root).join(r"$Recycle.Bin").join(&sid))
        .filter(|p| p.is_dir())
        .collect()
}

/// Original path and size of a recycled item, from its `$I` metadata file.
fn recycled_info(meta: &Path) -> Option<(String, u64)> {
    let data = std::fs::read(meta).ok()?;
    if data.len() < 24 {
        return None;
    }
    let version = u64::from_le_bytes(data[0..8].try_into().ok()?);
    let size = u64::from_le_bytes(data[8..16].try_into().ok()?);
    let name = match version {
        // Vista/7: fixed 260 UTF-16 characters. Windows 10: length prefix.
        1 => data.get(24..24 + 520)?,
        2 => {
            let chars = u32::from_le_bytes(data[24..28].try_into().ok()?) as usize;
            data.get(28..28 + chars.min(32_768) * 2)?
        }
        _ => return None,
    };
    let units: Vec<u16> = name.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let end = units.iter().position(|&c| c == 0).unwrap_or(units.len());
    Some((String::from_utf16_lossy(&units[..end]), size))
}

/// Items in the current user's Recycle Bin (each with its original path).
fn recycle_items() -> Vec<CleanItem> {
    let mut out = Vec::new();
    for dir in recycle_dirs() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Some(rest) = name.strip_prefix("$I") else { continue };
            let data = dir.join(format!("$R{rest}"));
            if !data.exists() {
                continue;
            }
            let (original, size) = recycled_info(&e.path()).unwrap_or_else(|| (data.to_string_lossy().to_string(), 0));
            out.push(CleanItem { path: s(&data), size, mtime: 0, display: Some(original) });
        }
    }
    out
}

/// Remove recycled items (the `$R` data and its `$I` metadata).
fn empty_recycle_items(items: &[CleanItem], dry_run: bool) -> CleanOutcome {
    let mut o = CleanOutcome::default();
    let dirs: Vec<String> = recycle_dirs().iter().map(|d| s(d).to_lowercase()).collect();
    for it in items {
        let p = it.path.to_lowercase();
        let file = Path::new(&it.path).file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        let inside = dirs.iter().any(|d| within(&it.path, d));
        if !inside || !file.starts_with("$r") || p.contains(r"\..\") {
            o.failed += 1;
            o.errors.push(format!("{}: outside the Recycle Bin", it.path));
            continue;
        }
        if dry_run {
            o.removed += 1;
            o.freed += it.size;
            continue;
        }
        let md = match std::fs::symlink_metadata(&it.path) {
            Ok(m) => m,
            Err(_) => continue, // already gone
        };
        let r = if md.is_dir() && file_attrs(&md) & REPARSE == 0 { std::fs::remove_dir_all(&it.path) } else { std::fs::remove_file(&it.path) };
        match r {
            Ok(()) => {
                o.removed += 1;
                o.freed += it.size;
                // The metadata file of the same item.
                if let Some(rest) = file.strip_prefix("$r") {
                    let meta = Path::new(&it.path).with_file_name(format!("$I{}", &it.path[it.path.len() - rest.len()..]));
                    let _ = std::fs::remove_file(meta);
                }
            }
            Err(e) if e.raw_os_error() == Some(32) => o.in_use += 1,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => o.denied += 1,
            Err(e) => {
                o.failed += 1;
                o.errors.push(format!("{}: {e}", it.path));
            }
        }
    }
    if !dry_run && o.removed > 0 {
        // Let Explorer refresh its Recycle Bin view.
        use windows_sys::Win32::UI::Shell::{SHChangeNotify, SHCNE_UPDATEDIR, SHCNF_FLUSH, SHCNF_PATHW};
        for d in recycle_dirs() {
            let w = wide(&d);
            unsafe { SHChangeNotify(SHCNE_UPDATEDIR as i32, SHCNF_PATHW | SHCNF_FLUSH, w.as_ptr().cast(), std::ptr::null()) };
        }
    }
    o
}

fn chromium_download_items(dbs: &[String]) -> Result<Vec<CleanItem>> {
    let mut out = Vec::new();
    for db in dbs {
        if !Path::new(db).is_file() {
            continue;
        }
        // Read-only, and immutable so an open browser's lock is not taken.
        let uri = format!("file:{}?mode=ro&immutable=1", db.replace('\\', "/").replace('?', "%3f").replace('#', "%23"));
        let conn = rusqlite::Connection::open_with_flags(&uri, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI)
            .map_err(|e| AppError::Corrupt(format!("{db}: {e}")))?;
        let mut st = match conn.prepare("SELECT id, target_path FROM downloads ORDER BY id DESC LIMIT 5000") {
            Ok(s) => s,
            Err(_) => continue, // not a Chromium History database
        };
        let rows = st
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?)))
            .map_err(|e| AppError::Corrupt(e.to_string()))?;
        for (id, target) in rows.flatten() {
            out.push(CleanItem { path: format!("{db}#download:{id}"), size: 0, mtime: 0, display: Some(target.unwrap_or_default()) });
        }
    }
    Ok(out)
}

/// Open a SQLite database without taking a lock (analysis is read-only).
fn open_readonly(db: &str) -> Result<rusqlite::Connection> {
    let uri = format!("file:{}?mode=ro&immutable=1", db.replace('\\', "/").replace('?', "%3f").replace('#', "%23"));
    rusqlite::Connection::open_with_flags(&uri, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI)
        .map_err(|e| AppError::Corrupt(format!("{db}: {e}")))
}

/// Visited pages of Firefox profiles (`places.sqlite`).
fn firefox_history_items(dbs: &[String]) -> Result<Vec<CleanItem>> {
    let mut out = Vec::new();
    for db in dbs {
        let conn = open_readonly(db)?;
        let Ok(mut st) = conn.prepare(
            "SELECT p.id, p.url, COUNT(v.id) FROM moz_places p JOIN moz_historyvisits v ON v.place_id = p.id
             GROUP BY p.id ORDER BY MAX(v.visit_date) DESC LIMIT 20000",
        ) else {
            continue; // not a places database
        };
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))).map_err(|e| AppError::Corrupt(e.to_string()))?;
        for (id, url) in rows.flatten() {
            out.push(CleanItem { path: format!("{db}#place:{id}"), size: 0, mtime: 0, display: Some(url) });
        }
    }
    Ok(out)
}

/// Download entries of Firefox profiles (annotations on visited pages).
fn firefox_download_items(dbs: &[String]) -> Result<Vec<CleanItem>> {
    let mut out = Vec::new();
    for db in dbs {
        let conn = open_readonly(db)?;
        let Ok(mut st) = conn.prepare(
            "SELECT a.id, a.content FROM moz_annos a JOIN moz_anno_attributes n ON n.id = a.anno_attribute_id
             WHERE n.name = 'downloads/destinationFileURI' ORDER BY a.id DESC LIMIT 5000",
        ) else {
            continue;
        };
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?))).map_err(|e| AppError::Corrupt(e.to_string()))?;
        for (id, uri) in rows.flatten() {
            let display = uri.map(|u| u.strip_prefix("file:///").map(|p| p.replace('/', "\\")).unwrap_or(u)).unwrap_or_default();
            out.push(CleanItem { path: format!("{db}#anno:{id}"), size: 0, mtime: 0, display: Some(display) });
        }
    }
    Ok(out)
}

/// Group `db#tag:id` items by database.
fn by_database<'a>(items: &'a [CleanItem], tag: &str) -> std::collections::BTreeMap<&'a str, Vec<i64>> {
    let mut out: std::collections::BTreeMap<&str, Vec<i64>> = Default::default();
    let sep = format!("#{tag}:");
    for it in items {
        if let Some((db, id)) = it.path.split_once(&sep) {
            if let Ok(id) = id.parse::<i64>() {
                out.entry(db).or_default().push(id);
            }
        }
    }
    out
}

/// A consistent copy of a SQLite database, made before changing it.
fn backup_database(db: &str, backup_dir: &Path) -> Result<()> {
    crate::util::ensure_dir(backup_dir)?;
    let name: String = Path::new(db)
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| format!("{}-", n.to_string_lossy()))
        .unwrap_or_default();
    let file = Path::new(db).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "database.sqlite".into());
    // A second category of the same profile must not overwrite the first backup.
    let mut dest = backup_dir.join(format!("{name}{file}"));
    for n in 2..100 {
        if !dest.exists() {
            break;
        }
        dest = backup_dir.join(format!("{name}{file}.{n}"));
    }
    let conn = rusqlite::Connection::open(db).map_err(|e| AppError::Corrupt(format!("{db}: {e}")))?;
    conn.execute("VACUUM INTO ?1", [dest.to_string_lossy().to_string()])
        .map(|_| ())
        .map_err(|e| AppError::Corrupt(format!("backing up {db}: {e}")))
}

/// Remove visits (and the pages left without visits and without bookmarks).
fn clear_firefox(items: &[CleanItem], tag: &str, backup_dir: &Path, dry_run: bool) -> CleanOutcome {
    let mut o = CleanOutcome::default();
    for (db, ids) in by_database(items, tag) {
        if dry_run {
            o.removed += ids.len() as u64;
            continue;
        }
        if let Err(e) = backup_database(db, backup_dir) {
            // Without a backup nothing is touched.
            o.failed += ids.len() as u64;
            o.errors.push(e.to_string());
            continue;
        }
        let r = (|| -> rusqlite::Result<usize> {
            let mut conn = rusqlite::Connection::open(db)?;
            conn.busy_timeout(std::time::Duration::from_millis(800))?;
            let tx = conn.transaction()?;
            let mut n = 0;
            for id in &ids {
                if tag == "place" {
                    tx.execute("DELETE FROM moz_historyvisits WHERE place_id = ?1", [id])?;
                    let _ = tx.execute("DELETE FROM moz_inputhistory WHERE place_id = ?1", [id]);
                    // Bookmarked or annotated pages have foreign_count > 0 and stay.
                    n += tx.execute("DELETE FROM moz_places WHERE id = ?1 AND foreign_count = 0", [id])?;
                } else {
                    let place: Option<i64> = tx.query_row("SELECT place_id FROM moz_annos WHERE id = ?1", [id], |r| r.get(0)).ok();
                    n += tx.execute("DELETE FROM moz_annos WHERE id = ?1", [id])?;
                    if let Some(p) = place {
                        // The metadata annotation of the same download goes too.
                        let _ = tx.execute(
                            "DELETE FROM moz_annos WHERE place_id = ?1 AND anno_attribute_id IN
                             (SELECT id FROM moz_anno_attributes WHERE name LIKE 'downloads/%')",
                            [p],
                        );
                    }
                }
            }
            tx.commit()?;
            Ok(n)
        })();
        match r {
            Ok(n) => o.removed += n as u64,
            Err(e) => {
                o.in_use += ids.len() as u64;
                o.errors.push(format!("{db}: {e}"));
            }
        }
    }
    o
}

pub fn analyze_category(c: &Category, procs: &[crate::processes::ProcInfo]) -> CategoryResult {
    let mut r = CategoryResult {
        category: c.clone(),
        items: Vec::new(),
        count: 0,
        bytes: 0,
        running: is_running(c, procs),
        recent_skipped: 0,
        admin_pending: c.admin && !crate::system::is_elevated(),
        truncated: false,
        error: None,
    };
    if r.admin_pending {
        // C:\Windows\Temp and friends cannot be listed by a standard user.
        return r;
    }
    match &c.special {
        Some(Special::RecycleBin) => r.items = recycle_items(),
        Some(Special::ChromiumDownloads { databases }) => match chromium_download_items(databases) {
            Ok(items) => r.items = items,
            Err(e) => r.error = Some(e.to_payload()),
        },
        Some(Special::RegistryMru { keys }) => r.items = mru_items(keys),
        Some(Special::FirefoxHistory { databases }) => match firefox_history_items(databases) {
            Ok(items) => r.items = items,
            Err(e) => r.error = Some(e.to_payload()),
        },
        Some(Special::FirefoxDownloads { databases }) => match firefox_download_items(databases) {
            Ok(items) => r.items = items,
            Err(e) => r.error = Some(e.to_payload()),
        },
        None => {
            let now = now_filetime();
            let mut recent = 0;
            for rule in &c.rules {
                rule_items(rule, now, &mut r.items, &mut recent);
            }
            // Rules may overlap (Cache and Cache\Cache_Data): list each file once.
            let mut seen = std::collections::HashSet::new();
            r.items.retain(|i| seen.insert(i.path.to_lowercase()));
            r.recent_skipped = recent;
        }
    }
    r.truncated = r.items.len() >= MAX_ITEMS;
    r.count = r.items.len() as u64;
    r.bytes = r.items.iter().map(|i| i.size).sum();
    r
}

/// Analyze every category (nothing is removed).
pub fn analyze() -> Vec<CategoryResult> {
    use rayon::prelude::*;
    let procs = crate::processes::list();
    catalog().par_iter().map(|c| analyze_category(c, &procs)).collect()
}

/// What the elevated helper sends back for a machine-wide category.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AdminAnalysis {
    pub id: String,
    pub items: Vec<CleanItem>,
    pub recent_skipped: u64,
}

/// Runs inside the elevated helper: list the items of machine-wide categories.
pub fn analyze_admin(ids: &[String]) -> Result<Vec<AdminAnalysis>> {
    let procs = crate::processes::list();
    let mut out = Vec::new();
    for c in catalog().into_iter().filter(|c| ids.contains(&c.id)) {
        if !c.admin {
            return Err(AppError::InvalidInput("only machine-wide categories are analyzed elevated".into()));
        }
        // The helper is elevated, so the category is listed normally here.
        let r = analyze_category(&c, &procs);
        out.push(AdminAnalysis { id: c.id.clone(), items: r.items, recent_skipped: r.recent_skipped });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Cleaning
// ---------------------------------------------------------------------------

fn within(path: &str, root: &str) -> bool {
    let (p, r) = (path.to_lowercase(), root.trim_end_matches('\\').to_lowercase());
    p.starts_with(&r) && p.as_bytes().get(r.len()) == Some(&b'\\') && !p.contains(r"\..\")
}

/// A file item may be removed only if it matches one of the category rules.
fn item_allowed(c: &Category, path: &str) -> bool {
    let name = Path::new(path).file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let parent = Path::new(path).parent().map(s).unwrap_or_default();
    c.rules.iter().any(|r| match r {
        Rule::Dir { dir, recursive: true, .. } => within(path, dir),
        Rule::Dir { dir, recursive: false, .. } => parent.eq_ignore_ascii_case(dir.trim_end_matches('\\')),
        Rule::Files { dir, names } => parent.eq_ignore_ascii_case(dir.trim_end_matches('\\')) && names.iter().any(|n| n.eq_ignore_ascii_case(&name)),
        Rule::Match { dir, prefix, suffix } => {
            parent.eq_ignore_ascii_case(dir.trim_end_matches('\\')) && name.starts_with(&prefix.to_lowercase()) && name.ends_with(&suffix.to_lowercase())
        }
    })
}

fn remove_files(c: &Category, items: &[CleanItem], dry_run: bool, progress: &mut dyn FnMut(u64)) -> CleanOutcome {
    let mut o = CleanOutcome::default();
    for (n, it) in items.iter().enumerate() {
        if n % 200 == 0 {
            progress(n as u64);
        }
        if !item_allowed(c, &it.path) {
            o.failed += 1;
            o.errors.push(format!("{}: outside the category", it.path));
            continue;
        }
        let md = match std::fs::symlink_metadata(&it.path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue, // already gone
            Err(_) => {
                o.denied += 1;
                continue;
            }
        };
        if !md.is_file() || file_attrs(&md) & REPARSE != 0 || md.len() != it.size || mtime_of(&md) != it.mtime {
            o.changed += 1;
            continue;
        }
        if dry_run {
            o.removed += 1;
            o.freed += it.size;
            continue;
        }
        match std::fs::remove_file(&it.path) {
            Ok(()) => {
                o.removed += 1;
                o.freed += it.size;
            }
            Err(e) if e.raw_os_error() == Some(32) || e.raw_os_error() == Some(33) => o.in_use += 1,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                // Read-only files in caches: clear the attribute and retry once.
                let mut perm = md.permissions();
                #[allow(clippy::permissions_set_readonly_false)]
                perm.set_readonly(false);
                if std::fs::set_permissions(&it.path, perm).is_ok() && std::fs::remove_file(&it.path).is_ok() {
                    o.removed += 1;
                    o.freed += it.size;
                } else {
                    o.denied += 1;
                }
            }
            Err(e) => {
                o.failed += 1;
                if o.errors.len() < 50 {
                    o.errors.push(format!("{}: {e}", it.path));
                }
            }
        }
    }
    if !dry_run {
        prune_empty_dirs(c);
    }
    o
}

/// Remove folders left empty inside recursive rules (never the rule root).
fn prune_empty_dirs(c: &Category) {
    fn prune(d: &Path) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        for e in rd.flatten() {
            let Ok(md) = e.metadata() else { continue };
            if md.is_dir() && file_attrs(&md) & REPARSE == 0 {
                prune(&e.path());
                let _ = std::fs::remove_dir(e.path()); // only succeeds when empty
            }
        }
    }
    for r in &c.rules {
        if let Rule::Dir { dir, recursive: true, .. } = r {
            let d = Path::new(dir);
            if std::fs::symlink_metadata(d).is_ok_and(|m| m.is_dir() && file_attrs(&m) & REPARSE == 0) {
                prune(d);
            }
        }
    }
}

fn clear_downloads(items: &[CleanItem], dry_run: bool) -> CleanOutcome {
    let mut o = CleanOutcome::default();
    let mut by_db: std::collections::BTreeMap<&str, Vec<i64>> = Default::default();
    for it in items {
        if let Some((db, id)) = it.path.split_once("#download:") {
            if let Ok(id) = id.parse::<i64>() {
                by_db.entry(db).or_default().push(id);
            }
        }
    }
    for (db, ids) in by_db {
        if !db.to_lowercase().ends_with(r"\history") {
            o.failed += ids.len() as u64;
            continue;
        }
        if dry_run {
            o.removed += ids.len() as u64;
            continue;
        }
        let r = (|| -> rusqlite::Result<usize> {
            let mut conn = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
            conn.busy_timeout(std::time::Duration::from_millis(500))?;
            let tx = conn.transaction()?;
            let mut n = 0;
            for id in &ids {
                tx.execute("DELETE FROM downloads_url_chains WHERE id = ?1", [id])?;
                let _ = tx.execute("DELETE FROM downloads_slices WHERE download_id = ?1", [id]);
                n += tx.execute("DELETE FROM downloads WHERE id = ?1", [id])?;
            }
            tx.commit()?;
            Ok(n)
        })();
        match r {
            Ok(n) => o.removed += n as u64,
            Err(e) => {
                o.in_use += ids.len() as u64;
                o.errors.push(format!("{db}: {e}"));
            }
        }
    }
    o
}

fn clear_mru(items: &[CleanItem], backup_dir: &Path, dry_run: bool) -> CleanOutcome {
    use windows_sys::Win32::System::Registry::*;
    let mut o = CleanOutcome::default();
    let mut targets = Vec::new();
    let mut list = Vec::new();
    for it in items {
        match split_reg_item(&it.path) {
            Some((key, value)) if mru_key_allowed(key) && mru_value_is_entry(key, value) => {
                targets.push(crate::regops::RegTarget { hive: Hive::CurrentUser, path: key.into(), view: View::Default, value: Some(value.into()) });
                list.push((key.to_string(), value.to_string()));
            }
            _ => o.failed += 1,
        }
    }
    if dry_run {
        o.removed = list.len() as u64;
        return o;
    }
    if targets.is_empty() {
        return o;
    }
    if let Err(e) = crate::regops::export(&targets, &backup_dir.join("recent-lists.reg")) {
        // No backup, no removal.
        o.failed += list.len() as u64;
        o.errors.push(e.to_string());
        return o;
    }
    for (key, value) in list {
        let (k, v) = (wide(&key), wide(&value));
        let rc = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), v.as_ptr()) };
        match rc {
            0 | 2 => o.removed += 1,
            5 => o.denied += 1,
            _ => o.failed += 1,
        }
    }
    o
}

/// Clean one category from its analysis. The category is looked up again in
/// the current catalog (the stored definition is not trusted), a running
/// browser/app blocks it, and only analysed items are touched.
pub fn clean_category(id: &str, items: &[CleanItem], backup_dir: &Path, dry_run: bool, progress: &mut dyn FnMut(u64)) -> Result<CleanOutcome> {
    let c = catalog().into_iter().find(|c| c.id == id).ok_or_else(|| AppError::NotFound { path: format!("cleaner category {id}") })?;
    // A dry run changes nothing, so it does not need the rights the real run does.
    if c.admin && !dry_run && !crate::system::is_elevated() {
        return Err(AppError::ElevationRequired(format!("cleaner category {id}")));
    }
    if is_running(&c, &crate::processes::list()) {
        return Err(AppError::InvalidInput(format!("{} is running; close it first", c.owner.as_deref().unwrap_or(id))));
    }
    Ok(match &c.special {
        Some(Special::RecycleBin) => empty_recycle_items(items, dry_run),
        Some(Special::ChromiumDownloads { .. }) => clear_downloads(items, dry_run),
        Some(Special::RegistryMru { .. }) => clear_mru(items, backup_dir, dry_run),
        Some(Special::FirefoxHistory { .. }) => clear_firefox(items, "place", backup_dir, dry_run),
        Some(Special::FirefoxDownloads { .. }) => clear_firefox(items, "anno", backup_dir, dry_run),
        None => remove_files(&c, items, dry_run, progress),
    })
}

/// Runs inside the elevated helper: only machine-wide categories.
pub fn clean_elevated(id: &str, items: &[CleanItem]) -> Result<CleanOutcome> {
    let c = catalog().into_iter().find(|c| c.id == id).ok_or_else(|| AppError::NotFound { path: format!("cleaner category {id}") })?;
    if !c.admin || c.special.is_some() {
        return Err(AppError::InvalidInput("only machine-wide cleaner categories run elevated".into()));
    }
    Ok(remove_files(&c, items, false, &mut |_| {}))
}

/// Merge helper results.
pub fn merge(into: &mut CleanOutcome, other: CleanOutcome) {
    into.merge(other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_sane() {
        let c = catalog();
        let mut ids: Vec<&str> = c.iter().map(|c| c.id.as_str()).collect();
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(n, ids.len(), "unique ids");
        for c in &c {
            // Cookies, sessions and site data are never pre-selected.
            if c.risk == Risk::Dangerous {
                assert!(!c.default_on, "{}", c.id);
            }
            for r in &c.rules {
                let root = r.root().to_lowercase();
                assert!(!root.is_empty());
                let win = crate::system::windows_dir().to_lowercase();
                // Nothing in Windows except its Temp / dumps / reports.
                if root.starts_with(&win) {
                    assert!(c.admin, "{}", c.id);
                    assert!(["\\temp", "\\minidump", "\\livekernelreports"].iter().any(|s| root.ends_with(s)) || root == win, "{root}");
                }
            }
        }
        assert!(c.iter().any(|c| c.id == "userTemp"));
    }

    #[test]
    fn allowed_items() {
        let c = cat("t", Group::System, "t", Risk::Safe, true, vec![
            Rule::Dir { dir: r"C:\X\Cache".into(), recursive: true, min_age_ms: None },
            Rule::Files { dir: r"C:\X\P".into(), names: vec!["History".into()] },
            Rule::Match { dir: r"C:\X\E".into(), prefix: "thumbcache_".into(), suffix: ".db".into() },
        ]);
        assert!(item_allowed(&c, r"C:\X\Cache\a\b.bin"));
        assert!(!item_allowed(&c, r"C:\X\Cache2\b.bin"));
        assert!(!item_allowed(&c, r"C:\X\Cache\..\Other\b.bin"));
        assert!(item_allowed(&c, r"C:\X\P\History"));
        assert!(!item_allowed(&c, r"C:\X\P\Bookmarks"));
        assert!(item_allowed(&c, r"C:\X\E\thumbcache_256.db"));
        assert!(!item_allowed(&c, r"C:\X\E\iconcache_256.db"));
    }

    #[test]
    fn mru_rules() {
        assert!(mru_key_allowed(RUN_MRU));
        assert!(mru_key_allowed(r"Software\Microsoft\Office\16.0\Word\User MRU\ADAL_X\File MRU"));
        assert!(!mru_key_allowed(r"Software\Microsoft\Windows\CurrentVersion\Run"));
        assert!(mru_value_is_entry(RUN_MRU, "a") && mru_value_is_entry(RUN_MRU, "MRUList"));
        assert!(!mru_value_is_entry(r"Software\Microsoft\Office\16.0\Word\File MRU", "Max Display"));
        assert_eq!(mru_display(RUN_MRU, r"cmd\1"), "cmd");
        assert_eq!(mru_display(r"x\File MRU", r"[F00000000][T01D][O00000000]*C:\d\a.docx"), r"C:\d\a.docx");
    }

    #[test]
    fn cleans_only_old_unchanged_files_and_keeps_links() {
        let root = tempfile::tempdir().unwrap();
        let d = root.path().join("cache");
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("old.tmp"), vec![1u8; 100]).unwrap();
        std::fs::write(d.join(r"sub\old2.tmp"), vec![1u8; 50]).unwrap();
        std::fs::write(d.join("changed.tmp"), b"a").unwrap();
        let outside = root.path().join("keep");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("precious.txt"), b"x").unwrap();
        // A junction inside the cache pointing outside must not be followed.
        let j = d.join("link");
        let ok = std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(&j).arg(&outside).output().map(|o| o.status.success()).unwrap_or(false);

        let c = cat("t", Group::System, "t", Risk::Safe, true, vec![Rule::Dir { dir: s(&d), recursive: true, min_age_ms: None }]);
        let r = analyze_category(&c, &[]);
        assert_eq!(r.count, 3, "{:?}", r.items);
        assert!(r.items.iter().all(|i| !i.path.to_lowercase().contains("precious")));
        // Change one file after the analysis: it must be left alone.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(d.join("changed.tmp"), b"abc").unwrap();
        let o = remove_files(&c, &r.items, false, &mut |_| {});
        assert_eq!((o.removed, o.changed, o.freed), (2, 1, 150));
        assert!(d.join("changed.tmp").exists());
        assert!(!d.join("sub").exists(), "emptied folders pruned");
        assert!(d.exists(), "rule root kept");
        assert!(outside.join("precious.txt").exists(), "junction target untouched");
        if ok {
            assert!(j.exists(), "junction itself is not removed");
        }
        // Age filter.
        let young = cat("y", Group::System, "y", Risk::Safe, true, vec![Rule::Dir { dir: s(&d), recursive: true, min_age_ms: Some(TEMP_MIN_AGE_MS) }]);
        let r = analyze_category(&young, &[]);
        assert_eq!((r.count, r.recent_skipped), (0, 1));
    }
}
