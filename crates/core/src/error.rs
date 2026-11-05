use serde::{Deserialize, Serialize};

pub type Result<T> = std::result::Result<T, Error>;

/// The error code remains stable across library and CLI interfaces.
#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[error("{code}: {message}")]
pub struct Error {
    pub code: String,
    pub message: String,
    #[source]
    #[serde(skip)]
    source: Option<std::sync::Arc<dyn std::error::Error + Send + Sync>>,
}

impl Error {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            source: None,
        }
    }

    pub fn with_source(mut self, source: impl std::error::Error + Send + Sync + 'static) -> Self {
        self.source = Some(std::sync::Arc::new(source));
        self
    }

    pub fn context<E>(code: &'static str, message: &'static str) -> impl FnOnce(E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        move |source| Self::new(code, message).with_source(source)
    }

    pub fn exit_code(&self) -> i32 {
        match self.code.as_str() {
            "cancelled" => 130,
            "unsupported_behavior" | "mandatory_context_overflow" | "context_exceeded" => 3,
            "session_conflict"
            | "configuration_conflict"
            | "lease_lost"
            | "storage"
            | "not_found"
            | "session_configuration_changed"
            | "pinned_composition_unavailable"
            | "composition_changed"
            | "future_schema" => 4,
            code if code.starts_with("invalid_")
                || code.starts_with("unsupported_")
                || matches!(
                    code,
                    "configuration"
                        | "session_configuration_required"
                        | "session_configuration_limit"
                ) =>
            {
                2
            }
            _ => 1,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::new("io", "The input or output operation failed.").with_source(value)
    }
}

#[cfg(feature = "sqlite")]
impl From<rusqlite::Error> for Error {
    fn from(value: rusqlite::Error) -> Self {
        Self::new("storage", "The database operation failed.").with_source(value)
    }
}

impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Self::new("invalid_json", "The JSON input is invalid.").with_source(value)
    }
}
