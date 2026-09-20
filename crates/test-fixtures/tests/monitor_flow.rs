//! Installation monitor against a real (fake) installation: snapshot, watch,
//! install, compare — and then use the trace to find leftovers.

use hdcleaner_core::monitor;
use hdcleaner_core::programs::registry_programs;
use std::path::PathBuf;
use std::process::Command;

const FIXTURE: &str = env!("CARGO_BIN_EXE_hdcleaner-fake-app");

struct Cleanup(String);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = Command::new(FIXTURE).args(["cleanup", "--name", &self.0]).output();
    }
}

#[test]
fn records_what_an_installation_added() {
    let name = format!("HDC Monitor Test {}", std::process::id());
    let _guard = Cleanup(name.clone());
    let programs = PathBuf::from(std::env::var("LOCALAPPDATA").unwrap()).join("Programs");
    std::fs::create_dir_all(&programs).unwrap();
    // The vendor folder already exists (another product of the same vendor);
    // watching it is how the trace catches data stored inside it.
    let vendor_root = PathBuf::from(std::env::var("LOCALAPPDATA").unwrap()).join("HdcFakeVendor");
    std::fs::create_dir_all(&vendor_root).unwrap();
    let roots = vec![programs.clone(), vendor_root.clone()];

    let before = monitor::snapshot(&roots);
    assert!(!before.keys.is_empty() && !before.programs.is_empty());
    let watcher = monitor::watch(&roots).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(150));

    // The "installer".
    assert!(Command::new(FIXTURE).args(["install", "--name", &name]).status().unwrap().success());
    // Another program writing in the same folder at the same time.
    let stranger = programs.join(format!("zz-unrelated-{}", std::process::id()));
    std::fs::create_dir_all(&stranger).unwrap();
    std::fs::write(stranger.join("data.bin"), vec![3u8; 64]).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(400));

    let watched = watcher.stop();
    let after = monitor::snapshot(&roots);
    let trace = monitor::diff(&name, &before, &after, &watched);

    let files: Vec<&str> = trace.files.iter().map(|f| f.path.as_str()).collect();
    let installed = programs.join(&name).to_string_lossy().to_lowercase();
    assert!(files.iter().any(|f| f == &installed), "the install folder is in the trace: {files:?}");
    assert!(files.iter().any(|f| f.ends_with(r"\app.exe")), "files inside it too");
    assert!(trace.bytes > 0);

    // The registry entries the installer created.
    let regs: Vec<&str> = trace.registry.iter().map(|r| r.path.as_str()).collect();
    assert!(regs.iter().any(|r| r.contains("hdcfakevendor")), "vendor key: {regs:?}");
    assert!(regs.iter().any(|r| r.contains("currentversion\\uninstall")), "uninstall entry: {regs:?}");
    assert!(regs.iter().any(|r| r.ends_with(&format!("→ {}", name.to_lowercase()))), "startup value: {regs:?}");

    // The program that appeared is identified.
    let program = registry_programs().into_iter().find(|p| p.name == name).expect("registered");
    assert_eq!(trace.program_id.as_deref(), Some(program.id.as_str()));
    assert_eq!(trace.program_name.as_deref(), Some(name.as_str()));
    assert!(trace.folders().iter().any(|d| d.eq_ignore_ascii_case(&installed)));

    // What another program wrote is recorded, but not as this program's.
    let stranger_file = stranger.join("data.bin").to_string_lossy().to_lowercase();
    let noise = trace.files.iter().find(|f| f.path == stranger_file).expect("recorded as noise");
    assert!(!noise.related, "another program's file is not attributed to this installation");
    assert!(trace.files.iter().filter(|f| f.related).all(|f| f.path.contains("monitor test")), "related items carry the program name");

    // The trace survives a round trip through the database.
    let db = hdcleaner_core::db::Database::open_in_memory().unwrap();
    let id = db.add_trace(&trace).unwrap();
    let back = db.trace(id).unwrap();
    assert_eq!(back.files.len(), trace.files.len());
    assert_eq!(back.registry.len(), trace.registry.len());

    // And it feeds the uninstaller: the items become leftovers with high confidence.
    let others: Vec<_> = registry_programs().into_iter().filter(|p| p.id != program.id).collect();
    let extra = hdcleaner_core::leftovers::from_trace(&program, &others, &back, &[]);

    // The point of the trace: something the rules alone would miss. The rules
    // look for *folders* named after the program; a loose file the installer
    // dropped in the vendor folder is only known from the trace.
    let vendor_log = vendor_root.join(format!("{name}.log")).to_string_lossy().to_lowercase();
    let rules = hdcleaner_core::leftovers::scan(&program, &others, hdcleaner_core::leftovers::Level::Advanced, false);
    assert!(!rules.iter().any(|l| l.path.eq_ignore_ascii_case(&vendor_log)), "the rules do not find it");
    assert!(extra.iter().any(|l| l.path.eq_ignore_ascii_case(&vendor_log) && l.reason == "installTrace"), "but the trace does: {:?}", extra.iter().map(|l| &l.path).collect::<Vec<_>>());
    assert!(extra.iter().any(|l| l.path.eq_ignore_ascii_case(&installed) && l.reason == "installTrace"));
    assert!(extra.iter().all(|l| l.confidence >= 90));
    assert!(!extra.iter().any(|l| l.path.eq_ignore_ascii_case(&stranger_file)), "another program's file is never proposed for removal");
    // Nothing is duplicated when the normal scan already found it.
    let again = hdcleaner_core::leftovers::from_trace(&program, &others, &back, &extra);
    assert!(again.is_empty(), "{again:?}");
    let _ = std::fs::remove_dir_all(&stranger);
}
