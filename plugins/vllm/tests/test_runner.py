"""Preconditions and the runner wrappers, with stand-in vLLM objects (the real vLLM is exercised
by scripts/check-vllm-plugin.py in the vllm-plugin CI job)."""

from types import SimpleNamespace

import pytest
import torch

from agentcoin_vllm import client, runner


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


# The re-check on a CPU with AMX needs oneDNN kept from AMX (m6-toploc-verify 8.1).
def test_verify_on_an_amx_cpu_needs_amx_off():
    amx = {"avx512f", "avx512_bf16", "amx_bf16", "amx_tile"}
    for isa in (None, "", "ALL", "AVX512_CORE_AMX"):
        with pytest.raises(runner.PreconditionError, match=runner.ISA_ENV):
            runner.check_cpu_isa(client.VERIFY, "cpu", amx, isa)
    runner.check_cpu_isa(client.VERIFY, "cpu", amx, "AVX512_CORE_BF16")
    # Prove mode, GPUs and CPUs without AMX are not affected.
    runner.check_cpu_isa(client.PROVE, "cpu", amx, None)
    runner.check_cpu_isa(client.VERIFY, "cuda", amx, None)
    runner.check_cpu_isa(client.VERIFY, "cpu", {"avx2", "avx512f"}, None)


class FakeCapture:
    def __init__(self, model_runner, path, mode=0):
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


def test_modes():
    assert runner.parse_mode(None) == client.PROVE
    assert runner.parse_mode("prove") == client.PROVE
    assert runner.parse_mode(" Verify ") == client.VERIFY
    with pytest.raises(runner.PreconditionError, match="AGENTCOIN_TOPLOC_MODE"):
        runner.parse_mode("audit")


class FakeSender:
    topk = 3

    def __init__(self):
        self.items = []

    def connected(self):
        return True

    def put(self, item):
        self.items.append(item)


def capture_in(mode):
    c = object.__new__(runner.Capture)
    c.active, c.mode, c.sender, c.failures = True, mode, FakeSender(), 0
    return c


def step_runner(hidden, req_ids, scheduled, computed, prompts):
    n = len(req_ids)
    model_runner = SimpleNamespace(
        execute_model_state=SimpleNamespace(hidden_states=hidden),
        input_batch=SimpleNamespace(
            req_ids=req_ids,
            num_computed_tokens_cpu=torch.tensor(computed + [0] * 2),
            num_prompt_tokens=torch.tensor(prompts + [0] * 2),
        ),
    )
    out = SimpleNamespace(
        total_num_scheduled_tokens=sum(scheduled),
        num_scheduled_tokens=dict(zip(req_ids, scheduled)),
        finished_req_ids=set(),
    )
    assert n == len(scheduled)
    return model_runner, out


def segments(sender):
    """(request, phase, values, count) of every Segment frame queued."""
    out = []
    for item in sender.items:
        for data in item():
            payload = data[4:]
            assert payload[0] == 0x01
            (idlen,) = __import__("struct").unpack(">H", payload[1:3])
            request = payload[3 : 3 + idlen].decode()
            phase, values, count = __import__("struct").unpack(">BIH", payload[3 + idlen : 10 + idlen])
            out.append((request, phase, values, count))
    return out


# Scenario "每个token一段" through the runner hook: verify mode sends one segment per prefilled
# row and nothing for decode rows; prove mode one segment per request and step.
def test_capture_modes_on_a_mixed_step():
    torch.manual_seed(1)
    hidden = torch.randn(7, 8, dtype=torch.bfloat16)
    # r1 decodes one token, r2 prefills 4 prompt tokens, r3 prefills its last 2.
    args = (["r1", "r2", "r3"], [1, 4, 2], [5, 0, 6], [5, 10, 8])
    verify = capture_in(client.VERIFY)
    verify.after(*step_runner(hidden, *args))
    assert segments(verify.sender) == [("r2", client.PREFILL, 8, 3)] * 4 + [("r3", client.PREFILL, 8, 3)] * 2
    prove = capture_in(client.PROVE)
    prove.after(*step_runner(hidden, *args))
    assert segments(prove.sender) == [
        ("r1", client.DECODE, 8, 3),
        ("r2", client.PREFILL, 32, 3),
        ("r3", client.PREFILL, 16, 3),
    ]
