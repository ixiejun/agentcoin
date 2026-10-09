"""Tests for scripts/calibrate-toploc.py without an engine (m6-toploc-gpu-calibration 1.1–1.4;
spec engineering/ci-quality-gates "复核阈值校准工作流", "GPU 跨硬件校准")."""

import contextlib
import importlib.util
import io
import json
import pathlib
import re
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent.parent
SPEC = importlib.util.spec_from_file_location("calibrate", ROOT / "scripts" / "calibrate-toploc.py")
cal = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(cal)

EXACT = {"exp_mismatches": 0, "mant_err_sum": 0, "mant_count": 128, "median": 0}


def chunk(exp=0, total=0, count=128, median=0):
    return {"exp_mismatches": exp, "mant_err_sum": total, "mant_count": count, "median": median}


def sample(case, chunks, outcome=None, reason=None):
    """A sample as `ac-auditor recheck` reports it, judged under version 2 unless given."""
    x = {"case": case, "chunks": chunks, "thresholds_version": 2, "outcome": "pass", "reason": ""}
    if outcome is None:
        x["outcome"], x["reason"] = cal.replay(x, cal.THRESHOLDS_V2)
    else:
        x["outcome"], x["reason"] = outcome, reason
    return x


def gpu_fp(name):
    return {"cpu": {"model": "Xeon", "flags": [], "onednn_max_cpu_isa": ""},
            "gpu": {"name": name, "capability": [8, 6]}, "vllm": "0.30.0"}


def cpu_fp(amx, isa):
    return {"cpu": {"model": "Xeon 8480", "flags": ["amx_bf16"] if amx else ["avx512_bf16"],
                    "onednn_max_cpu_isa": isa}, "gpu": None}


def shard(seed, prover, auditor, samples):
    return {"seed": seed, "prover": prover, "auditor": auditor, "host": auditor["cpu"],
            "thresholds_versions": [2], "samples": samples}


def merged(shards, *extra):
    with tempfile.TemporaryDirectory() as d:
        d = pathlib.Path(d)
        files = []
        for i, s in enumerate(shards):
            f = d / f"shard{i}.json"
            f.write_text(json.dumps(s))
            files.append(f)
        summary_out = d / "summary.json"
        with contextlib.redirect_stdout(io.StringIO()):
            code = cal.merge(files, d / "out", *extra, summary_out=summary_out)
        if code:
            return code, None, None
        return code, json.loads((d / "out" / "calibration.json").read_text()), summary_out.read_text()


class FingerprintTests(unittest.TestCase):
    """Design D2: the cell key of a fingerprint."""

    def test_gpu_is_its_model(self):
        self.assertEqual(cal.cell_key(gpu_fp("NVIDIA GeForce RTX 3090")), "NVIDIA GeForce RTX 3090")

    def test_amx_on_and_off(self):
        # As the plugin reads ONEDNN_MAX_CPU_ISA: unset, ALL or an AMX level allow AMX.
        for isa, on in (("", True), ("ALL", True), ("AVX512_CORE_AMX", True),
                        ("AVX512_CORE_BF16", False), ("AVX2", False)):
            self.assertEqual(cal.cell_key(cpu_fp(True, isa)), f"CPU Xeon 8480 AMX {'on' if on else 'off'}", isa)
        self.assertEqual(cal.cell_key(cpu_fp(False, "")), "CPU Xeon 8480 AMX off")

    def test_old_reports(self):
        # A shard written before fingerprints: its host is both sides.
        self.assertEqual(cal.cell_key({"cpu": None}), "CPU ? AMX off")
        self.assertEqual(cal.cell_key(None), "CPU ? AMX off")

    def test_fingerprint_without_gpu(self):
        fp = cal.fingerprint()
        self.assertIn("cpu", fp)
        self.assertIn("model", fp)
        self.assertTrue(cal.cell_key(fp))


