//! Putting files on the Windows clipboard, so Explorer can paste them.
//!
//! This is the same handoff Explorer itself uses: the list of paths as
//! `CF_HDROP` plus the "Preferred DropEffect" that says whether pasting
//! copies or moves. The app never moves anything itself here — the paste,
//! wherever the user does it, is what acts.

use crate::util::wide;
use crate::{AppError, Result};

/// Put `paths` on the clipboard. With `cut`, a paste in Explorer *moves*
/// them; otherwise it copies.
pub fn set_files(paths: &[String], cut: bool) -> Result<()> {
    use windows_sys::Win32::Foundation::HGLOBAL;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows_sys::Win32::System::Ole::CF_HDROP;
    use windows_sys::Win32::UI::Shell::DROPFILES;

    if paths.is_empty() {
        return Err(AppError::InvalidInput("nothing to put on the clipboard".into()));
    }
    // CF_HDROP: a DROPFILES header, then the paths as one UTF-16 block with a
    // double NUL at the end.
    let mut block: Vec<u16> = Vec::new();
    for p in paths {
        block.extend(p.encode_utf16());
        block.push(0);
    }
    block.push(0);
    let header = std::mem::size_of::<DROPFILES>();
    let bytes = header + block.len() * 2;

    unsafe {
        let handle: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if handle.is_null() {
            return Err(AppError::Helper("could not allocate clipboard memory".into()));
        }
        let ptr = GlobalLock(handle);
        if ptr.is_null() {
            GlobalFree(handle);
            return Err(AppError::Helper("could not lock clipboard memory".into()));
        }
        let drop_files = ptr as *mut DROPFILES;
        (*drop_files).pFiles = header as u32;
        (*drop_files).fWide = 1;
        (*drop_files).pt = std::mem::zeroed();
        (*drop_files).fNC = 0;
        std::ptr::copy_nonoverlapping(block.as_ptr(), (ptr as *mut u8).add(header) as *mut u16, block.len());
        GlobalUnlock(handle);

        if OpenClipboard(std::ptr::null_mut()) == 0 {
            GlobalFree(handle);
            return Err(AppError::Helper("the clipboard is in use by another program".into()));
        }
        EmptyClipboard();
        if SetClipboardData(CF_HDROP as u32, handle).is_null() {
            GlobalFree(handle);
            CloseClipboard();
            return Err(AppError::Helper("the clipboard refused the file list".into()));
        }
        // Windows now owns `handle`.

        // The drop effect: 2 = move (cut), 5 = copy.
        let name = wide("Preferred DropEffect");
        let format = RegisterClipboardFormatW(name.as_ptr());
        if format != 0 {
            let effect: u32 = if cut { 2 } else { 5 };
            let eh: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE, 4);
            if !eh.is_null() {
                let ep = GlobalLock(eh);
                if !ep.is_null() {
                    std::ptr::copy_nonoverlapping(&effect as *const u32, ep as *mut u32, 1);
                    GlobalUnlock(eh);
                    if SetClipboardData(format, eh).is_null() {
                        GlobalFree(eh);
                    }
                } else {
                    GlobalFree(eh);
                }
            }
        }
        CloseClipboard();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_an_empty_list() {
        assert!(set_files(&[], true).is_err());
    }

    #[test]
    fn puts_a_real_file_on_the_clipboard() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("cut me.txt");
        std::fs::write(&file, b"x").unwrap();
        // The clipboard is shared with the whole session: this only checks the
        // call is accepted, and the file itself is left untouched.
        set_files(&[file.to_string_lossy().into_owned()], true).unwrap();
        assert!(file.exists(), "putting a file on the clipboard never moves it");
    }
}
