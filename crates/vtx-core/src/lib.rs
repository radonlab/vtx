//! vtx-core: core model layer.
//!
//! This crate is GUI-agnostic and is intended to be reused by
//! `vtx-ui` and `vtx-app`.

pub mod cell;
pub mod line;
pub mod session;

/// Application configuration.
#[derive(Debug, Clone)]
pub struct AppConfig {
    /// Window title.
    pub title: String,
    /// Initial window size as `[width, height]`.
    pub window_size: [f32; 2],
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            title: "VTX".to_owned(),
            window_size: [1280.0, 800.0],
        }
    }
}