class BundleTests(unittest.TestCase):
    """Design D1: the case bundle and its manifest."""

    def bundle(self, d):
        b = pathlib.Path(d)
        (b / "cases").mkdir()
        for n in ("honest-00000.json", "honest-00001.json", "swap-00000.json"):
            (b / "cases" / n).write_text(json.dumps({"case": n}))
        (b / "prover.json").write_text(json.dumps({"seed": 5, "fingerprint": gpu_fp("A100")}))
        cal.write_manifest(b)
        return b

    def test_an_untouched_bundle(self):
        with tempfile.TemporaryDirectory() as d:
            b = self.bundle(d)
            self.assertEqual(cal.check_manifest(b), {"missing": [], "mismatch": [], "unlisted": []})

    # Scenario "案例包被改动": a changed case is flagged and still re-checked (the re-check, in
    # services/auditor/tests/recheck.rs `a_changed_case_fails`, fails it); a missing one fails
    # the re-check before any engine starts.
    def test_changed_missing_and_unlisted_cases(self):
        with tempfile.TemporaryDirectory() as d:
            b = self.bundle(d)
            (b / "cases" / "swap-00000.json").write_text('{"case": "changed"}')
            (b / "cases" / "int8-00000.json").write_text("{}")
            found = cal.check_manifest(b)
            self.assertEqual(found["mismatch"], ["swap-00000.json"])
            self.assertEqual(found["unlisted"], ["int8-00000.json"])
            (b / "cases" / "honest-00001.json").unlink()
            self.assertEqual(cal.check_manifest(b)["missing"], ["honest-00001.json"])
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertIsNone(cal.recheck_bundle(b, b / "out"))

    def test_shards_partition_the_cases(self):
        names = [f"honest-{i:05d}.json" for i in range(23)]
        parts = [cal.shard_of(names, f"{k}/10") for k in range(10)]
        self.assertEqual(sorted(n for p in parts for n in p), names)
        self.assertEqual(cal.shard_of(names, None), names)
        with self.assertRaises(ValueError):
            cal.shard_of(names, "10/10")


class CompareTests(unittest.TestCase):
    """1.6: the split run against the one-machine run of the same bundle."""

    def test_same_and_different_outcomes(self):
        a = {"samples": [sample("honest-0.json", [EXACT]), sample("swap-0.json", [chunk(exp=40)])]}
        self.assertEqual(cal.same_outcomes(a, json.loads(json.dumps(a))), [])
        b = {"samples": [sample("honest-0.json", [chunk(exp=3)]), a["samples"][1]]}
        self.assertEqual(cal.same_outcomes(a, b), ["honest-0.json"])
        self.assertEqual(cal.same_outcomes(a, {"samples": a["samples"][:1]}), ["swap-0.json"])


