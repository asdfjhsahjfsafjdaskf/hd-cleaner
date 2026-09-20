//! "True app size": the space a program really uses, not only what its
//! installer declared.
//!
//! Every location carries a confidence level and a stable reason key:
//! * `Confirmed` — registered by the installer (InstallLocation) or tied to
//!   the package identity (AppX data folder);
//! * `Probable` — folder of the registered uninstaller/icon;
//! * `Possible` — folder whose *name* matches the program in AppData /
//!   ProgramData. Shown separately; never treated as proof of ownership.
//!
//! This module only measures. Destructive decisions never depend on it.

use crate::programs::Program;
use crate::scan::model::{NodeId, ScanTree};
use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum Confidence {
    Confirmed,
    Probable,
    Possible,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LocationKind {
    Install,
    LocalAppData,
    RoamingAppData,
    ProgramData,
    PackageData,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub path: String,
    pub kind: LocationKind,
    pub confidence: Confidence,
    pub reason: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Measured {
    pub total: u64,
    pub cache: u64,
    pub logs: u64,
    pub files: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppLocation {
    #[serde(flatten)]
    pub candidate: Candidate,
    pub measured: Measured,
    /// "scan" when read from a loaded scan, "live" when measured now.
    pub source: &'static str,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppSize {
    pub program_id: String,
    pub locations: Vec<AppLocation>,
    /// Totals over Confirmed + Probable locations.
    pub install: u64,
    pub user_data: u64,
    pub cache: u64,
    pub logs: u64,
    pub total: u64,
    /// Everything in `Possible` locations (reported apart).
    pub possible_total: u64,
    pub reported: Option<u64>,
}

/// Lowercase alphanumerics only ("Discord PTB" → "discordptb").
pub fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

/// Name variants used to match folders: full name, name without trailing
/// version/arch/parenthesised parts, and name without the publisher prefix.
pub fn name_keys(p: &Program) -> Vec<String> {
    let mut keys = Vec::new();
    let full = norm(&p.name);
    if full.len() >= 3 {
        keys.push(full);
    }
    let mut short = String::new();
    let base = p.name.split('(').next().unwrap_or(&p.name);
    for (i, tok) in base.split([' ', '-', '_']).enumerate() {
        let t = tok.trim().to_lowercase();
        // The first word is always part of the name ("7-Zip", "3DMark").
        let versionish = i > 0 && t.chars().next().is_some_and(|c| c.is_ascii_digit())
            || (t.starts_with('v') && t[1..].chars().next().is_some_and(|c| c.is_ascii_digit()))
            || matches!(t.as_str(), "x64" | "x86" | "64-bit" | "32-bit" | "64bit" | "32bit" | "arm64" | "version");
        if versionish {
            break;
        }
        short.push_str(&norm(tok));
    }
    if short.len() >= 3 && !keys.contains(&short) {
        keys.push(short.clone());
    }
    if let Some(publisher) = &p.publisher {
        let first = norm(publisher.split([' ', ',', '.']).next().unwrap_or(""));
        if first.len() >= 3 && short.starts_with(&first) && short.len() >= first.len() + 4 {
            let rest = short[first.len()..].to_string();
            if !keys.contains(&rest) {
                keys.push(rest);
            }
        }
    }
    keys
}

fn list_dirs(root: &str) -> Vec<String> {
    std::fs::read_dir(crate::util::to_extended(root))
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|t| t.is_dir() && !t.is_symlink()).unwrap_or(false))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// Cached listings of the AppData/ProgramData roots (built once per batch).
pub struct Roots {
    roots: Vec<(String, LocationKind, Vec<String>)>,
    local: Option<String>,
}

impl Roots {
    pub fn load() -> Roots {
        let env = |v: &str| std::env::var(v).ok().filter(|s| !s.is_empty());
        let local = env("LOCALAPPDATA");
        let mut roots = Vec::new();
        for (var, kind) in [
            ("LOCALAPPDATA", LocationKind::LocalAppData),
            ("APPDATA", LocationKind::RoamingAppData),
            ("ProgramData", LocationKind::ProgramData),
        ] {
            if let Some(r) = env(var) {
                let names = list_dirs(&r);
                roots.push((r, kind, names));
            }
        }
        if let Some(l) = &local {
            let programs = format!(r"{l}\Programs");
            let names = list_dirs(&programs);
            roots.push((programs, LocationKind::Install, names));
        }
        Roots { roots, local }
    }
}

/// Folders plausibly belonging to `p`, most confident first, without nesting.
pub fn candidates(p: &Program, roots: &Roots) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    if let Some(loc) = &p.install_location {
        out.push(Candidate { path: loc.clone(), kind: LocationKind::Install, confidence: Confidence::Confirmed, reason: "registeredInstallLocation" });
    } else if let Some(loc) = &p.inferred_location {
        out.push(Candidate { path: loc.clone(), kind: LocationKind::Install, confidence: Confidence::Probable, reason: "uninstallerFolder" });
    }
    if let (Some(fam), Some(local)) = (&p.package_family_name, &roots.local) {
        out.push(Candidate {
            path: format!(r"{local}\Packages\{fam}"),
            kind: LocationKind::PackageData,
            confidence: Confidence::Confirmed,
            reason: "packageDataFolder",
        });
    }
    // Name matching only for classic programs: package data is identity-based.
    if p.package_family_name.is_none() {
        let keys = name_keys(p);
        let publisher = p.publisher.as_deref().map(norm).filter(|s| s.len() >= 3);
        for (root, kind, names) in &roots.roots {
            for dir in names {
                let n = norm(dir);
                if keys.iter().any(|k| *k == n) {
                    out.push(Candidate { path: format!(r"{root}\{dir}"), kind: *kind, confidence: Confidence::Possible, reason: "folderNameMatchesProgram" });
                } else if publisher.as_ref().is_some_and(|pb| *pb == n || pb.starts_with(&n) && n.len() >= 4) {
                    // Publisher folder: look one level down for the product.
                    for sub in list_dirs(&format!(r"{root}\{dir}")) {
                        if keys.iter().any(|k| *k == norm(&sub)) {
                            out.push(Candidate {
                                path: format!(r"{root}\{dir}\{sub}"),
                                kind: *kind,
                                confidence: Confidence::Possible,
                                reason: "publisherAndNameFolder",
                            });
                        }
                    }
                }
            }
        }
    }
    // Never count shared containers, and drop nested duplicates (keep the
    // more confident / outer folder).
    out.retain(|c| crate::protection::assess(&c.path).risk != crate::protection::Risk::Blocked);
    out.sort_by(|a, b| a.confidence.cmp(&b.confidence).then(a.path.len().cmp(&b.path.len())));
    let mut kept: Vec<Candidate> = Vec::new();
    for c in out {
        let lc = c.path.to_lowercase();
        let nested = kept.iter().any(|k| {
            let lk = k.path.to_lowercase();
            lc == lk || lc.starts_with(&(lk.clone() + "\\")) || lk.starts_with(&(lc.clone() + "\\"))
        });
        if !nested {
            kept.push(c);
        }
    }
    kept
}

const CACHE_NAMES: &[&str] = &[
    "cache", "caches", "code cache", "gpucache", "shadercache", "dawncache", "dawnwebgpucache", "dawngraphitecache",
    "grshadercache", "graphitedawncache", "cache_data", "cachestorage", "service worker", "temp", "tmp", "crashpad",
    "crashdumps", "crash reports", "blob_storage", "d3dscache", "component_crx_cache", "webcache",
];
const LOG_NAMES: &[&str] = &["logs", "log", "diagnostics"];

/// Size of a subtree split into cache / logs / rest by folder names.
pub fn measure_tree(tree: &ScanTree, node: NodeId) -> Measured {
    let mut m = Measured::default();
    // (node, class) where class: 0 = normal, 1 = cache, 2 = logs
    let mut stack: Vec<(NodeId, u8)> = vec![(node, 0)];
    while let Some((id, class)) = stack.pop() {
        for &c in tree.children(id) {
            let n = tree.node(c);
            if n.is_dir() {
                let lname = tree.name(c).to_lowercase();
                let cls = if class != 0 {
                    class
                } else if CACHE_NAMES.contains(&lname.as_str()) || lname.ends_with("cache") {
                    1
                } else if LOG_NAMES.contains(&lname.as_str()) {
                    2
                } else {
                    0
                };
                stack.push((c, cls));
            } else if n.counts_toward_totals() {
                m.files += 1;
                m.total += n.alloc;
                let ext = tree.ext(c);
                if class == 1 {
                    m.cache += n.alloc;
                } else if class == 2 || ext == "log" || ext == "etl" || ext == "dmp" {
                    m.logs += n.alloc;
                }
            }
        }
    }
    m
}

/// Measure a folder by scanning it now.
pub fn measure_live(path: &str, ctl: &crate::scan::ScanControl) -> Option<Measured> {
    use crate::scan::FileSystemScanner;
    let tree = crate::scan::standard::WindowsApiScanner
        .scan(path, &crate::scan::ScanOptions::default(), ctl)
        .ok()?;
    Some(measure_tree(&tree, crate::scan::ROOT))
}

/// Build the report; `measure` returns (measured, source) for a folder.
pub fn compute(p: &Program, roots: &Roots, mut measure: impl FnMut(&str) -> Option<(Measured, &'static str)>) -> AppSize {
    let mut r = AppSize { program_id: p.id.clone(), reported: p.reported_size, ..Default::default() };
    for c in candidates(p, roots) {
        let Some((m, source)) = measure(&c.path) else { continue };
        if c.confidence == Confidence::Possible {
            r.possible_total += m.total;
        } else {
            r.total += m.total;
            r.cache += m.cache;
            r.logs += m.logs;
            // cache and logs are disjoint by construction
            let rest = m.total.saturating_sub(m.cache).saturating_sub(m.logs);
            if c.kind == LocationKind::Install {
                r.install += rest;
            } else {
                r.user_data += rest;
            }
        }
        r.locations.push(AppLocation { candidate: c, measured: m, source });
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::programs::ProgramSource;

    fn prog(name: &str, publisher: Option<&str>) -> Program {
        let mut p = Program::blank("reg:test".into(), name.into(), ProgramSource::Win32);
        p.publisher = publisher.map(Into::into);
        p
    }

    #[test]
    fn keys() {
        assert_eq!(name_keys(&prog("Discord", None)), vec!["discord"]);
        let k = name_keys(&prog("7-Zip 23.01 (x64)", None));
        assert!(k.contains(&"7zip".to_string()));
        let k = name_keys(&prog("Mozilla Firefox (x64 pt-BR)", Some("Mozilla")));
        assert!(k.contains(&"mozillafirefox".to_string()));
        assert!(k.contains(&"firefox".to_string()));
        assert_eq!(norm("Discord PTB"), "discordptb");
    }

    #[test]
    fn measures_and_classifies_in_sandbox() {
        use crate::scan::FileSystemScanner;
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("MyApp");
        std::fs::create_dir_all(app.join("Cache").join("deep")).unwrap();
        std::fs::create_dir_all(app.join("logs")).unwrap();
        std::fs::write(app.join("Cache").join("deep").join("a.bin"), vec![0u8; 10_000]).unwrap();
        std::fs::write(app.join("logs").join("x.txt"), vec![0u8; 5000]).unwrap();
        std::fs::write(app.join("app.log"), vec![0u8; 3000]).unwrap();
        std::fs::write(app.join("settings.json"), vec![0u8; 2000]).unwrap();
        let tree = crate::scan::standard::WindowsApiScanner
            .scan(app.to_str().unwrap(), &Default::default(), &crate::scan::ScanControl::new())
            .unwrap();
        let m = measure_tree(&tree, crate::scan::ROOT);
        assert_eq!(m.files, 4);
        assert!(m.cache >= 10_000 && m.cache < m.total);
        assert!(m.logs >= 8000);

        // Name-matched AppData folder is only "possible".
        let roots = Roots {
            roots: vec![(dir.path().to_string_lossy().into_owned(), LocationKind::LocalAppData, vec!["MyApp".into(), "Other".into()])],
            local: None,
        };
        let mut p = prog("MyApp 2.1", None);
        let c = candidates(&p, &roots);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].confidence, Confidence::Possible);
        // Registered install location is confirmed and nested matches collapse.
        p.install_location = Some(app.to_string_lossy().into_owned());
        let c = candidates(&p, &roots);
        assert_eq!(c.len(), 1, "same folder is not listed twice");
        assert_eq!(c[0].confidence, Confidence::Confirmed);
    }
}
