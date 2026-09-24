//! Program icons as PNG bytes.
//!
//! `.exe`/`.dll` icons are extracted with `SHDefExtractIconW`, which loads the
//! file as a *resource* (it is never executed). `.ico` and `.png` files (AppX
//! logos) are handled too. Output is always re-encoded PNG, so the UI never
//! renders arbitrary file bytes.

use crate::util::wide;
use crate::{AppError, Result};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::Shell::SHDefExtractIconW;
use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, DrawIconEx, DI_NORMAL, HICON};

const MAX_FILE: u64 = 4 * 1024 * 1024;

fn encode_png(w: u32, h: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(|e| AppError::Corrupt(e.to_string()))?;
        writer.write_image_data(rgba).map_err(|e| AppError::Corrupt(e.to_string()))?;
    }
    Ok(out)
}

/// Render an HICON into RGBA pixels (handles icons with and without alpha).
fn render(icon: HICON, size: i32) -> Option<Vec<u8>> {
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        let dc = CreateCompatibleDC(screen);
        ReleaseDC(std::ptr::null_mut(), screen);
        if dc.is_null() {
            return None;
        }
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = size;
        bmi.bmiHeader.biHeight = -size; // top-down
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;
        // Draw twice, on black and on white: alpha = 255 - (white - black).
        // Works for 32-bit alpha icons and old masked icons alike.
        let mut shots: Vec<Vec<u8>> = Vec::with_capacity(2);
        for bg in [0u8, 255u8] {
            let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
            let bmp = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
            if bmp.is_null() || bits.is_null() {
                DeleteDC(dc);
                return None;
            }
            let old = SelectObject(dc, bmp);
            let len = (size * size * 4) as usize;
            std::ptr::write_bytes(bits as *mut u8, bg, len);
            DrawIconEx(dc, 0, 0, icon, size, size, 0, std::ptr::null_mut(), DI_NORMAL);
            GdiFlush();
            shots.push(std::slice::from_raw_parts(bits as *const u8, len).to_vec());
            SelectObject(dc, old);
            DeleteObject(bmp);
        }
        DeleteDC(dc);
        let (black, white) = (&shots[0], &shots[1]);
        let mut rgba = vec![0u8; black.len()];
        for i in (0..black.len()).step_by(4) {
            let a = 255 - (white[i + 1] as i32 - black[i + 1] as i32).clamp(0, 255);
            if a > 0 {
                // Un-premultiply from the black render (BGR order).
                let un = |c: u8| ((c as i32 * 255 + a / 2) / a).min(255) as u8;
                rgba[i] = un(black[i + 2]);
                rgba[i + 1] = un(black[i + 1]);
                rgba[i + 2] = un(black[i]);
                rgba[i + 3] = a as u8;
            }
        }
        rgba.as_chunks::<4>().0.iter().any(|p| p[3] > 0).then_some(rgba)
    }
}

fn from_resource(path: &str, index: i32, size: u32) -> Result<Vec<u8>> {
    let w = wide(path);
    let mut large: HICON = std::ptr::null_mut();
    let hr = unsafe { SHDefExtractIconW(w.as_ptr(), index, 0, &mut large, std::ptr::null_mut(), size) };
    if hr != 0 || large.is_null() {
        return Err(AppError::NotFound { path: format!("{path},{index}") });
    }
    let px = render(large, size as i32);
    unsafe { DestroyIcon(large) };
    let px = px.ok_or_else(|| AppError::NotFound { path: path.to_string() })?;
    encode_png(size, size, &px)
}