class MergeTests(unittest.TestCase):
    """Design D1, D6: cells, fingerprints and the condensed report."""

    # Scenario "跨机器复核": a bundle generated on A and re-checked on B lands in cell A → B,
    # with both fingerprints in the report.
    def test_cross_machine_cell(self):
        a, b = gpu_fp("NVIDIA GeForce RTX 3090"), gpu_fp("NVIDIA A100-SXM4-80GB")
        samples = [sample("honest-00000.json", [EXACT, EXACT]),
                   sample("honest-00001.json", [chunk(exp=1, total=10), EXACT]),
                   sample("swap-00000.json", [chunk(exp=60), EXACT])]
        code, report, condensed = merged([shard(5, a, b, samples), shard(5, a, a, samples)])
        self.assertEqual(code, 0)
        cell = report["cells"]["NVIDIA GeForce RTX 3090 → NVIDIA A100-SXM4-80GB"]
        self.assertEqual(cell["honest"], {"samples": 2, "pass": 2, "fail": 0, "inconclusive": 0, "inexact_prefill": 1})
        self.assertEqual(cell["cheats"]["swap"], {"samples": 1, "missed": 0, "inconclusive": 0})
        self.assertIn("NVIDIA GeForce RTX 3090 → NVIDIA GeForce RTX 3090", report["cells"])
        self.assertEqual(report["fingerprints"]["NVIDIA A100-SXM4-80GB"], [b])
        self.assertEqual(report["fingerprints"]["NVIDIA GeForce RTX 3090"], [a])
        # A handful of samples: the conclusion would keep the thresholds but needs more.
        self.assertEqual(report["conclusion"]["decision"], "too few samples")
        self.assertEqual(report["conclusion"]["would_be"], "keep")
        # Samples without prompt token counts are on neither side of the band.
        self.assertEqual(list(cell["by_band"]), ["unknown"])
        # The condensed report: numbers and case names, never request content.
        kept = json.loads(condensed)
        self.assertNotIn("samples", kept)
        self.assertEqual(kept["unexpected"], [])
        self.assertNotIn('"messages"', condensed)
        self.assertNotIn('"output"', condensed)

    def test_old_shards_keep_their_host(self):
        old = {"seed": 401, "host": {"model": "AMD EPYC 7763", "flags": ["avx2"], "onednn_max_cpu_isa": ""},
               "thresholds_versions": [2], "samples": [sample("honest-00000.json", [EXACT])]}
        code, report, _ = merged([old])
        self.assertEqual(code, 0)
        self.assertEqual(list(report["cells"]), ["CPU AMD EPYC 7763 AMX off → CPU AMD EPYC 7763 AMX off"])
        self.assertEqual(len(report["honest_by_host"]), 1)

    def test_a_sample_twice_in_a_cell_is_refused(self):
        s = shard(5, gpu_fp("A"), gpu_fp("B"), [sample("honest-00000.json", [EXACT])])
        code, _, _ = merged([s, s])
        self.assertEqual(code, 1)

    def test_unexpected_samples_are_kept(self):
        bad = sample("honest-00002.json", [chunk(exp=3), EXACT])
        missed = sample("int4-00000.json", [EXACT])
        code, _, condensed = merged([shard(5, gpu_fp("A"), gpu_fp("B"), [bad, missed])])
        self.assertEqual(code, 0)
        kept = json.loads(condensed)
        self.assertEqual(sorted(u["case"] for u in kept["unexpected"]), ["honest-00002.json", "int4-00000.json"])
        self.assertEqual(kept["unexpected"][0]["chunks"], bad["chunks"])


class TokenMismatchTests(unittest.TestCase):
    """m6-toploc-gpu-calibration 11.1 (design D15): honest answers whose text re-tokenizes to other
    token IDs are listed apart (issue I-023)."""

    # Spec "token ID 不一致的诚实回答单列".
    def test_listed_apart_and_out_of_the_conclusion(self):
        bad = dict(sample("honest-00002.json", [EXACT, chunk(exp=83, total=3500, median=25)]),
                   prompt_tokens=200, same_token_ids=False)
        good = dict(sample("honest-00003.json", [EXACT, EXACT]), prompt_tokens=200, same_token_ids=True)
        cheats = [dict(sample(f"{v}-00000.json", [chunk(exp=90, total=900, median=9)]), prompt_tokens=200)
                  for v in cal.CHEATS]
        code, report, condensed = merged([shard(5, gpu_fp("A"), gpu_fp("B"), [bad, good] + cheats)])
        self.assertEqual(code, 0)
        self.assertEqual([x["case"] for x in report["token_mismatch"]], ["honest-00002.json"])
        self.assertEqual(report["cells"]["A → B"]["honest"]["samples"], 1)
        self.assertEqual(report["cells"]["A → B"]["honest"]["fail"], 0)
        self.assertNotIn("honest", " ".join(report["conclusion"].get("broken", [])))
        self.assertIn("token_mismatch", json.loads(condensed))
        # The same answer without the generation's record counts as an honest failure.
        unknown = {k: v for k, v in bad.items() if k != "same_token_ids"}
        self.assertFalse(cal.token_mismatch(unknown))
        self.assertTrue(cal.token_mismatch(bad))
        # A cheat is never excused this way.
        self.assertFalse(cal.token_mismatch(dict(cheats[0], same_token_ids=False)))

    def test_the_quick_regression_excuses_them(self):
        s = {"honest": {"outcomes": {"pass": 47, "fail": 1, "inconclusive": 0}}}
        for v in cal.CHEATS:
            s[v] = {"outcomes": {"pass": 0, "fail": 8, "inconclusive": 0}}
        bad = {"case": "honest-00007.json", "outcome": "fail", "same_token_ids": False}
        self.assertEqual(cal.quick_failures(s, {}, 8, [bad]), [])
        self.assertTrue(cal.quick_failures(s, {}, 8, [dict(bad, same_token_ids=True)]))


