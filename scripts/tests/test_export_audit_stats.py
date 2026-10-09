"""Tests for scripts/export-audit-stats.py (OpenSpec change m6-audit-sprt 2.1, 2.2; design D2–D5,
D9)."""

import importlib.util
import json
import math
import pathlib
import re
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent.parent
SPEC = importlib.util.spec_from_file_location("export_stats", ROOT / "scripts" / "export-audit-stats.py")
ex = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ex)

K_PRE = len(ex.PREFILL_EDGES) + 1
K_DEC = len(ex.DECODE_EDGES) + 1


def group(pairs: dict) -> dict:
    """A histogram group as calibrate-toploc.py writes it, from joint counts."""
    g = {"samples": sum(pairs.values()), "prefill": {}, "decode": {}, "pairs": dict(pairs)}
    for pair, n in pairs.items():
        p, d = pair.split(",")
        g["prefill"][p] = g["prefill"].get(p, 0) + n
        if d != "-":
            g["decode"][d] = g["decode"].get(d, 0) + n
    return g


def cell(honest: dict, int8: dict) -> dict:
    return {"inside": {"honest": group(honest), "int8": group(int8)}}


# Honest around 0.5 / 1.0, int8 around 1.25 / 2.5 (hundredths), as in the GPU calibration.
HONEST = {"50,100": 900, "45,95": 100}
INT8 = {"125,250": 90, "130,255": 10}


