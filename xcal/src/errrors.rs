#[derive(Debug, thiserror::Error, smart_default::SmartDefault)]
pub enum Error {
    #[error("Initialization error: {0}")]
    InitializationError(String),

    #[error("hecs error {0}")]
    HecsError(String),

    #[error("Unknown error")]
    #[default]
    UnknownError,
}
