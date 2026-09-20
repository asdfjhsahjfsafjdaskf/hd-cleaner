//! Forced uninstall: remove programs that are broken, unregistered or half
//! removed, starting from whatever the user knows (name, .exe, folder).
//!
//! `resolve` turns that input into a target program (a registered one when it
//! can be matched reliably, otherwise a synthetic description), lists the
//! registered programs that might be the same product, and the processes
//! running from its folder. The leftover engine then does the rest; nothing is
//! removed without the user's review.

use crate::appsize::norm;
use crate::processes::{running_from, version_info, ProcInfo, VersionInfo};
use crate::programs::{clean_dir, exe_from_command, Program, ProgramSource};
use crate::protection::{assess, Risk};
use crate::{AppError, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForcedInput {
    pub name: Option<String>,
    pub exe: Option<String>,
    pub folder: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Match {
    pub program: Program,
    /// "sameFolder" | "uninstallerInFolder" | "sameName"
    pub reason: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForcedTarget {
    /// What will be cleaned (synthetic unless the user picks a match).
    pub program: Program,
    pub matches: Vec<Match>,
    pub processes: Vec<ProcInfo>,
    pub version: Option<VersionInfo>,
}

fn within(path: &str, dir: &str) -> bool {
    let (p, d) = (path.trim_end_matches('\\').to_lowercase(), dir.trim_end_matches('\\').to_lowercase());
    p == d || (p.starts_with(&d) && p.as_bytes().get(d.len()) == Some(&b'\\'))
}

pub fn resolve(input: &ForcedInput, all: &[Program]) -> Result<ForcedTarget> {
    let name_in = input.name.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let exe = match input.exe.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(e) => {
            let e = clean_dir(e).ok_or_else(|| AppError::InvalidInput(format!("invalid executable path: {e}")))?;
            if !std::path::Path::new(&e).is_file() {
                return Err(AppError::NotFound { path: e });
            }
            Some(e)
        }
        None => None,
    };
    let folder = match input.folder.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(f) => Some(clean_dir(f).ok_or_else(|| AppError::InvalidInput(format!("invalid folder: {f}")))?),
        None => exe.as_deref().and_then(|e| e.rsplit_once('\\').map(|(d, _)| d.to_string())),
    };
    // A registered folder that no longer exists is common for broken
    // programs: continue by name (its orphan entries are what is left).
    let folder = match folder {
        Some(f) if !std::path::Path::new(&f).is_dir() => {
            if name_in.is_none() {
                return Err(AppError::NotFound { path: f });
            }
            None
        }
        other => other,
    };
    if let Some(f) = &folder {
        // The folder becomes the program's "install folder": it must be a
        // specific program folder, never a shared or system container.
        let a = assess(f);
        if a.risk == Risk::Blocked {
            return Err(AppError::Protected { path: f.clone(), reason: a.reason.into() });
        }
    }
    let version = exe.as_deref().map(version_info);
    let name = name_in
        .map(str::to_string)
        .or_else(|| version.as_ref().and_then(|v| v.product.clone().or_else(|| v.description.clone())))
        .or_else(|| folder.as_deref().and_then(|f| f.rsplit('\\').next().map(str::to_string)))
        .ok_or_else(|| AppError::InvalidInput("give at least a name, an executable or a folder".into()))?;
    if norm(&name).len() < 3 && folder.is_none() {
        return Err(AppError::InvalidInput("the name is too short to search by name alone (use at least 3 letters or give a folder)".into()));
    }

    // Registered programs that are probably the same product.
    let mut matches = Vec::new();
    for p in all {
        let reason = match &folder {
            Some(f) if p.install_location.as_deref().or(p.inferred_location.as_deref()).is_some_and(|l| within(l, f) || within(f, l)) => {
                Some("sameFolder")
            }
            Some(f) if p.uninstall_string.as_deref().and_then(exe_from_command).is_some_and(|u| within(&u, f)) => {
                Some("uninstallerInFolder")
            }
            _ if norm(&p.name) == norm(&name) => Some("sameName"),
            _ => None,
        };
        if let Some(reason) = reason {
            matches.push(Match { program: p.clone(), reason });
        }
    }

    let mut program = Program::blank(format!("forced:{}", norm(&name)), name, ProgramSource::Win32);
    program.install_location = folder.clone();
    program.publisher = version.as_ref().and_then(|v| v.company.clone());
    program.version = version.as_ref().and_then(|v| v.version.clone());
    let processes = folder.as_deref().map(running_from).unwrap_or_default();
    Ok(ForcedTarget { program, matches, processes, version })
}

/// Target a registered match instead of the synthetic description, keeping
/// the user's folder when the registration has none.
pub fn target_from_match(m: &Program, synthetic: &Program) -> Program {
    let mut p = m.clone();
    if p.install_location.is_none() {
        p.install_location = synthetic.install_location.clone().or(p.inferred_location.clone());
    }
    if p.publisher.is_none() {
        p.publisher = synthetic.publisher.clone();
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_input() {
        assert!(resolve(&ForcedInput::default(), &[]).is_err());
        assert!(resolve(&ForcedInput { name: Some("ab".into()), ..Default::default() }, &[]).is_err());
        let win = crate::system::windows_dir();
        assert!(matches!(
            resolve(&ForcedInput { folder: Some(win.clone()), ..Default::default() }, &[]),
            Err(AppError::Protected { .. })
        ));
        assert!(resolve(&ForcedInput { exe: Some(r"C:\missing\x.exe".into()), ..Default::default() }, &[]).is_err());
        // Missing folder + name: continue by name.
        let t = resolve(&ForcedInput { name: Some("Gone App".into()), folder: Some(r"C:\gone\Gone App".into()), ..Default::default() }, &[]).unwrap();
        assert!(t.program.install_location.is_none());
    }

    #[test]
    fn resolves_from_folder_and_matches() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Broken App");
        std::fs::create_dir(&app).unwrap();
        let mut registered = Program::blank("reg:HKCU:Broken".into(), "Broken App".into(), ProgramSource::Win32);
        registered.uninstall_string = Some(format!(r#""{}\unins000.exe" /S"#, app.display()));
        let unrelated = Program::blank("reg:HKCU:Other".into(), "Other".into(), ProgramSource::Win32);
        let t = resolve(&ForcedInput { folder: Some(app.to_string_lossy().into()), ..Default::default() }, &[registered.clone(), unrelated]).unwrap();
        assert_eq!(t.program.name, "Broken App");
        assert_eq!(t.program.install_location.as_deref(), Some(app.to_str().unwrap()));
        assert_eq!(t.matches.len(), 1);
        assert_eq!(t.matches[0].reason, "uninstallerInFolder");
        let chosen = target_from_match(&t.matches[0].program, &t.program);
        assert_eq!(chosen.id, "reg:HKCU:Broken");
        assert_eq!(chosen.install_location, t.program.install_location);
    }
}
