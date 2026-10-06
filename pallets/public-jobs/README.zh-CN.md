> 🌐 [English](README.md) | **简体中文**

# pallet-public-jobs

AgentCoin 的公共任务队列（MVP 方案 §5.6；OpenSpec 变更 `m6-public-jobs`；规格 `market/public-jobs`）。零质押的工作者用 `ac-worker` 在链下三人一组执行评测、数据清洗与嵌入单元；本 pallet 负责抽取工作者、接收承诺与揭示、判定哪份摘要是单元的结果、记入公共工作量，并处罚被金丝雀抓到的多数方。数据与完整结果从不上链，上链的只有小摘要与结果哈希。

## 工作方式

1. **工作者**无需质押即可登记，声明最多 8 个可运行的模型（只做数据清洗时可不声明），并每轮声明一次就绪（`ready`；`ac-worker run` 自动完成）。每轮第一个区块冻结名单：上一轮就绪、未被暂停、待结算工作量未满的工作者，以及来自链上承诺—揭示随机数的种子（主题 `agentcoin/public-round`）。
2. **任务**由管理权限发布（M8 之前为 PoA 动议；`ac-wallet public publish` 打印调用）：类型、模型、比对规则版本 1、数据清单的 BLAKE3 哈希与地址、结果收集地址、1–100,000 个单元、每单元每名通过工作者的美元价格（不超过价格上限），以及可选的金丝雀 Merkle 根。同时进行中的任务最多 16 个；`cancel` 停止开放新单元。
3. **单元。**每轮按任务先后开放至多 `units_per_round` 个单元；某任务的单元凑不齐工作者时只让该任务等待，不阻塞其他任务。每个单元用本轮种子（域 `agentcoin 2026-10 public-assign v1`）抽取三名合格工作者；同一单元在各次尝试中不会再分给同一工作者。
4. **先承诺，再揭示。**在 `commit_by` 之前，每名工作者提交 `derive("agentcoin 2026-10 public-commit v1", job, unit, attempt, worker, summary, result_hash, salt)`；之后到 `reveal_by` 为止揭示摘要、结果哈希与盐。第三份揭示到达时单元结算，或在期限过后由 `close` 结算（对其工作者免费）。
5. **结算（规则 v1）。**两份摘要一致的条件：评测（答案序号）不同的题目至多 max(1, 2%)；嵌入（每段文本 32 位指纹）汉明距离超过 3 的文本至多 max(1, 2%)；数据清洗（结果的 BLAKE3）完全相等。基准是与其他摘要一致最多的那份（并列取靠前的位置）；至少两份与之一致时单元通过。多数方获得该单元价格的公共工作量；其余人与未揭示者记一次失误。没有多数的单元交给新的工作者重开，至多三次尝试。连续三次失误的工作者被暂停。
6. **金丝雀。**发布方知道金丝雀单元的预期摘要。单元通过后，任何人都可以用 Merkle 证明公开它的叶子 `derive("agentcoin 2026-10 public-canary v1", 0x00 ‖ job ‖ unit ‖ summary ‖ salt)`（`reveal_canary`；`ac-worker collect` 自动完成）。若基准与它不一致，单元失败，多数方每人被销毁锁定中的报酬、作废全部未领取工作量（已结算的份额从领取账户销毁）并被暂停；与金丝雀一致的少数方获得该单元。
7. **报酬。**公共工作量在 `challenge_epochs` 个排放纪元后计入；随后排放把该纪元的公共排放铸入领取账户（`PalletId(*b"ac/publc")`，经 `PublicPayout`）。工作者用 `claim` 领取已结算纪元的份额，份额转入其账户并以 `Locked` 冻结至 `lock_blocks` 之后（向上取整到锁定期的 1/16，至多 32 段），期间可被罚没，期满后用 `withdraw` 解冻。

工作者为被分配工作发出的调用（`ready`、`commit`、`reveal`、`close`、`claim`、`withdraw`）成功时退还手续费：工作者只需少量余额支付预先扣除的手续费。

## 参数

| 参数 | 正式链草案 | 测试预设 | 护栏 | 修改方式 |
|---|---|---|---|---|
| 轮次长度 | 600 个区块 | 10 | > 0 | 创世 |
| 每轮开放单元数 | 64 | 8 | 1–256 | 创世 |
| 承诺期限 | 1,800 个区块 | 20 | > 0 | 创世 |
| 揭示期限 | 300 个区块 | 10 | > 0 | 创世 |
| 挑战期 | 2 个纪元 | 1 | ≥ 1 | 创世 |
| 报酬锁定期 | 604,800 个区块 | 30 | > 0 | 创世 |
| 暂停期 | 86,400 个区块 | 30 | > 0 | 创世 |
| 单元记录保留期 | 604,800 个区块 | 200 | > 0 | 创世 |
| 每单元价格上限 | 1 美元 | 1 美元 | 0–10 美元 | 管理权限（`set_price_cap`） |

## 调用

`register`、`set_models`、`deregister`、`ready`、`publish` 与 `cancel`（管理权限）、`commit`、`reveal`、`close`、`reveal_canary`、`claim`（至多 16 个纪元）、`withdraw`、`set_price_cap`（管理权限）。钱包以 `ac-wallet public …` 封装这些调用；`PublicJobsApi` 查询轮次、名单、工作者、任务、单元、分配、纪元、待领取工作量、锁定中的报酬、领取账户余额与参数。

## 示例

创世参数的护栏：

```rust
use ac_primitives::market::public::{ParamsError, PublicParams};

assert!(PublicParams::LIVE.check().is_ok());
assert!(PublicParams::DEV.check().is_ok());
let broken = PublicParams { units_per_round: 0, ..PublicParams::LIVE };
assert_eq!(broken.check(), Err(ParamsError::UnitsPerRound));
```

## 功能开关

`std`（默认）、`runtime-benchmarks`、`try-runtime`。
