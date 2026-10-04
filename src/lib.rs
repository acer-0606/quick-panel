//! Shared data, persistence and search for the native Windows panel.
pub mod ai;
pub mod ai_jobs;
pub mod claude;
pub mod clipboard_formats;
pub mod connection_test;
pub mod embedding;
pub mod embedding_jobs;
pub mod engine;
pub mod hotkeys;
pub mod model;
pub mod palette;
#[cfg(windows)]
pub mod platform;
pub mod query_feedback;
pub mod repository;
pub mod search;
pub mod semantic;
mod semantic_store;
pub mod semantic_ui;
pub mod ui_state;
