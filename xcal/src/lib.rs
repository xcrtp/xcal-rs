pub mod app;
pub mod scene;
pub mod render;
pub mod types;
pub use app::App;
pub mod errrors;
pub mod utils;
pub use errrors::Error;
pub type Result<T> = std::result::Result<T, Error>;
