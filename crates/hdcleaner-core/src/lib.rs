//! HD Cleaner core library.
//!
//! Layering: UI (Tauri commands / CLI) → application services (this crate's
//! public functions) → system services (scan, registry, fsops...) → Win32.
//! No destructive logic lives outside this crate.

pub mod appanalysis;
pub mod appsize;
pub mod appx;
pub mod backups;
pub mod bootperf;
pub mod branding;
pub mod category;
pub mod cleaner;
pub mod clipboard;
pub mod correlate;
pub mod db;
pub mod diff;
pub mod disk;
pub mod duplicates;
pub mod elevation;
pub mod error;
pub mod export;
pub mod forced;
pub mod extensions;
pub mod fileinfo;
pub mod format;
pub mod fsops;
pub mod icons;
pub mod leftovers;
pub mod monitor;
pub mod processes;
pub mod programs;
pub mod protection;
pub mod regops;
pub mod registry;
pub mod report;
pub mod scan;
pub mod search;
pub mod shellmenu;
pub mod shortcuts;
pub mod startup;
pub mod smartstorage;
pub mod stats;
pub mod sysitems;
pub mod system;
pub mod target;
pub mod timeline;
pub mod tools;
pub mod uninstall;
pub mod treemap;
pub mod util;
pub mod wipe;

pub use error::{ErrorPayload, AppError, Result};