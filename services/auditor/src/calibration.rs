//! Re-check cases built from an engine's candidates, for the calibration of the thresholds and
//! for tests (`ac-auditor calibration-case`; design D7 of m6-toploc-verify). Not part of an
//! audit: the receipt is signed by a key generated for the run, which nothing on chain knows.

use ac_crypto::OsRng;
use ac_crypto::SigAlg;
use ac_crypto::sig::{SecretSeed, SigningKey};
use ac_market_proto::toploc::{MARKET_PARAMS, ToplocProofs, fit_segments};
use ac_primitives::market::receipt::RECEIPT_CONTEXT;
use ac_primitives::market::work::JobKind;
use ac_primitives::market::{MicroUsd, ReceiptBody, SignedReceipt};
use ac_toploc::{Bf16, Candidate, Phase, Segment, build_proofs_from_candidates};
use anyhow::{Context, Result};
use parity_scale_codec::Encode;
use rand_core::TryRng;
use serde::Deserialize;
use serde_json::Value;
use sp_core::H256;
use sp_runtime::AccountId32;

use crate::case::{CaseUsage, RecheckCase};

/// One engine segment: `phase` (`prefill` or `decode`), its length and `[index, bf16 bits]`
/// candidates.
#[derive(Deserialize)]
pub struct InputSegment {
    /// `prefill` or `decode`.
    pub phase: String,
    /// Values in the segment.
    pub len: u32,
    /// `[index, bits]` pairs.
    pub candidates: Vec<(u32, u16)>,
}

/// An inference as a prover's engine saw it.
#[derive(Deserialize)]
pub struct CalibrationInput {
    /// On-chain model ID (`0x` + 64 hex digits).
    pub model: String,
    /// The model's name on the re-check engine.
    pub engine_model: String,
    /// The request's messages.
    pub messages: Value,
    /// The answer's text.
    pub output: String,
    /// The answer's finish reason.
    pub finish_reason: String,
    /// The answer's token counts.
    pub usage: CaseUsage,
    /// The plugin's segments; none for an inference without proofs.
    pub segments: Option<Vec<InputSegment>>,
    /// The prompt tokens the engine computed, when the provider reports another count in
    /// `usage` (the calibration's hidden-prompt cheat): its segments are fitted to what the
    /// engine computed, as a cheating provider's own code would.
    #[serde(default)]
    pub engine_prompt_tokens: Option<u32>,
}

