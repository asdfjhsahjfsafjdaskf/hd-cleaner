//! Who owns a file and whether it is signed.
//!
//! Both answers come from Windows itself: the owner from the file's security
//! descriptor, the signature from `WinVerifyTrust` (the same check Explorer's
//! Digital Signatures tab runs) plus the signer name read from the
//! certificate. Nothing here guesses: when Windows cannot tell, the answer is
//! "unknown", never "fine".

use crate::util::{from_wide, wide};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SignatureState {
    /// Signed, and Windows trusts the chain.
    Trusted,
    /// Signed, but the check failed (expired, revoked, untrusted root…).
    Untrusted,
    /// No embedded signature (it may still be catalog-signed by Windows).
    Unsigned,
    /// The file could not be checked (in use, no rights, not a PE…).
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileInfo {
    /// `DOMAIN\Account` of the owner, when it can be read.
    pub owner: Option<String>,
    pub signature: SignatureState,
    /// Who signed it, when there is a signature to read.
    pub signer: Option<String>,
    /// Windows' own explanation when the check failed.
    pub signature_detail: Option<String>,
}

// ---- owner ------------------------------------------------------------------

/// The account that owns `path` (`DOMAIN\Account`), or `None`.
pub fn owner(path: &Path) -> Option<String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{LookupAccountSidW, OWNER_SECURITY_INFORMATION, PSID, SID_NAME_USE};

    let w = wide(path.as_os_str());
    let mut sid: PSID = std::ptr::null_mut();
    let mut sd = std::ptr::null_mut();
    let rc = unsafe {
        GetNamedSecurityInfoW(
            w.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut sid,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut sd,
        )
    };
    if rc != 0 || sid.is_null() {
        return None;
    }
    let mut name = vec![0u16; 256];
    let mut domain = vec![0u16; 256];
    let (mut name_len, mut domain_len) = (name.len() as u32, domain.len() as u32);
    let mut kind: SID_NAME_USE = 0;
    let ok = unsafe {
        LookupAccountSidW(
            std::ptr::null(),
            sid,
            name.as_mut_ptr(),
            &mut name_len,
            domain.as_mut_ptr(),
            &mut domain_len,
            &mut kind,
        )
    } != 0;
    unsafe { LocalFree(sd) };
    if !ok {
        return None;
    }
    let account = from_wide(&name[..name_len as usize]);
    let dom = from_wide(&domain[..domain_len as usize]);
    Some(if dom.is_empty() { account } else { format!("{dom}\\{account}") })
}

// ---- Authenticode -----------------------------------------------------------

fn trust_message(code: i32) -> &'static str {
    // The codes WinVerifyTrust returns for a file that is signed but not
    // trusted; anything else is reported as a plain failure.
    match code as u32 {
        0x8009_2003 => "signatureCorrupt",
        0x8009_6010 => "hashMismatch",
        0x800B_0100 => "noSignature",
        0x800B_0101 => "certificateExpired",
        0x800B_0109 => "untrustedRoot",
        0x800B_010C => "certificateRevoked",
        0x800B_0111 => "explicitDistrust",
        0x800B_0004 => "subjectNotTrusted",
        _ => "checkFailed",
    }
}

/// Signer name from the file's embedded certificate.
fn signer_name(path: &Path) -> Option<String> {
    use windows_sys::Win32::Security::Cryptography::{
        CertCloseStore, CertFindCertificateInStore, CertFreeCertificateContext, CertGetNameStringW, CryptMsgClose, CryptMsgGetParam,
        CryptQueryObject, CERT_FIND_SUBJECT_CERT, CERT_INFO, CERT_NAME_SIMPLE_DISPLAY_TYPE, CERT_QUERY_CONTENT_FLAG_PKCS7_SIGNED_EMBED,
        CERT_QUERY_FORMAT_FLAG_BINARY, CERT_QUERY_OBJECT_FILE, CMSG_SIGNER_INFO, CMSG_SIGNER_INFO_PARAM, HCERTSTORE,
        X509_ASN_ENCODING, PKCS_7_ASN_ENCODING,
    };

    let w = wide(path.as_os_str());
    let mut store: HCERTSTORE = std::ptr::null_mut();
    let mut msg: *mut std::ffi::c_void = std::ptr::null_mut();
    let ok = unsafe {
        CryptQueryObject(
            CERT_QUERY_OBJECT_FILE,
            w.as_ptr() as *const std::ffi::c_void,
            CERT_QUERY_CONTENT_FLAG_PKCS7_SIGNED_EMBED,
            CERT_QUERY_FORMAT_FLAG_BINARY,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut store,
            &mut msg,
            std::ptr::null_mut(),
        )
    } != 0;
    if !ok {
        return None;
    }
    struct Cleanup(HCERTSTORE, *mut std::ffi::c_void);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            unsafe {
                if !self.1.is_null() {
                    CryptMsgClose(self.1);
                }
                if !self.0.is_null() {
                    CertCloseStore(self.0, 0);
                }
            }
        }
    }
    let _c = Cleanup(store, msg);

    // Signer info → the certificate that signed, by issuer + serial.
    let mut size = 0u32;
    if unsafe { CryptMsgGetParam(msg, CMSG_SIGNER_INFO_PARAM, 0, std::ptr::null_mut(), &mut size) } == 0 || size == 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    if unsafe { CryptMsgGetParam(msg, CMSG_SIGNER_INFO_PARAM, 0, buf.as_mut_ptr() as *mut std::ffi::c_void, &mut size) } == 0 {
        return None;
    }
    let signer = unsafe { &*(buf.as_ptr() as *const CMSG_SIGNER_INFO) };
    let mut info: CERT_INFO = unsafe { std::mem::zeroed() };
    info.Issuer = signer.Issuer;
    info.SerialNumber = signer.SerialNumber;
    let cert = unsafe {
        CertFindCertificateInStore(
            store,
            X509_ASN_ENCODING | PKCS_7_ASN_ENCODING,
            0,
            CERT_FIND_SUBJECT_CERT,
            &info as *const CERT_INFO as *const std::ffi::c_void,
            std::ptr::null(),
        )
    };
    if cert.is_null() {
        return None;
    }
    let mut name = vec![0u16; 256];
    let len = unsafe {
        CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, std::ptr::null_mut(), name.as_mut_ptr(), name.len() as u32)
    };
    unsafe { CertFreeCertificateContext(cert) };
    if len <= 1 {
        return None;
    }
    Some(from_wide(&name[..(len - 1) as usize]))
}

