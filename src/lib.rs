//! herdr-code-board: a kanban queue for agentic prompts inside Herdr.
//!
//! The crate is split so the automation logic can be tested without a running
//! Herdr server: [`engine::reducer`] is a pure function over events, and
//! [`herdr::HerdrApi`] is a trait the dispatcher talks to.

pub mod agents;
pub mod app;
pub mod cli;
pub mod config;
pub mod engine;
pub mod git;
pub mod herdr;
pub mod integrate;
pub mod model;
pub mod overlay;
pub mod store;
pub mod tui;
