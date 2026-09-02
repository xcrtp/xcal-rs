//! errors 模块集成测试

use xcal::errrors::Error;
use xcal::Result;

#[test]
fn initialization_error_display() {
    let e = Error::InitializationError("test".to_string());
    assert_eq!(e.to_string(), "Initialization error: test");
}

#[test]
fn initialization_error_with_empty_msg() {
    let e = Error::InitializationError(String::new());
    assert_eq!(e.to_string(), "Initialization error: ");
}

#[test]
fn hecs_error_display() {
    let e = Error::HecsError("entity missing".to_string());
    assert_eq!(e.to_string(), "hecs error entity missing");
}

#[test]
fn unknown_error_display() {
    let e = Error::UnknownError;
    assert_eq!(e.to_string(), "Unknown error");
}

#[test]
fn default_is_unknown_error() {
    let e: Error = Default::default();
    assert!(matches!(e, Error::UnknownError));
}

#[test]
fn default_display_matches_unknown() {
    let e: Error = Default::default();
    assert_eq!(e.to_string(), "Unknown error");
}

#[test]
fn error_variants_are_distinct() {
    let a = Error::InitializationError("x".to_string());
    let b = Error::HecsError("x".to_string());
    let c = Error::UnknownError;
    // 确保不同的变体有不同的 Display 输出
    assert_ne!(a.to_string(), b.to_string());
    assert_ne!(b.to_string(), c.to_string());
    assert_ne!(a.to_string(), c.to_string());
}

#[test]
fn result_type_alias_works() {
    let ok: Result<i32> = Ok(42);
    assert!(ok.is_ok());
    assert_eq!(ok.unwrap(), 42);
}

#[test]
fn result_with_error() {
    let err: Result<i32> = Err(Error::UnknownError);
    assert!(err.is_err());
}

#[test]
fn error_debug_format() {
    let e = Error::InitializationError("test".to_string());
    let debug_str = format!("{:?}", e);
    assert!(debug_str.contains("InitializationError"));
    assert!(debug_str.contains("test"));
}
