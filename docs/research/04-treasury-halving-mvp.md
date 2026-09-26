> 🌐 **English** | [简体中文](04-treasury-halving-mvp.zh-CN.md)

# Round 4: DAO Treasury, Stepwise Halving, the MVP Boundary

> Status: discussion draft (Round 4). Date: 2026-09.

## Newly confirmed decisions

| # | Decision |
|---|---|
| D15 | Emission halves **stepwise every 4 years** (keeping the "halving event" narrative) |
| D16 | The DAO treasury takes 20% (see the analysis in §1 for the mechanism) |
| D17 | Project name **AgentCoin**, token symbol **ATC** |

---

## 1. Treasuries and token allocation on mainstream chains

| Chain | Launch | Protocol-level treasury | Other allocation | Lessons |
|---|---|---|---|---|
| Bitcoin | Fair launch, PoW | **None** | 100% to miners | Development relies on donations and corporate sponsorship; the purest narrative, but long-term development funding is unstable |
| Monero | Fair launch, PoW + tail emission | None (community crowdfunding, CCS) | 100% to miners | Also relies on donations |
| Kaspa | Fair launch, PoW | None | 100% to miners | Strong fair-launch narrative |
| **Zcash** | PoW | **20%**: paid directly to development organizations 2020–24; since 2024-11 **8% community grants (ZCG) + 12% holder-controlled lockbox**; renewed by NU6.1 in 2025-11 | 80% to miners | The treasury **has to be re-voted every 4 years**, each time with fierce debate; it evolved from "paid directly to companies" to "holder-controlled" |
| **Dash** | PoW + masternodes | **20%** (doubled from 10% in 2023) | 60% masternodes, 20% miners | Proposal voting dominated by whales and masternodes; disputes over "how to split leftover budget" |
| **Decred** | PoW + PoS hybrid | **10%** | 1% PoW, 89% PoS (DCP-0012) | Ticket holders approve proposals; once had a treasury spending-policy bug |
| Cardano | PoS, premined | **20%** of rewards (τ) flows to the treasury | The rest to stakers | 2026 budget process: 69 proposals requesting ~330M ADA; high governance overhead |
| Polkadot | PoS, premined | Part of issuance + slashes go to the treasury | Stakers | **Spent about $87M in half a year in 2024**, sparking "reckless spending" criticism |
| Bittensor | Fair launch (no premine) | None | 41% miners, 41% validators, 18% subnet owners | The subnet-owner share effectively works as a "developer fund" |
| Ethereum | Premined | No protocol-level treasury | — | Runs on the foundation's premine allocation |

**Patterns**:
1. Chains without a premine that need long-term development funding all ended up "taxing emission", concentrated at **10–20%** (Zcash, Dash, Decred).
2. 20% is the industry ceiling; **the higher the share, the louder the "developer tax" controversy**.
3. Every treasury eventually evolves towards "**holder control + an independent grants committee**"; **paying founders or companies directly always ends in controversy**.
4. The biggest treasury risk is not running out of money but **governance captured by whales and spending without results** (the cautionary tales of Polkadot and Dash).

## 2. Our special problem: an unconditional treasury = a hidden premine

In our design emission is demand-gated, but if **the treasury's 20% is emitted unconditionally**, its share of circulating supply balloons when early demand is low. Taking the first 4-year period, with maximum emission of 2.625M ATC per year:

| Demand utilization | Treasury unconditional 20% | Treasury 5% floor + 15% by utilization | **Treasury entirely by utilization** |
|---|---|---|---|
| 10% | **54.1%** | 27.7% | 10.5% |
| 30% | 39.2% | 23.5% | 16.2% |
| 100% | 20.0% | 20.0% | 20.0% |

**Conclusion**: if the treasury takes 20% unconditionally, during cold start it takes more than half of that year's new circulating supply. And since you effectively control the treasury early on, outsiders would see it as **a disguised premine**, directly undermining the "no premine, fair launch" positioning.

**Recommendation: treasury = 20% of actual emission, growing with demand, plus a small floor (0–5%) of your choosing.**
- Cold-start development funding can come from: the floor, equity financing (a development company, no tokens involved), community donations.
- The floor should **vest linearly** (e.g. over 2 years) and may only fund audits and cold-start procurement.

## 3. A related fundamental problem: genesis cold start of a PoS chain without a premine

**No premine means nobody holds ATC at genesis, so nobody can stake to become a validator.** Options:

| Option | Description | Problem |
|---|---|---|
| A. Genesis PoA | The founder and volunteers are the initial validators and receive the security budget | The security budget flows to the founder — another form of premine |
| **B. Unpaid PoA + gradual opening** | Initial PoA validators **do not receive** the security budget (the security share rolls over); GPU miners can stake once they have mined ATC; when stake exceeds a threshold (e.g. 10% of circulating supply) and there are more than N validators, switch to PoS automatically | Cold-start security relies on the honesty of the PoA set, but on-chain value is also low during that time |
| C. PoW bootstrap | Produce blocks with PoW first, then switch to PoS | Conflicts with the "useful work" philosophy and wastes compute |

