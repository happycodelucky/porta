use thiserror::Error;

#[derive(Debug, Error)]
#[error("{message}")]
pub struct PortaError {
    pub code: &'static str,
    pub message: String,
    pub exit_code: u8,
}

impl PortaError {
    #[must_use]
    pub fn new(code: &'static str, message: impl Into<String>, exit_code: u8) -> Self {
        Self {
            code,
            message: message.into(),
            exit_code,
        }
    }

    #[must_use]
    pub fn invalid(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(code, message, 2)
    }

    #[must_use]
    pub fn missing(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(code, message, 1)
    }

    #[must_use]
    pub fn infrastructure(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(code, message, 3)
    }
}

pub type Result<T> = std::result::Result<T, PortaError>;
