//! Product identity. Every user-visible product name, identifier and data
//! folder name derives from these constants so the product can be renamed in
//! one place (plus `src/config/branding.ts` and `src-tauri/tauri.conf.json`).

/// Human readable product name.
pub const APP_NAME: &str = "HD Cleaner";
/// Name of the command line executable (without extension).
pub const CLI_NAME: &str = "hdcleaner";
/// Folder name used under %LOCALAPPDATA% for databases, logs, snapshots.
pub const DATA_DIR_NAME: &str = "HDCleaner";
/// Magic header for the binary snapshot format.
pub const SNAPSHOT_MAGIC: &[u8; 8] = b"HDCSNAP1";
/// Magic of snapshots written before the product was renamed (still read).
pub const SNAPSHOT_MAGIC_LEGACY: &[u8; 8] = b"NXSNAP01";
/// Command line switch that puts an executable in "elevated helper" mode.
/// The helper accepts only a fixed, validated set of operations.
pub const ELEVATED_HELPER_SWITCH: &str = "--hdcleaner-elevated-helper";