**Recommend option B**, with the switch conditions hard-coded on chain so nobody can delay them.

## 4. Making stepwise halving compatible with rollover

In a pure "remainder × 50%" model, if period 1 has insufficient demand, period 2's absolute emission may **rise instead of fall**, contradicting the "halving" narrative.

**Recommendation: scheduled emission + a rollover reserve (two accounts)**

1. **Scheduled emission** copies Bitcoin exactly: the **per-epoch scheduled amount** in period n (4 years each) = `(10,500,000 / 2^(n−1)) / epochs per period`. "The emission rate halves every 4 years" holds strictly, and all scheduled amounts sum to exactly 21 million.
2. Actual emission per epoch = `min(scheduled amount + drawable reserve, demand-gated ceiling)`.
3. **Scheduled amounts not emitted go into the "rollover reserve"**. At most 1× the scheduled amount (adjustable) can be drawn from the reserve per epoch — i.e. at most 2× the scheduled rate when demand is strong — to smooth shocks.
4. Cap = sum of scheduled emission = 21 million, never exceeded.

## 5. Revised emission split

| Share | Of actual emission | Gating |
|---|---|---|
| Security budget (validators) | 10% of the scheduled amount | Unconditional (rolls over during the cold-start PoA phase, see §3) |
| Paid market work | 50% | Demand-gated |
| Public work (DAO jobs) | 20% | Demand-gated (DAO job budget) |
| DAO treasury | 20% | **Proportional to actual work emission** (+ optional floor) |

Treasury governance: split it into **two accounts**, following Zcash:
- **Community grants**: approved by an independent committee, covering development, audits and ecosystem;
- **Holder treasury**: on-chain voting, covering large spending and cold-start procurement.
- As the founder, you can obtain development funding through grant applications, **fully public and transparent**.

---

## 6. The MVP boundary: anonymous vouchers in α or β?

### In α (anonymous from day one)

| Pros | Cons |
|---|---|
| Consistent with the "privacy-native" positioning; stronger brand | Highest technical risk: STARK circuits + PQ note encryption, implemented from scratch by one person |
| Core users (people in excluded regions) are protected from day one | Launch delayed by 4–6+ months |
| **Transparent on-chain records cannot be taken back**: early transparent transactions stay public forever | **A ZK circuit bug = an inflation bug** (Zcash found a bug in 2018 that allowed minting coins out of thin air); an audit is mandatory before real value, and it is expensive |
| Payment data structures designed only once | Client proving experience (time in browsers and phones) and proof size (50–200 KB) need tuning |
| | Debugging economics and cryptography at the same time makes problems harder to isolate |
| | Regulatory attention arrives earlier (exchanges delisting privacy coins; the EU AMLR is expected to restrict CASPs from handling anonymous coins from 2027) |

### In β (transparent prepaid credits first)

| Pros | Cons |
|---|---|
| Fastest way to close the core loop: supply and demand, verification, emission | Early users' usage records are transparent |
| Adding the privacy module via a runtime upgrade **exercises exactly the "pluggable" capability** | "Privacy chain" is not yet true in marketing terms |
| Audit costs can wait until the treasury has funds | The market module may be tainted by the "transparent payer" assumption, making later retrofitting costly |

### Recommendation: a compromise — "privacy-ready α + privacy as a hard gate for mainnet"

1. α already defines a unified `Credit` interface (voucher ID, nullifier, redemption, settlement). **Transparent prepaid credits and anonymous vouchers are two implementations of the same interface**; the market module depends only on the interface.
2. α implements **non-ZK privacy measures** from day one: prompts never stored on chain; gateways keep no logs; providers cannot see payers' on-chain addresses (gateways pay on the user's behalf and settle in batches).
3. The **testnet** uses only transparent credits: testnet tokens have no real value, so transparency costs nothing.
4. **Mainnet launch condition: anonymous vouchers (M7) complete and externally audited.** Mainnet never has a "non-private phase", so no permanently public early records are left behind.

This validates the economic model quickly without sacrificing the mainnet privacy promise.

## Sources

- Zcash: https://zips.z.cash/zip-1015 · https://z.cash/upgrade/nu6-1/ · https://zips.z.cash/zip-0271 · https://electriccoin.co/blog/zcash-halvening-nu6-embracing-the-new-dev-fund/
- Dash: https://docs.dash.org/en/stable/docs/user/masternodes/understanding.html · https://www.gate.com/learn/articles/dash-token-economics-analysis-block-rewards-masternode-yields-dao-governance
- Decred: https://docs.decred.org/advanced/issuance/ · https://blog.decred.org/2021/06/25/Treasury-Expenditure-Policy-Bug/
- Cardano: https://www.intersectmbo.org/cardano-budget-submission · https://cardanofoundation.org/blog/voting-decisions-2026-intersect-budget-process
- Polkadot treasury spending: https://m.theblockbeats.info/en/news/54059
- Bittensor: https://arxiv.org/pdf/2507.02951
