//! Shared data, persistence and search for the native Windows panel.
pub mod ai;
pub mod ai_jobs;
pub mod claude;
pub mod clipboard_formats;
pub mod engine;
pub mod hotkeys;
pub mod model;
pub mod palette;
#[cfg(windows)]
pub mod platform;
pub mod repository;
pub mod search;
pub mod ui_state;
