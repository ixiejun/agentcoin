#![doc = include_str!("../README.md")]

//! AgentCoin public job worker (MVP plan §5.6; spec `market/public-worker`).
//!
//! A worker runs the units the chain assigns it: it checks the data against the manifest's
//! hashes, executes the unit with the rules of its kind (evaluation by log likelihood, embedding
//! fingerprints, data cleaning), commits to the summary, reveals it after the commit deadline and
//! uploads the full result to the publisher. Data and results are never logged.

pub mod agent;
pub mod clean;
pub mod engine;
pub mod exec;
pub mod fingerprint;
pub mod logging;
pub mod manifest;

pub use engine::EngineClient;
pub use exec::Output;