/// Windows signs its own files through catalogs rather than embedding the
/// signature. This is the check Explorer falls back to, and it is why
/// `notepad.exe` is signed even though the file itself carries nothing.
fn catalog_signature(path: &Path) -> Option<(SignatureState, Option<String>)> {
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Security::Cryptography::Catalog::{
        CryptCATAdminAcquireContext, CryptCATAdminCalcHashFromFileHandle, CryptCATAdminEnumCatalogFromHash,
        CryptCATAdminReleaseCatalogContext, CryptCATAdminReleaseContext, CryptCATCatalogInfoFromContext, CATALOG_INFO,
    };
    use windows_sys::Win32::Security::WinTrust::{
        WinVerifyTrust, DRIVER_ACTION_VERIFY, WINTRUST_CATALOG_INFO, WINTRUST_DATA, WINTRUST_DATA_0, WTD_CHOICE_CATALOG,
        WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE,
    };
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, OPEN_EXISTING};

    let w = wide(path.as_os_str());
    let file = unsafe {
        CreateFileW(w.as_ptr(), GENERIC_READ, FILE_SHARE_READ, std::ptr::null(), OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, std::ptr::null_mut())
    };
    if file == INVALID_HANDLE_VALUE {
        return None;
    }
    struct Handle(*mut std::ffi::c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }
    let _f = Handle(file);

    let mut admin: isize = 0;
    let mut action = DRIVER_ACTION_VERIFY;
    if unsafe { CryptCATAdminAcquireContext(&mut admin, &action, 0) } == 0 {
        return None;
    }
    struct Admin(isize);
    impl Drop for Admin {
        fn drop(&mut self) {
            unsafe { CryptCATAdminReleaseContext(self.0, 0) };
        }
    }
    let _a = Admin(admin);

    let mut size = 0u32;
    unsafe { CryptCATAdminCalcHashFromFileHandle(file, &mut size, std::ptr::null_mut(), 0) };
    if size == 0 {
        return None;
    }
    let mut hash = vec![0u8; size as usize];
    if unsafe { CryptCATAdminCalcHashFromFileHandle(file, &mut size, hash.as_mut_ptr(), 0) } == 0 {
        return None;
    }
    let catalog = unsafe { CryptCATAdminEnumCatalogFromHash(admin, hash.as_ptr(), size, 0, std::ptr::null_mut()) };
    if catalog == 0 {
        return None; // no catalog covers this file
    }
    let mut info: CATALOG_INFO = unsafe { std::mem::zeroed() };
    info.cbStruct = std::mem::size_of::<CATALOG_INFO>() as u32;
    let got = unsafe { CryptCATCatalogInfoFromContext(catalog, &mut info, 0) } != 0;
    let catalog_file: Vec<u16> = info.wszCatalogFile.iter().copied().take_while(|&c| c != 0).chain(std::iter::once(0)).collect();
    unsafe { CryptCATAdminReleaseCatalogContext(admin, catalog, 0) };
    if !got {
        return None;
    }

    // Hash as text, which the catalog check wants as the "member tag".
    let tag: String = hash.iter().map(|b| format!("{b:02X}")).collect();
    let tag_w = wide(&tag);
    let mut cat = WINTRUST_CATALOG_INFO {
        cbStruct: std::mem::size_of::<WINTRUST_CATALOG_INFO>() as u32,
        dwCatalogVersion: 0,
        pcwszCatalogFilePath: catalog_file.as_ptr(),
        pcwszMemberTag: tag_w.as_ptr(),
        pcwszMemberFilePath: w.as_ptr(),
        hMemberFile: file,
        pbCalculatedFileHash: hash.as_mut_ptr(),
        cbCalculatedFileHash: size,
        pcCatalogContext: std::ptr::null_mut(),
        hCatAdmin: admin,
    };
    let mut data: WINTRUST_DATA = unsafe { std::mem::zeroed() };
    data.cbStruct = std::mem::size_of::<WINTRUST_DATA>() as u32;
    data.dwUIChoice = WTD_UI_NONE;
    data.fdwRevocationChecks = WTD_REVOKE_NONE;
    data.dwUnionChoice = WTD_CHOICE_CATALOG;
    data.dwStateAction = WTD_STATEACTION_VERIFY;
    data.Anonymous = WINTRUST_DATA_0 { pCatalog: &mut cat };

    let rc = unsafe { WinVerifyTrust(std::ptr::null_mut(), &mut action, &mut data as *mut WINTRUST_DATA as *mut std::ffi::c_void) };
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    unsafe { WinVerifyTrust(std::ptr::null_mut(), &mut action, &mut data as *mut WINTRUST_DATA as *mut std::ffi::c_void) };

    let signer = signer_name(Path::new(&from_wide(&catalog_file[..catalog_file.len() - 1])));
    Some(if rc == 0 { (SignatureState::Trusted, signer) } else { (SignatureState::Untrusted, signer) })
}