class ReplayTests(unittest.TestCase):
    """Design D5: the thresholds replayed in Python, as `ac_market_proto::toploc::judge`."""

    # m6-toploc-gpu-calibration 4.3: the current version, band included, is the crate's.
    def test_the_current_version_is_the_crate_constant(self):
        text = (ROOT / "crates" / "ac-market-proto" / "src" / "toploc.rs").read_text()
        block = text[text.index("pub const AUDIT_THRESHOLDS"):]
        block = block[: block.index("\n};") + 3]
        version = int(re.search(r"version:\s*(\d+)", block).group(1))
        band = tuple(int(v) for v in re.search(r"band:\s*PromptBand\s*\{\s*min:\s*(\d+),\s*max:\s*(\d+)", block).groups())
        bounds = [tuple(int(v.replace("_", "")) for v in m) for m in re.findall(
            r"exp_mismatches:\s*([\d_]+),\s*mant_mean_centi:\s*([\d_]+),\s*mant_median:\s*([\d_]+)", block)]
        self.assertEqual(version, cal.CURRENT["version"])
        self.assertEqual(band, cal.CURRENT["band"])
        self.assertEqual(bounds, [cal.CURRENT["prefill"], cal.CURRENT["prefill_outside"], cal.CURRENT["decode"]])
        self.assertIs(cal.THRESHOLDS[cal.CURRENT["version"]], cal.CURRENT)

    # Spec market/toploc "区间的界含两端" and "区间外的 prompt 用宽松的预填充阈值", as the crate's
    # test `the_prefill_bounds_depend_on_the_prompt_length`.
    def test_the_band_picks_the_prefill_bounds(self):
        t = cal.THRESHOLDS_V3
        lo, hi = t["band"]
        between = chunk(total=128)  # mean 1.00: over the band's 0.85, under 5.00 outside it
        for n, outcome in ((lo, "fail"), (hi, "fail"), (lo - 1, "pass"), (hi + 1, "pass")):
            x = {"case": "honest-0.json", "chunks": [between, EXACT], "outcome": "pass", "reason": "",
                 "prompt_tokens": n}
            self.assertEqual(cal.replay(x, t)[0], outcome, n)
        self.assertEqual(cal.replay(dict(x, prompt_tokens=lo), t)[1], "chunk 0: mean mantissa error")
        # Without its prompt token count a sample cannot be judged under a banded version.
        with self.assertRaises(ValueError):
            cal.replay({"case": "honest-1.json", "chunks": [EXACT], "outcome": "pass", "reason": ""}, t)
        # Version 2 has no band.
        self.assertEqual(cal.replay({"case": "h.json", "chunks": [between], "outcome": "pass", "reason": ""},
                                    cal.THRESHOLDS_V2)[0], "fail")

    # The boundary cases of the crate's own tests (`a_mean_equal_to_the_bound_passes`,
    # `a_chunk_without_a_matching_exponent_fails`, `one_chunk_with_too_many_exponent_mismatches_fails`)
    # under its bounds (4, 300, 2).
    def test_the_crate_boundaries(self):
        b = (4, 300, 2)
        self.assertIsNone(cal.judge_chunk(chunk(total=3 * 128, median=2), b))
        self.assertEqual(cal.judge_chunk(chunk(total=3 * 128 + 1, median=2), b), "mean")
        self.assertEqual(cal.judge_chunk(chunk(median=3), b), "median")
        self.assertEqual(cal.judge_chunk(chunk(exp=5, count=123), b), "exp")
        self.assertEqual(cal.judge_chunk(chunk(exp=4, count=0, median=None), b), "noexp")
        # Half a unit in hundredths: 64/128 passes a bound of 50, 65/128 does not.
        self.assertIsNone(cal.judge_chunk(chunk(total=64), (0, 50, 0)))
        self.assertEqual(cal.judge_chunk(chunk(total=65), (0, 50, 0)), "mean")

    def test_reasons_are_the_auditors(self):
        x = sample("honest-0.json", [EXACT, chunk(exp=21)])
        self.assertEqual((x["outcome"], x["reason"]), ("fail", "chunk 1: exponent mismatches"))
        # Outcomes the thresholds do not decide are kept.
        fixed = {"case": "swap-0.json", "chunks": [], "outcome": "fail", "reason": "no proof"}
        self.assertEqual(cal.replay(fixed, cal.THRESHOLDS_V2), ("fail", "no proof"))

    def test_a_replay_that_disagrees_fails_the_merge(self):
        wrong = sample("honest-0.json", [chunk(exp=3)], outcome="pass", reason="")
        code, _, _ = merged([shard(5, gpu_fp("A"), gpu_fp("B"), [wrong])])
        self.assertEqual(code, 1)

    def test_other_thresholds(self):
        s = [dict(sample("honest-0.json", [chunk(exp=3), EXACT]), prompt_tokens=200),
             dict(sample("swap-0.json", [chunk(exp=40)]), prompt_tokens=200)]
        code, report, _ = merged([shard(5, gpu_fp("A"), gpu_fp("B"), s)], ["prefill=4,50,1", "band=100,250"])
        self.assertEqual(code, 0)
        under = report["under_thresholds"]
        self.assertEqual(under["thresholds"]["prefill"], [4, 50, 1])
        self.assertEqual(under["thresholds"]["band"], [100, 250])
        self.assertEqual(under["cells"]["A → B"],
                         {"inside": {"honest": 1, "honest_fail": 0, "cheats": {"swap": 1}, "missed": {"swap": 0}}})
        with self.assertRaises(ValueError):
            cal.parse_thresholds(["prefill=1,2"])
        with self.assertRaises(ValueError):
            cal.parse_thresholds(["band=1,2,3"])


