//! `hdcleaner-fake-app`: a fake per-user program for testing the uninstaller.
//!
//! * `install --name N`   writes an install folder, AppData data + cache + logs,
//!   a vendor registry key, an uninstall entry, a Start Menu shortcut and a
//!   Run entry (everything under the current user: no admin needed).
//! * `uninstall --name N` is a deliberately *sloppy* uninstaller: it removes only
//!   its uninstall entry and `app.exe`, leaving the rest as leftovers.
//! * `cleanup --name N`   removes everything (test teardown).

use std::path::PathBuf;
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::*;

const VENDOR: &str = "HdcFakeVendor";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn env(v: &str) -> PathBuf {
    PathBuf::from(std::env::var(v).expect("environment variable"))
}

fn key_name(name: &str) -> String {
    name.chars().filter(|c| c.is_alphanumeric()).collect()
}

struct Paths {
    install: PathBuf,
    roaming: PathBuf,
    local: PathBuf,
    shortcut: PathBuf,
}

fn paths(name: &str) -> Paths {
    Paths {
        install: env("LOCALAPPDATA").join("Programs").join(name),
        roaming: env("APPDATA").join(name),
        local: env("LOCALAPPDATA").join(name),
        shortcut: env("APPDATA").join(r"Microsoft\Windows\Start Menu\Programs").join(format!("{name}.lnk")),
    }
}

fn startup_link(name: &str) -> PathBuf {
    env("APPDATA").join(r"Microsoft\Windows\Start Menu\Programs\Startup").join(format!("{name}.lnk"))
}

fn set_str(h: HKEY, name: &str, value: &str) {
    let n = wide(name);
    let v = wide(value);
    unsafe { RegSetValueExW(h, n.as_ptr(), 0, REG_SZ, v.as_ptr() as *const u8, (v.len() * 2) as u32) };
}

fn create_key(path: &str) -> HKEY {
    let p = wide(path);
    let mut h: HKEY = std::ptr::null_mut();
    let rc = unsafe {
        RegCreateKeyExW(HKEY_CURRENT_USER, p.as_ptr(), 0, std::ptr::null(), 0, KEY_ALL_ACCESS, std::ptr::null(), &mut h, std::ptr::null_mut())
    };
    assert_eq!(rc, ERROR_SUCCESS, "creating HKCU\\{path}");
    h
}

fn delete_tree(path: &str) {
    let p = wide(path);
    unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, p.as_ptr()) };
    unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, p.as_ptr()) };
}

fn delete_value(path: &str, value: &str) {
    let p = wide(path);
    let v = wide(value);
    unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, p.as_ptr(), v.as_ptr()) };
}

fn create_shortcut(link: &std::path::Path, target: &std::path::Path) {
    use windows::core::{Interface, HSTRING, PCWSTR};
    use windows::Win32::System::Com::*;
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let sl: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).expect("ShellLink");
        sl.SetPath(PCWSTR(HSTRING::from(target.as_os_str()).as_ptr())).expect("SetPath");
        let pf: IPersistFile = sl.cast().expect("IPersistFile");
        pf.Save(PCWSTR(HSTRING::from(link.as_os_str()).as_ptr()), true).expect("save shortcut");
        CoUninitialize();
    }
}

