> 🌐 **English** | [简体中文](06-private-contracts-and-final-decisions.zh-CN.md)

# Round 6: Why Private Contracts + Converging Decisions

> Status: discussion draft (Round 6). Date: 2026-09.

## Newly confirmed decisions

| # | Decision | Implementation notes |
|---|---|---|
| D22 | Community-model weights are fully open; each paid inference on this network automatically diverts about 5% to training contributors | Derivatives (fine-tuned, new hash) can bypass royalties and cryptography cannot enforce them. Countermeasure: declare **lineage** at model registration; derivatives that declare lineage get a "verified lineage" badge and gateway routing priority — incentives instead of enforcement |
| D23 | Inference priced in USD, settled in ATC | Needs an ATC/USD oracle; **the oracle cannot work until ATC has enough market liquidity**. During the transition, governance sets a reference rate (with adjustment limits), or pricing is done directly in ATC |
| D24 | Constitutional clauses: 21 million cap; no content censorship or geo-blocking at the protocol layer; no premine; PoA → PoS switch conditions | See "Making the constitution truly immutable" below |
| D25 | Bicameral governance: token house + contributor house (votes weighted by work score) | The contributor house needs **private voting** to prevent vote buying — exactly one use of private contracts (§1) |
| D26 | The storage layer is dedicated storage only | |

### Making the "constitution" truly immutable

A Substrate runtime can be replaced wholesale by governance, so **any rule written in the runtime can, in essence, be voted away**. True immutability requires moving the checks into the **node client**, following Bitcoin's full-node model:

1. **Supply cap**: on importing each block, nodes independently check `total issuance ≤ 21,000,000 ATC` and reject violating blocks. Even if a malicious runtime upgrade passes a vote, honest nodes will not accept it. Changing this rule requires a **hard fork**, i.e. every node operator voluntarily upgrading their client.
2. **No premine / emission rules**: nodes verify that each epoch's minting ≤ the ceiling allowed by the emission curve.
3. **PoA → PoS switch**: nodes enforce the switch once conditions are met; blocks that still have not switched after the deadline are rejected.
4. **Protocol neutrality**: cannot be fully checked mechanically; it is constrained by "public review of runtime-upgrade content + social consensus + client checks for specific blacklist-type storage items".

---

## 1. What are private contracts for?

### 1.1 First: public contracts are the foundation; private contracts only add to them

Private contracts do not replace public ones; the two coexist. Developers choose freely — most DApps are fine with public EVM contracts. The only question is **which scenarios must be private because otherwise they cannot work at all, or they harm users**.

### 1.2 What goes wrong with only public contracts + a shielded pool

| Problem | Explanation | Relevance to us |
|---|---|---|
| **Privacy breaks at the contract boundary** | Users are anonymous inside the shielded pool, but as soon as they touch a public contract (e.g. swapping ATC for another asset on a DEX), amounts, timing and counterparties are exposed and attackers can link identities back | "Privacy-native" is only half achieved |
| **MEV / front-running** | Public transaction intents get sandwiched and front-run, extracting enormous value from users every year | Especially severe for high-frequency agent trading |
| **Strategy leakage** | Agents' trading strategies, bids and budgets are all public and can be copied or targeted | Agents are our core users |
| **Auction collusion** | With public bids in compute procurement, providers see each other's quotes and can collude | Compute procurement for large training jobs |
| **Vote buying and coercion** | Public votes let people prove "whom I voted for", enabling bribery or coercion | **Needed by D25's contributor house and token voting** |
| **Trade secrets** | Enterprises will not put payroll, supply chains or customer data on a public chain | Enterprise AI procurement |

### 1.3 Four levels of private capability

| Level | Content | Technology | Maturity | Post-quantum |
|---|---|---|---|---|
| **L0** Public contracts + shielded pool | Private transfers, anonymous inference vouchers | Fixed STARK circuits | Mature (the Zcash model) | ✅ (STARK + hashes) |
| **L1** Protocol-built private features | Private voting, sealed-bid auctions, private swaps inside the shielded pool, private compute procurement | **Fixed circuits** written by us and audited once | Feasible | ✅ |
| **L2** General programmable private contracts | Developers write their own private logic | Aztec / Noir style: executed and proven on the client | Aztec's 2026 Alpha still has serious vulnerabilities; needs a new language and toolchain; **Aztec's proof system is based on the BN254 curve and is not post-quantum** | ❌ would need an in-house PQ proof backend |
| **L3** Shared private state | E.g. a private AMM with hidden reserves, where many parties read and write the same encrypted state | FHE / MPC / TEE | Immature, poor performance | FHE can be post-quantum, but slow |

**Key points**:
- L1 already covers the scenarios we need most — agent trading, voting, compute procurement — all implementable with PQ circuits under our control.
- L2's generality is tempting, but there is no mature post-quantum solution today, the engineering effort is huge, and a circuit bug equals an inflation bug.
- L3 is a research area.

### 1.4 Recommendation

| Phase | Private capabilities |
|---|---|
| MVP | L0: shielded pool + anonymous inference vouchers |
| Full, P2 | L1: **private voting** (a prerequisite for the bicameral system), **sealed-bid compute procurement** |
| Full, P3 | L1: private swaps inside the shielded pool (privacy no longer breaks at the DEX); private agent budgets and payments |
| Research track | L2: reserve a STARK-verification precompile and the "private execution → proof → verification by a public contract" interface; decide on general private contracts once PQ proof backends and tooling mature (expected after 2028) |
| Not doing | L3 |

## Sources

- Continues the sources of earlier rounds; for Aztec's status see 01-landscape.md.
