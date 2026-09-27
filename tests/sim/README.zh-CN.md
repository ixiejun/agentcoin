> 🌐 [English](README.md) | **简体中文**

# ac-sim

AgentCoin 的经济模拟。它在大量纪元上重放排放结算（`ac_primitives::emission::settle`，与 runtime 运行的是同一份代码），并把每个纪元的结果与按 MVP 技术方案 §5.1 独立重写的公式逐项比较。

- `tests/eight_years.rs`：按正式链的纪元长度（3600 块）模拟两个 4 年期，分无工作量、满负荷和确定性随机工作量三种情形。每个纪元都与参考公式完全一致，累计铸币从不超过累计计划量，计划总量低于 21,000,000 ATC。

示例代码见英文版 [README.md](README.md)（作为 doctest 运行）。

运行：`cargo test -p ac-sim`。
