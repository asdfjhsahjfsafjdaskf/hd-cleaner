//! "End process tree": children first, each re-verified; nothing else touched.

use hdcleaner_core::processes::{descendants, image_path, terminate_tree, ProcessSampler};
use std::process::Command;

const FIXTURE: &str = env!("CARGO_BIN_EXE_hdcleaner-fake-app");

#[test]
fn ends_a_process_and_its_children() {
    // Copy the fixture so the tree runs from its own path (not Windows).
    let dir = std::env::temp_dir().join(format!("hdc-tree-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("tree-app.exe");
    std::fs::copy(FIXTURE, &exe).unwrap();
    let mut parent = Command::new(&exe).arg("spawn-tree").spawn().unwrap();
    let pid = parent.id();

    // Wait for the child to appear.
    let mut kids = Vec::new();
    for _ in 0..50 {
        kids = descendants(pid);
        if !kids.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert_eq!(kids.len(), 1, "one child: {kids:?}");
    let child = kids[0].pid;

    // The sampler sees both, with memory and owner.
    let mut s = ProcessSampler::default();
    s.sample();
    std::thread::sleep(std::time::Duration::from_millis(300));
    let rows = s.sample();
    let row = rows.iter().find(|r| r.pid == pid).expect("parent sampled");
    assert!(row.private_bytes.unwrap_or(0) > 0 && row.user.is_some() && row.cpu.is_some() && !row.is_windows);

    // A wrong path is refused before anything is touched.
    assert!(terminate_tree(pid, r"C:\somewhere\else.exe").is_err());
    assert!(image_path(child).is_some(), "child untouched after refusal");

    let path = image_path(pid).unwrap();
    let r = terminate_tree(pid, &path).unwrap();
    assert_eq!(r.len(), 2);
    assert_eq!(r[0].pid, child, "children first");
    assert!(r.iter().all(|k| k.ok), "{r:?}");
    let _ = parent.wait();
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(image_path(child).is_none() && image_path(pid).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}