/// The re-check case of `input`: proofs built from its segments (none if there are none or
/// they do not make proofs), committed to by a receipt that a fresh key signs as both parties.
///
/// # Errors
///
/// Unknown phases, segments that do not fit the usage (as a provider would give no proof), a bad
/// model ID, randomness or signing failures.
pub fn case_from(input: CalibrationInput) -> Result<RecheckCase> {
    let segments = input
        .segments
        .map(|segs| {
            segs.into_iter()
                .map(|s| {
                    let phase = match s.phase.as_str() {
                        "prefill" => Phase::Prefill,
                        "decode" => Phase::Decode,
                        other => anyhow::bail!("unknown phase {other}"),
                    };
                    Ok(Segment {
                        phase,
                        len: s.len,
                        candidates: s
                            .candidates
                            .into_iter()
                            .map(|(index, bits)| Candidate {
                                index,
                                value: Bf16(bits),
                            })
                            .collect(),
                    })
                })
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?;
    // As the provider does (m6-toploc-async-stop design D1): segments that do not fit the usage
    // make no case, and an end token fed back (one decode segment more) is dropped.
    let segments = segments
        .map(|s| {
            let prompt = input
                .engine_prompt_tokens
                .unwrap_or(input.usage.prompt_tokens);
            fit(s, prompt, input.usage.completion_tokens)
        })
        .transpose()?;
    let proofs = segments
        .and_then(|s| build_proofs_from_candidates(&s, &MARKET_PARAMS).ok())
        .map(|p| ToplocProofs::new(&MARKET_PARAMS, &p));
    let toploc_commit = match &proofs {
        Some(p) => p.commitment().map_err(|e| anyhow::anyhow!("{e}"))?,
        None => [0; 32],
    };

    let mut rng = OsRng::new()?;
    let mut request_id = [0u8; 32];
    rng.try_fill_bytes(&mut request_id)
        .map_err(|_| anyhow::anyhow!("randomness"))?;
    let mut seed = [0u8; 32];
    rng.try_fill_bytes(&mut seed)
        .map_err(|_| anyhow::anyhow!("randomness"))?;
    let key = SigningKey::from_seed(SigAlg::MlDsa44, &SecretSeed::new(seed))?;
    let body = ReceiptBody {
        genesis: H256([0; 32]),
        gateway: AccountId32::new([0; 32]),
        provider: AccountId32::new([0; 32]),
        kind: JobKind::Inference,
        model: ac_wallet::market::parse_model_id(&input.model)?,
        request_id,
        in_tokens: input.usage.prompt_tokens,
        out_tokens: input.usage.completion_tokens,
        fee: MicroUsd(0),
        toploc_commit,
        ttft_ms: 0,
        total_ms: 0,
    };
    let sig = key.sign(&body.payload()?, RECEIPT_CONTEXT, &mut rng)?;
    let public = key.public_key()?;
    let receipt = SignedReceipt {
        body,
        provider_key: public.clone(),
        provider_sig: sig.clone(),
        gateway_key: public,
        gateway_sig: sig,
    };
    Ok(RecheckCase {
        model: input.model,
        engine_model: input.engine_model,
        messages: input.messages,
        output: input.output,
        finish_reason: input.finish_reason,
        usage: input.usage,
        receipt: hex::encode(receipt.encode()),
        toploc: proofs.map(|p| hex::encode(p.encode())),
    })
}

/// The segments fitted to the usage by the provider's rule, the hidden size read from the
/// prefill (its values are the prompt tokens times the hidden size).
fn fit(segments: Vec<Segment>, prompt_tokens: u32, completion_tokens: u32) -> Result<Vec<Segment>> {
    let prefill: u64 = segments
        .iter()
        .take_while(|s| s.phase == Phase::Prefill)
        .map(|s| u64::from(s.len))
        .sum();
    let hidden = prefill
        .checked_div(u64::from(prompt_tokens))
        .filter(|h| h.checked_mul(u64::from(prompt_tokens)) == Some(prefill))
        .and_then(|h| u32::try_from(h).ok())
        .context("the prefill segments do not cover the prompt")?;
    fit_segments(segments, prompt_tokens, completion_tokens, hidden)
        .map_err(|e| anyhow::anyhow!("the segments do not fit the usage: {e}"))
}

/// [`case_from`] on JSON text.
///
/// # Errors
///
/// Malformed JSON and the errors of [`case_from`].
pub fn case_from_json(text: &str) -> Result<RecheckCase> {
    case_from(serde_json::from_str(text).context("calibration input")?)
}

#[cfg(test)]
mod tests {
    // Test code: unwrap and indexing make failures point at the case.
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    const HIDDEN: u32 = 256;

    fn segs(prompt: u32, decode: usize) -> Vec<Segment> {
        let mut out = vec![Segment {
            phase: Phase::Prefill,
            len: prompt * HIDDEN,
            candidates: Vec::new(),
        }];
        out.extend((0..decode).map(|_| Segment {
            phase: Phase::Decode,
            len: HIDDEN,
            candidates: Vec::new(),
        }));
        out
    }

    // m6-toploc-async-stop 3.1: as the provider: n − 1 kept, one more dropped, others refused.
    #[test]
    fn segments_are_fitted_as_the_provider_does() {
        assert_eq!(fit(segs(4, 9), 4, 10).unwrap().len(), 10);
        let fed = fit(segs(4, 10), 4, 10).unwrap();
        assert_eq!(fed.len(), 10);
        assert_eq!(fed, segs(4, 9));
        let err = fit(segs(4, 11), 4, 10).unwrap_err().to_string();
        assert!(
            err.contains("11 decode segments where the output calls for 9"),
            "{err}"
        );
        assert!(fit(segs(4, 9), 5, 10).is_err());
    }
}
