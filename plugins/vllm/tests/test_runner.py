"""Preconditions and the runner wrappers, with stand-in vLLM objects (the real vLLM is exercised
by scripts/check-vllm-plugin.py in the vllm-plugin CI job)."""

from types import SimpleNamespace

import pytest
import torch

from agentcoin_vllm import runner


def configs(**over):
    c = {
        "model_config": SimpleNamespace(dtype=torch.bfloat16),
        "cache_config": SimpleNamespace(enable_prefix_caching=False),
        "parallel_config": SimpleNamespace(pipeline_parallel_size=1),
        "speculative_config": None,
    }
    c.update(over)
    return c


def test_supported_versions():
    runner.check_version("0.30.0")
    runner.check_version("0.30.2+cpu")
    for bad in ("0.29.9", "0.31.0", "1.0.0", "dev"):
        with pytest.raises(runner.PreconditionError):
            runner.check_version(bad)


def test_a_good_configuration_passes():
    runner.check_config(**configs())


# Scenarios "前缀缓存开启时拒绝启动" and "非 bfloat16 时拒绝启动", and the other preconditions.
@pytest.mark.parametrize(
    "override, message",
    [
        ({"model_config": SimpleNamespace(dtype=torch.float16)}, "bfloat16"),
        ({"cache_config": SimpleNamespace(enable_prefix_caching=True)}, "--no-enable-prefix-caching"),
        ({"speculative_config": object()}, "speculative decoding"),
        ({"parallel_config": SimpleNamespace(pipeline_parallel_size=2)}, "pipeline parallelism"),
    ],
)
def test_unmet_preconditions_stop_the_engine(override, message):
    with pytest.raises(runner.PreconditionError, match=message):
        runner.check_config(**configs(**override))


class FakeCapture:
    def __init__(self, model_runner, path):
        self.calls = []
        self.failures = 0

    def before(self, scheduler_output):
        self.calls.append(("before", scheduler_output))

    def after(self, model_runner, scheduler_output):
        self.calls.append(("after", scheduler_output))
        if scheduler_output == "boom":
            raise RuntimeError("capture failed")


def fake_runner_class(**cfg):
    class Runner:
        def __init__(self):
            for k, v in configs(**cfg).items():
                setattr(self, k, v)
            self.loaded = False

        def load_model(self):
            self.loaded = True

        def execute_model(self, scheduler_output):
            return f"ran {scheduler_output}"

    return Runner


def test_wrappers_check_capture_and_never_break_inference(monkeypatch):
    monkeypatch.setattr(runner, "Capture", FakeCapture)
    Runner = fake_runner_class()
    runner._patch(Runner, "/tmp/x.sock")
    runner._patch(Runner, "/tmp/x.sock")  # re-entrant
    r = Runner()
    # Before load_model there is no capture: execution is untouched.
    assert r.execute_model("s0") == "ran s0"
    r.load_model()
    assert r.loaded
    assert r.execute_model("s1") == "ran s1"
    assert r._agentcoin.calls == [("before", "s1"), ("after", "s1")]
    # A failing capture is counted, not raised.
    assert r.execute_model("boom") == "ran boom"
    assert r._agentcoin.failures == 1


def test_load_model_refuses_bad_configurations(monkeypatch):
    monkeypatch.setattr(runner, "Capture", FakeCapture)
    Runner = fake_runner_class(cache_config=SimpleNamespace(enable_prefix_caching=True))
    runner._patch(Runner, "/tmp/x.sock")
    r = Runner()
    with pytest.raises(runner.PreconditionError):
        r.load_model()
    assert not r.loaded


def test_the_v2_runner_is_refused_by_environment(monkeypatch):
    monkeypatch.setenv(runner.V2_ENV, "1")
    monkeypatch.setattr(runner, "check_version", lambda v: None)
    import sys

    monkeypatch.setitem(sys.modules, "vllm", SimpleNamespace(__version__="0.30.0"))
    with pytest.raises(runner.PreconditionError, match=runner.V2_ENV):
        runner.install("/tmp/x.sock")