class TableTests(unittest.TestCase):
    def test_bins_include_their_lower_edge(self):
        self.assertEqual(ex.bin_of(29, ex.PREFILL_EDGES), 0)
        self.assertEqual(ex.bin_of(30, ex.PREFILL_EDGES), 1)
        self.assertEqual(ex.bin_of(65_535, ex.PREFILL_EDGES), K_PRE - 1)

    # Smoothing: the pseudo-counts make an empty bin finite; the ratio is rounded down.
    def test_smoothing_and_rounding(self):
        cells = {"A → A": cell(HONEST, INT8)}
        llr = ex.llr_table(cells, "prefill", ex.PREFILL_EDGES)
        j = ex.bin_of(125, ex.PREFILL_EDGES)
        alt = (100 + ex.PSEUDO_INT8) / (100 + ex.PSEUDO_INT8 * K_PRE)
        null = (0 + ex.PSEUDO_HONEST) / (1000 + ex.PSEUDO_HONEST * K_PRE)
        self.assertEqual(llr[j], math.floor(1000 * math.log(alt / null)))
        self.assertLess(llr[j], 1000 * math.log(alt / null))
        # A bin with neither honest nor int8 samples never counts as evidence of int8.
        self.assertLessEqual(llr[0], 0)
        self.assertEqual(len(llr), K_PRE)

    # The null hypothesis takes the worst (largest) honest frequency of any cell, bin by bin.
    def test_worst_honest_cell(self):
        shifted = {"125,250": 50, "50,100": 950}
        one = ex.llr_table({"A → A": cell(HONEST, INT8)}, "prefill", ex.PREFILL_EDGES)
        two = ex.llr_table({"A → A": cell(HONEST, INT8), "B → B": cell(shifted, {})},
                           "prefill", ex.PREFILL_EDGES)
        j = ex.bin_of(125, ex.PREFILL_EDGES)
        self.assertLess(two[j], one[j])
        # Other bins never get a larger ratio from a second cell.
        self.assertTrue(all(b <= a for a, b in zip(one, two)))

    def test_bound_and_cap(self):
        p = ex.derive({"A → A": cell(HONEST, INT8)}, 1, 4, (150, 300))
        self.assertEqual(p["bound"], 24_700)
        self.assertLessEqual(ex.AUDITS_PER_YEAR * math.exp(-p["bound"] / 1000), ex.TARGET)
        self.assertEqual(p["per_auditor_cap"], p["bound"] // 3)
        self.assertEqual((p["clamp"], p["max_entries"]), ([-500, 3_000], 128))

    def test_reports_need_both_variants(self):
        with self.assertRaises(SystemExit):
            ex.llr_table({"A → A": {"inside": {"honest": group(HONEST)}}}, "prefill", ex.PREFILL_EDGES)

    def test_edges_must_fit_the_histogram_bins(self):
        with self.assertRaises(SystemExit):
            ex.check_edges((30, 42))
        with self.assertRaises(SystemExit):
            ex.check_edges((40, 30))

    def test_a_cell_in_two_reports_is_an_error(self):
        with tempfile.TemporaryDirectory() as d:
            a, b = pathlib.Path(d, "a.json"), pathlib.Path(d, "b.json")
            for p in (a, b):
                p.write_text(json.dumps({"statistics": {"A → A": cell(HONEST, INT8)}}))
            with self.assertRaises(SystemExit):
                ex.load([a, b])


class CusumTests(unittest.TestCase):
    P = {"per_auditor_cap": 6_000, "bound": 18_000, "max_entries": 4}

    # The same cases as ac-primitives' SprtState tests.
    def test_matches_the_rust_state(self):
        s = ex.Cusum(self.P)
        for a, c in [(1, 1_000), (2, -500), (3, 2_000), (4, 3_000)]:
            s.record(a, c)
        self.assertEqual(s.s, 5_500)
        s.record(5, 100)
        self.assertEqual((s.s, [a for a, _ in s.entries]), (5_100, [3, 4, 5]))
        s = ex.Cusum(dict(self.P, max_entries=128))
        for c in (3_000, 2_000, 3_000, 3_000):
            s.record(1, c)
        self.assertEqual(s.s, 6_000)
        s.record(1, -6_000)
        self.assertEqual((s.s, s.entries), (0, []))


class SimulationTests(unittest.TestCase):
    def test_honest_and_int8_apart(self):
        cells = {"A → A": cell(HONEST, INT8)}
        p = ex.derive(cells, 1, 4, (150, 300))
        sim = ex.simulate(cells, p, seed=1, runs=50)
        self.assertTrue(sim["ok"])
        self.assertLess(sim["worst_e_lambda"], ex.GATE)
        self.assertLessEqual(sim["false_alarms_per_year"], ex.TARGET)
        self.assertEqual(sim["detection"]["100%"]["median"], 9)  # 9 × 3.0 ≥ 24.7 nats
        self.assertEqual(sim["detection"]["100%"]["crossed"], 1.0)
        self.assertEqual(sim, ex.simulate(cells, p, seed=1, runs=50), "deterministic")

    # The supermartingale condition does not hold: one cell's honest samples look like int8.
    def test_fails_above_the_gate(self):
        cells = {"A → A": cell(HONEST, INT8), "B → B": cell({"50,100": 700, "125,250": 300}, {})}
        p = ex.derive(cells, 1, 4, (150, 300))
        p["prefill_llr"] = ex.llr_table({"A → A": cells["A → A"]}, "prefill", ex.PREFILL_EDGES)
        p["decode_llr"] = ex.llr_table({"A → A": cells["A → A"]}, "decode", ex.DECODE_EDGES)
        sim = ex.simulate(cells, p, seed=1, runs=10)
        self.assertFalse(sim["ok"])
        self.assertEqual(sim["worst_cell"], "B → B")
        with tempfile.TemporaryDirectory() as d:
            r = pathlib.Path(d, "r.json")
            r.write_text(json.dumps({"statistics": cells}))
            # Even derived from both cells, B's honest samples overlap int8 too much: the script
            # reports the failure.
            out = pathlib.Path(d, "s.json")
            self.assertEqual(ex.main([str(r), "--simulate", str(out), "--runs", "10"]), 1)
            self.assertFalse(json.loads(out.read_text())["simulation"]["ok"])

    def test_rust_constant(self):
        p = ex.derive({"A → A": cell(HONEST, INT8)}, 2, 4, (150, 300))
        text = ex.rust(p)
        self.assertIn("pub const STATS_V2: StatsParams = StatsParams {", text)
        self.assertIn("bound: 24_700,", text)
        llr = re.search(r"prefill_llr: &\[([^\]]*)\]", text).group(1)
        self.assertEqual([int(x.replace("_", "")) for x in llr.split(",")], p["prefill_llr"])


if __name__ == "__main__":
    unittest.main()
