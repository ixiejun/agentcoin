//! Maximum possible fee of a request and provider ranking (design D4, D5).

use ac_primitives::market::receipt::fee_for;
use ac_primitives::market::{MicroUsd, PricePerMTok};
use sp_runtime::AccountId32;

/// The largest fee a request can cost at `price`: `input_bound` prompt tokens and
/// `max_output` generated tokens, rounded up like a receipt. `None` if the bound does not fit a
/// receipt (the request must be refused).
#[must_use]
pub fn max_fee(price: &PricePerMTok, input_bound: u64, max_output: u32) -> Option<MicroUsd> {
    fee_for(price, u32::try_from(input_bound).ok()?, max_output)
}

/// A provider that could serve a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// The provider account.
    pub provider: AccountId32,
    /// The request's maximum fee at this provider's price.
    pub max_fee: MicroUsd,
    /// Smoothed time to first token observed by the gateway, if any.
    pub ttft_ms: Option<u32>,
}

/// Orders candidates, best first. The formula is open: gateways may substitute their own, and
/// SLA and reputation join once audits exist (M6; full plan: `f(price, SLA, reputation,
/// lineage)`).
pub trait RouteScore {
    /// Sorts `candidates` best first.
    fn rank(&self, candidates: &mut [Candidate]);
}

/// Cheapest first, then fastest first; an unobserved provider counts as fastest so that it gets
/// tried. Ties break on the account for determinism.
#[derive(Clone, Copy, Debug, Default)]
pub struct PriceThenLatency;

impl RouteScore for PriceThenLatency {
    fn rank(&self, candidates: &mut [Candidate]) {
        candidates.sort_by(|a, b| {
            (
                a.max_fee,
                a.ttft_ms.unwrap_or(0),
                AsRef::<[u8]>::as_ref(&a.provider),
            )
                .cmp(&(
                    b.max_fee,
                    b.ttft_ms.unwrap_or(0),
                    AsRef::<[u8]>::as_ref(&b.provider),
                ))
        });
    }
}

/// Exponentially weighted moving average of TTFT with α = 0.2, in milliseconds.
#[must_use]
pub fn ewma(previous: Option<u32>, sample_ms: u32) -> u32 {
    previous.map_or(sample_ms, |p| {
        let v = (u64::from(p).saturating_mul(4)).saturating_add(u64::from(sample_ms)) / 5;
        u32::try_from(v).unwrap_or(u32::MAX)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRICE: PricePerMTok = PricePerMTok {
        input: MicroUsd(100_000),
        output: MicroUsd(200_000),
    };

    #[test]
    fn maximum_fee_bounds() {
        // 1,000 prompt + 500 output tokens at $0.1 / $0.2 per million: 200 micro-dollars.
        assert_eq!(max_fee(&PRICE, 1_000, 500), Some(MicroUsd(200)));
        assert_eq!(max_fee(&PRICE, 0, 0), Some(MicroUsd(0)));
        assert_eq!(max_fee(&PRICE, 1, 0), Some(MicroUsd(1)));
        assert_eq!(max_fee(&PRICE, u64::from(u32::MAX) + 1, 1), None);
    }

    fn cand(id: u8, fee: u128, ttft: Option<u32>) -> Candidate {
        Candidate {
            provider: AccountId32::new([id; 32]),
            max_fee: MicroUsd(fee),
            ttft_ms: ttft,
        }
    }

    #[test]
    fn price_then_latency() {
        let mut c = vec![
            cand(1, 20, Some(5)),
            cand(2, 10, Some(90)),
            cand(3, 10, Some(30)),
            cand(4, 10, None),
        ];
        PriceThenLatency.rank(&mut c);
        let order: Vec<u8> = c
            .iter()
            .map(|c| AsRef::<[u8]>::as_ref(&c.provider)[0])
            .collect();
        assert_eq!(order, [4, 3, 2, 1]);
    }

    #[test]
    fn moving_average() {
        assert_eq!(ewma(None, 100), 100);
        assert_eq!(ewma(Some(100), 200), 120);
    }
}
