#!/usr/bin/env python3
"""Parameters of the statistical judgment of audits, from the calibration (OpenSpec change
m6-audit-sprt design D2–D5, D9).

Reads the statistics histograms of one or more merged calibration reports (`calibration.json`
or the condensed `summary.json` of `scripts/calibrate-toploc.py --merge`). From them it derives
one version of the parameters:

- bins and log-likelihood ratios, in thousandths of a nat, rounded down, for each statistic
  (prefill mean, decode means averaged). The alternative is the int8 samples of every cell
  pooled. The null hypothesis takes, bin by bin, the largest honest frequency of any cell (the
  worst case). Pseudo-counts (honest 5, int8 0.5) keep a bin without honest samples from giving
  a huge ratio. Only prompts inside the audit length band count, as auditors send only those;
- the clamp on one verdict's contribution (−0.5 / +3.0 nats, design D3);
- the bound. A false alarm per provider and year is at most `audits × e^−h` when the honest
  `E[e^λ]` is at most 1 (Ville's inequality, design D4). At 52,560 audits a year and a target
  of 10⁻⁶, h is rounded up to a tenth of a nat;
- the per-auditor cap, a third of the bound, and the list size.

It writes them as JSON (--json) and as a Rust constant for `ac-primitives` (--rust). With
--simulate it also writes a simulation report:

- `E[e^λ]` of every cell's honest samples, from the joint counts of the two statistics, since
  they are correlated;
- the yearly false-alarm bound;
- the audits a CUSUM needs to cross the bound when a share of the requests (100%, 50%, 30%,
  20%) use int8, with a fixed seed.

It fails when the worst cell's `E[e^λ]` is above 0.8, the gate for enabling the judgment
(design D4).

Usage: scripts/export-audit-stats.py REPORT... [--version N] [--thresholds-version N]
       [--band MIN,MAX] [--json OUT] [--rust OUT] [--simulate OUT] [--seed N] [--runs N]
"""

from __future__ import annotations

import argparse
import json
import math
import pathlib
import random
import sys

STAT_BIN = 5  # the histograms' bin width, hundredths (scripts/calibrate-toploc.py)
PREFILL_EDGES = (30, 40, 50, 60, 70, 80, 90, 100, 110, 120, 140, 170, 220)
DECODE_EDGES = (60, 80, 100, 120, 140, 170, 200, 240, 280, 340, 450)
PSEUDO_HONEST = 5.0
PSEUDO_INT8 = 0.5
CLAMP = (-500, 3_000)
AUDITS_PER_YEAR = 52_560  # 2 auditors × 3 rounds an hour, per provider (live chains)
TARGET = 1e-6  # false alarms per provider and year
MAX_ENTRIES = 128
GATE = 0.8  # the worst cell's E[e^λ] at which the parameters cover a hardware class
FRACTIONS = (1.0, 0.5, 0.3, 0.2)


def load(paths: list[pathlib.Path]) -> dict:
    """The statistics of every cell of the reports; a cell in two reports is an error."""
    cells: dict = {}
    for p in paths:
        stats = json.loads(p.read_text()).get("statistics")
        if not stats:
            raise SystemExit(f"{p}: no statistics (merge it with a current calibrate-toploc.py)")
        for cell, sides in stats.items():
            if cell in cells:
                raise SystemExit(f"{p}: cell {cell} is in two reports")
            cells[cell] = sides
    return cells


def bin_of(value: int, edges: tuple[int, ...]) -> int:
    """Bin `j` holds the values from edge `j − 1` (included) to edge `j` (excluded)."""
    return sum(1 for e in edges if value >= e)


def counts(hist: dict, edges: tuple[int, ...]) -> list[int]:
    """A histogram in STAT_BIN bins ({bin start: count}) counted in the parameters' bins."""
    out = [0] * (len(edges) + 1)
    for start, n in hist.items():
        out[bin_of(int(start), edges)] += n
    return out


def check_edges(edges: tuple[int, ...]) -> None:
    if any(e % STAT_BIN for e in edges) or list(edges) != sorted(set(edges)) or not edges:
        raise SystemExit(f"edges must increase strictly and be multiples of {STAT_BIN}: {edges}")


def inside(cells: dict, variant: str) -> dict[str, dict]:
    """Every cell's histograms of `variant` for prompts inside the band."""
    out = {}
    for cell, sides in cells.items():
        g = sides.get("inside", {}).get(variant)
        if g and g["samples"]:
            out[cell] = g
    return out


