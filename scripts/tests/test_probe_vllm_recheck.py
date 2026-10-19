"""Tests for scripts/probe-vllm-recheck.py (m6-token-binding 1.1): the token IDs of a chat
completion, streamed and not, as vLLM 0.30 sends them with `return_token_ids`."""

import importlib.util
import pathlib
import unittest

SPEC = importlib.util.spec_from_file_location(
    "probe_vllm_recheck", pathlib.Path(__file__).resolve().parent.parent / "probe-vllm-recheck.py"
)
probe = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(probe)


def chunk(content=None, ids=None, finish=None, **top):
    choice = {"index": 0, "delta": {} if content is None else {"content": content},
              "finish_reason": finish}
    if ids is not None:
        choice["token_ids"] = ids
    return {"id": "x", "choices": [choice], **top}


class TokenIdTests(unittest.TestCase):
    def test_prompt_ids_come_from_the_first_chunk_and_output_ids_from_every_delta(self):
        chunks = [
            chunk(ids=[], prompt_token_ids=[1, 2, 3]),
            chunk("Hel", [10]),
            chunk("lo", [11]),
            # A chunk without text still carries its token (vLLM sends it for token IDs).
            chunk(None, [12]),
            chunk("", [2], finish="stop"),
            {"id": "x", "choices": [], "usage": {"prompt_tokens": 3, "completion_tokens": 4}},
        ]
        prompt, output, text, usage = probe.token_ids_of(chunks)
        self.assertEqual(prompt, [1, 2, 3])
        self.assertEqual(output, [10, 11, 12, 2])
        self.assertEqual(text, "Hello")
        self.assertEqual(usage["completion_tokens"], len(output))

    def test_missing_fields(self):
        prompt, output, text, usage = probe.token_ids_of([chunk("a")])
        self.assertIsNone(prompt)
        self.assertEqual(output, [])
        self.assertIsNone(usage)

    def test_a_non_streamed_response_has_the_prompt_at_the_top_and_the_output_in_the_choice(self):
        resp = {
            "id": "x", "prompt_token_ids": [1, 2, 3],
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "Hello"},
                         "finish_reason": "stop", "token_ids": [10, 11, 2]}],
            "usage": {"prompt_tokens": 3, "completion_tokens": 3},
        }
        prompt, output, text, usage = probe.token_ids_of_response(resp)
        self.assertEqual(prompt, [1, 2, 3])
        self.assertEqual(output, [10, 11, 2])
        self.assertEqual(text, "Hello")
        self.assertEqual(usage["completion_tokens"], len(output))

    def test_a_non_streamed_response_without_ids(self):
        resp = {"choices": [{"message": {"content": "a"}}], "usage": {"prompt_tokens": 1}}
        prompt, output, _, _ = probe.token_ids_of_response(resp)
        self.assertIsNone(prompt)
        self.assertIsNone(output)


if __name__ == "__main__":
    unittest.main()
