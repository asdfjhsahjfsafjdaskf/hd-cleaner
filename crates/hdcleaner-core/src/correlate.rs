//! Which program does this folder belong to?
//!
//! The uninstaller, the analyzer and Smart Storage all need the same answer,
//! and they need it to be *honest*: a folder is tied to a program only through
//! a signal that can be named — the registered install location, a shortcut
//! that points inside it, a running process started from it, an installation
//! trace, the publisher's own folder. A name that merely looks alike scores
//! low and says so, and nothing here ever decides to delete anything.

use crate::appsize::{norm, Confidence};
use crate::programs::Program;
use serde::Serialize;

/// Where a signal came from, with the weight it carries on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Signal {
    /// The program's own `InstallLocation` from the registry.
    RegisteredInstallLocation,
    /// A Microsoft Store package data folder.
    PackageDataFolder,
    /// The installation monitor recorded the installer creating it.
    InstallTrace,
    /// The folder holds the uninstaller the registry points at.
    UninstallerFolder,
    /// A process running right now was started from inside it.
    RunningProcess,
    /// A Start Menu / desktop shortcut of this program points inside it.
    Shortcut,
    /// `…\<Publisher>\…` with the program's publisher.
    PublisherFolder,
    /// Only the name looks alike.
    NameMatch,
}

impl Signal {
    pub fn weight(self) -> u8 {
        match self {
            Signal::RegisteredInstallLocation => 100,
            Signal::PackageDataFolder => 95,
            Signal::InstallTrace => 90,
            Signal::UninstallerFolder => 85,
            Signal::RunningProcess => 80,
            Signal::Shortcut => 75,
            Signal::PublisherFolder => 55,
            Signal::NameMatch => 45,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Signal::RegisteredInstallLocation => "registeredInstallLocation",
            Signal::PackageDataFolder => "packageDataFolder",
            Signal::InstallTrace => "installTrace",
            Signal::UninstallerFolder => "uninstallerFolder",
            Signal::RunningProcess => "runningProcess",
            Signal::Shortcut => "shortcut",
            Signal::PublisherFolder => "publisherFolder",
            Signal::NameMatch => "nameMatch",
        }
    }
}