class ConclusionTests(unittest.TestCase):
    """The conclusion rules of "GPU 跨硬件校准", per side of the audit length band (version 4:
    150–300 prompt tokens; prefill 20/6.00/5 on both sides, decode 28/12.00/12)."""

    IN, OUT = 200, 100

    def cheats(self, n=IN, int8=None):
        out = [dict(sample(f"{v}-{n}.json", [chunk(exp=90, total=900, median=9)]), prompt_tokens=n)
               for v in cal.CHEATS if v != "int8"]
        out.append(dict(sample(f"int8-{n}.json", [int8 or chunk(exp=90, total=900, median=9)]), prompt_tokens=n))
        return out

    @staticmethod
    def honest(n, *chunks):
        return dict(sample(f"honest-{n}-{len(chunks)}.json", list(chunks)), prompt_tokens=n)

    @staticmethod
    def conclusion(samples, minimum=False):
        return cal.conclusion([dict(x, prover="A", auditor="B") for x in samples], minimum)

    def test_keep(self):
        c = self.conclusion([self.honest(self.IN, EXACT, chunk(exp=5, total=200)),
                             self.honest(self.OUT, chunk(exp=10, total=300))]
                            + self.cheats() + self.cheats(self.OUT))
        self.assertEqual(c["decision"], "keep")
        self.assertTrue(c["minimal_holds"])

    # Scenario "报告按区间内外分开": per cell both sides, and the minimal thresholds per side.
    def test_the_report_has_both_sides(self):
        samples = [dict(x, prover="A", auditor="B") for x in
                   [self.honest(self.IN, chunk(exp=1, total=64), EXACT), self.honest(self.OUT, chunk(exp=9, total=320))]
                   + self.cheats() + self.cheats(self.OUT)]
        cell = cal.cells(samples)["A → B"]
        self.assertEqual(sorted(cell["by_band"]), ["inside", "outside"])
        self.assertEqual(cell["by_band"]["outside"]["honest"]["samples"], 1)
        self.assertEqual(cell["by_band"]["inside"]["cheats"]["int8"]["samples"], 1)
        low = cal.minimal_thresholds(samples)
        self.assertEqual((low["prefill"], low["prefill_outside"]), ((1, 50, 0), (9, 250, 0)))
        r = cal.under(samples, cal.CURRENT)["A → B"]
        self.assertEqual((r["inside"]["honest"], r["outside"]["honest"]), (1, 1))

    # Scenario "int8 不计入单次判定的漏检": int8 answers pass on both sides; still satisfied, and
    # their passes are reported.
    def test_int8_is_not_a_miss(self):
        passing = chunk(exp=1, total=64)  # mean 0.50: within the prefill bounds on both sides
        c = self.conclusion([self.honest(self.IN, EXACT), self.honest(self.OUT, EXACT)]
                            + self.cheats(int8=passing) + self.cheats(self.OUT, int8=passing))
        self.assertEqual(c["decision"], "keep")
        self.assertEqual(c["cells"]["A → B"]["inside"]["missed"]["int8"], 1)
        self.assertEqual(c["cells"]["A → B"]["outside"]["missed"]["int8"], 1)

    # Scenario "报告按区间内外分开": the statistics a verdict would carry, as histograms per cell,
    # side and variant.
    def test_statistics_per_side(self):
        xs = [dict(x, prover="A", auditor="B") for x in
              [self.honest(self.IN, chunk(total=64), chunk(total=128), chunk(total=256)),
               self.honest(self.OUT, chunk(total=64)), dict(self.cheats()[-1])]]
        st = cal.statistics(xs)["A → B"]
        self.assertEqual(st["inside"]["honest"], {"samples": 1, "prefill": {"50": 1}, "decode": {"150": 1},
                                                  "pairs": {"50,150": 1}})
        self.assertEqual(st["outside"]["honest"], {"samples": 1, "prefill": {"50": 1}, "decode": {},
                                                   "pairs": {"50,-": 1}})
        self.assertEqual(st["inside"]["int8"]["samples"], 1)
        self.assertEqual(cal.audit_stats(xs[0]), {"prompt_tokens": self.IN, "prefill": 50, "decode": 150,
                                                  "decode_chunks": 2})
        self.assertEqual(cal.centi(chunk(exp=128, count=0, median=None)), cal.STAT_MAX)

    # Honest prefill errors above the current values: widened to the honest maximum, still
    # catching every cheat a single audit must.
    def test_widen(self):
        c = self.conclusion([self.honest(self.IN, chunk(exp=25, total=100), EXACT)] + self.cheats())
        self.assertEqual(c["decision"], "widen")
        self.assertEqual(c["thresholds"]["prefill"], (25, 600, 5))
        self.assertEqual(c["thresholds"]["band"], cal.CURRENT["band"])
        self.assertEqual(c["current"]["A → B"]["inside"]["honest_fail"], 1)

    # Scenario "无法满足": the honest worst case is as bad as a cheat; the report names the cell
    # and the side.
    def test_no_thresholds(self):
        honest = self.honest(self.IN, chunk(exp=90, total=900, median=9))
        c = self.conclusion([honest] + self.cheats())
        self.assertEqual(c["decision"], "no thresholds")
        self.assertIn("A → B inside: 1 swap missed", c["broken"])
        self.assertFalse(c["minimal_holds"])
        c = self.conclusion([self.honest(self.IN, chunk(exp=128, count=0, median=None))])
        self.assertEqual(c["decision"], "no thresholds")

    def test_too_few_samples(self):
        c = self.conclusion([self.honest(self.IN, EXACT)] + self.cheats(), minimum=True)
        self.assertEqual((c["decision"], c["would_be"]), ("too few samples", "keep"))
        self.assertIn("A → B inside: 1 honest of 3000", c["short"])
        self.assertIn("A → B outside: 0 honest of 500", c["short"])


