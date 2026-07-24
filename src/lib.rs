//! gh-review-ui — a minimal, lazygit-style GitHub PR review TUI.
//!
//! The crate is split so that almost all logic is pure and unit-tested,
//! separate from the I/O layers:
//!
//! - [`diff`], [`markdown`], [`tree`], [`navigation`] — pure review logic
//! - [`models`] — data types + the central [`models::State`]
//! - [`theme`] — colors and diff/highlight styling
//! - [`gh`], [`api`] — `gh` CLI / GraphQL and the GitHub domain calls
//! - [`worker`] — the background thread running blocking `gh` jobs
//! - [`editor`] — external-editor integration
//! - [`controller`] — state transitions + job orchestration
//! - [`ui`], [`app`] — ratatui rendering and the event loop

pub mod api;
pub mod app;
pub mod controller;
pub mod diff;
pub mod editor;
pub mod gh;
pub mod markdown;
pub mod models;
pub mod navigation;
pub mod textbuffer;
pub mod theme;
pub mod tree;
pub mod ui;
pub mod worker;
