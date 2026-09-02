pub mod app;
pub mod scene;
pub mod render;
pub mod types;
pub use app::App;
pub mod errrors;
pub mod utils;
pub use errrors::Error;
pub type Result<T> = std::result::Result<T, Error>;

// 在 crate 自身内部为派生宏生成的 `xcal::` 路径提供别名。
// `#[derive(xcal_macro::Transform2D)]` 生成的代码引用 `xcal::render::Transform2D` 等，
// 在 crate 内部默认无法直接用 `xcal::` 访问自身，需要此别名。
extern crate self as xcal;