def llr_table(cells: dict, key: str, edges: tuple[int, ...]) -> list[int]:
    """Log-likelihood ratios of `key`'s bins: pooled int8 against the worst honest cell."""
    honest, int8 = inside(cells, "honest"), inside(cells, "int8")
    if not honest or not int8:
        raise SystemExit("the reports need honest and int8 samples inside the band")
    k = len(edges) + 1
    worst = [0.0] * k
    for g in honest.values():
        c = counts(g[key], edges)
        n = sum(c)
        worst = [max(w, (c[j] + PSEUDO_HONEST) / (n + PSEUDO_HONEST * k)) for j, w in enumerate(worst)]
    pooled = [0] * k
    for g in int8.values():
        pooled = [a + b for a, b in zip(pooled, counts(g[key], edges))]
    n = sum(pooled)
    alt = [(pooled[j] + PSEUDO_INT8) / (n + PSEUDO_INT8 * k) for j in range(k)]
    return [math.floor(1000 * math.log(alt[j] / worst[j])) for j in range(k)]


def bound() -> int:
    """h with AUDITS_PER_YEAR × e^−h ≤ TARGET, in thousandths of a nat, rounded up to 100."""
    h = 1000 * math.log(AUDITS_PER_YEAR / TARGET)
    return math.ceil(h / 100) * 100


def derive(cells: dict, version: int, thresholds_version: int, band: tuple[int, int]) -> dict:
    check_edges(PREFILL_EDGES)
    check_edges(DECODE_EDGES)
    b = bound()
    return {
        "version": version,
        "thresholds_version": thresholds_version,
        "band": list(band),
        "prefill_edges": list(PREFILL_EDGES),
        "prefill_llr": llr_table(cells, "prefill", PREFILL_EDGES),
        "decode_edges": list(DECODE_EDGES),
        "decode_llr": llr_table(cells, "decode", DECODE_EDGES),
        "clamp": list(CLAMP),
        "per_auditor_cap": b // 3,
        "bound": b,
        "max_entries": MAX_ENTRIES,
    }


def contribution(p: dict, prefill: str, decode: str) -> int:
    """The contribution of a sample of the joint counts (bins as STAT_BIN starts, "-" for no
    decode chunk; the prompt is inside the band)."""
    c = p["prefill_llr"][bin_of(int(prefill), tuple(p["prefill_edges"]))]
    if decode != "-":
        c += p["decode_llr"][bin_of(int(decode), tuple(p["decode_edges"]))]
    return max(p["clamp"][0], min(p["clamp"][1], c))


def lambdas(p: dict, g: dict) -> list[tuple[int, int]]:
    """(contribution, count) of a histogram group's joint counts."""
    out = []
    for pair, n in g["pairs"].items():
        prefill, decode = pair.split(",")
        out.append((contribution(p, prefill, decode), n))
    return out


def e_lambda(dist: list[tuple[int, int]]) -> float:
    n = sum(c for _, c in dist)
    return sum(c * math.exp(l / 1000) for l, c in dist) / n


class Cusum:
    """The CUSUM of `ac-primitives` (`SprtState::record`): S ← max(0, S + counted), the
    per-auditor cap on positive contributions, the oldest entry dropped and the rest replayed
    when the list is full."""

    def __init__(self, p: dict):
        self.p, self.s, self.entries = p, 0, []

    def counted(self, auditor: int, c: int) -> int:
        if c <= 0:
            return c
        cap, used = self.p["per_auditor_cap"], 0
        for a, e in self.entries:
            if a == auditor:
                used += max(0, min(e, max(0, cap - used)))
        return min(c, max(0, cap - used))

    def push(self, auditor: int, c: int) -> None:
        nxt = max(0, self.s + self.counted(auditor, c))
        if nxt == 0:
            self.s, self.entries = 0, []
        else:
            self.s = nxt
            self.entries.append((auditor, c))

    def record(self, auditor: int, c: int) -> None:
        if len(self.entries) >= self.p["max_entries"]:
            kept, self.s, self.entries = self.entries[1:], 0, []
            for a, e in kept:
                self.push(a, e)
        self.push(auditor, c)

    def crossed(self) -> bool:
        return self.s >= self.p["bound"]


def draw(rng: random.Random, dist: list[tuple[int, int]]) -> int:
    return rng.choices([l for l, _ in dist], weights=[c for _, c in dist])[0]