/// PNG icon for an icon reference (`path,index`, `.ico`, `.png`).
pub fn icon_png(reference: &str, size: u32) -> Result<Vec<u8>> {
    let size = size.clamp(16, 64);
    let (path, index) = crate::programs::parse_icon_ref(reference)
        .ok_or_else(|| AppError::InvalidInput("invalid icon reference".into()))?;
    let lower = path.to_lowercase();
    if lower.ends_with(".png") {
        // AppX logo: validate it really is a PNG and not something huge.
        let meta = std::fs::metadata(&path).map_err(|e| AppError::io("reading icon", Some(path.as_ref()), e))?;
        if meta.len() > MAX_FILE {
            return Err(AppError::InvalidInput("icon file too large".into()));
        }
        let bytes = std::fs::read(&path).map_err(|e| AppError::io("reading icon", Some(path.as_ref()), e))?;
        if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err(AppError::Corrupt("not a PNG file".into()));
        }
        return Ok(bytes);
    }
    from_resource(&path, index, size)
}

/// Best icon for a program: its icon reference (DisplayIcon or uninstaller),
/// then an `.ico` in the install folder, then an `.exe` there whose name
/// matches the program. Only files directly inside the folder are considered.
pub fn program_icon_png(p: &crate::programs::Program, size: u32) -> Option<Vec<u8>> {
    if let Some(png) = p.icon.as_deref().and_then(|r| icon_png(r, size).ok()) {
        return Some(png);
    }
    let dir = p.install_location.as_deref().or(p.inferred_location.as_deref())?;
    let entries: Vec<std::path::PathBuf> = std::fs::read_dir(crate::util::to_extended(dir))
        .ok()?
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| std::path::PathBuf::from(dir).join(e.file_name()))
        .take(500)
        .collect();
    let ext_is = |p: &std::path::Path, e: &str| p.extension().is_some_and(|x| x.eq_ignore_ascii_case(e));
    let keys = crate::appsize::name_keys(p);
    let named_exe = entries.iter().filter(|f| ext_is(f, "exe")).find(|f| {
        let stem = crate::appsize::norm(&f.file_stem().unwrap_or_default().to_string_lossy());
        keys.iter().any(|k| *k == stem || (stem.len() >= 4 && k.starts_with(&stem)))
    });
    let candidates = entries.iter().filter(|f| ext_is(f, "ico")).chain(named_exe);
    for c in candidates {
        if let Ok(png) = icon_png(&c.to_string_lossy(), size) {
            return Some(png);
        }
    }
    None
}

/// AppX logos are often declared as `Logo.png` while the file on disk is a
/// scale variant (`Logo.scale-100.png`). Resolve to an existing file.
pub fn resolve_appx_logo(path: &str) -> Option<String> {
    if std::path::Path::new(path).is_file() {
        return Some(path.to_string());
    }
    let p = std::path::Path::new(path);
    let dir = p.parent()?;
    let stem = p.file_stem()?.to_string_lossy().to_lowercase();
    let mut best: Option<(u32, String)> = None;
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().to_lowercase();
        if !(name.starts_with(&format!("{stem}.")) && name.ends_with(".png")) || name.contains("contrast-") {
            continue;
        }
        // Prefer scale-100/200 or targetsize-32/48.
        let score = if name.contains("targetsize-32") || name.contains("targetsize-48") {
            0
        } else if name.contains("scale-100") {
            1
        } else if name.contains("scale-200") {
            2
        } else {
            5
        };
        if best.as_ref().is_none_or(|(s, _)| score < *s) {
            best = Some((score, e.path().to_string_lossy().into_owned()));
        }
    }
    best.map(|(_, p)| p)
}

#[cfg(test)]
mod tests {
    #[test]
    fn extracts_notepad_icon() {
        let win = crate::system::windows_dir();
        let png = super::icon_png(&format!(r"{win}\System32\notepad.exe,0"), 32).unwrap();
        assert!(png.starts_with(b"\x89PNG"));
        let img = png::Decoder::new(std::io::Cursor::new(&png)).read_info().unwrap();
        assert_eq!(img.info().width, 32);
        assert!(super::icon_png(r"C:\definitely\missing.exe,0", 32).is_err());
    }
}
