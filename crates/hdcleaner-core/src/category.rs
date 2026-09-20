//! File categories used by the treemap colours, smart storage and filters.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
#[repr(u8)]
pub enum Category {
    Other = 0,
    Video = 1,
    Image = 2,
    Audio = 3,
    Executable = 4,
    Archive = 5,
    Game = 6,
    Document = 7,
    Code = 8,
    Cache = 9,
    System = 10,
    Installer = 11,
    DiskImage = 12,
}

impl Category {
    pub const ALL: [Category; 13] = [
        Category::Other,
        Category::Video,
        Category::Image,
        Category::Audio,
        Category::Executable,
        Category::Archive,
        Category::Game,
        Category::Document,
        Category::Code,
        Category::Cache,
        Category::System,
        Category::Installer,
        Category::DiskImage,
    ];

    pub fn from_u8(v: u8) -> Category {
        Self::ALL.get(v as usize).copied().unwrap_or(Category::Other)
    }

    pub fn name(self) -> &'static str {
        match self {
            Category::Other => "other",
            Category::Video => "video",
            Category::Image => "image",
            Category::Audio => "audio",
            Category::Executable => "executable",
            Category::Archive => "archive",
            Category::Game => "game",
            Category::Document => "document",
            Category::Code => "code",
            Category::Cache => "cache",
            Category::System => "system",
            Category::Installer => "installer",
            Category::DiskImage => "diskImage",
        }
    }

    pub fn parse(s: &str) -> Option<Category> {
        let s = s.to_ascii_lowercase();
        let alias = match s.as_str() {
            "videos" => "video",
            "images" | "picture" | "pictures" | "photo" | "photos" => "image",
            "music" => "audio",
            "exe" | "executables" | "program" => "executable",
            "archives" | "compressed" | "zip" => "archive",
            "games" => "game",
            "documents" | "doc" | "docs" => "document",
            "source" | "dev" => "code",
            "installers" | "setup" => "installer",
            "iso" | "diskimage" | "disk-image" => "diskImage",
            other => other,
        };
        Self::ALL.iter().copied().find(|c| c.name().eq_ignore_ascii_case(alias))
    }
}

/// Category for a lowercase extension (without the dot).
pub fn category_for_ext(ext: &str) -> Category {
    use Category::*;
    match ext {
        "mp4" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "webm" | "m4v" | "mpg" | "mpeg" | "ts" | "m2ts" | "3gp"
        | "vob" => Video,
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "tif" | "tiff" | "webp" | "heic" | "heif" | "raw" | "cr2" | "nef"
        | "arw" | "dng" | "psd" | "svg" | "ico" | "avif" => Image,
        "mp3" | "flac" | "wav" | "aac" | "ogg" | "m4a" | "wma" | "opus" | "aiff" | "mid" => Audio,
        "exe" | "dll" | "sys" | "ocx" | "com" | "scr" | "cpl" | "drv" | "efi" | "mui" => Executable,
        "msi" | "msix" | "msixbundle" | "appx" | "appxbundle" | "msp" | "msu" | "cab" => Installer,
        "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "zst" | "tgz" | "lz4" | "lzma" => Archive,
        "iso" | "img" | "vhd" | "vhdx" | "vmdk" | "vdi" | "qcow2" | "wim" | "esd" => DiskImage,
        "pak" | "vpk" | "bsa" | "ba2" | "upk" | "uasset" | "umap" | "forge" | "bik" | "big" | "gcf" | "ucas"
        | "utoc" => Game,
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "odt" | "ods" | "odp" | "rtf" | "txt" | "md"
        | "epub" | "csv" | "one" | "pst" | "ost" => Document,
        "rs" | "c" | "h" | "cpp" | "hpp" | "cs" | "java" | "js" | "mjs" | "tsx" | "jsx" | "py" | "go"
        | "rb" | "php" | "swift" | "kt" | "json" | "xml" | "yaml" | "yml" | "toml" | "html" | "css" | "scss"
        | "sql" | "pdb" | "lib" | "obj" | "o" | "a" | "rlib" | "rmeta" | "class" | "jar" | "pyc" | "node"
        | "wasm" | "ipynb" => Code,
        "tmp" | "temp" | "cache" | "log" | "etl" | "dmp" | "old" | "bak" | "crdownload" | "part" => Cache,
        "pf" | "evtx" | "cat" | "manifest" | "inf" | "pnf" | "etw" | "regtrans-ms" | "blf" => System,
        _ => Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories() {
        assert_eq!(category_for_ext("mp4"), Category::Video);
        assert_eq!(category_for_ext("zip"), Category::Archive);
        assert_eq!(category_for_ext("unknownext"), Category::Other);
        assert_eq!(Category::parse("videos"), Some(Category::Video));
        assert_eq!(Category::parse("Archive"), Some(Category::Archive));
        for c in Category::ALL {
            assert_eq!(Category::from_u8(c as u8), c);
        }
    }
}
