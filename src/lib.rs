//! keenwake: decide which alerts deserve to wake a human.

pub mod backend;
pub mod config;
pub mod decide;
pub mod digest;
pub mod history;
pub mod mapping;
pub mod metrics;
pub mod output;
pub mod pipeline;
pub mod redact;
pub mod report;
pub mod server;
pub mod state;
pub mod store;
pub mod worker;

/// A stored or configured name that matches no variant of the enum it should parse into.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown value {0:?}")]
pub struct UnknownValue(pub String);
