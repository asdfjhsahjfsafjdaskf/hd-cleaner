//! End-to-end: install a fake program, uninstall it with its (sloppy) official
//! uninstaller, find the leftovers, remove them, verify nothing is left.
//! Everything happens under the current user profile with a unique name.

use hdcleaner_core::leftovers::{self, LeftoverKind, Level, RemovalOptions};
use hdcleaner_core::programs::registry_programs;
use std::process::Command;
use std::sync::atomic::AtomicBool;

const FIXTURE: &str = env!("CARGO_BIN_EXE_hdcleaner-fake-app");

struct Cleanup(String);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = Command::new(FIXTURE).args(["cleanup", "--name", &self.0]).output();
    }
}

#[test]
fn uninstall_then_leftovers_then_clean() {
    let name = format!("HDC Fake App {}", std::process::id());
    let _guard = Cleanup(name.clone());
    let out = Command::new(FIXTURE).args(["install", "--name", &name]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    // 1. The program is listed like any other.
    let all = registry_programs();
    let prog = all.iter().find(|p| p.name == name).expect("fake program listed").clone();
    assert_eq!(prog.publisher.as_deref(), Some("HdcFakeVendor"));
    assert!(prog.install_location.is_some());

    // 2. Run the official uninstaller and wait for it.
    let cmd = hdcleaner_core::uninstall::command_for(&prog, false).unwrap();
    assert_eq!(cmd.kind, "registry");
    assert!(cmd.params.contains("uninstall"));
    let stop = AtomicBool::new(false);
    let outcome = hdcleaner_core::uninstall::run(&prog, &cmd, &stop, |_| {}).unwrap();
    assert!(!outcome.still_installed, "uninstall entry removed by the uninstaller");
    assert_eq!(outcome.exit_code, Some(0));
    assert!(outcome.duration_ms >= 600, "waited for the uninstaller to finish");

    // 3. Leftovers at the Safe level: only identity-bound items.
    let safe = leftovers::scan(&prog, &all, Level::Safe, true);
    let kinds = |ls: &[leftovers::Leftover]| ls.iter().map(|l| (l.kind, l.reason.clone())).collect::<Vec<_>>();
    assert!(safe.iter().all(|l| l.level == Level::Safe), "{:?}", kinds(&safe));
    assert!(safe.iter().any(|l| l.reason == "installFolderRemains"), "{:?}", kinds(&safe));
    assert!(safe.iter().any(|l| l.kind == LeftoverKind::Shortcut && l.reason == "shortcutTargetsProgram"), "{:?}", kinds(&safe));
    assert!(safe.iter().any(|l| l.kind == LeftoverKind::RegistryValue && l.reason == "startupEntryTargetsProgram"), "{:?}", kinds(&safe));
    assert!(!safe.iter().any(|l| l.reason == "uninstallEntryRemains"), "entry was removed by the uninstaller");

    // 4. Moderate adds name-matched data folders and the vendor\product key.
    let moderate = leftovers::scan(&prog, &all, Level::Moderate, true);
    let key = name.chars().filter(|c| c.is_alphanumeric()).collect::<String>();
    let roaming = std::path::Path::new(&std::env::var("APPDATA").unwrap()).join(&name);
    let local = std::path::Path::new(&std::env::var("LOCALAPPDATA").unwrap()).join(&name);
    assert!(moderate.iter().any(|l| l.kind == LeftoverKind::Folder && l.path.eq_ignore_ascii_case(&roaming.to_string_lossy())), "{:?}", kinds(&moderate));
    assert!(moderate.iter().any(|l| l.kind == LeftoverKind::Folder && l.path.eq_ignore_ascii_case(&local.to_string_lossy())), "{:?}", kinds(&moderate));
    assert!(moderate.iter().any(|l| l.kind == LeftoverKind::RegistryKey && l.path.ends_with(&format!(r"HdcFakeVendor\{key}"))), "{:?}", kinds(&moderate));
    // Local folder holds a 256 KiB cache: sizes are measured.
    assert!(moderate.iter().find(|l| l.path.eq_ignore_ascii_case(&local.to_string_lossy())).unwrap().size >= 256 * 1024);
    // Nothing from another program, nothing blocked.
    for l in &moderate {
        assert!(l.risk != Some(hdcleaner_core::protection::Risk::Blocked));
    }

    // 5. Advanced adds the vendor key (holds only this product).
    let advanced = leftovers::scan(&prog, &all, Level::Advanced, true);
    let vendor = advanced.iter().find(|l| l.reason == "publisherKeyOnlyThisProduct").expect("vendor key at advanced level");
    assert!(!vendor.preselected, "low-confidence items are never pre-selected");

    // 6. Dry run changes nothing; real removal (with registry backup) cleans up.
    let backup = tempfile_dir();
    let (dry, pending) = leftovers::remove(&advanced, &RemovalOptions { backup_dir: &backup, recycle: false, allow_dangerous: false, dry_run: true });
    assert!(pending.is_empty() && dry.iter().all(|r| r.status == "wouldRemove"));
    assert!(roaming.exists());

    let (results, pending) = leftovers::remove(&advanced, &RemovalOptions { backup_dir: &backup, recycle: false, allow_dangerous: false, dry_run: false });
    assert!(pending.is_empty(), "everything is per-user: no elevation needed ({pending:?})");
    assert!(results.iter().all(|r| r.status == "removed"), "{results:?}");
    assert!(!roaming.exists() && !local.exists());
    assert!(!std::path::Path::new(prog.install_location.as_deref().unwrap()).exists());
    assert!(!hdcleaner_core::regops::key_exists(hdcleaner_core::registry::Hive::CurrentUser, r"Software\HdcFakeVendor", hdcleaner_core::registry::View::Default));
    let reg_backup = std::fs::read(backup.join("registry.reg")).unwrap();
    let text = String::from_utf16_lossy(&reg_backup[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>());
    assert!(text.contains("HdcFakeVendor") && text.contains("InstallDir"), "registry backup written before deletion");

    // 7. A new scan finds nothing left.
    let again = leftovers::scan(&prog, &registry_programs(), Level::Advanced, true);
    assert!(again.is_empty(), "{:?}", kinds(&again));
    let _ = std::fs::remove_dir_all(&backup);
}

fn tempfile_dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("nexus-uninstall-test-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}
