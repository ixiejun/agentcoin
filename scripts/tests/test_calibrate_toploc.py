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
        self.assertEqual(report["conclusion"]["decision"], "keep")
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


class ReplayTests(unittest.TestCase):
    """Design D5: the thresholds replayed in Python, as `ac_market_proto::toploc::judge`."""

    # m6-toploc-gpu-calibration 4.3: the current version, band included, is the crate's.
    def test_the_current_version_is_the_crate_constant(self):
        text = (ROOT / "crates" / "ac-market-proto" / "src" / "toploc.rs").read_text()
        block = text[text.index("pub const AUDIT_THRESHOLDS"):]
        block = block[: block.index("\n};") + 3]
        version = int(re.search(r"version:\s*(\d+)", block).group(1))
        band = tuple(int(v) for v in re.search(r"band:\s*PromptBand\s*\{\s*min:\s*(\d+),\s*max:\s*(\d+)", block).groups())
        bounds = [tuple(int(v) for v in m) for m in re.findall(
            r"exp_mismatches:\s*(\d+),\s*mant_mean_centi:\s*(\d+),\s*mant_median:\s*(\d+)", block)]
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
        self.assertEqual(under["cells"]["A → B"], {"honest_fail": 0, "missed": {"swap": 0}})
        with self.assertRaises(ValueError):
            cal.parse_thresholds(["prefill=1,2"])
        with self.assertRaises(ValueError):
            cal.parse_thresholds(["band=1,2,3"])


class ConclusionTests(unittest.TestCase):
    """The conclusion rules of "GPU 跨硬件校准"."""

    def cheats(self):
        return [sample(f"{v}-0.json", [chunk(exp=90, total=900, median=9)]) for v in cal.CHEATS]

    @staticmethod
    def conclusion(samples):
        return cal.conclusion([dict(x, prover="A", auditor="B") for x in samples])

    # Scenario "现行阈值在所有格子成立".
    def test_keep(self):
        c = self.conclusion([sample("honest-0.json", [EXACT, chunk(exp=5, total=200)])] + self.cheats())
        self.assertEqual(c["decision"], "keep")

    # Scenario "需要统一放宽阈值": an honest prefill with 3 exponent mismatches; version 2 widened
    # to 3 still fails every cheat.
    def test_new_version(self):
        honest = sample("honest-0.json", [chunk(exp=3, total=10), EXACT])
        c = self.conclusion([honest] + self.cheats())
        self.assertEqual(c["decision"], "new version")
        self.assertEqual(c["thresholds"]["prefill"], (3, 50, 1))
        self.assertEqual(c["thresholds"]["decode"], (20, 800, 8))
        self.assertEqual(c["current"]["A → B"]["honest_fail"], 1)
        self.assertEqual(c["cells"]["A → B"]["honest_fail"], 0)

    # Scenario "统一阈值无法满足": the honest worst case is as bad as a cheat.
    def test_no_uniform_thresholds(self):
        honest = sample("honest-0.json", [chunk(exp=90, total=900, median=9)])
        c = self.conclusion([honest] + self.cheats())
        self.assertEqual(c["decision"], "no uniform thresholds")
        missed = sum(sum(o["missed"].values()) for o in c["cells"].values())
        self.assertEqual(missed, len(cal.CHEATS))
        # An honest chunk without a matching exponent: no bound passes it.
        c = self.conclusion([sample("honest-1.json", [chunk(exp=128, count=0, median=None)])])
        self.assertEqual(c["decision"], "no uniform thresholds")


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
