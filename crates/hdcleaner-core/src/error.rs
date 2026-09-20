//! Error type shared across the core. Every error carries enough structure for
//! the UI to explain *what* failed, *where* and *what the user can do*.

use serde::Serialize;
use std::path::Path;

pub type Result<T, E = AppError> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("access denied: {path}")]
    AccessDenied { path: String },

    #[error("not found: {path}")]
    NotFound { path: String },

    #[error("operation cancelled")]
    Cancelled,

    #[error("{context}: {source}")]
    Io {
        context: String,
        path: Option<String>,
        #[source]
        source: std::io::Error,
    },

    #[error("Win32 error {code} while {context}")]
    Win32 { code: u32, context: String, path: Option<String> },

    #[error("protected location: {path} ({reason})")]
    Protected { path: String, reason: String },

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("elevation required: {0}")]
    ElevationRequired(String),

    #[error("not supported: {0}")]
    NotSupported(String),

    #[error("item changed since it was reviewed: {path}")]
    ChangedSinceReview { path: String },

    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("corrupt data: {0}")]
    Corrupt(String),

    #[error("elevated helper failed: {0}")]
    Helper(String),
}

impl AppError {
    pub fn io(context: impl Into<String>, path: Option<&Path>, source: std::io::Error) -> Self {
        let path_s = path.map(|p| p.display().to_string());
        match source.kind() {
            std::io::ErrorKind::PermissionDenied => AppError::AccessDenied {
                path: path_s.unwrap_or_default(),
            },
            std::io::ErrorKind::NotFound => AppError::NotFound { path: path_s.unwrap_or_default() },
            _ => AppError::Io { context: context.into(), path: path_s, source },
        }
    }

    /// Build from `GetLastError()`.
    pub fn last_win32(context: impl Into<String>, path: Option<&str>) -> Self {
        let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        Self::from_win32(code, context, path)
    }

    pub fn from_win32(code: u32, context: impl Into<String>, path: Option<&str>) -> Self {
        use windows_sys::Win32::Foundation::*;
        let path_s = path.map(str::to_string);
        match code {
            ERROR_ACCESS_DENIED | ERROR_PRIVILEGE_NOT_HELD => {
                AppError::AccessDenied { path: path_s.unwrap_or_default() }
            }
            ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND | ERROR_INVALID_DRIVE => {
                AppError::NotFound { path: path_s.unwrap_or_default() }
            }
            _ => AppError::Win32 { code, context: context.into(), path: path_s },
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            AppError::AccessDenied { .. } => "accessDenied",
            AppError::NotFound { .. } => "notFound",
            AppError::Cancelled => "cancelled",
            AppError::Io { .. } => "io",
            AppError::Win32 { .. } => "win32",
            AppError::Protected { .. } => "protected",
            AppError::InvalidInput(_) => "invalidInput",
            AppError::ElevationRequired(_) => "elevationRequired",
            AppError::NotSupported(_) => "notSupported",
            AppError::ChangedSinceReview { .. } => "changedSinceReview",
            AppError::Database(_) => "database",
            AppError::Corrupt(_) => "corrupt",
            AppError::Helper(_) => "helper",
        }
    }

    pub fn path(&self) -> Option<&str> {
        match self {
            AppError::AccessDenied { path }
            | AppError::NotFound { path }
            | AppError::Protected { path, .. }
            | AppError::ChangedSinceReview { path } => Some(path),
            AppError::Io { path, .. } | AppError::Win32 { path, .. } => path.as_deref(),
            _ => None,
        }
    }

    /// Suggested remediation key (translated by the UI).
    pub fn suggestion(&self) -> Option<&'static str> {
        match self {
            AppError::AccessDenied { .. } | AppError::ElevationRequired(_) => Some("runElevated"),
            AppError::NotFound { .. } => Some("rescan"),
            AppError::ChangedSinceReview { .. } => Some("reviewAgain"),
            AppError::Protected { .. } => Some("protectedLocation"),
            _ => None,
        }
    }

    pub fn to_payload(&self) -> ErrorPayload {
        ErrorPayload {
            kind: self.kind(),
            message: self.to_string(),
            path: self.path().map(str::to_string),
            suggestion: self.suggestion(),
            code: match self {
                AppError::Win32 { code, .. } => Some(*code),
                AppError::Io { source, .. } => source.raw_os_error().map(|c| c as u32),
                _ => None,
            },
        }
    }
}

/// Serializable form of an error, sent to the UI and written to logs/history.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ErrorPayload {
    pub kind: &'static str,
    pub message: String,
    pub path: Option<String>,
    pub suggestion: Option<&'static str>,
    pub code: Option<u32>,
}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        self.to_payload().serialize(s)
    }
}
