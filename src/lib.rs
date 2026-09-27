//! alertsift: decide which alerts deserve to wake a human.

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