fn install(name: &str) {
    let p = paths(name);
    std::fs::create_dir_all(&p.install).unwrap();
    let me = std::env::current_exe().unwrap();
    std::fs::copy(&me, p.install.join("app.exe")).unwrap();
    std::fs::copy(&me, p.install.join("uninstall.exe")).unwrap();
    std::fs::write(p.install.join("readme.txt"), "fake program used by tests").unwrap();
    std::fs::create_dir_all(&p.roaming).unwrap();
    std::fs::write(p.roaming.join("settings.json"), r#"{"theme":"dark"}"#).unwrap();
    std::fs::create_dir_all(p.local.join("Cache")).unwrap();
    std::fs::write(p.local.join("Cache").join("blob.bin"), vec![7u8; 256 * 1024]).unwrap();
    std::fs::create_dir_all(p.local.join("logs")).unwrap();
    std::fs::write(p.local.join("logs").join("app.log"), "started\n").unwrap();

    let vendor = create_key(&format!(r"Software\{VENDOR}\{}", key_name(name)));
    set_str(vendor, "InstallDir", &p.install.to_string_lossy());
    unsafe { RegCloseKey(vendor) };

    let un = create_key(&format!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{}", key_name(name)));
    set_str(un, "DisplayName", name);
    set_str(un, "DisplayVersion", "1.0.0");
    set_str(un, "Publisher", VENDOR);
    set_str(un, "InstallLocation", &p.install.to_string_lossy());
    let uninstaller = p.install.join("uninstall.exe");
    set_str(un, "UninstallString", &format!("\"{}\" uninstall --name \"{name}\"", uninstaller.display()));
    set_str(un, "DisplayIcon", &format!("{},0", p.install.join("app.exe").display()));
    unsafe { RegCloseKey(un) };

    let run = create_key(r"Software\Microsoft\Windows\CurrentVersion\Run");
    set_str(run, name, &format!("\"{}\" --tray", p.install.join("app.exe").display()));
    unsafe { RegCloseKey(run) };

    create_shortcut(&p.shortcut, &p.install.join("app.exe"));
    println!("installed {name}");
}

fn sloppy_uninstall(name: &str) {
    let p = paths(name);
    // Simulate an uninstaller doing some work for a moment.
    std::thread::sleep(std::time::Duration::from_millis(700));
    delete_tree(&format!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{}", key_name(name)));
    let _ = std::fs::remove_file(p.install.join("app.exe"));
    println!("uninstalled {name} (leaving leftovers on purpose)");
}

fn cleanup(name: &str) {
    let p = paths(name);
    for d in [&p.install, &p.roaming, &p.local] {
        let _ = std::fs::remove_dir_all(d);
    }
    let _ = std::fs::remove_file(&p.shortcut);
    let _ = std::fs::remove_file(startup_link(name));
    delete_value(r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run", name);
    delete_value(r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\StartupFolder", &format!("{name}.lnk"));
    delete_tree(&format!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{}", key_name(name)));
    delete_tree(&format!(r"Software\{VENDOR}\{}", key_name(name)));
    delete_tree(&format!(r"Software\{VENDOR}"));
    delete_value(r"Software\Microsoft\Windows\CurrentVersion\Run", name);
    println!("cleaned {name}");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let name = args
        .iter()
        .position(|a| a == "--name")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "HDC Fake App".to_string());
    match args.first().map(String::as_str) {
        Some("install") => install(&name),
        Some("uninstall") => sloppy_uninstall(&name),
        Some("cleanup") => cleanup(&name),
        // Behaves like a resident app (tray) so tests can find a running process.
        Some("--tray") => std::thread::sleep(std::time::Duration::from_secs(120)),
        // A parent process with one child (for "end process tree").
        Some("spawn-tree") => {
            let me = std::env::current_exe().unwrap();
            let _child = std::process::Command::new(me).arg("--tray").spawn().unwrap();
            std::thread::sleep(std::time::Duration::from_secs(120));
        }
        // A shortcut in the user's Startup folder.
        Some("startup-link") => {
            let p = paths(&name);
            create_shortcut(&startup_link(&name), &p.install.join("app.exe"));
            println!("startup link for {name}");
        }
        Some("break") => {
            // Simulate a broken install: the uninstaller is gone.
            let _ = std::fs::remove_file(paths(&name).install.join("uninstall.exe"));
            println!("broke {name}");
        }
        None => println!("fake app (does nothing)"),
        Some(other) => {
            eprintln!("unknown command {other}; use install | uninstall | cleanup [--name N]");
            std::process::exit(2);
        }
    }
}
