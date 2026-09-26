"""Tests for scripts/sync-audit-exceptions.py (spec engineering/ci-quality-gates)."""

import datetime as dt
import importlib.util
import pathlib
import unittest

SPEC = importlib.util.spec_from_file_location(
    "sync", pathlib.Path(__file__).resolve().parent.parent / "sync-audit-exceptions.py"
)
sync = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(sync)

TODAY = dt.date(2026, 9, 26)


def entry(**overrides):
    base = {
        "id": "RUSTSEC-2026-0001",
        "crate": "demo 1.0.0",
        "path": "a → b → demo",
        "reason": "not reachable",
        "added": "2026-09-26",
        "review_by": "2026-12-20",
    }
    base.update(overrides)
    return base


class ValidateTests(unittest.TestCase):
    def test_valid_entry(self):
        self.assertEqual(sync.validate([entry()], TODAY), [])

    # Scenario "过期的忽略项".
    def test_expired_review_date(self):
        problems = sync.validate([entry(added="2026-06-01", review_by="2026-08-01")], TODAY)
        self.assertTrue(any("has passed" in p for p in problems))

    # Scenario "缺少理由的忽略项".
    def test_missing_reason(self):
        problems = sync.validate([entry(reason="  ")], TODAY)
        self.assertTrue(any("'reason'" in p for p in problems))
        without = entry()
        del without["review_by"]
        self.assertTrue(sync.validate([without], TODAY))

    def test_review_window_limited_to_90_days(self):
        problems = sync.validate([entry(review_by="2027-01-30")], TODAY)
        self.assertTrue(any("90 days" in p for p in problems))

    def test_duplicates_and_bad_ids(self):
        self.assertTrue(sync.validate([entry(), entry()], TODAY))
        self.assertTrue(sync.validate([entry(id="CVE-1")], TODAY))


class ScopeTests(unittest.TestCase):
    def test_audit_only_entries_stay_out_of_deny(self):
        entries = [entry(), entry(id="RUSTSEC-2026-0002", scope="audit")]
        self.assertEqual(sync.validate(entries, TODAY), [])
        block = sync.deny_block(entries)
        self.assertIn("RUSTSEC-2026-0001", block)
        self.assertNotIn("RUSTSEC-2026-0002", block)
        self.assertIn("RUSTSEC-2026-0002", sync.audit_toml(entries))

    def test_unknown_scope(self):
        self.assertTrue(sync.validate([entry(scope="deny")], TODAY))


class RenderTests(unittest.TestCase):
    def test_deny_block_is_replaced_between_markers(self):
        current = f"a = 1\n{sync.BEGIN}\nignore = []\n{sync.END}\nb = 2\n"
        out = sync.render_deny(current, [entry()])
        self.assertIn('id = "RUSTSEC-2026-0001"', out)
        self.assertTrue(out.startswith("a = 1\n") and out.endswith("b = 2\n"))


if __name__ == "__main__":
    unittest.main()
