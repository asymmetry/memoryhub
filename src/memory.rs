//! Memory actor sub-system for ClawChorus.
//!
//! The memory manager owns Storage, Index, and Synthesizer child actors.
//! See the actor handler modules in this directory.

pub mod chunking;
pub mod error;
pub mod index;
mod path;

pub mod storage;
pub mod synthesizer;

mod manager;
pub use manager::MemoryManager;

mod config;
pub use config::MemoryConfig;

pub mod message;

mod file_op;
pub use file_op::FileOp;

mod search_op;
pub use search_op::SearchOp;
