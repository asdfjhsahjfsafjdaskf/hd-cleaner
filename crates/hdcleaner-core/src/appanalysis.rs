//! Everything the app knows about one installed program, gathered in one
//! place: where it lives and how much it takes, what of it is running now,
//! what it starts at logon, the registry keys that belong to it, and the
//! caches it leaves behind.
//!
//! This only reads. The two actions the screen offers from here — clearing a
//! cache and disabling a startup entry — go through the same guarded paths as
//! the Cleaner and the Startup page, with the user choosing each time.

use crate::cleaner::{self, CategoryResult};
use crate::correlate::{self, Evidence};
use crate::programs::Program;
use crate::startup::StartupItem;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelatedProcess {
    pub pid: u32,
    pub name: String,
    pub path: Option<String>,
    /// Why this process counts as the program's.
    pub signals: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelatedRegistry {
    pub path: String,
    /// "uninstall" | "vendor" | "startup"
    pub kind: &'static str,
}

/// A cache the program leaves behind, as the Cleaner sees it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelatedCache {
    pub id: String,
    pub label: String,
    pub owner: Option<String>,
    pub count: u64,
    pub bytes: u64,
    /// The app is open: the Cleaner refuses to touch it.
    pub running: bool,
    pub admin: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppAnalysis {
    pub program_id: String,
    pub processes: Vec<RelatedProcess>,
    pub startup: Vec<StartupItem>,
    pub registry: Vec<RelatedRegistry>,
    pub caches: Vec<RelatedCache>,
    pub shortcuts: Vec<String>,
    /// Folders a shortcut or a process pointed at that are not the registered
    /// install location (shown so the user can see where else it lives).
    pub extra_locations: Vec<String>,
}

fn dirs_of(p: &Program) -> Vec<String> {
    [p.install_location.clone(), p.inferred_location.clone()].into_iter().flatten().collect()
}

fn within(path: &str, dir: &str) -> bool {
    let (p, d) = (path.to_lowercase(), dir.trim_end_matches('\\').to_lowercase());
    !d.is_empty() && (p == d || (p.starts_with(&d) && p.as_bytes().get(d.len()) == Some(&b'\\')))
}

/// Processes running from the program's folders right now.
pub fn processes_of(p: &Program, procs: &[crate::processes::ProcInfo]) -> Vec<RelatedProcess> {
    let dirs = dirs_of(p);
    procs
        .iter()
        .filter_map(|pr| {
            let path = pr.path.as_deref()?;
            dirs.iter().any(|d| within(path, d)).then(|| RelatedProcess {
                pid: pr.pid,
                name: pr.name.clone(),
                path: pr.path.clone(),
                signals: vec![correlate::Signal::RunningProcess.key()],
            })
        })
        .collect()
}

/// Startup entries whose command runs a file of this program.
pub fn startup_of(p: &Program, items: &[StartupItem]) -> Vec<StartupItem> {
    let dirs = dirs_of(p);
    let name = crate::appsize::norm(&p.name);
    items
        .iter()
        .filter(|it| {
            let by_path = it.exe.as_deref().is_some_and(|exe| dirs.iter().any(|d| within(exe, d)));
            // A name match alone is only accepted when the program has no
            // folder to compare against (nothing else to go on).
            let by_name = dirs.is_empty() && name.len() >= 3 && crate::appsize::norm(&it.name) == name;
            by_path || by_name
        })
        .cloned()
        .collect()
}

/// The registry keys this program owns: its uninstall entry, its vendor key
/// (only when it holds this product alone) and its Run values.
pub fn registry_of(p: &Program, others: &[Program]) -> Vec<RelatedRegistry> {
    let mut out = Vec::new();
    if let Some(key) = p.registry_key.as_deref() {
        out.push(RelatedRegistry { path: key.to_string(), kind: "uninstall" });
    }
    // The leftover rules already know how to find a program's own keys; here
    // they are only listed, never proposed for removal.
    for l in crate::leftovers::scan(p, others, crate::leftovers::Level::Advanced, false) {
        let kind = match l.kind {
            crate::leftovers::LeftoverKind::RegistryKey => "vendor",
            crate::leftovers::LeftoverKind::RegistryValue => "startup",
            _ => continue,
        };
        if !out.iter().any(|r| r.path.eq_ignore_ascii_case(&l.path)) {
            out.push(RelatedRegistry { path: l.path, kind });
        }
    }
    out
}

/// Cleaner categories that belong to this program.
pub fn caches_of(p: &Program, analysis: &[CategoryResult]) -> Vec<RelatedCache> {
    let dirs = dirs_of(p);
    let name = crate::appsize::norm(&p.name);
    let mut out: Vec<RelatedCache> = analysis
        .iter()
        .filter(|r| {
            // The owner name must be the program's, not merely similar:
            // "Discord PTB" is a different program from "Discord".
            let owner_matches = r.category.owner.as_ref().is_some_and(|o| {
                let on = crate::appsize::norm(o);
                !on.is_empty() && on == name
            });
            // Or the items themselves live inside the program's folders.
            let inside = !dirs.is_empty() && r.items.iter().take(20).any(|i| dirs.iter().any(|d| within(&i.path, d)));
            (owner_matches || inside) && (r.count > 0 || r.running)
        })
        .map(|r| RelatedCache {
            id: r.category.id.clone(),
            label: r.category.label.clone(),
            owner: r.category.owner.clone(),
            count: r.count,
            bytes: r.bytes,
            running: r.running,
            admin: r.category.admin,
        })
        .collect();
    out.sort_by_key(|c| std::cmp::Reverse(c.bytes));
    out
}

/// Shortcuts pointing at a file of this program.
pub fn shortcuts_of(p: &Program) -> Vec<String> {
    let dirs = dirs_of(p);
    if dirs.is_empty() {
        return Vec::new();
    }
    let links = crate::shortcuts::find_shortcuts(&crate::shortcuts::shortcut_roots());
    let targets = crate::shortcuts::resolve_targets(&links);
    links
        .into_iter()
        .zip(targets)
        .filter_map(|(link, target)| {
            let t = target?;
            dirs.iter().any(|d| within(&t, d)).then(|| link.to_string_lossy().into_owned())
        })
        .collect()
}

/// Build the whole picture. `analysis` is a Cleaner analysis (pass an empty
/// slice to skip caches), `startup` the startup list, `procs` a process list.
pub fn analyze(
    p: &Program,
    others: &[Program],
    procs: &[crate::processes::ProcInfo],
    startup: &[StartupItem],
    analysis: &[CategoryResult],
) -> AppAnalysis {
    let processes = processes_of(p, procs);
    let shortcuts = shortcuts_of(p);
    let dirs = dirs_of(p);
    let mut extra_locations: Vec<String> = Vec::new();
    for dir in processes.iter().filter_map(|pr| pr.path.as_deref()).filter_map(|path| path.rsplit_once('\\').map(|(d, _)| d.to_string())) {
        if !dirs.iter().any(|d| within(&dir, d)) && !extra_locations.contains(&dir) {
            extra_locations.push(dir);
        }
    }
    AppAnalysis {
        program_id: p.id.clone(),
        processes,
        startup: startup_of(p, startup),
        registry: registry_of(p, others),
        caches: caches_of(p, analysis),
        shortcuts,
        extra_locations,
    }
}

/// Evidence for the correlation engine built from a program's live state.
pub fn evidence_for(programs: &[Program], traces: &[(String, Vec<String>)]) -> Evidence {
    Evidence {
        processes: correlate::process_evidence(programs),
        shortcuts: Vec::new(),
        traces: traces.iter().flat_map(|(id, dirs)| dirs.iter().map(move |d| (id.clone(), d.clone()))).collect(),
    }
}

/// Analyze only the categories that could belong to `p` (the whole catalog is
/// far more than one program needs).
pub fn cache_analysis(p: &Program) -> Vec<CategoryResult> {
    let procs = crate::processes::list();
    let name = crate::appsize::norm(&p.name);
    let dirs = dirs_of(p);
    cleaner::catalog()
        .into_iter()
        .filter(|c| {
            if c.admin {
                return false; // machine-wide categories are never one program's
            }
            let owner = c.owner.as_ref().map(|o| crate::appsize::norm(o)).unwrap_or_default();
            let by_owner = !owner.is_empty() && !name.is_empty() && owner == name;
            let by_dir = c.rules.iter().any(|r| dirs.iter().any(|d| within(r.root(), d)));
            by_owner || by_dir
        })
        .map(|c| cleaner::analyze_category(&c, &procs))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::processes::ProcInfo;
    use crate::programs::ProgramSource;

    fn prog(name: &str, dir: &str) -> Program {
        let mut p = Program::blank(format!("reg:HKCU:{name}"), name.into(), ProgramSource::Win32);
        p.install_location = Some(dir.into());
        p
    }

    fn cache_result(id: &str, owner: &str, bytes: u64) -> CategoryResult {
        CategoryResult {
            category: crate::cleaner::Category {
                id: id.into(),
                group: crate::cleaner::Group::Apps,
                label: "appCache".into(),
                owner: Some(owner.into()),
                risk: crate::cleaner::Risk::Safe,
                default_on: false,
                admin: false,
                rules: vec![crate::cleaner::Rule::Dir { dir: format!(r"C:\Users\me\AppData\Roaming\{id}\Cache"), recursive: true, min_age_ms: None }],
                special: None,
                processes: Vec::new(),
                process_path_hint: None,
                warning: None,
            },
            items: Vec::new(),
            count: 3,
            bytes,
            running: false,
            recent_skipped: 0,
            admin_pending: false,
            truncated: false,
            error: None,
        }
    }

    #[test]
    fn a_similar_name_is_not_the_same_program() {
        // Discord, Discord PTB and Discord Canary each keep their own cache.
        let p = prog("Discord", r"C:\Users\me\AppData\Local\Discord");
        let results = vec![cache_result("discord", "Discord", 300), cache_result("discordptb", "Discord PTB", 400)];
        let found = caches_of(&p, &results);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].id, "discord");
    }

    #[test]
    fn only_processes_from_the_program_folder_count() {
        let p = prog("Acme", r"C:\Program Files\Acme");
        let procs = vec![
            ProcInfo { pid: 1, parent: 0, name: "acme.exe".into(), path: Some(r"C:\Program Files\Acme\acme.exe".into()) },
            ProcInfo { pid: 2, parent: 0, name: "acme.exe".into(), path: Some(r"C:\Program Files\Acme Reader\acme.exe".into()) },
            ProcInfo { pid: 3, parent: 0, name: "hidden.exe".into(), path: None },
        ];
        let found = processes_of(&p, &procs);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].pid, 1);
        assert_eq!(found[0].signals, vec!["runningProcess"]);
    }

    fn startup_item(name: &str, exe: &str) -> StartupItem {
        StartupItem {
            id: format!("run:{name}"),
            name: name.into(),
            source: crate::startup::Source::RunKey {
                hive: crate::registry::Hive::CurrentUser,
                view: crate::registry::View::Default,
                once: false,
            },
            location: r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run".into(),
            key: name.into(),
            command: format!("\"{exe}\""),
            exe: Some(exe.into()),
            exe_exists: true,
            enabled: true,
            disabled_at_ms: None,
            can_disable: true,
            can_remove: true,
            needs_admin: false,
            is_windows: false,
            company: None,
            description: None,
            detail: None,
        }
    }

    #[test]
    fn startup_follows_the_file_it_runs_not_the_name() {
        let p = prog("Acme", r"C:\Program Files\Acme");
        let mine = startup_item("Acme Helper", r"C:\Program Files\Acme\helper.exe");
        let theirs = startup_item("Acme", r"C:\Program Files\Other\acme.exe");

        let found = startup_of(&p, &[mine, theirs]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Acme Helper");
    }
}