/// Verify the Authenticode signature of `path` the way Windows does.
pub fn signature(path: &Path) -> (SignatureState, Option<String>, Option<String>) {
    use windows_sys::Win32::Security::WinTrust::{
        WinVerifyTrust, WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO, WTD_CHOICE_FILE,
        WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE,
    };

    if !path.is_file() {
        return (SignatureState::Unknown, None, None);
    }
    let w = wide(path.as_os_str());
    let mut file = WINTRUST_FILE_INFO {
        cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: w.as_ptr(),
        hFile: std::ptr::null_mut(),
        pgKnownSubject: std::ptr::null_mut(),
    };
    let mut data: WINTRUST_DATA = unsafe { std::mem::zeroed() };
    data.cbStruct = std::mem::size_of::<WINTRUST_DATA>() as u32;
    data.dwUIChoice = WTD_UI_NONE;
    data.fdwRevocationChecks = WTD_REVOKE_NONE;
    data.dwUnionChoice = WTD_CHOICE_FILE;
    data.dwStateAction = WTD_STATEACTION_VERIFY;
    data.Anonymous = WINTRUST_DATA_0 { pFile: &mut file };

    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    let rc = unsafe { WinVerifyTrust(std::ptr::null_mut(), &mut action, &mut data as *mut WINTRUST_DATA as *mut std::ffi::c_void) };
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    unsafe { WinVerifyTrust(std::ptr::null_mut(), &mut action, &mut data as *mut WINTRUST_DATA as *mut std::ffi::c_void) };

    match rc {
        0 => (SignatureState::Trusted, signer_name(path), None),
        // No embedded signature: the file may still be covered by a Windows
        // catalog, which this check does not look at — so it is not a verdict.
        code if code as u32 == 0x800B_0100 => match catalog_signature(path) {
            Some((state, signer)) => (state, signer, Some("catalogSigned".into())),
            None => (SignatureState::Unsigned, None, Some("noSignature".into())),
        },
        // Not a kind of file that carries an Authenticode signature at all.
        code if matches!(code as u32, 0x800B_0001 | 0x800B_0003) => (SignatureState::Unsigned, None, Some("notSignable".into())),
        code => {
            let detail = trust_message(code);
            let signer = signer_name(path);
            let state = if signer.is_some() { SignatureState::Untrusted } else { SignatureState::Unknown };
            (state, signer, Some(detail.into()))
        }
    }
}

/// Owner and signature of one file.
pub fn read(path: &str) -> FileInfo {
    let p = Path::new(path);
    let (signature, signer, signature_detail) = signature(p);
    FileInfo { owner: owner(p), signature, signer, signature_detail }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_own_files_are_signed_and_owned() {
        let notepad = std::path::Path::new(&crate::system::windows_dir()).join("System32").join("notepad.exe");
        if !notepad.is_file() {
            return; // not this machine's layout: nothing to assert
        }
        let info = read(&notepad.to_string_lossy());
        assert!(info.owner.is_some(), "an owner is readable: {info:?}");
        // Windows binaries are catalog-signed rather than embedded-signed, so
        // both outcomes are legitimate — what must never happen is a false
        // "trusted" with no signer behind it.
        assert_eq!(info.signature, SignatureState::Trusted, "Windows signs its own binaries: {info:?}");
        assert!(info.signer.is_some(), "trusted means there is a signer: {info:?}");
    }

    #[test]
    fn a_plain_file_is_not_signed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes.txt");
        std::fs::write(&file, b"hello").unwrap();
        let info = read(&file.to_string_lossy());
        assert_eq!(info.signature, SignatureState::Unsigned);
        assert!(info.signer.is_none());
        assert!(info.owner.is_some(), "the file we just wrote has an owner");
    }

    #[test]
    fn a_missing_file_is_unknown_not_unsigned() {
        let info = read(r"C:\this\does\not\exist\anywhere.exe");
        assert_eq!(info.signature, SignatureState::Unknown);
    }
}
