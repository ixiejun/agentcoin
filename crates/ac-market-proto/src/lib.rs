#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

pub mod announce;
pub mod engine;
pub mod frame;
pub mod msg;
pub mod openai;
pub mod route;
pub mod toploc;

use core::fmt;

pub use announce::GatewayKey;
pub use msg::{
    ErrorCode, GatewayMsg, PROTOCOL_VERSION, Payment, ProviderMsg, ProviderReq, Usage, UserMsg,
};
pub use openai::ChatRequest;
pub use route::{Candidate, PriceThenLatency, RouteScore, max_fee};
pub use toploc::{MARKET_PARAMS, ToplocError, ToplocProofs};

/// Protocol errors. None of them carries request content.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// A frame exceeds the maximum size.
    FrameTooLarge,
    /// A message could not be decoded.
    Decode,
    /// A message uses an unknown protocol version.
    UnsupportedVersion(u8),
    /// A request is not valid JSON of the expected shape.
    BadJson,
    /// A required request field is missing or has the wrong type.
    MissingField(&'static str),
    /// A gateway key announcement is malformed or does not verify.
    InvalidAnnouncement,
    /// A cryptographic operation failed.
    Crypto(ac_crypto::Error),
}

impl From<ac_crypto::Error> for Error {
    fn from(e: ac_crypto::Error) -> Self {
        Self::Crypto(e)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FrameTooLarge => f.write_str("frame too large"),
            Self::Decode => f.write_str("malformed message"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported protocol version {v}"),
            Self::BadJson => f.write_str("request is not a valid JSON object"),
            Self::MissingField(name) => write!(f, "missing or invalid field `{name}`"),
            Self::InvalidAnnouncement => f.write_str("invalid gateway key announcement"),
            Self::Crypto(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}
