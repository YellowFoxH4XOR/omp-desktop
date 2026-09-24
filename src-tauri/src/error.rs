use std::fmt;

/// Human-readable application error. The `Display` text is what the UI shows,
/// so messages must stay free of secrets, tokens, and raw protocol dumps.
#[derive(Debug)]
pub struct AppError(pub String);

impl AppError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AppError {}

impl From<String> for AppError {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for AppError {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        Self(format!("{e}"))
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        Self(format!("Local store error: {e}"))
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        Self(format!("Invalid data: {e}"))
    }
}

impl serde::Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

pub type AppResult<T> = Result<T, AppError>;

/// Result type returned to the frontend: `Err(String)` maps to a rejected
/// invoke promise with a human-readable message.
pub type CmdResult<T> = Result<T, String>;

pub fn cmd_err(e: AppError) -> String {
    e.0
}
