//! Cleaner end to end in a sandbox: TEMP, LOCALAPPDATA and APPDATA point to
//! temporary folders holding a fake Chrome profile (cache, History database
//! with downloads, cookies), a fake Discord cache and old/new temp files.
//! Nothing of the real user profile is touched.

use hdcleaner_core::cleaner::{self, CategoryResult};
use std::path::Path;

fn write(p: &Path, bytes: usize) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, vec![7u8; bytes]).unwrap();
}

fn age(p: &Path, days: u64) {
    let t = std::time::SystemTime::now() - std::time::Duration::from_secs(days * 86_400);
    std::fs::File::options().write(true).open(p).unwrap().set_modified(t).unwrap();
}

fn find<'a>(r: &'a [CategoryResult], id: &str) -> &'a CategoryResult {
    r.iter().find(|c| c.category.id == id).unwrap_or_else(|| panic!("category {id} missing"))
}

#[test]
fn analyze_then_clean_in_a_sandbox() {
    let sb = tempfile::tempdir().unwrap();
    let (temp, local, roaming) = (sb.path().join("Temp"), sb.path().join("Local"), sb.path().join("Roaming"));
    for d in [&temp, &local, &roaming] {
        std::fs::create_dir_all(d).unwrap();
    }
    // SAFETY: this test binary runs this single test; nothing else reads the environment concurrently.
    unsafe {
        std::env::set_var("TEMP", &temp);
        std::env::set_var("LOCALAPPDATA", &local);
        std::env::set_var("APPDATA", &roaming);
    }

    // Temp: one old file, one fresh file.
    write(&temp.join(r"old\setup.log"), 1000);
    age(&temp.join(r"old\setup.log"), 3);
    write(&temp.join("fresh.tmp"), 10);

    // Chrome profile.
    let profile = local.join(r"Google\Chrome\User Data\Default");
    write(&profile.join(r"Cache\Cache_Data\f_000001"), 4096);
    write(&profile.join(r"Code Cache\js\index"), 100);
    write(&profile.join(r"Network\Cookies"), 2048);
    write(&profile.join("Bookmarks"), 50);
    let db = rusqlite::Connection::open(profile.join("History")).unwrap();
    db.execute_batch(
        "CREATE TABLE downloads (id INTEGER PRIMARY KEY, target_path TEXT);
         CREATE TABLE downloads_url_chains (id INTEGER, chain_index INTEGER, url TEXT);
         CREATE TABLE urls (id INTEGER PRIMARY KEY, url TEXT);
         INSERT INTO downloads VALUES (1, 'C:\\Users\\x\\Downloads\\a.zip'), (2, 'C:\\Users\\x\\Downloads\\b.exe');
         INSERT INTO downloads_url_chains VALUES (1, 0, 'https://a'), (2, 0, 'https://b');
         INSERT INTO urls VALUES (1, 'https://visited');",
    )
    .unwrap();
    drop(db);

    // Discord cache.
    write(&roaming.join(r"discord\Cache\Cache_Data\data_1"), 3000);
    write(&roaming.join(r"discord\settings.json"), 20);

    // Firefox profile: two visited pages, one of them bookmarked, one download.
    let ffp = roaming.join(r"Mozilla\Firefox\Profiles\abc.default");
    std::fs::create_dir_all(&ffp).unwrap();
    let places = rusqlite::Connection::open(ffp.join("places.sqlite")).unwrap();
    places
        .execute_batch(
            "CREATE TABLE moz_places (id INTEGER PRIMARY KEY, url TEXT, foreign_count INTEGER DEFAULT 0);
             CREATE TABLE moz_historyvisits (id INTEGER PRIMARY KEY, place_id INTEGER, visit_date INTEGER);
             CREATE TABLE moz_inputhistory (place_id INTEGER, input TEXT);
             CREATE TABLE moz_bookmarks (id INTEGER PRIMARY KEY, fk INTEGER, title TEXT);
             CREATE TABLE moz_anno_attributes (id INTEGER PRIMARY KEY, name TEXT);
             CREATE TABLE moz_annos (id INTEGER PRIMARY KEY, place_id INTEGER, anno_attribute_id INTEGER, content TEXT);
             INSERT INTO moz_places VALUES (1, 'https://plain.example', 0), (2, 'https://bookmarked.example', 1), (3, 'https://dl.example', 0);
             INSERT INTO moz_historyvisits VALUES (1, 1, 100), (2, 2, 200), (3, 3, 300);
             INSERT INTO moz_bookmarks VALUES (1, 2, 'keep me');
             INSERT INTO moz_anno_attributes VALUES (1, 'downloads/destinationFileURI'), (2, 'downloads/metaData');
             INSERT INTO moz_annos VALUES (1, 3, 1, 'file:///C:/Users/x/Downloads/f.zip'), (2, 3, 2, '{}');",
        )
        .unwrap();
    drop(places);

    let results = cleaner::analyze();
    let t = find(&results, "userTemp");
    assert_eq!((t.count, t.recent_skipped), (1, 1), "only files older than 24 h");
    let cache = find(&results, "browser.chrome.cache");
    assert_eq!(cache.count, 2);
    assert!(cache.category.default_on);
    let cookies = find(&results, "browser.chrome.cookies");
    assert!(!cookies.category.default_on, "cookies never pre-selected");
    let downloads = find(&results, "browser.chrome.downloads");
    assert_eq!(downloads.count, 2);
    assert!(downloads.items.iter().any(|i| i.display.as_deref() == Some(r"C:\Users\x\Downloads\a.zip")));
    let discord = find(&results, "app.discord");
    assert_eq!(discord.count, 1);

    let backup = sb.path().join("backup");
    // Dry run changes nothing.
    let o = cleaner::clean_category("browser.chrome.cache", &cache.items, &backup, true, &mut |_| {}).unwrap();
    assert_eq!(o.removed, 2);
    assert!(profile.join(r"Cache\Cache_Data\f_000001").exists());

    // Chrome may be open on the machine running the tests: then it is refused.
    let chrome_open = find(&results, "browser.chrome.cache").running;
    for id in ["userTemp", "browser.chrome.cache", "browser.chrome.downloads", "app.discord"] {
        let r = find(&results, id);
        let res = cleaner::clean_category(id, &r.items, &backup, false, &mut |_| {});
        if r.running {
            assert!(res.is_err(), "{id} must be refused while its program runs");
            continue;
        }
        assert_eq!(res.unwrap().removed, r.count, "{id}");
    }
    assert!(!temp.join(r"old\setup.log").exists() && temp.join("fresh.tmp").exists());
    assert_eq!(profile.join(r"Cache\Cache_Data\f_000001").exists(), chrome_open);
    assert!(profile.join(r"Network\Cookies").exists(), "cookies not selected: kept");
    assert!(profile.join("Bookmarks").exists(), "bookmarks never touched");
    assert_eq!(roaming.join(r"discord\Cache\Cache_Data\data_1").exists(), discord.running, "removed unless Discord is open");
    assert!(roaming.join(r"discord\settings.json").exists(), "app settings kept");
    if chrome_open {
        return;
    }

    // Downloads list emptied, browsing history kept.
    let db = rusqlite::Connection::open(profile.join("History")).unwrap();
    let n: i64 = db.query_row("SELECT COUNT(*) FROM downloads", [], |r| r.get(0)).unwrap();
    let chains: i64 = db.query_row("SELECT COUNT(*) FROM downloads_url_chains", [], |r| r.get(0)).unwrap();
    let urls: i64 = db.query_row("SELECT COUNT(*) FROM urls", [], |r| r.get(0)).unwrap();
    assert_eq!((n, chains, urls), (0, 0, 1));

    // Firefox: history and downloads live in the bookmarks database.
    let ff_hist = find(&results, "browser.firefox.history");
    assert_eq!(ff_hist.count, 3, "three visited pages");
    let ff_dl = find(&results, "browser.firefox.downloads");
    assert_eq!(ff_dl.count, 1);
    assert_eq!(ff_dl.items[0].display.as_deref(), Some(r"C:\Users\x\Downloads\f.zip"));
    if !ff_hist.running {
        cleaner::clean_category("browser.firefox.history", &ff_hist.items, &backup, false, &mut |_| {}).unwrap();
        cleaner::clean_category("browser.firefox.downloads", &ff_dl.items, &backup, false, &mut |_| {}).unwrap();
        let db = rusqlite::Connection::open(ffp.join("places.sqlite")).unwrap();
        let visits: i64 = db.query_row("SELECT COUNT(*) FROM moz_historyvisits", [], |r| r.get(0)).unwrap();
        let kept: i64 = db.query_row("SELECT COUNT(*) FROM moz_places WHERE foreign_count > 0", [], |r| r.get(0)).unwrap();
        let gone: i64 = db.query_row("SELECT COUNT(*) FROM moz_places WHERE foreign_count = 0", [], |r| r.get(0)).unwrap();
        let annos: i64 = db.query_row("SELECT COUNT(*) FROM moz_annos", [], |r| r.get(0)).unwrap();
        assert_eq!((visits, kept, gone, annos), (0, 1, 0, 0), "visits and downloads gone, bookmarked page kept");
        assert!(backup.join("abc.default-places.sqlite").is_file(), "database backed up before the change");
    }

    // An item not produced by the analysis is refused.
    let fake = vec![cleaner::CleanItem { path: profile.join("Bookmarks").to_string_lossy().into(), size: 50, mtime: 0, display: None }];
    let o = cleaner::clean_category("browser.chrome.cache", &fake, &backup, false, &mut |_| {}).unwrap();
    assert_eq!((o.removed, o.failed), (0, 1));
    assert!(profile.join("Bookmarks").exists());
}
