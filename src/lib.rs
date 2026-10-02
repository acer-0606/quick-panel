//! Shared data, persistence and search for the native Windows panel.
pub mod clipboard_formats;
pub mod engine;
pub mod model;
#[cfg(windows)]
pub mod platform;
pub mod repository;
pub mod search;
pub mod ui_state;