class PromptLengthTests(unittest.TestCase):
    """The long-prompt experiment: prompts lengthened to a minimum, statistics by prompt length."""

    def test_min_words_keeps_the_seed_otherwise(self):
        plain = cal.prompt_set("honest", 48, 1)
        self.assertEqual(plain, cal.prompt_set("honest", 48, 1, 0))
        long = cal.prompt_set("honest", 48, 1, 100)
        self.assertGreaterEqual(min(len(m[-1]["content"].split()) for m, _ in long), 100)
        # The same tasks and answer lengths, only lengthened.
        self.assertEqual([t for _, t in long], [t for _, t in plain])
        for (lm, _), (pm, _) in zip(long, plain):
            self.assertTrue(lm[-1]["content"].endswith(pm[-1]["content"]))
        self.assertEqual(long, cal.prompt_set("honest", 48, 1, 100))

    def test_targets_cover_both_sides_of_the_band(self):
        # m6-toploc-gpu-calibration 7.1 (design D10): about 80% in the band, 10% shorter, 10%
        # longer, the same for the same seed and variant.
        band = (150, 300)
        t = cal.prompt_targets("honest", 10_000, 4, band)
        self.assertEqual(t, cal.prompt_targets("honest", 10_000, 4, band))
        self.assertNotEqual(t, cal.prompt_targets("honest", 10_000, 5, band))
        self.assertNotEqual(t, cal.prompt_targets("int8", 10_000, 4, band))
        inside = sum(band[0] <= x <= band[1] - cal.SENTENCE for x in t)
        shorter = sum(cal.SHORTEST <= x <= band[0] - cal.SENTENCE for x in t)
        longer = sum(band[1] < x <= cal.LONGEST for x in t)
        self.assertEqual(inside + shorter + longer, len(t))
        self.assertTrue(7_700 <= inside <= 8_300, inside)
        self.assertTrue(800 <= shorter <= 1_200 and 800 <= longer <= 1_200, (shorter, longer))

    def test_prompts_are_fitted_to_their_targets(self):
        # A token per word, as the fake count; a sentence adds at most SENTENCE tokens.
        def count(m):
            return sum(len(x["content"].split()) for x in m) + 3 * len(m)

        band = (150, 300)
        prompts = cal.prompt_set("honest", 200, 2, preamble=False)
        targets = cal.prompt_targets("honest", 200, 2, band)
        for i, ((m, _), target) in enumerate(zip(prompts, targets)):
            fitted = cal.fit_prompt(m, target, count, cal.random.Random(i))
            n = count(fitted)
            if count(m) < target:
                self.assertTrue(target <= n < target + cal.SENTENCE, (target, n))
            else:
                self.assertEqual(fitted, m)
            # The question and its marker stay at the end; the input is not changed.
            self.assertTrue(fitted[-1]["content"].endswith(m[-1]["content"]))
            self.assertIn(cal.MARKER, fitted[-1]["content"])
            self.assertEqual(band[0] <= n <= band[1], band[0] <= target <= band[1])
        again = cal.fit_prompt(prompts[0][0], 250, count, cal.random.Random(0))
        self.assertEqual(again, cal.fit_prompt(prompts[0][0], 250, count, cal.random.Random(0)))

    def test_a_stale_auditor_is_refused(self):
        # A binary built before a pull: without `thresholds`, or with other thresholds.
        with tempfile.TemporaryDirectory() as d:
            fake = pathlib.Path(d) / "ac-auditor"
            saved = cal.AUDITOR
            try:
                cal.AUDITOR = fake
                current = {k: list(v) if isinstance(v, tuple) else v for k, v in cal.CURRENT.items()}
                for printed, stale in ((current, False), (dict(current, prefill=[2, 50, 1]), True),
                                       (dict(current, version=2), True)):
                    fake.write_text(f"#!/bin/sh\necho '{json.dumps(printed)}'\n")
                    fake.chmod(0o755)
                    self.assertEqual(cal.stale_auditor() is not None, stale, printed)
                fake.write_text("#!/bin/sh\necho 'unrecognized subcommand' >&2\nexit 2\n")
                self.assertIn("cannot print its thresholds", cal.stale_auditor())
            finally:
                cal.AUDITOR = saved

    def test_buckets(self):
        samples = [dict(sample("honest-0.json", [chunk(total=128)]), prompt_tokens=20, prover="A", auditor="B"),
                   dict(sample("honest-1.json", [chunk(total=64)]), prompt_tokens=300, prover="A", auditor="B"),
                   dict(sample("int8-0.json", [chunk(exp=3, total=200)]), prompt_tokens=150, prover="A", auditor="B"),
                   dict(sample("honest-2.json", [EXACT]), prover="A", auditor="B")]
        got = cal.by_prompt_length(samples)["A → B"]
        self.assertEqual(sorted(got), ["0-63", "128-255", ">=256"])
        self.assertEqual(got["0-63"]["honest"]["prefill_mean_max"], 1.0)
        self.assertEqual(got[">=256"]["honest"]["prefill_mean_max"], 0.5)
        self.assertEqual(got["128-255"]["int8"], {"samples": 1, "prefill_mean_min": 1.562, "prefill_mean_median": 1.562,
                                                  "prefill_mean_max": 1.562, "prefill_exp_max": 3})


