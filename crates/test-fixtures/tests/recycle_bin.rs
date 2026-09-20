//! Recycle Bin: the analysis lists each item with its original path, and
//! cleaning removes only the items handed to it — never the whole bin.

use hdcleaner_core::cleaner::{self, CleanItem};

#[test]
fn removes_only_the_listed_item() {
    let dir = std::env::temp_dir().join(format!("hdc-recycle-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("hdcleaner recycle test.txt");
    std::fs::write(&file, vec![9u8; 4096]).unwrap();
    let path = file.to_string_lossy().to_string();
    if hdcleaner_core::fsops::recycle(&path).is_err() {
        return; // no Recycle Bin on this volume (e.g. a network TEMP)
    }
    assert!(!file.exists(), "moved to the Recycle Bin");

    let cat = cleaner::catalog().into_iter().find(|c| c.id == "recycleBin").unwrap();
    let before = cleaner::analyze_category(&cat, &[]);
    let mine: Vec<CleanItem> = before.items.iter().filter(|i| i.display.as_deref() == Some(path.as_str())).cloned().collect();
    assert_eq!(mine.len(), 1, "the recycled file is listed with its original path");
    assert_eq!(mine[0].size, 4096);

    // Dry run keeps it.
    let backup = dir.join("backup");
    let o = cleaner::clean_category("recycleBin", &mine, &backup, true, &mut |_| {}).unwrap();
    assert_eq!(o.removed, 1);
    assert!(std::path::Path::new(&mine[0].path).exists());

    let o = cleaner::clean_category("recycleBin", &mine, &backup, false, &mut |_| {}).unwrap();
    assert_eq!((o.removed, o.freed), (1, 4096));
    let after = cleaner::analyze_category(&cat, &[]);
    assert_eq!(after.count, before.count - 1, "everything else in the bin is untouched");
    assert!(!std::path::Path::new(&mine[0].path).exists());

    // Anything outside the Recycle Bin is refused.
    let outside = vec![CleanItem { path: dir.join("x.txt").to_string_lossy().into(), size: 1, mtime: 0, display: None }];
    let o = cleaner::clean_category("recycleBin", &outside, &backup, false, &mut |_| {}).unwrap();
    assert_eq!((o.removed, o.failed), (0, 1));
    let _ = std::fs::remove_dir_all(&dir);
}
