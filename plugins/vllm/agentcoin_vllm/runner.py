"""Hooks into vLLM's V1 model runner (vLLM >=0.30,<0.31; design D2 of m5-engine-toploc).

`load_model` is wrapped to check the preconditions (spec "加载前提") so that the engine fails
to start when they do not hold. `execute_model` is wrapped to read the step's final hidden
states (the output of the model's final norm, as the reference implementation reads it) once
the forward pass has been launched, take each request's top-k on the device and hand them to
the background sender; the inference thread never waits for I/O.
"""

from __future__ import annotations

import logging
import os
import re

from . import client, extract

SUPPORTED = ((0, 30, 0), (0, 31, 0))  # [min, max)
V2_ENV = "VLLM_USE_V2_MODEL_RUNNER"

log = logging.getLogger("agentcoin_vllm")


class PreconditionError(RuntimeError):
    """The engine configuration cannot produce TOPLOC proofs."""


def parse_mode(text: str | None) -> int:
    """The plugin mode named by `AGENTCOIN_TOPLOC_MODE` (unset: prove)."""
    value = (text or "prove").strip().lower()
    for mode, name in client.MODE_NAMES.items():
        if value == name:
            return mode
    raise PreconditionError(
        f"agentcoin toploc: AGENTCOIN_TOPLOC_MODE={text!r} is not a mode; use prove (providers) "
        "or verify (auditors)"
    )


def parse_version(text: str) -> tuple[int, int, int]:
    m = re.match(r"(\d+)\.(\d+)\.(\d+)", text)
    if not m:
        raise PreconditionError(f"agentcoin toploc: cannot parse the vLLM version {text!r}")
    return tuple(int(x) for x in m.groups())


def check_version(text: str) -> None:
    lo, hi = SUPPORTED
    if not lo <= parse_version(text) < hi:
        raise PreconditionError(
            f"agentcoin toploc: vLLM {text} is not supported; this plugin needs "
            f">={'.'.join(map(str, lo))},<{'.'.join(map(str, hi))}"
        )


def check_config(model_config, cache_config, parallel_config, speculative_config) -> None:
    """Raises `PreconditionError` naming the first unmet precondition."""
    import torch

    if model_config.dtype != torch.bfloat16:
        raise PreconditionError(
            f"agentcoin toploc: the model runs in {model_config.dtype}; TOPLOC proofs need "
            "bfloat16 (start vLLM with --dtype bfloat16)"
        )
    if cache_config.enable_prefix_caching:
        raise PreconditionError(
            "agentcoin toploc: prefix caching skips the prompt tokens it caches; start vLLM "
            "with --no-enable-prefix-caching"
        )
    if speculative_config is not None:
        raise PreconditionError(
            "agentcoin toploc: speculative decoding is not supported; start vLLM without "
            "--speculative-config"
        )
    if parallel_config.pipeline_parallel_size != 1:
        raise PreconditionError(
            "agentcoin toploc: pipeline parallelism is not supported; use "
            "--pipeline-parallel-size 1"
        )


