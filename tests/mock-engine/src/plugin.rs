//! A stand-in for the engine's TOPLOC plugin (m5-engine-toploc design D9, m6-toploc-verify
//! design D6). Pseudo-activations depend on the model seed, the position and the tokens up to
//! and including it, so that a prover that generates a sequence and a verifier that prefills it
//! get the same values bit for bit, and another seed plays another model.
//!
//! In the prove mode, after each answer the engine sends a prefill (in two segments when the
//! prompt has more than one token) and one decode segment per output token but the last, then
//! the end marker, as the vLLM plugin does. In the verify mode it sends one segment per prefilled
//! token row of a completion, then the end marker.

use std::path::PathBuf;

use ac_market_proto::engine::{ENGINE_PROTOCOL_VERSION, EngineMode, EngineMsg, EngineReader};
use ac_toploc::{Bf16, Phase, top_k_candidates};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::Mutex;

/// Hidden size of the pseudo-model (values per token); at least the market's top-k of 128.
pub const HIDDEN: u32 = 256;

/// How the stand-in plugin behaves.
#[derive(Clone, Debug)]
pub struct PluginConfig {
    /// The receiver's socket (a provider's `--toploc-socket`, an auditor's `--socket`).
    pub socket: PathBuf,
    /// Send only half of the decode segments (to test missing proofs; prove mode).
    pub half_decode: bool,
    /// Prove (provider) or verify (auditor).
    pub mode: EngineMode,
}

impl PluginConfig {
    /// A prove-mode plugin on `socket`.
    #[must_use]
    pub const fn prove(socket: PathBuf) -> Self {
        Self {
            socket,
            half_decode: false,
            mode: EngineMode::Prove,
        }
    }
}

/// A connection to the receiver, opened on first use and reopened after failures.
#[derive(Debug)]
pub struct Plugin {
    config: PluginConfig,
    seed: u64,
    conn: Mutex<Option<(UnixStream, usize)>>,
}

fn fnv(h: u64, bytes: impl IntoIterator<Item = u8>) -> u64 {
    bytes
        .into_iter()
        .fold(h, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// Rolling hashes of the prefixes of `tokens`: element `t` covers `tokens[..=t]`.
#[must_use]
pub fn prefix_hashes(seed: u64, tokens: &[u32]) -> Vec<u64> {
    let mut h = fnv(0xcbf2_9ce4_8422_2325, seed.to_le_bytes());
    tokens
        .iter()
        .map(|t| {
            h = fnv(h, t.to_le_bytes());
            h
        })
        .collect()
}

/// The pseudo-activation `i` of the token whose prefix hashes to `prefix`: finite bfloat16
/// values of both signs.
#[must_use]
pub fn activation(prefix: u64, i: u32) -> Bf16 {
    let h = fnv(prefix, i.to_le_bytes());
    let h = h ^ (h >> 29);
    let bits = u16::try_from(h % 0x7f00).unwrap_or(0) | if h & (1 << 40) == 0 { 0 } else { 0x8000 };
    Bf16(bits)
}

/// The activations of positions `from..to` of a sequence, flattened.
#[must_use]
pub fn activations(prefixes: &[u64], from: usize, to: usize) -> Vec<Bf16> {
    prefixes
        .get(from..to)
        .unwrap_or_default()
        .iter()
        .flat_map(|p| (0..HIDDEN).map(move |i| activation(*p, i)))
        .collect()
}

impl Plugin {
    /// A plugin of the model `seed` that connects on first use.
    #[must_use]
    pub fn new(config: PluginConfig, seed: u64) -> Self {
        Self {
            config,
            seed,
            conn: Mutex::new(None),
        }
    }

    /// The plugin's mode.
    #[must_use]
    pub const fn mode(&self) -> EngineMode {
        self.config.mode
    }

    /// Prove mode: sends the segments of a finished chat request whose `prompt` tokens were
    /// followed by the `output` tokens; failures are ignored (the provider then signs a receipt
    /// without proofs), as the real plugin never blocks inference.
    pub async fn send(&self, request: &str, prompt: &[u32], output: &[u32]) {
        let mut tokens = prompt.to_vec();
        tokens.extend_from_slice(output);
        let prefixes = prefix_hashes(self.seed, &tokens);
        let p = prompt.len();
        let decode = output.len().saturating_sub(1);
        let decode = if self.config.half_decode {
            decode / 2
        } else {
            decode
        };
        let split = if p > 1 { p / 2 } else { p };
        let mut segs = Vec::new();
        for (from, to) in [(0, split), (split, p)] {
            if from < to {
                segs.push((Phase::Prefill, activations(&prefixes, from, to)));
            }
        }
        for t in p..p.saturating_add(decode) {
            segs.push((Phase::Decode, activations(&prefixes, t, t + 1)));
        }
        self.write(request, segs).await;
    }

    /// Verify mode: sends one segment per row of a prefilled sequence.
    pub async fn send_rows(&self, request: &str, tokens: &[u32]) {
        let prefixes = prefix_hashes(self.seed, tokens);
        let segs = (0..tokens.len())
            .map(|t| (Phase::Prefill, activations(&prefixes, t, t + 1)))
            .collect();
        self.write(request, segs).await;
    }

    async fn write(&self, request: &str, segs: Vec<(Phase, Vec<Bf16>)>) {
        let mut conn = self.conn.lock().await;
        if conn.is_none() {
            *conn = self.connect().await;
        }
        let Some((stream, topk)) = conn.as_mut() else {
            return;
        };
        let topk = *topk;
        let mut msgs: Vec<EngineMsg> = segs
            .iter()
            .map(|(phase, values)| EngineMsg::Segment {
                request: request.to_string(),
                phase: *phase,
                len: u32::try_from(values.len()).unwrap_or(u32::MAX),
                candidates: top_k_candidates(values, topk),
            })
            .collect();
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
            mode: self.config.mode,
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
                // Another version, or a receiver refusing this mode (top-k 0).
                if version != ENGINE_PROTOCOL_VERSION || topk == 0 {
                    return None;
                }
                return Some((stream, usize::from(topk)));
            }
        }
    }
}