class QuickTests(unittest.TestCase):
    """m6-toploc-async-stop 3.2: the CI regression's verdict, with the prover's answers without
    proof."""

    def summary(self, honest=None, cheat=None):
        s = {"honest": {"outcomes": honest or {"pass": 48, "fail": 0, "inconclusive": 0}}}
        for v in cal.CHEATS:
            s[v] = {"outcomes": cheat or {"pass": 0, "fail": 8, "inconclusive": 0}}
        return s

    def test_a_clean_run_passes(self):
        self.assertEqual(cal.quick_failures(self.summary(), {}, 8), [])
        self.assertEqual(cal.quick_failures(self.summary(), {"honest": {}}, 8), [])

    def test_honest_answers_without_proof_fail_it(self):
        reasons = {"the segments do not fit the usage: 11 decode segments where the output calls for 9": 1}
        failures = cal.quick_failures(self.summary(), {"honest": reasons}, 8)
        self.assertEqual(len(failures), 1)
        self.assertIn("without proof", failures[0])
        # A cheat the prover could not prove is only counted (it is not re-checked).
        self.assertEqual(cal.quick_failures(self.summary(), {"swap": reasons}, 8), [])

    def test_outcomes(self):
        self.assertTrue(cal.quick_failures(self.summary(honest={"pass": 47, "fail": 1}), {}, 8))
        self.assertTrue(cal.quick_failures(self.summary(honest={"pass": 46, "inconclusive": 2}), {}, 8))
        self.assertFalse(cal.quick_failures(self.summary(honest={"pass": 47, "inconclusive": 1}), {}, 8))
        self.assertTrue(cal.quick_failures(self.summary(cheat={"pass": 1, "fail": 7}), {}, 8))

    def test_int8_may_pass(self):
        # 2026-10-09 (design D13): a single audit is not required to catch int8, in or out of the band.
        s = self.summary()
        s["int8"]["outcomes"] = {"pass": 5, "fail": 3, "inconclusive": 0}
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(cal.quick_failures(s, {}, 8), [])
        s["swap"]["outcomes"] = {"pass": 1, "fail": 7, "inconclusive": 0}
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertTrue(cal.quick_failures(s, {}, 8))


