//! Forced uninstall of a broken program: uninstaller missing, a process still
//! running from its folder, entries left everywhere.

use hdcleaner_core::forced::{resolve, target_from_match, ForcedInput};
use hdcleaner_core::leftovers::{self, LeftoverKind, Level, RemovalOptions};
use hdcleaner_core::programs::registry_programs;
use std::process::Command;

const FIXTURE: &str = env!("CARGO_BIN_EXE_hdcleaner-fake-app");

struct Cleanup(String, Option<std::process::Child>);
impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(c) = self.1.as_mut() {
            let _ = c.kill();
        }
        let _ = Command::new(FIXTURE).args(["cleanup", "--name", &self.0]).output();
    }
}

#[test]
fn forced_uninstall_of_broken_program() {
    let name = format!("HDC Broken App {}", std::process::id());
    let mut guard = Cleanup(name.clone(), None);
    assert!(Command::new(FIXTURE).args(["install", "--name", &name]).status().unwrap().success());
    assert!(Command::new(FIXTURE).args(["break", "--name", &name]).status().unwrap().success());
    let folder = std::path::Path::new(&std::env::var("LOCALAPPDATA").unwrap()).join("Programs").join(&name);
    // A resident process running from the program folder.
    guard.1 = Some(Command::new(folder.join("app.exe")).arg("--tray").spawn().unwrap());
    std::thread::sleep(std::time::Duration::from_millis(500));

    let all = registry_programs();
    let registered = all.iter().find(|p| p.name == name).unwrap();
    // The official uninstaller is gone: the normal path is impossible.
    assert!(hdcleaner_core::uninstall::command_for(registered, false).is_err());

    // Forced: start from the folder only.
    let t = resolve(&ForcedInput { folder: Some(folder.to_string_lossy().into()), ..Default::default() }, &all).unwrap();
    assert_eq!(t.program.name, name, "name derived from the folder");
    assert!(t.matches.iter().any(|m| m.program.id == registered.id && m.reason == "sameFolder"));
    let app = t.processes.iter().find(|p| p.path.as_deref().is_some_and(|x| x.to_lowercase().ends_with("app.exe"))).expect("running process found");

    // End the process (PID + path re-verified), as the UI does.
    hdcleaner_core::processes::terminate(app.pid, app.path.as_deref().unwrap()).unwrap();
    guard.1 = None;
    assert!(hdcleaner_core::processes::running_from(&folder.to_string_lossy()).is_empty());

    // Target the registered entry and scan in forced mode (still registered).
    let target = target_from_match(&t.matches.iter().find(|m| m.program.id == registered.id).unwrap().program, &t.program);
    assert!(hdcleaner_core::uninstall::still_installed(&target));
    let others: Vec<_> = all.iter().filter(|p| p.id != target.id).cloned().collect();
    let found = leftovers::scan(&target, &others, Level::Moderate, true);
    let reasons: Vec<&str> = found.iter().map(|l| l.reason.as_str()).collect();
    for r in ["installFolderRemains", "uninstallEntryRemains", "shortcutTargetsProgram", "startupEntryTargetsProgram", "registryKeyUnderPublisher"] {
        assert!(reasons.contains(&r), "missing {r}: {reasons:?}");
    }
    assert!(found.iter().filter(|l| l.kind == LeftoverKind::Folder).count() >= 3, "{reasons:?}");

    let backup = std::env::temp_dir().join(format!("hdc-forced-test-{}", std::process::id()));
    let chosen: Vec<_> = found.into_iter().filter(|l| l.preselected).collect();
    let leftovers::Removal { results, pending, .. } = leftovers::remove(&chosen, &RemovalOptions { backup_dir: &backup, recycle: false, allow_dangerous: false, dry_run: false, quarantine: true });
    assert!(pending.is_empty(), "{pending:?}");
    assert!(results.iter().all(|r| r.status == "removed"), "{results:?}");
    assert!(!hdcleaner_core::uninstall::still_installed(&target), "orphan uninstall entry removed");
    assert!(!folder.exists());
    assert!(!registry_programs().iter().any(|p| p.name == name));
    let _ = std::fs::remove_dir_all(&backup);
}