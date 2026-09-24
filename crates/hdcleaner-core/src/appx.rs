//! AppX / MSIX / Store packages of the current user via the WinRT
//! `Windows.Management.Deployment.PackageManager` (no elevation needed).
//! Frameworks and resource packages are reported as frameworks (hidden by
//! default in the UI) so dependencies stay visible on request.

use crate::programs::{Arch, Program, ProgramSource, Scope};
use crate::{AppError, Result};
use windows::core::HSTRING;
use windows::ApplicationModel::{Package, PackageSignatureKind};
use windows::Management::Deployment::PackageManager;
use windows::System::ProcessorArchitecture;

fn werr(e: windows::core::Error, ctx: &str) -> AppError {
    AppError::Win32 { code: e.code().0 as u32, context: format!("{ctx}: {}", e.message()), path: None }
}

/// WinRT DateTime (100 ns since 1601) → YYYY-MM-DD.
fn date_of(ticks: i64) -> Option<String> {
    let ms = crate::util::filetime_to_unix_ms(ticks as u64);
    (ms > 0).then(|| crate::format::datetime(ms)[..10].to_string())
}

fn to_program(p: &Package) -> Option<Program> {
    let id = p.Id().ok()?;
    let full = id.FullName().ok()?.to_string();
    let name_id = id.Name().ok()?.to_string();
    let display = p.DisplayName().ok().map(|s| s.to_string()).filter(|s| !s.is_empty() && !s.starts_with("ms-resource:"));
    let v = id.Version().ok()?;
    let mut prog = Program {
        id: format!("appx:{full}"),
        name: display.unwrap_or_else(|| name_id.clone()),
        version: Some(format!("{}.{}.{}.{}", v.Major, v.Minor, v.Build, v.Revision)),
        publisher: p.PublisherDisplayName().ok().map(|s| s.to_string()).filter(|s| !s.is_empty()),
        install_date: p.InstalledDate().ok().and_then(|d| date_of(d.UniversalTime)),
        install_location: p.InstalledPath().ok().map(|s| s.to_string()).filter(|s| !s.is_empty()),
        inferred_location: None,
        uninstall_string: None,
        quiet_uninstall_string: None,
        modify_path: None,
        reported_size: None,
        icon: None,
        url: None,
        source: ProgramSource::Appx,
        arch: match id.Architecture().ok() {
            Some(ProcessorArchitecture::X64) => Arch::X64,
            Some(ProcessorArchitecture::X86) => Arch::X86,
            Some(ProcessorArchitecture::Arm64) => Arch::Arm64,
            Some(ProcessorArchitecture::Neutral) => Arch::Neutral,
            _ => Arch::Unknown,
        },
        scope: Scope::User,
        registry_key: None,
        msi_product_code: None,
        system_component: false,
        is_update: false,
        no_remove: false,
        package_full_name: Some(full),
        package_family_name: id.FamilyName().ok().map(|s| s.to_string()),
        is_framework: p.IsFramework().unwrap_or(false) || p.IsResourcePackage().unwrap_or(false),
        signature_kind: None,
        dependencies: Vec::new(),
    };
    let kind = p.SignatureKind().ok();
    prog.signature_kind = Some(
        match kind {
            Some(PackageSignatureKind::System) => "system",
            Some(PackageSignatureKind::Store) => "store",
            Some(PackageSignatureKind::Developer) => "developer",
            Some(PackageSignatureKind::Enterprise) => "enterprise",
            _ => "none",
        }
        .to_string(),
    );
    if kind == Some(PackageSignatureKind::Store) {
        prog.source = ProgramSource::Store;
    }
    // System-signed packages ship with Windows: removable only with care.
    prog.system_component = kind == Some(PackageSignatureKind::System);
    if let Ok(deps) = p.Dependencies() {
        for d in deps {
            if let Ok(n) = d.Id().and_then(|i| i.FullName()) {
                prog.dependencies.push(n.to_string());
            }
        }
    }
    if let Ok(logo) = p.Logo() {
        if let Ok(uri) = logo.AbsoluteUri() {
            // file:///C:/... → local path (only local files are ever read).
            let s = uri.to_string();
            if let Some(rest) = s.strip_prefix("file:///") {
                prog.icon = crate::icons::resolve_appx_logo(&percent_decode(rest).replace('/', "\\"));
            }
        }
    }
    Some(prog)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Packages installed for the current user.
pub fn packages() -> Result<Vec<Program>> {
    let pm = PackageManager::new().map_err(|e| werr(e, "creating PackageManager"))?;
    let list = pm.FindPackagesByUserSecurityId(&HSTRING::new()).map_err(|e| werr(e, "listing packages"))?;
    let mut out = Vec::new();
    for p in list {
        if let Some(prog) = to_program(&p) {
            out.push(prog);
        }
    }
    Ok(out)
}

/// Packages Windows itself needs: removing them breaks the shell or the
/// Store, so the app refuses and says so.
pub fn is_critical(p: &Program) -> bool {
    let family = p.package_family_name.as_deref().unwrap_or("").to_lowercase();
    const CRITICAL: &[&str] = &[
        "microsoft.windows.shellexperiencehost",
        "microsoft.windows.startmenuexperiencehost",
        "microsoft.windows.search",
        "microsoft.windows.cortana",
        "microsoft.ui.xaml",
        "microsoft.vclibs",
        "microsoft.net.native",
        "microsoft.windowsstore",
        "microsoft.desktopappinstaller",
        "microsoft.accountscontrol",
        "microsoft.windows.sechealthui",
        "windows.cbspreview",
        "microsoft.windows.immersivecontrolpanel",
        "microsoft.windows.contentdeliverymanager",
        "microsoft.windows.peopleexperiencehost",
        "microsoft.aad.brokerplugin",
        "microsoft.creddialoghost",
    ];
    p.signature_kind.as_deref() == Some("system") || CRITICAL.iter().any(|c| family.starts_with(c))
}

/// Remove a package. `all_users` needs administrator rights and goes through
/// the elevated helper; without it only the current user's copy goes.
pub fn remove(full_name: &str, all_users: bool) -> Result<()> {
    use windows::core::HSTRING;
    use windows::Management::Deployment::{PackageManager, RemovalOptions};
    let map = |e: windows::core::Error| AppError::Win32 {
        code: e.code().0 as u32,
        context: format!("removing package: {}", e.message()),
        path: Some(full_name.into()),
    };
    let pm = PackageManager::new().map_err(map)?;
    let options = if all_users { RemovalOptions::RemoveForAllUsers } else { RemovalOptions::None };
    let op = pm.RemovePackageWithOptionsAsync(&HSTRING::from(full_name), options).map_err(map)?;
    let result = op.join().map_err(map)?;
    let hr = result.ExtendedErrorCode().map_err(map)?;
    if hr.is_err() {
        let text = result.ErrorText().map(|t| t.to_string()).unwrap_or_default();
        return Err(AppError::Win32 { code: hr.0 as u32, context: format!("removing package: {text}"), path: Some(full_name.into()) });
    }
    Ok(())
}

/// Re-register a package for the current user — the same repair Windows does
/// for an app that stopped opening. It reinstalls nothing and keeps app data.
pub fn repair(full_name: &str) -> Result<()> {
    use windows::core::HSTRING;
    use windows::Management::Deployment::{DeploymentOptions, PackageManager};
    let map = |e: windows::core::Error| AppError::Win32 {
        code: e.code().0 as u32,
        context: format!("repairing package: {}", e.message()),
        path: Some(full_name.into()),
    };
    let pm = PackageManager::new().map_err(map)?;
    let op = pm
        .RegisterPackageByFullNameAsync(&HSTRING::from(full_name), None, DeploymentOptions::None)
        .map_err(map)?;
    let result = op.join().map_err(map)?;
    let hr = result.ExtendedErrorCode().map_err(map)?;
    if hr.is_err() {
        let text = result.ErrorText().map(|t| t.to_string()).unwrap_or_default();
        return Err(AppError::Win32 { code: hr.0 as u32, context: format!("repairing package: {text}"), path: Some(full_name.into()) });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn lists_packages() {
        let list = super::packages().expect("PackageManager available on Windows 10+");
        // Windows ships with system-signed packages (e.g. shell experiences).
        assert!(list.iter().any(|p| p.signature_kind.as_deref() == Some("system")));
        assert!(list.iter().all(|p| p.package_full_name.is_some()));
        assert_eq!(super::percent_decode("C:/Program%20Files/x"), "C:/Program Files/x");
    }
}
