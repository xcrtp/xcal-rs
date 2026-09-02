// errors.rs (从 errrors.rs 修正拼写)

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initialization_error_display() {
        let e = Error::InitializationError("test".to_string());
        assert_eq!(e.to_string(), "Initialization error: test");
    }

    #[test]
    fn test_hecs_error_display() {
        let e = Error::HecsError("entity missing".to_string());
        assert_eq!(e.to_string(), "hecs error entity missing");
    }

    #[test]
    fn test_unknown_error_display() {
        let e = Error::UnknownError;
        assert_eq!(e.to_string(), "Unknown error");
    }

    #[test]
    fn test_default_is_unknown() {
        let e: Error = Default::default();
        assert!(matches!(e, Error::UnknownError));
    }

    #[test]
    fn test_error_variants_distinct_display() {
        let a = Error::InitializationError("x".to_string());
        let b = Error::HecsError("x".to_string());
        let c = Error::UnknownError;
        assert_ne!(a.to_string(), b.to_string());
        assert_ne!(b.to_string(), c.to_string());
        assert_ne!(a.to_string(), c.to_string());
    }
}