class WorkflowTests(unittest.TestCase):
    """2.1: the calibration workflow and plugins/vllm/ci/calibration.env agree."""

    def test_variables_and_shards(self):
        env = (ROOT / "plugins" / "vllm" / "ci" / "calibration.env").read_text()
        workflow = (ROOT / ".github" / "workflows" / "toploc-calibration.yml").read_text()
        script = (ROOT / "scripts" / "calibrate-toploc.py").read_text()
        defined = set(re.findall(r"^(CALIBRATION_[A-Z_]+)=", env, re.M))
        used = set(re.findall(r"\b(CALIBRATION_[A-Z_]+)\b", workflow))
        self.assertLessEqual(used, defined, "the workflow uses variables calibration.env lacks")
        for name in defined:
            self.assertTrue(name in workflow or name in script, f"{name} is used nowhere")
        for name in ("CALIBRATION_BUNDLE_RELEASE", "CALIBRATION_BUNDLE_FILE", "CALIBRATION_BUNDLE_SHA256"):
            self.assertIn(name, workflow)
        self.assertIn("--recheck-only", workflow)
        self.assertIn("sha256sum -c", workflow)
        self.assertIn("CALIBRATION_PROVER_ISA", script)
        shards = int(re.search(r"^CALIBRATION_SHARDS=(\d+)", env, re.M).group(1))
        matrix = re.search(r"shard:\s*\[([^\]]*)\]", workflow).group(1)
        self.assertEqual([int(v) for v in matrix.split(",")], list(range(shards)))


if __name__ == "__main__":
    unittest.main()
