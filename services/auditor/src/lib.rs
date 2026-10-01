#![doc = include_str!("../README.md")]

//! AgentCoin auditor agent (MVP plan §5.5, §7; spec `market/auditor-agent`).
//!
//! An auditor re-checks inferences it requested itself: it re-creates the tokens with its own
//! engine (vLLM with the TOPLOC plugin in verify mode), prefills "prompt + output", collects one
//! candidate segment per token row on a local socket, compares the rebuilt chunks with the
//! provider's proofs and judges them by the market's thresholds. Messages, outputs, tokens and
//! candidates are never logged.

pub mod calibration;
pub mod case;
pub mod engine;
pub mod logging;
pub mod recheck;
pub mod socket;

pub use case::{FailReason, Inconclusive, Outcome, RecheckCase, Report};
pub use engine::EngineClient;
pub use recheck::Verifier;
pub use socket::Rows;
