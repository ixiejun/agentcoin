> 🌐 **English** | [简体中文](README.zh-CN.md)

# pallet-audit

On-chain audits of the inference market (m6-audit-chain; spec `market/audit`): auditors with a
dollar-denominated stake, random assignment per round, verdicts on signed receipts, disputes
decided by randomly drawn reviewers, slashing and jailing, and payments from the audit pot.

```rust
use ac_primitives::market::audit::AuditParams;
assert!(AuditParams::LIVE.check().is_ok());
```