class Capture:
    """Per-runner state: the sender, the mode and whether this rank sends."""

    def __init__(self, runner, path: str, mode: int = client.PROVE) -> None:
        from vllm.distributed import get_tensor_model_parallel_rank

        # The final hidden states are the same on every tensor-parallel rank.
        self.active = get_tensor_model_parallel_rank() == 0
        hidden = runner.model_config.get_hidden_size()
        self.mode = mode
        self.sender = client.Sender(path, hidden, mode) if self.active else None
        self.failures = 0

    def before(self, scheduler_output) -> None:
        if not self.active:
            return
        for request in getattr(scheduler_output, "finished_req_ids", ()) or ():
            data = client.finish(request)
            self.sender.put(lambda data=data: [data])

    def after(self, runner, scheduler_output) -> None:
        if not self.active or not self.sender.connected():
            return
        state = runner.execute_model_state
        total = scheduler_output.total_num_scheduled_tokens
        if state is None or total == 0:
            return
        batch = runner.input_batch
        req_ids = list(batch.req_ids)
        n = len(req_ids)
        step = extract.spans(
            req_ids,
            [scheduler_output.num_scheduled_tokens.get(r, 0) for r in req_ids],
            batch.num_computed_tokens_cpu[:n],
            batch.num_prompt_tokens[:n],
        )
        hidden = state.hidden_states[:total]
        if self.mode == client.VERIFY:
            # Re-checks prefill "prompt + output"; decode steps (the one sampled token) are not
            # part of it.
            prefill = [s for s in step if s.phase == client.PREFILL]
            meta, idx, bits = extract.rows(hidden, prefill, self.sender.topk)
        else:
            meta, idx, bits = extract.candidates(hidden, step, self.sender.topk)
        if not meta:
            return
        idx = idx.to("cpu", non_blocking=True)
        bits = bits.to("cpu", non_blocking=True)
        event = None
        if state.hidden_states.is_cuda:
            import torch

            event = torch.cuda.Event()
            event.record()

        def encode():
            if event is not None:
                event.synchronize()
            return [
                client.segment(request, phase, values, pairs)
                for request, phase, values, pairs in extract.frames(meta, idx, bits)
            ]

        self.sender.put(encode)


def _refuse_v2(path: str) -> None:
    try:
        from vllm.v1.worker.gpu import model_runner as v2
    except Exception:  # noqa: BLE001 - a platform without the V2 runner (e.g. no Triton)
        return
    cls = v2.GPUModelRunner
    if getattr(cls, "_agentcoin_refused", False):
        return

    def load_model(self, *args, **kwargs):
        raise PreconditionError(
            "agentcoin toploc: vLLM's V2 model runner is not supported; set "
            f"{V2_ENV}=0 (the plugin is enabled by {path!r})"
        )

    cls.load_model = load_model
    cls._agentcoin_refused = True


def _patch(cls, path: str, mode: int = client.PROVE) -> None:
    if cls.__dict__.get("_agentcoin_patched"):
        return
    if "load_model" in cls.__dict__:
        load = cls.__dict__["load_model"]

        def load_model(self, *args, **kwargs):
            check_config(
                self.model_config, self.cache_config, self.parallel_config, self.speculative_config
            )
            result = load(self, *args, **kwargs)
            if "_agentcoin" not in self.__dict__:
                self._agentcoin = Capture(self, path, mode)
            return result

        cls.load_model = load_model
    if "execute_model" in cls.__dict__:
        execute = cls.__dict__["execute_model"]

        def execute_model(self, scheduler_output, *args, **kwargs):
            capture = self.__dict__.get("_agentcoin")
            if capture is not None:
                _guard(capture, capture.before, scheduler_output)
            result = execute(self, scheduler_output, *args, **kwargs)
            if capture is not None:
                _guard(capture, capture.after, self, scheduler_output)
            return result

        cls.execute_model = execute_model
    cls._agentcoin_patched = True


def _guard(capture: Capture, fn, *args) -> None:
    # A failure of the plugin must never break inference: count it, log it once.
    try:
        fn(*args)
    except Exception:  # noqa: BLE001
        capture.failures += 1
        if capture.failures == 1:
            log.exception("agentcoin toploc: capturing activations failed; proofs will be missing")


def install(path: str, mode: int = client.PROVE) -> None:
    """Checks the vLLM version and patches the model runners (re-entrant)."""
    import vllm

    check_version(vllm.__version__)
    if os.environ.get(V2_ENV, "").strip().lower() in ("1", "true"):
        raise PreconditionError(
            f"agentcoin toploc: vLLM's V2 model runner is not supported; set {V2_ENV}=0"
        )
    from vllm.v1.worker import gpu_model_runner

    _patch(gpu_model_runner.GPUModelRunner, path, mode)
    try:
        from vllm.v1.worker import cpu_model_runner
    except ImportError:
        cpu_model_runner = None
    if cpu_model_runner is not None:
        _patch(cpu_model_runner.CPUModelRunner, path, mode)
    _refuse_v2(path)
