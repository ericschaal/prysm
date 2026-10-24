//! Application setup for device/file input and desktop/WLED output.

mod app;
mod config;
mod desktop;
mod input;
mod stream;
pub mod video;
mod wled;

pub use app::run_from_config;
pub use desktop::run;