def detection(p: dict, honest: list, int8: list, share: float, seed: int, runs: int,
              auditors: int = 20, limit: int = 5_000) -> dict:
    """Audits to cross the bound when `share` of the requests use int8, auditors drawn at random
    from `auditors`; runs that do not cross within `limit` audits count at the limit."""
    rng = random.Random(seed)
    need = []
    for _ in range(runs):
        s, n = Cusum(p), 0
        while not s.crossed() and n < limit:
            n += 1
            s.record(rng.randrange(auditors), draw(rng, int8 if rng.random() < share else honest))
        need.append(n)
    need.sort()
    return {"median": need[len(need) // 2], "p95": need[min(len(need) - 1, len(need) * 95 // 100)],
            "crossed": sum(1 for n in need if n < limit) / runs}


def simulate(cells: dict, p: dict, seed: int, runs: int) -> dict:
    honest = {cell: lambdas(p, g) for cell, g in inside(cells, "honest").items()}
    int8: list = []
    for g in inside(cells, "int8").values():
        int8 += lambdas(p, g)
    per_cell = {cell: {"honest": sum(c for _, c in d), "e_lambda": round(e_lambda(d), 4)}
                for cell, d in honest.items()}
    worst = max(per_cell, key=lambda c: per_cell[c]["e_lambda"])
    e = per_cell[worst]["e_lambda"]
    return {
        "cells": per_cell,
        "worst_cell": worst,
        "worst_e_lambda": e,
        "gate": GATE,
        "audits_per_year": AUDITS_PER_YEAR,
        "false_alarms_per_year": AUDITS_PER_YEAR * math.exp(-p["bound"] / 1000),
        "detection": {f"{share:.0%}": detection(p, honest[worst], int8, share, seed, runs)
                      for share in FRACTIONS},
        "seed": seed,
        "ok": e <= GATE,
    }


def rust(p: dict) -> str:
    """`p` as the `ac-primitives` constant `STATS_V<version>`."""
    def nums(xs):
        return ", ".join(f"{x:_}" for x in xs)
    return (f"pub const STATS_V{p['version']}: StatsParams = StatsParams {{\n"
            f"    version: {p['version']},\n"
            f"    thresholds_version: {p['thresholds_version']},\n"
            f"    band: ({p['band'][0]}, {p['band'][1]}),\n"
            f"    prefill_edges: &[{nums(p['prefill_edges'])}],\n"
            f"    prefill_llr: &[{nums(p['prefill_llr'])}],\n"
            f"    decode_edges: &[{nums(p['decode_edges'])}],\n"
            f"    decode_llr: &[{nums(p['decode_llr'])}],\n"
            f"    clamp: ({p['clamp'][0]:_}, {p['clamp'][1]:_}),\n"
            f"    per_auditor_cap: {p['per_auditor_cap']:_},\n"
            f"    bound: {p['bound']:_},\n"
            f"    max_entries: MAX_SPRT_ENTRIES,\n"
            f"}};\n")


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("reports", nargs="+", type=pathlib.Path)
    ap.add_argument("--version", type=int, default=1)
    ap.add_argument("--thresholds-version", type=int, default=4)
    ap.add_argument("--band", default="150,300")
    ap.add_argument("--json", type=pathlib.Path)
    ap.add_argument("--rust", type=pathlib.Path)
    ap.add_argument("--simulate", type=pathlib.Path)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--runs", type=int, default=1_000)
    a = ap.parse_args(argv)
    lo, hi = (int(x) for x in a.band.split(","))
    cells = load(a.reports)
    p = derive(cells, a.version, a.thresholds_version, (lo, hi))
    if a.json:
        a.json.write_text(json.dumps(p, indent=1) + "\n")
    if a.rust:
        a.rust.write_text(rust(p))
    print(json.dumps(p))
    if a.simulate:
        sim = simulate(cells, p, a.seed, a.runs)
        a.simulate.write_text(json.dumps({"params": p, "simulation": sim}, indent=1) + "\n")
        print(f"worst E[e^λ] {sim['worst_e_lambda']} ({sim['worst_cell']}), gate {GATE}; "
              f"false alarms per provider and year ≤ {sim['false_alarms_per_year']:.2e}")
        for share, d in sim["detection"].items():
            print(f"int8 on {share} of requests: {d['median']} audits (median), {d['p95']} (p95)")
        if not sim["ok"]:
            print(f"FAIL: E[e^λ] {sim['worst_e_lambda']} above the gate {GATE}", file=sys.stderr)
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
