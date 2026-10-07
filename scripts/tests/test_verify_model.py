"""Tests for scripts/verify-model.py (m6-toploc-gpu-calibration 3.1)."""

import importlib.util
import pathlib
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location(
    "verify_model", pathlib.Path(__file__).resolve().parent.parent / "verify-model.py"
)
vm = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(vm)


class DigestTests(unittest.TestCase):
    def tree(self, d, files):
        root = pathlib.Path(d)
        for name, data in files.items():
            (root / name).parent.mkdir(parents=True, exist_ok=True)
            (root / name).write_bytes(data)
        return root

    def test_content_and_names_count(self):
        with tempfile.TemporaryDirectory() as a, tempfile.TemporaryDirectory() as b:
            files = {"config.json": b"{}", "model.safetensors": b"weights", "sub/tok.json": b"t"}
            base = vm.digest(self.tree(a, files))
            self.assertEqual(base, vm.digest(self.tree(b, files)))
            self.assertEqual(len(base), 64)
            (pathlib.Path(b) / "model.safetensors").write_bytes(b"weightz")
            self.assertNotEqual(base, vm.digest(pathlib.Path(b)))
        with tempfile.TemporaryDirectory() as c:
            renamed = {"config.json": b"{}", "model2.safetensors": b"weights", "sub/tok.json": b"t"}
            self.assertNotEqual(base, vm.digest(self.tree(c, renamed)))


if __name__ == "__main__":
    unittest.main()
