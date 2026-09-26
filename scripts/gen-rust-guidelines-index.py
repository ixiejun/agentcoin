#!/usr/bin/env python3
"""Generate docs/rust-guidelines/INDEX.md and rules.tsv from the upstream SUMMARY.md.

Usage: python3 scripts/gen-rust-guidelines-index.py
The output is deterministic: rerunning on the same SUMMARY.md yields identical files.
"""
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent / "docs" / "rust-guidelines"
SUMMARY = ROOT / "src" / "SUMMARY.md"
LINK = re.compile(r"^(?P<indent>\s*)- \[(?P<title>.+?)\]\((?P<path>[^)]+)\)")
RULE = re.compile(r"^(?P<id>[PG](?:\.[A-Z]{2,4})+\.\d{2})\s+(?P<text>.+)$")

# AgentCoin 对上游规则的取舍（见 AGENT.md §7.3）。未列出的规则一律采纳。
OVERRIDES = {
    "P.ERR.02": "覆盖：非测试代码禁止 unwrap/expect，改用 `?` 或显式错误",
    "G.ERR.02": "覆盖：同上，非测试代码禁止 expect",
    "P.NAM.09": "不采纳：静态变量沿用 SCREAMING_SNAKE_CASE，不加 G_ 前缀",
    "G.TYP.BOL.07": "不采纳：使用 `!` 取反即可",
    "P.CMT.04": "不采纳：不要求文件头版权注释（仓库级 LICENSE）",
    "G.MTH.LCK.03": "视情况：runtime（no_std）不适用；链下服务可用 parking_lot",
    "G.MTH.LCK.04": "视情况：链下服务优先 tokio::sync / crossbeam",
}

# 与本项目高度相关、编码智能体必须优先阅读的规则
FOCUS = [
    "G.TYP.INT.01", "G.TYP.INT.02", "G.TYP.01", "G.TYP.03", "P.TYP.01",
    "G.TYP.ARR.02", "G.EXP.03", "G.ERR.01", "P.ERR.01", "G.CMT.01", "G.CMT.02",
    "G.TYP.SCT.01", "G.TYP.ENM.05", "G.TYP.ENM.07", "P.MOD.01", "G.MOD.03",
    "P.CAR.02", "G.CAR.02", "G.CAR.04", "P.SEC.01", "G.SEC.01",
    "P.EMB.01", "P.EMB.02", "G.MEM.DRP.01", "P.UNS.01", "P.UNS.02",
    "P.UNS.SAS.09", "G.UNS.SAS.01", "G.ASY.02", "G.ASY.05", "G.FUD.01", "G.FUD.03",
]


def parse():
    sections, rules = [], []
    stack = []  # (indent, title)
    for line in SUMMARY.read_text(encoding="utf-8").splitlines():
        m = LINK.match(line)
        if not m:
            continue
        indent = len(m["indent"].replace("\t", "    "))
        title, path = m["title"].strip(), m["path"].strip().lstrip("./")
        r = RULE.match(title)
        while stack and stack[-1][0] >= indent:
            stack.pop()
        if r:
            crumb = " / ".join(t for _, t in stack[1:]) or stack[0][1] if stack else ""
            rules.append((r["id"], r["text"], crumb, f"src/{path}"))
        else:
            sections.append((indent, title, f"src/{path}"))
            stack.append((indent, title))
    return sections, rules


def cell(text):
    """Escape a value for use inside a Markdown table cell."""
    return text.replace("|", "\\|")


def main():
    sections, rules = parse()
    by_id = {r[0]: r for r in rules}
    out = []
    w = out.append
    w("# Rust 编码规范索引（AgentCoin 本地副本）\n")
    w("> 自动生成，请勿手改：`python3 scripts/gen-rust-guidelines-index.py`。")
    w("> 上游：<https://rust-coding-guidelines.github.io/rust-coding-guidelines-zh/>，来源与版本见 `SOURCE.md`（English）/ `SOURCE.zh-CN.md`（中文），许可证 MIT（`LICENSE`）。")
    w("> 编号约定：`P.*` 为原则（Principle），`G.*` 为规则（Guideline），详见 `src/safe-guides/overview/convention.md`。")
    w("> 查找方式：`grep -n '<关键词或编号>' docs/rust-guidelines/rules.tsv`，再打开对应文件。\n")
    w(f"共 {len(rules)} 条（原则 {sum(1 for r in rules if r[0].startswith('P'))}，规则 {sum(1 for r in rules if r[0].startswith('G'))}）。\n")

    w("## 1. AgentCoin 重点规则（编码前必读）\n")
    w("| 编号 | 内容 | 文件 |")
    w("|---|---|---|")
    for rid in FOCUS:
        if rid in by_id:
            _, text, _, path = by_id[rid]
            w(f"| {rid} | {cell(text)} | [{path}]({path}) |")
    w("")

    w("## 2. AgentCoin 取舍（覆盖或不采纳的上游条款）\n")
    w("| 编号 | 上游内容 | AgentCoin 决定 |")
    w("|---|---|---|")
    for rid, note in OVERRIDES.items():
        text = by_id[rid][1] if rid in by_id else "（上游未找到）"
        w(f"| {rid} | {cell(text)} | {note} |")
    w("")

    w("## 3. 章节目录\n")
    for indent, title, path in sections:
        w(f"{'  ' * (indent // 4)}- [{title}]({path})")
    w("")

    w("## 4. 全部条款（按章节）\n")
    current = None
    for rid, text, crumb, path in rules:
        if crumb != current:
            w(f"\n### {crumb}\n")
            w("| 编号 | 内容 | 文件 |")
            w("|---|---|---|")
            current = crumb
        mark = f" ⚠️ {OVERRIDES[rid]}" if rid in OVERRIDES else ""
        w(f"| {rid} | {cell(text)}{mark} | [{path}]({path}) |")
    w("")
    (ROOT / "INDEX.md").write_text("\n".join(out), encoding="utf-8")

    tsv = ["id\tkind\tsection\ttext\tpath\tagentcoin"]
    for rid, text, crumb, path in rules:
        kind = "principle" if rid.startswith("P") else "guideline"
        tsv.append(f"{rid}\t{kind}\t{crumb}\t{text}\t{path}\t{OVERRIDES.get(rid, '')}")
    (ROOT / "rules.tsv").write_text("\n".join(tsv) + "\n", encoding="utf-8")
    print(f"indexed {len(rules)} rules, {len(sections)} sections")


if __name__ == "__main__":
    main()
