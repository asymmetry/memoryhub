//! # MemoryHub
//!
//! A centralized memory service for AI agents.
//!
//! Personal AI agents (Claude Code, etc.) each build up knowledge as local Markdown
//! files, in isolation. MemoryHub pools those per-user memories into one shared,
//! searchable knowledge base: it ingests raw Markdown from each agent, embeds it for
//! semantic search, and runs a synthesizer that produces per-user and cross-user
//! summaries.
//!
//! ## What it does
//!
//! - **Collects memories** from many users and agents over a small JSON API.
//! - **Searchable instantly** — every memory is indexed the moment it's written, so
//!   it's immediately available. No batch or reprocess step.
//! - **Finds the right memory** — search handles both meaning-based and exact
//!   keyword queries, so results hold up whether you describe an idea or quote a term.
//! - **Synthesizes** — generates per-user and cross-user summaries from the pooled
//!   memories as they come in.
//! - **Human-readable storage** — memories stay as plain Markdown files you can read,
//!   edit, and version directly.
//! - **Bring your own models** — chat and embeddings are configured separately, so you
//!   can mix providers (and swap in a stub for tests).
//!
//! It's a single self-contained service.

#![allow(clippy::uninlined_format_args)]

pub mod error;

pub mod http;
pub mod llm;
pub mod memory;

pub mod config;
pub use config::Config;

mod supervisor;
pub use supervisor::MemoryHub;
