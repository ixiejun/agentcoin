> 🌐 [English](README.md) | **简体中文**

# pallet-audit

推理市场的链上审计（m6-audit-chain；规格 `market/audit`）：以美元计质押的审计员、按轮次随机分配、附双签收据的裁决、由随机抽出的复核人决定的争议、罚没与禁闭，以及审计资金池的支付。

```rust
use ac_primitives::market::audit::AuditParams;
assert!(AuditParams::LIVE.check().is_ok());
```
