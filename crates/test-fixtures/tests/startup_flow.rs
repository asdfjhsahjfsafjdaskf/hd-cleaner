//! Startup manager on real (per-user) entries created by the fake program:
//! a Run value and a Startup folder shortcut. Disable/enable go through
//! StartupApproved exactly like Windows; removal backs up first.

use hdcleaner_core::registry::{Hive, Key, View};
use hdcleaner_core::startup::{self, Source};
use hdcleaner_core::AppError;
use std::process::Command;

const FIXTURE: &str = env!("CARGO_BIN_EXE_hdcleaner-fake-app");
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";

struct Cleanup(String);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = Command::new(FIXTURE).args(["cleanup", "--name", &self.0]).output();
    }
}

fn find(name: &str, folder: bool) -> startup::StartupItem {
    startup::list(&[])
        .into_iter()
        .find(|i| {
            i.name == name
                && match i.source {
                    Source::RunKey { hive: Hive::CurrentUser, .. } => !folder,
                    Source::StartupFolder { common: false } => folder,
                    _ => false,
                }
        })
        .unwrap_or_else(|| panic!("{name} (folder: {folder}) not listed"))
}

fn approval(sub: &str, value: &str) -> Option<Vec<u8>> {
    Key::open(Hive::CurrentUser, &format!(r"{APPROVED}\{sub}"), View::Default)?.value_raw(value).map(|(_, d)| d)
}

#[test]
fn disable_enable_and_remove_user_startup_entries() {
    let name = format!("HDC Startup Test {}", std::process::id());
    let _guard = Cleanup(name.clone());
    assert!(Command::new(FIXTURE).args(["install", "--name", &name]).status().unwrap().success());
    assert!(Command::new(FIXTURE).args(["startup-link", "--name", &name]).status().unwrap().success());

    for folder in [false, true] {
        let item = find(&name, folder);
        assert!(item.enabled && item.can_disable && item.can_remove && !item.needs_admin, "{item:?}");
        assert!(item.exe_exists && item.exe.as_deref().is_some_and(|e| e.to_lowercase().ends_with("app.exe")), "{item:?}");
        let (sub, value) = if folder { ("StartupFolder", format!("{name}.lnk")) } else { ("Run", name.clone()) };

        // Disable: StartupApproved flag odd + timestamp; entry itself untouched.
        assert!(startup::set_enabled(&item, false).unwrap().is_none(), "per-user: no elevation");
        let data = approval(sub, &value).expect("approval written");
        assert_eq!(data.len(), 12);
        assert_eq!(data[0] & 1, 1);
        let off = find(&name, folder);
        assert!(!off.enabled && off.disabled_at_ms.is_some());
        assert_eq!(off.command, item.command);

        // Enable again.
        assert!(startup::set_enabled(&off, true).unwrap().is_none());
        assert_eq!(approval(sub, &value).unwrap()[0] & 1, 0);
        assert!(find(&name, folder).enabled);

        // Stale review: a different command is refused.
        assert!(matches!(startup::find(&item.id, "something else", &[]), Err(AppError::ChangedSinceReview { .. })));

        // Remove with backup.
        let backup = std::env::temp_dir().join(format!("hdc-startup-test-{}-{folder}", std::process::id()));
        assert!(startup::remove(&item, &backup).unwrap().is_none());
        assert!(startup::list(&[]).iter().all(|i| i.id != item.id), "entry gone");
        assert!(approval(sub, &value).is_none(), "approval value forgotten");
        if folder {
            assert!(backup.join(format!("{name}.lnk")).is_file(), "shortcut copied to the backup");
        } else {
            let reg = std::fs::read(backup.join("startup.reg")).unwrap();
            let text = String::from_utf16_lossy(&reg[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>());
            assert!(text.contains(&name), "backup .reg holds the Run value");
        }
        let _ = std::fs::remove_dir_all(&backup);
    }
}