/// Extra evidence gathered once and reused for every path.
#[derive(Default)]
pub struct Evidence {
    /// (program id, folder) of processes running now.
    pub processes: Vec<(String, String)>,
    /// (program id, folder) a shortcut of that program points into.
    pub shortcuts: Vec<(String, String)>,
    /// (program id, folder) recorded by the installation monitor.
    pub traces: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Match {
    pub program_id: String,
    pub name: String,
    /// 0-100: how sure we are that the path belongs to this program.
    pub score: u8,
    /// Every signal that pointed here, strongest first.
    pub signals: Vec<&'static str>,
    /// The folder the strongest signal was about.
    pub location: String,
    pub confidence: Confidence,
}

fn within(path_lc: &str, dir: &str) -> bool {
    let d = dir.trim_end_matches('\\').to_lowercase();
    if d.is_empty() {
        return false;
    }
    path_lc == d || (path_lc.starts_with(&d) && path_lc.as_bytes().get(d.len()) == Some(&b'\\'))
}

fn confidence_of(score: u8) -> Confidence {
    if score >= 90 {
        Confidence::Confirmed
    } else if score >= 70 {
        Confidence::Probable
    } else {
        Confidence::Possible
    }
}

/// Score how much `path` looks like it belongs to each program.
///
/// Several signals raise the score above the strongest one, but never to
/// certainty on their own: the cap is 99 unless a registered location matched.
pub fn match_path(programs: &[Program], path: &str, evidence: &Evidence) -> Vec<Match> {
    let p = path.trim_end_matches('\\').to_lowercase();
    let local = std::env::var("LOCALAPPDATA").ok();
    let mut out: Vec<Match> = Vec::new();

    for prog in programs {
        let mut hits: Vec<(Signal, String)> = Vec::new();
        if let Some(loc) = &prog.install_location {
            if within(&p, loc) {
                hits.push((Signal::RegisteredInstallLocation, loc.clone()));
            }
        }
        if let Some(loc) = &prog.inferred_location {
            if within(&p, loc) {
                hits.push((Signal::UninstallerFolder, loc.clone()));
            }
        }
        if let (Some(fam), Some(local)) = (&prog.package_family_name, &local) {
            let data = format!(r"{local}\Packages\{fam}");
            if within(&p, &data) {
                hits.push((Signal::PackageDataFolder, data));
            }
        }
        for (id, dir) in &evidence.traces {
            if id == &prog.id && within(&p, dir) {
                hits.push((Signal::InstallTrace, dir.clone()));
                break;
            }
        }
        for (id, dir) in &evidence.processes {
            if id == &prog.id && within(&p, dir) {
                hits.push((Signal::RunningProcess, dir.clone()));
                break;
            }
        }
        for (id, dir) in &evidence.shortcuts {
            if id == &prog.id && within(&p, dir) {
                hits.push((Signal::Shortcut, dir.clone()));
                break;
            }
        }
        // `…\Publisher\Product` and `…\Product` under a user data root.
        let name = norm(&prog.name);
        let folder = norm(p.trim_end_matches('\\').rsplit('\\').next().unwrap_or(""));
        if !name.is_empty() && name.len() >= 3 && (folder == name || folder.contains(&name)) {
            hits.push((Signal::NameMatch, path.to_string()));
        }
        if let Some(pub_name) = prog.publisher.as_ref().map(|s| norm(s)).filter(|s| s.len() >= 3) {
            if p.split('\\').any(|c| norm(c) == pub_name) {
                hits.push((Signal::PublisherFolder, path.to_string()));
            }
        }
        if hits.is_empty() {
            continue;
        }
        hits.sort_by_key(|(s, _)| std::cmp::Reverse(s.weight()));
        let best = hits[0].0;
        // Extra signals add a little, and only a registered location is worth
        // full certainty.
        let extra: u32 = hits.iter().skip(1).map(|(s, _)| (s.weight() as u32) / 10).sum();
        let cap = if best == Signal::RegisteredInstallLocation { 100 } else { 99 };
        let score = ((best.weight() as u32 + extra).min(cap as u32)) as u8;
        out.push(Match {
            program_id: prog.id.clone(),
            name: prog.name.clone(),
            score,
            signals: hits.iter().map(|(s, _)| s.key()).collect(),
            location: hits[0].1.clone(),
            confidence: confidence_of(score),
        });
    }

    // Deepest folder first (a program inside another program's folder), then
    // the score; on a tie the program whose name matches the file or folder.
    let stem = norm(p.rsplit('\\').next().unwrap_or("").trim_end_matches(".exe"));
    let named = |m: &Match| {
        let n = norm(&m.name);
        let folder = norm(m.location.trim_end_matches('\\').rsplit('\\').next().unwrap_or(""));
        !n.is_empty() && (stem.starts_with(&n) || (!stem.is_empty() && n.starts_with(&stem)) || folder == n)
    };
    out.sort_by(|a, b| {
        b.location.len().cmp(&a.location.len()).then(b.score.cmp(&a.score)).then(named(b).cmp(&named(a)))
    });
    out
}

/// The best match for a path, when there is one worth showing.
pub fn owner_of(programs: &[Program], path: &str, evidence: &Evidence) -> Option<Match> {
    match_path(programs, path, evidence).into_iter().max_by_key(|m| m.score)
}

/// Folders of processes running now, tied to the program they belong to.
pub fn process_evidence(programs: &[Program]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for pr in crate::processes::list() {
        let Some(path) = pr.path.as_deref() else { continue };
        let Some((dir, _)) = path.rsplit_once('\\') else { continue };
        // Only the registered/inferred locations count here, or every process
        // in Program Files would look like every program.
        for prog in programs {
            let owns = [prog.install_location.as_deref(), prog.inferred_location.as_deref()]
                .into_iter()
                .flatten()
                .any(|loc| within(&dir.to_lowercase(), loc));
            if owns {
                let entry = (prog.id.clone(), dir.to_string());
                if !out.contains(&entry) {
                    out.push(entry);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::programs::ProgramSource;

    fn prog(id: &str, name: &str) -> Program {
        Program::blank(id.into(), name.into(), ProgramSource::Win32)
    }

    #[test]
    fn a_registered_location_beats_a_name_that_looks_alike() {
        let mut real = prog("reg:HKLM:Real", "Acme Player");
        real.install_location = Some(r"C:\Program Files\Acme Player".into());
        let lookalike = prog("reg:HKCU:Other", "Acme Player Codec Pack");

        let m = match_path(&[real, lookalike], r"C:\Program Files\Acme Player\bin", &Evidence::default());
        assert_eq!(m[0].program_id, "reg:HKLM:Real");
        assert_eq!(m[0].score, 100);
        assert_eq!(m[0].signals[0], "registeredInstallLocation");
        assert_eq!(m[0].confidence, Confidence::Confirmed);
    }

    #[test]
    fn several_signals_raise_the_score_but_never_to_certainty() {
        let p = prog("reg:HKCU:App", "Sketchpad");
        let ev = Evidence {
            traces: vec![("reg:HKCU:App".into(), r"C:\Tools\sketch".into())],
            processes: vec![("reg:HKCU:App".into(), r"C:\Tools\sketch".into())],
            ..Default::default()
        };
        let m = match_path(std::slice::from_ref(&p), r"C:\Tools\sketch\data", &ev);
        assert_eq!(m.len(), 1);
        assert!(m[0].score > 90 && m[0].score < 100, "{}", m[0].score);
        assert_eq!(m[0].signals, vec!["installTrace", "runningProcess"]);

        // On its own, a name that looks alike stays low and says why.
        let only_name = match_path(std::slice::from_ref(&p), r"C:\Users\me\AppData\Local\Sketchpad", &Evidence::default());
        assert_eq!(only_name[0].signals, vec!["nameMatch"]);
        assert_eq!(only_name[0].score, 45);
        assert_eq!(only_name[0].confidence, Confidence::Possible);
    }

    #[test]
    fn a_publisher_folder_is_a_weak_signal_of_its_own() {
        let mut p = prog("reg:HKCU:Thing", "Thing");
        p.publisher = Some("Contoso Ltd".into());
        let m = match_path(std::slice::from_ref(&p), r"C:\ProgramData\Contoso Ltd\Shared", &Evidence::default());
        assert_eq!(m[0].signals, vec!["publisherFolder"]);
        assert!(m[0].score < 70);
    }

    #[test]
    fn nothing_matches_an_unrelated_path() {
        let mut p = prog("reg:HKLM:X", "Xyz");
        p.install_location = Some(r"C:\Program Files\Xyz".into());
        assert!(match_path(std::slice::from_ref(&p), r"C:\Windows\System32", &Evidence::default()).is_empty());
    }
}
