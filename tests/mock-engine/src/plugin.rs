//! A stand-in for the engine's TOPLOC plugin (m5-engine-toploc design D9): after each answer the
//! engine sends deterministic pseudo-activations over the plugin protocol, as the vLLM plugin
//! would: a prefill (in two segments when the prompt has more than one token) and one decode
//! segment per output token but the last, then the end marker.

use std::path::PathBuf;

use ac_market_proto::engine::{ENGINE_PROTOCOL_VERSION, EngineMsg, EngineReader};
use ac_toploc::{Bf16, Phase, top_k_candidates};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::Mutex;

/// Hidden size of the pseudo-model (values per token); at least the market's top-k of 128.
pub const HIDDEN: u32 = 256;

/// How the stand-in plugin behaves.
#[derive(Clone, Debug)]
pub struct PluginConfig {
    /// The provider's `--toploc-socket`.
    pub socket: PathBuf,
    /// Send only half of the decode segments (to test missing proofs).
    pub half_decode: bool,
}

/// A connection to the provider, opened on first use and reopened after failures.
#[derive(Debug)]
pub struct Plugin {
    config: PluginConfig,
    conn: Mutex<Option<(UnixStream, usize)>>,
}

/// The pseudo-activation `i` of token `t` of a request: finite bfloat16 values of both signs,
/// determined by the request ID.
#[must_use]
pub fn activation(request: &str, t: u64, i: u32) -> Bf16 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in request
        .bytes()
        .chain(t.to_le_bytes())
        .chain(i.to_le_bytes())
    {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    let bits = u16::try_from(h % 0x7f00).unwrap_or(0) | if h & (1 << 40) == 0 { 0 } else { 0x8000 };
    Bf16(bits)
}

/// The activations of tokens `from..to` of a request, flattened.
#[must_use]
pub fn activations(request: &str, from: u64, to: u64) -> Vec<Bf16> {
    (from..to)
        .flat_map(|t| (0..HIDDEN).map(move |i| activation(request, t, i)))
        .collect()
}

impl Plugin {
    /// A plugin that connects on first use.
    #[must_use]
    pub fn new(config: PluginConfig) -> Self {
        Self {
            config,
            conn: Mutex::new(None),
        }
    }

    /// Sends the segments of a finished request; failures are ignored (the provider then signs a
    /// receipt without proofs), as the real plugin never blocks inference.
    pub async fn send(&self, request: &str, prompt: u64, completion: u64) {
        let mut conn = self.conn.lock().await;
        if conn.is_none() {
            *conn = self.connect().await;
        }
        let Some((stream, topk)) = conn.as_mut() else {
            return;
        };
        let topk = *topk;
        let decode = completion.saturating_sub(1);
        let decode = if self.config.half_decode {
            decode / 2
        } else {
            decode
        };
        let mut msgs = Vec::new();
        let split = if prompt > 1 { prompt / 2 } else { prompt };
        for (from, to) in [(0, split), (split, prompt)] {
            if from < to {
                msgs.push(segment(
                    request,
                    Phase::Prefill,
                    &activations(request, from, to),
                    topk,
                ));
            }
        }
        for t in prompt..prompt.saturating_add(decode) {
            msgs.push(segment(
                request,
                Phase::Decode,
                &activations(request, t, t + 1),
                topk,
            ));
        }
        msgs.push(EngineMsg::Finish {
            request: request.to_string(),
        });
        for m in msgs {
            let Ok(frame) = m.to_frame() else { return };
            if stream.write_all(&frame).await.is_err() {
                *conn = None;
                return;
            }
        }
    }

    async fn connect(&self) -> Option<(UnixStream, usize)> {
        let mut stream = UnixStream::connect(&self.config.socket).await.ok()?;
        let hello = EngineMsg::Hello {
            version: ENGINE_PROTOCOL_VERSION,
            hidden_size: HIDDEN,
        };
        stream.write_all(&hello.to_frame().ok()?).await.ok()?;
        let mut reader = EngineReader::new();
        let mut buf = [0u8; 64];
        loop {
            let n = stream.read(&mut buf).await.ok()?;
            if n == 0 {
                return None;
            }
            reader.push(buf.get(..n)?);
            if let Some(msg) = reader.next_msg().ok()? {
                let EngineMsg::Welcome { version, topk } = msg else {
                    return None;
                };
                if version != ENGINE_PROTOCOL_VERSION {
                    return None;
                }
                return Some((stream, usize::from(topk)));
            }
        }
    }
}

fn segment(request: &str, phase: Phase, values: &[Bf16], topk: usize) -> EngineMsg {
    EngineMsg::Segment {
        request: request.to_string(),
        phase,
        len: u32::try_from(values.len()).unwrap_or(u32::MAX),
        candidates: top_k_candidates(values, topk),
    }
}
