> 🌐 **English** | [简体中文](07-constitution-immutability.zh-CN.md)

# Round 7: How Constitutional Clauses Become "Immutable" — Polkadot, Bitcoin and Cardano Compared

> Status: discussion draft (Round 7). Date: 2026-09.

## Newly confirmed decisions

| # | Decision |
|---|---|
| D27 | Private capabilities: MVP ships L0 (private transfers, anonymous inference vouchers); the full version ships L1 (private voting, sealed-bid compute procurement, private swaps inside the shielded pool) with in-house circuits audited once; L2 general private contracts only have interfaces reserved and are re-evaluated around 2028; no L3 |
| D28 | 5% trainer royalties: incentives instead of enforcement (lineage declaration + routing priority) |
| D29 | Transition period for USD pricing: governance sets a reference rate (with a cap on each adjustment); switch to an oracle once liquidity is sufficient |

---

## 1. Polkadot's approach: **no immutable clauses — everything can be voted on**

- **Mechanism**: OpenGov multi-track referenda.
  - **Root track**: the highest privileges; can replace the entire runtime. Very high thresholds: on day 1 it needs support of 46.8% of total issuance and approval above 88%; the support curve falls linearly to 25% over 7 days and approaches 0 by day 14. Long preparation and enactment periods.
  - **Wish For Change track**: **executes no code**; an on-chain "signal vote" used to build consensus before a formal proposal.
  - **Technical Fellowship whitelist**: the technical fellowship can whitelist urgent fixes for a shorter process (for security fixes).
- **How the 2.1B hard cap landed**: first Ref 1710 passed on the Wish For Change track with 81% approval as a signal; then formal proposals such as Ref 1828 wrote it into the protocol via runtime upgrade v2.1.0 (2026-03-12).
- **Key fact**: the hard cap **lives in the runtime**, and the runtime can be replaced wholesale by a Root referendum. Analysts therefore point out that "**governance-driven supply changes carry execution risk — future votes could theoretically reverse them — while Bitcoin's halvings are mathematically inevitable.**"
- **Conclusion**: Polkadot's "constitution" is **a high-threshold mutable rule**, not an immutable clause. It relies on high voting thresholds and long timelines, not technical impossibility.

## 2. Bitcoin's approach: **enforced by node clients; change requires a hard fork**

- The 21 million cap exists in no governance system; **every full node checks it independently when validating blocks**. Any over-issuance is rejected by all honest nodes.
- The only way to change the cap is to convince the vast majority of node operators to **voluntarily install new client software** (a hard fork). Those who refuse stay on the original chain.
- **Conclusion**: immutability really means "**raising the cost of change until it requires society-wide voluntary agreement**". There is no absolute technical immutability, but this is the strongest form available.

## 3. Cardano's approach: **written constitution + constitutional committee + guardrails script**

- Since the Plomin hard fork in 2025, the hash of the constitution text is formally recorded on chain.
- The **constitutional committee** reviews whether each governance action is constitutional.
- **Guardrails script**: an on-chain validation script that **automatically rejects** parameter changes outside the ranges the constitution allows (e.g. a parameter may only move between X and Y).
- Amending the constitution or the guardrails script itself needs a higher threshold in the 65–90% range.
- **Conclusion**: in between Polkadot and Bitcoin — "**mutable, but with machine-enforced boundaries**".

## 4. Comparison

| | Polkadot | Cardano | Bitcoin |
|---|---|---|---|
| Where the core clauses live | Runtime (upgradable) | On-chain constitution + guardrails script | Node client |
| How to change them | Root referendum | Higher-threshold constitutional amendment | Hard fork (nodes upgrade voluntarily) |
| Machine enforcement | No | Partial (parameter bounds) | Yes |
| Difficulty of change | High | Higher | Extremely high |
| Adaptability | Best | Medium | Worst |

---

## 5. AgentCoin's design: three layers of protection

Combine Bitcoin's "node enforcement", Cardano's "guardrails" and Polkadot's "track-based governance", each applied to rules of a different level of importance.

### Layer 1: constitutional invariants enforced by the node client (the Bitcoin model)

For the clauses in D24 that can be checked mechanically:

| Invariant | What nodes check on every block |
|---|---|
| Supply cap | `total_issuance ≤ 21,000,000 × 10^18` |
| Emission curve / no premine | This epoch's minting ≤ scheduled amount + drawable reserve (computed by the formula fixed at genesis); no minting outside the emission path |
| PoA → PoS switch | Once conditions are met, the switch must complete by the next era, or blocks are rejected |

Implementation notes:
- The check logic lives in the **node client (native code)**, **not in the runtime**, so runtime upgrades cannot change it.
- After executing a block, the node reads agreed storage keys (e.g. total issuance) and validates them. If a runtime upgrade removes those keys or changes their format, the node **rejects by default** (fail-closed).
- Changing these clauses is only possible via a **hard fork**: release a new client and let node operators upgrade voluntarily.

### Layer 2: constitution + guardrails (the Cardano model)

For clauses that **cannot be fully mechanized** or **need some flexibility**:

- **On-chain constitution text** (its hash on chain), including "protocol neutrality: no content censorship or geo-blocking at the protocol layer".
- **Guardrails pallet**: every parameter change must fall within the ranges the constitution specifies, e.g.:
  - treasury share only between 0–20%;
  - burn ratio only within the specified interval;
  - each reference-rate adjustment at most ±20%.
- **Prohibition checks**: before execution, a runtime-upgrade proposal must include a public code-diff report; both houses (contributor house + token house) review whether it is constitutional.
- Amending the constitution itself: **supermajorities in both houses** (e.g. 75% each) plus a longer delay (e.g. 90 days).

### Layer 3: day-to-day governance (the Polkadot OpenGov model)

- Track-based referenda: Root (runtime upgrades), parameters, treasury, security council and other tracks, each with its own threshold curves and enactment delay.
- **Signal track** (like Wish For Change): expresses intent only and executes no code, used to build consensus before major proposals.
- **Security-council fast track** (like the Fellowship whitelist): **can only pause modules and revoke TEE attestations**, never touches layer-1 or layer-2 clauses, and must be ratified by a full vote afterwards.
- After a runtime upgrade passes, a mandatory 28-day wait before it takes effect gives dissenters time to exit.

### An honest caveat

Absolute immutability does not exist. Bitcoin's 21 million cap is ultimately maintained by social consensus too. What we can achieve:
- layer-1 clauses as hard to change as **Bitcoin's**;
- layer 2 at **Cardano's** level;
- everything else keeps **Polkadot-style evolvability**.

## Sources

- Polkadot hard cap: https://www.bitget.com/news/detail/12560605240453 · https://rwatimes.substack.com/p/polkadot-dot-price-prediction-2026-10b · https://www.kucoin.com/news/articles/polkadot-set-to-implement-landmark-2-1-billion-dot-supply-cap-in-march-2026
- OpenGov: https://wiki.polkadot.com/learn/learn-polkadot-opengov/ · https://wiki.polkadot.com/learn/learn-polkadot-opengov-origins/ · https://wiki.polkadot.com/learn/learn-polkadot-technical-fellowship/ · https://docs.polkadot.com/reference/governance/
- Cardano: https://cips.cardano.org/cip/CIP-1694 · https://docs.intersectmbo.org/archive/cardano-governance-archive/cardano-constitution/read-the-cardano-constitution · https://www.theblock.co/post/337680/cardano-plans-transition-to-full-decentralized-governance-after-wednesdays-plomin-hard-fork · https://developers.cardano.org/docs/governance/cardano-governance/submitting-governance-actions/
